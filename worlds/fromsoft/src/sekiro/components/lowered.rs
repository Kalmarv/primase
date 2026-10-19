//! The components the Sekiro passes make for later passes: the UV tiling the geometry pass applied.
use primase_core::Component;

/// A material's UV tiling applied to a mesh's UV0 by `lower-fromsoft-geometry{uv-tiling=apply}`
/// (`fromsoft.uv_tiling`, on the material entity): the scale every primitive drawing it was multiplied by, so a
/// material pass binds its textures relative to it.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct UvTiling(pub [f32; 2]);
impl Component for UvTiling {
    const NAME: &'static str = "fromsoft.uv_tiling";
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
