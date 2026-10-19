//! Reading a scene folder into a [`World`].
//!
//! The reader applies the reference validator's checks (`scene-skate validate`): an input it rejects, this rejects,
//! with the same cause in the message. Not checked here: the GLB contents (the glTF crate's load pass reads them;
//! its primitive count must match the parts) and whether texture files decode.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the scene folder"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component as PathComponent, Path, PathBuf};

use primase_core::{
    Entity, Instance, LodForm, Lods, MaterialVariants, MeshFile, Name, NormalGreen, NormalZ, Priority,
    Source, TextureFile, Tile, Transform, World,
};

use crate::components::*;
use crate::interchange::tables::{self, flags};
use crate::{Error, Result};
use serde_json::{Map, Value};

/// What the reader noticed but did not refuse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    /// The validator's warnings (unused textures, unplaced prototypes, ...).
    pub warnings: Vec<String>,
    /// Things accepted but not kept as written, by what, with counts (an explicit `null` for an optional key is
    /// read as absent and written absent; table bytes a flag says are unused; ...).
    pub tolerated: BTreeMap<String, u64>,
}

impl ReadReport {
    fn tolerate(&mut self, what: &str) {
        *self.tolerated.entry(what.to_owned()).or_default() += 1;
    }
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Scene(msg.into())
}

/// FNV-1a 32 of a string's UTF-16 code units, low byte then high byte (the default prototype `key`).
pub fn fnv32(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for c in s.encode_utf16() {
        h ^= u32::from(c as u8);
        h = h.wrapping_mul(16_777_619);
        h ^= u32::from(c >> 8);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// What a JSON value is, for messages.
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a bool",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The manifest object of one item, handing out its keys and keeping the rest as `other`.
struct Obj<'a> {
    what: String,
    map: &'a Map<String, Value>,
    used: BTreeSet<&'a str>,
}

impl<'a> Obj<'a> {
    fn new(v: &'a Value, what: impl Into<String>) -> Result<Obj<'a>> {
        let what = what.into();
        match v {
            Value::Object(map) => Ok(Obj {
                what,
                map,
                used: BTreeSet::new(),
            }),
            other => Err(bad(format!(
                "{what}: {} where an object was expected",
                kind(other)
            ))),
        }
    }

    /// The value of a key, null included.
    fn raw(&mut self, k: &str) -> Option<&'a Value> {
        let (key, v) = self.map.get_key_value(k)?;
        self.used.insert(key.as_str());
        Some(v)
    }

    /// The value of a key; `null` counts as absent (and is reported).
    fn take(&mut self, k: &str, rep: &mut ReadReport) -> Option<&'a Value> {
        match self.raw(k) {
            Some(Value::Null) => {
                rep.tolerate("explicit null for an optional key (read and written as absent)");
                None
            }
            v => v,
        }
    }

    fn err(&self, msg: impl std::fmt::Display) -> Error {
        bad(format!("{}: {msg}", self.what))
    }

    fn str(&mut self, k: &str, rep: &mut ReadReport) -> Result<Option<String>> {
        match self.take(k, rep) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(v) => Err(self.err(format!("'{k}' is {} where a string was expected", kind(v)))),
        }
    }

    fn bool(&mut self, k: &str, rep: &mut ReadReport) -> Result<Option<bool>> {
        match self.take(k, rep) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(self.err(format!("'{k}' is not true or false"))),
        }
    }

    /// A number read as f64: absent or null is `None`, anything else must be a number.
    fn f64(&mut self, k: &str, rep: &mut ReadReport) -> Result<Option<f64>> {
        match self.take(k, rep) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(Some(n.as_f64().expect("a JSON number has an f64 value"))),
            Some(_) => Err(self.err(format!("'{k}': not a finite number"))),
        }
    }

    /// As [`Obj::f64`], within `[lo, hi]` (the default must be too, so an absent value passes).
    fn f64_in(&mut self, k: &str, lo: f64, hi: f64, rep: &mut ReadReport) -> Result<Option<f64>> {
        let x = self.f64(k, rep)?;
        if let Some(x) = x
            && !(x >= lo && x <= hi)
        {
            return Err(self.err(format!("'{k}' {x} is not in [{lo}, {hi}]")));
        }
        Ok(x)
    }

    fn other(&self) -> Result<Other> {
        Ok(self
            .map
            .iter()
            .filter(|(k, _)| !self.used.contains(k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

/// An f32: the number's f64 value rounded to f32, finite. (The nearest f32 of the decimal text in every case the
/// real-data census has seen: see the census test.)
fn f32_of(v: &Value, what: &str) -> Result<f32> {
    match v {
        Value::Number(n) => match n.as_f64().map(|x| x as f32) {
            Some(x) if x.is_finite() => Ok(x),
            _ => Err(bad(format!("{what}: not finite"))),
        },
        _ => Err(bad(format!("{what}: not a number"))),
    }
}

/// `n` f32s.
fn floats<const N: usize>(v: &Value, what: &str) -> Result<[f32; N]> {
    match v {
        Value::Array(items) if items.len() == N => {
            let mut out = [0.0; N];
            for (o, x) in out.iter_mut().zip(items) {
                *o = f32_of(x, what)?;
            }
            Ok(out)
        }
        _ => Err(bad(format!("{what}: {N} numbers expected"))),
    }
}

/// An integer (written without a fraction or exponent) that fits `T`.
fn int_of<T: TryFrom<i64> + TryFrom<u64>>(v: &Value, what: &str) -> Result<T> {
    let range = |n: &serde_json::Number| bad(format!("{what}: {n} out of range"));
    match v {
        Value::Number(n) if n.is_i64() => T::try_from(n.as_i64().expect("an i64")).map_err(|_| range(n)),
        Value::Number(n) if n.is_u64() => T::try_from(n.as_u64().expect("a u64")).map_err(|_| range(n)),
        _ => Err(bad(format!("{what}: not an integer"))),
    }
}

fn bytes_of<const N: usize>(v: &Value, what: &str, key: &str) -> Result<[u8; N]> {
    let err = || bad(format!("{what}: '{key}' is not {N} integers 0-255"));
    match v {
        Value::Array(items) if items.len() == N => {
            let mut out = [0u8; N];
            for (o, x) in out.iter_mut().zip(items) {
                *o = int_of::<u8>(x, what).map_err(|_| err())?;
            }
            Ok(out)
        }
        _ => Err(err()),
    }
}

fn str_list(v: &Value, what: &str, rep: &mut ReadReport) -> Result<Vec<String>> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => Ok(s.clone()),
                Value::Null => {
                    rep.tolerate("null in a string list (read as \"\")");
                    Ok(String::new())
                }
                _ => Err(bad(format!("{what}: {} in a list of strings", kind(x)))),
            })
            .collect(),
        _ => Err(bad(format!("{what}: not a list"))),
    }
}

fn array<'a>(v: &'a Value, what: &str) -> Result<&'a [Value]> {
    match v {
        Value::Array(items) => Ok(items),
        other => Err(bad(format!("{what}: {} where a list was expected", kind(other)))),
    }
}

/// Resolves file references against the scene folder.
struct Files<'a> {
    dir: &'a Path,
}

impl Files<'_> {
    /// A path inside the folder, refusing links on the way and checking the sha256 when given.
    fn check(&self, rel: &str, sha: Option<&str>, what: &str) -> Result<PathBuf> {
        if rel.is_empty()
            || rel.contains('\\')
            || rel.starts_with('/')
            || rel.as_bytes().get(1) == Some(&b':')
        {
            return Err(bad(format!(
                "{what}: '{rel}' is not a relative path with forward slashes"
            )));
        }
        let mut parts: Vec<&str> = Vec::new();
        for c in Path::new(rel).components() {
            match c {
                PathComponent::Normal(s) => parts.push(s.to_str().expect("from a str")),
                PathComponent::CurDir => {}
                PathComponent::ParentDir => {
                    if parts.pop().is_none() {
                        return Err(bad(format!("{what}: '{rel}' is outside the scene folder")));
                    }
                }
                _ => {
                    return Err(bad(format!(
                        "{what}: '{rel}' is not a relative path with forward slashes"
                    )));
                }
            }
        }
        let mut full = self.dir.to_path_buf();
        for p in &parts {
            full.push(p);
            if std::fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(bad(format!(
                    "{what}: '{rel}' goes through a link ({}): the scene folder must hold its files",
                    parts[..parts.iter().position(|x| std::ptr::eq(x, p)).unwrap_or(0) + 1].join("/")
                )));
            }
        }
        if !full.is_file() {
            return Err(bad(format!("{what}: '{rel}' does not exist")));
        }
        if let Some(want) = sha {
            let got = sha256_file(&full)?;
            if got != want.to_ascii_lowercase() {
                return Err(bad(format!(
                    "{what}: '{rel}' sha256 {got} differs from the manifest's {want}"
                )));
            }
        }
        Ok(full)
    }

    /// A file reference: a string, `{"file": "...", "sha256"?}`, or the same object nested as `"file"`.
    fn file_ref(&self, v: &Value, what: &str, rep: &mut ReadReport) -> Result<(FileRef, PathBuf)> {
        if let Some(n @ Value::Object(_)) = v.get("file") {
            rep.tolerate("nested file reference object (written flat)");
            return self.file_ref(n, what, rep);
        }
        let (rel, sha) = match v {
            Value::String(s) => (s.clone(), None),
            Value::Object(_) => match v.get("file") {
                Some(Value::String(s)) => {
                    let sha = match v.get("sha256") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(h)) => Some(h.clone()),
                        Some(_) => return Err(bad(format!("{what}: sha256 is not a string"))),
                    };
                    (s.clone(), sha)
                }
                _ => return Err(bad(format!("{what}: not a file reference"))),
            },
            _ => return Err(bad(format!("{what}: not a file reference"))),
        };
        let full = self.check(&rel, sha.as_deref(), what)?;
        Ok((
            FileRef {
                path: rel,
                sha256: sha.map(|h| h.to_ascii_lowercase()),
            },
            full,
        ))
    }

    /// One file reference or a list (a table in chunks, read in order); an object's `file` may be the list.
    fn table_ref(&self, v: &Value, what: &str, rep: &mut ReadReport) -> Result<(TableRef, Vec<PathBuf>)> {
        let (items, list, object): (Vec<&Value>, bool, bool) = match v {
            Value::Array(items) => (items.iter().collect(), true, false),
            Value::Object(_) => match v.get("file") {
                Some(Value::Array(items)) => (items.iter().collect(), true, true),
                _ => (vec![v], false, true),
            },
            _ => (vec![v], false, false),
        };
        let mut files = Vec::new();
        let mut paths = Vec::new();
        for (i, x) in items.iter().enumerate() {
            let w = if list {
                format!("{what}[{i}]")
            } else {
                what.to_owned()
            };
            if list && matches!(x, Value::Object(_)) && x.get("sha256").is_none() {
                rep.tolerate("list item {\"file\": ...} without sha256 (written as a string)");
            }
            let (f, p) = self.file_ref(x, &w, rep)?;
            files.push(f);
            paths.push(p);
        }
        Ok((
            TableRef {
                files,
                list,
                object,
                counts: Vec::new(),
            },
            paths,
        ))
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

fn table_err(rel: &str, e: tables::TableError) -> Error {
    Error::Table {
        path: rel.to_owned(),
        message: e.0,
    }
}

/// The soup chunks of a reference read in order as one soup (vertex indices rebased), with per-chunk counts.
fn read_soups(table: &mut TableRef, paths: &[PathBuf], surfaces: usize) -> Result<SkateSoup> {
    let mut soup = SkateSoup::default();
    let mut counts = Vec::new();
    for (f, p) in table.files.iter().zip(paths) {
        let t = tables::read_soup(&read_file(p)?, surfaces).map_err(|e| table_err(&f.path, e))?;
        let base =
            u32::try_from(soup.positions.len()).map_err(|_| bad(format!("{}: too many vertices", f.path)))?;
        counts.push([t.triangles.len() as u32, t.positions.len() as u32]);
        soup.positions.extend(t.positions);
        soup.triangles
            .extend(t.triangles.iter().map(|tri| tri.map(|i| i + base)));
        soup.surfaces.extend(t.surfaces);
    }
    table.counts = counts;
    Ok(soup)
}

const TOP_KEYS: &[&str] = &[
    "format",
    "version",
    "generator",
    "name",
    "display_name",
    "region",
    "surfaces",
    "streaming_distances",
    "texture_budget",
    "min_height",
    "max_render_edge",
    "pause_map",
    "trainer_note",
    "light_prefix",
    "warnings",
    "stats",
    "materials",
    "textures",
    "prototypes",
    "placements",
    "placement_sources",
    "collision",
    "lights",
    "light_groups",
    "light_sources",
    "grind_curves",
    "water",
    "markers",
];

/// Read the scene folder `dir` into `w`. The world must not hold a scene already.
pub fn read(dir: &Path, w: &mut World) -> Result<ReadReport> {
    if w.resource::<SkateScene>().is_some() {
        return Err(bad("the world already holds a skate scene"));
    }
    let path = dir.join("scene.json");
    if !path.is_file() {
        return Err(bad(format!("{}: no scene.json", dir.display())));
    }
    let root: Value = serde_json::from_slice(&read_file(&path)?).map_err(|source| Error::Manifest {
        path: path.clone(),
        source,
    })?;
    let mut rep = ReadReport::default();
    let mut r = Reader {
        files: Files { dir },
        rep: &mut rep,
        w,
        surface_names: Vec::new(),
        forms: Vec::new(),
    };
    r.read(&root)?;
    Ok(rep)
}

struct Reader<'a> {
    files: Files<'a>,
    rep: &'a mut ReadReport,
    w: &'a mut World,
    /// The scene's surfaces by index (its list, or the standard ones).
    surface_names: Vec<String>,
    /// Prototypes' levels of detail, read with the prototypes and spawned after everything else.
    forms: Vec<PendingLods>,
}

impl Reader<'_> {
    fn read(&mut self, root: &Value) -> Result<()> {
        let mut top = Obj::new(root, "scene.json")?;
        let key_order: Vec<String> = top.map.keys().cloned().collect();
        if top.raw("format") != Some(&Value::String("skate-scene".into())) {
            return Err(bad("format is not \"skate-scene\""));
        }
        match top.raw("version") {
            Some(v @ Value::Number(_)) if int_of::<i32>(v, "version").ok() == Some(1) => {}
            _ => return Err(bad("version is not 1")),
        }
        let rep = &mut *self.rep;
        let name = top.str("name", rep)?.ok_or_else(|| bad("no name"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(bad(format!("name '{name}' is not [A-Za-z0-9_-]+")));
        }
        let mut scene = SkateScene {
            name,
            display_name: top.str("display_name", rep)?,
            generator: top.str("generator", rep)?,
            key_order,
            ..SkateScene::default()
        };
        if let Some(reg) = top.take("region", rep) {
            let g: [f32; 4] = floats(reg, "region")?;
            if !(g[0] < g[2] && g[1] < g[3]) {
                return Err(bad("region is empty"));
            }
            scene.region = Some(g);
        }
        // surfaces
        let surfaces = match top.take("surfaces", rep) {
            Some(v) => Some(str_list(v, "surfaces", rep)?),
            None => None,
        };
        let n_surfaces = match &surfaces {
            None => STANDARD_SURFACES.len(),
            Some(list) if list.is_empty() => STANDARD_SURFACES.len(),
            Some(list) => {
                if list.len() > 128 {
                    return Err(bad("surfaces: more than 128"));
                }
                for (i, n) in list.iter().enumerate() {
                    if let Some(p) = n.strip_prefix("packed:") {
                        match p.trim().parse::<i32>() {
                            Ok(v) if v >= 0 && v & 0x3f == 0x20 => {}
                            _ => {
                                return Err(bad(format!(
                                    "surfaces[{i}]: '{n}' is not a Studio packed material (n << 6 | 32)"
                                )));
                            }
                        }
                    } else if !STANDARD_SURFACES.contains(&n.as_str()) {
                        return Err(bad(format!("surfaces[{i}]: unknown surface '{n}'")));
                    }
                }
                list.len()
            }
        };
        self.surface_names = crate::interchange::soup::scene_surfaces(surfaces.as_deref());
        scene.surfaces = surfaces;

        let v_textures = top.take("textures", self.rep);

        let textures = self.read_textures(v_textures)?;
        let v_materials = top.take("materials", self.rep);
        let materials = self.read_materials(v_materials, &textures)?;
        let v_prototypes = top.take("prototypes", self.rep);
        let protos = self.read_prototypes(v_prototypes, &materials, n_surfaces)?;
        let rep = &mut *self.rep;
        let placement_sources = match top.take("placement_sources", rep) {
            Some(v) => Some(str_list(v, "placement_sources", rep)?),
            None => None,
        };
        let v_placements = top.take("placements", self.rep);
        let placed = self.read_placements(
            v_placements,
            &protos,
            placement_sources.as_deref().unwrap_or(&[]),
            &mut scene,
        )?;
        self.check_variants(&protos, &placed)?;
        scene.placement_sources = placement_sources;
        let v_collision = top.take("collision", self.rep);
        self.read_world_soups(v_collision, n_surfaces)?;
        let rep = &mut *self.rep;
        let groups = match top.take("light_groups", rep) {
            Some(v) => Some(str_list(v, "light_groups", rep)?),
            None => None,
        };
        let light_sources = match top.take("light_sources", rep) {
            Some(v) => Some(str_list(v, "light_sources", rep)?),
            None => None,
        };
        let v_lights = top.take("lights", self.rep);
        self.read_lights(
            v_lights,
            groups.as_deref().unwrap_or(&[]),
            light_sources.as_deref().unwrap_or(&[]),
            &mut scene,
        )?;
        scene.light_groups = groups;
        scene.light_sources = light_sources;
        let v_grind_curves = top.take("grind_curves", self.rep);
        self.read_curves(v_grind_curves, &mut scene)?;
        let v_water = top.take("water", self.rep);
        self.read_water(v_water)?;
        let v_markers = top.take("markers", self.rep);
        self.read_markers(v_markers)?;

        // options
        let rep = &mut *self.rep;
        if let Some(v) = top.take("streaming_distances", rep) {
            let mut o = Obj::new(v, "streaming_distances")?;
            scene.streaming_distances = Some(StreamingDistances {
                near: o.f64_in("near", 0.0, 1e6, rep)?,
                medium: o.f64_in("medium", 0.0, 1e6, rep)?,
                far: o.f64_in("far", 0.0, 1e6, rep)?,
                other: o.other()?,
            });
        }
        if let Some(v) = top.take("texture_budget", rep) {
            let mut o = Obj::new(v, "texture_budget")?;
            scene.texture_budget = Some(TextureBudget {
                max: o.f64_in("max", 2.0, 1e7, rep)?,
                aux_weight: o.f64_in("aux_weight", 0.0, 1e6, rep)?,
                other: o.other()?,
            });
        }
        if let Some(v) = top.take("min_height", rep) {
            let mut o = Obj::new(v, "min_height")?;
            scene.min_height = Some(MinHeight {
                height: o.f64_in("height", -1e6, 1e6, rep)?,
                stub_below: o.f64_in("stub_below", 0.0, 1e6, rep)?,
                floor: o.f64_in("floor", -1e6, 1e6, rep)?,
                other: o.other()?,
            });
        }
        scene.max_render_edge = top.f64_in("max_render_edge", 0.0, 1e6, rep)?;
        scene.trainer_note = top.str("trainer_note", rep)?;
        scene.light_prefix = top.str("light_prefix", rep)?;
        if let Some(v) = top.take("warnings", rep) {
            scene.warnings = Some(str_list(v, "warnings", rep)?);
        }
        if let Some(v) = top.take("stats", rep) {
            scene.stats = Some(v.clone());
        }
        if let Some(v) = top.take("pause_map", rep) {
            let mut o = Obj::new(v, "pause_map")?;
            let img = o.raw("image").ok_or_else(|| bad("pause_map: no image"))?;
            let (image, image_file) = self.files.file_ref(img, "pause_map.image", rep)?;
            let bounds = floats(
                o.raw("bounds").ok_or_else(|| bad("pause_map: no bounds"))?,
                "pause_map.bounds",
            )?;
            scene.pause_map = Some(PauseMap {
                image,
                image_file,
                bounds,
                other: o.other()?,
            });
        }
        for k in TOP_KEYS {
            top.raw(k);
        }
        scene.other = top.other()?;
        self.spawn_forms();
        self.w.insert_resource(scene);
        Ok(())
    }

    fn read_textures(
        &mut self,
        v: Option<&Value>,
    ) -> Result<BTreeMap<String, (Entity, Option<&'static str>)>> {
        let mut out = BTreeMap::new();
        let Some(v) = v else { return Ok(out) };
        let Value::Object(kv) = v else {
            return Err(bad("textures: not an object"));
        };
        let mut names: BTreeSet<String> = BTreeSet::new();
        for (key, e) in kv {
            let what = format!("texture '{key}'");
            let rep = &mut *self.rep;
            let (file, path) = self.files.file_ref(e, &what, rep)?;
            if matches!(e, Value::String(_)) {
                rep.tolerate("texture given as a bare file string (written as {\"file\": ...})");
            }
            let tex = self.w.spawn();
            let (name, stem, source_format, source_size, linear, green, z, source, other) = match e {
                Value::Object(_) => {
                    let mut o = Obj::new(e, what.clone())?;
                    o.raw("file");
                    o.raw("sha256");
                    let name = o.str("name", rep)?;
                    let stem = o.str("stem", rep)?;
                    let source_format = o.str("source_format", rep)?;
                    let source_size = match o.take("source_size", rep) {
                        Some(Value::Array(a)) if a.len() == 2 => {
                            Some([int_of(&a[0], &what)?, int_of(&a[1], &what)?])
                        }
                        Some(_) => return Err(bad(format!("{what}: source_size is not [w, h]"))),
                        None => None,
                    };
                    let cs = o.str("color_space", rep)?;
                    let g = o.str("green", rep)?;
                    let zz = o.str("z", rep)?;
                    let linear = match cs.as_deref() {
                        None => None,
                        Some("srgb") => Some(false),
                        Some("linear") => Some(true),
                        Some(_) => None.ok_or_else(|| enum_err(&what))?,
                    };
                    let green = match g.as_deref() {
                        None => None,
                        Some("up") => Some(NormalGreen::Up),
                        Some("down") => Some(NormalGreen::Down),
                        Some(_) => None.ok_or_else(|| enum_err(&what))?,
                    };
                    let z = match zz.as_deref() {
                        None => None,
                        Some("stored") => Some(NormalZ::Stored),
                        Some("rebuild") => Some(NormalZ::Rebuild),
                        Some(_) => None.ok_or_else(|| enum_err(&what))?,
                    };
                    let source = source_string(&mut o, rep);
                    (
                        name,
                        stem,
                        source_format,
                        source_size,
                        linear,
                        green,
                        z,
                        source,
                        o.other()?,
                    )
                }
                _ => (None, None, None, None, None, None, None, None, Other::new()),
            };
            let effective = name.clone().unwrap_or_else(|| {
                Path::new(&file.path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            if !names.insert(effective.to_lowercase()) {
                return Err(bad(format!(
                    "{what}: name '{effective}' is used twice (one name, one image)"
                )));
            }
            self.w.insert(tex, TextureFile { path });
            if let Some(n) = name {
                self.w.insert(tex, Name(n));
            }
            if let Some(s) = source {
                self.w.insert(tex, Source(s));
            }
            self.w.insert(
                tex,
                SkateTexture {
                    key: key.clone(),
                    file,
                    stem,
                    source_format,
                    source_size,
                    linear,
                    green,
                    z,
                    other,
                },
            );
            out.insert(key.clone(), (tex, None));
        }
        Ok(out)
    }

    fn read_materials(
        &mut self,
        v: Option<&Value>,
        textures: &BTreeMap<String, (Entity, Option<&'static str>)>,
    ) -> Result<BTreeMap<String, Entity>> {
        let mut out = BTreeMap::new();
        let mut roles: BTreeMap<String, &'static str> = BTreeMap::new();
        let mut layered = Vec::new();
        if let Some(v) = v {
            let Value::Object(kv) = v else {
                return Err(bad("materials: not an object"));
            };
            let mut lower: BTreeSet<String> = BTreeSet::new();
            for (key, e) in kv {
                if !lower.insert(key.to_lowercase()) {
                    return Err(bad(format!(
                        "material keys '{key}' and another differ only in case"
                    )));
                }
                let what = format!("material '{key}'");
                let rep = &mut *self.rep;
                let mut o = Obj::new(e, what.clone())?;
                let class_s = o.str("class", rep)?;
                let class = class_s
                    .as_deref()
                    .and_then(MaterialClass::parse)
                    .ok_or_else(|| bad(format!("{what}: class '{}' unknown", class_s.unwrap_or_default())))?;
                let mut tex = |o: &mut Obj,
                               k: &str,
                               role: &'static str,
                               rep: &mut ReadReport|
                 -> Result<Option<Entity>> {
                    let Some(tk) = o.str(k, rep)? else { return Ok(None) };
                    let (te, _) = textures
                        .get(&tk)
                        .ok_or_else(|| bad(format!("{what}: {k} texture '{tk}' unknown")))?;
                    if let Some(r) = roles.get(&tk)
                        && *r != role
                    {
                        return Err(bad(format!("texture '{tk}' is used as {r} and as {role}")));
                    }
                    roles.insert(tk, role);
                    Ok(Some(*te))
                };
                let base = tex(&mut o, "base", "color", rep)?;
                let normal = tex(&mut o, "normal", "normal", rep)?;
                let mask = tex(&mut o, "mask", "mask", rep)?;
                let alpha = enum_field(&mut o, "alpha", AlphaMode::parse, &what, rep)?;
                let domain = enum_field(&mut o, "domain", Domain::parse, &what, rep)?;
                let wrap = enum_field(&mut o, "wrap", Wrap::parse, &what, rep)?;
                let base_color = match o.take("base_color", rep) {
                    Some(x) => Some(bytes_of::<4>(x, &what, "base_color")?),
                    None => None,
                };
                let tint = match o.take("tint", rep) {
                    Some(x) => Some(bytes_of::<3>(x, &what, "tint")?),
                    None => None,
                };
                let cutoff = o.f64_in("cutoff", 0.0, 1.0, rep)?;
                let roughness = o.f64_in("roughness", 0.0, 1.0, rep)?;
                let metallic = o.f64_in("metallic", 0.0, 1.0, rep)?;
                let two_sided = o.bool("two_sided", rep)?;
                let emissive = o.bool("emissive", rep)?;
                let vertex_alpha = o.bool("vertex_alpha", rep)?;
                let ocean = o.bool("ocean", rep)?;
                let cast_shadows = o.bool("cast_shadows", rep)?;
                let label = match o.take("label", rep) {
                    None => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(_) => return Err(bad(format!("{what}: label is not a string"))),
                };
                let source = source_string(&mut o, rep);
                let m = self.w.spawn();
                if class == MaterialClass::Layered {
                    let layers = o
                        .take("layers", rep)
                        .cloned()
                        .ok_or_else(|| bad(format!("{what}: missing layers")))?;
                    let weights = o
                        .take("weights", rep)
                        .cloned()
                        .ok_or_else(|| bad(format!("{what}: missing weights")))?;
                    layered.push((m, layers, weights));
                }
                if let Some(l) = label {
                    self.w.insert(m, Name(l));
                }
                if let Some(s) = source {
                    self.w.insert(m, Source(s));
                }
                self.w.insert(
                    m,
                    SkateMaterial {
                        key: key.clone(),
                        class,
                        base,
                        normal,
                        mask,
                        base_color,
                        tint,
                        alpha,
                        cutoff,
                        two_sided,
                        domain,
                        metallic,
                        roughness,
                        wrap,
                        ocean,
                        cast_shadows,
                        emissive,
                        vertex_alpha,
                        other: o.other()?,
                    },
                );
                out.insert(key.clone(), m);
            }
        }
        for (entity, layers, weights) in layered {
            crate::interchange::layered::read(self.w, entity, layers, weights, &out)?;
        }
        for (entity, material) in self.w.iter::<primase_core::LayeredMaterial>() {
            primase_core::Component::verify(material, entity, self.w).map_err(bad)?;
        }
        for (key, _) in textures.iter().filter(|(k, _)| !roles.contains_key(*k)) {
            self.rep.warnings.push(format!("texture '{key}' is not used"));
        }
        Ok(out)
    }

    fn read_prototypes(
        &mut self,
        v: Option<&Value>,
        materials: &BTreeMap<String, Entity>,
        n_surfaces: usize,
    ) -> Result<Vec<Proto>> {
        let mut out = Vec::new();
        let Some(v) = v else { return Ok(out) };
        let mut ids: BTreeSet<String> = BTreeSet::new();
        let mut keys: BTreeMap<u32, String> = BTreeMap::new();
        for e in array(v, "prototypes")? {
            let i = out.len();
            let rep = &mut *self.rep;
            let mut o = Obj::new(e, format!("prototype {i}"))?;
            let id = o
                .str("id", rep)?
                .ok_or_else(|| bad(format!("prototype {i}: no id")))?;
            if !ids.insert(id.clone()) {
                return Err(bad(format!("prototype id '{id}' twice")));
            }
            let what = format!("prototype '{id}'");
            o.what.clone_from(&what);
            let name = o.str("name", rep)?;
            let key = match o.take("key", rep) {
                Some(k) => Some(int_of::<u32>(k, &what)?),
                None => None,
            };
            let eff_key = key.unwrap_or_else(|| fnv32(&id));
            if let Some(other) = keys.get(&eff_key) {
                return Err(bad(format!(
                    "prototypes '{other}' and '{id}' have the same key {eff_key}"
                )));
            }
            keys.insert(eff_key, id.clone());
            let foliage = o.bool("foliage", rep)?;
            let ground = o.bool("ground", rep)?;
            let mesh = match o.take("mesh", rep) {
                Some(m) => {
                    let m = int_of::<u32>(m, &what)?;
                    if m == 0 {
                        rep.tolerate("explicit \"mesh\": 0 (written absent)");
                    }
                    m
                }
                None => 0,
            };
            let glb = match o.raw("glb") {
                None => return Err(bad(format!("{what}: no glb (null for none)"))),
                Some(Value::Null) => None,
                Some(g) => Some(self.files.file_ref(g, &format!("{what} glb"), rep)?),
            };
            let collision = match o.take("collision", rep) {
                None => None,
                Some(c) => Some(self.soup_ref(c, &format!("{what} collision"), false)?),
            };
            let parts_j = o.raw("parts").ok_or_else(|| bad(format!("{what}: no parts")))?;
            let own = self.read_parts(parts_j, &what, materials)?;
            if glb.is_none() && !own.parts.is_empty() {
                return Err(bad(format!(
                    "{what}: parts on a prototype without geometry are not supported"
                )));
            }
            let rep = &mut *self.rep;
            let detail = match o.take("detail", rep) {
                None => 0,
                Some(d) => {
                    let d = int_of::<u8>(d, &format!("{what} detail"))?;
                    if d == 0 {
                        rep.tolerate("explicit \"detail\": 0 (written absent)");
                    }
                    d
                }
            };
            let lods_j = o.take("lods", rep);
            let default_j = o.take("lod_default", rep);
            let mut forms = Vec::new();
            if let Some(lods_j) = lods_j {
                let list = array(lods_j, &format!("{what} lods"))?;
                if glb.is_none() {
                    return Err(bad(format!("{what}: lods without a glb")));
                }
                if list.is_empty() {
                    self.rep.tolerate("explicit empty \"lods\" (written absent)");
                }
                for (k, le) in list.iter().enumerate() {
                    forms.push(self.read_form(le, &format!("{what} lod {k}"), &what, materials)?);
                }
            }
            let rep = &mut *self.rep;
            let default = match default_j {
                None => 0,
                Some(d) => {
                    let n = int_of::<i64>(d, &format!("{what} lod_default"))?;
                    let count = forms.len() + 1;
                    let n = usize::try_from(n)
                        .ok()
                        .filter(|&n| n < count)
                        .ok_or_else(|| bad(format!("{what}: lod_default {n} of {count} forms")))?;
                    if n == 0 {
                        rep.tolerate("explicit \"lod_default\": 0 (written absent)");
                    }
                    n
                }
            };
            let source = source_string(&mut o, rep);
            let other = o.other()?;
            let p = self.w.spawn();
            let soup = match collision {
                Some((mut sref, paths)) => {
                    let soup = read_soups(&mut sref.table, &paths, n_surfaces)
                        .map_err(|e| bad(format!("{what} collision: {e}")))?;
                    crate::interchange::soup::insert_soup(self.w, p, soup, &self.surface_names)
                        .map_err(|e| bad(format!("{what} collision: {e}")))?;
                    Some(sref)
                }
                None => None,
            };
            match &glb {
                Some((_, path)) => own.attach(self.w, p, path, mesh),
                None => {
                    self.w.insert(p, SkateParts(Vec::new()));
                }
            }
            if let Some(n) = name {
                self.w.insert(p, Name(n));
            }
            if let Some(s) = source {
                self.w.insert(p, Source(s));
            }
            // the validator checks the form a writer takes by default
            let (has_glb, checked) = match default.checked_sub(1).map(|k| &forms[k]) {
                None => (glb.is_some(), own),
                Some(f) => (f.mesh.is_some(), f.parts.clone()),
            };
            out.push(Proto {
                entity: p,
                id: id.clone(),
                has_glb,
                materials: checked.materials,
                variants: checked.variants,
            });
            if detail != 0 || !forms.is_empty() {
                self.forms.push(PendingLods {
                    prototype: p,
                    detail,
                    default,
                    glb: glb.as_ref().map(|(_, path)| path.clone()).unwrap_or_default(),
                    forms,
                });
            }
            self.w.insert(
                p,
                SkatePrototype {
                    id,
                    glb: glb.map(|(f, _)| f),
                    key,
                    foliage,
                    ground,
                    collision: soup,
                    other,
                },
            );
        }
        Ok(out)
    }

    /// A mesh's `parts` list: a prototype's (`what`) or one of its forms'.
    fn read_parts(
        &mut self,
        parts_j: &Value,
        what: &str,
        materials: &BTreeMap<String, Entity>,
    ) -> Result<ReadParts> {
        let rep = &mut *self.rep;
        let mut out = ReadParts::default();
        for (pi, pe) in array(parts_j, &format!("{what} parts"))?.iter().enumerate() {
            let mut po = Obj::new(pe, format!("{what} part {pi}"))?;
            let material = match po.raw("material") {
                None | Some(Value::Null) => None,
                Some(Value::String(m)) => Some(
                    *materials
                        .get(m)
                        .ok_or_else(|| bad(format!("{what}: material '{m}' unknown")))?,
                ),
                Some(_) => return Err(bad(format!("{what} part {pi}: material is not a string"))),
            };
            let mut vmap = BTreeMap::new();
            let has_variants = match po.take("variants", rep) {
                None => false,
                Some(Value::Object(vs)) => {
                    for (vk, vv) in vs {
                        let vi: u32 = vk
                            .parse()
                            .ok()
                            .filter(|n: &u32| n.to_string() == *vk)
                            .ok_or_else(|| bad(format!("{what}: variant '{vk}' is not a number")))?;
                        let Value::String(mk) = vv else {
                            return Err(bad(format!("{what}: variant '{vk}' is not a material key")));
                        };
                        let me = materials
                            .get(mk)
                            .ok_or_else(|| bad(format!("{what}: material '{mk}' unknown")))?;
                        if self.w.has::<primase_core::LayeredMaterial>(*me) {
                            return Err(bad(format!(
                                "{what}: layered material '{mk}' is not allowed as a variant"
                            )));
                        }
                        vmap.insert(vi, *me);
                    }
                    true
                }
                Some(_) => return Err(bad(format!("{what} part {pi}: variants is not an object"))),
            };
            let source = po.take("source", rep).cloned();
            out.materials.push(material);
            out.variants.push((has_variants, vmap));
            out.parts.push(Part {
                source,
                variants_key: has_variants,
                other: po.other()?,
            });
        }
        Ok(out)
    }

    /// One `lods` entry (`fwhat`) of prototype `what`: `{"detail": n, "mesh": k or null, "parts": [...]}`.
    fn read_form(
        &mut self,
        le: &Value,
        fwhat: &str,
        what: &str,
        materials: &BTreeMap<String, Entity>,
    ) -> Result<PendingForm> {
        let mut lo = Obj::new(le, fwhat)?;
        let rep = &mut *self.rep;
        let detail = match lo.raw("detail") {
            None => {
                rep.tolerate("lod without \"detail\" (written 0)");
                0
            }
            Some(d) => int_of::<u8>(d, &format!("{fwhat} detail"))?,
        };
        let mesh = match lo.raw("mesh") {
            None => {
                rep.tolerate("lod without \"mesh\" (written null)");
                None
            }
            Some(Value::Null) => None,
            Some(m) => Some(int_of::<u32>(m, &format!("{fwhat} mesh"))?),
        };
        let parts_j = lo.raw("parts").ok_or_else(|| bad(format!("{fwhat}: no parts")))?;
        let parts = self.read_parts(parts_j, what, materials)?;
        if mesh.is_none() && !parts.parts.is_empty() {
            return Err(bad(format!("{what}: a lod without a mesh has parts")));
        }
        Ok(PendingForm {
            detail,
            mesh,
            parts,
            other: lo.other()?,
        })
    }

    /// The prototypes' other forms, each an entity after every other one (so the manifest's other entities keep the
    /// ids they have in a scene without levels of detail), and their `core.lods`.
    fn spawn_forms(&mut self) {
        for l in std::mem::take(&mut self.forms) {
            let mut forms = Vec::new();
            for f in l.forms {
                let e = self.w.spawn();
                match f.mesh {
                    Some(mesh) => f.parts.attach(self.w, e, &l.glb, mesh),
                    None => {
                        self.w.insert(e, SkateParts(Vec::new()));
                    }
                }
                if !f.other.is_empty() {
                    self.w.insert(e, SkateForm { other: f.other });
                }
                forms.push(LodForm {
                    detail: f.detail,
                    form: e,
                });
            }
            self.w.insert(
                l.prototype,
                Lods {
                    detail: l.detail,
                    forms,
                    // the format's default form is 0 when lod_default is absent: an authored default
                    default: Some(l.default),
                },
            );
        }
    }

    /// A soup reference (world `collision[i]` or a prototype's `collision`), its files checked.
    fn soup_ref(&mut self, v: &Value, what: &str, world: bool) -> Result<(SoupRef, Vec<PathBuf>)> {
        let (table, paths) = self.files.table_ref(v, what, self.rep)?;
        let list = table.list;
        let mut sref = SoupRef {
            table,
            mode: None,
            ground: None,
            region_cut: None,
            other: Other::new(),
        };
        if let Value::Object(_) = v {
            let rep = &mut *self.rep;
            let mut o = Obj::new(v, what)?;
            o.raw("file");
            if !list {
                o.raw("sha256");
            }
            sref.mode = match o.str("mode", rep)? {
                None => None,
                Some(m) => Some(CollisionMode::parse(&m).ok_or_else(|| {
                    // the reference words the two places differently
                    match what.strip_suffix(" collision") {
                        Some(proto) => bad(format!("{proto}: collision mode '{m}'")),
                        None => bad(format!("{what}: mode '{m}'")),
                    }
                })?),
            };
            if world {
                sref.ground = o.bool("ground", rep)?;
                sref.region_cut = o.bool("region_cut", rep)?;
            }
            sref.other = o.other()?;
        }
        Ok((sref, paths))
    }

    fn read_placements(
        &mut self,
        v: Option<&Value>,
        protos: &[Proto],
        sources: &[String],
        scene: &mut SkateScene,
    ) -> Result<Vec<(usize, u32)>> {
        let mut used = Vec::new();
        let Some(v) = v else { return Ok(used) };
        let (mut table, paths) = self.files.table_ref(v, "placements", self.rep)?;
        let mut index = 0usize;
        for (f, p) in table.files.clone().iter().zip(&paths) {
            let recs = tables::read_placements(&read_file(p)?).map_err(|e| table_err(&f.path, e))?;
            table.counts.push([recs.len() as u32, 0]);
            for r in recs {
                let i = index;
                index += 1;
                let proto = protos.get(r.proto as usize).ok_or_else(|| {
                    bad(format!(
                        "placement {i}: prototype {} of {}",
                        r.proto,
                        protos.len()
                    ))
                })?;
                if r.source != u32::MAX && r.source as usize >= sources.len() {
                    return Err(bad(format!(
                        "placement {i}: source {} of {}",
                        r.source,
                        sources.len()
                    )));
                }
                let e = self.w.spawn();
                let m = r.m;
                self.w.insert(
                    e,
                    Transform {
                        cols: [
                            [m[0], m[1], m[2]],
                            [m[3], m[4], m[5]],
                            [m[6], m[7], m[8]],
                            [m[9], m[10], m[11]],
                        ],
                    },
                );
                self.w.insert(
                    e,
                    Instance {
                        prototype: proto.entity,
                        variant: r.variant,
                    },
                );
                if r.flags & flags::TILE != 0 {
                    self.w.insert(
                        e,
                        Tile {
                            col: r.tile[0],
                            row: r.tile[1],
                        },
                    );
                } else if r.tile != [0, 0] {
                    self.rep
                        .tolerate("placement tile bytes without the tile flag (written 0)");
                }
                if r.flags & flags::RADIUS == 0 && r.radius.to_bits() != 0 {
                    self.rep
                        .tolerate("placement radius bytes without the radius flag (written 0)");
                }
                self.w.insert(e, Priority(((r.flags >> 8) & 15) as u8));
                let mut unknown = r.flags & !(63 | 0xF00);
                if r.flags & flags::CLIP != 0 && r.flags & flags::CLIP_GIVEN == 0 {
                    unknown |= flags::CLIP;
                }
                self.w.insert(
                    e,
                    SkatePlacement {
                        radius: (r.flags & flags::RADIUS != 0).then_some(r.radius),
                        clip: (r.flags & flags::CLIP_GIVEN != 0).then_some(r.flags & flags::CLIP != 0),
                        no_collision: r.flags & flags::NO_COLLISION != 0,
                        split_by_tile: r.flags & flags::SPLIT != 0,
                        unknown_flags: unknown,
                    },
                );
                if r.source != u32::MAX {
                    self.w.insert(e, Source(sources[r.source as usize].clone()));
                }
                used.push((r.proto as usize, r.variant));
            }
        }
        scene.placements = Some(table);
        Ok(used)
    }

    fn check_variants(&mut self, protos: &[Proto], placed: &[(usize, u32)]) -> Result<()> {
        let used: BTreeSet<(usize, u32)> = placed.iter().copied().collect();
        let mut is_placed = vec![false; protos.len()];
        for &(pi, v) in &used {
            is_placed[pi] = true;
            let p = &protos[pi];
            if !p.has_glb {
                continue;
            }
            let has_variants = p.variants.iter().any(|(h, _)| *h);
            for (i, (mat, (hv, vm))) in p.materials.iter().zip(&p.variants).enumerate() {
                let chosen = if has_variants && *hv {
                    vm.get(&v).copied().or(*mat)
                } else {
                    *mat
                };
                if chosen.is_none() {
                    return Err(bad(format!(
                        "prototype '{}' part {i}: no material for variant {}",
                        p.id,
                        if has_variants { v } else { 0 }
                    )));
                }
            }
        }
        for (p, placed) in protos.iter().zip(is_placed) {
            if !placed {
                self.rep
                    .warnings
                    .push(format!("prototype '{}' is not placed", p.id));
            } else if !p.has_glb {
                self.rep.warnings.push(format!(
                    "prototype '{}' has no geometry; its placements draw nothing",
                    p.id
                ));
            }
        }
        Ok(())
    }

    fn read_world_soups(&mut self, v: Option<&Value>, n_surfaces: usize) -> Result<()> {
        let Some(v) = v else { return Ok(()) };
        for (i, e) in array(v, "collision")?.iter().enumerate() {
            let what = format!("collision[{i}]");
            let (mut sref, paths) = self.soup_ref(e, &what, true)?;
            let soup = read_soups(&mut sref.table, &paths, n_surfaces)?;
            let ent = self.w.spawn();
            crate::interchange::soup::insert_soup(self.w, ent, soup, &self.surface_names)
                .map_err(|e| bad(format!("{what}: {e}")))?;
            self.w.insert(ent, SkateCollision(sref));
        }
        Ok(())
    }

    fn read_lights(
        &mut self,
        v: Option<&Value>,
        groups: &[String],
        sources: &[String],
        scene: &mut SkateScene,
    ) -> Result<()> {
        let Some(v) = v else { return Ok(()) };
        let (mut table, paths) = self.files.table_ref(v, "lights", self.rep)?;
        let mut i = 0usize;
        for (f, p) in table.files.clone().iter().zip(&paths) {
            let recs = tables::read_lights(&read_file(p)?).map_err(|e| table_err(&f.path, e))?;
            table.counts.push([recs.len() as u32, 0]);
            for l in recs {
                if l.kind > 1 {
                    return Err(bad(format!("light {i}: type {}", l.kind)));
                }
                let bad_group = if groups.is_empty() {
                    l.group != 0
                } else {
                    l.group as usize >= groups.len()
                };
                if bad_group {
                    return Err(bad(format!("light {i}: group {} of {}", l.group, groups.len())));
                }
                if l.source != u32::MAX && l.source as usize >= sources.len() {
                    return Err(bad(format!(
                        "light {i}: source {} of {}",
                        l.source,
                        sources.len()
                    )));
                }
                if l.flags & flags::PRIORITY == 0 && l.priority.to_bits() != 0 {
                    self.rep
                        .tolerate("light priority bytes without the priority flag (written 0)");
                }
                let e = self.w.spawn();
                self.w.insert(
                    e,
                    SkateLight {
                        spot: l.kind == 1,
                        position: l.position,
                        direction: l.direction,
                        tangent: l.tangent,
                        color: l.color,
                        energy: l.energy,
                        range: l.range,
                        radius: l.radius,
                        spot_size: l.spot_size,
                        spot_blend: l.spot_blend,
                        time_of_day: l.time_of_day,
                        cast_shadow: l.flags & flags::CAST_SHADOW != 0,
                        priority: (l.flags & flags::PRIORITY != 0).then_some(l.priority),
                        group: if groups.is_empty() {
                            String::new()
                        } else {
                            groups[l.group as usize].clone()
                        },
                        unknown_flags: l.flags & !3,
                    },
                );
                if l.source != u32::MAX {
                    self.w.insert(e, Source(sources[l.source as usize].clone()));
                }
                i += 1;
            }
        }
        scene.lights = Some(table);
        Ok(())
    }

    fn read_curves(&mut self, v: Option<&Value>, scene: &mut SkateScene) -> Result<()> {
        let Some(v) = v else { return Ok(()) };
        let (mut table, paths) = self.files.table_ref(v, "grind_curves", self.rep)?;
        for (f, p) in table.files.clone().iter().zip(&paths) {
            let t = tables::read_curves(&read_file(p)?).map_err(|e| table_err(&f.path, e))?;
            table.counts.push([t.curves.len() as u32, t.points.len() as u32]);
            if t.reserved != 0 {
                self.rep.tolerate("CURV reserved word not 0 (written 0)");
            }
            let mut next = 0u32;
            for c in &t.curves {
                if c.first != next {
                    self.rep
                        .tolerate("curve points not stored in curve order (re-packed on write)");
                }
                next = c.first + c.count;
                let e = self.w.spawn();
                let curve = SkateGrindCurve {
                    points: t.points[c.first as usize..(c.first + c.count) as usize].to_vec(),
                    closed: c.flags & flags::CLOSED != 0,
                    kind: ((c.flags >> 4) & 15) as u8,
                    radius: c.radius,
                    surface: c.surface,
                    unknown_flags: c.flags & !0xF1,
                };
                self.w.insert(e, curve.core());
                self.w.insert(e, curve);
            }
            if next as usize != t.points.len() {
                self.rep
                    .tolerate("CURV points no curve uses after the last (dropped on write)");
            }
        }
        scene.grind_curves = Some(table);
        Ok(())
    }

    fn read_water(&mut self, v: Option<&Value>) -> Result<()> {
        let Some(v) = v else { return Ok(()) };
        for (i, e) in array(v, "water")?.iter().enumerate() {
            let what = format!("water[{i}]");
            let rep = &mut *self.rep;
            let mut o = Obj::new(e, what.clone())?;
            let cj = o
                .raw("corners")
                .ok_or_else(|| bad(format!("{what}: no corners")))?;
            let corners: Vec<[f32; 2]> = array(cj, &what)?
                .iter()
                .map(|c| match c {
                    Value::Array(a) if a.len() == 2 => Ok([
                        f32_of(&a[0], &format!("{what} corner"))?,
                        f32_of(&a[1], &format!("{what} corner"))?,
                    ]),
                    _ => Err(bad(format!("{what}: a corner is not [east, north]"))),
                })
                .collect::<Result<_>>()?;
            if corners.len() < 3 {
                return Err(bad(format!("{what}: fewer than 3 corners")));
            }
            let (mut pos, mut neg) = (false, false);
            let n = corners.len();
            for k in 0..n {
                let (a, b, c) = (corners[k], corners[(k + 1) % n], corners[(k + 2) % n]);
                // f32 arithmetic, as the reference reader computes it
                let cr = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
                if cr > 0.0 {
                    pos = true;
                } else if cr < 0.0 {
                    neg = true;
                }
            }
            if pos && neg {
                return Err(bad(format!("{what}: the outline is not convex")));
            }
            let height = o.f64("height", rep).map_err(|e| bad(format!("{what}: {e}")))?;
            let ent = self.w.spawn();
            self.w.insert(
                ent,
                SkateWater {
                    corners,
                    height,
                    other: o.other()?,
                },
            );
        }
        Ok(())
    }

    fn read_markers(&mut self, v: Option<&Value>) -> Result<()> {
        let Some(v) = v else { return Ok(()) };
        let mut travel = 0;
        for (i, e) in array(v, "markers")?.iter().enumerate() {
            let what = format!("markers[{i}]");
            let rep = &mut *self.rep;
            let mut o = Obj::new(e, what.clone())?;
            let kind_s = o.str("kind", rep)?;
            let kind = kind_s
                .as_deref()
                .and_then(MarkerKind::parse)
                .ok_or_else(|| bad(format!("{what}: kind '{}'", kind_s.unwrap_or_default())))?;
            if kind == MarkerKind::Travel {
                travel += 1;
            }
            let name = o.str("name", rep)?;
            let position = match o.take("position", rep) {
                None => None,
                Some(Value::Array(p)) if p.len() == 3 => {
                    let x = f32_of(&p[0], &format!("{what} position"))?;
                    let z = f32_of(&p[2], &format!("{what} position"))?;
                    let y = match &p[1] {
                        Value::Null => None,
                        y => Some(f32_of(y, &format!("{what} position"))?),
                    };
                    Some(([x, z], y))
                }
                Some(_) => return Err(bad(format!("{what}: position needs 3 numbers"))),
            };
            let m = match o.take("m", rep) {
                None => None,
                Some(mj) => {
                    let t: [f32; 12] = floats(mj, &format!("{what}.m"))?;
                    // f32 arithmetic, as the reference reader computes it
                    let det = t[0] * (t[4] * t[8] - t[5] * t[7]) - t[3] * (t[1] * t[8] - t[2] * t[7])
                        + t[6] * (t[1] * t[5] - t[2] * t[4]);
                    // NaN fails too, as in the reference
                    if f64::from(det).partial_cmp(&1e-8) != Some(std::cmp::Ordering::Greater) {
                        return Err(bad(format!(
                            "{what}.m: a mirrored or zero-scale basis (determinant {det})"
                        )));
                    }
                    Some(t)
                }
            };
            if position.is_none() && m.is_none() {
                return Err(bad(format!("{what}: no position")));
            }
            let yaw = o.f64("yaw", rep).map_err(|e| bad(format!("{what}: {e}")))?;
            let lift = o.f64("lift", rep).map_err(|e| bad(format!("{what}: {e}")))?;
            let shelter = o.bool("shelter", rep)?;
            let trainer = o.bool("trainer", rep)?;
            let blueprint = o.str("blueprint", rep)?;
            if matches!(kind, MarkerKind::Vfx | MarkerKind::TriggerEffect)
                && blueprint.as_deref().is_none_or(|b| b.trim().is_empty())
            {
                return Err(bad(format!("{what}: an effect needs a blueprint")));
            }
            // a height comes from the position, or from an effect matrix
            let no_height = position.is_some_and(|(_, y)| y.is_none()) && m.is_none();
            let snap = match o.take("snap", rep) {
                None => None,
                Some(s) => Some(read_snap(s, no_height, &what, rep)?),
            };
            let other = o.other()?;
            let ent = self.w.spawn();
            if let Some(n) = name {
                self.w.insert(ent, Name(n));
            }
            let marker = SkateMarker {
                kind,
                position,
                yaw,
                m,
                blueprint,
                snap,
                lift,
                shelter,
                trainer,
                other,
            };
            let (core, transform) = marker.core();
            self.w.insert(ent, core);
            if let Some(t) = transform {
                self.w.insert(ent, t);
            }
            self.w.insert(ent, marker);
        }
        if travel > 25 {
            self.rep
                .warnings
                .push(format!("{travel} travel points: Studio takes at most 25"));
        }
        Ok(())
    }
}

fn read_snap(s: &Value, no_height: bool, what: &str, rep: &mut ReadReport) -> Result<Snap> {
    let mode_of = |m: Option<&str>| {
        m.and_then(SnapMode::parse)
            .ok_or_else(|| bad(format!("{what}: snap mode '{}'", m.unwrap_or_default())))
    };
    let snap = match s {
        Value::String(m) => Snap::Mode(mode_of(Some(m))?),
        Value::Object(_) => {
            let w = format!("{what}.snap");
            let mut o = Obj::new(s, w.clone())?;
            let mode = mode_of(o.str("mode", rep)?.as_deref())?;
            let above = o.f64_in("above", 0.0, 1e6, rep)?;
            let min_up = o.f64_in("min_up", -1.0, 1.0, rep)?;
            let directions = o.f64_in("directions", 1.0, 360.0, rep)?;
            let max_above_ground = o.f64_in("max_above_ground", 0.0, 1e6, rep)?;
            let headroom = o.f64_in("headroom", 0.0, 1e6, rep)?;
            let from = o.str("from", rep)?;
            let rings = match o.take("rings", rep) {
                None => None,
                Some(Value::Array(rs)) => Some(
                    rs.iter()
                        .map(|r| match r {
                            Value::Number(n) if n.as_f64().is_some_and(|x| x > 0.0 && x < 1e6) => {
                                f32_of(r, &w)
                            }
                            _ => Err(bad(format!("{w}: rings must be positive distances"))),
                        })
                        .collect::<Result<Vec<f32>>>()?,
                ),
                Some(_) => return Err(bad(format!("{w}: rings must be positive distances"))),
            };
            Snap::Object {
                mode,
                above,
                from,
                min_up,
                rings,
                directions,
                max_above_ground,
                headroom,
                other: o.other()?,
            }
        }
        _ => return Err(bad(format!("{what}: snap mode ''"))),
    };
    let mode = match &snap {
        Snap::Mode(m) | Snap::Object { mode: m, .. } => *m,
    };
    if mode == SnapMode::None && no_height {
        return Err(bad(format!("{what}: snap none needs a height")));
    }
    Ok(snap)
}

fn enum_err(what: &str) -> Error {
    bad(format!(
        "{what}: color_space / green / z not one of srgb|linear, up|down, stored|rebuild"
    ))
}

fn enum_field<T>(
    o: &mut Obj,
    k: &str,
    parse: fn(&str) -> Option<T>,
    what: &str,
    rep: &mut ReadReport,
) -> Result<Option<T>> {
    match o.str(k, rep)? {
        None => Ok(None),
        Some(s) => parse(&s)
            .map(Some)
            .ok_or_else(|| bad(format!("{what}: {k} '{s}'"))),
    }
}

/// A string `source` becomes `core.source`; another value stays in `other`.
fn source_string(o: &mut Obj, _rep: &mut ReadReport) -> Option<String> {
    match o.map.get_key_value("source") {
        Some((k, Value::String(s))) => {
            o.used.insert(k.as_str());
            Some(s.clone())
        }
        _ => None,
    }
}

/// A prototype as the rest of the reader needs it.
/// What the variant check needs of a prototype: the form a writer takes by default (`lod_default`).
struct Proto {
    entity: Entity,
    id: String,
    has_glb: bool,
    materials: Vec<Option<Entity>>,
    variants: Vec<(bool, BTreeMap<u32, Entity>)>,
}

/// A `parts` list as read: the parts, each one's material, and its variants (whether it has a `variants` object).
#[derive(Clone, Default)]
struct ReadParts {
    parts: Vec<Part>,
    materials: Vec<Option<Entity>>,
    variants: Vec<(bool, BTreeMap<u32, Entity>)>,
}

impl ReadParts {
    /// The parts as mesh `mesh` of the GLB at `path`, on `e`: `skate.parts`, `core.mesh_file` and, when a part has
    /// variants, `core.material_variants`.
    fn attach(&self, w: &mut World, e: Entity, path: &Path, mesh: u32) {
        w.insert(
            e,
            MeshFile {
                path: path.to_owned(),
                mesh,
                materials: self.materials.clone(),
            },
        );
        if self.variants.iter().any(|(h, _)| *h) {
            w.insert(
                e,
                MaterialVariants(self.variants.iter().map(|(_, m)| m.clone()).collect()),
            );
        }
        w.insert(e, SkateParts(self.parts.clone()));
    }
}

/// A prototype's levels of detail as read, its forms not spawned yet.
struct PendingLods {
    prototype: Entity,
    detail: u8,
    default: usize,
    /// The prototype's GLB, every form's mesh file.
    glb: PathBuf,
    forms: Vec<PendingForm>,
}

/// One `lods` entry as read.
struct PendingForm {
    detail: u8,
    mesh: Option<u32>,
    parts: ReadParts,
    other: Other,
}

/// What [`load`] read: the prototypes' meshes, the textures, and the scene's own pause-map picture.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub meshes: primase_gltf::LoadReport,
    pub textures: primase_image::TextureLoadReport,
    /// Whether the scene names a pause-map picture, read into [`crate::passes::SkatePauseImage`].
    pub pause_image: bool,
}

/// Read every file a scene read by [`read`] references into the world, each file once: each `core.mesh_file` as
/// `core.mesh` (the glTF crate's GLB reader; the mesh's materials from the file reference), each `core.texture_file`
/// without pixels as `core.texture` (the image toolkit's PNG and DDS decoding), paths relative to the scene folder
/// `dir`, and the scene's pause-map picture (`pause_map`, decoded to RGBA8) as [`crate::components::SkatePauseImage`]
/// for `prepare-reskate-pause-map`. Each payload keeps the bytes it was read from and their fingerprint
/// ([`crate::components::LoadedFile`]), so a writer passes an unchanged payload through without reopening the file;
/// the file references stay, as the payloads' origin. Everything is read and checked before the world changes.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the scene's GLBs, textures and pause-map picture"
)]
pub fn load(dir: &Path, world: &mut primase_core::World) -> Result<LoadReport> {
    use crate::components::LoadedFile;
    use crate::components::loaded::{image_fingerprint, mesh_fingerprint, texture_fingerprint};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let read_bytes = |path: &Path| -> Result<Arc<[u8]>> {
        std::fs::read(path)
            .map(Arc::from)
            .map_err(|source| crate::Error::Io {
                path: path.to_owned(),
                source,
            })
    };
    let mut report = LoadReport::default();
    // textures without pixels
    let jobs: Vec<(primase_core::Entity, std::path::PathBuf)> = world
        .iter::<TextureFile>()
        .filter(|(e, _)| !world.has::<primase_core::Texture>(*e))
        .map(|(e, f)| (e, dir.join(&f.path)))
        .collect();
    let mut textures: BTreeMap<std::path::PathBuf, (primase_core::Texture, LoadedFile)> = BTreeMap::new();
    let mut loaded_textures = Vec::new();
    for (e, path) in jobs {
        if !textures.contains_key(&path) {
            let bytes = read_bytes(&path)?;
            let t = primase_image::decode_texture_file(
                &bytes,
                &path.display().to_string(),
                &mut report.textures,
            )?;
            let file = LoadedFile {
                decoded: texture_fingerprint(&t),
                bytes,
            };
            textures.insert(path.clone(), (t, file));
            report.textures.files += 1;
        }
        loaded_textures.push((e, textures[&path].clone()));
        report.textures.textures += 1;
    }
    // every mesh file, each GLB parsed once
    let jobs: Vec<(primase_core::Entity, MeshFile)> =
        world.iter::<MeshFile>().map(|(e, f)| (e, f.clone())).collect();
    let mut glbs: BTreeMap<std::path::PathBuf, (primase_gltf::Gltf, Arc<[u8]>)> = BTreeMap::new();
    let mut loaded_meshes = Vec::new();
    for (e, mf) in jobs {
        let path = dir.join(&mf.path);
        if !glbs.contains_key(&path) {
            let bytes = read_bytes(&path)?;
            let gltf = primase_gltf::Gltf::from_glb(&bytes)
                .map_err(|err| crate::Error::Scene(format!("{}: {err}", path.display())))?;
            glbs.insert(path.clone(), (gltf, bytes));
            report.meshes.files += 1;
        }
        let (gltf, bytes) = &glbs[&path];
        let read = gltf
            .read_mesh(mf.mesh as usize)
            .map_err(|err| crate::Error::Scene(format!("{}: {err}", path.display())))?;
        if read.primitives.len() != mf.materials.len() {
            return Err(crate::Error::Scene(format!(
                "{e:?}: {} mesh {} has {} primitives, core.mesh_file lists {} materials",
                path.display(),
                mf.mesh,
                read.primitives.len(),
                mf.materials.len()
            )));
        }
        let mut mesh = read.to_mesh();
        for (mut p, m) in mesh.primitives_mut().zip(&mf.materials) {
            *p.material_mut() = *m;
        }
        report.meshes.primitives += mesh.primitives().len() as u64;
        report.meshes.meshes += 1;
        let file = LoadedFile {
            bytes: bytes.clone(),
            decoded: mesh_fingerprint(&mesh),
        };
        loaded_meshes.push((e, mesh, file));
    }
    // the scene's own pause-map picture
    let image_file = world
        .resource::<crate::components::SkateScene>()
        .and_then(|s| s.pause_map.as_ref())
        .map(|p| p.image_file.clone());
    let pause = match image_file {
        Some(file) => {
            let bytes = read_bytes(&file)?;
            let src = crate::texture::TexSource::from_file_bytes(
                &file.display().to_string(),
                bytes.to_vec(),
                None,
                None,
            )?;
            let rgba = src
                .decode(0, src.width, src.height)?
                .ok_or_else(|| crate::Error::Scene("pause_map: the image cannot be decoded".into()))?;
            Some(crate::components::SkatePauseImage {
                width: src.width,
                height: src.height,
                file: Some(LoadedFile {
                    decoded: image_fingerprint(src.width, src.height, &rgba),
                    bytes,
                }),
                rgba,
            })
        }
        None => None,
    };
    for (e, (t, file)) in loaded_textures {
        world.insert(e, t);
        world.insert(e, file);
    }
    for (e, mesh, file) in loaded_meshes {
        world.insert(e, mesh);
        world.insert(e, file);
    }
    if let Some(p) = pause {
        world.insert_resource(p);
        report.pause_image = true;
    }
    Ok(report)
}
