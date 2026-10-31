//! A program's explicitly stated NPC-row and empty-mask policy over authored model observations.
pub use crate::sekiro::outfit_data::{Dressing, DressingInputs, DressingResult};
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Resource, World};
#[derive(Clone, Debug)]
pub struct SelectFromSoftDressing {
    pub npc: String,
    pub empty: String,
}
pub const SELECT_FROMSOFT_DRESSING: PassSpec = PassSpec {
    name: "select-fromsoft-dressing",
    options: &[
        OptionSpec::required(
            "npc",
            "first-map",
            "NPC row id, none, or explicit first-map choice",
        ),
        OptionSpec::required(
            "empty",
            "show",
            "show or hide when the selection would leave no visible mesh",
        ),
    ],
    make: |g| {
        let npc = g.required("npc");
        if !matches!(npc, "first-map" | "none") && npc.parse::<i32>().is_err() {
            return Err("npc is not a row id, none or first-map".into());
        }
        let empty = g.required("empty");
        if !matches!(empty, "show" | "hide") {
            return Err("empty is not show or hide".into());
        }
        Ok(Box::new(SelectFromSoftDressing {
            npc: npc.into(),
            empty: empty.into(),
        }))
    },
};
impl Pass for SelectFromSoftDressing {
    fn name(&self) -> &'static str {
        "select-fromsoft-dressing"
    }
    fn options(&self) -> Options {
        Options::from([
            ("npc".into(), self.npc.clone()),
            ("empty".into(), self.empty.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[DressingInputs::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[DressingResult::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let input = w.resource::<DressingInputs>().ok_or("no mask/row observations")?;
        let asked = if matches!(self.npc.as_str(), "first-map" | "none") {
            None
        } else {
            Some(self.npc.parse().map_err(|_| "npc is not an integer")?)
        };
        let named = if self.npc == "first-map" {
            input.named.as_slice()
        } else {
            &[]
        };
        let (mut dressing, mut note) = dress(&input.id, &input.marks, &input.rows, asked, named);
        if self.empty == "hide" && dressing == Dressing::ShowAll {
            dressing = Dressing::Hidden;
            note = None;
        }
        if let Some(why) = &note {
            log.approximate(why, 1);
        }
        log.count("character display-mask choices", 1);
        w.insert_resource(DressingResult { dressing, note });
        Ok(())
    }
}
fn dress(
    id: &str,
    marks: &[Vec<u32>],
    rows: &[fromsoft_formats::param::NpcRow],
    asked: Option<i32>,
    named: &[i32],
) -> (Dressing, Option<String>) {
    let unmarked = marks.iter().any(Vec::is_empty);
    let marked: std::collections::BTreeSet<u32> = marks.iter().flatten().copied().collect();
    let list = |masks: &std::collections::BTreeSet<u32>| {
        masks.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    };
    let first_row = named
        .iter()
        .copied()
        .find(|row| rows.is_empty() || rows.iter().any(|param| param.id == *row));
    let (row, chosen) = match (asked, first_row) {
        (Some(row), _) => (Some(row), false),
        (None, Some(row)) => (Some(row), true),
        (None, None) => (None, false),
    };
    let Some(row) = row else {
        return if unmarked {
            (Dressing::Hidden, None)
        } else {
            (
                Dressing::ShowAll,
                Some(format!(
                    "no NPC row names {id} and every one of its meshes is marked with a display mask ({}): hiding the \
                     masks would leave nothing, so every mask group is drawn",
                    list(&marked)
                )),
            )
        };
    };
    let found = rows.iter().find(|r| r.id == row);
    let shown: std::collections::BTreeSet<u32> = found
        .map(|r| {
            marked
                .iter()
                .copied()
                .filter(|k| r.display_mask >> k & 1 == 1)
                .collect()
        })
        .unwrap_or_default();
    if found.is_some() && !unmarked && shown.is_empty() {
        return (
            Dressing::ShowAll,
            Some(format!(
                "NPC row {row} turns off every display mask {id}'s meshes are marked with ({}) and the model has no \
                 unmarked mesh: the row hides every mesh, so the game probably does not draw this NPC from this model \
                 as it stands; the lab draws every mask group instead",
                list(&marked)
            )),
        );
    }
    let note = chosen.then(|| {
        format!(
            "the lab chose NPC row {row}: the first row of a map's part that names {id}; state npc to dress it \
             otherwise (masks on: {})",
            if shown.is_empty() {
                "none".to_owned()
            } else {
                list(&shown)
            }
        )
    });
    (Dressing::Row(row), note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::param::NpcRow;
    fn row(id: i32, display_mask: u32) -> NpcRow {
        NpcRow { id, display_mask }
    }
    fn selected(
        id: &str,
        marks: &[Vec<u32>],
        rows: &[NpcRow],
        asked: Option<i32>,
        named: &[i32],
    ) -> (Dressing, Option<String>) {
        let mut w = World::new();
        w.insert_resource(DressingInputs {
            id: id.into(),
            marks: marks.to_vec(),
            rows: rows.to_vec(),
            named: named.to_vec(),
        });
        let choice = SelectFromSoftDressing {
            npc: asked.map_or_else(|| "first-map".into(), |n| n.to_string()),
            empty: "show".into(),
        };
        let mut log = PassLog::default();
        choice.run(&mut w, &mut log).unwrap();
        let result = w.resource::<DressingResult>().unwrap();
        (result.dressing, result.note.clone())
    }
    #[test]
    fn the_lab_dresses_a_masked_character_and_never_leaves_nothing() {
        // c1100-like: every mesh is marked (masks 10 and 11), no row names it: hiding would leave nothing
        let all_marked = vec![vec![10], vec![11], vec![10]];
        let (dressing, note) = selected("c1100", &all_marked, &[], None, &[]);
        assert_eq!(dressing, Dressing::ShowAll);
        assert!(note.unwrap().contains("every mask group is drawn"));
        // the first row naming it dresses it, and the notice says which and what it turns on
        let rows = [row(11000000, 1 << 10)];
        let (dressing, note) = selected("c1100", &all_marked, &rows, None, &[11000000]);
        assert_eq!(dressing, Dressing::Row(11000000));
        let note = note.unwrap();
        assert!(note.contains("the lab chose NPC row 11000000"), "{note}");
        assert!(note.contains("masks on: 10"), "{note}");
        // a row the caller asked for is theirs: no choice notice
        let (dressing, note) = selected("c1100", &all_marked, &rows, Some(11000000), &[5]);
        assert_eq!(dressing, Dressing::Row(11000000));
        assert_eq!(note, None);
        // c1320's row 13209200 turns off every mask there is: shown whole, with the reason
        let rows = [row(13209200, 0)];
        let (dressing, note) = selected("c1320", &all_marked, &rows, Some(13209200), &[]);
        assert_eq!(dressing, Dressing::ShowAll);
        assert!(note.unwrap().contains("hides every mesh"));
        // a model with an unmarked body and no row keeps the stated `npc=none`
        let with_body = vec![vec![], vec![3]];
        assert_eq!(
            selected("c3000", &with_body, &[], None, &[]),
            (Dressing::Hidden, None)
        );
        // ... and a row that turns every mask off still leaves the body: it is the caller's row
        let rows = [row(7, 0)];
        assert_eq!(
            selected("c3000", &with_body, &rows, Some(7), &[]).0,
            Dressing::Row(7)
        );
        // a row the param does not have is primase's to refuse
        assert_eq!(
            selected("c3000", &all_marked, &[], Some(9), &[]).0,
            Dressing::Row(9)
        );
        // a row a map names that the NPC param does not hold is skipped (Elden's c3300 names 33000061 first)
        let held = [row(33000070, 1 << 10)];
        assert_eq!(
            selected("c3300", &all_marked, &held, None, &[33000061, 33000070]).0,
            Dressing::Row(33000070)
        );
        assert_eq!(
            selected("c3300", &all_marked, &held, None, &[33000061]).0,
            Dressing::ShowAll,
            "no named row is held: every mask group is drawn, as with no row at all"
        );
    }
}

/// Bind an already lowered character clip to its chosen model skeleton, interpreting only source identity here.
#[derive(Clone, Debug)]
pub struct BindFromSoftAnimation {
    pub from: String,
    pub to: String,
    pub tolerance: f64,
    pub empty: String,
}
pub const BIND_FROMSOFT_ANIMATION: PassSpec = PassSpec {
    name: "bind-fromsoft-animation",
    options: &[
        OptionSpec::required("from", "c0000 havok skeleton", "source skeleton name"),
        OptionSpec::required("to", "assembled skeleton", "target skeleton name"),
        OptionSpec::required("tolerance", "0.001", "rest agreement in model units"),
        OptionSpec::required(
            "empty",
            "model-space",
            "refuse or explicitly use model-space when no same-hierarchy pairs exist",
        ),
    ],
    make: |g| {
        let tolerance = g
            .required("tolerance")
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.)
            .ok_or("tolerance is not finite and positive")?;
        let empty = g.required("empty");
        if !matches!(empty, "refuse" | "model-space") {
            return Err("empty is not refuse or model-space".into());
        }
        Ok(Box::new(BindFromSoftAnimation {
            from: g.required("from").into(),
            to: g.required("to").into(),
            tolerance,
            empty: empty.into(),
        }))
    },
};
impl Pass for BindFromSoftAnimation {
    fn name(&self) -> &'static str {
        "bind-fromsoft-animation"
    }
    fn options(&self) -> Options {
        Options::from([
            ("from".into(), self.from.clone()),
            ("to".into(), self.to.clone()),
            ("tolerance".into(), self.tolerance.to_string()),
            ("empty".into(), self.empty.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        use primase_core::Component;
        &[
            primase_core::Skeleton::NAME,
            primase_core::Clip::NAME,
            crate::sekiro::source::CharacterGame::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        use primase_core::Component;
        &[primase_core::JointMap::NAME, primase_core::Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        use primase_core::Component;
        &[primase_core::Clip::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use primase_animation::{
            correspond::{CorrespondSkeletons, Parents, hierarchy_counts},
            map::{MapClip, Reparented, Rests, Unmapped},
        };
        let game = w
            .resource::<crate::sekiro::source::CharacterGame>()
            .ok_or("clip has no source game identity")?
            .0;
        let counts = hierarchy_counts(w, &self.from, &self.to, self.tolerance);
        let fallback = game == fromsoft_formats::Game::Sekiro
            && counts.is_some_and(|c| c.paired == 0)
            && self.empty == "model-space";
        let model = game == fromsoft_formats::Game::EldenRing || fallback;
        if fallback {
            log.approximate(
                "caller choice: no same-hierarchy bone pairs; map by model-space pose",
                1,
            );
        }
        CorrespondSkeletons {
            from: self.from.clone(),
            to: self.to.clone(),
            tolerance: self.tolerance,
            rotation_tolerance: self.tolerance,
            parents: if model { Parents::Any } else { Parents::Same },
        }
        .run(w, log)?;
        MapClip {
            unmapped: Unmapped::Drop,
            rests: if model { Rests::Source } else { Rests::Target },
            reparented: if model {
                Reparented::ModelSpace
            } else {
                Reparented::Refuse
            },
        }
        .run(w, log)
    }
}

/// One part-level policy shared with initializer selection; the reader only supplies candidates.
#[derive(Clone, Copy, Debug)]
pub struct SelectFromSoftWeaponForms;
pub const SELECT_FROMSOFT_WEAPON_FORMS: PassSpec = PassSpec {
    name: "select-fromsoft-weapon-forms",
    options: &[],
    make: |_| Ok(Box::new(SelectFromSoftWeaponForms)),
};
impl Pass for SelectFromSoftWeaponForms {
    fn name(&self) -> &'static str {
        "select-fromsoft-weapon-forms"
    }
    fn requires(&self) -> &[&'static str] {
        &[
            crate::sekiro::standalone::WeaponInventory::NAME,
            crate::sekiro::outfit_data::PartLevelSelection::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[crate::sekiro::standalone::SelectedWeaponForms::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::{
            outfit_data::{PartLevel, PartLevelSelection},
            standalone::{SelectedWeaponForms, WeaponInventory},
        };
        let level = w
            .resource::<PartLevelSelection>()
            .ok_or("no stated part-level policy")?
            .0;
        let forms = w
            .resource::<WeaponInventory>()
            .ok_or("no weapon inventory")?
            .rows
            .iter()
            .map(|row| {
                let selected = row
                    .candidates
                    .iter()
                    .filter(|c| c.present)
                    .find(|c| match level {
                        PartLevel::PreferLow => true,
                        PartLevel::Low => c.name.ends_with("_l"),
                        PartLevel::Full => !c.name.ends_with("_l"),
                    })
                    .map(|c| c.name.clone());
                (row.row.row, selected)
            })
            .collect();
        log.count("weapon form selections", 1);
        w.insert_resource(SelectedWeaponForms(forms));
        Ok(())
    }
}
/// Interpret the named saved slot's active hand and arm style, rather than doing so in a viewer.
#[derive(Clone, Copy, Debug)]
pub struct SelectFromSoftSavedHold;
pub const SELECT_FROMSOFT_SAVED_HOLD: PassSpec = PassSpec {
    name: "select-fromsoft-saved-hold",
    options: &[],
    make: |_| Ok(Box::new(SelectFromSoftSavedHold)),
};
impl Pass for SelectFromSoftSavedHold {
    fn name(&self) -> &'static str {
        "select-fromsoft-saved-hold"
    }
    fn requires(&self) -> &[&'static str] {
        &[crate::sekiro::standalone::SavedPlayer::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[crate::sekiro::standalone::SelectedSavedHold::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::{
            attack_clip::{Hand, PlayerState},
            standalone::{SavedPlayer, SelectedSavedHold},
        };
        let read = &w.resource::<SavedPlayer>().ok_or("no saved player")?.0;
        let state = PlayerState::from_save(read)?;
        let report = serde_json::json!({"slot":read.index,"version":read.version,"arm_style":format!("{:?}",read.hold.arm_style),"active":{"right":read.hold.right_hand,"left":read.hold.left_hand},"right_hand":read.equipped.right_hand,"left_hand":read.equipped.left_hand,"held":{"right":state.active_weapon(Hand::Right),"left":state.active_weapon(Hand::Left)}});
        log.count("saved player holds selected", 1);
        w.insert_resource(SelectedSavedHold { state, report });
        Ok(())
    }
}
