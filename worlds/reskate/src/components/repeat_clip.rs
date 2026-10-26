//! Prepared render primitives for Studio's repeat cut, at the final assembled f32 positions.
use super::OverlayGrouping;
use crate::glb::GlbPrim;
use primase_core::Resource;
use std::collections::BTreeMap;

/// A mesh's source identity and the changed primitives; `None` excludes a primitive completely.
#[derive(Clone, Debug, PartialEq)]
pub struct RepeatMesh {
    pub source: [u8; 32],
    pub replacements: BTreeMap<usize, Option<GlbPrim>>,
}
/// The explicit preparation selected by `prepare-reskate-repeat-clipping`. The writer validates identities and
/// encodes these primitives; it makes no clipping/area decision. Each part carries its own mesh records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkateRepeatClipping {
    pub grouping: Option<OverlayGrouping>,
    pub meshes: BTreeMap<String, RepeatMesh>,
    pub discarded: BTreeMap<String, u64>,
    pub source_triangles: u64,
    pub opacity_clamped: u64,
    pub normals_cancelled: u64,
    pub split: u64,
}
impl Resource for SkateRepeatClipping {
    const NAME: &'static str = "skate.repeat_clipping";
}
