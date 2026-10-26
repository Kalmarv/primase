//! The World data the Shaun White Skateboarding reader attaches (`shaunwhite.*`).

use std::collections::BTreeMap;

use primase_core::{Component, Entity, Geometry};

/// Where a placement comes from: its cell entry, its Entity object's id, and its Visual's ResourceRef kind byte (0xfe
/// or 0xff; its meaning is not read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedBy {
    pub cell: String,
    pub entity: u32,
    pub kind: u8,
}

impl Component for PlacedBy {
    const NAME: &'static str = "shaunwhite.placed_by";
    const GEOMETRY: Geometry = Geometry::None;
}

/// One texture slot of a stored Material: the texture (a `core.texture` entity) and the UV transform the slot states:
/// the scale and offset its u and v are multiplied and moved by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotRef {
    pub texture: Entity,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// The fixed fields of a stored Material that say how it is drawn ([`crate::read::material`] gives their offsets and
/// what is known of them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialState {
    pub blend: u8,
    pub blended: bool,
    pub alpha_test: bool,
    pub alpha_ref: u8,
    /// The stored water shader switch at body offset 91.
    pub water: bool,
    /// RGBA, multiplying the diffuse layer.
    pub color: [f32; 4],
    /// A self-lit factor (0: none): the float at 87.
    pub illumination: f32,
    /// The float at 110, 0 to 1.
    pub specular_level: f32,
    /// The float at 114: an exponent, 1 and up.
    pub specular_power: f32,
    /// RGBA of the specular, 0 to 1.
    pub specular_color: [f32; 4],
}

/// A Material as the source stores it: its id, its object's name (empty when no forge read holds it), its fields
/// (`None` when its body is not one the reader reads), the textures of the slots read (`diffuse`, `normal`,
/// `specular`, `mask`, `diffuse_b`, `specular_b`, `diffuse_c`: `core.texture` entities) and the names of the slots
/// that hold a texture spec the reader does not read (`detail`, `diffuse2`, `generic`). Lowered by
/// `lower-shaunwhite-material`.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialRef {
    pub id: u32,
    pub name: String,
    pub state: Option<MaterialState>,
    pub slots: BTreeMap<String, SlotRef>,
    pub unread: Vec<String>,
}

impl MaterialRef {
    /// The textures of the slots read.
    pub fn textures(&self) -> impl Iterator<Item = Entity> + '_ {
        self.slots.values().map(|s| s.texture)
    }
}

impl Component for MaterialRef {
    const NAME: &'static str = "shaunwhite.material";
    const VERSION: u32 = 2;
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures());
    }
}

/// How the source samples a texture: its U, V, W address modes (wrap 0, mirror 1, clamp 2, border 3, mirror-once 4) and
/// its magnification, minification and mip filters (point 0, linear 1, none 2, anisotropic 3, comparison-linear 4),
/// on the `core.texture` entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampler {
    pub address: [u32; 3],
    pub filter: [u32; 3],
}

impl Component for Sampler {
    const NAME: &'static str = "shaunwhite.sampler";
    const GEOMETRY: Geometry = Geometry::None;
}

/// A Mesh prototype's vertex stride and flags word as its CompiledMesh header stores them. The reader reads the
/// vertex layout from them ([`crate::read::mesh`]); the flags stay as data so a pass can choose by them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshFormat {
    pub stride: u32,
    pub flags: u32,
}

impl MeshFormat {
    /// The flag that adds skinning (bone indices and weights) to the vertex. Of level 01's 101 distinct meshes with it,
    /// 93 are named `CH_*` (characters and their accessories), 5 `GFX_*` and 3 others; one `CH_*` mesh lacks it.
    pub const SKINNED: u32 = 1;

    /// Whether the vertices hold bone indices and weights.
    pub fn skinned(&self) -> bool {
        self.flags & Self::SKINNED != 0
    }
}

impl Component for MeshFormat {
    const NAME: &'static str = "shaunwhite.mesh_format";
    const GEOMETRY: Geometry = Geometry::None;
}

/// Authored collision geometry in core's frame, before source surface ids are lowered. A list is represented by
/// one prototype per leaf, with any leaf-local matrix retained in the placement's transform.
#[derive(Clone, Debug, PartialEq)]
pub enum CollisionSource {
    Mesh {
        positions: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
        surfaces: Vec<u32>,
    },
    Shape {
        shape: primase_core::Shape,
        surface: u32,
    },
}
impl Component for CollisionSource {
    const NAME: &'static str = "shaunwhite.collision";
    const GEOMETRY: Geometry = Geometry::None;
}

/// Source collision-material object ids and directory names. Missing objects keep an explicit hexadecimal id;
/// no destination material or game-specific grind/walkability behaviour is inferred.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollisionSurfaces(pub BTreeMap<u32, String>);
impl primase_core::Resource for CollisionSurfaces {
    const NAME: &'static str = "shaunwhite.collision_surfaces";
}

/// The source entity and shape of a collision placement, independent of its visual mesh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionPlacedBy {
    pub cell: String,
    pub entity: u32,
    pub shape: u32,
}
impl Component for CollisionPlacedBy {
    const NAME: &'static str = "shaunwhite.collision_placed_by";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
}

/// The four guidance classes the game stores (read by [`crate::read::guidance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceClass {
    /// SWSkateGuidanceSystem.
    Plain,
    /// SWSkateFreeFormGuidanceSystem: the shaping mechanic's placed grind rails.
    FreeForm,
    /// SWSkateScaleGuidanceSystem.
    Scale,
    /// SWSkateMorphGuidanceSystem.
    Morph,
}

impl GuidanceClass {
    pub const ALL: [GuidanceClass; 4] = [
        GuidanceClass::Plain,
        GuidanceClass::FreeForm,
        GuidanceClass::Scale,
        GuidanceClass::Morph,
    ];

    /// The game's class name.
    pub const fn name(self) -> &'static str {
        match self {
            GuidanceClass::Plain => "SWSkateGuidanceSystem",
            GuidanceClass::FreeForm => "SWSkateFreeFormGuidanceSystem",
            GuidanceClass::Scale => "SWSkateScaleGuidanceSystem",
            GuidanceClass::Morph => "SWSkateMorphGuidanceSystem",
        }
    }
}

/// What the source states of a grind guidance chain, on the entity of its `core.curve` (read by
/// [`crate::read::guidance`]): the Entity that holds the guidance system and the system's class, the edges'
/// GuidanceObjectSubType, and per edge (between curve points `i` and `i + 1`) its two face normals, world space in
/// core's frame like the curve's points, and its stored length (metres). For a morph system's chain, `morph_points`
/// has one list per state past the first (the shape at rest is the curve): the chain's points, by the same point
/// indices, in that state, world space like the curve's. Lowered by `lower-shaunwhite-grinds`.
#[derive(Clone, Debug, PartialEq)]
pub struct Guidance {
    pub cell: String,
    pub entity: u32,
    pub class: GuidanceClass,
    pub sub_type: u32,
    pub normals: Vec<[[f32; 3]; 2]>,
    pub lengths: Vec<f32>,
    pub morph_points: Vec<Vec<[f32; 3]>>,
}

impl Component for Guidance {
    const NAME: &'static str = "shaunwhite.guidance";
    const GEOMETRY: Geometry = Geometry::None;
    /// World space, as `core.curve`: not in any entity's local space.
    fn apply_linear(&self, _: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
}

/// A light's authored shape. Spot angles are stored radians; source range falloff has a near and a far distance.
#[derive(Clone, Debug, PartialEq)]
pub enum LightShape {
    Point {
        near: f32,
        far: f32,
    },
    Spot {
        inner: f32,
        outer: f32,
        cookie: u32,
        unknown: u32,
        near: f32,
        far: f32,
    },
    Directional,
}

/// The source flare, retained even though the punctual-light lowering cannot draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct LightFlare {
    pub id: u32,
    pub enabled: bool,
    pub texture: u32,
    /// Width, height, occlusion width, occlusion height, intensity.
    pub values: [f32; 5],
    pub scale: bool,
    pub fade: bool,
    pub offset: f32,
    pub blend: u32,
}

/// Source light data before its uncalibrated intensity/colour and spot axis are interpreted by a chosen pass.
#[derive(Clone, Debug, PartialEq)]
pub struct LightSource {
    pub id: u32,
    pub class: u32,
    pub category: u32,
    pub generated: bool,
    pub colour_id: u32,
    pub colour: [f32; 4],
    /// Dynamic, cast shadows, affect characters, affect environment, dirty.
    pub flags: [bool; 5],
    pub intensity: f32,
    pub flare: LightFlare,
    pub shadow_resolution: u32,
    pub shape: LightShape,
}
impl Component for LightSource {
    const NAME: &'static str = "shaunwhite.light";
    const GEOMETRY: Geometry = Geometry::None;
}

/// The level root's SkyLight settings, retained alongside its inline sun. Vectors are in the source Z-up frame.
/// Native property hashes establish the sun and ambient names; the other fields remain unlabelled. Ambient alpha,
/// the second colour's meaning and global fog units are not established and are not lowered to core ambient/fog.
#[derive(Clone, Debug, PartialEq)]
pub struct SkySettings {
    pub id: u32,
    pub direction: [f32; 3],
    pub sun_colour_id: u32,
    pub sun_colour: u32,
    pub sun_intensity: f32,
    pub unknown_scalars: [f32; 2],
    pub use_character_sun: bool,
    pub character_colour_id: u32,
    pub character_colour: u32,
    pub character_intensity: f32,
    pub unknown_vector: [f32; 3],
    pub ambient_colour: [f32; 4],
    /// Native property hash 0xea0e031c.
    pub unknown_colour: [f32; 4],
}
impl Component for SkySettings {
    const NAME: &'static str = "shaunwhite.sky_settings";
    const GEOMETRY: Geometry = Geometry::None;
}
