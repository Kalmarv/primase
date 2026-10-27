//! The World data the Shaun White Skateboarding reader attaches (`shaunwhite.*`).

use anvil_formats::shaunwhite::guidance::Class as GuidanceClass;
use anvil_formats::shaunwhite::light::{LightSource as StoredLightSource, SkySettings as StoredSkySettings};
use anvil_formats::shaunwhite::material::LayerState;
use anvil_formats::shaunwhite::shader_material::MaterialShader as StoredMaterialShader;
use primase_core::{Component, Entity, Geometry};
use std::collections::BTreeMap;

/// A record of `anvil_formats` held as a component: the record as stored, its component name, checks and size given
/// by its `impl Component for Stored<..>` here. It derefs to the record and has the record's JSON (none).
#[derive(Clone, Debug, PartialEq)]
pub struct Stored<T>(pub T);

impl<T> std::ops::Deref for Stored<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Stored<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// A Material's shader key fields ([`StoredMaterialShader`]), `shaunwhite.material_shader`.
pub type MaterialShader = Stored<StoredMaterialShader>;
/// A light as stored ([`StoredLightSource`]), `shaunwhite.light`.
pub type LightSource = Stored<StoredLightSource>;
/// The level root's SkyLight settings as stored ([`StoredSkySettings`]), `shaunwhite.sky_settings`.
pub type SkySettings = Stored<StoredSkySettings>;

/// The chosen SDB2 shader database, supplied by the reader. Raw sources, stage bytecodes and pass metadata
/// are retained; selecting a material's permutation and resolving its bindings is a separate pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderLibrary(pub std::sync::Arc<anvil_formats::shaunwhite::shaders::Database>);
impl primase_core::Resource for ShaderLibrary {
    const NAME: &'static str = "shaunwhite.shader_library";
}

impl Component for Stored<StoredMaterialShader> {
    const NAME: &'static str = "shaunwhite.material_shader";
    const GEOMETRY: Geometry = Geometry::None;
}

/// The exact source records retained by shader resolution. Record numbers are variants of
/// `shaunwhite.shader.record`; their keys and both stage cache identities remain in the chosen library.
/// This is a compatible variant set, not a claim that the game draws all passes at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderPasses(pub Vec<u32>);
impl Component for ShaderPasses {
    const NAME: &'static str = "shaunwhite.shader_passes";
    const GEOMETRY: Geometry = Geometry::None;
}

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
    /// Source TexCoordRotation in degrees, about the UV origin before scale and offset.
    pub rotation: f32,
    pub texcoord: u8,
    /// Material map address codes (0 wrap, 1 clamp, 2 border), when authored here rather than on the texture.
    pub address: Option<[u32; 2]>,
}

/// The fixed fields of a stored Material that say how it is drawn ([`anvil_formats::shaunwhite::material`] gives their offsets and
/// what is known of them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialState {
    pub blend: u8,
    pub blended: bool,
    pub alpha_test: bool,
    pub alpha_ref: u8,
    /// The stored water shader switch at body offset 91.
    pub water: bool,
    /// TwoSided (body offset 36): the draw culls no face.
    pub two_sided: bool,
    /// InvertCulling (body offset 37): the culled winding is the other one. Moot where `two_sided` is set.
    pub invert_culling: bool,
    /// Layer1_ModulationColor; its tint is interpolated by the shader's red vertex/mask weight.
    pub color: [f32; 4],
    /// A self-lit factor (0: none): the float at 87.
    pub illumination: f32,
    /// The float at 110, 0 to 1.
    pub specular_level: f32,
    /// The float at 114: an exponent, 1 and up.
    pub specular_power: f32,
    /// The normal slot's `HeightScale` (the float at 265): the shader's `g_NormalMap_HeightScales.x`, which scales the
    /// normal map's X and Y; 1 where the Material has no normal map.
    pub normal_scale: f32,
    pub layering: LayerState,
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
    const VERSION: u32 = 4;
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures());
    }
}

/// Generic shader material inputs retained by `resolve-shaunwhite-shaders` across material lowering.
///
/// `material` keeps the authored controls (including base-UV composition inputs) needed by a later
/// graph derivation. `bindings` keeps every exact compatible Colour record with the established material
/// words filled; pass/frame/varying inputs stay unresolved. No record or influence state is selected.
/// `unresolved` names records whose material allocation could not be filled; their original binding is
/// retained. The original `core.shader_binding` is unchanged. `lower-shaunwhite-material` consumes only
/// `shaunwhite.material`, leaving this component and the shader provenance intact.
/// Empty stages do not imply successful resolution: read the named unavailable reason from `MaterialPrograms`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderInputs {
    pub material: MaterialRef,
    pub bindings: primase_core::shader::ShaderBinding,
    pub unresolved: BTreeMap<u32, String>,
}
impl Component for ShaderInputs {
    const NAME: &'static str = "shaunwhite.shader_inputs";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        self.material.references(out);
        self.bindings.references(out);
    }
    fn verify(&self, entity: Entity, world: &primase_core::World) -> Result<(), String> {
        self.bindings.verify(entity, world)
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

/// Marks a Mesh prototype built from a RealTreeMesh ([`crate::read::tree`]): a tree the game draws from its own
/// trunk, branch and leaf data, as opposed to a stored Mesh. An Entity that holds one and a stored Mesh Visual draws
/// the stored Mesh when the world is not restored (`select-shaunwhite-content`'s `influence`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealTree;

impl Component for RealTree {
    const NAME: &'static str = "shaunwhite.real_tree";
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

/// What the source states of a grind guidance chain, on the entity of its `core.curve` (read by
/// [`anvil_formats::shaunwhite::guidance`]): the Entity that holds the guidance system and the system's class, the edges'
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

impl Component for Stored<StoredLightSource> {
    const NAME: &'static str = "shaunwhite.light";
    const GEOMETRY: Geometry = Geometry::None;
}

impl Component for Stored<StoredSkySettings> {
    const NAME: &'static str = "shaunwhite.sky_settings";
    const GEOMETRY: Geometry = Geometry::None;
}
