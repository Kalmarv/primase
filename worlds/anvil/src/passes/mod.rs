//! Anvil's lowerings: its dialect to core meanings, each choice the program's.

use std::collections::BTreeMap;

mod bake;
mod gbuffer;
mod surface;
pub use surface::{BakeAnvilSurface, SurfaceSediments, BAKE_ANVIL_SURFACE};
mod charts;
mod clips;
mod decals;
pub mod formulas;
mod grass;
mod impostor;
mod preview;
mod far;
mod far_terrain;
pub use far_terrain::{LowerAnvilFarTerrain, LOWER_ANVIL_FAR_TERRAIN};
pub use far::{SelectAnvilFarView, SELECT_ANVIL_FAR_VIEW};
mod lights;
mod sky;
pub use sky::{LowerAnvilSky, LOWER_ANVIL_SKY};
pub use lights::{LowerAnvilLights, LOWER_ANVIL_LIGHTS};
pub use preview::{SelectAnvilPreviewLod, SELECT_ANVIL_PREVIEW_LOD};
pub use impostor::{LowerAnvilImpostor, LOWER_ANVIL_IMPOSTOR};
mod listed;
mod material;
mod resolve_shaders;
mod select;
mod select_clips;
mod skeleton;
mod skin;
mod splat;
mod water;
mod world_uv;
pub use bake::{BAKE_ANVIL_MATERIAL, BakeAnvilMaterial, gbuffer_pixel_key};
pub use clips::{LOWER_ANVIL_CLIPS, LowerAnvilClips};
pub use decals::{Family, FamilyRule, LOWER_ANVIL_DECALS, LowerAnvilDecals, TerrainDecals, family};
pub use grass::{Density, LOWER_ANVIL_GRASS, LowerAnvilGrass};
pub use listed::{Coarse, SELECT_ANVIL_LISTED, SelectAnvilListed};
pub use material::{
    AlphaChoice, Decals1Cutout, Emission, LOWER_ANVIL_MATERIAL, LowerAnvilMaterial, ShaderUv,
    fake_river_albedo, world_uv_rows,
};
pub use resolve_shaders::{
    MESH_LAYOUT_FIELD, RESOLVE_ANVIL_SHADERS, ResolveAnvilShaders, UnsetTexture, alpha_tested_key,
    gbuffer_key, gbuffer_vertex_key, material_constants, render_state, shading_class,
};
pub use select::{CollisionOnly, Keep, Overlap, SELECT_ANVIL_ENTITIES, SelectAnvilEntities};
pub use select_clips::{SELECT_ANVIL_CLIPS, SelectAnvilClips};
pub use skeleton::{LOWER_ANVIL_RIG, LOWER_ANVIL_SKELETON, LowerAnvilRig, LowerAnvilSkeleton, OwnTransform};
pub use skin::{LOWER_ANVIL_SKIN, LowerAnvilSkin};
pub use splat::{Splat, SplatUv};
pub use water::{LOWER_ANVIL_WATER, LowerAnvilWater, Shore};
pub use world_uv::{LOWER_ANVIL_WORLD_UV, LowerAnvilWorldUv};

use primase_core::layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassRegistry, PassSpec};
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
    AnvilCollisionShape, AnvilDataLayers, AnvilFloorHeight, AnvilLayout, AnvilLods, AnvilMesh,
    AnvilMeshShape, AnvilPalette, AnvilTerrain, AnvilTextureMap, CollisionMaterialRef, ShapeGeometry,
    TerrainTile,
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

impl Choice for SurfaceBy {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("collision-material", SurfaceBy::CollisionMaterial),
        ("one", SurfaceBy::One),
    ];
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
    /// From its declared options: `surface` not empty.
    fn from_given(g: &Given) -> Result<LowerAnvilCollision, String> {
        let surface = g.required("surface");
        if surface.is_empty() {
            return Err("surface=NAME is not empty: the surface of what has no CollisionMaterial (or every surface, by=one)".into());
        }
        Ok(LowerAnvilCollision {
            by: g.choice("by"),
            surface: surface.to_owned(),
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

/// `by` and `surface`, as `lower-anvil-collision` and `lower-anvil-shapes` take them.
const SURFACE_OPTIONS: [OptionSpec; 2] = [
    OptionSpec::choice::<SurfaceBy>(
        "by",
        "each triangle's surface named by its CollisionMaterial's entry, or every triangle's by surface",
    ),
    OptionSpec::required(
        "surface",
        "rr-diagnostic-unknown",
        "the surface of what has no CollisionMaterial (by=collision-material), or of everything (by=one)",
    ),
];

/// `lower-anvil-collision`'s declaration ([`PassRegistry::add`]): both options are the program's.
pub const LOWER_ANVIL_COLLISION: PassSpec = PassSpec {
    name: "lower-anvil-collision",
    options: &SURFACE_OPTIONS,
    make: |g| Ok(Box::new(LowerAnvilCollision::from_given(g)?)),
};

impl Pass for LowerAnvilCollision {
    fn name(&self) -> &'static str {
        "lower-anvil-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("by".to_owned(), self.by.text().to_owned()),
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

/// `lower-anvil-shapes`' declaration ([`PassRegistry::add`]): both options are the program's, as
/// `lower-anvil-collision`'s.
pub const LOWER_ANVIL_SHAPES: PassSpec = PassSpec {
    name: "lower-anvil-shapes",
    options: &SURFACE_OPTIONS,
    make: |g| {
        let c = LowerAnvilCollision::from_given(g)?;
        Ok(Box::new(LowerAnvilShapes {
            by: c.by,
            surface: c.surface,
        }))
    },
};

impl Pass for LowerAnvilShapes {
    fn name(&self) -> &'static str {
        "lower-anvil-shapes"
    }
    fn options(&self) -> Options {
        Options::from([
            ("by".to_owned(), self.by.text().to_owned()),
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

impl Choice for Combine {
    const VALUES: &'static [(&'static str, Self)] = &[("all", Combine::All), ("any", Combine::Any)];
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

/// `lower-anvil-data-layers`' declaration ([`PassRegistry::add`]): how a placement's actions combine isn't
/// established, so the program states it.
pub const LOWER_ANVIL_DATA_LAYERS: PassSpec = PassSpec {
    name: "lower-anvil-data-layers",
    options: &[OptionSpec::choice::<Combine>(
        "combine",
        "a placement's several DataLayerActions all holding, or any",
    )],
    make: |g| {
        Ok(Box::new(LowerAnvilDataLayers {
            combine: g.choice("combine"),
        }))
    },
};

impl Pass for LowerAnvilDataLayers {
    fn name(&self) -> &'static str {
        "lower-anvil-data-layers"
    }
    fn options(&self) -> Options {
        Options::from([("combine".to_owned(), self.combine.text().to_owned())])
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

/// What `lower-anvil-mesh` takes as a static16 mesh's normal (the far terrain's cells: two spherical vectors, which the
/// pixel stage takes as its normal not read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Static16Normal {
    /// `P` (bytes 10, 11): up on every cell measured (23,951 vertices).
    P,
    /// None (the consumer's flat shading).
    Omit,
}

impl Choice for Static16Normal {
    const VALUES: &'static [(&'static str, Self)] =
        &[("p", Static16Normal::P), ("omit", Static16Normal::Omit)];
}

/// `lower-anvil-mesh{static16-normal=p|omit}`: every Mesh's buffers ([`AnvilMesh`]) become a core mesh on its
/// prototype: one triangle primitive with no material, or, where the reader read its Materials, a primitive per stored
/// range, of the vertices its triangles use, drawn with the range's Material entity. Its streams are the attributes the
/// engine's vertex declarations give ([`crate::read::vertex`], converted through the reader's frame): positions;
/// normals and tangents (unit length, as glTF requires: the vertex programs don't normalise; the tangent's w the
/// bitangent's sign, flipped where the frame mirrors); texture coordinates 0 and 1; the vertex colour as UNORM8 RGBA;
/// and on a mesh with bones the skin: `Joints(k)` (the slot of each influence, U8x4) and `Weights(k)` (U8x4, summing to
/// 255 as stored) for each four influences, kept with the bone list as [`AnvilPalette`] (a core mesh may not hold them
/// before it has a skin) for `lower-anvil-skin`. An influence with no weight stores a slot the vertex doesn't use
/// (anything on the stored mesh): its slot is written as 0, as core's padding. static16 meshes take
/// `static16-normal`'s choice (the one reading not established). The buffers are consumed.
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMesh {
    pub static16_normal: Static16Normal,
}

/// `lower-anvil-mesh`' declaration ([`PassRegistry::add`]): a static16 mesh's normal is not established, so the
/// program states it.
pub const LOWER_ANVIL_MESH: PassSpec = PassSpec {
    name: "lower-anvil-mesh",
    options: &[OptionSpec::choice::<Static16Normal>(
        "static16-normal",
        "a static16 mesh's normal from its P bytes, or none",
    )],
    make: |g| {
        Ok(Box::new(LowerAnvilMesh {
            static16_normal: g.choice("static16-normal"),
        }))
    },
};

/// `v` at unit length (`None` for a zero vector).
fn unit_length(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (len > 0.0).then(|| v.map(|x| x / len))
}

impl Pass for LowerAnvilMesh {
    fn name(&self) -> &'static str {
        "lower-anvil-mesh"
    }
    fn options(&self) -> Options {
        Options::from([(
            "static16-normal".to_owned(),
            self.static16_normal.text().to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME, AnvilPalette::NAME, AnvilLayout::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "static16 meshes' normal a stated choice of their two stored vectors"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<AnvilMesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let Some(m) = w.remove::<AnvilMesh>(e) else {
                continue;
            };
            let n = m.positions.len();
            let a = &m.attributes;
            let flip = m.frame.flips_winding();
            let mut streams = BTreeMap::new();
            for (set, uvs) in (0u8..).zip(&a.uvs) {
                if !uvs.is_empty() && uvs.len() == n {
                    streams.insert(Semantic::TexCoord(set), Stream::F32x2(uvs.clone()));
                }
            }
            if a.uvs[0].is_empty() {
                log.omit(
                    &format!(
                        "texture coordinates: none declared for kind {} stride {}",
                        m.kind, m.stride
                    ),
                    1,
                );
            }
            // normals: the declared ones, or static16's stated vector
            let normals: Vec<[f32; 3]> = match (m.kind, self.static16_normal) {
                (11, Static16Normal::P) => a.spherical.iter().map(|[p, _]| *p).collect(),
                (11, Static16Normal::Omit) => Vec::new(),
                _ => a.normals.clone(),
            };
            if m.kind == 11 && self.static16_normal == Static16Normal::P {
                log.approximate("static16 meshes' normal their spherical vector P (stated)", 1);
            }
            if !normals.is_empty() && normals.len() == n {
                let mut zero = 0u64;
                let converted = normals
                    .iter()
                    .map(|&v| {
                        m.frame.direction(unit_length(v).unwrap_or_else(|| {
                            zero += 1;
                            [0.0, 0.0, 1.0]
                        }))
                    })
                    .collect();
                streams.insert(Semantic::Normal, Stream::F32x3(converted));
                if zero > 0 {
                    log.omit("vertices with a zero normal: up", zero);
                }
                if a.tangents.len() == n {
                    let mut zero = 0u64;
                    let tangents = a
                        .tangents
                        .iter()
                        .map(|&[x, y, z, sign]| {
                            let t = m.frame.direction(unit_length([x, y, z]).unwrap_or_else(|| {
                                zero += 1;
                                [1.0, 0.0, 0.0]
                            }));
                            [t[0], t[1], t[2], if flip { -sign } else { sign }]
                        })
                        .collect();
                    streams.insert(Semantic::Tangent, Stream::F32x4(tangents));
                    if zero > 0 {
                        log.omit("vertices with a zero tangent: +x", zero);
                    }
                }
            }
            if !a.colour.is_empty() && a.colour.len() == n {
                streams.insert(Semantic::Color(0), Stream::U8x4(a.colour.clone()));
            }
            let triangles = m.triangles.len() as u64;
            if let Some(s) = &m.skin {
                let sets = s.skin.influences / 4;
                let mut padding = 0u64;
                for k in 0..sets {
                    let (mut joints, mut weights) = (Vec::with_capacity(n), Vec::with_capacity(n));
                    for (slots, ws) in s.skin.vertices() {
                        let at = k * 4;
                        let (Some(&[s0, s1, s2, s3]), Some(&[w0, w1, w2, w3])) = (
                            slots.get(at..at + 4).and_then(|s| s.first_chunk()),
                            ws.get(at..at + 4).and_then(|w| w.first_chunk()),
                        ) else {
                            return Err(format!(
                                "a skin of {} influences isn't whole sets of four",
                                s.skin.influences
                            ));
                        };
                        let used = [(s0, w0), (s1, w1), (s2, w2), (s3, w3)];
                        padding += used.iter().filter(|&&(slot, w)| w == 0 && slot != 0).count() as u64;
                        joints.push(used.map(|(slot, w)| if w == 0 { 0 } else { slot }));
                        weights.push(used.map(|(_, w)| w));
                    }
                    let set = u8::try_from(k).map_err(|_| "too many influence sets".to_owned())?;
                    streams.insert(Semantic::Joints(set), Stream::U8x4(joints));
                    streams.insert(Semantic::Weights(set), Stream::U8x4(weights));
                }
                if padding > 0 {
                    log.omit("stored slot numbers of zero-weight influences: 0", padding);
                }
                log.count("skinned meshes lowered", 1);
            }
            streams.insert(Semantic::Position, Stream::F32x3(m.positions));
            let mut primitives = if m.material_entities.is_empty() {
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
            if let Some(s) = &m.skin {
                // the skin streams leave the primitives: they come back with the skin
                let mut kept = Vec::with_capacity(primitives.len());
                for p in &mut primitives {
                    let (skin, rest): (BTreeMap<_, _>, BTreeMap<_, _>) = std::mem::take(&mut p.streams)
                        .into_iter()
                        .partition(|(s, _)| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)));
                    p.streams = rest;
                    kept.push(skin);
                }
                w.insert(
                    e,
                    AnvilPalette {
                        frame: m.frame,
                        bones: s.bones.clone(),
                        sets: u8::try_from(s.skin.influences / 4)
                            .map_err(|_| "too many influence sets".to_owned())?,
                        streams: kept,
                    },
                );
            }
            log.count("primitives lowered", primitives.len() as u64);
            w.insert(
                e,
                AnvilLayout {
                    kind: vec![m.kind; primitives.len()],
                },
            );
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

/// `lower-anvil-lods`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_ANVIL_LODS: PassSpec = PassSpec {
    name: "lower-anvil-lods",
    options: &[],
    make: |_| Ok(Box::new(LowerAnvilLods)),
};

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

impl Choice for CellPoint {
    const VALUES: &'static [(&'static str, Self)] =
        &[("corner", CellPoint::Corner), ("centre", CellPoint::Centre)];
}

/// A ground lowering's stated choices: its rectangle, step, height offset and where a sample lies in its cell.
struct Ground {
    rect: [f64; 4],
    step: u32,
    offset: f64,
    sample: CellPoint,
}

/// A ground lowering's `rect` (x0,y0,x1,y1, in the grid's own units).
pub(crate) const RECT: OptionSpec = OptionSpec::required(
    "rect",
    "-64,-64,64,64",
    "x0,y0,x1,y1 in the source's units, x0 < x1 and y0 < y1: the part of the ground lowered",
);
/// A ground lowering's `step`.
pub(crate) const STEP: OptionSpec =
    OptionSpec::required("step", "1", "every step-th column and row, at least 1");
/// A terrain ground's `range`.
pub(crate) const RANGE: OptionSpec = OptionSpec::required(
    "range",
    "1",
    "above 0: a height value's metres are range x 2^(level - 28) (not stored as such)",
);
/// A ground lowering's `offset`.
pub(crate) const OFFSET: OptionSpec =
    OptionSpec::required("offset", "0", "metres added to every height (not stored)");
/// A ground lowering's `sample`.
pub(crate) const SAMPLE: OptionSpec = OptionSpec::choice::<CellPoint>(
    "sample",
    "where a sample lies in its cell: its corner or its centre",
);

/// The ground options ([`RECT`], [`STEP`], [`OFFSET`], [`SAMPLE`]) of a declaration that has them.
fn ground_options(g: &Given) -> Result<Ground, String> {
    let rect: Vec<f64> = g
        .required("rect")
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
    let step: u32 = g
        .required("step")
        .parse()
        .map_err(|_| "step is a whole number".to_owned())?;
    if step == 0 {
        return Err("step is at least 1".to_owned());
    }
    let offset: f64 = g
        .required("offset")
        .parse()
        .map_err(|_| "offset is a number".to_owned())?;
    if !offset.is_finite() {
        return Err(format!("offset {offset}: a finite number"));
    }
    Ok(Ground {
        rect: [x0, y0, x1, y1],
        step,
        offset,
        sample: g.choice("sample"),
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
    /// From its declared options.
    fn from_given(g: &Given) -> Result<LowerAnvilFloor, String> {
        let g = ground_options(g)?;
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

/// `lower-anvil-floor`'s declaration ([`PassRegistry::add`]): the offset isn't stored and the rounding isn't read, so
/// every option is the program's.
pub const LOWER_ANVIL_FLOOR: PassSpec = PassSpec {
    name: "lower-anvil-floor",
    options: &[RECT, STEP, OFFSET, SAMPLE],
    make: |g| Ok(Box::new(LowerAnvilFloor::from_given(g)?)),
};

impl Pass for LowerAnvilFloor {
    fn name(&self) -> &'static str {
        "lower-anvil-floor"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        let sample = self.sample.text();
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
        Ok([
            self.x_of(gx) as f32,
            self.y_of(gy) as f32,
            self.height(gx, gy)? as f32,
        ])
    }

    /// Sample (`gx`, `gy`)'s height in the terrain's source units, in f64: the value at its node's core sample,
    /// `unit` x it + `offset`.
    pub fn height(&self, gx: u32, gy: u32) -> Result<f64, String> {
        let node = (gy / TERRAIN_CORE, gx / TERRAIN_CORE);
        let (i, j) = ((gy % TERRAIN_CORE) as usize, (gx % TERRAIN_CORE) as usize);
        let tile = self
            .tiles
            .get(&node)
            .ok_or_else(|| format!("sample ({gx}, {gy}): node {node:?} not read"))?;
        let value =
            f64::from(tile.heights[(i + TerrainTile::CORE_AT) * TERRAIN_TILE + j + TerrainTile::CORE_AT]);
        Ok(self.unit * value + self.offset)
    }

    /// Every vertex's near-camera normal, row by row as the grid's vertices, in core's frame (`frame`): the engine's
    /// terrain pixel program's `n0` (ps 1908 `TerrainTesselFlatOnly`, ShaderDataDX11.bin container 1908; docs/terrain.md
    /// 3.6 (1)): the unit normal of the height field `z = h(x, y)`, `(-dh/dx, -dh/dy, 1)` in the terrain's source frame,
    /// the slopes the central differences of the terrain's own height samples **one sample either side** along each
    /// axis (the program's `cb0[48].x`, one atlas texel: two samples apart whatever `step` is), one-sided where a
    /// neighbour's node is not read. The heights are in the source frame's units (`range` x the stored value, the
    /// program's `cb4[2].z` factor, plus the offset), the spacing in the same units, so the slopes are rises over runs.
    /// It is computed once over the whole grid, so every piece that draws a vertex draws the same bits.
    pub fn normals(&self, frame: &primase_core::frame::SourceFrame) -> Result<Vec<[f32; 3]>, String> {
        Ok(self
            .normals_source()?
            .into_iter()
            .map(|n| frame.direction(n))
            .collect())
    }

    /// [`Self::normals`] in the terrain's source frame (z up), before the frame's direction map.
    pub fn normals_source(&self) -> Result<Vec<[f32; 3]>, String> {
        let height = |gx: Option<u32>, gy: Option<u32>| match (gx, gy) {
            (Some(gx), Some(gy)) => self.height(gx, gy).ok(),
            _ => None,
        };
        // the slope along one axis from the neighbours' heights and the sample spacing: central, one-sided where
        // one neighbour is missing, flat where both are
        let slope = |before: Option<f64>, here: f64, after: Option<f64>| match (before, after) {
            (Some(b), Some(a)) => (a - b) / (2.0 * self.spacing),
            (Some(b), None) => (here - b) / self.spacing,
            (None, Some(a)) => (a - here) / self.spacing,
            (None, None) => 0.0,
        };
        let mut out = Vec::with_capacity(self.columns.len() * self.rows.len());
        for &gy in &self.rows {
            for &gx in &self.columns {
                let here = self.height(gx, gy)?;
                let dx = slope(
                    height(gx.checked_sub(1), Some(gy)),
                    here,
                    height(gx.checked_add(1), Some(gy)),
                );
                let dy = slope(
                    height(Some(gx), gy.checked_sub(1)),
                    here,
                    height(Some(gx), gy.checked_add(1)),
                );
                let len = (dx * dx + dy * dy + 1.0).sqrt();
                out.push([(-dx / len) as f32, (-dy / len) as f32, (1.0 / len) as f32]);
            }
        }
        Ok(out)
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
/// and interpolated across each triangle (the engine evaluates it per pixel: the approximation); the weight of a
/// material without a detail diffuse goes to the node's far colour (its colour tile, as the splat's fallback), a layer
/// of its own sampling `TexCoord(1)`; a texel past every read node has neither, its weight shared out. Each layer's
/// normal map is its material's detail normal (its TextureSet's slot 1: x and y, z rebuilt; red along the tangent +x,
/// green toward -y with v along +y, the notes' reading of the terrain shader's frame: against core's bitangent
/// `cross(N, T) w` a green-down map). With `splat-occlusion=block-8` each vertex's `COLOR_0` is the shader's occlusion
/// factor `0.5 + b` there (block 8, bilinear), which the colour is multiplied by (it may pass 1). A piece takes
/// at most 4 layers (core's weights are one vec4 a vertex), the far colour one where any weight goes to it: one needing
/// more is cut in quarters, down to 16 squares a side, and one still needing more keeps its 3 heaviest detail layers
/// and the far colour, which takes the rest's weight (an approximation, counted). Left out
/// (counted): the ramps, the masks, the detail normals' flattening on flat ground and the per-cell uv rotation.
///
/// `layers-detail=F` (required, in (0, 1]) weighs each detail layer at F times its material's blend, and the base layer
/// takes what that leaves (with every weight on a detailed material: 1 - F). `layers-base=tile` (required) is the far
/// colour tile, drawn as a layer only where weight goes to it (always, under F < 1; F = 1 is the layers above);
/// `layers-base=bake` (with `splat-px=UNITS`) is the splat's bake of the piece itself, at the same `splat-k`,
/// `splat-uv` and `splat-noise`, as the base of every piece (a piece then holds 3 detail layers and the base),
/// without the occlusion tint (the vertex tint multiplies the whole blend, the base too, so it is not taken twice).
/// A lerp toward each detail's colour at F cannot add the high frequencies the bake averaged away, only stand the
/// details' own over it: counted as an approximation.
///
/// `pieces=whole` draws the ground as one primitive with one texture over `rect`; `pieces=per-node` (with
/// `ground=splat`) as a primitive per terrain node, each square of the grid in the node of its corner sample with the
/// least x and y, each piece with its own near colour baked over its squares' extent (a node's whole square is
/// 128 samples, its side at `splat-px` a pixel: 64 m at 0.25 m is 256 pixels). The pieces' positions are the whole
/// ground's, computed alike, so neighbours share their seam vertices exactly. Each piece (the whole, or a node's) is a
/// prototype of its own, its collision (where drawn) over its own squares, its vertices local to its origin
/// ([`local_origin`]: its box's least corner rounded down to 64 m, which keeps them exact), placed once there.
///
/// With `normals=near-camera` (required: `near-camera` or `none`) every `ground=` and `pieces=` draws **smooth
/// normals**: each vertex's `Normal` is the engine's near-camera terrain normal at its sample, carried to core's frame
/// by the frame's direction map (so it is up whatever the frame, and agrees with the triangles' winding). **Read:** the
/// terrain pixel program (ps 1908 `TerrainTesselFlatOnly`, ShaderDataDX11.bin container 1908; docs/terrain.md 3.6 (1),
/// from the disassembly and executed there) samples the height atlas one texel either side of the pixel on each axis
/// (`cb0[48].x`) and takes `n0 = normalize((h(x-1) - h(x+1), h(y-1) - h(y+1)) * range, 1)` (`range` its `cb4[2].z`):
/// central differences, two samples apart, no stored normal or normal map in that path; here the same on the terrain's
/// own samples, whatever `step` is, one-sided where a neighbour's node is not read. **Not read:** past about 27 to 32 m
/// of the camera the program blends `n0` into the mesh's interpolated vertex normal (a smoothstep of the distance), whose
/// derivation is not read; this pass draws the near-camera normal at every distance, which is the approximation (counted).
/// The normals are computed once over the whole grid and each piece takes its vertices' own, so the vertices neighbouring
/// pieces share (they share their positions exactly) carry the same bits and there is no lighting seam. With
/// `normals=none` the ground has none, and a writer makes flat per-triangle normals, which shade the 0.5 m grid as
/// facets.
///
/// With `steep=chart` (required: `none` or `chart` with `steep-threshold=DEGREES` and `steep-max-atlas=PIXELS`; `chart` needs `ground=splat` and
/// `pieces=per-node`) the triangles steeper than the threshold (by their face normal) leave their node's top-down
/// primitive for a primitive of their own with a **chart atlas**. **Why:** a top-down texture spends `pixel / cos(slope)`
/// of surface on a pixel (8.8 cm at 45 degrees, 36 cm at 80 for 6.25 cm), and the game does not draw a steep face from
/// a top-down lookup at all: its vertical-triangle pixel program (ps 1924 `TerrainTesselVerticalOnly`, docs/terrain.md
/// 3.10 (3), read and executed) addresses the detail diffuse in the vertical planes, at the point's (x, z) and at its
/// (y, z), the two results mixed by `saturate(5 (n.x^2 / (n.x^2 + n.y^2) - 0.4))` of the pixel's normal. **What the pass
/// does:** the steep triangles are grouped into charts (connected by shared edges, looking alike: the larger horizontal
/// component of the face normal, and its sign), each laid out in the plane that holds it ((x, z) for faces looking along
/// y, (y, z) along x), packed per node into one atlas at the top-down bake's pixel (`splat-px`) along both axes, and every
/// texel is baked at the point of the surface it maps to (the face under it, its nearest edge for the two pixels of padding)
/// with that program's colour at the point's smooth normal. The collision is the whole ground's either way. **Ours, not
/// the game's, and counted:** the threshold (the game splits its draw lists into flat, mixed and vertical triangles by a
/// CPU rule that is not read; its vertex stage carries no class), the laying out, a fold of the surface (a second face
/// claiming a chart pixel keeps the first's), and an atlas that would pass 4,096 pixels a side, built at twice the pixel
/// size until it fits. The vertical program's other terms (the steep-slope detail normal `t83`, the plane-frame normal
/// map) are colour-free and left out, as the top-down bake leaves out the normals.
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
    /// The ground's normals (`normals=`): the program's stated choice, the game's own not read.
    pub normals: GroundNormals,
    /// The steep ground (`steep=`): the program's stated choice, the game's own class rule not read.
    pub steep: Steep,
}

/// How `lower-anvil-terrain` draws the ground steeper than an angle (`steep=`, required; `chart` needs `ground=splat`
/// and `pieces=per-node`): the engine draws steep triangles with the vertical-triangle pixel program, which addresses the
/// details in the vertical planes (docs/terrain.md 3.10 (3)), and a top-down texture holds them at `pixel / cos(slope)`
/// of surface a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Steep {
    /// Every triangle in its node's top-down primitive and texture.
    None,
    /// The triangles steeper than `threshold` degrees (by their face normal) are taken out of the top-down primitive into
    /// one primitive per node with a chart atlas: each connected face laid out in the vertical plane it looks along,
    /// at the top-down bake's pixel, each texel baked at its point of the surface with the vertical program's colour
    /// ([`splat::DetailPlane::Vertical`]). The threshold is this pass's choice: the game's rule that splits its draw
    /// lists into flat, mixed and vertical triangles is not read.
    Chart {
        threshold: f64,
        /// An atlas's greatest side in pixels (`steep-max-atlas`): the program's choice, its destination's texture cap.
        /// A node whose charts do not fit are baked at twice the pixel, again until they do, counted.
        max_atlas: u32,
    },
}

/// The normals `lower-anvil-terrain` gives the ground (`normals=`, required): the game's terrain pixel program's
/// near-camera normal is read, the far normal it blends toward is not, so the program states which it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundNormals {
    /// The pixel program's near-camera normal `n0` at each sample, at every distance ([`GroundGrid::normals`]): read
    /// within about 27 m, an approximation beyond about 32 m (the mesh vertex normal the game blends to is not read),
    /// counted.
    NearCamera,
    /// None: a writer makes its own (flat per triangle, faceting the grid).
    None,
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
/// the splat's, and the base the details lie over (`layers-base`) at what weight (`layers-detail`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainLayers {
    pub k: f64,
    pub uv: SplatUv,
    pub noise_srgb: bool,
    pub occlusion: bool,
    /// The layer the details lie over: what the weight they don't take is drawn with.
    pub base: LayersBase,
    /// `layers-detail`: each detail layer's weight is its material's splat weight times this, in (0, 1]; the base
    /// takes the rest (so 1 draws the details alone, where a base is only the far colour of what has no detail).
    pub detail: f64,
}

/// The base layer of `ground=layers` (`layers-base`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayersBase {
    /// The nodes' far colour tiles (the mosaic `ground=tile` draws).
    Tile,
    /// The near colour baked over the piece, at `pixel` source units a pixel (`splat-px`): the splat's bake of the
    /// same `splat-k`, `splat-uv` and `splat-noise`, without the occlusion (the vertex tint multiplies the blend).
    Bake { pixel: f64 },
}

/// A terrain tile's side in samples (and in colour texels), the samples of it its node covers, and its colour tile's
/// side in 4 x 4 blocks.
const TERRAIN_TILE: usize = 132;
const TERRAIN_CORE: u32 = 128;
const TERRAIN_BLOCKS: usize = 33;

impl LowerAnvilTerrain {
    /// From its declared options: the splat, layers and steep options only with the choices that take them.
    fn from_given(g: &Given) -> Result<LowerAnvilTerrain, String> {
        let ground_options = ground_options(g)?;
        let given = |k: &str| g.optional(k).is_some();
        let needed = |k: &str, with: &str| {
            g.optional(k)
                .ok_or_else(|| format!("{k} is required with {with}"))
        };
        let number = |k: &str, with: &str| -> Result<f64, String> {
            let v: f64 = needed(k, with)?.parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() && v > 0.0 {
                Ok(v)
            } else {
                Err(format!("{k} {v}: a positive number"))
            }
        };
        let uv = |with: &str| -> Result<SplatUv, String> {
            Ok(match needed("splat-uv", with)? {
                "over-tile-size" => SplatUv::OverTileSize,
                _ => SplatUv::Factor(number("splat-uv", with)?),
            })
        };
        let word = |k: &str, with: &str| -> Result<(), String> { needed(k, with).map(|_| ()) };
        let ground = match g.choice::<GroundKind>("ground") {
            GroundKind::Tile => {
                if let Some(k) = SPLAT_KEYS.iter().find(|k| given(k)) {
                    return Err(format!("{k} is only taken with ground=splat or layers"));
                }
                GroundColour::Tile
            }
            GroundKind::Splat => {
                if let Some(k) = ["layers-base", "layers-detail"].iter().find(|k| given(k)) {
                    return Err(format!("{k} is only taken with ground=layers"));
                }
                word("splat-noise", "ground=splat")?;
                word("splat-occlusion", "ground=splat")?;
                GroundColour::Splat(Splat {
                    k: number("splat-k", "ground=splat")?,
                    pixel: number("splat-px", "ground=splat")?,
                    uv: uv("ground=splat")?,
                    noise_srgb: g.optional_choice("splat-noise") == Some(SplatNoise::Srgb),
                    occlusion: g.optional_choice("splat-occlusion") == Some(SplatOcclusion::Block8),
                })
            }
            GroundKind::Layers => {
                let base = match g.optional_choice::<LayersBaseKind>("layers-base") {
                    Some(LayersBaseKind::Tile) => {
                        if given("splat-px") {
                            return Err("splat-px is only taken with ground=splat or layers-base=bake".into());
                        }
                        LayersBase::Tile
                    }
                    Some(LayersBaseKind::Bake) => LayersBase::Bake {
                        pixel: number("splat-px", "layers-base=bake")?,
                    },
                    None => return Err("layers-base is required with ground=layers".into()),
                };
                let detail = number("layers-detail", "ground=layers")?;
                if detail > 1.0 {
                    return Err(format!("layers-detail {detail}: a number in (0, 1]"));
                }
                word("splat-noise", "ground=layers")?;
                word("splat-occlusion", "ground=layers")?;
                GroundColour::Layers(TerrainLayers {
                    base,
                    detail,
                    k: number("splat-k", "ground=layers")?,
                    uv: uv("ground=layers")?,
                    noise_srgb: g.optional_choice("splat-noise") == Some(SplatNoise::Srgb),
                    occlusion: g.optional_choice("splat-occlusion") == Some(SplatOcclusion::Block8),
                })
            }
        };
        let collision = match g.choice::<TerrainCollision>("collision") {
            TerrainCollision::None => None,
            TerrainCollision::CollisionMaterial => Some(SurfaceBy::CollisionMaterial),
            TerrainCollision::One => Some(SurfaceBy::One),
        };
        let surface = match (collision, g.optional("surface").filter(|s| !s.is_empty())) {
            (None, None) => String::new(),
            (_, Some(s)) => s.to_owned(),
            (Some(_), None) => return Err("surface is required with collision other than none".into()),
        };
        let colour = match g.choice::<Colour>("colour") {
            Colour::Srgb => ColorSpace::Srgb,
            Colour::Linear => ColorSpace::Linear,
        };
        let pieces = g.choice::<Pieces>("pieces");
        if pieces == Pieces::PerNode && matches!(ground, GroundColour::Tile) {
            return Err(
                "pieces=per-node needs ground=splat or layers (a node's colour tile is sample-centred: it doesn't cover its \
                 last squares)"
                    .into(),
            );
        }
        let range: f64 = g
            .required("range")
            .parse()
            .map_err(|_| "range is a number".to_owned())?;
        if !(range.is_finite() && range > 0.0) {
            return Err(format!("range {range}: a positive number"));
        }
        let steep = match g.choice::<SteepKind>("steep") {
            SteepKind::None => {
                if let Some(k) = ["steep-threshold", "steep-max-atlas"].iter().find(|k| given(k)) {
                    return Err(format!("{k} goes with steep=chart only"));
                }
                Steep::None
            }
            SteepKind::Chart => {
                if !matches!((ground, pieces), (GroundColour::Splat(_), Pieces::PerNode)) {
                    return Err(
                        "steep=chart needs ground=splat and pieces=per-node (a chart atlas per node)"
                            .to_owned(),
                    );
                }
                let v = needed("steep-threshold", "steep=chart")?;
                let threshold: f64 = v
                    .parse()
                    .map_err(|_| format!("steep-threshold={v} is not a number"))?;
                if !(threshold > 0.0 && threshold < 90.0) {
                    return Err(format!(
                        "steep-threshold={v}: degrees from the horizontal, 0 < T < 90"
                    ));
                }
                let v = needed("steep-max-atlas", "steep=chart")?;
                let max_atlas: u32 = v
                    .parse()
                    .map_err(|_| format!("steep-max-atlas={v}: a whole number of pixels"))?;
                if !(16..=65_536).contains(&max_atlas) {
                    return Err(format!("steep-max-atlas={v}: pixels a side, 16 to 65536"));
                }
                Steep::Chart { threshold, max_atlas }
            }
        };
        Ok(LowerAnvilTerrain {
            rect: ground_options.rect,
            step: ground_options.step,
            range,
            offset: ground_options.offset,
            sample: ground_options.sample,
            colour,
            collision,
            surface,
            ground,
            pieces,
            normals: g.choice("normals"),
            steep,
        })
    }
}

/// `splat-noise`'s word for whether the noise is decoded as sRGB.
fn noise_text(srgb: bool) -> &'static str {
    if srgb { SplatNoise::Srgb } else { SplatNoise::Raw }.text()
}

/// `splat-occlusion`'s word for whether the colour is multiplied by the occlusion.
fn occlusion_text(occlusion: bool) -> &'static str {
    if occlusion {
        SplatOcclusion::Block8
    } else {
        SplatOcclusion::None
    }
    .text()
}

/// The options only `ground=splat` or `ground=layers` takes.
const SPLAT_KEYS: [&str; 7] = [
    "splat-k",
    "splat-px",
    "splat-uv",
    "splat-noise",
    "splat-occlusion",
    "layers-base",
    "layers-detail",
];

/// Option `colour`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colour {
    Srgb,
    Linear,
}

impl Choice for Colour {
    const VALUES: &'static [(&'static str, Self)] = &[("srgb", Colour::Srgb), ("linear", Colour::Linear)];
}

/// Option `collision`: none, or the surfaces as [`SurfaceBy`] names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerrainCollision {
    None,
    CollisionMaterial,
    One,
}

impl Choice for TerrainCollision {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("none", TerrainCollision::None),
        ("collision-material", TerrainCollision::CollisionMaterial),
        ("one", TerrainCollision::One),
    ];
}

/// Option `ground`: [`GroundColour`]'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroundKind {
    Tile,
    Splat,
    Layers,
}

impl Choice for GroundKind {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("tile", GroundKind::Tile),
        ("splat", GroundKind::Splat),
        ("layers", GroundKind::Layers),
    ];
}

/// Option `splat-noise`: the noise decoded as stored, or as sRGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplatNoise {
    Raw,
    Srgb,
}

impl Choice for SplatNoise {
    const VALUES: &'static [(&'static str, Self)] = &[("raw", SplatNoise::Raw), ("srgb", SplatNoise::Srgb)];
}

/// Option `splat-occlusion`: the colour times `0.5 + block 8`, or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplatOcclusion {
    Block8,
    None,
}

impl Choice for SplatOcclusion {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("block-8", SplatOcclusion::Block8),
        ("none", SplatOcclusion::None),
    ];
}

/// Option `layers-base`: [`LayersBase`]'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayersBaseKind {
    Tile,
    Bake,
}

impl Choice for LayersBaseKind {
    const VALUES: &'static [(&'static str, Self)] =
        &[("tile", LayersBaseKind::Tile), ("bake", LayersBaseKind::Bake)];
}

/// Option `steep`: [`Steep`]'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SteepKind {
    None,
    Chart,
}

impl Choice for SteepKind {
    const VALUES: &'static [(&'static str, Self)] = &[("none", SteepKind::None), ("chart", SteepKind::Chart)];
}

impl Choice for GroundNormals {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("near-camera", GroundNormals::NearCamera),
        ("none", GroundNormals::None),
    ];
}

impl Choice for Pieces {
    const VALUES: &'static [(&'static str, Self)] =
        &[("whole", Pieces::Whole), ("per-node", Pieces::PerNode)];
}

/// `lower-anvil-terrain`'s declaration ([`PassRegistry::add`]): every choice the game's data doesn't settle is the
/// program's; the splat, layers and steep options only with the choices that take them.
pub const LOWER_ANVIL_TERRAIN: PassSpec = PassSpec {
    name: "lower-anvil-terrain",
    options: &[
        RECT,
        STEP,
        RANGE,
        OFFSET,
        SAMPLE,
        OptionSpec::choice::<Colour>("colour", "the colour tiles' encoding: sRGB or linear"),
        OptionSpec::choice::<TerrainCollision>(
            "collision",
            "the ground's collision: none, its surfaces named by CollisionMaterial, or all of one surface",
        ),
        OptionSpec::optional(
            "surface",
            "rr-diagnostic-unknown",
            "the surface where no CollisionMaterial names one (or of every square, collision=one)",
            "required with collision other than none",
        ),
        OptionSpec::choice::<GroundKind>(
            "ground",
            "the colour drawn: the far colour tiles, the near colour baked, or the near colour as detail layers",
        ),
        OptionSpec::optional(
            "splat-k",
            "0.5",
            "the near colour blend's k, above 0 (ground=splat or layers)",
            "required with ground=splat or layers, not taken with tile",
        ),
        OptionSpec::optional(
            "splat-px",
            "1",
            "source units a baked pixel, above 0 (ground=splat, or layers with layers-base=bake)",
            "required with ground=splat or layers-base=bake, not taken otherwise",
        ),
        OptionSpec::optional(
            "splat-uv",
            "1",
            "the detail uv: a factor above 0, or over-tile-size (ground=splat or layers)",
            "required with ground=splat or layers, not taken with tile",
        ),
        OptionSpec::optional_choice::<SplatNoise>(
            "splat-noise",
            "the noise decoded as stored or as sRGB (ground=splat or layers)",
            "required with ground=splat or layers, not taken with tile",
        ),
        OptionSpec::optional_choice::<SplatOcclusion>(
            "splat-occlusion",
            "the colour times 0.5 + block 8, or not (ground=splat or layers)",
            "required with ground=splat or layers, not taken with tile",
        ),
        OptionSpec::optional_choice::<LayersBaseKind>(
            "layers-base",
            "what the details lie over: the far colour tiles, or the near colour baked (ground=layers)",
            "required with ground=layers, not taken otherwise",
        ),
        OptionSpec::optional(
            "layers-detail",
            "1",
            "each detail layer's weight times this, in (0, 1] (ground=layers)",
            "required with ground=layers, not taken otherwise",
        ),
        OptionSpec::choice::<Pieces>(
            "pieces",
            "one primitive over the rectangle, or one per terrain node (per-node with ground=splat or layers)",
        ),
        OptionSpec::choice::<GroundNormals>(
            "normals",
            "the pixel program's near-camera normal at each sample, or none (a writer makes its own)",
        ),
        OptionSpec::choice::<SteepKind>(
            "steep",
            "steep triangles in the top-down texture, or in a chart atlas per node (chart with ground=splat and \
             pieces=per-node)",
        ),
        OptionSpec::optional(
            "steep-threshold",
            "45",
            "degrees from the horizontal, 0 < T < 90: what is steep (steep=chart)",
            "required with steep=chart, not taken with none",
        ),
        OptionSpec::optional(
            "steep-max-atlas",
            "4096",
            "pixels a side, 16 to 65536: an atlas's greatest side (steep=chart)",
            "required with steep=chart, not taken with none",
        ),
    ],
    make: |g| Ok(Box::new(LowerAnvilTerrain::from_given(g)?)),
};

impl Pass for LowerAnvilTerrain {
    fn name(&self) -> &'static str {
        "lower-anvil-terrain"
    }
    fn options(&self) -> Options {
        let [x0, y0, x1, y1] = self.rect;
        let sample = self.sample.text();
        Options::from([
            ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ("step".to_owned(), self.step.to_string()),
            ("range".to_owned(), self.range.to_string()),
            ("offset".to_owned(), self.offset.to_string()),
            ("sample".to_owned(), sample.to_owned()),
            (
                "colour".to_owned(),
                match self.colour {
                    ColorSpace::Srgb => Colour::Srgb,
                    ColorSpace::Linear => Colour::Linear,
                }
                .text()
                .to_owned(),
            ),
            (
                "collision".to_owned(),
                match self.collision {
                    None => TerrainCollision::None,
                    Some(SurfaceBy::CollisionMaterial) => TerrainCollision::CollisionMaterial,
                    Some(SurfaceBy::One) => TerrainCollision::One,
                }
                .text()
                .to_owned(),
            ),
        ])
        .into_iter()
        .chain((!self.surface.is_empty()).then(|| ("surface".to_owned(), self.surface.clone())))
        .chain([
            ("pieces".to_owned(), self.pieces.text().to_owned()),
            ("normals".to_owned(), self.normals.text().to_owned()),
        ])
        .chain(match self.steep {
            Steep::None => vec![("steep".to_owned(), SteepKind::None.text().to_owned())],
            Steep::Chart { threshold, max_atlas } => vec![
                ("steep".to_owned(), SteepKind::Chart.text().to_owned()),
                ("steep-threshold".to_owned(), threshold.to_string()),
                ("steep-max-atlas".to_owned(), max_atlas.to_string()),
            ],
        })
        .chain(match self.ground {
            GroundColour::Tile => vec![("ground".to_owned(), GroundKind::Tile.text().to_owned())],
            GroundColour::Splat(s) => vec![
                ("ground".to_owned(), GroundKind::Splat.text().to_owned()),
                ("splat-k".to_owned(), s.k.to_string()),
                ("splat-px".to_owned(), s.pixel.to_string()),
                (
                    "splat-uv".to_owned(),
                    match s.uv {
                        SplatUv::Factor(f) => f.to_string(),
                        SplatUv::OverTileSize => "over-tile-size".to_owned(),
                    },
                ),
                ("splat-noise".to_owned(), noise_text(s.noise_srgb).to_owned()),
                (
                    "splat-occlusion".to_owned(),
                    occlusion_text(s.occlusion).to_owned(),
                ),
            ],
            GroundColour::Layers(l) => {
                let mut v = vec![
                    ("ground".to_owned(), GroundKind::Layers.text().to_owned()),
                    ("layers-detail".to_owned(), l.detail.to_string()),
                    (
                        "layers-base".to_owned(),
                        match l.base {
                            LayersBase::Tile => LayersBaseKind::Tile,
                            LayersBase::Bake { .. } => LayersBaseKind::Bake,
                        }
                        .text()
                        .to_owned(),
                    ),
                    ("splat-k".to_owned(), l.k.to_string()),
                    (
                        "splat-uv".to_owned(),
                        match l.uv {
                            SplatUv::Factor(f) => f.to_string(),
                            SplatUv::OverTileSize => "over-tile-size".to_owned(),
                        },
                    ),
                    ("splat-noise".to_owned(), noise_text(l.noise_srgb).to_owned()),
                    (
                        "splat-occlusion".to_owned(),
                        occlusion_text(l.occlusion).to_owned(),
                    ),
                ];
                if let LayersBase::Bake { pixel } = l.base {
                    v.push(("splat-px".to_owned(), pixel.to_string()));
                }
                v
            }
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
         stated choices; its normals, where stated, the near-camera normal at every distance; its steep ground, where \
         charted, above a threshold of this pass's choice (the game's triangle class rule is not read)"
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
            let mut positions = Vec::with_capacity(columns.len() * rows.len());
            for &gy in rows {
                for &gx in columns {
                    positions.push(t.frame.point(grid.point(gx, gy)?));
                }
            }
            // with normals=near-camera, smooth normals: one per sample of the whole grid, shared by every piece
            let normals = match self.normals {
                GroundNormals::NearCamera => {
                    let n = grid.normals(&t.frame)?;
                    log.approximate(
                        "ground normals as the terrain pixel program's near-camera normal at every distance \
                         (normals=near-camera; the mesh vertex normal it blends to beyond about 32 m is not read)",
                        n.len() as u64,
                    );
                    Some(n)
                }
                GroundNormals::None => None,
            };
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
                        tiles,
                        spacing,
                        shift,
                        flip,
                    },
                    log,
                )?,
                GroundColour::Tile => {
                    let mosaic = ColourMosaic::new(tiles, rows, columns, self.colour)?;
                    let mut uvs = Vec::with_capacity(positions.len());
                    for &gy in rows {
                        for &gx in columns {
                            uvs.push(mosaic.uv(gx, gy));
                        }
                    }
                    log.count("terrain nodes used", tiles.len() as u64);
                    log.count("ground samples", positions.len() as u64);
                    log.count("ground triangles", (indices.len() / 3) as u64);
                    log.count(
                        "ground colour texels",
                        u64::from(mosaic.texture.width) * u64::from(mosaic.texture.height),
                    );
                    log.approximate(
                        "ground samples at a stated range and offset",
                        positions.len() as u64,
                    );
                    log.approximate("ground drawn with its colour tiles alone", 1);
                    let texture = w.spawn();
                    w.insert(texture, mosaic.texture);
                    vec![GroundPiece {
                        texture: Some(texture),
                        layered: None,
                        uvs,
                        vertices: None,
                        indices: indices.clone(),
                        squares: (0..square_count).collect(),
                        steep: None,
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
                                steep: None,
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
                                    steep: None,
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
            let mut pieces = pieces;
            if let (Steep::Chart { threshold, max_atlas }, GroundColour::Splat(splat)) =
                (self.steep, self.ground)
            {
                self.steep_pieces(w, &t, &grid, &mut pieces, (threshold, max_atlas, &splat), log)?;
            }
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
            for (k, mut piece) in pieces.into_iter().enumerate() {
                let prototype = if k == 0 { e } else { w.spawn() };
                let steep = piece.steep.take();
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
                            far_uvs,
                        }),
                        _,
                    ) => {
                        streams.insert(Semantic::Color(1), Stream::F32x4(weights));
                        if let Some(o) = occlusion {
                            streams.insert(Semantic::Color(0), Stream::F32x4(o));
                        }
                        if let Some(uvs) = far_uvs {
                            streams.insert(Semantic::TexCoord(1), Stream::F32x2(uvs));
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
                if let Some(part) = steep {
                    // the flat remainder keeps the top-down texture, the charts have their atlas
                    let local_of = |vertices: &[u32]| -> Vec<[f32; 3]> {
                        vertices
                            .iter()
                            .map(|&i| std::array::from_fn(|a| positions[i as usize][a] - origin[a]))
                            .collect()
                    };
                    let primitive = |part: DrawPart, material: Entity| {
                        let mut streams = BTreeMap::new();
                        streams.insert(Semantic::Position, Stream::F32x3(local_of(&part.vertices)));
                        if let Some(normals) = &normals {
                            streams.insert(
                                Semantic::Normal,
                                Stream::F32x3(part.vertices.iter().map(|&i| normals[i as usize]).collect()),
                            );
                        }
                        streams.insert(Semantic::TexCoord(0), Stream::F32x2(part.uvs));
                        Primitive {
                            topology: Topology::Triangles,
                            streams,
                            indices: Some(part.indices),
                            material: Some(material),
                        }
                    };
                    let textured = |w: &mut World, texture: Entity| {
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
                    };
                    let mut primitives = Vec::with_capacity(2);
                    if !part.flat.indices.is_empty() {
                        primitives.push(primitive(part.flat, material));
                    }
                    let charts = textured(w, part.atlas);
                    primitives.push(primitive(part.charts, charts));
                    w.insert(prototype, Mesh::new(primitives));
                } else {
                    streams.insert(Semantic::Position, Stream::F32x3(local));
                    if let Some(normals) = &normals {
                        streams.insert(
                            Semantic::Normal,
                            Stream::F32x3(match &piece.vertices {
                                None => normals.clone(),
                                Some(v) => v.iter().map(|&i| normals[i as usize]).collect(),
                            }),
                        );
                    }
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
                }
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

impl LowerAnvilTerrain {
    /// `steep=chart`: each per-node piece's triangles steeper than `threshold` degrees (by their face normal) are taken
    /// out of its top-down primitive into charts in one atlas ([`charts`]), the atlas baked at the points of the surface its
    /// texels map to with the vertical program's colour ([`splat::DetailPlane::Vertical`]); the piece keeps its
    /// triangles for the collision, which is the whole ground's either way.
    fn steep_pieces(
        &self,
        w: &mut World,
        t: &AnvilTerrain,
        grid: &GroundGrid,
        pieces: &mut [GroundPiece],
        (threshold, max_atlas, splat): (f64, u32, &Splat),
        log: &mut PassLog,
    ) -> Result<(), String> {
        let width = grid.columns.len();
        // every vertex's point and smooth normal, in the terrain's source frame (where z is up and the details are
        // addressed)
        let mut points = Vec::with_capacity(width * grid.rows.len());
        for &gy in &grid.rows {
            for &gx in &grid.columns {
                points.push([grid.x_of(gx), grid.y_of(gy), grid.height(gx, gy)?]);
            }
        }
        let normals: Vec<[f64; 3]> = grid
            .normals_source()?
            .into_iter()
            .map(|n| n.map(f64::from))
            .collect();
        let point = |v: u32| points[v as usize];
        let normal = |v: u32| normals[v as usize];
        struct Made {
            piece: usize,
            flat: DrawPart,
            charts: DrawPart,
            atlas: Texture,
        }
        let mut made: Vec<Made> = Vec::new();
        let (mut triangles, mut charted, mut texels) = (0u64, 0u64, 0u64);
        let (mut overlaps, mut outside, mut fallback, mut halved) = (0u64, 0u64, 0u64, 0u64);
        {
            let mut baker = splat::Baker::new(w, t, splat, self.colour)?;
            for (k, piece) in pieces.iter().enumerate() {
                let Some(vertices) = &piece.vertices else {
                    continue;
                };
                let tris: Vec<[u32; 3]> = piece
                    .indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|tri| tri.map(|i| vertices[i as usize]))
                    .collect();
                let Some(split) = charts::split(&point, &tris, threshold, splat.pixel, max_atlas) else {
                    continue;
                };
                // the atlas: opaque, each chart texel baked at its point of the surface
                let [atlas_w, atlas_h] = split.size;
                let mut rgba = vec![255u8; atlas_w as usize * atlas_h as usize * 4];
                for chart in &split.charts {
                    let found = charts::texels(chart, &tris, &point, &normal, split.pixel);
                    overlaps += found.overlaps;
                    for (idx, sample) in found.samples.iter().enumerate() {
                        let Some(sample) = sample else {
                            continue;
                        };
                        texels += 1;
                        let (i, j) = (idx as u32 % chart.size[0], idx as u32 / chart.size[0]);
                        let p = sample.point;
                        match baker.texel(
                            p[0],
                            p[1],
                            p[2],
                            splat::DetailPlane::vertical(sample.normal),
                            split.pixel,
                        )? {
                            Some(texel) => {
                                let at = (((chart.at[1] + j) * atlas_w + chart.at[0] + i) * 4) as usize;
                                rgba[at..at + 3].copy_from_slice(&texel.rgb);
                                fallback += u64::from(texel.tile_fallback);
                            }
                            None => outside += 1,
                        }
                    }
                }
                // the flat remainder: the triangles that stay, over the vertices they use, in first use
                let mut flat = DrawPart {
                    vertices: Vec::new(),
                    uvs: Vec::new(),
                    indices: Vec::new(),
                };
                let mut kept: BTreeMap<u32, u32> = BTreeMap::new();
                for (ti, tri) in piece.indices.as_chunks::<3>().0.iter().enumerate() {
                    if split.steep[ti] {
                        continue;
                    }
                    for &li in tri {
                        let next = flat.vertices.len() as u32;
                        let at = *kept.entry(li).or_insert_with(|| {
                            flat.vertices.push(vertices[li as usize]);
                            flat.uvs.push(piece.uvs[li as usize]);
                            next
                        });
                        flat.indices.push(at);
                    }
                }
                // the charts: each chart's triangles over its own copies of the vertices
                let mut drawn = DrawPart {
                    vertices: Vec::new(),
                    uvs: Vec::new(),
                    indices: Vec::new(),
                };
                for chart in &split.charts {
                    let mut own: BTreeMap<u32, u32> = BTreeMap::new();
                    for &ti in &chart.triangles {
                        for v in tris[ti] {
                            let next = drawn.vertices.len() as u32;
                            let at = *own.entry(v).or_insert_with(|| {
                                drawn.vertices.push(v);
                                drawn
                                    .uvs
                                    .push(charts::uv_of(chart, split.size, split.pixel, point(v)));
                                next
                            });
                            drawn.indices.push(at);
                        }
                    }
                }
                triangles += split.steep.iter().filter(|&&s| s).count() as u64;
                charted += split.charts.len() as u64;
                halved += u64::from(split.halvings > 0);
                made.push(Made {
                    piece: k,
                    flat,
                    charts: drawn,
                    atlas: Texture {
                        width: atlas_w,
                        height: atlas_h,
                        format: TexFormat::Rgba8Unorm,
                        levels: vec![rgba],
                        color_space: ColorSpace::Srgb,
                        role: TexRole::Color,
                        normal: None,
                    },
                });
            }
        }
        let atlases = made.len() as u64;
        for m in made {
            let atlas = w.spawn();
            w.insert(atlas, m.atlas);
            pieces[m.piece].steep = Some(SteepPart {
                flat: m.flat,
                charts: m.charts,
                atlas,
            });
        }
        log.count("steep ground triangles drawn from charts", triangles);
        log.count("ground charts", charted);
        log.count("ground chart atlases", atlases);
        log.count("ground chart pixels baked", texels);
        log.approximate(
            "steep ground drawn from charts above a threshold of this pass's choice (the game's triangle class rule is \
             not read)",
            triangles,
        );
        if halved > 0 {
            log.approximate(
                &format!(
                    "chart atlases built at a coarser pixel than the top-down bake's (they would pass steep-max-atlas={max_atlas} pixels a side)"
                ),
                halved,
            );
        }
        if overlaps > 0 {
            log.omit(
                "chart pixels claimed by a second face of the same chart (a fold of the surface): the first face's point",
                overlaps,
            );
        }
        if outside > 0 {
            log.omit("chart pixels outside every read node: left white", outside);
        }
        if fallback > 0 {
            log.omit(
                "chart pixels a material of which has no detail diffuse: its far colour tile there",
                fallback,
            );
        }
        Ok(())
    }
}

/// A `ground=layers` piece's colour: its layered material, its layer weights per vertex, its occlusion tint per
/// vertex where stated, and its far colour layer's coordinates (`TexCoord(1)`) where it has one.
struct LayeredPiece {
    material: Entity,
    weights: Vec<[f32; 4]>,
    occlusion: Option<Vec<[f32; 4]>>,
    far_uvs: Option<Vec<[f32; 2]>>,
}

/// The far colour of the nodes a ground's samples lie in: each node's 128 x 128 core texels (from
/// [`TerrainTile::CORE_AT`], not on its BC1 blocks' edges), decoded (BC1's 8-bit decode is its meaning) and laid side
/// by side, each sample at its own texel's centre.
struct ColourMosaic {
    texture: Texture,
    /// The first node's row and column.
    first: (u32, u32),
}

impl ColourMosaic {
    fn new(
        tiles: &BTreeMap<(u32, u32), &TerrainTile>,
        rows: &[u32],
        columns: &[u32],
        colour: ColorSpace,
    ) -> Result<Self, String> {
        let (first_row, first_column) = (rows[0] / TERRAIN_CORE, columns[0] / TERRAIN_CORE);
        let (node_rows, node_columns) = (
            rows[rows.len() - 1] / TERRAIN_CORE - first_row + 1,
            columns[columns.len() - 1] / TERRAIN_CORE - first_column + 1,
        );
        let (texels_wide, texels_high) = (node_columns * TERRAIN_CORE, node_rows * TERRAIN_CORE);
        let core = TERRAIN_CORE as usize;
        let mut rgba = vec![0u8; texels_wide as usize * texels_high as usize * 4];
        for nr in 0..node_rows {
            for nc in 0..node_columns {
                let node = (first_row + nr, first_column + nc);
                let tile = tiles
                    .get(&node)
                    .ok_or_else(|| format!("node {node:?} not read"))?;
                let stored = Texture {
                    width: TERRAIN_TILE as u32,
                    height: TERRAIN_TILE as u32,
                    format: TexFormat::Bc1,
                    levels: vec![tile.colour.clone()],
                    color_space: colour,
                    role: TexRole::Color,
                    normal: None,
                };
                let meta = primase_image::TextureMetadata {
                    color_space: colour,
                    role: TexRole::Color,
                    normal: None,
                };
                let decoded = primase_image::map_rgba8_levels(&stored, 0..1, meta, |x| Ok(x.rgba))
                    .map_err(|e| format!("node {node:?}'s colour tile: {e}"))?;
                let texels = &decoded.levels[0];
                for i in 0..core {
                    let from = ((i + TerrainTile::CORE_AT) * TERRAIN_TILE + TerrainTile::CORE_AT) * 4;
                    let row = nr as usize * core + i;
                    let to = (row * texels_wide as usize + nc as usize * core) * 4;
                    rgba[to..to + core * 4].copy_from_slice(&texels[from..from + core * 4]);
                }
            }
        }
        Ok(Self {
            texture: Texture {
                width: texels_wide,
                height: texels_high,
                format: TexFormat::Rgba8Unorm,
                levels: vec![rgba],
                color_space: colour,
                role: TexRole::Color,
                normal: None,
            },
            first: (first_row, first_column),
        })
    }

    /// Sample (`gx`, `gy`)'s texel centre.
    fn uv(&self, gx: u32, gy: u32) -> [f32; 2] {
        let centre = |g: u32, first: u32, texels: u32| {
            ((f64::from(g - first * TERRAIN_CORE) + 0.5) / f64::from(texels)) as f32
        };
        [
            centre(gx, self.first.1, self.texture.width),
            centre(gy, self.first.0, self.texture.height),
        ]
    }
}

/// Squares of the ground grid, by (row, column).
type Squares = Vec<(u32, u32)>;

/// The ground grid `ground=layers` draws: its sample indices along each axis, the nodes they lie in, their spacing and
/// shift, and whether the frame flips the winding.
struct LayerGrid<'g> {
    rows: &'g [u32],
    columns: &'g [u32],
    tiles: &'g BTreeMap<(u32, u32), &'g TerrainTile>,
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
        // a base layer every piece has: the bake, or the far colour where the details take less than all the weight
        let base_always = matches!(layers.base, LayersBase::Bake { .. }) || layers.detail < 1.0;
        let (width, height) = (g.columns.len(), g.rows.len());
        let x_of = |gx: u32| t.origin[0] + g.spacing * f64::from(gx) + g.shift;
        let y_of = |gy: u32| t.origin[1] + g.spacing * f64::from(gy) + g.shift;
        // each vertex's weight per material (the engine's blend at it) on the materials with a detail diffuse, and the
        // weight of those without one (the far colour's, as the splat's)
        let (mut outside, mut undetailed) = (0u64, 0u64);
        let mut weights: Vec<(Vec<(u8, f64)>, f64)> = Vec::with_capacity(width * height);
        // each vertex's occlusion factor, where stated
        let mut occlusion: Vec<f32> = Vec::with_capacity(if layers.occlusion { width * height } else { 0 });
        for &gy in g.rows {
            for &gx in g.columns {
                let mut at: Vec<(u8, f64)> = Vec::new();
                let mut far = 0.0f64;
                let corners = lookup.corners(x_of(gx), y_of(gy))?;
                if layers.occlusion {
                    occlusion.push(match &corners {
                        Some(c) => lookup.occlusion(c.gx, c.gy)? as f32,
                        None => 1.0,
                    });
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
                                far += wt;
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
                // a corner past every read node has neither: its weight shared out
                let sum: f64 = at.iter().map(|(_, v)| v).sum::<f64>() + far;
                if sum > 0.0 {
                    for (_, v) in &mut at {
                        *v /= sum;
                    }
                    far /= sum;
                }
                weights.push((at, far));
            }
        }
        let at = |r: usize, c: usize| &weights[r * width + c].0;
        let far_at = |r: usize, c: usize| weights[r * width + c].1;
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
        // a rectangle's detail materials by summed weight, and its far colour's summed weight
        let used = |[r0, r1, c0, c1]: [usize; 4]| -> (BTreeMap<u8, f64>, f64) {
            let mut m = BTreeMap::new();
            let mut far = 0.0;
            for r in r0..=r1 {
                for c in c0..=c1 {
                    for &(k, v) in at(r, c) {
                        *m.entry(k).or_insert(0.0) += v;
                    }
                    far += far_at(r, c);
                }
            }
            (m, far)
        };
        // the layers a rectangle needs: its detail materials and the base where any weight goes to it
        let slots = |b: [usize; 4]| {
            let (m, far) = used(b);
            m.len() + usize::from(base_always || far > 0.0)
        };
        let mut leaves = Vec::new();
        let mut stack: Vec<[usize; 4]> = starts.into_values().collect();
        while let Some(b @ [r0, r1, c0, c1]) = stack.pop() {
            if slots(b) <= 4 || (r1 - r0 <= 16 && c1 - c0 <= 16) {
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
        // the base layer's bake of each piece (`layers-base=bake`), over the piece's own extent
        let mut bakes = Vec::new().into_iter();
        let mut baked_total = splat::Baked::default();
        if let LayersBase::Bake { pixel } = layers.base {
            let rects: Vec<[f64; 4]> = leaves
                .iter()
                .map(|&[r0, r1, c0, c1]| {
                    [
                        x_of(g.columns[c0]),
                        y_of(g.rows[r0]),
                        x_of(g.columns[c1]),
                        y_of(g.rows[r1]),
                    ]
                })
                .collect();
            let splat = Splat {
                k: layers.k,
                pixel,
                uv: layers.uv,
                noise_srgb: layers.noise_srgb,
                // the vertex tint multiplies the whole blend, the bake included
                occlusion: false,
            };
            bakes = splat::bake(w, t, &rects, &splat, self.colour)?.into_iter();
        }
        // each terrain material's layer, made once; the far colour's layer (the ground's colour mosaic, as
        // `ground=tile` draws it), made once where a piece takes it
        let mut layer_materials: BTreeMap<u8, Entity> = BTreeMap::new();
        let mut far_layer: Option<(Entity, ColourMosaic)> = None;
        let mut far_pieces = 0u64;
        let mut pieces = Vec::with_capacity(leaves.len());
        let (mut trimmed, mut renormalised) = (0u64, 0u64);
        // piece vertices left with no layer weight: every corner past every read node
        let mut bare = 0u64;
        for b @ [r0, r1, c0, c1] in leaves {
            let (used_by, far_used) = used(b);
            let mut by_weight: Vec<(u8, f64)> = used_by.into_iter().collect();
            by_weight.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            // the far colour takes what no detail layer holds: the weight of materials without a detail, and of those
            // the cap drops
            let far = base_always || far_used > 0.0 || by_weight.len() > 4;
            let detail_slots = if far { 3 } else { 4 };
            if by_weight.len() > detail_slots {
                trimmed += 1;
            }
            let kept: Vec<u8> = by_weight.iter().take(detail_slots).map(|&(m, _)| m).collect();
            if far && far_layer.is_none() && layers.base == LayersBase::Tile {
                let mosaic = ColourMosaic::new(g.tiles, g.rows, g.columns, self.colour)?;
                let texture = w.spawn();
                w.insert(texture, mosaic.texture.clone());
                let e = w.spawn();
                w.insert(
                    e,
                    PbrMaterial {
                        base_color_texture: Some(TextureRef {
                            wrap: [Wrap::Clamp; 2],
                            ..TextureRef::new(texture)
                        }),
                        metallic: 0.0,
                        ..PbrMaterial::default()
                    },
                );
                far_layer = Some((e, mosaic));
            }
            let (ox, oy) = (x_of(g.columns[c0]), y_of(g.rows[r0]));
            // this piece's bake as its base layer: its material, and its texture's extent in source units
            let bake_layer = match layers.base {
                LayersBase::Bake { pixel } => {
                    let (image, baked) = bakes.next().ok_or("a ground piece without its bake")?;
                    baked_total.pixels += baked.pixels;
                    baked_total.tile_fallback += baked.tile_fallback;
                    baked_total.outside += baked.outside;
                    let extent = [f64::from(image.width) * pixel, f64::from(image.height) * pixel];
                    let texture = w.spawn();
                    w.insert(texture, image);
                    let e = w.spawn();
                    w.insert(
                        e,
                        PbrMaterial {
                            base_color_texture: Some(TextureRef {
                                wrap: [Wrap::Clamp; 2],
                                ..TextureRef::new(texture)
                            }),
                            metallic: 0.0,
                            ..PbrMaterial::default()
                        },
                    );
                    Some((e, extent))
                }
                LayersBase::Tile => None,
            };
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
            let mut far_uvs = Vec::with_capacity(if far { vertices.len() } else { 0 });
            for &v in &vertices {
                let (r, c) = (v as usize / width, v as usize % width);
                let mut out = [0.0f64; 4];
                for (slot, m) in kept.iter().enumerate() {
                    out[slot] =
                        layers.detail * at(r, c).iter().find(|(n, _)| n == m).map_or(0.0, |(_, x)| *x);
                }
                let held: f64 = out.iter().sum();
                if far {
                    // the base: the rest, what the kept detail layers don't hold
                    out[kept.len()] = (1.0 - held).max(0.0);
                    far_uvs.push(match (&bake_layer, &far_layer) {
                        (Some((_, extent)), _) => [
                            ((x_of(g.columns[c]) - ox) / extent[0]) as f32,
                            ((y_of(g.rows[r]) - oy) / extent[1]) as f32,
                        ],
                        (None, Some((_, mosaic))) => mosaic.uv(g.columns[c], g.rows[r]),
                        (None, None) => return Err("a base layer without its colour".into()),
                    });
                }
                let sum: f64 = out.iter().sum();
                if sum == 0.0 {
                    // no layer holds any weight here (past every read node): drawn black by a sum of layers
                    bare += 1;
                } else if (sum - 1.0).abs() > 1e-9 {
                    renormalised += 1;
                    for x in &mut out {
                        *x /= sum;
                    }
                }
                piece_weights.push(out.map(|x| x as f32));
                uvs.push([(x_of(g.columns[c]) - ox) as f32, (y_of(g.rows[r]) - oy) as f32]);
            }
            if far {
                let material = match (&bake_layer, &far_layer) {
                    (Some((e, _)), _) | (None, Some((e, _))) => *e,
                    (None, None) => return Err("a base layer without its colour".into()),
                };
                layer_list.push(Some(Layer {
                    material,
                    texcoord: Some(1),
                    transform: None,
                }));
                if bake_layer.is_none() {
                    far_pieces += 1;
                }
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
                    far_uvs: far.then_some(far_uvs),
                }),
                uvs,
                vertices: Some(vertices),
                indices,
                squares,
                steep: None,
            });
        }
        log.count("ground layer pieces", pieces.len() as u64);
        log.count("ground layer materials", layer_materials.len() as u64);
        log.approximate(
            "ground layer weights evaluated at the vertices and interpolated (the engine's blend is per pixel)",
            1,
        );
        if layers.detail < 1.0 {
            // the base is the far colour tile or the bake: a lerp over it moves it toward the details' own colours by
            // that weight, it cannot add what the base lacks
            log.approximate(
                "ground detail layers weighted by layers-detail times their splat weight over the base layer (a lerp \
                 toward the detail's colour at that weight; the high frequencies a bake averaged away are not added \
                 back exactly)",
                pieces.len() as u64,
            );
        }
        if let LayersBase::Bake { .. } = layers.base {
            log.count("ground near-colour pixels baked", baked_total.pixels);
            log.approximate(
                "ground base layer: the near colour baked by the notes' reading of the terrain shader, its choices \
                 stated, without the occlusion (the vertex tint multiplies the blend)",
                pieces.len() as u64,
            );
            if baked_total.tile_fallback > 0 {
                log.omit(
                    "ground pixels a material of which has no detail diffuse: its far colour tile there",
                    baked_total.tile_fallback,
                );
            }
            if baked_total.outside > 0 {
                log.omit(
                    "ground pixels outside every read node: black",
                    baked_total.outside,
                );
            }
        }
        if trimmed > 0 {
            log.approximate(
                "ground pieces using more materials than they can hold at 16 squares a side: their heaviest kept, \
                 the rest's weight on the base layer",
                trimmed,
            );
        }
        if far_pieces > 0 {
            log.approximate(
                "ground pieces with a far colour layer (the weight of materials without a detail diffuse or dropped \
                 by the cap: the node's colour tile there, as the splat's)",
                far_pieces,
            );
        }
        if renormalised > 0 {
            log.approximate(
                "ground vertices whose weights were normalised (a corner past every read node)",
                renormalised,
            );
        }
        if outside > 0 {
            log.omit(
                "ground layer texels past every read node (their weight shared out)",
                outside,
            );
        }
        if bare > 0 {
            log.omit(
                "ground piece vertices with no layer weight: every corner past every read node (drawn black)",
                bare,
            );
        }
        if undetailed > 0 {
            log.count(
                "ground layer texels of materials without a detail diffuse (their weight on the far colour)",
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
    /// With `steep=chart` and steep triangles: what is drawn instead of `vertices`, `uvs` and `indices` (which stay
    /// the collision's): the flat remainder and the charts.
    steep: Option<SteepPart>,
}

/// A primitive's vertices (by number in the whole grid), their coordinates and triangles over them.
struct DrawPart {
    vertices: Vec<u32>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// A piece's steep ground drawn: the triangles that stay in its top-down texture, and the charts' atlas with theirs.
struct SteepPart {
    flat: DrawPart,
    charts: DrawPart,
    atlas: Entity,
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

/// Register Anvil's passes by their declarations.
pub fn register(reg: &mut PassRegistry) {
    for spec in [
        &RESOLVE_ANVIL_SHADERS,
        &LOWER_ANVIL_COLLISION,
        &LOWER_ANVIL_SHAPES,
        &LOWER_ANVIL_LODS,
        &LOWER_ANVIL_WORLD_UV,
        &SELECT_ANVIL_LISTED,
        &SELECT_ANVIL_ENTITIES,
        &LOWER_ANVIL_MATERIAL,
        &BAKE_ANVIL_MATERIAL,
        &BAKE_ANVIL_SURFACE,
        &LOWER_ANVIL_WATER,
        &LOWER_ANVIL_DECALS,
        &LOWER_ANVIL_GRASS,
        &LOWER_ANVIL_SKELETON,
        &LOWER_ANVIL_RIG,
        &LOWER_ANVIL_CLIPS,
        &SELECT_ANVIL_CLIPS,
        &LOWER_ANVIL_SKIN,
        &LOWER_ANVIL_DATA_LAYERS,
        &LOWER_ANVIL_FLOOR,
        &LOWER_ANVIL_TERRAIN,
        &LOWER_ANVIL_FAR_TERRAIN,
        &LOWER_ANVIL_MESH,
          &LOWER_ANVIL_IMPOSTOR,
          &LOWER_ANVIL_LIGHTS,
          &LOWER_ANVIL_SKY,
          &SELECT_ANVIL_PREVIEW_LOD,
          &SELECT_ANVIL_FAR_VIEW,
    ] {
        reg.add(spec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOWER_TEXT: &str = "lower-anvil-material{normal-green=up,alpha=prelude,unset=white,shader-uv=texcoord,                              emissive-exposure=1,underground-view=0.5,fake-river-s=0,decals1-cutout=decal}";

    /// `lower-anvil-material` after a full `bake-anvil-material`: the bake takes `anvil.material` from every Material it
    /// baked, so a world it baked whole holds none and the lowering is skipped, as a run skips a pass that may lack its
    /// inputs and finds none, instead of failing with "requires anvil.material".
    #[test]
    fn a_world_with_no_anvil_material_skips_the_lowering() {
        use primase_core::pass::{PassManager, RunReport, SKIPPED};
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let pipeline = reg.parse(LOWER_TEXT).expect("parses");
        let mut w = primase_core::World::new();
        let e = w.spawn();
        w.insert(e, primase_core::Mesh::new(Vec::new()));
        let mut report = RunReport::default();
        PassManager::new(reg)
            .run(&mut w, &pipeline, &mut report)
            .expect("a full bake leaves nothing to lower, which is not a failure");
        assert_eq!(report.steps.len(), 1);
        assert_eq!(
            report.steps[0].log.counts.get(SKIPPED),
            Some(&1),
            "{:?}",
            report.steps[0].log
        );
    }

    /// The declarations agree: with the component absent the pass is not unfed (it is skipped and provides nothing), and
    /// with it present the pass can run and provides the lowered material.
    #[test]
    fn the_pipeline_check_sees_the_skip() {
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let pipeline = reg.parse(LOWER_TEXT).expect("parses");
        let none = pipeline.unfed(&std::collections::BTreeSet::new());
        assert!(none.passes.is_empty(), "{:?}", none.passes);
        assert!(
            none.held.is_empty(),
            "a skipped pass provides nothing: {:?}",
            none.held
        );
        let some = pipeline.unfed(&std::collections::BTreeSet::from(["anvil.material".to_owned()]));
        assert!(some.passes.is_empty(), "{:?}", some.passes);
        assert!(some.held.contains("pbr.material"), "{:?}", some.held);
    }

    /// The crate registers exactly these passes by their declarations: those with required options are made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_anvil_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let names = [
            "bake-anvil-material",
            "bake-anvil-surface",
            "lower-anvil-clips",
            "lower-anvil-collision",
            "lower-anvil-data-layers",
            "lower-anvil-decals",
            "lower-anvil-far-terrain",
            "lower-anvil-floor",
            "lower-anvil-grass",
            "lower-anvil-impostor",
            "lower-anvil-lights",
            "lower-anvil-lods",
            "lower-anvil-material",
            "lower-anvil-mesh",
            "lower-anvil-rig",
            "lower-anvil-shapes",
            "lower-anvil-skeleton",
            "lower-anvil-skin",
            "lower-anvil-sky",
            "lower-anvil-terrain",
            "lower-anvil-water",
            "lower-anvil-world-uv",
            "resolve-anvil-shaders",
            "select-anvil-clips",
            "select-anvil-entities",
            "select-anvil-far-view",
            "select-anvil-listed",
            "select-anvil-preview-lod",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let free = ["lower-anvil-lods", "lower-anvil-skin", "lower-anvil-world-uv", "select-anvil-preview-lod"];
        for name in names {
            assert_eq!(reg.requires_options(name), !free.contains(&name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert_eq!(
                reg.make(name, &Options::new()).is_err(),
                !free.contains(&name),
                "{name}"
            );
        }
        assert!(
            reg.describe("lower-anvil-floor")
                .unwrap()
                .starts_with("lower-anvil-floor{rect*,step*,offset*,sample*}\n")
        );
    }
}
