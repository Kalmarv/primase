//! A skate-scene world as the map writer takes it, in the writer's frame (east, north, up) = (x, -z, y) of scene
//! space: textures, materials made on first use, prototypes built from their GLBs, placements, collision soups,
//! lights, grind curves, water and markers.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use primase_core::{
    Instance, MaterialVariants, MeshFile, Name, NormalGreen, NormalZ, Semantic, Source, Stream, TextureFile,
    Tile, Transform, World,
};
use primase_hash::skate_fnv_string;
use primase_skate_scene::{
    AlphaMode, CollisionMode as SceneMode, Domain, MarkerKind, MaterialClass, SkateCollision,
    SkateGrindCurve, SkateLight, SkateMarker, SkateMaterial, SkatePlacement, SkatePrototype, SkateScene,
    SkateTexture, SkateWater, Snap, SnapMode as SceneSnap, Wrap,
};

use crate::material::{MatLib, MaterialSpec, VClass};
use crate::model::{CollisionMode, SrcMesh, SrcPart};
use crate::texture::{MASK_TAG, NORMAL_TAG, TexLib, TexRef, TexSource};
use crate::{Error, Result};
use primase_meshops::vector::Vec3;

/// Scene space to the writer's frame: (x, y, z) -> (x, -z, y).
pub fn w(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, -z, y)
}

pub fn w_at(v: &[f32], o: usize) -> Vec3 {
    Vec3::new(v[o], -v[o + 2], v[o + 1])
}

/// The collision surfaces: Studio packed materials; the first 17 the standard table.
#[derive(Clone, Debug)]
pub struct SurfaceTable {
    pub list: Vec<(String, i32)>,
}

pub const STANDARD_SURFACES: [(&str, i32); 17] = [
    ("default", 32),
    ("concrete", 96),
    ("asphalt", 160),
    ("metal", 352),
    ("wood", 416),
    ("marble", 480),
    ("grass", 3424),
    ("brick", 2016),
    ("glass", 2144),
    ("earth", 3488),
    ("sand", 4896),
    ("gravel", 4960),
    ("plastic", 3296),
    ("rubber", 4128),
    ("metal_grate", 1632),
    ("metal_thin", 1696),
    ("roofing", 2272),
];

impl Default for SurfaceTable {
    fn default() -> Self {
        SurfaceTable {
            list: STANDARD_SURFACES
                .iter()
                .map(|(n, p)| ((*n).to_owned(), *p))
                .collect(),
        }
    }
}

impl SurfaceTable {
    pub fn packed(&mut self, packed: i32) -> usize {
        if let Some(i) = self.list.iter().position(|s| s.1 == packed) {
            return i;
        }
        self.list.push((format!("packed{packed}"), packed));
        self.list.len() - 1
    }
    pub fn slot(packed: i32) -> i32 {
        (packed >> 6) & 0x1fff
    }
    pub fn soft(&self, s: usize) -> bool {
        matches!(Self::slot(self.list[s].1), 9 | 53 | 76)
    }
}

/// A texture of the scene.
#[derive(Debug)]
pub struct TexDef {
    pub key: String,
    pub name: String,
    pub stem: Option<String>,
    pub linear: bool,
    pub green_down: bool,
    pub z_rebuild: bool,
    pub src: Arc<TexSource>,
}

/// A part of a prototype.
#[derive(Clone, Debug, Default)]
pub struct PartDef {
    pub material: Option<String>,
    pub variants: Option<BTreeMap<u32, String>>,
    pub shader: String,
    pub bucket: u8,
    pub model: i32,
    pub geom: i32,
}

/// A prototype of the scene.
#[derive(Debug)]
pub struct ProtoDef {
    pub index: usize,
    pub id: String,
    pub glb: Option<PathBuf>,
    pub name: String,
    pub col_mode: CollisionMode,
    pub mesh: u32,
    pub key: u32,
    pub foliage: bool,
    pub ground: bool,
    pub collision: Option<(Vec<f32>, Vec<u32>, Vec<u8>)>,
    pub parts: Vec<PartDef>,
    /// Local box (glTF space) from the GLB's POSITION accessors; `None`: no geometry.
    pub min: Option<[f32; 3]>,
    pub max: Option<[f32; 3]>,
}

/// A world collision soup.
#[derive(Debug)]
pub struct SoupDef {
    /// The first file, relative to the scene folder (for stats).
    pub file: String,
    pub mode: CollisionMode,
    pub ground: bool,
    pub region_cut: bool,
    pub pos: Vec<f32>,
    pub idx: Vec<u32>,
    pub surf: Vec<u8>,
    pub read: i64,
    pub cut: i64,
    pub kept: i64,
}

/// A marker in the writer's frame.
#[derive(Clone, Debug)]
pub struct Marker {
    pub kind: &'static str,
    pub name: Option<String>,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f64,
    pub snap: SnapSpec,
    pub lift: f64,
    pub shelter: bool,
    pub trainer: bool,
    pub blueprint: Option<String>,
    pub transform: Option<[f32; 12]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapMode {
    None,
    Street,
    Ground,
    Open,
}

#[derive(Clone, Debug)]
pub struct SnapSpec {
    pub mode: SnapMode,
    pub above: f32,
    pub from_all: bool,
    pub min_up: f32,
    pub rings: Vec<f32>,
    pub directions: i32,
    pub max_above_ground: f32,
    pub headroom: f32,
}

impl SnapSpec {
    pub fn of(mode: SnapMode) -> SnapSpec {
        SnapSpec {
            mode,
            above: 1.0,
            from_all: false,
            min_up: f32::NAN,
            rings: vec![4.0, 8.0, 12.0, 16.0, 20.0, 26.0],
            directions: 8,
            max_above_ground: 1.5,
            headroom: 3.0,
        }
    }
}

/// A light in the writer's frame.
#[derive(Clone, Debug)]
pub struct PlacedLight {
    pub spot: bool,
    pub pos: Vec3,
    pub dir: Vec3,
    pub tan: Vec3,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub energy: f32,
    pub range: f32,
    pub radius: f32,
    pub spot_size: f32,
    pub spot_blend: f32,
    pub time_of_day: i32,
    pub cast_shadow: bool,
    pub priority: f32,
    pub group: String,
    pub source: String,
}

/// A grind curve in the writer's frame.
#[derive(Clone, Debug)]
pub struct GrindCurve {
    pub points: Vec<Vec3>,
    pub closed: bool,
    pub radius: f32,
    pub surface: usize,
    pub kind: i32,
}

/// A water surface: convex outline (ground frame) at a height, with its box.
#[derive(Clone, Debug)]
pub struct WaterQuad {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub z: f32,
    pub corners: Vec<(f32, f32)>,
}

impl WaterQuad {
    pub fn of(corners: Vec<(f32, f32)>, z: f32) -> WaterQuad {
        // Min/Max over the corners as LINQ does (first element, then comparisons)
        let min = |g: fn(&(f32, f32)) -> f32| {
            corners
                .iter()
                .map(g)
                .reduce(|a, b| if b < a { b } else { a })
                .unwrap_or(0.0)
        };
        let max = |g: fn(&(f32, f32)) -> f32| {
            corners
                .iter()
                .map(g)
                .reduce(|a, b| if b > a { b } else { a })
                .unwrap_or(0.0)
        };
        WaterQuad {
            x0: min(|c| c.0),
            x1: max(|c| c.0),
            y0: min(|c| c.1),
            y1: max(|c| c.1),
            z,
            corners,
        }
    }

    /// The point lies below this surface: inside its (convex) outline and lower.
    pub fn over(&self, x: f32, y: f32, z: f32) -> bool {
        if x < self.x0
            || x > self.x1
            || y < self.y0
            || y > self.y1
            || z.is_nan()
            || self.z.is_nan()
            || z >= self.z
        {
            return false;
        }
        let (mut left, mut right) = (false, false);
        let n = self.corners.len();
        for i in 0..n {
            let (a, b) = (self.corners[i], self.corners[(i + 1) % n]);
            let side = (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
            if side > 0.0 {
                left = true;
            } else if side < 0.0 {
                right = true;
            }
        }
        !(left && right)
    }
}

/// The scene as the writer reads it.
#[derive(Debug)]
pub struct Scene {
    pub dir: PathBuf,
    pub name: String,
    pub display_name: Option<String>,
    pub generator: Option<String>,
    /// Ground frame, `None`: the box of everything.
    pub region: Option<[f64; 4]>,
    pub surfaces: SurfaceTable,
    surf_map: Vec<usize>,
    pub textures: BTreeMap<String, TexDef>,
    /// Material keys in the scene's order, with their entity's component.
    pub materials: HashMap<String, SkateMaterial>,
    pub protos: Vec<ProtoDef>,
    pub placements: Vec<ScenePlacement>,
    pub soups: Vec<SoupDef>,
    pub lights: Vec<PlacedLight>,
    pub curves: Vec<GrindCurve>,
    pub water: Vec<WaterQuad>,
    pub markers: Vec<Marker>,
    pub streaming: Option<[f64; 3]>,
    pub tex_budget: i32,
    pub tex_aux_weight: f32,
    pub min_height: f32,
    pub stub_below: f32,
    pub floor: f32,
    pub max_render_edge: f32,
    pub trainer_note: Option<String>,
    pub light_prefix: Option<String>,
    pub warnings: Vec<String>,
    pub stats: Option<serde_json::Value>,
    pub texture_keys_by_entity: HashMap<primase_core::Entity, String>,
    /// Material key -> its `label` (the material's `core.name`).
    pub labels: HashMap<String, String>,
    pub material_keys_by_entity: HashMap<primase_core::Entity, String>,
    pub report_warnings: Vec<String>,
}

fn class(c: MaterialClass) -> VClass {
    match c {
        MaterialClass::Opaque => VClass::Opaque,
        MaterialClass::Cutout => VClass::Cutout,
        MaterialClass::Decal => VClass::Decal,
        MaterialClass::Glass => VClass::Glass,
        MaterialClass::Water => VClass::Water,
        MaterialClass::Terrain => VClass::Terrain,
        MaterialClass::Leaves => VClass::Leaves,
        MaterialClass::Wood => VClass::Wood,
        MaterialClass::Alpha => VClass::Alpha,
    }
}

fn mode_of(m: Option<SceneMode>) -> CollisionMode {
    match m {
        Some(SceneMode::Water) => CollisionMode::Water,
        Some(SceneMode::Hull) => CollisionMode::Hull,
        _ => CollisionMode::TriangleMesh,
    }
}

fn snap_of(s: &Option<Snap>, no_height: bool) -> SnapSpec {
    let mode = |m: SceneSnap| match m {
        SceneSnap::None => SnapMode::None,
        SceneSnap::Ground => SnapMode::Ground,
        SceneSnap::Street => SnapMode::Street,
        SceneSnap::Open => SnapMode::Open,
    };
    match s {
        None => SnapSpec::of(if no_height {
            SnapMode::Street
        } else {
            SnapMode::None
        }),
        Some(Snap::Mode(m)) => SnapSpec::of(mode(*m)),
        Some(Snap::Object {
            mode: m,
            above,
            from,
            min_up,
            rings,
            directions,
            max_above_ground,
            headroom,
            ..
        }) => {
            let mut spec = SnapSpec::of(mode(*m));
            if let Some(a) = above {
                spec.above = *a as f32;
            }
            spec.from_all = from.as_deref() == Some("all");
            spec.min_up = min_up.map_or(f32::NAN, |v| v as f32);
            if let Some(r) = rings {
                spec.rings = r.clone();
            }
            if let Some(d) = directions {
                spec.directions = *d as i32;
            }
            if let Some(v) = max_above_ground {
                spec.max_above_ground = *v as f32;
            }
            if let Some(v) = headroom {
                spec.headroom = *v as f32;
            }
            spec
        }
    }
}

/// A prototype's local box as the reference reader takes it: per primitive the POSITION accessor's min/max when it
/// has both, else the box of its positions; then the box of the primitives.
fn proto_bounds(g: &primase_gltf::Gltf, mesh: usize) -> Result<([f32; 3], [f32; 3])> {
    let j = g.json();
    let m = g.read_mesh(mesh).map_err(|e| Error::Scene(e.to_string()))?;
    let prims = j
        .get("meshes")
        .and_then(|v| v.get(mesh))
        .and_then(|v| v.get("primitives"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (pi, p) in prims.iter().enumerate() {
        let acc = p
            .get("attributes")
            .and_then(|a| a.get("POSITION"))
            .and_then(|a| a.as_u64())
            .and_then(|a| j.get("accessors").and_then(|v| v.get(a as usize)));
        let get = |k: &str| -> Option<[f32; 3]> {
            let v = acc?.get(k)?.as_array()?;
            if v.len() != 3 {
                return None;
            }
            Some([
                v[0].as_f64()? as f32,
                v[1].as_f64()? as f32,
                v[2].as_f64()? as f32,
            ])
        };
        let (lo, hi) = match (get("min"), get("max")) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => {
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for v in m.primitives[pi].primitive.positions().unwrap_or(&[]) {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
                (lo, hi)
            }
        };
        for k in 0..3 {
            if lo[k] < mn[k] {
                mn[k] = lo[k];
            }
            if hi[k] > mx[k] {
                mx[k] = hi[k];
            }
        }
    }
    Ok((mn, mx))
}

/// A placement record of the scene.
#[derive(Clone, Debug)]
pub struct ScenePlacement {
    pub proto: u32,
    pub variant: u32,
    pub m: [f32; 12],
    pub tile: Option<[i32; 2]>,
    pub radius: Option<f32>,
    pub clip: Option<bool>,
    pub no_collision: bool,
    pub split: bool,
}

impl Scene {
    /// The scene a skate-scene reader attached to `w`, read from `dir`.
    pub fn from_world(world: &World, dir: &std::path::Path) -> Result<Scene> {
        let sc = world
            .resource::<SkateScene>()
            .ok_or_else(|| Error::Scene("the world holds no skate scene".into()))?;
        let mut surfaces = SurfaceTable::default();
        let surf_map: Vec<usize> = match &sc.surfaces {
            Some(list) if !list.is_empty() => list
                .iter()
                .map(|n| match n.strip_prefix("packed:") {
                    Some(p) => surfaces.packed(p.trim().parse::<i32>().unwrap_or(32)),
                    None => STANDARD_SURFACES.iter().position(|s| s.0 == n).unwrap_or(0),
                })
                .collect(),
            _ => (0..STANDARD_SURFACES.len()).collect(),
        };
        let mut textures = BTreeMap::new();
        let mut tex_keys = HashMap::new();
        for (e, t) in world.iter::<SkateTexture>() {
            let path = world
                .get::<TextureFile>(e)
                .ok_or_else(|| Error::Scene(format!("texture '{}' has no file", t.key)))?
                .path
                .clone();
            let name = world.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let src = TexSource::open(&path, t.source_format.as_deref(), t.source_size)?;
            textures.insert(
                t.key.clone(),
                TexDef {
                    key: t.key.clone(),
                    name,
                    stem: t.stem.clone(),
                    linear: t.linear == Some(true),
                    green_down: t.green == Some(NormalGreen::Down),
                    z_rebuild: t.z == Some(NormalZ::Rebuild),
                    src: Arc::new(src),
                },
            );
            tex_keys.insert(e, t.key.clone());
        }
        let mut materials = HashMap::new();
        let mut mat_keys = HashMap::new();
        let mut labels = HashMap::new();
        for (e, m) in world.iter::<SkateMaterial>() {
            materials.insert(m.key.clone(), m.clone());
            mat_keys.insert(e, m.key.clone());
            if let Some(n) = world.get::<Name>(e) {
                labels.insert(m.key.clone(), n.0.clone());
            }
        }
        let mkey = |e: primase_core::Entity| mat_keys.get(&e).cloned();
        let mut protos = Vec::new();
        let mut proto_index = HashMap::new();
        let mut report_warnings = Vec::new();
        for (i, (e, p)) in world.iter::<SkatePrototype>().enumerate() {
            proto_index.insert(e, i);
            let mf = world.get::<MeshFile>(e);
            let variants = world.get::<MaterialVariants>(e);
            let parts: Vec<PartDef> = p
                .parts
                .iter()
                .enumerate()
                .map(|(pi, part)| {
                    let (mut shader, mut bucket, mut model, mut geom) = (String::new(), 0u8, -1i32, -1i32);
                    match &part.source {
                        Some(serde_json::Value::Object(o)) => {
                            shader = o.get("shader").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                            let num = |k: &str, d: f64| o.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
                            bucket = num("bucket", 0.0) as u8;
                            model = num("model", -1.0) as i32;
                            geom = num("geometry", -1.0) as i32;
                        }
                        Some(serde_json::Value::String(s)) => shader = s.clone(),
                        _ => {}
                    }
                    PartDef {
                        material: mf
                            .and_then(|m| m.materials.get(pi).copied().flatten())
                            .and_then(mkey),
                        variants: (part.variants_key
                            || variants.is_some_and(|v| v.0.get(pi).is_some_and(|m| !m.is_empty())))
                        .then(|| {
                            variants
                                .and_then(|v| v.0.get(pi))
                                .map(|m| m.iter().filter_map(|(k, e)| Some((*k, mkey(*e)?))).collect())
                                .unwrap_or_default()
                        }),
                        shader,
                        bucket,
                        model,
                        geom,
                    }
                })
                .collect();
            let (min, max) = match mf {
                Some(mf) => {
                    let bytes = std::fs::read(&mf.path).map_err(|source| Error::Io {
                        path: mf.path.clone(),
                        source,
                    })?;
                    let g = primase_gltf::Gltf::from_glb(&bytes)
                        .map_err(|e| Error::Scene(format!("{}: {e}", mf.path.display())))?;
                    let (a, b) = proto_bounds(&g, mf.mesh as usize)?;
                    (Some(a), Some(b))
                }
                None => (None, None),
            };
            let soup = primase_skate_scene::soup(world, e, Some(sc))
                .map_err(|m| Error::Scene(format!("prototype {} collision: {m}", p.id)))?;
            let collision = match (soup.as_ref(), &p.collision) {
                (Some(s), Some(_)) => Some((
                    s.positions.iter().flatten().copied().collect(),
                    s.triangles.iter().flatten().copied().collect(),
                    s.surfaces
                        .iter()
                        .map(|&b| (surf_map[(b & 0x7F) as usize] as u8) | (b & 0x80))
                        .collect(),
                )),
                _ => None,
            };
            protos.push(ProtoDef {
                index: i,
                id: p.id.clone(),
                glb: mf.map(|m| m.path.clone()),
                name: world.get::<Name>(e).map_or_else(|| p.id.clone(), |n| n.0.clone()),
                col_mode: mode_of(p.collision.as_ref().and_then(|c| c.mode)),
                mesh: mf.map_or(0, |m| m.mesh),
                key: p.key.unwrap_or_else(|| primase_hash::prototype_key(&p.id)),
                foliage: p.foliage == Some(true),
                ground: p.ground == Some(true),
                collision,
                parts,
                min,
                max,
            });
        }
        let mut placements = Vec::new();
        for (e, inst) in world.iter::<Instance>() {
            let t = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
            let sp = world.get::<SkatePlacement>(e).cloned().unwrap_or_default();
            let c = t.cols;
            let m = [
                c[0][0], c[0][1], c[0][2], c[1][0], c[1][1], c[1][2], c[2][0], c[2][1], c[2][2], c[3][0],
                c[3][1], c[3][2],
            ];
            let proto = *proto_index
                .get(&inst.prototype)
                .ok_or_else(|| Error::Scene(format!("placement {e:?} has no prototype")))?
                as u32;
            placements.push(ScenePlacement {
                proto,
                variant: inst.variant,
                m,
                tile: world.get::<Tile>(e).map(|t| [t.col, t.row]),
                radius: sp.radius,
                clip: sp.clip,
                no_collision: sp.no_collision,
                split: sp.split_by_tile,
            });
        }
        let mut soups = Vec::new();
        for (e, c) in world.iter::<SkateCollision>() {
            let s = primase_skate_scene::soup(world, e, Some(sc))
                .map_err(|m| Error::Scene(format!("collision {e:?}: {m}")))?
                .ok_or_else(|| Error::Scene(format!("collision {e:?} has no soup")))?;
            let s = &s;
            soups.push(SoupDef {
                file: c
                    .0
                    .table
                    .files
                    .first()
                    .map(|f| f.path.clone())
                    .unwrap_or_default(),
                mode: mode_of(c.0.mode),
                ground: c.0.ground.unwrap_or(true),
                region_cut: c.0.region_cut.unwrap_or(true),
                pos: s.positions.iter().flatten().copied().collect(),
                idx: s.triangles.iter().flatten().copied().collect(),
                surf: s
                    .surfaces
                    .iter()
                    .map(|&b| (surf_map[(b & 0x7F) as usize] as u8) | (b & 0x80))
                    .collect(),
                read: 0,
                cut: 0,
                kept: 0,
            });
        }
        let lights = world
            .iter::<SkateLight>()
            .map(|(e, l)| PlacedLight {
                spot: l.spot,
                pos: w(l.position[0], l.position[1], l.position[2]),
                dir: w(l.direction[0], l.direction[1], l.direction[2]),
                tan: w(l.tangent[0], l.tangent[1], l.tangent[2]),
                r: l.color[0],
                g: l.color[1],
                b: l.color[2],
                energy: l.energy,
                range: l.range,
                radius: l.radius,
                spot_size: l.spot_size,
                spot_blend: l.spot_blend,
                time_of_day: l.time_of_day as i32,
                cast_shadow: l.cast_shadow,
                priority: l.priority.unwrap_or(l.energy * l.range),
                group: l.group.clone(),
                source: world.get::<Source>(e).map(|s| s.0.clone()).unwrap_or_default(),
            })
            .collect();
        let curves = world
            .iter::<SkateGrindCurve>()
            .map(|(_, c)| GrindCurve {
                points: c.points.iter().map(|p| Vec3::new(p[0], -p[2], p[1])).collect(),
                closed: c.closed,
                radius: if c.radius == 0.0 {
                    0.03
                } else {
                    c.radius.clamp(0.005, 0.25)
                },
                surface: c.surface as usize,
                kind: i32::from(c.kind),
            })
            .collect();
        let water = world
            .iter::<SkateWater>()
            .map(|(_, wa)| {
                WaterQuad::of(
                    wa.corners.iter().map(|c| (c[0], c[1])).collect(),
                    wa.height.unwrap_or(0.0) as f32,
                )
            })
            .collect();
        let mut markers = Vec::new();
        let mut spots = 0;
        for (e, m) in world.iter::<SkateMarker>() {
            let kind = match m.kind {
                MarkerKind::Spawn => "spawn",
                MarkerKind::Travel => "travel",
                MarkerKind::Trainer => "trainer",
                MarkerKind::Vfx => "vfx",
                MarkerKind::TriggerEffect => "trigger_effect",
            };
            let name = world.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_else(|| {
                if kind == "spawn" {
                    "spawn".into()
                } else {
                    format!("Spot {spots}")
                }
            });
            if matches!(kind, "travel" | "trainer") {
                spots += 1;
            }
            let (mut x, mut y, mut z) = (0f32, 0f32, f32::NAN);
            if let Some(([px, pz], py)) = m.position {
                x = px;
                y = -pz;
                z = py.unwrap_or(f32::NAN);
            }
            if let Some(t) = m.m {
                x = t[9];
                y = -t[11];
                if z.is_nan() {
                    z = t[10];
                }
            }
            markers.push(Marker {
                kind,
                name: Some(name),
                x,
                y,
                z,
                yaw: m.yaw.unwrap_or(0.0),
                snap: snap_of(&m.snap, z.is_nan()),
                lift: m.lift.unwrap_or(f64::NAN),
                shelter: m.shelter == Some(true),
                trainer: m.trainer != Some(false),
                blueprint: m.blueprint.clone(),
                transform: m.m,
            });
        }
        let _ = &mut report_warnings;
        Ok(Scene {
            dir: dir.to_owned(),
            name: sc.name.clone(),
            display_name: sc.display_name.clone(),
            generator: sc.generator.clone(),
            region: sc.region.map(|g| g.map(f64::from)),
            surfaces,
            surf_map,
            textures,
            materials,
            protos,
            placements,
            soups,
            lights,
            curves,
            water,
            markers,
            streaming: sc.streaming_distances.as_ref().map(|s| {
                [
                    s.near.unwrap_or(50.0),
                    s.medium.unwrap_or(250.0),
                    s.far.unwrap_or(500.0),
                ]
            }),
            tex_budget: sc
                .texture_budget
                .as_ref()
                .and_then(|t| t.max)
                .map_or(0, |m| m as i32),
            tex_aux_weight: sc
                .texture_budget
                .as_ref()
                .and_then(|t| t.aux_weight)
                .map_or(0.25, |a| a as f32),
            min_height: sc
                .min_height
                .as_ref()
                .map_or(f32::NAN, |m| m.height.unwrap_or(100.01) as f32),
            stub_below: sc
                .min_height
                .as_ref()
                .and_then(|m| m.stub_below)
                .map_or(250.0, |v| v as f32),
            floor: sc
                .min_height
                .as_ref()
                .and_then(|m| m.floor)
                .map_or(f32::NAN, |v| v as f32),
            max_render_edge: sc.max_render_edge.unwrap_or(0.0) as f32,
            trainer_note: sc.trainer_note.clone(),
            light_prefix: sc.light_prefix.clone(),
            warnings: sc.warnings.clone().unwrap_or_default(),
            stats: sc.stats.clone(),
            texture_keys_by_entity: tex_keys,
            labels,
            material_keys_by_entity: mat_keys,
            report_warnings,
        })
    }

    /// A soup surface byte as the writer's: table index, bit 7 natural rock (already mapped when read).
    pub fn surf(&self, b: u8) -> u8 {
        b
    }

    pub fn surf_count(&self) -> usize {
        self.surf_map.len()
    }
}

/// The prototype's primitives as the reference reads them: positions, normals (None: absent), uv (None: absent),
/// indices (absent: sequential, whole triangles).
#[derive(Debug)]
pub struct GlbPrim {
    pub pos: Vec<f32>,
    pub nrm: Option<Vec<f32>>,
    pub uv: Option<Vec<f32>>,
    pub idx: Vec<u32>,
}

pub fn read_glb(path: &std::path::Path, mesh: u32) -> Result<Vec<GlbPrim>> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    let g =
        primase_gltf::Gltf::from_glb(&bytes).map_err(|e| Error::Scene(format!("{}: {e}", path.display())))?;
    let m = g
        .read_mesh(mesh as usize)
        .map_err(|e| Error::Scene(format!("{}: {e}", path.display())))?;
    Ok(m.primitives
        .iter()
        .map(|p| {
            let pr = &p.primitive;
            let pos: Vec<f32> = pr.positions().unwrap_or(&[]).iter().flatten().copied().collect();
            let nv = pos.len() / 3;
            let nrm = match pr.streams.get(&Semantic::Normal) {
                Some(Stream::F32x3(v)) => Some(v.iter().flatten().copied().collect()),
                _ => None,
            };
            let uv = match pr.streams.get(&Semantic::TexCoord(0)) {
                Some(Stream::F32x2(v)) => Some(v.iter().flatten().copied().collect()),
                _ => None,
            };
            let idx = pr
                .indices
                .clone()
                .unwrap_or_else(|| (0..(nv - nv % 3) as u32).collect());
            GlbPrim { pos, nrm, uv, idx }
        })
        .collect())
}

/// Materials of the scene made on first use (the reference's Materializer).
#[derive(Debug)]
pub struct Materializer {
    refs: HashMap<String, Arc<TexRef>>,
    ids: HashMap<(String, bool), usize>,
    derived: HashMap<(String, u32), i32>,
}

impl Default for Materializer {
    fn default() -> Self {
        Self::new()
    }
}

/// The class's defaults for an alpha mode, with that mode.
fn respec(cls: VClass, alpha: i32) -> MaterialSpec<'static> {
    let mut s = MaterialSpec::derived(cls, alpha != 1);
    s.alpha = alpha;
    s
}

impl Materializer {
    pub fn new() -> Self {
        Materializer {
            refs: HashMap::new(),
            ids: HashMap::new(),
            derived: HashMap::new(),
        }
    }

    fn tex_ref(&mut self, t: &TexDef) -> Arc<TexRef> {
        self.refs
            .entry(t.key.clone())
            .or_insert_with(|| {
                Arc::new(TexRef {
                    src: t.src.clone(),
                    key: skate_fnv_string(0xcbf2_9ce4_8422_2325, &t.key),
                    name: t.stem.as_deref().unwrap_or(&t.name).to_lowercase(),
                    has_alpha: true,
                    linear: t.linear,
                })
            })
            .clone()
    }

    /// Whether the material repeats its texture.
    pub fn repeats(scene: &Scene, key: &str) -> bool {
        matches!(
            scene.materials[key].wrap,
            None | Some(Wrap::Repeat) | Some(Wrap::Mirror)
        )
    }

    /// Studio cuts this material's triangles at texture repeats: (a decal, blended with a base texture).
    pub fn cuts(scene: &Scene, key: &str) -> (bool, bool) {
        let m = &scene.materials[key];
        let cls = class(m.class);
        let alpha = match m.alpha {
            Some(AlphaMode::Mask) => 2,
            Some(AlphaMode::Blend) => 3,
            Some(AlphaMode::Opaque) => 1,
            None => MaterialSpec::derived(cls, true).alpha,
        };
        (
            cls == VClass::Decal,
            matches!(cls, VClass::Glass | VClass::Alpha) && alpha == 3 && m.base.is_some(),
        )
    }

    pub fn id(
        &mut self,
        scene: &Scene,
        mats: &mut MatLib,
        tex: &mut TexLib,
        key: &str,
        cutout: bool,
    ) -> Result<usize> {
        if let Some(&id) = self.ids.get(&(key.to_owned(), cutout)) {
            return Ok(id);
        }
        let id = self.make(scene, mats, tex, key, cutout)?;
        self.ids.insert((key.to_owned(), cutout), id);
        Ok(id)
    }

    fn make(
        &mut self,
        scene: &Scene,
        mats: &mut MatLib,
        tex: &mut TexLib,
        key: &str,
        cutout: bool,
    ) -> Result<usize> {
        let e = &scene.materials[key];
        let cls = if cutout { VClass::Cutout } else { class(e.class) };
        let mut alpha = MaterialSpec::derived(cls, true).alpha;
        if cutout {
            alpha = 2;
        } else if let Some(a) = e.alpha {
            alpha = match a {
                AlphaMode::Mask => 2,
                AlphaMode::Blend => 3,
                AlphaMode::Opaque => 1,
            };
        }
        let mut spec = respec(cls, alpha);
        if !cutout {
            if let Some(c) = e.cutoff {
                spec.cutoff = c as f32;
            }
            if let Some(t) = e.two_sided {
                spec.two_sided = t;
            }
            if let Some(d) = e.domain {
                spec.domain = match d {
                    Domain::Decal => 1,
                    Domain::Foliage => 2,
                    Domain::Surface => 0,
                };
            }
        }
        spec.emissive = e.emissive == Some(true);
        spec.roughness = e.roughness;
        spec.metallic = e.metallic;
        if let Some(w) = e.wrap
            && w != Wrap::Repeat
        {
            spec.wrap = Some(w.as_str().to_owned());
        }
        spec.ocean = e.ocean;
        spec.cast_shadows = e.cast_shadows;
        let key_of =
            |te: Option<primase_core::Entity>| te.and_then(|t| scene.texture_keys_by_entity.get(&t)).cloned();
        let base_key = key_of(e.base);
        let base_tex = base_key.as_ref().map(|k| &scene.textures[k]);
        spec.label = Some(
            scene_label(scene, key)
                .or_else(|| base_tex.map(|t| t.name.clone()))
                .unwrap_or_else(|| key.to_owned()),
        );
        let base_ref = base_tex.map(|t| self.tex_ref(t));
        if let Some(bt) = base_tex {
            spec.srgb = !bt.linear;
            if let Some(t) = e.tint {
                spec.tint = u32::from(t[0]) << 16 | u32::from(t[1]) << 8 | u32::from(t[2]);
            }
        } else if let Some(bc) = e.base_color {
            spec.flat = u32::from(bc[0]) << 16 | u32::from(bc[1]) << 8 | u32::from(bc[2]);
            spec.flat_alpha = bc[3];
        }
        let cap_name = base_tex
            .map(|t| t.stem.clone().unwrap_or_else(|| t.name.clone()))
            .unwrap_or_else(|| key.to_owned())
            .to_lowercase();
        if let Some(nk) = key_of(e.normal) {
            spec.normal = self.derived(scene, tex, &nk, &cap_name, NORMAL_TAG)?;
        }
        if let Some(mk) = key_of(e.mask) {
            spec.mask = self.derived(scene, tex, &mk, &cap_name, MASK_TAG)?;
        }
        spec.base = base_ref.as_deref();
        mats.resolve(tex, &spec)
    }

    /// A normal or mask texture at its material's base cap, in Studio's convention.
    fn derived(
        &mut self,
        scene: &Scene,
        tex: &mut TexLib,
        key: &str,
        cap_name: &str,
        tag: u32,
    ) -> Result<i32> {
        let t = &scene.textures[key];
        let cap = tex.cap_for(cap_name);
        if let Some(&id) = self.derived.get(&(key.to_owned(), cap)) {
            return Ok(id);
        }
        let id = match tex.decode(&t.src, cap)? {
            None => -1,
            Some(p) => {
                let mut px = p.rgba;
                if tag == NORMAL_TAG && (t.green_down || t.z_rebuild) {
                    for q in px.as_chunks_mut::<4>().0 {
                        if t.green_down {
                            q[1] = 255 - q[1];
                        }
                        if t.z_rebuild {
                            let x = f32::from(q[0]) / 255.0 * 2.0 - 1.0;
                            let y = f32::from(q[1]) / 255.0 * 2.0 - 1.0;
                            let z = ((0f32.max(1.0 - x * x - y * y)).sqrt() * 0.5 + 0.5) * 255.0;
                            // to the nearest level, half to even: the rebuilt blue channel's definition
                            q[2] = (z.round_ties_even() as i32).clamp(0, 255) as u8;
                        }
                        q[3] = 255;
                    }
                }
                let name = t.stem.as_deref().unwrap_or(&t.name).to_lowercase();
                tex.register_pixels(
                    &name,
                    tag,
                    p.w,
                    p.h,
                    &px,
                    &t.src,
                    if tag == NORMAL_TAG { 0x8080FF } else { 0x00FF00 },
                )
            }
        };
        self.derived.insert((key.to_owned(), cap), id);
        Ok(id)
    }
}

/// A material's `label` (its `core.name` in the world).
fn scene_label(scene: &Scene, key: &str) -> Option<String> {
    scene.labels.get(key).cloned()
}

/// Builds a prototype from its GLB (the reference's Meshes.Build).
pub fn build_mesh(
    scene: &Scene,
    p: &ProtoDef,
    mat: &mut Materializer,
    mats: &mut MatLib,
    tex: &mut TexLib,
    counters: &mut MeshCounters,
) -> Result<Option<SrcMesh>> {
    let Some(glb) = &p.glb else { return Ok(None) };
    let prims = read_glb(glb, p.mesh)?;
    let has_variants = p.parts.iter().any(|x| x.variants.is_some());
    let mut mesh = SrcMesh {
        hash: p.key,
        name: p.name.clone(),
        is_tree: p.foliage,
        has_variants,
        ..SrcMesh::default()
    };
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (i, g) in prims.iter().enumerate() {
        let d = &p.parts[i];
        let nv = g.pos.len() / 3;
        let mut part = SrcPart {
            shader: d.shader.clone(),
            bucket: d.bucket,
            model: d.model,
            geom: d.geom,
            def: Some((p.index, i)),
            ..SrcPart::default()
        };
        part.pos = vec![0.0; nv * 3];
        part.nrm = vec![0.0; nv * 3];
        part.uv = g.uv.clone().unwrap_or_else(|| vec![0.0; nv * 2]);
        part.idx = g.idx.clone();
        for v in 0..nv {
            let q = w_at(&g.pos, v * 3);
            part.pos[v * 3] = q.x;
            part.pos[v * 3 + 1] = q.y;
            part.pos[v * 3 + 2] = q.z;
            mn = mn.min(q);
            mx = mx.max(q);
        }
        let mut verts = nv;
        if let Some(n) = &g.nrm {
            for v in 0..nv {
                let q = w_at(n, v * 3);
                let r = primase_vertex::renormalize_stored_normal([q.x, q.y, q.z])
                    .map_err(|e| Error::Scene(e.to_string()))?;
                part.nrm[v * 3..v * 3 + 3].copy_from_slice(&r);
            }
        } else {
            flat_normals(&mut part)?;
            verts = part.nverts();
        }
        // the UV-repeat rule: decal triangles over too many repeats left out; a blended surface with such a
        // triangle alpha tested instead
        let keys: Vec<String> = d
            .variants
            .iter()
            .flat_map(|v| v.values().cloned())
            .chain(d.material.clone())
            .collect();
        if keys
            .iter()
            .all(|k| Materializer::cuts(scene, k).0 && Materializer::repeats(scene, k))
        {
            let mut kept = Vec::with_capacity(part.idx.len());
            for t in part.idx.as_chunks::<3>().0 {
                if !over(&part.uv, t)? {
                    kept.extend_from_slice(t);
                }
            }
            if kept.len() != part.idx.len() {
                counters.repeat_dropped += ((part.idx.len() - kept.len()) / 3) as i64;
                part.idx = kept;
            }
        }
        let mut cutout = false;
        if keys
            .iter()
            .any(|k| Materializer::cuts(scene, k).1 && Materializer::repeats(scene, k))
        {
            for t in part.idx.as_chunks::<3>().0 {
                if over(&part.uv, t)? {
                    cutout = true;
                    break;
                }
            }
        }
        if cutout {
            counters.repeat_masked += 1;
        }
        part.cutout = cutout;
        if d.variants.is_none() {
            let mk = d
                .material
                .as_deref()
                .ok_or_else(|| Error::Scene(format!("prototype '{}' part {i}: no material", p.id)))?;
            let cut = cutout && Materializer::cuts(scene, mk).1;
            part.material = Some(mat.id(scene, mats, tex, mk, cut)?);
        }
        mesh.tris += (part.idx.len() / 3) as i64;
        mesh.verts += verts as i64;
        mesh.bytes += (part.pos.len() * 4 * 2 + part.uv.len() * 4 + part.idx.len() * 4) as i64;
        mesh.parts.push(part);
    }
    if mesh.tris == 0 {
        mn = Vec3::ZERO;
        mx = Vec3::ZERO;
    }
    mesh.min = mn;
    mesh.max = mx;
    if let Some((pos, idx, surf)) = &p.collision {
        let mut cp = vec![0f32; pos.len()];
        for v in (0..pos.len()).step_by(3) {
            let q = w_at(pos, v);
            cp[v] = q.x;
            cp[v + 1] = q.y;
            cp[v + 2] = q.z;
        }
        mesh.col_pos = Some(cp);
        mesh.col_idx = if idx.is_empty() { None } else { Some(idx.clone()) };
        mesh.col_surf = surf.clone();
        mesh.col_mode = p.col_mode;
        mesh.col_ground = p.ground;
    }
    Ok(Some(mesh))
}

fn over(uv: &[f32], t: &[u32]) -> Result<bool> {
    let c = |i: u32| [uv[i as usize * 2], uv[i as usize * 2 + 1]];
    Ok(
        primase_meshops::uv_tiles([c(t[0]), c(t[1]), c(t[2])]).map_err(|e| Error::Scene(e.to_string()))?
            > primase_meshops::MAX_UV_TILES,
    )
}

fn flat_normals(p: &mut SrcPart) -> Result<()> {
    let m = primase_meshops::Mesh {
        positions: p
            .pos
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
        normals: Vec::new(),
        uv: p.uv.as_chunks::<2>().0.iter().map(|c| [c[0], c[1]]).collect(),
        indices: p.idx.clone(),
    };
    let f = primase_meshops::flat_normals(&m).map_err(|e| Error::Scene(e.to_string()))?;
    p.pos = f.positions.iter().flatten().copied().collect();
    p.nrm = f.normals.iter().flatten().copied().collect();
    p.uv = f.uv.iter().flatten().copied().collect();
    p.idx = f.indices;
    Ok(())
}

/// Counts of the UV-repeat rule.
#[derive(Clone, Debug, Default)]
pub struct MeshCounters {
    pub failed: i64,
    pub repeat_dropped: i64,
    pub repeat_masked: i64,
}

/// The material of a part for a placement's variant row (the reference's MaterialOf).
pub fn material_of(
    scene: &Scene,
    part: &SrcPart,
    row: u32,
    mat: &mut Materializer,
    mats: &mut MatLib,
    tex: &mut TexLib,
) -> Result<usize> {
    if let Some(m) = part.material {
        return Ok(m);
    }
    let (pi, i) = part.def.expect("variant part");
    let d = &scene.protos[pi].parts[i];
    let key = d
        .variants
        .as_ref()
        .and_then(|v| v.get(&row))
        .or(d.material.as_ref())
        .ok_or_else(|| {
            Error::Scene(format!(
                "prototype '{}' part {i}: no material for variant {row}",
                scene.protos[pi].id
            ))
        })?
        .clone();
    let cut = part.cutout && Materializer::cuts(scene, &key).1;
    mat.id(scene, mats, tex, &key, cut)
}

/// Hand out the kinds' name.
pub fn class_of(c: MaterialClass) -> VClass {
    class(c)
}
