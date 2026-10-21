//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use std::collections::BTreeMap;

mod bake;
pub mod formulas;
mod material;
mod splat;
mod water;
pub use bake::{BakeAnvilMaterial, gbuffer_pixel_key};
pub use material::{AlphaChoice, Emission, LowerAnvilMaterial, ShaderUv};
pub use splat::{Splat, SplatUv};
pub use water::{LowerAnvilWater, Shore};

use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{PbrMaterial, TextureRef, Wrap};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::world::Resource;
use primase_core::{Collider, Shape};
use primase_core::{
    Collision, ColorSpace, Component, Entity, Instance, LodForm, Lods, Mesh, Primitive, Semantic, Stream,
    Surfaces, TexFormat, TexRole, Texture, Topology, Transform, World,
};

use crate::components::{
    AnvilCollisionShape, AnvilDataLayers, AnvilFloorHeight, AnvilLods, AnvilMesh, AnvilMeshShape,
    AnvilTerrain, CollisionMaterialRef, ShapeGeometry, TerrainTile, UvWordsAt,
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
}

/// What `lower-anvil-terrain` draws the ground with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GroundColour {
    /// The nodes' far colour tiles.
    Tile,
    /// The near colour, baked.
    Splat(Splat),
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
            "ground=tile|splat (with splat-k=K, splat-px=UNITS, splat-uv=FACTOR|over-tile-size, splat-noise=raw|srgb, ",
            "splat-occlusion=block-8|none)"
        );
        let splat_keys = [
            "splat-k",
            "splat-px",
            "splat-uv",
            "splat-noise",
            "splat-occlusion",
        ];
        let mut known = vec!["range", "colour", "collision", "surface", "ground"];
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
            v => return Err(format!("ground={v} is not tile or splat")),
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
            let level = t.level.min(30);
            let nodes = 1u32 << level;
            let spacing = t.side / f64::from(nodes) / f64::from(TERRAIN_CORE);
            let shift = match self.sample {
                CellPoint::Corner => 0.0,
                CellPoint::Centre => spacing / 2.0,
            };
            let samples = nodes.saturating_mul(TERRAIN_CORE);
            let [x0, y0, x1, y1] = self.rect;
            let indices = |origin: f64, (from, to): (f64, f64)| -> Vec<u32> {
                let first = ((from - origin - shift) / spacing).ceil().max(0.0);
                let last = ((to - origin - shift) / spacing)
                    .floor()
                    .min(f64::from(samples) - 1.0);
                if first.is_nan() || last.is_nan() || first > last {
                    return Vec::new();
                }
                // in range of u32: clamped to 0..samples above
                (first as u32..=last as u32).step_by(self.step as usize).collect()
            };
            let columns = indices(t.origin[0], (x0, x1));
            let rows = indices(t.origin[1], (y0, y1));
            if columns.len() < 2 || rows.len() < 2 {
                log.omit("terrains with under 2 by 2 samples in the rectangle", 1);
                continue;
            }
            let tiles: BTreeMap<(u32, u32), &TerrainTile> = t
                .tiles
                .iter()
                .filter(|tile| {
                    tile.heights.len() == TERRAIN_TILE * TERRAIN_TILE
                        && tile.colour.len() == TERRAIN_BLOCKS * TERRAIN_BLOCKS * 8
                })
                .map(|tile| ((tile.row, tile.column), tile))
                .collect();
            // the nodes the samples lie in: the texture holds their 128 x 128 texels, side by side
            let (first_row, first_column) = (rows[0] / TERRAIN_CORE, columns[0] / TERRAIN_CORE);
            let (node_rows, node_columns) = (
                rows[rows.len() - 1] / TERRAIN_CORE - first_row + 1,
                columns[columns.len() - 1] / TERRAIN_CORE - first_column + 1,
            );
            let unit = self.range * 2f64.powi(level as i32 - 28);
            let mut positions = Vec::with_capacity(columns.len() * rows.len());
            for &gy in &rows {
                for &gx in &columns {
                    let node = (gy / TERRAIN_CORE, gx / TERRAIN_CORE);
                    let (i, j) = ((gy % TERRAIN_CORE) as usize, (gx % TERRAIN_CORE) as usize);
                    let tile = tiles
                        .get(&node)
                        .ok_or_else(|| format!("sample ({gx}, {gy}): node {node:?} not read"))?;
                    let value = f64::from(
                        tile.heights[(i + TerrainTile::CORE_AT) * TERRAIN_TILE + j + TerrainTile::CORE_AT],
                    );
                    let x = t.origin[0] + spacing * f64::from(gx) + shift;
                    let y = t.origin[1] + spacing * f64::from(gy) + shift;
                    positions.push(
                        t.frame
                            .point([x as f32, y as f32, (unit * value + self.offset) as f32]),
                    );
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
            let (texture, uvs) = match self.ground {
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
                    for &gy in &rows {
                        for &gx in &columns {
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
                    (texture, uvs)
                }
                GroundColour::Splat(splat) => {
                    let (image, baked) = splat::bake(w, &t, self.rect, &splat, self.colour)?;
                    let (bw, bh) = (
                        f64::from(image.width) * splat.pixel,
                        f64::from(image.height) * splat.pixel,
                    );
                    let uvs = rows
                        .iter()
                        .flat_map(|&gy| columns.iter().map(move |&gx| (gx, gy)))
                        .map(|(gx, gy)| {
                            let x = t.origin[0] + spacing * f64::from(gx) + shift;
                            let y = t.origin[1] + spacing * f64::from(gy) + shift;
                            [((x - x0) / bw) as f32, ((y - y0) / bh) as f32]
                        })
                        .collect::<Vec<_>>();
                    log.count("terrain nodes used", tiles.len() as u64);
                    log.count("ground samples", positions.len() as u64);
                    log.count("ground triangles", (indices.len() / 3) as u64);
                    log.count("ground near-colour pixels baked", baked.pixels);
                    log.approximate(
                        "ground samples at a stated range and offset",
                        positions.len() as u64,
                    );
                    log.approximate(
                        "ground near colour baked by the notes' reading of the terrain shader, its choices stated",
                        1,
                    );
                    if baked.tile_fallback > 0 {
                        log.omit(
                            "ground pixels a material of which has no detail diffuse: its far colour tile there",
                            baked.tile_fallback,
                        );
                    }
                    if baked.outside > 0 {
                        log.omit("ground pixels outside every read node: black", baked.outside);
                    }
                    log.omit(
                        "ground near colour's altitude ramps, detail normals, slope normal, per-cell uv rotation and \
                         cell-hash term (not read)",
                        1,
                    );
                    let texture = w.spawn();
                    w.insert(texture, image);
                    (texture, uvs)
                }
            };
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
            if self.collision.is_some() {
                let mut surfaces = w.remove_resource::<Surfaces>().unwrap_or_default();
                let mut per_square = Vec::with_capacity(square_names.len());
                for name in &square_names {
                    per_square.push(surfaces.index(name).map_err(|e| e.to_string())?);
                    log.count(&format!("ground collision squares of surface {name}"), 1);
                }
                w.insert_resource(surfaces);
                let triangles: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
                let surfaces = per_square.iter().flat_map(|&s| [s, s]).collect();
                w.insert(
                    e,
                    Collision {
                        positions: positions.clone(),
                        triangles,
                        surfaces,
                    },
                );
            }
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Position, Stream::F32x3(positions));
            streams.insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
            w.insert(
                e,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(material),
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
    reg.register_requiring(
        "lower-anvil-material",
        |o| Ok(Box::new(LowerAnvilMaterial::from_options(o)?)),
        &[
            ("normal-green", "up"),
            ("alpha", "opaque"),
            ("unset", "white"),
            ("shader-uv", "texcoord"),
            ("emissive-exposure", "1"),
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
        ],
    );
    reg.register_requiring(
        "lower-anvil-water",
        |o| Ok(Box::new(LowerAnvilWater::from_options(o)?)),
        &[
            ("depth", "3:2,4:1,5:0.5,6:0.5"),
            ("river-depth", "1"),
            ("colour-max", "0.15"),
            ("roughness", "0.05"),
            ("shore", "coverage"),
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
