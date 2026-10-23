//! A joint table: a person's (or a tool's) statement of which joint of one skeleton is which joint of another, by name,
//! kept as a reviewable file and resolved against the two skeletons into a `core.joint_map`'s pairs.
//!
//! The file is JSON:
//!
//! ```json
//! {
//!   "version": 2,
//!   "from": "Quaternius UAL (Rigify DEF bones)",
//!   "to": "Sekiro c1010",
//!   "pairs": [["DEF-hips", "Root"], ["DEF-spine.001", "Spine"]],
//!   "notes": {"DEF-hips": "the hips carry both the pelvis and the spine, as Sekiro's Root does"},
//!   "placements": {"Spine": {"t": [0.0, 0.02, 0.0], "r": [0.0, 0.0, 0.0, 1.0]}}
//! }
//! ```
//!
//! `from` and `to` describe the skeletons for people (they are not checked); `pairs` are (source joint name, target joint
//! name); `notes` (optional) say why a pair was chosen, by source name. Names are matched exactly; a listed name that
//! two joints of its skeleton hold cannot be resolved by name (refused, naming it); names the table does not list may
//! repeat. Every listed name must be a joint of its
//! skeleton and every joint appears at most once on its side; nothing is inferred or completed. What a pass does with
//! the joints the table leaves out is that pass's stated choice.
//!
//! Version 2 adds `placements` (optional; version 1 files, which have none, are still read): where a person stood each
//! target joint inside the source mesh, as an offset from where the automatic fit puts it
//! ([`primase_core::JointOffset`], see [`crate::placement`]), keyed by target joint name, which a pair lists. `t` is
//! metres along the source's model axes (zero when absent), `r` a unit quaternion `[x, y, z, w]` (within 1e-3; the
//! identity when absent). A file with placements is version 2; [`JointTable::to_json`] writes what [`JointTable::parse`]
//! reads back.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::{JointId, JointOffset, Skeleton};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The table as the file states it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointTable {
    pub version: u32,
    pub from: String,
    pub to: String,
    pub pairs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes: BTreeMap<String, String>,
    /// Offsets by target joint name (version 2).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placements: BTreeMap<String, Placed>,
}

/// A target joint's offset as the file states it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placed {
    #[serde(default)]
    pub t: [f32; 3],
    #[serde(default = "identity_rotation")]
    pub r: [f32; 4],
}

fn identity_rotation() -> [f32; 4] {
    JointOffset::IDENTITY.rotation
}

impl Placed {
    /// The offset as a world holds it.
    pub fn offset(&self) -> JointOffset {
        JointOffset {
            translation: self.t,
            rotation: self.r,
        }
    }
}

impl From<JointOffset> for Placed {
    fn from(o: JointOffset) -> Placed {
        Placed {
            t: o.translation,
            r: o.rotation,
        }
    }
}

/// The newest version this reads and the one it writes.
pub const VERSION: u32 = 2;

/// The first version this reads.
pub const FIRST_VERSION: u32 = 1;

impl JointTable {
    /// The table in `text` (see the module documentation): version 1 or 2, each name once a side, each note on a
    /// listed source name, each placement on a listed target name (version 2 only) with finite entries and a unit
    /// rotation.
    pub fn parse(text: &str) -> Result<JointTable> {
        let t: JointTable = serde_json::from_str(text).map_err(|e| Error(format!("joint table: {e}")))?;
        t.check()?;
        Ok(t)
    }

    /// The table as the file states it, pretty-printed, as [`JointTable::parse`] reads it back; refused when
    /// [`JointTable::parse`] would refuse it.
    pub fn to_json(&self) -> Result<String> {
        self.check()?;
        serde_json::to_string_pretty(self).map_err(|e| Error(format!("joint table: {e}")))
    }

    fn check(&self) -> Result<()> {
        let t = self;
        if !(FIRST_VERSION..=VERSION).contains(&t.version) {
            return Err(Error(format!(
                "joint table version {}: only {FIRST_VERSION} to {VERSION} are read",
                t.version
            )));
        }
        if t.version < 2 && !t.placements.is_empty() {
            return Err(Error(format!(
                "joint table version {}: placements are version 2",
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
        for (k, p) in &t.placements {
            if !b.contains(k.as_str()) {
                return Err(Error(format!(
                    "joint table: a placement of {k}, which no pair lists as a target"
                )));
            }
            p.offset()
                .check()
                .map_err(|why| Error(format!("joint table: placement of {k}: {why}")))?;
        }
        Ok(())
    }

    /// The placements as target joint ids of `to` and their offsets (a `core.joint_placement`'s), in name order.
    pub fn offsets(&self, to: &Skeleton) -> Result<Vec<(JointId, JointOffset)>> {
        // only the placed names must be unique on the target, as for the pairs
        let placed: BTreeSet<&str> = self.placements.keys().map(String::as_str).collect();
        let b = by_name(to, "target", &placed)?;
        self.placements
            .iter()
            .map(|(k, p)| {
                Ok((
                    *b.get(k.as_str())
                        .ok_or_else(|| Error(format!("joint table: no target joint {k}")))?,
                    p.offset(),
                ))
            })
            .collect()
    }

    /// The pairs as joint ids of `from` and `to` (a `core.joint_map`'s pairs), in the table's order.
    pub fn resolve(&self, from: &Skeleton, to: &Skeleton) -> Result<Vec<(JointId, JointId)>> {
        let listed =
            |side: fn(&(String, String)) -> &str| self.pairs.iter().map(side).collect::<BTreeSet<&str>>();
        let (a, b) = (
            by_name(from, "source", &listed(|p| &p.0))?,
            by_name(to, "target", &listed(|p| &p.1))?,
        );
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

/// A skeleton's joints by name among the `listed` names, refused when two joints share a listed name.
fn by_name<'a>(s: &'a Skeleton, side: &str, listed: &BTreeSet<&str>) -> Result<BTreeMap<&'a str, JointId>> {
    let mut out = BTreeMap::new();
    for j in s.joints.iter().filter(|j| listed.contains(j.name.as_str())) {
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
    fn a_table_resolves_by_name() {
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
                base(r#"[["a", "X"]]"#, r#", "placements": {"Y": {"t": [0, 0, 0]}}"#),
                "a placement of Y, which no pair lists as a target",
            ),
            (
                base(r#"[["a", "X"]]"#, r#", "placements": {"X": {"r": [0, 0, 0, 2]}}"#),
                "not a unit quaternion",
            ),
            (
                base(r#"[["a", "X"]]"#, r#", "placements": {"X": {"q": [0, 0, 0, 1]}}"#),
                "unknown field",
            ),
            (
                base(r#"[["a", "X"], ["b", "X"]]"#, ""),
                "target joint X listed twice",
            ),
            (base(r#"[["a", "X"]]"#, r#", "notes": {"b": "?"}"#), "a note on b"),
            (base(r#"[["a", "X"]]"#, r#", "extra": 1"#), "unknown field"),
            (
                r#"{"version": 3, "from": "", "to": "", "pairs": []}"#.to_owned(),
                "version 3",
            ),
            (
                r#"{"version": 1, "from": "", "to": "", "pairs": [["a", "X"]], "placements": {"X": {}}}"#
                    .to_owned(),
                "placements are version 2",
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

    /// Only a name the table lists must be unique: two unlisted joints named alike do not stop the listed pairs from
    /// resolving, and a listed name held twice on the target side is refused as on the source side.
    #[test]
    fn duplicate_names_matter_only_when_listed() {
        let t = JointTable::parse(r#"{"version": 1, "from": "", "to": "", "pairs": [["a", "X"]]}"#).unwrap();
        let pairs = t
            .resolve(
                &skeleton(&["a", "twin", "twin"]),
                &skeleton(&["spare", "X", "spare"]),
            )
            .unwrap();
        assert_eq!(pairs, [(JointId(10), JointId(11))]);
        let err = t
            .resolve(&skeleton(&["a"]), &skeleton(&["X", "X"]))
            .unwrap_err()
            .0;
        assert!(err.contains("target skeleton has two joints named X"), "{err}");
    }

    /// A version 2 file's placements come out as offsets of the target's joints (an absent `t` or `r` is the identity's),
    /// version 1 files are still read, and what is written is read back as it was.
    #[test]
    fn placements_resolve_and_the_file_round_trips() {
        let text = r#"{"version": 2, "from": "a", "to": "b", "pairs": [["hips", "Root"], ["head", "Head"]],
            "notes": {"head": "n"},
            "placements": {"Head": {"t": [0.0, 0.25, -0.125], "r": [0.0, 0.0, 0.0, 1.0]}, "Root": {"t": [0.5, 0.0, 0.0]}}}"#;
        let t = JointTable::parse(text).unwrap();
        let offsets = t.offsets(&skeleton(&["Root", "Head"])).unwrap();
        assert_eq!(
            offsets,
            [
                (
                    JointId(11),
                    JointOffset {
                        translation: [0.0, 0.25, -0.125],
                        rotation: [0.0, 0.0, 0.0, 1.0]
                    }
                ),
                (
                    JointId(10),
                    JointOffset {
                        translation: [0.5, 0.0, 0.0],
                        ..JointOffset::IDENTITY
                    }
                )
            ]
        );
        let again = JointTable::parse(&t.to_json().unwrap()).unwrap();
        assert_eq!(again, t);
        // a version 1 table (no placements) is read, and written back as the version it states
        let v1 =
            JointTable::parse(r#"{"version": 1, "from": "a", "to": "b", "pairs": [["a", "X"]]}"#).unwrap();
        assert!(v1.placements.is_empty());
        assert_eq!(JointTable::parse(&v1.to_json().unwrap()).unwrap(), v1);
        // a table cannot be written that could not be read
        let mut bad = v1.clone();
        bad.placements
            .insert("X".into(), Placed::from(JointOffset::IDENTITY));
        assert!(bad.to_json().unwrap_err().0.contains("placements are version 2"));
        assert!(
            t.offsets(&skeleton(&["Root"]))
                .unwrap_err()
                .0
                .contains("no target joint Head")
        );
    }
}
