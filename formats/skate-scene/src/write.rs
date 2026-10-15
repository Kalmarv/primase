//! Writing a world back as a scene folder: `scene.json`, the binary tables, and the referenced files copied byte
//! for byte (textures, prototype GLBs whose geometry is unchanged, the pause map).
//!
//! The manifest keeps the top-level key order the reader saw (and writes lists that were present but are empty);
//! inside an item keys come in the specification's order, then the item's other keys in theirs. Floats are written
//! as their shortest round-trip text for the type the field is read as. String lists (`placement_sources`,
//! `light_groups`, `light_sources`) keep the order they were read in, new strings appended, so an unchanged world
//! gives the same table bytes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component as PathComponent, Path, PathBuf};

use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, MeshFile, Name, NormalGreen, NormalZ, Source, TextureFile,
    Tile, Transform, World,
};

use crate::components::*;
use crate::json::{self, J};
use crate::read::sha256_file;
use crate::tables::{self, CurveRec, CurveTable, LightRec, PlacementRec, SoupTable, flags};
use crate::{Error, Result};

/// What the writer did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    /// Table files written (relative paths).
    pub tables: Vec<String>,
    /// Files copied (relative paths), and their bytes.
    pub copied: Vec<String>,
    pub copied_bytes: u64,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Write(msg.into())
}

/// What goes at one output path.
enum Out {
    Bytes(Vec<u8>),
    Copy(PathBuf),
}

/// A relative path as path parts under the output folder; refuses paths that leave it.
fn parts_of(rel: &str) -> Result<Vec<String>> {
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(rel).components() {
        match c {
            PathComponent::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            PathComponent::CurDir => {}
            PathComponent::ParentDir if parts.pop().is_some() => {}
            _ => return Err(bad(format!("'{rel}' is not a path inside the scene folder"))),
        }
    }
    if parts.is_empty() || rel.contains('\\') {
        return Err(bad(format!(
            "'{rel}' is not a relative path with forward slashes"
        )));
    }
    Ok(parts)
}

struct Plan {
    /// Lower-cased normalised path -> (path as written, content); case-insensitive like the file systems we write to.
    files: BTreeMap<String, (String, Out)>,
}

impl Plan {
    fn add(&mut self, rel: &str, out: Out) -> Result<()> {
        let key = parts_of(rel)?.join("/").to_lowercase();
        match self.files.get(&key) {
            None => {
                self.files.insert(key, (rel.to_owned(), out));
                Ok(())
            }
            Some((_, Out::Copy(a))) if matches!(&out, Out::Copy(b) if b == a) => Ok(()),
            Some(_) => Err(bad(format!("two different files would be written to '{rel}'"))),
        }
    }
}

/// Write `w` as a scene folder at `out`, which must not exist or be empty.
pub fn write(w: &World, out: &Path) -> Result<WriteReport> {
    if out.exists() {
        let mut entries = std::fs::read_dir(out).map_err(|source| Error::Io {
            path: out.to_owned(),
            source,
        })?;
        if entries.next().is_some() {
            return Err(bad(format!("{} is not empty", out.display())));
        }
    }
    let scene = w
        .resource::<SkateScene>()
        .ok_or_else(|| bad("the world has no skate.scene resource"))?;
    let mut wr = Writer {
        w,
        scene,
        plan: Plan {
            files: BTreeMap::new(),
        },
    };
    let manifest = wr.manifest()?;
    let mut report = WriteReport::default();
    for (rel, content) in wr.plan.files.values() {
        let mut path = out.to_path_buf();
        for p in parts_of(rel)? {
            path.push(p);
        }
        let parent = path.parent().expect("under out");
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_owned(),
            source,
        })?;
        match content {
            Out::Bytes(b) => {
                std::fs::write(&path, b).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                report.tables.push(rel.clone());
            }
            Out::Copy(src) => {
                let n = std::fs::copy(src, &path).map_err(|source| Error::Io {
                    path: src.clone(),
                    source,
                })?;
                report.copied.push(rel.clone());
                report.copied_bytes += n;
            }
        }
    }
    let path = out.join("scene.json");
    std::fs::write(&path, json::write(&manifest)).map_err(|source| Error::Io { path, source })?;
    Ok(report)
}

struct Writer<'a> {
    w: &'a World,
    scene: &'a SkateScene,
    plan: Plan,
}

/// Builds a JSON object in order.
#[derive(Default)]
struct ObjB(Vec<(String, J)>);

impl ObjB {
    fn put(&mut self, k: &str, v: J) {
        self.0.push((k.to_owned(), v));
    }
    fn opt<T>(&mut self, k: &str, v: Option<T>, f: impl FnOnce(T) -> J) {
        if let Some(v) = v {
            self.put(k, f(v));
        }
    }
    fn other(&mut self, other: &Other) {
        for (k, v) in other {
            self.put(k, json::from_value(v));
        }
    }
    fn done(self) -> J {
        J::Obj(self.0)
    }
}

fn f32s(v: &[f32]) -> J {
    J::Arr(v.iter().map(|x| J::f32(*x)).collect())
}

/// A string list that keeps the order it was read in and appends new strings.
struct StrTable {
    list: Vec<String>,
    index: HashMap<String, u32>,
    appended: bool,
}

impl StrTable {
    fn new(read: Option<&Vec<String>>) -> StrTable {
        let list = read.cloned().unwrap_or_default();
        let mut index = HashMap::new();
        for (i, s) in list.iter().enumerate() {
            index.entry(s.clone()).or_insert(i as u32);
        }
        StrTable {
            list,
            index,
            appended: false,
        }
    }
    fn get(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.to_owned());
        self.index.insert(s.to_owned(), i);
        self.appended = true;
        i
    }
}

impl Writer<'_> {
    fn copy(&mut self, f: &FileRef, src: &Path) -> Result<FileRef> {
        self.plan.add(&f.path, Out::Copy(src.to_owned()))?;
        Ok(FileRef {
            path: f.path.clone(),
            sha256: match &f.sha256 {
                Some(_) => Some(sha256_file(src)?),
                None => None,
            },
        })
    }

    /// The reference as written: a string, `{"file", "sha256"}`, a list, or an object around them.
    fn table_ref(&self, t: &TableRef, fields: Vec<(String, J)>) -> J {
        let item = |f: &FileRef| match &f.sha256 {
            None => J::str(&f.path),
            Some(h) => J::Obj(vec![
                ("file".into(), J::str(&f.path)),
                ("sha256".into(), J::str(h)),
            ]),
        };
        let mut o = ObjB::default();
        if t.list {
            let inner = J::Arr(t.files.iter().map(item).collect());
            if !t.object && fields.is_empty() {
                return inner;
            }
            o.put("file", inner);
        } else {
            let f = &t.files[0];
            if !t.object && fields.is_empty() && f.sha256.is_none() {
                return J::str(&f.path);
            }
            o.put("file", J::str(&f.path));
            o.opt("sha256", f.sha256.as_ref(), J::str);
        }
        o.0.extend(fields);
        o.done()
    }

    /// Split `total` records into the files of a reference by the counts it was read with, or `None` when they no
    /// longer add up (then everything goes into the first file).
    fn split(t: &TableRef, total: usize) -> Option<Vec<usize>> {
        (t.counts.len() == t.files.len() && t.counts.iter().map(|c| c[0] as usize).sum::<usize>() == total)
            .then(|| t.counts.iter().map(|c| c[0] as usize).collect())
    }

    /// The reference whole into its first file.
    fn whole(t: &TableRef) -> TableRef {
        TableRef {
            files: vec![t.files[0].clone()],
            list: t.list,
            object: t.object,
            counts: Vec::new(),
        }
    }

    /// Plan the table chunks; returns the reference as written.
    fn tables(&mut self, t: &TableRef, chunks: Vec<Vec<u8>>) -> Result<TableRef> {
        let mut out = t.clone();
        for (f, bytes) in out.files.iter_mut().zip(chunks) {
            if f.sha256.is_some() {
                use sha2::{Digest, Sha256};
                f.sha256 = Some(
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                );
            }
            self.plan.add(&f.path, Out::Bytes(bytes))?;
        }
        Ok(out)
    }

    fn soup_tables(&mut self, r: &SoupRef, soup: &SkateSoup, what: &str) -> Result<J> {
        if soup.triangles.len() != soup.surfaces.len() {
            return Err(bad(format!(
                "{what}: {} triangles, {} surface bytes",
                soup.triangles.len(),
                soup.surfaces.len()
            )));
        }
        if r.table.files.is_empty() {
            return Err(bad(format!("{what}: a soup reference without files")));
        }
        // the chunks as read, while every chunk's triangles still index only its own vertices
        let mut chunks = None;
        if let Some(tris) = Self::split(&r.table, soup.triangles.len()) {
            let verts: Vec<usize> = r.table.counts.iter().map(|c| c[1] as usize).collect();
            if verts.iter().sum::<usize>() == soup.positions.len() {
                let (mut t0, mut v0) = (0, 0);
                let mut out = Vec::new();
                for (&nt, &nv) in tris.iter().zip(&verts) {
                    let ok = soup.triangles[t0..t0 + nt]
                        .iter()
                        .all(|t| t.iter().all(|&i| (i as usize) >= v0 && (i as usize) < v0 + nv));
                    if !ok {
                        out.clear();
                        break;
                    }
                    out.push(tables::write_soup(&SoupTable {
                        positions: soup.positions[v0..v0 + nv].to_vec(),
                        triangles: soup.triangles[t0..t0 + nt]
                            .iter()
                            .map(|t| t.map(|i| i - v0 as u32))
                            .collect(),
                        surfaces: soup.surfaces[t0..t0 + nt].to_vec(),
                        reserved: 0,
                    }));
                    t0 += nt;
                    v0 += nv;
                }
                if out.len() == tris.len() {
                    chunks = Some(out);
                }
            }
        }
        let (table, bytes) = match chunks {
            Some(c) => (r.table.clone(), c),
            None => (
                Self::whole(&r.table),
                vec![tables::write_soup(&SoupTable {
                    positions: soup.positions.clone(),
                    triangles: soup.triangles.clone(),
                    surfaces: soup.surfaces.clone(),
                    reserved: 0,
                })],
            ),
        };
        let table = self.tables(&table, bytes)?;
        let mut fields = ObjB::default();
        fields.opt("mode", r.mode, |m| J::str(m.as_str()));
        fields.opt("ground", r.ground, J::Bool);
        fields.opt("region_cut", r.region_cut, J::Bool);
        fields.other(&r.other);
        Ok(self.table_ref(&table, fields.0))
    }

    fn manifest(&mut self) -> Result<J> {
        let w = self.w;
        let scene = self.scene;
        let present = |k: &str| scene.key_order.iter().any(|x| x == k);
        let mut known: Vec<(&'static str, Option<J>)> = Vec::new();
        let mut put = |k: &'static str, v: Option<J>| known.push((k, v));

        put("format", Some(J::str("skate-scene")));
        put("version", Some(J::int(1)));
        put("generator", scene.generator.as_ref().map(J::str));
        put("name", Some(J::str(&scene.name)));
        put("display_name", scene.display_name.as_ref().map(J::str));
        put("region", scene.region.map(|r| f32s(&r)));
        put(
            "surfaces",
            scene
                .surfaces
                .as_ref()
                .map(|s| J::Arr(s.iter().map(J::str).collect())),
        );
        put(
            "streaming_distances",
            scene.streaming_distances.as_ref().map(|s| {
                let mut o = ObjB::default();
                o.opt("near", s.near, J::f64);
                o.opt("medium", s.medium, J::f64);
                o.opt("far", s.far, J::f64);
                o.other(&s.other);
                o.done()
            }),
        );
        put(
            "texture_budget",
            scene.texture_budget.as_ref().map(|s| {
                let mut o = ObjB::default();
                o.opt("max", s.max, J::f64);
                o.opt("aux_weight", s.aux_weight, J::f64);
                o.other(&s.other);
                o.done()
            }),
        );
        put(
            "min_height",
            scene.min_height.as_ref().map(|s| {
                let mut o = ObjB::default();
                o.opt("height", s.height, J::f64);
                o.opt("stub_below", s.stub_below, J::f64);
                o.opt("floor", s.floor, J::f64);
                o.other(&s.other);
                o.done()
            }),
        );
        put("max_render_edge", scene.max_render_edge.map(J::f64));
        let pause = match &scene.pause_map {
            None => None,
            Some(pm) => {
                let image = self.copy(&pm.image, &pm.image_file)?;
                let mut o = ObjB::default();
                o.put(
                    "image",
                    match &image.sha256 {
                        None => J::str(&image.path),
                        Some(h) => J::Obj(vec![
                            ("file".into(), J::str(&image.path)),
                            ("sha256".into(), J::str(h)),
                        ]),
                    },
                );
                o.put("bounds", f32s(&pm.bounds));
                o.other(&pm.other);
                Some(o.done())
            }
        };
        let mut put = |k: &'static str, v: Option<J>| known.push((k, v));
        put("pause_map", pause);
        put("trainer_note", scene.trainer_note.as_ref().map(J::str));
        put("light_prefix", scene.light_prefix.as_ref().map(J::str));
        put(
            "warnings",
            scene
                .warnings
                .as_ref()
                .map(|s| J::Arr(s.iter().map(J::str).collect())),
        );
        put("stats", scene.stats.as_ref().map(json::from_value));

        // textures
        let mut tex_key: HashMap<Entity, String> = HashMap::new();
        let mut textures = ObjB::default();
        for (e, t) in w.iter::<SkateTexture>() {
            let src = w
                .get::<TextureFile>(e)
                .ok_or_else(|| bad(format!("texture '{}' has no core.texture_file", t.key)))?;
            let file = self.copy(&t.file, &src.path)?;
            let mut o = ObjB::default();
            o.put("file", J::str(&file.path));
            o.opt("sha256", file.sha256, J::Str);
            o.opt("name", w.get::<Name>(e), |n| J::str(&n.0));
            o.opt("stem", t.stem.as_ref(), J::str);
            o.opt("source_format", t.source_format.as_ref(), J::str);
            o.opt("source_size", t.source_size, |s| {
                J::Arr(vec![J::int(s[0]), J::int(s[1])])
            });
            o.opt("color_space", t.linear, |l| {
                J::str(if l { "linear" } else { "srgb" })
            });
            o.opt("green", t.green, |g| {
                J::str(match g {
                    NormalGreen::Up => "up",
                    NormalGreen::Down => "down",
                })
            });
            o.opt("z", t.z, |z| {
                J::str(match z {
                    NormalZ::Stored => "stored",
                    NormalZ::Rebuild => "rebuild",
                })
            });
            o.opt("source", w.get::<Source>(e), |s| J::str(&s.0));
            o.other(&t.other);
            if tex_key.insert(e, t.key.clone()).is_some() || textures.0.iter().any(|(k, _)| *k == t.key) {
                return Err(bad(format!("texture key '{}' twice", t.key)));
            }
            textures.put(&t.key, o.done());
        }

        // materials
        let mut mat_key: HashMap<Entity, String> = HashMap::new();
        let mut materials = ObjB::default();
        for (e, m) in w.iter::<SkateMaterial>() {
            let tref = |te: Option<Entity>, role: &str| -> Result<Option<J>> {
                te.map(|te| {
                    tex_key.get(&te).map(J::str).ok_or_else(|| {
                        bad(format!(
                            "material '{}': its {role} {te:?} is not a texture",
                            m.key
                        ))
                    })
                })
                .transpose()
            };
            let mut o = ObjB::default();
            o.put("class", J::str(m.class.as_str()));
            o.opt("base", tref(m.base, "base")?, |j| j);
            o.opt("normal", tref(m.normal, "normal")?, |j| j);
            o.opt("mask", tref(m.mask, "mask")?, |j| j);
            o.opt("base_color", m.base_color, |c| {
                J::Arr(c.iter().map(|x| J::int(*x)).collect())
            });
            o.opt("tint", m.tint, |c| J::Arr(c.iter().map(|x| J::int(*x)).collect()));
            o.opt("alpha", m.alpha, |a| J::str(a.as_str()));
            o.opt("cutoff", m.cutoff, J::f64);
            o.opt("two_sided", m.two_sided, J::Bool);
            o.opt("domain", m.domain, |d| J::str(d.as_str()));
            o.opt("metallic", m.metallic, J::f64);
            o.opt("roughness", m.roughness, J::f64);
            o.opt("wrap", m.wrap, |x| J::str(x.as_str()));
            o.opt("ocean", m.ocean, J::Bool);
            o.opt("cast_shadows", m.cast_shadows, J::Bool);
            o.opt("emissive", m.emissive, J::Bool);
            o.opt("label", w.get::<Name>(e), |n| J::str(&n.0));
            o.opt("source", w.get::<Source>(e), |s| J::str(&s.0));
            o.other(&m.other);
            if materials.0.iter().any(|(k, _)| k.eq_ignore_ascii_case(&m.key)) {
                return Err(bad(format!("material key '{}' twice (up to case)", m.key)));
            }
            mat_key.insert(e, m.key.clone());
            materials.put(&m.key, o.done());
        }
        let mkey = |e: Entity, what: &str| -> Result<J> {
            mat_key
                .get(&e)
                .map(J::str)
                .ok_or_else(|| bad(format!("{what}: {e:?} is not a material")))
        };

        // prototypes
        let mut proto_index: HashMap<Entity, u32> = HashMap::new();
        let mut protos = Vec::new();
        let proto_list: Vec<(Entity, &SkatePrototype)> = w.iter::<SkatePrototype>().collect();
        for (i, (e, p)) in proto_list.into_iter().enumerate() {
            let what = format!("prototype '{}'", p.id);
            proto_index.insert(e, i as u32);
            let mut o = ObjB::default();
            o.put("id", J::str(&p.id));
            o.opt("name", w.get::<Name>(e), |n| J::str(&n.0));
            o.opt("key", p.key, |k| J::uint(u64::from(k)));
            let mesh_file = w.get::<MeshFile>(e);
            let mats: Vec<Option<Entity>> = match (&p.glb, mesh_file) {
                (None, _) => {
                    o.put("glb", J::Null);
                    if !p.parts.is_empty() {
                        return Err(bad(format!("{what}: parts without geometry")));
                    }
                    Vec::new()
                }
                (Some(g), Some(mf)) => {
                    let g = self.copy(g, &mf.path)?;
                    o.put(
                        "glb",
                        match &g.sha256 {
                            None => J::str(&g.path),
                            Some(h) => J::Obj(vec![
                                ("file".into(), J::str(&g.path)),
                                ("sha256".into(), J::str(h)),
                            ]),
                        },
                    );
                    if mf.mesh != 0 {
                        o.put("mesh", J::uint(u64::from(mf.mesh)));
                    }
                    mf.materials.clone()
                }
                (Some(_), None) if w.has::<Mesh>(e) => {
                    return Err(bad(format!(
                        "{what}: its mesh changed (no core.mesh_file); writing meshes needs the glTF writer"
                    )));
                }
                (Some(_), None) => return Err(bad(format!("{what}: a glb but no core.mesh_file"))),
            };
            if mats.len() != p.parts.len() {
                return Err(bad(format!(
                    "{what}: {} parts, {} primitive materials",
                    p.parts.len(),
                    mats.len()
                )));
            }
            let variants = w.get::<MaterialVariants>(e);
            if variants.is_some_and(|v| v.0.len() != p.parts.len()) {
                return Err(bad(format!(
                    "{what}: material variants for a different number of parts"
                )));
            }
            o.opt("source", w.get::<Source>(e), |s| J::str(&s.0));
            o.opt("foliage", p.foliage, J::Bool);
            o.opt("ground", p.ground, J::Bool);
            let mut parts = Vec::new();
            for (pi, (part, mat)) in p.parts.iter().zip(&mats).enumerate() {
                let mut po = ObjB::default();
                po.put(
                    "material",
                    mat.map(|m| mkey(m, &what)).transpose()?.unwrap_or(J::Null),
                );
                let vm = variants.map(|v| &v.0[pi]);
                if part.variants_key || vm.is_some_and(|m| !m.is_empty()) {
                    let mut vo = ObjB::default();
                    for (k, m) in vm.into_iter().flatten() {
                        vo.put(&k.to_string(), mkey(*m, &what)?);
                    }
                    po.put("variants", vo.done());
                }
                po.opt("source", part.source.as_ref(), json::from_value);
                po.other(&part.other);
                parts.push(po.done());
            }
            o.put("parts", J::Arr(parts));
            match (&p.collision, w.get::<SkateSoup>(e)) {
                (Some(r), Some(soup)) => {
                    let j = self.soup_tables(r, soup, &format!("{what} collision"))?;
                    o.put("collision", j);
                }
                (None, Some(soup)) => {
                    let r = SoupRef {
                        table: TableRef::single(&format!("collision/proto_{i}.bin")),
                        mode: None,
                        ground: None,
                        region_cut: None,
                        other: Other::new(),
                    };
                    let j = self.soup_tables(&r, soup, &format!("{what} collision"))?;
                    o.put("collision", j);
                }
                (Some(_), None) => {
                    return Err(bad(format!("{what}: a collision reference but no skate.soup")));
                }
                (None, None) => {}
            }
            o.other(&p.other);
            protos.push(o.done());
        }

        // placements
        let mut psrc = StrTable::new(scene.placement_sources.as_ref());
        let mut recs = Vec::new();
        for (e, inst) in w.iter::<Instance>() {
            let proto = *proto_index.get(&inst.prototype).ok_or_else(|| {
                bad(format!(
                    "placement {e:?}: its prototype {:?} is not one",
                    inst.prototype
                ))
            })?;
            let t = w
                .get::<Transform>(e)
                .ok_or_else(|| bad(format!("placement {e:?}: no core.transform")))?;
            let sp = w.get::<SkatePlacement>(e).cloned().unwrap_or_default();
            let tile = w.get::<Tile>(e);
            let mut fl = sp.unknown_flags;
            if tile.is_some() {
                fl |= flags::TILE;
            }
            if sp.radius.is_some() {
                fl |= flags::RADIUS;
            }
            if let Some(c) = sp.clip {
                fl |= flags::CLIP_GIVEN | if c { flags::CLIP } else { 0 };
            }
            if sp.no_collision {
                fl |= flags::NO_COLLISION;
            }
            if sp.split_by_tile {
                fl |= flags::SPLIT;
            }
            let c = t.cols;
            recs.push(PlacementRec {
                proto,
                variant: inst.variant,
                m: [
                    c[0][0], c[0][1], c[0][2], c[1][0], c[1][1], c[1][2], c[2][0], c[2][1], c[2][2], c[3][0],
                    c[3][1], c[3][2],
                ],
                tile: tile.map_or([0, 0], |t| [t.col, t.row]),
                radius: sp.radius.unwrap_or(0.0),
                flags: fl,
                source: w.get::<Source>(e).map_or(u32::MAX, |s| psrc.get(&s.0)),
            });
        }
        let placements = if !recs.is_empty() || scene.placements.is_some() {
            let t = scene
                .placements
                .clone()
                .unwrap_or_else(|| TableRef::single("tables/placements.bin"));
            let (t, chunks) = match Self::split(&t, recs.len()) {
                Some(sizes) => {
                    let mut chunks = Vec::new();
                    let mut at = 0;
                    for n in sizes {
                        chunks.push(tables::write_placements(&recs[at..at + n]));
                        at += n;
                    }
                    (t, chunks)
                }
                None => (Self::whole(&t), vec![tables::write_placements(&recs)]),
            };
            let t = self.tables(&t, chunks)?;
            Some(self.table_ref(&t, Vec::new()))
        } else {
            None
        };

        // world soups
        let mut soups = Vec::new();
        for (e, c) in w.iter::<SkateCollision>() {
            let soup = w
                .get::<SkateSoup>(e)
                .ok_or_else(|| bad(format!("collision {e:?}: no skate.soup")))?;
            soups.push(self.soup_tables(&c.0, soup, &format!("collision {e:?}"))?);
        }

        // lights
        let mut lsrc = StrTable::new(scene.light_sources.as_ref());
        let mut groups = StrTable::new(scene.light_groups.as_ref());
        let group_list =
            scene.light_groups.is_some() || w.iter::<SkateLight>().any(|(_, l)| !l.group.is_empty());
        let mut lrecs = Vec::new();
        for (e, l) in w.iter::<SkateLight>() {
            lrecs.push(LightRec {
                kind: u32::from(l.spot),
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
                flags: l.unknown_flags
                    | if l.cast_shadow { flags::CAST_SHADOW } else { 0 }
                    | if l.priority.is_some() { flags::PRIORITY } else { 0 },
                priority: l.priority.unwrap_or(0.0),
                group: if group_list { groups.get(&l.group) } else { 0 },
                source: w.get::<Source>(e).map_or(u32::MAX, |s| lsrc.get(&s.0)),
            });
        }
        let lights = if !lrecs.is_empty() || scene.lights.is_some() {
            let t = scene
                .lights
                .clone()
                .unwrap_or_else(|| TableRef::single("tables/lights.bin"));
            let (t, chunks) = match Self::split(&t, lrecs.len()) {
                Some(sizes) => {
                    let mut chunks = Vec::new();
                    let mut at = 0;
                    for n in sizes {
                        chunks.push(tables::write_lights(&lrecs[at..at + n]));
                        at += n;
                    }
                    (t, chunks)
                }
                None => (Self::whole(&t), vec![tables::write_lights(&lrecs)]),
            };
            let t = self.tables(&t, chunks)?;
            Some(self.table_ref(&t, Vec::new()))
        } else {
            None
        };

        // grind curves: each chunk's points in curve order
        let curves: Vec<&SkateGrindCurve> = w.iter::<SkateGrindCurve>().map(|(_, c)| c).collect();
        let grind = if !curves.is_empty() || scene.grind_curves.is_some() {
            let t = scene
                .grind_curves
                .clone()
                .unwrap_or_else(|| TableRef::single("tables/grind.bin"));
            let (t, sizes) = match Self::split(&t, curves.len()) {
                Some(sizes) => (t, sizes),
                None => (Self::whole(&t), vec![curves.len()]),
            };
            let mut chunks = Vec::new();
            let mut at = 0;
            for n in sizes {
                let mut table = CurveTable::default();
                for c in &curves[at..at + n] {
                    if c.points.len() < 2 {
                        return Err(bad("a grind curve with fewer than 2 points"));
                    }
                    table.curves.push(CurveRec {
                        first: table.points.len() as u32,
                        count: c.points.len() as u32,
                        flags: c.unknown_flags | u32::from(c.closed) | (u32::from(c.kind & 15) << 4),
                        radius: c.radius,
                        surface: c.surface,
                    });
                    table.points.extend_from_slice(&c.points);
                }
                chunks.push(tables::write_curves(&table));
                at += n;
            }
            let t = self.tables(&t, chunks)?;
            Some(self.table_ref(&t, Vec::new()))
        } else {
            None
        };

        // water and markers
        let water: Vec<J> = w
            .iter::<SkateWater>()
            .map(|(_, wa)| {
                let mut o = ObjB::default();
                o.put("corners", J::Arr(wa.corners.iter().map(|c| f32s(c)).collect()));
                o.opt("height", wa.height, J::f64);
                o.other(&wa.other);
                o.done()
            })
            .collect();
        let markers: Vec<J> = w
            .iter::<SkateMarker>()
            .map(|(e, m)| {
                let mut o = ObjB::default();
                o.put("kind", J::str(m.kind.as_str()));
                o.opt("name", w.get::<Name>(e), |n| J::str(&n.0));
                o.opt("position", m.position, |([x, z], y)| {
                    J::Arr(vec![J::f32(x), y.map_or(J::Null, J::f32), J::f32(z)])
                });
                o.opt("yaw", m.yaw, J::f64);
                o.opt("m", m.m, |t| f32s(&t));
                o.opt("blueprint", m.blueprint.as_ref(), J::str);
                o.opt("snap", m.snap.as_ref(), snap_json);
                o.opt("lift", m.lift, J::f64);
                o.opt("shelter", m.shelter, J::Bool);
                o.opt("trainer", m.trainer, J::Bool);
                o.other(&m.other);
                o.done()
            })
            .collect();

        let list = |items: Vec<J>, k: &str| (!items.is_empty() || present(k)).then_some(J::Arr(items));
        let strs = |t: &StrTable, read: bool, k: &str| {
            (read || t.appended || present(k)).then(|| J::Arr(t.list.iter().map(J::str).collect()))
        };
        known.push((
            "materials",
            (!materials.0.is_empty() || present("materials")).then(|| materials.done()),
        ));
        known.push((
            "textures",
            (!textures.0.is_empty() || present("textures")).then(|| textures.done()),
        ));
        known.push(("prototypes", list(protos, "prototypes")));
        known.push(("placements", placements));
        known.push((
            "placement_sources",
            strs(&psrc, scene.placement_sources.is_some(), "placement_sources"),
        ));
        known.push(("collision", list(soups, "collision")));
        known.push(("lights", lights));
        known.push((
            "light_groups",
            if group_list {
                strs(&groups, scene.light_groups.is_some(), "light_groups")
            } else {
                None
            },
        ));
        known.push((
            "light_sources",
            strs(&lsrc, scene.light_sources.is_some(), "light_sources"),
        ));
        known.push(("grind_curves", grind));
        known.push(("water", list(water, "water")));
        known.push(("markers", list(markers, "markers")));

        // top level: the order read, then known keys with content, then other keys
        let mut out = ObjB::default();
        let mut done: Vec<String> = Vec::new();
        for k in &scene.key_order {
            if let Some((_, v)) = known.iter_mut().find(|(n, _)| n == k) {
                if let Some(v) = v.take() {
                    out.put(k, v);
                    done.push(k.clone());
                }
            } else if let Some(v) = scene.other.get(k) {
                out.put(k, json::from_value(v));
                done.push(k.clone());
            }
        }
        for (k, v) in known {
            if let Some(v) = v {
                out.put(k, v);
                done.push(k.to_owned());
            }
        }
        for (k, v) in &scene.other {
            if !done.contains(k) {
                out.put(k, json::from_value(v));
            }
        }
        Ok(out.done())
    }
}

fn snap_json(s: &Snap) -> J {
    match s {
        Snap::Mode(m) => J::str(m.as_str()),
        Snap::Object {
            mode,
            above,
            from,
            min_up,
            rings,
            directions,
            max_above_ground,
            headroom,
            other,
        } => {
            let mut o = ObjB::default();
            o.put("mode", J::str(mode.as_str()));
            o.opt("above", *above, J::f64);
            o.opt("from", from.as_ref(), J::str);
            o.opt("min_up", *min_up, J::f64);
            o.opt("rings", rings.as_ref(), |r| f32s(r));
            o.opt("directions", *directions, J::f64);
            o.opt("max_above_ground", *max_above_ground, J::f64);
            o.opt("headroom", *headroom, J::f64);
            o.other(other);
            o.done()
        }
    }
}
