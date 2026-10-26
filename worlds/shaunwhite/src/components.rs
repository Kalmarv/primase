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

/// A Material as the source stores it: its id, its object's name (empty when no forge read holds it), and the
/// textures of the slots read (`diffuse`, `normal`, `specular`: `core.texture` entities). Lowered by
/// `lower-shaunwhite-material`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialRef {
    pub id: u32,
    pub name: String,
    pub textures: BTreeMap<String, Entity>,
}

impl Component for MaterialRef {
    const NAME: &'static str = "shaunwhite.material";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures.values().copied());
    }
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
