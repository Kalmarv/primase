//! Authored placement identities and MAIN.DB rows. Visibility is a separate selection pass.
use super::Stored;
use primase_core::{Component, Entity, Geometry, Linear, Resource, world::Linearity};
use serde::Serialize;

/// Every word of one SCP placement, alongside its frame-converted core transform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourcePlacement {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub index: u32,
    pub offset: usize,
    /// `+0x0c`: case-sensitive sdbm of its name, including task IDENTIFIERS names.
    pub name: u32,
    /// All twelve big-endian words; fields other than matrix pointer, prototype and name remain opaque.
    pub words: [u32; 12],
    /// Kept reachable even if a selection removes the core instance.
    pub prototype: Entity,
}

/// One explicit level row and all rows of its session, goal and task tables, as source data. No active session or
/// task state is chosen.
pub type SessionGraph = Stored<rme_formats::session::SessionGraph>;

impl Component for SourcePlacement {
    const NAME: &'static str = "shred.placement";
    const GEOMETRY: Geometry = Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.prototype);
    }
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

impl Resource for SessionGraph {
    const NAME: &'static str = "shred.session_graph";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(&self.0).map(Some)
    }
}
