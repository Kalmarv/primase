//! GTA's water: the reader's side ([`attach_water`]: every water file, each quad placed as a `gta.water_surface`, the
//! rest of the file as a `gta.water`), `gta-select-water`, which clips the placed water to the region, and
//! `lower-gta-water`, which makes the surfaces drawn meshes.
//!
//! The data: the game holds one water file at a time ([`WATER_FILES`]), the world map's
//! (`common.rpf\data\levels\gta5\water.xml`) or, while a script loads the island, Cayo Perico's
//! (`water_heistisland.xml`, with its own tuning). Both are read; which one holds is the state axis `gta.water`
//! (default the world map's), and each file's placed surfaces carry its condition, so `resolve-state` picks the file
//! like any other state. A file: water quads, axis-aligned rectangles in GTA ground coordinates at a height `z` (the
//! sea at 0, lakes, rivers and pools above), each the whole rectangle (type 0) or one of its four corner triangles
//! (types 1-4, by the corners (minX, minY), (maxX, minY), (minX, maxY), (maxX, maxY) as 0-3: 1 = 0 1 2, 2 = 0 3 2,
//! 3 = 1 3 2, 4 = 0 1 3), with a per-corner value `a1`-`a4`, invisibility, depth and stencil flags; calming quads (a
//! dampening of the waves) and wave quads (amplitude and direction). The tuning file (`common.rpf\data\watertune.xml`,
//! the island's `watertune_heistisland.xml`) gives, among others, the specular falloff of the water shader.
//!
//! The reader places every quad as authored: a prototype holding its outline and the quad as stored
//! (`gta.water_surface`), placed once with an identity transform; the file's calming and wave quads and its path stay
//! in a `gta.water` entity of their own.
//!
//! `gta-select-water{x0,y0,x1,y1,clip}` (a selection pass): each placed water surface clipped to the region
//! (`clip=region`, the default), or to the region and as far past it as the placements `gta-select-region` kept reach
//! (`clip=kept`: its `gta.region_extent`, which this pass then removes; the region rules keep a placement whose box
//! meets the region, so the seabed reaches past it); a surface with nothing left is left out with its placement.
//! Every placed surface is counted once, kept or outside; surfaces whose placement another state left out go too,
//! counted. It removes `gta.water`, counting the calming and wave quads of every file read (they are not placed).
//!
//! `lower-gta-water` (after loading): each `gta.water_surface` a flat upward mesh in the core frame with one water
//! material: the earlier exporter's blended colour ([`crate::lower::WATER`], as water shaders get), roughness from the
//! tuning's specular falloff taken as the Blinn-Phong exponent (the water shader is not read: approximated), marked
//! `core.material_usage` water (what the water file states), and
//! its `core.collision`: the same outline as triangles at the water's height, every one on surface `WATER` (class
//! water, from the surface table), so a writer that knows water surfaces makes it water to fall into, not ground.
//! An invisible quad gets neither (counted); the corner values and the depth and stencil flags are not drawn:
//! counted. The surfaces are counted by height: the sea (z = 0) and above it.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{
    Collision, Component, Entity, Instance, JsonError, MaterialUsage, Mesh, Name, Primitive, Resource,
    Semantic, Source, Stream, Surfaces, Topology, Transform, World,
};

use crate::archive_set::Install;
use crate::keys::Keys;
use crate::region::GTA;
use crate::select::RegionExtent;
use crate::{Result, invalid};

/// The world map's water data and the water tuning, as the install stores them.
pub const WATER_FILE: &str = r"common.rpf\data\levels\gta5\water.xml";
pub const TUNE_FILE: &str = r"common.rpf\data\watertune.xml";

/// The water files the game holds one at a time: the `gta.water` axis's value name, the water data and its tuning.
/// The first is the world map's (required; the axis's default); the others are read when the install has them.
pub const WATER_FILES: [(&str, &str, &str); 2] = [
    ("world", WATER_FILE, TUNE_FILE),
    (
        "heistisland",
        r"update\update.rpf\common\data\levels\gta5\water_heistisland.xml",
        r"update\update.rpf\common\data\watertune_heistisland.xml",
    ),
];

/// A water quad, as stored (GTA ground coordinates, metres).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaterQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub z: f32,
    /// 0: the whole rectangle; 1-4: one of its corner triangles (see the module documentation).
    pub kind: u8,
    pub invisible: bool,
    pub limited_depth: bool,
    /// `a1`-`a4`, by corner.
    pub corners: [f32; 4],
    pub no_stencil: bool,
}

/// A calming quad: where waves are damped.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct CalmingQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub dampening: f32,
}

/// A wave quad: a wave's amplitude and direction over a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaveQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub amplitude: f32,
    pub direction: [f32; 2],
}

/// A water file as stored, with its tuning's specular falloff.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterFile {
    pub path: String,
    pub quads: Vec<WaterQuad>,
    pub calming: Vec<CalmingQuad>,
    pub waves: Vec<WaveQuad>,
    /// The tuning's `SpecularFalloff`, when the tuning file has one.
    pub specular_falloff: Option<f32>,
}

/// What a water file holds besides its quads (which are placed): its calming and wave quads, raw.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaWater {
    pub path: String,
    pub calming: Vec<CalmingQuad>,
    pub waves: Vec<WaveQuad>,
}

impl Component for GtaWater {
    const NAME: &'static str = "gta.water";
    /// 2: the quads are placed (`gta.water_surface`), the tuning's falloff is theirs.
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

fn err(detail: impl ToString) -> crate::Error {
    invalid("water", 0, detail)
}

/// Each water file of [`WATER_FILES`] the install has (the world map's is required) with its value name on
/// `gta.water` and its tuning's specular falloff, as stored.
pub fn read_water(install: &Install, keys: &Keys) -> Result<Vec<(&'static str, WaterFile)>> {
    let mut out = Vec::new();
    for (i, (value, path, tune_path)) in WATER_FILES.into_iter().enumerate() {
        let Some(bytes) = install.read(path, keys)? else {
            if i == 0 {
                return Err(err(format!("{path} not found")));
            }
            continue;
        };
        let mut w = parse_water(&bytes)?;
        w.path = path.to_string();
        if let Some(tune) = install.read(tune_path, keys)? {
            w.specular_falloff = specular_falloff(&tune, tune_path)?;
        }
        out.push((value, w));
    }
    Ok(out)
}

/// A tuning file's `SpecularFalloff`: `None` when it has none, an error when it is not a number.
pub fn specular_falloff(tune: &[u8], path: &str) -> Result<Option<f32>> {
    let root = crate::xml::parse(tune, path)?;
    root.child_value("SpecularFalloff")
        .map(|v| {
            v.parse()
                .map_err(|_| err(format!("{path}: SpecularFalloff {v:?} is not a number")))
        })
        .transpose()
}

/// Every water file the install has ([`read_water`]) into the world (see the module documentation): with more than
/// one, the `gta.water` axis (default the world map's); each quad a placed `gta.water_surface` (with its file's
/// condition), the rest of each file a `gta.water`. Returns each file's path with its water, calming and wave quad
/// counts.
pub fn attach_water(w: &mut World, install: &Install, keys: &Keys) -> Result<Vec<(String, [usize; 3])>> {
    let files = read_water(install, keys)?;
    Ok(place_water(w, files))
}

/// [`attach_water`] for files already read.
pub fn place_water(w: &mut World, files: Vec<(&str, WaterFile)>) -> Vec<(String, [usize; 3])> {
    let axis = (files.len() > 1).then(|| {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: "gta.water".into(),
                domain: Domain::Enum(files.iter().map(|(v, _)| v.to_string()).collect()),
                default: 0,
                scope: Scope::Global,
                class: "water".into(),
                source: "the water file the game holds: the world map's, or Cayo Perico's while a script loads the \
                         island"
                    .into(),
            },
        );
        e
    });
    let mut counts = Vec::new();
    for (i, (value, file)) in files.into_iter().enumerate() {
        counts.push((
            file.path.clone(),
            [file.quads.len(), file.calming.len(), file.waves.len()],
        ));
        let condition = axis.map(|axis| {
            Condition(vec![Clause(vec![Literal {
                axis,
                values: 1u64 << i,
            }])])
        });
        for (k, q) in file.quads.iter().enumerate() {
            let proto = w.spawn();
            w.insert(
                proto,
                GtaWaterSurface {
                    outline: outline(q),
                    quad: *q,
                    specular_falloff: file.specular_falloff,
                },
            );
            // the world map's names as before; another file's carry its value
            w.insert(
                proto,
                Name(if i == 0 {
                    format!("gta_water_{k}")
                } else {
                    format!("gta_water_{value}_{k}")
                }),
            );
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(p, Transform::IDENTITY);
            if let Some(c) = &condition {
                w.insert(p, c.clone());
            }
            w.insert(p, Source(format!("{}#water_quad_{k}", file.path)));
        }
        let e = w.spawn();
        w.insert(
            e,
            GtaWater {
                path: file.path,
                calming: file.calming,
                waves: file.waves,
            },
        );
    }
    counts
}

/// Water data from `water.xml` bytes (`path` empty, no tuning).
pub fn parse_water(bytes: &[u8]) -> Result<WaterFile> {
    let root = crate::xml::parse(bytes, "water.xml")?;
    let num = |e: &crate::xml::Element, k: &str| -> Result<f32> {
        e.child_value(k)
            .ok_or_else(|| err(format!("a quad without {k}")))?
            .parse()
            .map_err(|_| err(format!("{k}: not a number")))
    };
    // a flag left out is false; any value but true or false is an error
    let flag = |e: &crate::xml::Element, k: &str| -> Result<bool> {
        match e.child_value(k) {
            None => Ok(false),
            Some(v) if v.eq_ignore_ascii_case("true") => Ok(true),
            Some(v) if v.eq_ignore_ascii_case("false") => Ok(false),
            Some(v) => Err(err(format!("{k}: {v:?} is neither true nor false"))),
        }
    };
    let rect = |e: &crate::xml::Element| -> Result<([f32; 2], [f32; 2])> {
        Ok((
            [num(e, "minX")?, num(e, "minY")?],
            [num(e, "maxX")?, num(e, "maxY")?],
        ))
    };
    let section = |name: &str| root.child(name).into_iter().flat_map(|s| s.items());
    let mut quads = Vec::new();
    for q in section("WaterQuads") {
        let (min, max) = rect(q)?;
        let kind = num(q, "Type")? as u8;
        if kind > 4 {
            return Err(err(format!("water quad type {kind}")));
        }
        quads.push(WaterQuad {
            min,
            max,
            z: num(q, "z")?,
            kind,
            invisible: flag(q, "IsInvisible")?,
            limited_depth: flag(q, "HasLimitedDepth")?,
            corners: [num(q, "a1")?, num(q, "a2")?, num(q, "a3")?, num(q, "a4")?],
            no_stencil: flag(q, "NoStencil")?,
        });
    }
    let mut calming = Vec::new();
    for q in section("CalmingQuads") {
        let (min, max) = rect(q)?;
        calming.push(CalmingQuad {
            min,
            max,
            dampening: num(q, "fDampening")?,
        });
    }
    let mut waves = Vec::new();
    for q in section("WaveQuads") {
        let (min, max) = rect(q)?;
        waves.push(WaveQuad {
            min,
            max,
            amplitude: num(q, "Amplitude")?,
            direction: [num(q, "XDirection")?, num(q, "YDirection")?],
        });
    }
    Ok(WaterFile {
        path: String::new(),
        quads,
        calming,
        waves,
        specular_falloff: None,
    })
}

/// A quad's outline (GTA ground coordinates), counter-clockwise seen from above.
pub fn outline(q: &WaterQuad) -> Vec<[f32; 2]> {
    let c = [
        [q.min[0], q.min[1]],
        [q.max[0], q.min[1]],
        [q.min[0], q.max[1]],
        [q.max[0], q.max[1]],
    ];
    let mut p: Vec<[f32; 2]> = match q.kind {
        0 => vec![c[0], c[1], c[3], c[2]],
        1 => vec![c[0], c[1], c[2]],
        2 => vec![c[0], c[3], c[2]],
        3 => vec![c[1], c[3], c[2]],
        _ => vec![c[0], c[1], c[3]],
    };
    if area(&p) < 0.0 {
        p.reverse();
    }
    p
}

/// Twice the signed area of a polygon (positive: counter-clockwise).
fn area(p: &[[f32; 2]]) -> f32 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// A convex polygon clipped to the rectangle `[x0, y0, x1, y1]` (Sutherland-Hodgman); empty when nothing is inside.
pub fn clip(p: &[[f32; 2]], r: [f32; 4]) -> Vec<[f32; 2]> {
    let mut out = p.to_vec();
    // each edge of the rectangle: axis, bound, keep the side above (true) or below
    for (axis, bound, above) in [
        (0, r[0], true),
        (0, r[2], false),
        (1, r[1], true),
        (1, r[3], false),
    ] {
        let inside = |v: [f32; 2]| if above { v[axis] >= bound } else { v[axis] <= bound };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            if inside(a) {
                out.push(a);
            }
            if inside(a) != inside(b) {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    if out.len() < 3 || area(&out).abs() < 1e-6 {
        Vec::new()
    } else {
        out
    }
}

/// One water surface to draw: a water quad's outline (GTA ground coordinates, counter-clockwise from above; the whole
/// quad as read, clipped by `gta-select-water`), the quad as stored (its height, corner values and flags) and its
/// tuning's specular falloff; a prototype of its own, placed once by the reader and lowered by `lower-gta-water`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaWaterSurface {
    pub outline: Vec<[f32; 2]>,
    pub quad: WaterQuad,
    pub specular_falloff: Option<f32>,
}

impl Component for GtaWaterSurface {
    const NAME: &'static str = "gta.water_surface";
    /// 2: the quad as stored (`quad`) instead of its height alone.
    const VERSION: u32 = 2;
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl GtaWaterSurface {
    /// Its box in the core frame.
    pub fn bounds(&self) -> Option<[[f64; 3]; 2]> {
        let mut b: Option<[[f64; 3]; 2]> = None;
        for p in &self.outline {
            let c = GTA.point([p[0], p[1], self.quad.z]).map(f64::from);
            let [lo, hi] = b.get_or_insert([c, c]);
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        b
    }
}

/// `gta-select-water{x0,y0,x1,y1,clip}` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectGtaWater {
    /// `[x0, y0, x1, y1]`, GTA ground coordinates; `None`: every surface whole.
    pub region: Option<[f32; 4]>,
    /// `clip=kept`: also as far as `gta.region_extent` reaches.
    pub kept: bool,
}

impl SelectGtaWater {
    pub fn from_options(o: &Options) -> std::result::Result<SelectGtaWater, String> {
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1", "clip"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let kept = match o.get("clip").map(String::as_str) {
            None | Some("region") => false,
            Some("kept") => true,
            Some(v) => return Err(format!("clip: {v} is neither region nor kept")),
        };
        let num = |k: &str| -> std::result::Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number")))
                .transpose()
        };
        let region = match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            [None, None, None, None] => None,
            _ => return Err("x0, y0, x1 and y1 go together".into()),
        };
        if kept && region.is_none() {
            return Err("clip=kept needs the region (x0, y0, x1, y1)".into());
        }
        Ok(SelectGtaWater { region, kept })
    }
}

impl Pass for SelectGtaWater {
    fn name(&self) -> &'static str {
        "gta-select-water"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if let Some([x0, y0, x1, y1]) = self.region {
            for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
                o.insert(k.to_string(), v.to_string());
            }
        }
        if self.kept {
            o.insert("clip".into(), "kept".into());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        if self.kept {
            &["gta.water", RegionExtent::NAME]
        } else {
            &["gta.water"]
        }
    }
    fn modifies(&self) -> &[&'static str] {
        &["gta.water_surface"]
    }
    fn removes(&self) -> &[&'static str] {
        if self.kept {
            &["gta.water", RegionExtent::NAME]
        } else {
            &["gta.water"]
        }
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let waters: Vec<(Entity, GtaWater)> = w.iter::<GtaWater>().map(|(e, g)| (e, g.clone())).collect();
        for (e, water) in waters {
            w.remove::<GtaWater>(e);
            log.count("calming quads not drawn", water.calming.len() as u64);
            log.count("wave quads not drawn", water.waves.len() as u64);
        }
        // the region, and with clip=kept as far as what is drawn
        let clip_to = match (self.region, self.kept) {
            (Some(r), true) => {
                let extent = w
                    .remove_resource::<RegionExtent>()
                    .ok_or("clip=kept: no gta.region_extent")?;
                Some(extent.0.map_or(r, |e| {
                    [r[0].min(e[0]), r[1].min(e[1]), r[2].max(e[2]), r[3].max(e[3])]
                }))
            }
            (r, _) => r,
        };
        let placements: Vec<(Entity, Entity)> = w
            .iter::<Instance>()
            .filter(|(_, i)| w.has::<GtaWaterSurface>(i.prototype))
            .map(|(p, i)| (p, i.prototype))
            .collect();
        let placed: BTreeSet<Entity> = placements.iter().map(|&(_, proto)| proto).collect();
        // a surface whose placement another state left out (resolve-state) goes with it
        let unplaced: Vec<Entity> = w
            .iter::<GtaWaterSurface>()
            .map(|(e, _)| e)
            .filter(|e| !placed.contains(e))
            .collect();
        log.count(
            "water surfaces left out: placement left out by its state",
            unplaced.len() as u64,
        );
        for e in unplaced {
            w.despawn(e);
        }
        let Some(r) = clip_to else {
            log.count("water surfaces kept", placements.len() as u64);
            return Ok(());
        };
        for (p, proto) in placements {
            let Some(s) = w.get_mut::<GtaWaterSurface>(proto) else {
                continue;
            };
            s.outline = clip(&s.outline, r);
            if s.outline.is_empty() {
                log.count("water surfaces left out: outside the region", 1);
                w.despawn(p);
                w.despawn(proto);
            } else {
                log.count("water surfaces kept", 1);
            }
        }
        Ok(())
    }
}

/// `lower-gta-water` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaWater;

/// The water surface material (see the module documentation).
pub fn water_material(specular_falloff: Option<f32>) -> PbrMaterial {
    let srgb = |b: u8| primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
    let w = crate::lower::WATER;
    PbrMaterial {
        base_color: [srgb(w[0]), srgb(w[1]), srgb(w[2]), f32::from(w[3]) / 255.0],
        metallic: 0.0,
        // a Blinn-Phong exponent n as GGX: alpha = sqrt(2 / (n + 2)), roughness sqrt(alpha)
        roughness: specular_falloff.map_or(1.0, |n| (2.0 / (n.max(0.0) + 2.0)).powf(0.25)),
        alpha_mode: AlphaMode::Blend,
        ..PbrMaterial::default()
    }
}

impl Pass for LowerGtaWater {
    fn name(&self) -> &'static str {
        "lower-gta-water"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn provides(&self) -> &[&'static str] {
        &[
            "core.mesh",
            "pbr.material",
            "core.material_usage",
            "core.name",
            "core.collision",
            "core.surfaces",
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.water_surface"]
    }
    /// The one surface it adds (`WATER`) is classified from the surface table as it is added, so it can run after
    /// `lower-gta-surfaces`.
    fn settles(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    fn approximates(&self) -> &'static str {
        "the water shader as the earlier exporter's blended flat colour, its roughness from the tuning's specular \
         falloff; waves, calming, corner values and depth and stencil flags not drawn"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let surfaces: Vec<(Entity, GtaWaterSurface)> =
            w.iter::<GtaWaterSurface>().map(|(e, s)| (e, s.clone())).collect();
        if surfaces.is_empty() {
            return Ok(());
        }
        // one material per specular falloff (one per water file)
        let mut materials: BTreeMap<Option<u32>, Entity> = BTreeMap::new();
        // the water's collision surface, classified by the surface table
        let mut table = w.remove_resource::<Surfaces>().unwrap_or_default();
        let water = table.index("WATER").map_err(|e| e.to_string())?;
        if let Some(t) = table.0.get_mut(usize::from(water)) {
            t.class = crate::surfaces::surface_class("WATER");
        }
        for (proto, s) in surfaces {
            w.remove::<GtaWaterSurface>(proto);
            if s.quad.invisible {
                log.count("invisible water quads: no mesh, no collision", 1);
                continue;
            }
            let material = *materials
                .entry(s.specular_falloff.map(f32::to_bits))
                .or_insert_with(|| {
                    let m = w.spawn();
                    w.insert(m, water_material(s.specular_falloff));
                    w.insert(m, MaterialUsage::Water);
                    w.insert(m, Name("gta_water".into()));
                    log.approximate("water drawn as a blended flat colour", 1);
                    m
                });
            // as stored, with no glTF meaning: counted
            log.approximate("water surfaces drawn without their corner values (a1-a4)", 1);
            if s.quad.limited_depth {
                log.approximate("water surfaces drawn without their limited depth", 1);
            }
            if s.quad.no_stencil {
                log.approximate("water surfaces drawn without their no-stencil flag", 1);
            }
            let positions: Vec<[f32; 3]> = s
                .outline
                .iter()
                .map(|p| GTA.point([p[0], p[1], s.quad.z]))
                .collect();
            let up = GTA.direction([0.0, 0.0, 1.0]);
            let indices: Vec<u32> = (1..s.outline.len() as u32 - 1)
                .flat_map(|k| [0, k, k + 1])
                .collect();
            let triangles: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
            log.count("water collision triangles", triangles.len() as u64);
            w.insert(
                proto,
                Collision {
                    positions: positions.clone(),
                    surfaces: vec![water; triangles.len()],
                    triangles,
                },
            );
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Normal, Stream::F32x3(vec![up; positions.len()]));
            streams.insert(Semantic::Position, Stream::F32x3(positions));
            w.insert(
                proto,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(material),
                }]),
            );
            log.count(
                if s.quad.z == 0.0 {
                    "water surfaces: sea"
                } else {
                    "water surfaces: above sea level (lakes, rivers, pools)"
                },
                1,
            );
        }
        w.insert_resource(table);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::state::ResolveState;

    fn quad(kind: u8) -> WaterQuad {
        WaterQuad {
            min: [0.0, 0.0],
            max: [10.0, 20.0],
            z: 5.0,
            kind,
            invisible: false,
            limited_depth: false,
            corners: [26.0; 4],
            no_stencil: false,
        }
    }

    #[test]
    fn outlines_are_the_rectangle_or_its_corner_triangles_counter_clockwise() {
        assert_eq!(
            outline(&quad(0)),
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 20.0], [0.0, 20.0]]
        );
        // type 1: corners 0 1 2, the triangle without (maxX, maxY)
        let t = outline(&quad(1));
        assert_eq!(t.len(), 3);
        assert!(!t.contains(&[10.0, 20.0]));
        // type 3: corners 1 3 2, without (minX, minY)
        assert!(!outline(&quad(3)).contains(&[0.0, 0.0]));
        for k in 0..=4 {
            assert!(area(&outline(&quad(k))) > 0.0, "type {k} counter-clockwise");
        }
    }

    #[test]
    fn clipping_keeps_the_part_inside_the_region() {
        let c = clip(&outline(&quad(0)), [5.0, -5.0, 15.0, 10.0]);
        assert!((area(&c) / 2.0 - 50.0).abs() < 1e-4, "5 x 10 left: {c:?}");
        assert!(clip(&outline(&quad(0)), [20.0, 0.0, 30.0, 10.0]).is_empty());
        // a corner triangle half in the region: the part of triangle 0 1 2 with x <= 5 is a trapezoid of 150
        let t = clip(&outline(&quad(1)), [0.0, 0.0, 5.0, 20.0]);
        assert!((area(&t) / 2.0 - 75.0).abs() < 1e-3, "{t:?}");
    }

    #[test]
    fn water_xml_reads_its_three_kinds_of_quads() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<WaterData>
  <WaterQuads>
    <Item><minX value="-2" /><maxX value="2" /><minY value="0" /><maxY value="4" /><Type value="2" />
      <IsInvisible value="false" /><HasLimitedDepth value="true" /><z value="1.5" />
      <a1 value="26" /><a2 value="26" /><a3 value="0" /><a4 value="26" /><NoStencil value="false" /></Item>
  </WaterQuads>
  <CalmingQuads>
    <Item><minX value="0" /><maxX value="1" /><minY value="0" /><maxY value="1" /><fDampening value="0.5" /></Item>
  </CalmingQuads>
  <WaveQuads>
    <Item><minX value="0" /><maxX value="1" /><minY value="0" /><maxY value="1" /><Amplitude value="2" />
      <XDirection value="0.5" /><YDirection value="-0.5" /></Item>
  </WaveQuads>
</WaterData>"#;
        let w = parse_water(xml).unwrap();
        assert_eq!(w.quads.len(), 1);
        let q = w.quads[0];
        assert_eq!(
            (q.min, q.max, q.z, q.kind, q.limited_depth),
            ([-2.0, 0.0], [2.0, 4.0], 1.5, 2, true)
        );
        assert_eq!(q.corners, [26.0, 26.0, 0.0, 26.0]);
        assert_eq!(w.calming[0].dampening, 0.5);
        assert_eq!((w.waves[0].amplitude, w.waves[0].direction), (2.0, [0.5, -0.5]));
    }

    /// A type-0 quad at sea level as water.xml writes it.
    fn quad_xml(x: [i32; 4], invisible: bool, limited_depth: bool, no_stencil: bool) -> String {
        let [x0, x1, y0, y1] = x;
        format!(
            r#"<Item><minX value="{x0}" /><maxX value="{x1}" /><minY value="{y0}" /><maxY value="{y1}" />
<Type value="0" /><IsInvisible value="{invisible}" /><HasLimitedDepth value="{limited_depth}" /><z value="0" />
<a1 value="26" /><a2 value="26" /><a3 value="26" /><a4 value="26" /><NoStencil value="{no_stencil}" /></Item>"#
        )
    }

    fn file(path: &str, quads: &[String]) -> WaterFile {
        let mut f = parse_water(
            format!(
                "<WaterData><WaterQuads>{}</WaterQuads></WaterData>",
                quads.concat()
            )
            .as_bytes(),
        )
        .unwrap();
        f.path = path.into();
        f.specular_falloff = Some(1118.0);
        f
    }

    fn select(region: [f32; 4], clip: Option<&str>) -> SelectGtaWater {
        let mut o = Options::new();
        for (k, v) in ["x0", "y0", "x1", "y1"].into_iter().zip(region) {
            o.insert(k.into(), v.to_string());
        }
        if let Some(c) = clip {
            o.insert("clip".into(), c.into());
        }
        SelectGtaWater::from_options(&o).unwrap()
    }

    fn surfaces(w: &World) -> Vec<GtaWaterSurface> {
        w.iter::<Instance>()
            .filter_map(|(_, i)| w.get::<GtaWaterSurface>(i.prototype).cloned())
            .collect()
    }

    /// The reader places every quad whole, the rest of the file apart; one file has no state.
    #[test]
    fn the_reader_places_every_quad_as_stored() {
        let mut w = World::new();
        let counts = place_water(
            &mut w,
            vec![(
                "world",
                file("w.xml", &[quad_xml([0, 10, 0, 10], true, false, false)]),
            )],
        );
        assert_eq!(counts, vec![("w.xml".to_string(), [1, 0, 0])]);
        let s = surfaces(&w);
        assert_eq!(s.len(), 1);
        assert!(s[0].quad.invisible && (area(&s[0].outline) / 2.0 - 100.0).abs() < 1e-3);
        assert_eq!(w.iter::<StateAxis>().count(), 0);
        assert_eq!(w.iter::<Condition>().count(), 0);
        assert_eq!(w.iter::<GtaWater>().count(), 1);
    }

    /// The options decide the clip: the region alone by default; with clip=kept, also as far as the kept placements
    /// reach (here GTA x 0-20, y 0-10, past the 5 x 5 region), and the extent is used up.
    #[test]
    fn the_clip_is_the_region_or_the_region_and_what_is_kept() {
        for (clip, area_left) in [(None, 25.0), (Some("kept"), 200.0)] {
            let mut w = World::new();
            w.insert_resource(RegionExtent(Some([0.0, 0.0, 20.0, 10.0])));
            place_water(
                &mut w,
                vec![(
                    "world",
                    file("w.xml", &[quad_xml([-50, 50, -50, 50], false, false, false)]),
                )],
            );
            let pass = select([0.0, 0.0, 5.0, 5.0], clip);
            assert_eq!(pass.requires().contains(&RegionExtent::NAME), clip.is_some());
            pass.run(&mut w, &mut PassLog::default()).unwrap();
            let s = surfaces(&w);
            assert_eq!(s.len(), 1);
            assert!(
                (area(&s[0].outline) / 2.0 - area_left).abs() < 1e-3,
                "{clip:?}: {:?}",
                s[0].outline
            );
            assert_eq!(w.resource::<RegionExtent>().is_none(), clip.is_some());
        }
        let mut o = Options::new();
        o.insert("clip".into(), "kept".into());
        assert!(
            SelectGtaWater::from_options(&o).is_err(),
            "clip=kept without a region"
        );
        o.insert("clip".into(), "all".into());
        assert!(SelectGtaWater::from_options(&o).is_err());
    }

    /// Two files: each placement carries its file's value on `gta.water`; resolve-state keeps the world map's by
    /// default, and the selection counts every surface once (kept, outside, or left out by its state). Lowering draws
    /// no invisible quad and counts what it does not draw.
    #[test]
    fn the_water_file_is_a_state_and_every_surface_is_counted_once() {
        let mut w = World::new();
        let counts = place_water(
            &mut w,
            vec![
                (
                    "world",
                    file(
                        "w.xml",
                        &[
                            quad_xml([0, 10, 0, 10], false, true, true),
                            quad_xml([0, 10, 0, 10], true, false, false),
                            quad_xml([100, 110, 0, 10], false, false, false),
                        ],
                    ),
                ),
                (
                    "heistisland",
                    file("h.xml", &[quad_xml([0, 10, 0, 10], false, false, false)]),
                ),
            ],
        );
        assert_eq!(counts.len(), 2);
        let axis = w.iter::<StateAxis>().next().unwrap().1.clone();
        assert_eq!(
            (axis.key.as_str(), axis.domain.len(), axis.default),
            ("gta.water", 2, 0)
        );
        assert_eq!(w.iter::<Condition>().count(), 4);
        let mut log = PassLog::default();
        ResolveState::default().run(&mut w, &mut log).unwrap();
        assert_eq!(log.counts.get("gta.water=world kept"), Some(&3));
        assert_eq!(log.counts.get("gta.water=world despawned"), Some(&1));
        let mut log = PassLog::default();
        select([-5.0, -5.0, 5.0, 5.0], None)
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(log.counts.get("water surfaces kept"), Some(&2));
        assert_eq!(
            log.counts.get("water surfaces left out: outside the region"),
            Some(&1)
        );
        assert_eq!(
            log.counts
                .get("water surfaces left out: placement left out by its state"),
            Some(&1)
        );
        assert_eq!(w.iter::<GtaWaterSurface>().count(), 2);
        let s = surfaces(&w);
        assert!(s.iter().any(|s| s.quad.limited_depth && s.quad.no_stencil));
        let json = s[0].to_json().unwrap().unwrap();
        assert_eq!(json["quad"]["corners"][0], 26.0);
        let mut log = PassLog::default();
        LowerGtaWater.run(&mut w, &mut log).unwrap();
        assert_eq!(
            log.counts.get("invisible water quads: no mesh, no collision"),
            Some(&1)
        );
        assert_eq!(log.counts.get("water surfaces: sea"), Some(&1));
        for what in [
            "water surfaces drawn without their corner values (a1-a4)",
            "water surfaces drawn without their limited depth",
            "water surfaces drawn without their no-stencil flag",
        ] {
            assert_eq!(log.approximations.get(what), Some(&1), "{what}");
        }
    }

    /// The selected surface lowered: flat at its height, the water material.
    #[test]
    fn a_selected_surface_is_lowered_flat_with_the_water_material() {
        let mut w = World::new();
        place_water(
            &mut w,
            vec![(
                "world",
                file("w.xml", &[quad_xml([0, 10, 0, 10], false, false, false)]),
            )],
        );
        let mut log = PassLog::default();
        select([-5.0, -5.0, 5.0, 5.0], None)
            .run(&mut w, &mut log)
            .unwrap();
        assert!(w.iter::<GtaWater>().next().is_none());
        let protos: Vec<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        assert_eq!(protos.len(), 1);
        assert!(w.get::<Mesh>(protos[0]).is_none());
        LowerGtaWater.run(&mut w, &mut log).unwrap();
        assert!(w.get::<GtaWaterSurface>(protos[0]).is_none());
        let mesh = w.get::<Mesh>(protos[0]).unwrap();
        let p = mesh.primitives()[0].positions().unwrap();
        // the 5 x 5 corner of the quad, flat at the sea's height (core y up)
        assert!(p.iter().all(|v| v[1] == 0.0));
        let m = w
            .get::<PbrMaterial>(mesh.primitives()[0].material.unwrap())
            .unwrap();
        assert_eq!(m.alpha_mode, AlphaMode::Blend);
        assert!((m.roughness - (2.0f32 / 1120.0).powf(0.25)).abs() < 1e-6);
        assert_eq!(
            w.get::<MaterialUsage>(mesh.primitives()[0].material.unwrap()),
            Some(&MaterialUsage::Water)
        );
        // its collision: the same two triangles at the sea's height, facing up, on surface WATER of class water
        let c = w.get::<Collision>(protos[0]).unwrap();
        assert_eq!(c.triangles.len(), 2);
        assert!(c.positions.iter().all(|v| v[1] == 0.0));
        let [a, b, d] = c.triangles[0].map(|i| c.positions[i as usize]);
        let (u, v) = ([b[0] - a[0], b[2] - a[2]], [d[0] - a[0], d[2] - a[2]]);
        // core y up: the normal's y is the cross product's (z, x) part; positive faces up
        assert!(u[1] * v[0] - u[0] * v[1] > 0.0, "{:?}", c.triangles[0]);
        let table = w.resource::<Surfaces>().unwrap();
        let s = &table.0[usize::from(c.surfaces[0])];
        assert_eq!(
            (s.name.as_str(), s.class),
            ("WATER", Some(primase_core::SurfaceClass::Water))
        );
        assert!(c.surfaces.iter().all(|&x| x == c.surfaces[0]));
    }

    /// A tuning value that is not a number, or a flag that is neither true nor false, is an error naming it.
    #[test]
    fn malformed_values_are_errors() {
        let tune = |v: &str| format!("<WaterTune><SpecularFalloff value=\"{v}\" /></WaterTune>");
        assert_eq!(
            specular_falloff(tune("1118").as_bytes(), "t.xml").unwrap(),
            Some(1118.0)
        );
        let e = specular_falloff(tune("1,118").as_bytes(), "t.xml")
            .unwrap_err()
            .to_string();
        assert!(e.contains("SpecularFalloff"), "{e}");
        assert_eq!(specular_falloff(b"<WaterTune />", "t.xml").unwrap(), None);
        let bad = quad_xml([0, 1, 0, 1], false, false, false)
            .replace(r#"IsInvisible value="false""#, r#"IsInvisible value="1""#);
        let e = parse_water(format!("<WaterData><WaterQuads>{bad}</WaterQuads></WaterData>").as_bytes())
            .unwrap_err()
            .to_string();
        assert!(e.contains("IsInvisible"), "{e}");
    }
}
