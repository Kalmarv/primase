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
