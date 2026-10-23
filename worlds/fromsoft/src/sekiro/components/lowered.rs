//! The components the Sekiro passes make for later passes: the UV tiling the geometry pass applied, and the FLVER
//! mesh each primitive of a character came from.
use primase_core::Component;

/// A material's UV tiling applied to a mesh's UV0 by `lower-fromsoft-geometry{uv-tiling=apply}`
/// (`fromsoft.uv_tiling`, on the material entity): the scale every primitive drawing it was multiplied by, so a
/// material pass binds its textures relative to it.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct UvTiling(pub [f32; 2]);
impl Component for UvTiling {
    const NAME: &'static str = "fromsoft.uv_tiling";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Per primitive of a character's lowered mesh, the index of the FLVER mesh it was lowered from
/// (`fromsoft.source_meshes`, on the prototype and on each coarser form): what ties a primitive to the display mask
/// marks of its mesh ([`MeshMasks`](super::source::MeshMasks)). Follows its primitive through every edit.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct SourceMeshes(pub Vec<u32>);
impl Component for SourceMeshes {
    const NAME: &'static str = "fromsoft.source_meshes";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(SourceMeshes))
    }
}
