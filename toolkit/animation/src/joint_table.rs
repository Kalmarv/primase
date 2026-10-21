//! A joint table: a person's (or a tool's) statement of which joint of one skeleton is which joint of another, by name,
//! kept as a reviewable file and resolved against the two skeletons into a `core.joint_map`'s pairs.
//!
//! The file is JSON:
//!
//! ```json
//! {
//!   "version": 1,
//!   "from": "Quaternius UAL (Rigify DEF bones)",
//!   "to": "Sekiro c1010",
//!   "pairs": [["DEF-hips", "Root"], ["DEF-spine.001", "Spine"]],
//!   "notes": {"DEF-hips": "the hips carry both the pelvis and the spine, as Sekiro's Root does"}
//! }
//! ```
//!
//! `from` and `to` describe the skeletons for people (they are not checked); `pairs` are (source joint name, target joint
//! name); `notes` (optional) say why a pair was chosen, by source name. Names are matched exactly; a skeleton with two
//! joints of one name cannot be resolved by name (refused, naming it). Every listed name must be a joint of its
//! skeleton and every joint appears at most once on its side; nothing is inferred or completed. What a pass does with
//! the joints the table leaves out is that pass's stated choice.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::{JointId, Skeleton};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The table as the file states it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointTable {
    pub version: u32,
    pub from: String,
    pub to: String,
    pub pairs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes: BTreeMap<String, String>,
}

/// The only version this reads.
pub const VERSION: u32 = 1;

impl JointTable {
    /// The table in `text` (see the module documentation): its version 1, each name once a side, each note on a
    /// listed source name.
    pub fn parse(text: &str) -> Result<JointTable> {
        let t: JointTable = serde_json::from_str(text).map_err(|e| Error(format!("joint table: {e}")))?;
        if t.version != VERSION {
            return Err(Error(format!(
                "joint table version {}: only {VERSION} is read",
                t.version
            )));
        }
        let (mut a, mut b) = (BTreeSet::new(), BTreeSet::new());
        for (s, d) in &t.pairs {
            if !a.insert(s.as_str()) {
                return Err(Error(format!("joint table: source joint {s} listed twice")));
            }
            if !b.insert(d.as_str()) {
                return Err(Error(format!("joint table: target joint {d} listed twice")));
            }
        }
        if let Some(k) = t.notes.keys().find(|k| !a.contains(k.as_str())) {
            return Err(Error(format!("joint table: a note on {k}, which no pair lists")));
        }
        Ok(t)
    }

    /// The table as JSON text, pairs in their order.
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The pairs as joint ids of `from` and `to` (a `core.joint_map`'s pairs), in the table's order.
    pub fn resolve(&self, from: &Skeleton, to: &Skeleton) -> Result<Vec<(JointId, JointId)>> {
        let (a, b) = (by_name(from, "source")?, by_name(to, "target")?);
        self.pairs
            .iter()
            .map(|(s, d)| {
                Ok((
                    *a.get(s.as_str())
                        .ok_or_else(|| Error(format!("joint table: no source joint {s}")))?,
                    *b.get(d.as_str())
                        .ok_or_else(|| Error(format!("joint table: no target joint {d}")))?,
                ))
            })
            .collect()
    }
}

/// A skeleton's joints by name, refused when two share one.
fn by_name<'a>(s: &'a Skeleton, side: &str) -> Result<BTreeMap<&'a str, JointId>> {
    let mut out = BTreeMap::new();
    for j in &s.joints {
        if out.insert(j.name.as_str(), j.id).is_some() {
            return Err(Error(format!(
                "joint table: the {side} skeleton has two joints named {}: not resolvable by name",
                j.name
            )));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Joint, Rest, Transform};

    fn skeleton(names: &[&str]) -> Skeleton {
        Skeleton {
            joints: names
                .iter()
                .enumerate()
                .map(|(i, n)| Joint {
                    id: JointId(10 + i as u32),
                    name: (*n).into(),
                    parent: i.checked_sub(1).map(|p| JointId(10 + p as u32)),
                    rest: Rest::Affine(Transform::IDENTITY),
                })
                .collect(),
        }
    }

    #[test]
    fn a_table_resolves_by_name_and_round_trips() {
        let text = r#"{"version": 1, "from": "a", "to": "b", "pairs": [["hips", "Root"], ["head", "Head"]],
            "notes": {"hips": "carries the spine"}}"#;
        let t = JointTable::parse(text).unwrap();
        let pairs = t
            .resolve(
                &skeleton(&["hips", "spine", "head"]),
                &skeleton(&["Root", "Head"]),
            )
            .unwrap();
        assert_eq!(pairs, [(JointId(10), JointId(10)), (JointId(12), JointId(11))]);
        assert_eq!(JointTable::parse(&t.to_text()).unwrap(), t);
    }

    #[test]
    fn a_table_refuses_what_it_cannot_state() {
        let base = |pairs: &str, notes: &str| {
            format!(r#"{{"version": 1, "from": "a", "to": "b", "pairs": {pairs}{notes}}}"#)
        };
        for (text, why) in [
            (
                base(r#"[["a", "X"], ["a", "Y"]]"#, ""),
                "source joint a listed twice",
            ),
            (
                base(r#"[["a", "X"], ["b", "X"]]"#, ""),
                "target joint X listed twice",
            ),
            (base(r#"[["a", "X"]]"#, r#", "notes": {"b": "?"}"#), "a note on b"),
            (base(r#"[["a", "X"]]"#, r#", "extra": 1"#), "unknown field"),
            (
                r#"{"version": 2, "from": "", "to": "", "pairs": []}"#.to_owned(),
                "version 2",
            ),
        ] {
            let err = JointTable::parse(&text).unwrap_err().0;
            assert!(err.contains(why), "{text}: {err}");
        }
        let t = JointTable::parse(&base(r#"[["a", "X"]]"#, "")).unwrap();
        let err = t.resolve(&skeleton(&["b"]), &skeleton(&["X"])).unwrap_err().0;
        assert!(err.contains("no source joint a"), "{err}");
        let err = t
            .resolve(&skeleton(&["a", "a"]), &skeleton(&["X"]))
            .unwrap_err()
            .0;
        assert!(err.contains("two joints named a"), "{err}");
    }
}
