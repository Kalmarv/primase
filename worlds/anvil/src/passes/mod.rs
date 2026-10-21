//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use std::collections::BTreeMap;

mod bake;
mod decals;
pub mod formulas;
mod material;
mod splat;
mod water;
mod world_uv;
pub use bake::{BakeAnvilMaterial, gbuffer_pixel_key};
pub use decals::{Family, FamilyRule, LowerAnvilDecals, TerrainDecals, family};
pub use material::{AlphaChoice, Emission, LowerAnvilMaterial, ShaderUv, fake_river_albedo, world_uv_rows};
pub use splat::{Splat, SplatUv};
pub use water::{LowerAnvilWater, Shore};
pub use world_uv::LowerAnvilWorldUv;

use primase_core::layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::world::Resource;
use primase_core::{Collider, Shape};
use primase_core::{
    Collision, ColorSpace, Component, Entity, Instance, LodForm, Lods, Mesh, NormalBasis, NormalConvention,
    NormalGreen, NormalX, NormalZ, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole, Texture,
    Topology, Transform, World,
};

use crate::components::{
    AnvilCollisionShape, AnvilDataLayers, AnvilFloorHeight, AnvilLods, AnvilMesh, AnvilMeshShape,
    AnvilTerrain, AnvilTextureMap, CollisionMaterialRef, ShapeGeometry, TerrainTile, UvWordsAt,
};

/// What names a lowered collision surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceBy {
    /// Each face's (or primitive's) CollisionMaterial, by its entry's name; `surface` where it has none, or its name
    /// isn't read.
    CollisionMaterial,
    /// `surface`, for everything.
    One,
}

/// `lower-anvil-collision{by=collision-material|one,surface=NAME}`: every MeshShape's geometry ([`AnvilMeshShape`])
/// becomes `core.collision` on its prototype, each triangle of the surface its CollisionMaterial's entry name names
/// (`by=collision-material`; `surface` where it has none) or every triangle of `surface` (`by=one`). A surface named
/// so is unclassified: what a CollisionMaterial is to physics isn't read. The geometry is consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilCollision {
    pub by: SurfaceBy,
    pub surface: String,
}

impl LowerAnvilCollision {
    /// From its options: `by` and `surface`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilCollision, String> {
        if let Some(k) = o.keys().find(|k| !["by", "surface"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let by = match o.get("by").map(String::as_str) {
            Some("collision-material") => SurfaceBy::CollisionMaterial,
            Some("one") => SurfaceBy::One,
            Some(v) => return Err(format!("by={v} is not collision-material or one")),
            None => return Err("by=collision-material|one is required".to_owned()),
        };
        let surface = o
            .get("surface")
            .filter(|s| !s.is_empty())
            .ok_or("surface=NAME is required: the surface of what has no CollisionMaterial (or every surface, by=one)")?;
        Ok(LowerAnvilCollision {
            by,
            surface: surface.clone(),
        })
    }

    /// The surface name for a CollisionMaterial (or none).
    fn name<'m>(&'m self, material: Option<&'m CollisionMaterialRef>) -> &'m str {
        match (self.by, material.and_then(|m| m.name.as_deref())) {
            (SurfaceBy::CollisionMaterial, Some(name)) => name,
            _ => &self.surface,
        }
    }
}

impl Pass for LowerAnvilCollision {
    fn name(&self) -> &'static str {
        "lower-anvil-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("by".to_owned(), by_text(self.by).to_owned()),
            ("surface".to_owned(), self.surface.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collision::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMeshShape::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let shapes: Vec<Entity> = w.iter::<AnvilMeshShape>().map(|(e, _)| e).collect();
        if shapes.is_empty() {
            return Ok(());
        }
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        for e in shapes {
            let Some(shape) = w.remove::<AnvilMeshShape>(e) else {
                continue;
            };
            // each stored material's surface, then each face's
            let named = shape
                .materials
                .iter()
                .map(|m| surfaces.index(self.name(m.as_ref())).map_err(|e| e.to_string()))
                .collect::<Result<Vec<u16>, String>>()?;
            let fallback = surfaces.index(&self.surface).map_err(|e| e.to_string())?;
            let triangles = shape.faces.len();
            let face_surfaces: Vec<u16> = (0..triangles)
                .map(|i| {
                    let k = shape.face_materials.get(i).map_or(0, |&k| usize::from(k));
                    named.get(k).copied().unwrap_or(fallback)
                })
                .collect();
            for i in 0..triangles {
                let k = shape.face_materials.get(i).map_or(0, |&k| usize::from(k));
                let name = self.name(shape.materials.get(k).and_then(Option::as_ref));
                log.count(&format!("collision triangles of surface {name}"), 1);
            }
            w.insert(
                e,
                Collision {
                    positions: shape.positions,
                    triangles: shape.faces,
                    surfaces: face_surfaces,
                },
            );
            log.count("MeshShapes lowered to collision", 1);
        }
        w.insert_resource(surfaces);
        Ok(())
    }
}

/// `lower-anvil-shapes{by=collision-material|one,surface=NAME}`: every SphereShape, CapsuleShape and BoxShape
/// ([`AnvilCollisionShape`]) becomes a core collider of that shape, of the surface its CollisionMaterial's entry name
/// names or `surface`, as for `lower-anvil-collision`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilShapes {
    pub by: SurfaceBy,
    pub surface: String,
}

impl LowerAnvilShapes {
    /// From its options: `by` and `surface`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilShapes, String> {
        LowerAnvilCollision::from_options(o).map(|c| LowerAnvilShapes {
            by: c.by,
            surface: c.surface,
        })
    }
}

/// `by`'s option text.
fn by_text(by: SurfaceBy) -> &'static str {
    match by {
        SurfaceBy::CollisionMaterial => "collision-material",
        SurfaceBy::One => "one",
    }
}

impl Pass for LowerAnvilShapes {
    fn name(&self) -> &'static str {
        "lower-anvil-shapes"
    }
    fn options(&self) -> Options {
        Options::from([
            ("by".to_owned(), by_text(self.by).to_owned()),
            ("surface".to_owned(), self.surface.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilCollisionShape::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Collider::NAME, Surfaces::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilCollisionShape::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let shapes: Vec<Entity> = w.iter::<AnvilCollisionShape>().map(|(e, _)| e).collect();
        if shapes.is_empty() {
            return Ok(());
        }
        let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
        let namer = LowerAnvilCollision {
            by: self.by,
            surface: self.surface.clone(),
        };
        for e in shapes {
            let Some(s) = w.remove::<AnvilCollisionShape>(e) else {
                continue;
            };
            let name = namer.name(s.material.as_ref());
            let surface = surfaces.index(name).map_err(|e| e.to_string())?;
            log.count(&format!("collision shapes of surface {name}"), 1);
            let (shape, what) = match s.geometry {
                ShapeGeometry::Sphere { centre, radius } => (
                    Shape::Sphere {
                        center: centre,
                        radius,
                    },
                    "spheres",
                ),
                ShapeGeometry::Capsule { a, b, radius } => (Shape::Capsule { a, b, radius }, "capsules"),
                ShapeGeometry::Box { half_extents } => (
                    Shape::Box {
                        center: [0.0; 3],
                        half_extents,
                    },
                    "boxes",
                ),
            };
            w.insert(e, Collider { shape, surface });
            log.count(&format!("collision {what} lowered"), 1);
        }
        w.insert_resource(surfaces);
        Ok(())
    }
}

/// How a placement's several DataLayerActions combine: the program's stated choice (the Riders Republic notes read
/// them as all holding, from Entities carrying two event layers that appear only in both; not established).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    All,
    Any,
}

/// `lower-anvil-data-layers{combine=all|any}`: DataLayers become world state. Each DataLayer a placement's actions
/// name becomes a global on/off axis (key `anvil.datalayer.<its name>`, or its ClassID in hex where unnamed; class
/// `datalayer`; no default: which layers a world has on isn't read), and each placement with actions a condition: an
/// action whose u32 is 0 holds while its layer is on, 1 while it is off (the Riders Republic notes' reading, from one
/// layer: Back2School's 1-objects are the furniture its 0-objects replace; docs/world.md §9); another u32 is refused.
/// `resolve-state` then makes the program's world of them. The actions are consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilDataLayers {
    pub combine: Combine,
}

impl LowerAnvilDataLayers {
    /// From its options: `combine`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilDataLayers, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "combine") {
            return Err(format!("unknown option {k}"));
        }
        let combine = match o.get("combine").map(String::as_str) {
            Some("all") => Combine::All,
            Some("any") => Combine::Any,
            Some(v) => return Err(format!("combine={v} is not all or any")),
            None => {
                return Err("combine=all|any is required: how a placement's several actions combine isn't established".into());
            }
        };
        Ok(LowerAnvilDataLayers { combine })
    }
}

impl Pass for LowerAnvilDataLayers {
    fn name(&self) -> &'static str {
        "lower-anvil-data-layers"
    }
    fn options(&self) -> Options {
        let combine = match self.combine {
            Combine::All => "all",
            Combine::Any => "any",
        };
        Options::from([("combine".to_owned(), combine.to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilDataLayers::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[StateAxis::NAME, Condition::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilDataLayers::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let placements: Vec<Entity> = w.iter::<AnvilDataLayers>().map(|(e, _)| e).collect();
        let mut axes: BTreeMap<u64, Entity> = BTreeMap::new();
        for e in placements {
            let Some(layers) = w.remove::<AnvilDataLayers>(e) else {
                continue;
            };
            let mut literals = Vec::with_capacity(layers.actions.len());
            for a in &layers.actions {
                let values = match a.word {
                    0 => 0b10,
                    1 => 0b01,
                    word => {
                        return Err(format!(
                            "DataLayer {:#x}: an action's u32 {word}, of which only 0 and 1 are read",
                            a.layer
                        ));
                    }
                };
                let axis = match axes.get(&a.layer) {
                    Some(&axis) => axis,
                    None => {
                        let axis = w.spawn();
                        let label = a.name.clone().unwrap_or_else(|| format!("{:x}", a.layer));
                        w.insert(
                            axis,
                            StateAxis {
                                key: format!("anvil.datalayer.{label}"),
                                domain: Domain::Bool,
                                default: None,
                                scope: Scope::Global,
                                class: "datalayer".into(),
                                source: format!("DataLayer {:#x}", a.layer),
                            },
                        );
                        log.count("DataLayer axes", 1);
                        axes.insert(a.layer, axis);
                        axis
                    }
                };
                literals.push(Literal { axis, values });
            }
            let condition = match self.combine {
                Combine::All => Condition(vec![Clause(literals)]),
                Combine::Any => Condition(literals.into_iter().map(|l| Clause(vec![l])).collect()),
            };
            w.insert(e, condition);
            log.count("placements given a DataLayer condition", 1);
        }
        Ok(())
    }
}

/// Whether `lower-anvil-mesh` reads normals from each vertex's bytes 8..11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshNormals {
    /// No normals (the writer's or viewer's flat shading).
    Omit,
    /// Bytes 8, 9 and 10 each read as `b / 127.5 - 1`, normalized, converted through the reader's frame.
    Packed,
    /// Bytes 8, 9 and 10 each read as `(b - 127) / 127`, normalized, converted through the reader's frame. (Against
    /// area-weighted face normals of the positions, this reading's median angle is 0.45 to 0.50 degrees on static
    /// 24-byte meshes of Riders Republic, against 0.72 to 0.79 for `Packed`, by a separate measurement; neither is
    /// the game's own decode.)
    Packed127,
}

/// How `lower-anvil-mesh` reads the two 16-bit words of each vertex's bytes 20..24.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvWords {
    /// Each as unsigned.
    Unsigned,
    /// Each as signed (two's complement).
    Signed,
}

/// Whether `lower-anvil-mesh` writes the vertex colour, and in which channel order its four bytes are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshColour {
    Omit,
    /// Bytes 16, 17, 18, 19 as R, G, B, A.
    Rgba,
    /// As B, G, R, A.
    Bgra,
}

/// What `lower-anvil-mesh` makes of the second u16 of each vertex's bytes 20..24.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvV {
    /// v as stored, over the divisor.
    AsStored,
    /// 1 minus that (a bottom-left origin).
    OneMinus,
}

/// `lower-anvil-mesh{uv-words=unsigned|signed,uv-divisor=N,uv-v=as-stored|one-minus,uv-tool=omit|read,colour=omit|rgba|bgra,normals=omit|packed|packed-127}`:
/// a diagnostic lowering. Every Mesh's buffers ([`AnvilMesh`]) become a core mesh on its prototype: one triangle
/// primitive with no material, or, where the reader read its Materials, a primitive per stored range, of the vertices
/// its triangles use, drawn with the range's Material entity; positions and triangles as read; normals per `normals`.
/// Texture coordinates 0: each vertex's two words where its layout keeps them ([`UvWordsAt`]), read per `uv-words`
/// over `uv-divisor`, the second per `uv-v`. The words of kinds 2 and 3 (bytes 20..24) are lowered always; those
/// whose place is RRAssetStudio's reading alone (kind 11's bytes 12..16, the split layouts' primary bytes 4..8) as
/// `uv-tool` states (`omit` leaves those meshes without texture coordinates, reported). Colour 0: the vertex colour
/// ([`AnvilMesh`]'s `colour`, kind 2's) in the channel order `colour` states, as UNORM8 (`omit`: none). (RRAssetStudio reads the
/// words as signed over 2048; read so, the triangles of 196 meshes spanning more than 16 units drop from 10,773 to 650
/// against unsigned, by a separate count; neither proves what the game does.) The words' divisor and origin, and that
/// bytes 8..11 are a normal, are not established: each reading is the program's stated choice, every lowered mesh
/// reports it as an approximation, and the output is not evidence of the format. The buffers are consumed.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMesh {
    pub uv_words: UvWords,
    pub uv_divisor: f32,
    pub uv_v: UvV,
    /// Whether the texture coordinates whose place is a tool's reading alone are lowered.
    pub uv_tool: bool,
    pub colour: MeshColour,
    pub normals: MeshNormals,
}

impl LowerAnvilMesh {
    /// From its options, every one required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilMesh, String> {
        if let Some(k) = o.keys().find(|k| {
            !["uv-words", "uv-divisor", "uv-v", "uv-tool", "colour", "normals"].contains(&k.as_str())
        }) {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).map(String::as_str).ok_or_else(|| {
                format!(
                    "{k} is required: uv-words=unsigned|signed, uv-divisor=N, uv-v=as-stored|one-minus, uv-tool=omit|read, colour=omit|rgba|bgra, normals=omit|packed|packed-127"
                )
            })
        };
        let uv_words = match get("uv-words")? {
            "unsigned" => UvWords::Unsigned,
            "signed" => UvWords::Signed,
            v => return Err(format!("uv-words={v} is not unsigned or signed")),
        };
        let uv_divisor: f32 = get("uv-divisor")?
            .parse()
            .map_err(|_| "uv-divisor is a number".to_owned())?;
        if !(uv_divisor.is_finite() && uv_divisor > 0.0) {
            return Err(format!("uv-divisor {uv_divisor}: a positive number"));
        }
        let uv_v = match get("uv-v")? {
            "as-stored" => UvV::AsStored,
            "one-minus" => UvV::OneMinus,
            v => return Err(format!("uv-v={v} is not as-stored or one-minus")),
        };
        let uv_tool = match get("uv-tool")? {
            "omit" => false,
            "read" => true,
            v => return Err(format!("uv-tool={v} is not omit or read")),
        };
        let colour = match get("colour")? {
            "omit" => MeshColour::Omit,
            "rgba" => MeshColour::Rgba,
            "bgra" => MeshColour::Bgra,
            v => return Err(format!("colour={v} is not omit, rgba or bgra")),
        };
        let normals = match get("normals")? {
            "omit" => MeshNormals::Omit,
            "packed" => MeshNormals::Packed,
            "packed-127" => MeshNormals::Packed127,
            v => return Err(format!("normals={v} is not omit, packed or packed-127")),
        };
        Ok(LowerAnvilMesh {
            uv_words,
            uv_divisor,
            uv_v,
            uv_tool,
            colour,
            normals,
        })
    }
}

impl Pass for LowerAnvilMesh {
    fn name(&self) -> &'static str {
        "lower-anvil-mesh"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "uv-words".to_owned(),
                match self.uv_words {
                    UvWords::Unsigned => "unsigned",
                    UvWords::Signed => "signed",
                }
                .to_owned(),
            ),
            ("uv-divisor".to_owned(), self.uv_divisor.to_string()),
            (
                "uv-v".to_owned(),
                match self.uv_v {
                    UvV::AsStored => "as-stored",
                    UvV::OneMinus => "one-minus",
                }
                .to_owned(),
            ),
            (
                "uv-tool".to_owned(),
                if self.uv_tool { "read" } else { "omit" }.to_owned(),
            ),
            (
                "colour".to_owned(),
                match self.colour {
                    MeshColour::Omit => "omit",
                    MeshColour::Rgba => "rgba",
                    MeshColour::Bgra => "bgra",
                }
                .to_owned(),
            ),
            (
                "normals".to_owned(),
                match self.normals {
                    MeshNormals::Omit => "omit",
                    MeshNormals::Packed => "packed",
                    MeshNormals::Packed127 => "packed-127",
                }
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "texture coordinates and normals from stated readings of bytes whose meaning isn't established"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<AnvilMesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let Some(m) = w.remove::<AnvilMesh>(e) else {
                continue;
            };
            let mut streams = BTreeMap::new();
            let tool = matches!(m.uv_at, UvWordsAt::Bytes12 | UvWordsAt::Primary4);
            if tool && !self.uv_tool && !m.uv_words.is_empty() {
                log.omit(
                    &format!(
                        "texture coordinates: {:?} (kind {} stride {}) not lowered (a tool's reading; uv-tool=omit)",
                        m.uv_at, m.kind, m.stride
                    ),
                    1,
                );
            } else if !m.uv_words.is_empty() && m.uv_words.len() == m.positions.len() {
                let uv = m
                    .uv_words
                    .iter()
                    .map(|&[u, v]| {
                        let word = |x: u16| match self.uv_words {
                            UvWords::Unsigned => f32::from(x),
                            UvWords::Signed => f32::from(x.cast_signed()),
                        };
                        let (u, v) = (word(u) / self.uv_divisor, word(v) / self.uv_divisor);
                        [u, if self.uv_v == UvV::OneMinus { 1.0 - v } else { v }]
                    })
                    .collect();
                streams.insert(Semantic::TexCoord(0), Stream::F32x2(uv));
                log.approximate(
                    &format!(
                        "texture coordinates: {:?} as two {:?} words over {} (a stated reading), v {:?}",
                        m.uv_at, self.uv_words, self.uv_divisor, self.uv_v
                    ),
                    1,
                );
            } else {
                log.omit(
                    &format!(
                        "texture coordinates: not read for kind {} stride {} (where they are isn't established)",
                        m.kind, m.stride
                    ),
                    1,
                );
            }
            match self.normals {
                MeshNormals::Packed | MeshNormals::Packed127 if m.kind == 2 && m.stride == 24 => {
                    let decode = |x: u8| match self.normals {
                        MeshNormals::Packed127 => (f32::from(x) - 127.0) / 127.0,
                        _ => f32::from(x) / 127.5 - 1.0,
                    };
                    let normals = m
                        .vertex_records
                        .as_chunks::<24>()
                        .0
                        .iter()
                        .map(|r| {
                            let b = [r[8], r[9], r[10]];
                            let d = [b[0], b[1], b[2]].map(decode);
                            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                            let d = if len > 0.0 {
                                d.map(|x| x / len)
                            } else {
                                [0.0, 1.0, 0.0]
                            };
                            m.frame.direction(d)
                        })
                        .collect();
                    streams.insert(Semantic::Normal, Stream::F32x3(normals));
                    log.approximate(
                        &format!(
                            "normals: bytes 8..11 read as a normal, {:?} (stated)",
                            self.normals
                        ),
                        1,
                    );
                }
                MeshNormals::Packed | MeshNormals::Packed127 => log.omit(
                    &format!(
                        "normals: not read for kind {} stride {} (where they are isn't established)",
                        m.kind, m.stride
                    ),
                    1,
                ),
                MeshNormals::Omit => log.count("meshes lowered without normals (stated)", 1),
            }
            if self.colour != MeshColour::Omit && m.colour.len() == m.positions.len() && !m.colour.is_empty()
            {
                let order = |[a, b, c, d]: [u8; 4]| match self.colour {
                    MeshColour::Bgra => [c, b, a, d],
                    _ => [a, b, c, d],
                };
                streams.insert(
                    Semantic::Color(0),
                    Stream::U8x4(m.colour.iter().map(|&c| order(c)).collect()),
                );
                log.approximate(
                    &format!(
                        "vertex colour: bytes 16..20 as UNORM8, {:?} (the order a stated reading)",
                        self.colour
                    ),
                    1,
                );
            }
            let triangles = m.triangles.len() as u64;
            streams.insert(Semantic::Position, Stream::F32x3(m.positions));
            let primitives = if m.material_entities.is_empty() {
                vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(m.triangles.into_iter().flatten().collect()),
                    material: None,
                }]
            } else {
                // a primitive per range, of its own vertices, drawn with its Material
                let ends = m.ranges.iter().skip(1).copied().chain([m.triangles.len() as u32]);
                m.ranges
                    .iter()
                    .zip(ends)
                    .zip(&m.material_entities)
                    .map(|((&start, end), &material)| {
                        let tris = m.triangles.get(start as usize..end as usize).unwrap_or_default();
                        let mut keep: Vec<u32> = tris.iter().flatten().copied().collect();
                        keep.sort_unstable();
                        keep.dedup();
                        let new: BTreeMap<u32, u32> = (0u32..).zip(&keep).map(|(i, &v)| (v, i)).collect();
                        Primitive {
                            topology: Topology::Triangles,
                            streams: streams.iter().map(|(s, v)| (*s, gather(v, &keep))).collect(),
                            indices: Some(tris.iter().flatten().map(|v| new[v]).collect()),
                            material,
                        }
                    })
                    .collect()
            };
            log.count("primitives lowered", primitives.len() as u64);
            w.insert(e, Mesh::new(primitives));
            log.count("Meshes lowered", 1);
            log.count("triangles lowered", triangles);
        }
        Ok(())
    }
}

/// The elements of `stream` at `keep`, in that order.
fn gather(stream: &Stream, keep: &[u32]) -> Stream {
    fn pick<T: Copy>(v: &[T], keep: &[u32]) -> Vec<T> {
        keep.iter().filter_map(|&i| v.get(i as usize).copied()).collect()
    }
    match stream {
        Stream::F32x2(v) => Stream::F32x2(pick(v, keep)),
        Stream::F32x3(v) => Stream::F32x3(pick(v, keep)),
        Stream::F32x4(v) => Stream::F32x4(pick(v, keep)),
        Stream::U8x4(v) => Stream::U8x4(pick(v, keep)),
        Stream::U16x4(v) => Stream::U16x4(pick(v, keep)),
    }
}

/// `lower-anvil-lods`: every prototype's mesh levels ([`AnvilLods`]) become `core.lods`: its own mesh at its level,
/// each other level a form, no default (the game chooses a level by distance, which isn't read). Run after
/// `lower-anvil-mesh`, so every form holds a core mesh. The levels are consumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerAnvilLods;

impl Pass for LowerAnvilLods {
    fn name(&self) -> &'static str {
        "lower-anvil-lods"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilLods::NAME, Mesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Lods::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilLods::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let prototypes: Vec<Entity> = w.iter::<AnvilLods>().map(|(e, _)| e).collect();
        for e in prototypes {
            let Some(l) = w.remove::<AnvilLods>(e) else {
                continue;
            };
            let forms = l
                .forms
                .iter()
                .map(|&(detail, form)| LodForm { detail, form })
                .collect::<Vec<_>>();
            log.count("prototypes given levels of detail", 1);
            log.count("level-of-detail forms", forms.len() as u64);
            w.insert(
                e,
                Lods {
                    detail: l.detail,
                    forms,
                    default: None,
                },
            );
        }
        Ok(())
    }
}

/// Where in its cell a FloorHeightData word's sample lies: the program's stated choice, as a half-cell shift isn't
/// detectable in the Riders Republic notes' fit (it changes the residual by 0.1 m).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellPoint {
    /// At the cell's corner: x = origin x + side x column.
    Corner,
    /// At its centre, half a side further along each axis.
    Centre,
}

/// A ground lowering's stated choices: its rectangle, step, height offset and where a sample lies in its cell.
struct Ground {
    rect: [f64; 4],
    step: u32,
    offset: f64,
    sample: CellPoint,
}

/// `o`'s ground options, `rect`, `step`, `offset` and `sample`, all required, besides which only `extra` names are
/// taken; `usage` lists them for a missing one.
fn ground_options(o: &Options, extra: &[&str], usage: &str) -> Result<Ground, String> {
    if let Some(k) = o
        .keys()
        .find(|k| !["rect", "step", "offset", "sample"].contains(&k.as_str()) && !extra.contains(&k.as_str()))
    {
        return Err(format!("unknown option {k}"));
    }
    let get = |k: &str| {
        o.get(k)
            .map(String::as_str)
            .ok_or_else(|| format!("{k} is required: {usage}"))
    };
    let rect: Vec<f64> = get("rect")?
        .split(',')
        .map(|v| {
            v.trim()
                .parse::<f64>()
                .map_err(|_| format!("rect: {v} is not a number"))
        })
        .collect::<Result<_, _>>()?;
    let [x0, y0, x1, y1] = rect[..] else {
        return Err("rect takes four numbers, x0,y0,x1,y1".to_owned());
    };
    if !(x0.is_finite() && y0.is_finite() && x1 > x0 && y1 > y0) {
        return Err(format!("rect {x0},{y0},{x1},{y1}: x0 < x1 and y0 < y1"));
    }
    let step: u32 = get("step")?
        .parse()
        .map_err(|_| "step is a whole number".to_owned())?;
    if step == 0 {
        return Err("step is at least 1".to_owned());
    }
    let offset: f64 = get("offset")?
        .parse()
        .map_err(|_| "offset is a number".to_owned())?;
    if !offset.is_finite() {
        return Err(format!("offset {offset}: a finite number"));
    }
    let sample = match get("sample")? {
        "corner" => CellPoint::Corner,
        "centre" => CellPoint::Centre,
        v => return Err(format!("sample={v} is not corner or centre")),
    };
    Ok(Ground {
        rect: [x0, y0, x1, y1],
        step,
        offset,
        sample,
    })
}

/// `lower-anvil-floor`: every FloorHeightData grid ([`AnvilFloorHeight`]) becomes ground: a core mesh of the cells
/// whose samples lie in `rect` (x0, y0, x1, y1, in the grid's own units), every `step`-th column and row, two
/// triangles to each square of samples, placed once. A sample's height is the trailer's scale times its word plus
/// `offset`, the word `lo + (hi - lo) index / 3` computed exactly. Every option is required: the offset isn't stored
/// (the notes fit 925 to 928 m), and how the game rounds the word isn't read (under one word step either way).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilFloor {
    pub rect: [f64; 4],
    pub step: u32,
    pub offset: f64,
    pub sample: CellPoint,
}

impl LowerAnvilFloor {
    /// From its options: `rect=x0,y0,x1,y1`, `step=N`, `offset=METRES`, `sample=corner|centre`, all required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilFloor, String> {
        let g = ground_options(
            o,
            &[],
            "rect=x0,y0,x1,y1, step=N, offset=METRES (not stored), sample=corner|centre",
        )?;
        Ok(LowerAnvilFloor {
            rect: g.rect,
            step: g.step,
            offset: g.offset,
            sample: g.sample,
        })
    }

    /// The columns (or rows) whose samples lie in `from..=to`, every `step`-th, of `cells` cells of side `side` from
    /// `origin`, the sample `shift` into the cell.
    fn indices(&self, origin: f64, side: f64, shift: f64, cells: u32, (from, to): (f64, f64)) -> Vec<u32> {
        let first = ((from - origin - shift) / side).ceil().max(0.0);
        let last = ((to - origin - shift) / side).floor().min(f64::from(cells) - 1.0);
        if first.is_nan() || last.is_nan() || first > last {
            return Vec::new();
        }
        // in range of u32: clamped to 0..cells above
        let (first, last) = (first as u32, last as u32);
        (first..=last).step_by(self.step as usize).collect()
    }
}

impl Pass for LowerAnvilFloor {
    fn name(&self) -> &'static str {
        "lower-anvil-floor"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        let sample = match self.sample {
            CellPoint::Corner => "corner",
            CellPoint::Centre => "centre",
        };
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("step".to_owned(), self.step.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            ("sample".to_owned(), sample.to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilFloorHeight::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME, Instance::NAME, Transform::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilFloorHeight::NAME]
    }
    fn approximates(&self) -> &'static str {
        "ground heights from a stated offset; each word computed exactly where the game's rounding isn't read"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let grids: Vec<Entity> = w.iter::<AnvilFloorHeight>().map(|(e, _)| e).collect();
        for e in grids {
            let Some(grid) = w.remove::<AnvilFloorHeight>(e) else {
                continue;
            };
            let frame = grid.frame;
            let side = f64::from(grid.cell);
            let shift = match self.sample {
                CellPoint::Corner => 0.0,
                CellPoint::Centre => side / 2.0,
            };
            let [ox, oy] = grid.origin.map(f64::from);
            let [dx, dy] = grid.cells;
            let [x0, y0, x1, y1] = self.rect;
            let columns = self.indices(ox, side, shift, dx, (x0, x1));
            let rows = self.indices(oy, side, shift, dy, (y0, y1));
            if columns.len() < 2 || rows.len() < 2 {
                log.omit(
                    "FloorHeightData grids with under 2 by 2 samples in the rectangle",
                    1,
                );
                continue;
            }
            let scale = f64::from(grid.scale);
            let mut positions = Vec::with_capacity(columns.len() * rows.len());
            let mut zero = 0;
            for &r in &rows {
                for &c in &columns {
                    let (lo, hi, index) = grid
                        .cell(c, r)
                        .ok_or_else(|| format!("cell ({c}, {r}) outside the grid"))?;
                    let (lo, hi) = (f64::from(lo), f64::from(hi));
                    let word = lo + (hi - lo) * f64::from(index) / 3.0;
                    zero += u64::from(word == 0.0);
                    let x = ox + side * f64::from(c) + shift;
                    let y = oy + side * f64::from(r) + shift;
                    positions.push(frame.point([x as f32, y as f32, (scale * word + self.offset) as f32]));
                }
            }
            let width = u32::try_from(columns.len()).map_err(|_| "too many columns".to_owned())?;
            let height = u32::try_from(rows.len()).map_err(|_| "too many rows".to_owned())?;
            let flip = frame.flips_winding();
            let mut indices = Vec::new();
            for r in 0..height - 1 {
                for c in 0..width - 1 {
                    let [a, b] = [r * width + c, r * width + c + 1];
                    let [d, f] = [a + width, b + width];
                    for t in [[a, b, f], [a, f, d]] {
                        indices.extend(if flip { [t[0], t[2], t[1]] } else { t });
                    }
                }
            }
            log.count("ground samples", positions.len() as u64);
            log.count("ground triangles", (indices.len() / 3) as u64);
            log.count("ground samples of word 0", zero);
            log.approximate("ground samples at a stated height offset", positions.len() as u64);
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Position, Stream::F32x3(positions));
            w.insert(
                e,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: None,
                }]),
            );
            let placed = w.spawn();
            w.insert(
                placed,
                Instance {
                    prototype: e,
                    variant: 0,
                },
            );
            w.insert(placed, Transform::IDENTITY);
            log.place(placed);
        }
        Ok(())
    }
}

/// The ground grid `lower-anvil-terrain` draws from one terrain (see it), which `lower-anvil-decals` drapes on: the
/// sample indices along each axis (every `step`-th from the first in the rectangle), their spacing, the sample's
/// shift into its cell, and each sample's point.
pub(crate) struct GroundGrid<'t> {
    pub t: &'t AnvilTerrain,
    /// The tiles with whole rasters, by (row, column) of their node.
    pub tiles: BTreeMap<(u32, u32), &'t TerrainTile>,
    pub columns: Vec<u32>,
    pub rows: Vec<u32>,
    pub spacing: f64,
    pub shift: f64,
    /// A height value's metres: `range` x 2^(level - 28).
    unit: f64,
    offset: f64,
}

impl<'t> GroundGrid<'t> {
    /// `t`'s grid over `rect` by the ground choices; `None` where under 2 by 2 of its samples lie in the rectangle.
    pub fn new(
        t: &'t AnvilTerrain,
        rect: [f64; 4],
        step: u32,
        range: f64,
        offset: f64,
        sample: CellPoint,
    ) -> Option<GroundGrid<'t>> {
        let level = t.level.min(30);
        let nodes = 1u32 << level;
        let spacing = t.side / f64::from(nodes) / f64::from(TERRAIN_CORE);
        let shift = match sample {
            CellPoint::Corner => 0.0,
            CellPoint::Centre => spacing / 2.0,
        };
        let samples = nodes.saturating_mul(TERRAIN_CORE);
        let [x0, y0, x1, y1] = rect;
        let indices = |origin: f64, (from, to): (f64, f64)| -> Vec<u32> {
            let first = ((from - origin - shift) / spacing).ceil().max(0.0);
            let last = ((to - origin - shift) / spacing)
                .floor()
                .min(f64::from(samples) - 1.0);
            if first.is_nan() || last.is_nan() || first > last {
                return Vec::new();
            }
            // in range of u32: clamped to 0..samples above
            (first as u32..=last as u32).step_by(step as usize).collect()
        };
        let columns = indices(t.origin[0], (x0, x1));
        let rows = indices(t.origin[1], (y0, y1));
        if columns.len() < 2 || rows.len() < 2 {
            return None;
        }
        let tiles = t
            .tiles
            .iter()
            .filter(|tile| {
                tile.heights.len() == TERRAIN_TILE * TERRAIN_TILE
                    && tile.colour.len() == TERRAIN_BLOCKS * TERRAIN_BLOCKS * 8
            })
            .map(|tile| ((tile.row, tile.column), tile))
            .collect();
        Some(GroundGrid {
            t,
            tiles,
            columns,
            rows,
            spacing,
            shift,
            unit: range * 2f64.powi(level as i32 - 28),
            offset,
        })
    }

    /// Sample column `gx`'s x and row `gy`'s y.
    pub fn x_of(&self, gx: u32) -> f64 {
        self.t.origin[0] + self.spacing * f64::from(gx) + self.shift
    }
    pub fn y_of(&self, gy: u32) -> f64 {
        self.t.origin[1] + self.spacing * f64::from(gy) + self.shift
    }

    /// Sample (`gx`, `gy`)'s point, `(x, y, height)` as f32 in the terrain's source frame (before it applies): the
    /// value at its node's core sample, `unit` x it + `offset`.
    pub fn point(&self, gx: u32, gy: u32) -> Result<[f32; 3], String> {
        let node = (gy / TERRAIN_CORE, gx / TERRAIN_CORE);
        let (i, j) = ((gy % TERRAIN_CORE) as usize, (gx % TERRAIN_CORE) as usize);
        let tile = self
            .tiles
            .get(&node)
            .ok_or_else(|| format!("sample ({gx}, {gy}): node {node:?} not read"))?;
        let value =
            f64::from(tile.heights[(i + TerrainTile::CORE_AT) * TERRAIN_TILE + j + TerrainTile::CORE_AT]);
        Ok([
            self.x_of(gx) as f32,
            self.y_of(gy) as f32,
            (self.unit * value + self.offset) as f32,
        ])
    }
}

/// `lower-anvil-terrain`: terrain tiles ([`AnvilTerrain`]) become ground: a core mesh of the samples in `rect`
/// (x0, y0, x1, y1), every `step`-th along each axis, two triangles to each square, placed once. A node's 128 x 128
/// samples cover its square, stored from index 2 of its 132 ([`TerrainTile::CORE_AT`]: the engine's
/// reading; the census finds a tile's last four equal to its +x and +y neighbours' first four on all 173,740
/// same-level pairs), sample (i, j) at a 128th of the node's side from its corner (or a half further
/// along each axis, by `sample`), at height `range` x (the value x 2^(level - 28)) + `offset`. The mesh is drawn
/// with a base-colour texture: with `ground=tile` the nodes' colour tiles as stored (their 32 x 32 BC1 blocks a node,
/// side by side), each sample at the centre of its texel, in the colour space `colour` states (the far colour: the
/// game draws it beyond a distance); with `ground=splat` the near colour baked over `rect` (the module `splat`: the
/// materials' detail diffuses blended as the terrain shader does, by the Riders Republic notes' reading, its choices
/// `splat-k=K`, `splat-px=UNITS` a pixel, `splat-uv=FACTOR|over-tile-size`, `splat-noise=raw|srgb` and
/// `splat-occlusion=block-8|none` stated), each vertex at its place in it. Every option
/// is required: the range (an f32 5000 in the Terrain entity, its meaning a tool claim) and the offset (0 by the Riders
/// Republic notes' 101 props on one plaza) aren't stored as such, and neither the half-sample convention nor the colour
/// tiles' colour space is established.
///
/// With `ground=layers` the near colour is the game's own layers instead of a bake: each piece a `core.layered` of
/// the terrain materials its squares use, each layer a material's detail diffuse at its uv scale (step 4 of the
/// module `splat`), its weight at each vertex the engine's blend there (the noise-jittered lookup of the material
/// raster and the 2 x 2 sigmoid at `splat-k`, `splat-noise`: steps 1 to 3), summed per material over the four texels
/// and interpolated across each triangle (the engine evaluates it per pixel: the approximation); a material without a
/// detail diffuse, or a texel past every read node, has no layer, its weight shared out over the others. Each layer's
/// normal map is its material's detail normal (its TextureSet's slot 1: x and y, z rebuilt; red along the tangent +x,
/// green toward -y with v along +y, the notes' reading of the terrain shader's frame: against core's bitangent
/// `cross(N, T) w` a green-down map). With `splat-occlusion=block-8` each vertex's `COLOR_0` is the shader's occlusion
/// factor `0.5 + b` there (block 8, bilinear), which the colour is multiplied by (it may pass 1). A piece takes
/// at most 4 layers (core's weights are one vec4 a vertex): one using more is cut in quarters, down to 16 squares a
/// side, and one still using more keeps its 4 heaviest, the others' weight shared out over them, counted. Left out
/// (counted): the ramps, the masks, the detail normals' flattening on flat ground and the per-cell uv rotation.
///
/// `pieces=whole` draws the ground as one primitive with one texture over `rect`; `pieces=per-node` (with
/// `ground=splat`) as a primitive per terrain node, each square of the grid in the node of its corner sample with the
/// least x and y, each piece with its own near colour baked over its squares' extent (a node's whole square is
/// 128 samples, its side at `splat-px` a pixel: 64 m at 0.25 m is 256 pixels). The pieces' positions are the whole
/// ground's, computed alike, so neighbours share their seam vertices exactly. Each piece (the whole, or a node's) is a
/// prototype of its own, its collision (where drawn) over its own squares, its vertices local to its origin
/// ([`local_origin`]: its box's least corner rounded down to 64 m, which keeps them exact), placed once there.
///
/// With `collision` other than none the ground is also its own collision, on the same prototype: each square's two
/// triangles of the surface its corner sample with the least x and y names (a square spans four samples, whose
/// materials may differ: that sample is this pass's choice), by the CollisionMaterial of that sample's material in the
/// terrain's bank (`collision=collision-material`; `surface` where the bank wasn't read or names none) or `surface`
/// (`collision=one`).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilTerrain {
    pub rect: [f64; 4],
    pub step: u32,
    pub range: f64,
    pub offset: f64,
    pub sample: CellPoint,
    pub colour: ColorSpace,
    /// The ground's collision: none, or its triangles' surfaces as [`SurfaceBy`] names them.
    pub collision: Option<SurfaceBy>,
    /// The surface of a square whose material has no named CollisionMaterial (or of every square, by one).
    pub surface: String,
    /// The colour drawn: the far colour tiles, or the near colour baked.
    pub ground: GroundColour,
    pub pieces: Pieces,
}

/// How `lower-anvil-terrain` divides the ground it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pieces {
    /// One primitive, one texture over the rectangle.
    Whole,
    /// A primitive per terrain node, its near colour baked over its own squares (`ground=splat`).
    PerNode,
}

/// What `lower-anvil-terrain` draws the ground with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GroundColour {
    /// The nodes' far colour tiles.
    Tile,
    /// The near colour, baked.
    Splat(Splat),
    /// The near colour as layers of the terrain materials' details.
    Layers(TerrainLayers),
}

/// `ground=layers`' stated choices: the blend's `k`, the detail uv, the noise's decoding and the occlusion tint, as
/// the splat's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainLayers {
    pub k: f64,
    pub uv: SplatUv,
    pub noise_srgb: bool,
    pub occlusion: bool,
}

/// A terrain tile's side in samples (and in colour texels), the samples of it its node covers, and its colour tile's
/// side in 4 x 4 blocks.
const TERRAIN_TILE: usize = 132;
const TERRAIN_CORE: u32 = 128;
const TERRAIN_BLOCKS: usize = 33;

impl LowerAnvilTerrain {
    /// From its options: `rect=x0,y0,x1,y1`, `step=N`, `range=METRES`, `offset=METRES`, `sample=corner|centre`, all
    /// required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilTerrain, String> {
        let usage = concat!(
            "rect=x0,y0,x1,y1, step=N, range=METRES, offset=METRES (neither stored as such), sample=corner|centre, ",
            "colour=srgb|linear, collision=none|collision-material|one, surface=NAME unless collision=none, ",
            "ground=tile|splat|layers (with splat-k=K, splat-px=UNITS (splat), splat-uv=FACTOR|over-tile-size, splat-noise=raw|srgb, ",
            "splat-occlusion=block-8|none (splat, layers)), pieces=whole|per-node (per-node with ground=splat or layers)"
        );
        let splat_keys = [
            "splat-k",
            "splat-px",
            "splat-uv",
            "splat-noise",
            "splat-occlusion",
        ];
        let mut known = vec!["range", "colour", "collision", "surface", "ground", "pieces"];
        known.extend(splat_keys);
        let g = ground_options(o, &known, usage)?;
        let get = |k: &str| {
            o.get(k)
                .map(String::as_str)
                .ok_or_else(|| format!("{k} is required: {usage}"))
        };
        let number = |k: &str| -> Result<f64, String> {
            let v: f64 = get(k)?.parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() && v > 0.0 {
                Ok(v)
            } else {
                Err(format!("{k} {v}: a positive number"))
            }
        };
        let ground = match get("ground")? {
            "tile" => {
                if let Some(k) = splat_keys.iter().find(|k| o.contains_key(**k)) {
                    return Err(format!("{k} is only taken with ground=splat"));
                }
                GroundColour::Tile
            }
            "splat" => GroundColour::Splat(Splat {
                k: number("splat-k")?,
                pixel: number("splat-px")?,
                uv: match get("splat-uv")? {
                    "over-tile-size" => SplatUv::OverTileSize,
                    _ => SplatUv::Factor(number("splat-uv")?),
                },
                noise_srgb: match get("splat-noise")? {
                    "raw" => false,
                    "srgb" => true,
                    v => return Err(format!("splat-noise={v} is not raw or srgb")),
                },
                occlusion: match get("splat-occlusion")? {
                    "block-8" => true,
                    "none" => false,
                    v => return Err(format!("splat-occlusion={v} is not block-8 or none")),
                },
            }),
            "layers" => {
                if o.contains_key("splat-px") {
                    return Err("splat-px is only taken with ground=splat".into());
                }
                GroundColour::Layers(TerrainLayers {
                    k: number("splat-k")?,
                    uv: match get("splat-uv")? {
                        "over-tile-size" => SplatUv::OverTileSize,
                        _ => SplatUv::Factor(number("splat-uv")?),
                    },
                    noise_srgb: match get("splat-noise")? {
                        "raw" => false,
                        "srgb" => true,
                        v => return Err(format!("splat-noise={v} is not raw or srgb")),
                    },
                    occlusion: match get("splat-occlusion")? {
                        "block-8" => true,
                        "none" => false,
                        v => return Err(format!("splat-occlusion={v} is not block-8 or none")),
                    },
                })
            }
            v => return Err(format!("ground={v} is not tile, splat or layers")),
        };
        let collision = match o.get("collision").map(String::as_str) {
            Some("none") => None,
            Some("collision-material") => Some(SurfaceBy::CollisionMaterial),
            Some("one") => Some(SurfaceBy::One),
            Some(v) => return Err(format!("collision={v} is not none, collision-material or one")),
            None => return Err(format!("collision is required: {usage}")),
        };
        let surface = match (collision, o.get("surface").filter(|s| !s.is_empty())) {
            (None, None) => String::new(),
            (_, Some(s)) => s.clone(),
            (Some(_), None) => return Err(format!("surface is required with collision: {usage}")),
        };
        let colour = match o.get("colour").map(String::as_str) {
            Some("srgb") => ColorSpace::Srgb,
            Some("linear") => ColorSpace::Linear,
            Some(v) => return Err(format!("colour={v} is not srgb or linear")),
            None => return Err(format!("colour is required: {usage}")),
        };
        let pieces = match get("pieces")? {
            "whole" => Pieces::Whole,
            "per-node" if !matches!(ground, GroundColour::Tile) => Pieces::PerNode,
            "per-node" => {
                return Err(
                    "pieces=per-node needs ground=splat or layers (a node's colour tile is sample-centred: it doesn't cover its \
                     last squares)"
                        .into(),
                );
            }
            v => return Err(format!("pieces={v} is not whole or per-node")),
        };
        let range: f64 = o
            .get("range")
            .ok_or_else(|| format!("range is required: {usage}"))?
            .parse()
            .map_err(|_| "range is a number".to_owned())?;
        if !(range.is_finite() && range > 0.0) {
            return Err(format!("range {range}: a positive number"));
        }
        Ok(LowerAnvilTerrain {
            rect: g.rect,
            step: g.step,
            range,
            offset: g.offset,
            sample: g.sample,
            colour,
            collision,
            surface,
            ground,
            pieces,
        })
    }
}

impl Pass for LowerAnvilTerrain {
    fn name(&self) -> &'static str {
        "lower-anvil-terrain"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        let sample = match self.sample {
            CellPoint::Corner => "corner",
            CellPoint::Centre => "centre",
        };
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("step".to_owned(), self.step.to_string()),
            ("range".to_owned(), self.range.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            ("sample".to_owned(), sample.to_owned()),
            (
                "colour".to_owned(),
                match self.colour {
                    ColorSpace::Srgb => "srgb",
                    ColorSpace::Linear => "linear",
                }
                .to_owned(),
            ),
            (
                "collision".to_owned(),
                self.collision.map_or("none", by_text).to_owned(),
            ),
        ])
        .into_iter()
        .chain((!self.surface.is_empty()).then(|| ("surface".to_owned(), self.surface.clone())))
        .chain([(
            "pieces".to_owned(),
            match self.pieces {
                Pieces::Whole => "whole",
                Pieces::PerNode => "per-node",
            }
            .to_owned(),
        )])
        .chain(match self.ground {
            GroundColour::Tile => vec![("ground".to_owned(), "tile".to_owned())],
            GroundColour::Splat(s) => vec![
                ("ground".to_owned(), "splat".to_owned()),
                ("splat-k".to_owned(), s.k.to_string()),
                ("splat-px".to_owned(), s.pixel.to_string()),
                (
                    "splat-uv".to_owned(),
                    match s.uv {
                        SplatUv::Factor(f) => f.to_string(),
                        SplatUv::OverTileSize => "over-tile-size".to_owned(),
                    },
                ),
                (
                    "splat-noise".to_owned(),
                    if s.noise_srgb { "srgb" } else { "raw" }.to_owned(),
                ),
                (
                    "splat-occlusion".to_owned(),
                    if s.occlusion { "block-8" } else { "none" }.to_owned(),
                ),
            ],
            GroundColour::Layers(l) => vec![
                ("ground".to_owned(), "layers".to_owned()),
                ("splat-k".to_owned(), l.k.to_string()),
                (
                    "splat-uv".to_owned(),
                    match l.uv {
                        SplatUv::Factor(f) => f.to_string(),
                        SplatUv::OverTileSize => "over-tile-size".to_owned(),
                    },
                ),
                (
                    "splat-noise".to_owned(),
                    if l.noise_srgb { "srgb" } else { "raw" }.to_owned(),
                ),
                (
                    "splat-occlusion".to_owned(),
                    if l.occlusion { "block-8" } else { "none" }.to_owned(),
                ),
            ],
        })
        .collect()
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilTerrain::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Mesh::NAME,
            Instance::NAME,
            Transform::NAME,
            Texture::NAME,
            PbrMaterial::NAME,
            LayeredMaterial::NAME,
            Collision::NAME,
            Surfaces::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Surfaces::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilTerrain::NAME]
    }
    fn approximates(&self) -> &'static str {
        "ground heights from a stated range and offset, drawn with its far colour tiles or its near colour baked by \
         stated choices"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let terrains: Vec<Entity> = w.iter::<AnvilTerrain>().map(|(e, _)| e).collect();
        for e in terrains {
            let Some(t) = w.remove::<AnvilTerrain>(e) else {
                continue;
            };
            let Some(grid) = GroundGrid::new(&t, self.rect, self.step, self.range, self.offset, self.sample)
            else {
                log.omit("terrains with under 2 by 2 samples in the rectangle", 1);
                continue;
            };
            let (rows, columns, tiles) = (&grid.rows, &grid.columns, &grid.tiles);
            let (spacing, shift) = (grid.spacing, grid.shift);
            // the nodes the samples lie in: the texture holds their 128 x 128 texels, side by side
            let (first_row, first_column) = (rows[0] / TERRAIN_CORE, columns[0] / TERRAIN_CORE);
            let (node_rows, node_columns) = (
                rows[rows.len() - 1] / TERRAIN_CORE - first_row + 1,
                columns[columns.len() - 1] / TERRAIN_CORE - first_column + 1,
            );
            let mut positions = Vec::with_capacity(columns.len() * rows.len());
            for &gy in rows {
                for &gx in columns {
                    positions.push(t.frame.point(grid.point(gx, gy)?));
                }
            }
            let width = u32::try_from(columns.len()).map_err(|_| "too many columns".to_owned())?;
            let height = u32::try_from(rows.len()).map_err(|_| "too many rows".to_owned())?;
            let flip = t.frame.flips_winding();
            let mut indices = Vec::new();
            // each square's collision surface: the material at its corner sample with the least x and y
            let mut square_names: Vec<&str> = Vec::new();
            for r in 0..height - 1 {
                for c in 0..width - 1 {
                    let [a, b] = [r * width + c, r * width + c + 1];
                    let [d, f] = [a + width, b + width];
                    for tri in [[a, b, f], [a, f, d]] {
                        indices.extend(if flip { [tri[0], tri[2], tri[1]] } else { tri });
                    }
                    if let Some(by) = self.collision {
                        let (gy, gx) = (rows[r as usize], columns[c as usize]);
                        let node = (gy / TERRAIN_CORE, gx / TERRAIN_CORE);
                        let at = ((gy % TERRAIN_CORE) as usize + TerrainTile::CORE_AT) * TERRAIN_TILE
                            + (gx % TERRAIN_CORE) as usize
                            + TerrainTile::CORE_AT;
                        let material = tiles
                            .get(&node)
                            .and_then(|tile| tile.material.get(at))
                            .and_then(|&v| t.materials.get(usize::from(v)))
                            .and_then(|m| m.as_ref())
                            .and_then(|m| m.name.as_deref());
                        square_names.push(match (by, material) {
                            (SurfaceBy::CollisionMaterial, Some(name)) => name,
                            _ => &self.surface,
                        });
                    }
                }
            }
            let square_count = ((width - 1) * (height - 1)) as usize;
            // what is drawn: per piece its texture, its vertices' coordinates in it, and (per node) which of the
            // whole's vertices it draws and its triangles over them
            let pieces: Vec<GroundPiece> = match self.ground {
                GroundColour::Layers(layers) => self.layer_pieces(
                    w,
                    &t,
                    &layers,
                    LayerGrid {
                        rows,
                        columns,
                        spacing,
                        shift,
                        flip,
                    },
                    log,
                )?,
                GroundColour::Tile => {
                    // the colour: each node's 128 x 128 core texels (from TerrainTile::CORE_AT, not on its BC1 blocks' edges), decoded
                    // (BC1's 8-bit decode is its meaning) and laid side by side
                    let (texels_wide, texels_high) = (node_columns * TERRAIN_CORE, node_rows * TERRAIN_CORE);
                    let core = TERRAIN_CORE as usize;
                    let mut rgba = vec![0u8; texels_wide as usize * texels_high as usize * 4];
                    for nr in 0..node_rows {
                        for nc in 0..node_columns {
                            let node = (first_row + nr, first_column + nc);
                            let tile = tiles
                                .get(&node)
                                .ok_or_else(|| format!("node {node:?} not read"))?;
                            let tile_texture = Texture {
                                width: TERRAIN_TILE as u32,
                                height: TERRAIN_TILE as u32,
                                format: TexFormat::Bc1,
                                levels: vec![tile.colour.clone()],
                                color_space: self.colour,
                                role: TexRole::Color,
                                normal: None,
                            };
                            let meta = primase_image::TextureMetadata {
                                color_space: self.colour,
                                role: TexRole::Color,
                                normal: None,
                            };
                            let decoded =
                                primase_image::map_rgba8_levels(&tile_texture, 0..1, meta, |x| Ok(x.rgba))
                                    .map_err(|e| format!("node {node:?}'s colour tile: {e}"))?;
                            let texels = &decoded.levels[0];
                            for i in 0..core {
                                let from =
                                    ((i + TerrainTile::CORE_AT) * TERRAIN_TILE + TerrainTile::CORE_AT) * 4;
                                let row = nr as usize * core + i;
                                let to = (row * texels_wide as usize + nc as usize * core) * 4;
                                rgba[to..to + core * 4].copy_from_slice(&texels[from..from + core * 4]);
                            }
                        }
                    }
                    // each sample at its texel's centre
                    let mut uvs = Vec::with_capacity(positions.len());
                    for &gy in rows {
                        for &gx in columns {
                            uvs.push([
                                ((f64::from(gx - first_column * TERRAIN_CORE) + 0.5) / f64::from(texels_wide))
                                    as f32,
                                ((f64::from(gy - first_row * TERRAIN_CORE) + 0.5) / f64::from(texels_high))
                                    as f32,
                            ]);
                        }
                    }
                    log.count("terrain nodes used", tiles.len() as u64);
                    log.count("ground samples", positions.len() as u64);
                    log.count("ground triangles", (indices.len() / 3) as u64);
                    log.count(
                        "ground colour texels",
                        u64::from(texels_wide) * u64::from(texels_high),
                    );
                    log.approximate(
                        "ground samples at a stated range and offset",
                        positions.len() as u64,
                    );
                    log.approximate("ground drawn with its colour tiles alone", 1);
                    let texture = w.spawn();
                    w.insert(
                        texture,
                        Texture {
                            width: texels_wide,
                            height: texels_high,
                            format: TexFormat::Rgba8Unorm,
                            levels: vec![rgba],
                            color_space: self.colour,
                            role: TexRole::Color,
                            normal: None,
                        },
                    );
                    vec![GroundPiece {
                        texture: Some(texture),
                        layered: None,
                        uvs,
                        vertices: None,
                        indices: indices.clone(),
                        squares: (0..square_count).collect(),
                    }]
                }
                GroundColour::Splat(splat) => {
                    let x_of = |gx: u32| t.origin[0] + spacing * f64::from(gx) + shift;
                    let y_of = |gy: u32| t.origin[1] + spacing * f64::from(gy) + shift;
                    // per piece: its rectangle, and its squares (row, column) of the grid
                    let groups: Vec<([f64; 4], Squares)> = match self.pieces {
                        Pieces::Whole => vec![(
                            self.rect,
                            (0..height - 1)
                                .flat_map(|r| (0..width - 1).map(move |c| (r, c)))
                                .collect(),
                        )],
                        Pieces::PerNode => {
                            let mut by_node: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
                            for r in 0..height - 1 {
                                for c in 0..width - 1 {
                                    let node = (
                                        rows[r as usize] / TERRAIN_CORE,
                                        columns[c as usize] / TERRAIN_CORE,
                                    );
                                    by_node.entry(node).or_default().push((r, c));
                                }
                            }
                            by_node
                                .into_values()
                                .map(|squares| {
                                    let (r0, c0) = squares[0];
                                    let (r1, c1) = squares[squares.len() - 1];
                                    let rect = [
                                        x_of(columns[c0 as usize]),
                                        y_of(rows[r0 as usize]),
                                        x_of(columns[c1 as usize + 1]),
                                        y_of(rows[r1 as usize + 1]),
                                    ];
                                    (rect, squares)
                                })
                                .collect()
                        }
                    };
                    let rects: Vec<[f64; 4]> = groups.iter().map(|(r, _)| *r).collect();
                    let images = splat::bake(w, &t, &rects, &splat, self.colour)?;
                    let mut total = splat::Baked::default();
                    let mut pieces = Vec::with_capacity(images.len());
                    for ((rect, squares), (image, baked)) in groups.into_iter().zip(images) {
                        total.pixels += baked.pixels;
                        total.tile_fallback += baked.tile_fallback;
                        total.outside += baked.outside;
                        let (bw, bh) = (
                            f64::from(image.width) * splat.pixel,
                            f64::from(image.height) * splat.pixel,
                        );
                        let uv = |v: u32| {
                            let (gx, gy) = (columns[(v % width) as usize], rows[(v / width) as usize]);
                            [
                                ((x_of(gx) - rect[0]) / bw) as f32,
                                ((y_of(gy) - rect[1]) / bh) as f32,
                            ]
                        };
                        let texture = w.spawn();
                        w.insert(texture, image);
                        pieces.push(match self.pieces {
                            Pieces::Whole => GroundPiece {
                                texture: Some(texture),
                                layered: None,
                                uvs: (0..width * height).map(uv).collect(),
                                vertices: None,
                                indices: indices.clone(),
                                squares: (0..square_count).collect(),
                            },
                            Pieces::PerNode => {
                                // the whole's vertices this piece's squares use, in first use, and its triangles
                                let mut local: BTreeMap<u32, u32> = BTreeMap::new();
                                let mut vertices = Vec::new();
                                let mut piece_indices = Vec::with_capacity(squares.len() * 6);
                                let square_index = squares
                                    .iter()
                                    .map(|&(r, c)| (r * (width - 1) + c) as usize)
                                    .collect();
                                for (r, c) in squares {
                                    let [a, b] = [r * width + c, r * width + c + 1];
                                    let [d, f] = [a + width, b + width];
                                    for tri in [[a, b, f], [a, f, d]] {
                                        let tri = if flip { [tri[0], tri[2], tri[1]] } else { tri };
                                        for v in tri {
                                            let next = u32::try_from(vertices.len())
                                                .map_err(|_| "too many vertices".to_owned())?;
                                            let at = *local.entry(v).or_insert_with(|| {
                                                vertices.push(v);
                                                next
                                            });
                                            piece_indices.push(at);
                                        }
                                    }
                                }
                                GroundPiece {
                                    texture: Some(texture),
                                    layered: None,
                                    uvs: vertices.iter().map(|&v| uv(v)).collect(),
                                    vertices: Some(vertices),
                                    indices: piece_indices,
                                    squares: square_index,
                                }
                            }
                        });
                    }
                    log.count("terrain nodes used", tiles.len() as u64);
                    log.count("ground samples", positions.len() as u64);
                    log.count("ground triangles", (indices.len() / 3) as u64);
                    log.count("ground pieces", pieces.len() as u64);
                    log.count("ground near-colour pixels baked", total.pixels);
                    log.approximate(
                        "ground samples at a stated range and offset",
                        positions.len() as u64,
                    );
                    log.approximate(
                        "ground near colour baked by the notes' reading of the terrain shader, its choices stated",
                        1,
                    );
                    if total.tile_fallback > 0 {
                        log.omit(
                            "ground pixels a material of which has no detail diffuse: its far colour tile there",
                            total.tile_fallback,
                        );
                    }
                    if total.outside > 0 {
                        log.omit("ground pixels outside every read node: black", total.outside);
                    }
                    log.omit(
                        "ground near colour's altitude ramps, detail normals, slope normal, per-cell uv rotation and \
                         cell-hash term (not read)",
                        1,
                    );
                    pieces
                }
            };
            // each square's collision surface, by index
            let mut per_square = Vec::new();
            if self.collision.is_some() {
                let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
                per_square.reserve(square_names.len());
                for name in &square_names {
                    per_square.push(surfaces.index(name).map_err(|e| e.to_string())?);
                    log.count(&format!("ground collision squares of surface {name}"), 1);
                }
                w.insert_resource(surfaces);
            }
            // each piece a prototype of its own (the terrain's entity the first), its vertices local to its origin
            for (k, piece) in pieces.into_iter().enumerate() {
                let prototype = if k == 0 { e } else { w.spawn() };
                let world_positions: Vec<[f32; 3]> = match &piece.vertices {
                    None => positions.clone(),
                    Some(v) => v.iter().map(|&i| positions[i as usize]).collect(),
                };
                let origin = local_origin(&world_positions);
                let local: Vec<[f32; 3]> = world_positions
                    .iter()
                    .map(|p| std::array::from_fn(|a| p[a] - origin[a]))
                    .collect();
                if self.collision.is_some() {
                    w.insert(
                        prototype,
                        Collision {
                            positions: local.clone(),
                            triangles: piece.indices.as_chunks::<3>().0.to_vec(),
                            surfaces: piece.squares.iter().flat_map(|&q| [per_square[q]; 2]).collect(),
                        },
                    );
                }
                let mut streams = BTreeMap::new();
                let material = match (piece.layered, piece.texture) {
                    (
                        Some(LayeredPiece {
                            material: layered,
                            weights,
                            occlusion,
                        }),
                        _,
                    ) => {
                        streams.insert(Semantic::Color(1), Stream::F32x4(weights));
                        if let Some(o) = occlusion {
                            streams.insert(Semantic::Color(0), Stream::F32x4(o));
                        }
                        layered
                    }
                    (None, Some(texture)) => {
                        let material = w.spawn();
                        w.insert(
                            material,
                            PbrMaterial {
                                base_color_texture: Some(TextureRef {
                                    wrap: [Wrap::Clamp; 2],
                                    ..TextureRef::new(texture)
                                }),
                                metallic: 0.0,
                                ..PbrMaterial::default()
                            },
                        );
                        material
                    }
                    (None, None) => return Err("a ground piece with no colour".into()),
                };
                streams.insert(Semantic::Position, Stream::F32x3(local));
                streams.insert(Semantic::TexCoord(0), Stream::F32x2(piece.uvs));
                w.insert(
                    prototype,
                    Mesh::new(vec![Primitive {
                        topology: Topology::Triangles,
                        streams,
                        indices: Some(piece.indices),
                        material: Some(material),
                    }]),
                );
                let placed = w.spawn();
                w.insert(
                    placed,
                    Instance {
                        prototype,
                        variant: 0,
                    },
                );
                w.insert(placed, placed_at(origin));
                log.place(placed);
            }
        }
        Ok(())
    }
}

/// A `ground=layers` piece's colour: its layered material, its layer weights per vertex and its occlusion tint per
/// vertex, where stated.
struct LayeredPiece {
    material: Entity,
    weights: Vec<[f32; 4]>,
    occlusion: Option<Vec<[f32; 4]>>,
}

/// Squares of the ground grid, by (row, column).
type Squares = Vec<(u32, u32)>;

/// The ground grid `ground=layers` draws: its sample indices along each axis, their spacing and shift, and whether
/// the frame flips the winding.
struct LayerGrid<'g> {
    rows: &'g [u32],
    columns: &'g [u32],
    spacing: f64,
    shift: f64,
    flip: bool,
}

impl LowerAnvilTerrain {
    /// `ground=layers`' pieces (see [`LowerAnvilTerrain`]).
    fn layer_pieces(
        &self,
        w: &mut World,
        t: &AnvilTerrain,
        layers: &TerrainLayers,
        g: LayerGrid<'_>,
        log: &mut PassLog,
    ) -> Result<Vec<GroundPiece>, String> {
        let lookup = splat::Lookup::new(w, t, layers.k, layers.noise_srgb, self.colour)?;
        let (width, height) = (g.columns.len(), g.rows.len());
        let x_of = |gx: u32| t.origin[0] + g.spacing * f64::from(gx) + g.shift;
        let y_of = |gy: u32| t.origin[1] + g.spacing * f64::from(gy) + g.shift;
        // each vertex's weight per material (the engine's blend at it), on the materials with a detail diffuse
        let (mut outside, mut undetailed) = (0u64, 0u64);
        let mut weights: Vec<Vec<(u8, f64)>> = Vec::with_capacity(width * height);
        // each vertex's occlusion factor, where stated
        let mut occlusion: Vec<f32> = Vec::with_capacity(if layers.occlusion { width * height } else { 0 });
        for &gy in g.rows {
            for &gx in g.columns {
                let mut at: Vec<(u8, f64)> = Vec::new();
                let corners = lookup.corners(x_of(gx), y_of(gy));
                if layers.occlusion {
                    occlusion.push(
                        corners
                            .as_ref()
                            .map_or(1.0, |c| lookup.occlusion(c.gx, c.gy) as f32),
                    );
                }
                match corners {
                    None => outside += 1,
                    Some(c) => {
                        for (m, wt) in c.materials.into_iter().zip(c.weights()) {
                            let Some(m) = m else {
                                outside += 1;
                                continue;
                            };
                            if splat::layer_detail(t, layers.uv, m).is_none() {
                                undetailed += 1;
                                continue;
                            }
                            if wt > 0.0 {
                                match at.iter_mut().find(|(n, _)| *n == m) {
                                    Some((_, v)) => *v += wt,
                                    None => at.push((m, wt)),
                                }
                            }
                        }
                    }
                }
                let sum: f64 = at.iter().map(|(_, v)| v).sum();
                if sum > 0.0 {
                    for (_, v) in &mut at {
                        *v /= sum;
                    }
                }
                weights.push(at);
            }
        }
        let at = |r: usize, c: usize| &weights[r * width + c];
        // the pieces' square rectangles: a node's (or the whole), cut in quarters while it uses over 4 materials
        let mut starts: BTreeMap<(u32, u32), [usize; 4]> = BTreeMap::new();
        for r in 0..height - 1 {
            for c in 0..width - 1 {
                let key = match self.pieces {
                    Pieces::Whole => (0, 0),
                    Pieces::PerNode => (g.rows[r] / TERRAIN_CORE, g.columns[c] / TERRAIN_CORE),
                };
                let b = starts.entry(key).or_insert([r, r + 1, c, c + 1]);
                *b = [b[0].min(r), b[1].max(r + 1), b[2].min(c), b[3].max(c + 1)];
            }
        }
        let used = |[r0, r1, c0, c1]: [usize; 4]| -> BTreeMap<u8, f64> {
            let mut m = BTreeMap::new();
            for r in r0..=r1 {
                for c in c0..=c1 {
                    for &(k, v) in at(r, c) {
                        *m.entry(k).or_insert(0.0) += v;
                    }
                }
            }
            m
        };
        let mut leaves = Vec::new();
        let mut stack: Vec<[usize; 4]> = starts.into_values().collect();
        while let Some(b @ [r0, r1, c0, c1]) = stack.pop() {
            if used(b).len() <= 4 || (r1 - r0 <= 16 && c1 - c0 <= 16) {
                leaves.push(b);
                continue;
            }
            let (rm, cm) = ((r0 + r1) / 2, (c0 + c1) / 2);
            for q in [
                [r0, rm, c0, cm],
                [r0, rm, cm, c1],
                [rm, r1, c0, cm],
                [rm, r1, cm, c1],
            ] {
                if q[0] < q[1] && q[2] < q[3] {
                    stack.push(q);
                }
            }
        }
        leaves.sort_unstable();
        // each terrain material's layer, made once
        let mut layer_materials: BTreeMap<u8, Entity> = BTreeMap::new();
        let mut pieces = Vec::with_capacity(leaves.len());
        let (mut trimmed, mut renormalised) = (0u64, 0u64);
        for b @ [r0, r1, c0, c1] in leaves {
            let mut by_weight: Vec<(u8, f64)> = used(b).into_iter().collect();
            by_weight.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            if by_weight.len() > 4 {
                trimmed += 1;
            }
            let kept: Vec<u8> = by_weight.iter().take(4).map(|&(m, _)| m).collect();
            let (ox, oy) = (x_of(g.columns[c0]), y_of(g.rows[r0]));
            let mut layer_list = Vec::with_capacity(kept.len());
            for &m in &kept {
                let (diffuse, normal, scale) =
                    splat::layer_detail(t, layers.uv, m).ok_or("a kept layer without its detail")?;
                let material = match layer_materials.get(&m) {
                    Some(&e) => e,
                    None => {
                        let srgb = w.get::<AnvilTextureMap>(diffuse).is_some_and(|d| d.field_25 == 1);
                        if let Some(tex) = w.get_mut::<Texture>(diffuse) {
                            tex.role = TexRole::Color;
                            tex.color_space = if srgb {
                                ColorSpace::Srgb
                            } else {
                                ColorSpace::Linear
                            };
                        }
                        if let Some(n) = normal
                            && let Some(tex) = w.get_mut::<Texture>(n)
                        {
                            tex.role = TexRole::Normal;
                            tex.color_space = ColorSpace::Linear;
                            tex.normal = Some(NormalConvention {
                                green: NormalGreen::Down,
                                z: NormalZ::Rebuild,
                                x: NormalX::Red,
                                basis: NormalBasis::TangentBitangent,
                            });
                        }
                        let e = w.spawn();
                        w.insert(
                            e,
                            PbrMaterial {
                                base_color_texture: Some(TextureRef::new(diffuse)),
                                normal_texture: normal.map(TextureRef::new),
                                metallic: 0.0,
                                ..PbrMaterial::default()
                            },
                        );
                        layer_materials.insert(m, e);
                        e
                    }
                };
                // the piece's coordinates are its own metres from (ox, oy): the detail's phase there kept
                let phase = |o: f64| ((o * scale).rem_euclid(1.0)) as f32;
                layer_list.push(Some(Layer {
                    material,
                    texcoord: None,
                    transform: Some(UvTransform {
                        offset: [phase(ox), phase(oy)],
                        scale: [scale as f32; 2],
                        rotation: 0.0,
                    }),
                }));
            }
            // the piece's vertices, in first use, and its triangles; per vertex its kept layers' weights
            let mut local: BTreeMap<u32, u32> = BTreeMap::new();
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            let mut squares = Vec::new();
            let w32 = width as u32;
            for r in r0..r1 {
                for c in c0..c1 {
                    squares.push(r * (width - 1) + c);
                    let (r, c) = (r as u32, c as u32);
                    let [a, bb] = [r * w32 + c, r * w32 + c + 1];
                    let [d, f] = [a + w32, bb + w32];
                    for tri in [[a, bb, f], [a, f, d]] {
                        let tri = if g.flip { [tri[0], tri[2], tri[1]] } else { tri };
                        for v in tri {
                            let next =
                                u32::try_from(vertices.len()).map_err(|_| "too many vertices".to_owned())?;
                            let at = *local.entry(v).or_insert_with(|| {
                                vertices.push(v);
                                next
                            });
                            indices.push(at);
                        }
                    }
                }
            }
            let mut piece_weights = Vec::with_capacity(vertices.len());
            let piece_occlusion: Option<Vec<[f32; 4]>> = layers.occlusion.then(|| {
                vertices
                    .iter()
                    .map(|&v| {
                        let o = occlusion[v as usize];
                        [o, o, o, 1.0]
                    })
                    .collect()
            });
            let mut uvs = Vec::with_capacity(vertices.len());
            for &v in &vertices {
                let (r, c) = (v as usize / width, v as usize % width);
                let mut out = [0.0f64; 4];
                for (slot, m) in kept.iter().enumerate() {
                    out[slot] = at(r, c).iter().find(|(n, _)| n == m).map_or(0.0, |(_, x)| *x);
                }
                let sum: f64 = out.iter().sum();
                if sum > 0.0 && (sum - 1.0).abs() > 1e-12 {
                    renormalised += 1;
                    for x in &mut out {
                        *x /= sum;
                    }
                }
                piece_weights.push(out.map(|x| x as f32));
                uvs.push([(x_of(g.columns[c]) - ox) as f32, (y_of(g.rows[r]) - oy) as f32]);
            }
            let layered = w.spawn();
            w.insert(
                layered,
                LayeredMaterial {
                    layers: layer_list,
                    weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
                },
            );
            pieces.push(GroundPiece {
                texture: None,
                layered: Some(LayeredPiece {
                    material: layered,
                    weights: piece_weights,
                    occlusion: piece_occlusion,
                }),
                uvs,
                vertices: Some(vertices),
                indices,
                squares,
            });
        }
        log.count("ground layer pieces", pieces.len() as u64);
        log.count("ground layer materials", layer_materials.len() as u64);
        log.approximate(
            "ground layer weights evaluated at the vertices and interpolated (the engine's blend is per pixel)",
            1,
        );
        if trimmed > 0 {
            log.approximate(
                "ground pieces using more than 4 materials at 16 squares a side: their 4 heaviest kept",
                trimmed,
            );
        }
        if renormalised > 0 {
            log.approximate(
                "ground vertices whose dropped layers' weight was shared out over the kept",
                renormalised,
            );
        }
        if outside > 0 {
            log.omit(
                "ground layer texels past every read node (their weight shared out)",
                outside,
            );
        }
        if undetailed > 0 {
            log.omit(
                "ground layer texels of materials without a detail diffuse (their weight shared out)",
                undetailed,
            );
        }
        log.omit(
            "ground layers' altitude ramps, masks, detail normals' flattening and per-cell uv rotation (not lowered)",
            1,
        );
        Ok(pieces)
    }
}

/// One primitive of the ground: its texture, its vertices' coordinates in it, which of the whole grid's vertices it
/// draws (`None`: all, in order) and its triangles over its own vertices.
struct GroundPiece {
    /// Its colour: a texture, or a layered material and its weights per vertex.
    texture: Option<Entity>,
    /// With its occlusion tint per vertex (`COLOR_0`), where stated.
    layered: Option<LayeredPiece>,
    uvs: Vec<[f32; 2]>,
    vertices: Option<Vec<u32>>,
    indices: Vec<u32>,
    /// Its squares, by their index in the grid's row order (two triangles each, in `indices`' order).
    squares: Vec<usize>,
}

/// Where a prototype of `positions` (in the world) is placed: its box's least corner rounded down to a multiple of
/// 64 units on each axis. A position less that origin is exact in f32 (the origin a multiple of the position's ulp
/// and nearer zero), so the content placed there is where it was to the bit, and its vertices lie near its origin
/// for a writer that files a placement by its position and reach.
pub(crate) fn local_origin(positions: &[[f32; 3]]) -> [f32; 3] {
    std::array::from_fn(|k| {
        let least = positions.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
        if least.is_finite() {
            (least / 64.0).floor() * 64.0
        } else {
            0.0
        }
    })
}

/// A translation to `origin`.
pub(crate) fn placed_at(origin: [f32; 3]) -> Transform {
    Transform {
        cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], origin],
    }
}

/// Register Anvil's passes by name.
pub fn register(reg: &mut PassRegistry) {
    reg.register_requiring(
        "lower-anvil-collision",
        |o| Ok(Box::new(LowerAnvilCollision::from_options(o)?)),
        &[("by", "one"), ("surface", "rr-diagnostic-unknown")],
    );
    reg.register_requiring(
        "lower-anvil-shapes",
        |o| Ok(Box::new(LowerAnvilShapes::from_options(o)?)),
        &[("by", "one"), ("surface", "rr-diagnostic-unknown")],
    );
    reg.register("lower-anvil-lods", |_| Ok(Box::new(LowerAnvilLods)));
    reg.register("lower-anvil-world-uv", |_| Ok(Box::new(LowerAnvilWorldUv)));
    reg.register_requiring(
        "lower-anvil-material",
        |o| Ok(Box::new(LowerAnvilMaterial::from_options(o)?)),
        &[
            ("normal-green", "up"),
            ("alpha", "opaque"),
            ("unset", "white"),
            ("shader-uv", "texcoord"),
            ("emissive-exposure", "1"),
            ("underground-view", "0.5"),
            ("fake-river-s", "0"),
        ],
    );
    reg.register_requiring(
        "bake-anvil-material",
        |o| Ok(Box::new(BakeAnvilMaterial::from_options(o)?)),
        &[
            ("size", "512"),
            ("normal-green", "up"),
            ("shader-uv", "texcoord"),
            ("mip-bias", "0"),
            ("terrain-ao", "0.5"),
            ("sun-elevation", "45"),
            ("view-cosine", "0.7"),
            ("unset", "white"),
        ],
    );
    reg.register_requiring(
        "lower-anvil-water",
        |o| Ok(Box::new(LowerAnvilWater::from_options(o)?)),
        &[
            ("rect", "-64,-64,64,64"),
            ("depth", "3:2,4:1,5:0.5,6:0.5"),
            ("river-depth", "1"),
            ("colour-max", "0.15"),
            ("roughness", "0.05"),
            ("shore", "coverage"),
        ],
    );
    reg.register_requiring(
        "lower-anvil-decals",
        |o| Ok(Box::new(LowerAnvilDecals::from_options(o)?)),
        &[
            ("rect", "-64,-64,64,64"),
            ("step", "1"),
            ("range", "1"),
            ("offset", "0"),
            ("sample", "corner"),
            ("family", "write-gbuffer-roughness"),
            ("cutoff", "0.4"),
            ("lift", "0.01"),
            ("terrain-family", "out"),
        ],
    );
    reg.register_requiring(
        "lower-anvil-data-layers",
        |o| Ok(Box::new(LowerAnvilDataLayers::from_options(o)?)),
        &[("combine", "all")],
    );
    reg.register_requiring(
        "lower-anvil-floor",
        |o| Ok(Box::new(LowerAnvilFloor::from_options(o)?)),
        &[
            ("rect", "-64,-64,64,64"),
            ("step", "1"),
            ("offset", "0"),
            ("sample", "corner"),
        ],
    );
    reg.register_requiring(
        "lower-anvil-terrain",
        |o| Ok(Box::new(LowerAnvilTerrain::from_options(o)?)),
        &[
            ("rect", "-64,-64,64,64"),
            ("step", "1"),
            ("range", "1"),
            ("offset", "0"),
            ("sample", "corner"),
            ("colour", "srgb"),
            ("ground", "tile"),
            ("collision", "none"),
            ("pieces", "whole"),
        ],
    );
    reg.register_requiring(
        "lower-anvil-mesh",
        |o| Ok(Box::new(LowerAnvilMesh::from_options(o)?)),
        &[
            ("uv-words", "signed"),
            ("uv-divisor", "2048"),
            ("uv-v", "as-stored"),
            ("uv-tool", "omit"),
            ("colour", "omit"),
            ("normals", "omit"),
        ],
    );
}
