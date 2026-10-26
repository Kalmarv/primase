//! Authored placement identities and MAIN.DB rows. Visibility is a separate selection pass.
use primase_core::{Component, Entity, Geometry, Linear, Resource, world::Linearity};
use serde::Serialize;
use std::collections::BTreeMap;

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

/// One exact token of an IDENTIFIERS cell and its case-sensitive sdbm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionIdentifier {
    pub name: String,
    pub hash: u32,
}

/// All cells of a row, including unknown columns and unparsed relationship expressions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionRow {
    pub rowid: i64,
    pub id: u32,
    /// MAIN.DB stores these columns as TEXT. SQL NULL remains distinct from an empty string.
    pub fields: BTreeMap<String, Option<String>>,
    /// Ampersand-separated names, excluding empty/none/NULL/-1 sentinel cells.
    pub identifiers: Vec<SessionIdentifier>,
}

/// One explicit level row and all rows of its session, goal and task tables, as source data.
/// No active session or task state is chosen; TASKS_OPT brackets/ranges remain authored text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionGraph {
    pub level_id: u32,
    pub level: SessionRow,
    pub session_table: String,
    pub goal_table: String,
    pub task_table: String,
    /// Source rowid order, including duplicate logical IDs (observed in Papa_tasks).
    pub sessions: Vec<SessionRow>,
    pub goals: Vec<SessionRow>,
    pub tasks: Vec<SessionRow>,
}

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
        primase_core::json::to_value(self).map(Some)
    }
}
