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
        let masked = input.marks.iter().any(|m| !m.is_empty());
        w.insert_resource(DressingResult {
            masked,
            dressing,
            note: if masked { note } else { None },
        });
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
                 as it stands; every mask group is drawn instead",
                list(&marked)
            )),
        );
    }
    let note = chosen.then(|| {
        format!(
            "npc=first-map chose NPC row {row}: the first row of a map's part that names {id}; state npc=<row> to \
             dress it otherwise (masks on: {})",
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
        assert!(note.contains("npc=first-map chose NPC row 11000000"), "{note}");
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
///
/// The program states every choice of the three steps it runs: `lower-havok-clips` (`absent`, `rate`, `root-motion`,
/// `clips`), `correspond-skeletons` (`from`, `to`, `tolerance`, `parents`) and `map-clip` (`unmapped`, `rests`,
/// `reparented`). None is a fact of the source, so none has a default. `parents`, `rests` and `reparented` state how
/// the clips map where the two skeletons share their hierarchy; `empty=model-space` is the program's statement that,
/// where no joint pair shares a hierarchy, the clips map in model space instead (`parents=any`, `rests=source`,
/// `reparented=model-space`) and `empty=refuse` that they do not.
#[derive(Clone, Debug)]
pub struct BindFromSoftAnimation {
    pub from: String,
    pub to: String,
    pub tolerance: f64,
    pub empty: String,
    pub absent: String,
    pub rate: u32,
    pub root_motion: String,
    pub clips: String,
    pub unmapped: String,
    pub parents: String,
    pub rests: String,
    pub reparented: String,
}
pub const BIND_FROMSOFT_ANIMATION: PassSpec = PassSpec {
    name: "bind-fromsoft-animation",
    options: &[
        OptionSpec::required("absent", "neutral", "interpretation of absent track components"),
        OptionSpec::required("rate", "1", "sample keys per source frame"),
        OptionSpec::required(
            "root-motion",
            "leave",
            "leave, translation or full extracted root motion",
        ),
        OptionSpec::required("clips", "all", "selected animation names or all"),
        OptionSpec::required("from", "c0000 havok skeleton", "source skeleton name"),
        OptionSpec::required("to", "assembled skeleton", "target skeleton name"),
        OptionSpec::required("tolerance", "0.001", "rest agreement in model units"),
        OptionSpec::required(
            "empty",
            "model-space",
            "refuse or explicitly use model-space when no same-hierarchy pairs exist",
        ),
        OptionSpec::required(
            "unmapped",
            "drop",
            "what becomes of a source track with no target joint: map-clip's unmapped",
        ),
        OptionSpec::required(
            "parents",
            "same",
            "which joint pairs correspond: correspond-skeletons' parents (same or any)",
        ),
        OptionSpec::required(
            "rests",
            "target",
            "whose rest the clips are expressed against: map-clip's rests (source or target)",
        ),
        OptionSpec::required(
            "reparented",
            "refuse",
            "a mapped joint whose parent differs: map-clip's reparented (model-space or refuse)",
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
            absent: g.required("absent").into(),
            rate: g
                .required("rate")
                .parse()
                .map_err(|_| "rate is not a whole number")?,
            root_motion: g.required("root-motion").into(),
            clips: g.required("clips").into(),
            unmapped: g.required("unmapped").into(),
            parents: g.required("parents").into(),
            rests: g.required("rests").into(),
            reparented: g.required("reparented").into(),
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
            ("absent".into(), self.absent.clone()),
            ("rate".into(), self.rate.to_string()),
            ("root-motion".into(), self.root_motion.clone()),
            ("clips".into(), self.clips.clone()),
            ("unmapped".into(), self.unmapped.clone()),
            ("parents".into(), self.parents.clone()),
            ("rests".into(), self.rests.clone()),
            ("reparented".into(), self.reparented.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        use primase_core::Component;
        &[
            primase_core::Skeleton::NAME,
            primase_havok::world::HavokAnimation::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        use primase_core::Component;
        &[
            primase_core::JointMap::NAME,
            primase_core::Name::NAME,
            primase_core::Clip::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        use primase_core::Component;
        &[primase_core::Clip::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let steps = self.expansion(w)?;
        let mut registry = primase_core::pass::PassRegistry::new();
        registry.add(&primase_havok::world::LOWER_HAVOK_CLIPS);
        registry.add(&primase_animation::correspond::CORRESPOND_SKELETONS);
        registry.add(&primase_animation::map::MAP_CLIP);
        let pipeline = registry.parse(&steps.join(", ")).map_err(|e| e.to_string())?;
        for pass in pipeline.passes {
            pass.run(w, log)?;
        }
        Ok(())
    }
}

/// `select-fromsoft-requested-form` (no options): the form of a part the read was asked for, kept. A part's file holds
/// every form of the part and the read reads them all; it records the form it was asked for (its `fromsoft-parts`
/// scope's `form=`, in `core.read_scopes`), and this pass keeps that form's asset and the animations the read was asked
/// for (`animations_of=`, the asset `<of> animations`), leaving every other entity out as `select-assets{names=...}`
/// does. A read asked for no form (a character, or every form of a part) is left as it is, and the pass says so.
#[derive(Clone, Copy, Debug)]
pub struct SelectFromSoftRequestedForm;
pub const SELECT_FROMSOFT_REQUESTED_FORM: PassSpec = PassSpec {
    name: "select-fromsoft-requested-form",
    options: &[],
    make: |_| Ok(Box::new(SelectFromSoftRequestedForm)),
};
impl Pass for SelectFromSoftRequestedForm {
    fn name(&self) -> &'static str {
        "select-fromsoft-requested-form"
    }
    fn requires(&self) -> &[&'static str] {
        use primase_core::Component;
        &[
            primase_core::Asset::NAME,
            <primase_core::read_scope::ReadScopes as Resource>::NAME,
        ]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let scopes = w
            .resource::<primase_core::read_scope::ReadScopes>()
            .ok_or("no read scope: the read records what it was asked for")?;
        let parts: Vec<&primase_core::read_scope::ReadScope> = scopes
            .0
            .iter()
            .filter(|scope| scope.reader == crate::sekiro::components::standalone::PARTS_SCOPE.name)
            .collect();
        let scope = match parts.as_slice() {
            [] => None,
            [scope] => Some(*scope),
            more => {
                return Err(format!(
                    "{} reads of parts in one world: which form each was asked for is not one",
                    more.len()
                ));
            }
        };
        let Some((form, of)) = scope.and_then(|scope| {
            scope
                .choices
                .get("form")
                .map(|form| (form.clone(), scope.choices.get("animations_of").cloned()))
        }) else {
            log.count("reads asked for no one form: every asset kept", 1);
            return Ok(());
        };
        let mut names = vec![form];
        names.extend(of.map(|of| format!("{of} animations")));
        primase_core::asset::keep_assets(w, primase_core::asset::AssetField::Name, &names, log)
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
            crate::sekiro::components::standalone::WeaponInventory::NAME,
            crate::sekiro::outfit_data::PartLevelSelection::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[crate::sekiro::components::standalone::SelectedWeaponForms::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::{
            components::standalone::{SelectedWeaponForms, WeaponInventory},
            outfit_data::{PartLevel, PartLevelSelection},
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
        &[crate::sekiro::components::standalone::SavedPlayer::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[crate::sekiro::components::standalone::SelectedSavedHold::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::{
            components::attack_clip::{Hand, PlayerState},
            components::standalone::{SavedPlayer, SelectedSavedHold},
        };
        let read = &w.resource::<SavedPlayer>().ok_or("no saved player")?.0;
        let state = PlayerState::from_save(read)?;
        let report = serde_json::json!({"slot":read.index,"version":read.version,"arm_style":format!("{:?}",read.hold.arm_style),"active":{"right":read.hold.right_hand,"left":read.hold.left_hand},"right_hand":read.equipped.right_hand,"left_hand":read.equipped.left_hand,"held":{"right":state.active_weapon(Hand::Right),"left":state.active_weapon(Hand::Left)}});
        log.count("saved player holds selected", 1);
        w.insert_resource(SelectedSavedHold { state, report });
        Ok(())
    }
}

/// Apply program-stated initializer choices to the reader's character family. Delegates to the one shared selector implementation.
#[derive(Clone, Debug)]
pub struct SelectFromSoftCharacterRead {
    pub outfit: String,
    pub initializer: String,
}
pub const SELECT_FROMSOFT_CHARACTER_READ: PassSpec = PassSpec {
    name: "select-fromsoft-character-read",
    options: &[
        OptionSpec::required(
            "outfit",
            "first-readable",
            "explicit listed player outfit, first-readable or none",
        ),
        OptionSpec::required(
            "initializer",
            "first-readable",
            "NPC initializer id, first-readable or none",
        ),
    ],
    make: |g| {
        Ok(Box::new(SelectFromSoftCharacterRead {
            outfit: g.required("outfit").into(),
            initializer: g.required("initializer").into(),
        }))
    },
};
impl Pass for SelectFromSoftCharacterRead {
    fn name(&self) -> &'static str {
        "select-fromsoft-character-read"
    }
    fn options(&self) -> Options {
        Options::from([
            ("outfit".into(), self.outfit.clone()),
            ("initializer".into(), self.initializer.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        use crate::sekiro::{
            components::standalone::CharacterOutfitKind,
            outfit_data::{NpcOutfitVariants, OutfitVariants},
        };
        &[
            CharacterOutfitKind::NAME,
            OutfitVariants::NAME,
            NpcOutfitVariants::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        use crate::sekiro::{
            components::standalone::CharacterReadChoice,
            outfit_data::{SelectedNpcInitializer, SelectedPlayerOutfit},
        };
        &[
            CharacterReadChoice::NAME,
            SelectedPlayerOutfit::NAME,
            SelectedNpcInitializer::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        self.provides()
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::passes::initializer_outfits::{
            SelectFromSoftNpcInitializer, SelectFromSoftPlayerOutfit,
        };
        use crate::sekiro::{
            components::standalone::{CharacterOutfitKind, CharacterReadChoice},
            outfit_data::{SelectedNpcInitializer, SelectedPlayerOutfit},
        };
        use primase_core::convert::{Notice, NoticeKind};
        use serde_json::json;
        let kind = w
            .resource::<CharacterOutfitKind>()
            .ok_or("no character outfit family")?
            .clone();
        let mut choice = CharacterReadChoice {
            player: None,
            npc: None,
            report: json!({}),
            notices: vec![],
        };
        // a stated initializer row dresses a placed NPC whose model has no mesh of its own, and nothing else
        if !matches!(self.initializer.as_str(), "none" | "first-readable") {
            if kind.player {
                return Err(format!(
                    "initializer {} dresses a placed NPC, and {} is read as the player: state initializer=none, or                      read {} as an NPC",
                    self.initializer, kind.id, kind.id
                ));
            }
            if !kind.npc_parts {
                return Err(format!(
                    "initializer {}: an initializer dresses a Sekiro NPC with no drawable mesh of its own, and {} is                      not one: state initializer=none",
                    self.initializer, kind.id
                ));
            }
        }
        if kind.player && self.outfit != "none" {
            SelectFromSoftPlayerOutfit(self.outfit.clone()).run(w, log)?;
            let selected = w
                .resource::<SelectedPlayerOutfit>()
                .ok_or("player selector made no choice")?;
            let outfit = &selected.chosen;
            choice.report["player_outfit"] = json!({"chosen":outfit,"key":outfit.key(),"selection":if selected.automatic{"outfit=first-readable: the first readable listed variant; not the game's default"}else{"explicit listed variant"}});
            if selected.automatic {
                choice.report["selection_choices"] = json!([{"source":"fromsoft-character","option":"outfit","value":outfit.key(),"why":"outfit=first-readable: the first readable installed initializer variant, not the game's default"}]);
            }
            choice.notices.push(Notice::new(
                "fromsoft",
                NoticeKind::Selection,
                format!(
                    "{} outfit {}: {} ({})",
                    if selected.automatic {
                        "First readable"
                    } else {
                        "Selected"
                    },
                    outfit.key(),
                    outfit.name,
                    if selected.automatic {
                        "not the game's default"
                    } else {
                        "installed initializer"
                    }
                ),
                1,
            ));
            choice.player = Some(outfit.clone());
        } else if self.outfit != "none" && self.outfit != "first-readable" {
            return Err(format!(
                "outfit selects a listed player presentation; it cannot replace {}'s own model or NPC initializer",
                kind.id
            ));
        }
        if kind.player && choice.player.is_none() {
            // outfit=none: the player's model read alone
            let id = &kind.id;
            choice.report["player_model"] = json!({"skeleton_only": kind.meshless, "outfit_selected": false,
                "reason": "outfit=none: no player outfit is read"});
            choice.notices.push(Notice::new(
                "fromsoft",
                NoticeKind::Selection,
                if kind.meshless {
                    format!(
                        "{id} is the player's rig: its skeleton and dummy sockets are read and, with outfit=none, no                          wearable mesh; state outfit (a listed player outfit, or first-readable) to dress it"
                    )
                } else {
                    format!("{id} is read as the player's own model with outfit=none; state outfit to dress it")
                },
                1,
            ));
        }
        if kind.npc_parts && choice.player.is_none() && self.initializer != "none" {
            SelectFromSoftNpcInitializer(self.initializer.clone()).run(w, log)?;
            let selected = w
                .resource::<SelectedNpcInitializer>()
                .ok_or("NPC selector made no choice")?;
            choice.report["npc_equipment"] = json!({"chosen":selected.chosen,"choices":selected.choices,"selection":"caller init or first authored readable outfit in map/placement order","omitted":"face-generation morphs, body scale and runtime equipment replacements"});
            choice.notices.push(Notice::new("fromsoft",NoticeKind::Selection,"meshless human dressed from the MSB enemy's CharaInitID and equipped protector models; alternative authored outfits are listed; face morphs/body scale/runtime replacements are not read",1));
            choice.npc = Some(selected.chosen.clone());
        }
        w.insert_resource(choice);
        Ok(())
    }
}

impl BindFromSoftAnimation {
    /// Diagnostic expansion of this registered operation. Programs run the operation, never branch on this text.
    pub fn expansion(&self, w: &World) -> Result<Vec<String>, String> {
        let counts = primase_animation::correspond::hierarchy_counts(w, &self.from, &self.to, self.tolerance);
        let model_space = counts.is_some_and(|c| c.paired == 0) && self.empty == "model-space";
        let (parents, rests, reparented) = if model_space {
            ("any", "source", "model-space")
        } else {
            (
                self.parents.as_str(),
                self.rests.as_str(),
                self.reparented.as_str(),
            )
        };
        Ok(vec![
            format!(
                "lower-havok-clips{{absent={},rate={},root-motion={},clips={}}}",
                self.absent, self.rate, self.root_motion, self.clips
            ),
            format!(
                "correspond-skeletons{{from={},to={},tolerance={},parents={parents}}}",
                self.from, self.to, self.tolerance
            ),
            format!(
                "map-clip{{unmapped={},rests={rests},reparented={reparented}}}",
                self.unmapped
            ),
        ])
    }
}
#[cfg(test)]
mod binding_contract {
    use super::*;
    #[test]
    fn registered_binding_states_the_three_steps_it_is_given() {
        for (parents, rests, reparented) in [("any", "source", "model-space"), ("same", "target", "refuse")] {
            let bind = BindFromSoftAnimation {
                from: "c2010 havok skeleton".into(),
                to: "c2010 skeleton".into(),
                tolerance: 0.0001,
                empty: "model-space".into(),
                absent: "neutral".into(),
                rate: 1,
                root_motion: "translation".into(),
                clips: "a000_000000".into(),
                unmapped: "drop".into(),
                parents: parents.into(),
                rests: rests.into(),
                reparented: reparented.into(),
            };
            let world = World::new();
            assert_eq!(
                bind.expansion(&world).unwrap(),
                vec![
                    "lower-havok-clips{absent=neutral,rate=1,root-motion=translation,clips=a000_000000}"
                        .to_owned(),
                    format!(
                        "correspond-skeletons{{from=c2010 havok skeleton,to=c2010 skeleton,tolerance=0.0001,parents={parents}}}"
                    ),
                    format!("map-clip{{unmapped=drop,rests={rests},reparented={reparented}}}")
                ]
            );
        }
    }

    #[test]
    fn each_choice_of_the_three_steps_is_a_required_option() {
        let mut registry = primase_core::pass::PassRegistry::new();
        registry.add(&BIND_FROMSOFT_ANIMATION);
        let stated = "from=c0000 havok skeleton,to=c0000 skeleton,tolerance=0.0001,empty=model-space,absent=neutral,            rate=1,root-motion=leave,clips=all,unmapped=drop,parents=any,rests=source,reparented=model-space";
        registry
            .parse(&format!("bind-fromsoft-animation{{{stated}}}"))
            .expect("all stated");
        for option in ["unmapped", "parents", "rests", "reparented"] {
            let without: Vec<_> = stated
                .split(',')
                .filter(|part| !part.starts_with(&format!("{option}=")))
                .collect();
            let err = registry
                .parse(&format!("bind-fromsoft-animation{{{}}}", without.join(",")))
                .err()
                .unwrap_or_else(|| panic!("{option} has a default"))
                .to_string();
            assert!(err.contains(option), "{err}");
        }
    }
}

#[cfg(test)]
mod character_read_contract {
    use super::*;
    use crate::sekiro::components::standalone::{CharacterOutfitKind, CharacterReadChoice};

    fn read(
        kind: CharacterOutfitKind,
        outfit: &str,
        initializer: &str,
    ) -> Result<CharacterReadChoice, String> {
        let mut w = World::new();
        w.insert_resource(kind);
        let pass = SelectFromSoftCharacterRead {
            outfit: outfit.into(),
            initializer: initializer.into(),
        };
        pass.run(&mut w, &mut PassLog::default())?;
        Ok(w.resource::<CharacterReadChoice>().ok_or("no choice")?.clone())
    }
    fn kind(player: bool, npc_parts: bool, meshless: bool) -> CharacterOutfitKind {
        CharacterOutfitKind {
            id: "c0000".into(),
            player,
            npc_parts,
            meshless,
        }
    }

    /// The player read with `outfit=none` is its skeleton alone, and the read says so; an initializer row is refused
    /// where the stated kind has no use for it.
    #[test]
    fn the_player_read_alone_is_recorded_and_an_initializer_it_cannot_use_is_refused() {
        let choice = read(kind(true, false, true), "none", "none").unwrap();
        assert_eq!(choice.report["player_model"]["skeleton_only"], true);
        assert_eq!(choice.report["player_model"]["outfit_selected"], false);
        let notice = choice
            .notices
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        assert!(notice.contains("state outfit"), "{notice}");
        let err = read(kind(true, false, true), "none", "4500").unwrap_err();
        assert!(
            err.contains("initializer 4500") && err.contains("read as the player"),
            "{err}"
        );
        let err = read(kind(false, false, false), "none", "4500").unwrap_err();
        assert!(
            err.contains("initializer 4500") && err.contains("state initializer=none"),
            "{err}"
        );
        // an NPC read with no initializer has no player model to report
        let choice = read(kind(false, false, false), "none", "none").unwrap();
        assert!(choice.report.get("player_model").is_none());
    }
}

/// Execute the mask lowering selected earlier, retaining every delegated pass's declarations and counts.
#[derive(Debug)]
pub struct LowerFromSoftSelectedOutfit {
    provides: Vec<&'static str>,
    removes: Vec<&'static str>,
    modifies: Vec<&'static str>,
}
fn outfit_registry() -> primase_core::pass::PassRegistry {
    let mut registry = primase_core::pass::PassRegistry::new();
    registry.add(&super::outfits::RAISE_FROMSOFT_DISPLAY_MASKS);
    registry.add(&super::outfits::SELECT_FROMSOFT_OUTFIT);
    registry.add(&super::visibility::PRESERVE_FROMSOFT_DISPLAY_MASKS);
    registry.add(&crate::display_masks::LOWER_FROMSOFT_DISPLAY_MASKS);
    registry.add(&primase_core::state::RESOLVE_STATE);
    registry
}
impl LowerFromSoftSelectedOutfit {
    fn new() -> Result<Self, String> {
        let registry = outfit_registry();
        let mut provides = std::collections::BTreeSet::new();
        let mut removes = std::collections::BTreeSet::new();
        let mut modifies = std::collections::BTreeSet::new();
        for name in registry.names() {
            let pass = registry.make_registered(name).map_err(|e| e.to_string())?;
            provides.extend(pass.provides().iter().copied());
            removes.extend(pass.removes().iter().copied());
            modifies.extend(pass.modifies().iter().copied());
        }
        Ok(Self {
            provides: provides.into_iter().collect(),
            removes: removes.into_iter().collect(),
            modifies: modifies.into_iter().collect(),
        })
    }
    fn expansion(w: &World) -> Result<Vec<String>, String> {
        let result = w
            .resource::<DressingResult>()
            .ok_or("no stated display-mask selection")?;
        if !result.masked {
            return Ok(vec![]);
        }
        let id = &w
            .resource::<DressingInputs>()
            .ok_or("no character mask identity")?
            .id;
        let mut texts = vec!["raise-fromsoft-display-masks".into()];
        match result.dressing {
            Dressing::Row(row) => texts.push(format!("select-fromsoft-outfit{{chr={id},npc={row}}}")),
            Dressing::Hidden => texts.push(format!("select-fromsoft-outfit{{chr={id},npc=none}}")),
            Dressing::ShowAll => {}
        }
        texts.push(format!(
            "preserve-fromsoft-display-masks{{defaults={}}}",
            if result.dressing == Dressing::ShowAll {
                "show"
            } else {
                "assigned"
            }
        ));
        texts.push("lower-fromsoft-display-masks".into());
        texts.push("resolve-state".into());
        Ok(texts)
    }
}
pub const LOWER_FROMSOFT_SELECTED_OUTFIT: PassSpec = PassSpec {
    name: "lower-fromsoft-selected-outfit",
    options: &[],
    make: |_| Ok(Box::new(LowerFromSoftSelectedOutfit::new()?)),
};
impl Pass for LowerFromSoftSelectedOutfit {
    fn name(&self) -> &'static str {
        "lower-fromsoft-selected-outfit"
    }
    fn requires(&self) -> &[&'static str] {
        &[DressingResult::NAME, DressingInputs::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &self.provides
    }
    fn removes(&self) -> &[&'static str] {
        &self.removes
    }
    fn modifies(&self) -> &[&'static str] {
        &self.modifies
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn removes_by_log(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let texts = Self::expansion(w)?;
        let registry = outfit_registry();
        let pipeline = registry.parse(&texts.join(", ")).map_err(|e| e.to_string())?;
        for pass in pipeline.passes {
            pass.run(w, log)?;
        }
        Ok(())
    }
}
