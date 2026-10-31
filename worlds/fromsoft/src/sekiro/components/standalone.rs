//! Authored standalone inventories and explicit pass-selected inputs; no reader I/O.
use super::weapons::WeaponRow;
use primase_core::{Resource, convert::Notice};
#[derive(Clone, Debug)]
pub struct WeaponBinderCandidate {
    pub name: String,
    pub present: bool,
}
#[derive(Clone, Debug)]
pub struct WeaponInventoryRow {
    pub row: WeaponRow,
    pub base: String,
    pub candidates: Vec<WeaponBinderCandidate>,
}
#[derive(Clone, Debug)]
pub struct WeaponInventory {
    pub rows: Vec<WeaponInventoryRow>,
}
impl Resource for WeaponInventory {
    const NAME: &'static str = "fromsoft.weapon_inventory";
}

/// The selected forms keyed by weapon row. Missing binders are explicit, never fabricated.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SelectedWeaponForms(pub Vec<(i32, Option<String>)>);
impl Resource for SelectedWeaponForms {
    const NAME: &'static str = "fromsoft.selected_weapon_forms";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A raw save slot; the registered hold selector interprets it.
#[derive(Clone, Debug)]
pub struct SavedPlayer(pub fromsoft_formats::save::Slot);
impl Resource for SavedPlayer {
    const NAME: &'static str = "fromsoft.saved_player";
}

/// The explicit saved hold selected by a registered pass; reporting remains portable and display-only.
#[derive(Clone, Debug)]
pub struct SelectedSavedHold {
    pub state: crate::sekiro::components::attack_clip::PlayerState,
    pub report: serde_json::Value,
}
impl Resource for SelectedSavedHold {
    const NAME: &'static str = "fromsoft.selected_saved_hold";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(self.report.clone()))
    }
}

/// The source records which character families have authored part outfits. This is not a selected outfit.
#[derive(Clone, Debug)]
pub struct CharacterOutfitKind {
    pub id: String,
    pub player: bool,
    pub npc_parts: bool,
}
impl Resource for CharacterOutfitKind {
    const NAME: &'static str = "fromsoft.character_outfit_kind";
}

/// Selected typed inputs and portable descriptive output for the standalone read.
#[derive(Clone, Debug)]
pub struct CharacterReadChoice {
    pub player: Option<crate::sekiro::components::outfits::CharacterOutfit>,
    pub npc: Option<crate::sekiro::outfit_data::SelectedNpcOutfit>,
    pub report: serde_json::Value,
    pub notices: Vec<Notice>,
}
impl Resource for CharacterReadChoice {
    const NAME: &'static str = "fromsoft.character_read_choice";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(self.report.clone()))
    }
}
