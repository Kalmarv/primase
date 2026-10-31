//! Named flag inputs and the results of common-event initialization selection.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// An independently observed snapshot, named by the caller with its source.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlagSet {
    pub name: String,
    pub source: String,
    pub flags: BTreeMap<u32, bool>,
}

/// Actions of the selected initialization path, not actions performed in a game.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub groups: BTreeMap<i32, bool>,
    pub envmap: Option<u32>,
    pub animations: BTreeMap<i32, i32>,
    pub gparam_sub_ids: BTreeMap<String, i32>,
    pub written_flags: BTreeMap<u32, bool>,
    pub instructions: Vec<usize>,
}

/// Source and exact installed script of a selection. Flags are not reconstructed
/// from its output; their independently obtained provenance belongs to the input.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub input: FlagSet,
    pub common_script_sha256: String,
    pub selection: Selection,
    pub sky: Option<String>,
    pub assignments: BTreeMap<String, u32>,
}
impl Report {
    pub fn new(input: FlagSet, script: &[u8], selection: Selection) -> Self {
        Self {
            input,
            common_script_sha256: Sha256::digest(script)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            selection,
            sky: None,
            assignments: BTreeMap::new(),
        }
    }
}
