//! Authored initializer inventories and pass-selected wearable presentations.
use fromsoft_formats::{
    Game,
    equipment::{OutfitCandidates, PresenceChoice},
};
use primase_core::Resource;
use serde::Serialize;
/// Which spelling the viewer permits where an initializer's protector permits both player presentations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum OutfitGender {
    Male,
    Female,
}

impl OutfitGender {
    pub fn key(self) -> &'static str {
        match self {
            Self::Male => "m",
            Self::Female => "f",
        }
    }
}

/// One installed `CharaInitParam` row, resolved through its `EquipParamProtector` rows and archive membership.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CharacterOutfit {
    pub initializer: i32,
    /// The stored param-row name, otherwise the param and row number. Community labels are not game data.
    pub name: String,
    pub gender: OutfitGender,
    /// The shared player animation rig of these games; no particular animation is chosen or decoded here.
    pub animations_of: String,
    pub parts: Vec<String>,
    pub unresolved: Vec<String>,
    pub chosen_by_presence: Vec<PresenceChoice>,
}

impl CharacterOutfit {
    pub fn key(&self) -> String {
        format!("initializer/{}/{}", self.initializer, self.gender.key())
    }

    pub fn readable(&self) -> bool {
        !self.parts.is_empty() && self.unresolved.is_empty()
    }
}

/// One authored initializer, including a named decoding refusal instead of fabricated parts.
#[derive(Clone, Debug, Serialize)]
pub struct InitializerEntry {
    pub id: i32,
    pub name: String,
    pub candidates: Result<OutfitCandidates, String>,
}
/// Raw initializer records and every supported file alternative. No viewer presentation is selected here.
#[derive(Clone, Debug, Serialize)]
pub struct InitializerOutfits {
    pub game: Game,
    pub rows: Vec<InitializerEntry>,
}
impl Resource for InitializerOutfits {
    const NAME: &'static str = "fromsoft.initializer_outfits";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
/// Chosen presentations, suitable for the character reader's explicit outfit scope.
#[derive(Clone, Debug, Serialize)]
pub struct OutfitVariants(pub Vec<CharacterOutfit>);
impl Resource for OutfitVariants {
    const NAME: &'static str = "fromsoft.outfit_variants";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Every map-authored NPC initializer and its unselected candidate inventory.
#[derive(Clone, Debug, Serialize)]
pub struct NpcOutfitRows {
    pub game: Game,
    pub rows: Vec<fromsoft_formats::equipment::NpcOutfit>,
}
impl Resource for NpcOutfitRows {
    const NAME: &'static str = "fromsoft.npc_outfit_rows";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
/// A pass-selected NPC presentation, preserving the existing portable report fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectedNpcOutfit {
    pub npc: i32,
    pub init: Option<i32>,
    pub map: String,
    pub placement: String,
    pub parts: Vec<String>,
    pub unresolved: Vec<String>,
    pub chosen_by_presence: Vec<PresenceChoice>,
}
/// Selected NPC presentations; the reader loads only explicitly selected scopes derived from these values.
#[derive(Clone, Debug, Serialize)]
pub struct NpcOutfitVariants(pub Vec<SelectedNpcOutfit>);
impl Resource for NpcOutfitVariants {
    const NAME: &'static str = "fromsoft.npc_outfit_variants";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Required level policy; `PreferLow` preserves the previous caller's stated preference and reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartLevel {
    PreferLow,
    Low,
    Full,
}
impl PartLevel {
    pub fn text(self) -> &'static str {
        match self {
            Self::PreferLow => "prefer-low",
            Self::Low => "low",
            Self::Full => "full",
        }
    }
}

/// The explicit shared part-level choice made by the registered initializer selector.
#[derive(Clone,Copy,Debug)]
pub struct PartLevelSelection(pub PartLevel);
impl Resource for PartLevelSelection {const NAME:&'static str="fromsoft.part_level_selection";}
/// The one outfit selected by a program from the listed, pass-selected presentations.
#[derive(Clone, Debug, Serialize)]
pub struct SelectedPlayerOutfit {
    pub chosen: CharacterOutfit,
    pub automatic: bool,
}
impl Resource for SelectedPlayerOutfit {
    const NAME: &'static str = "fromsoft.selected_player_outfit";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A program-selected map-authored initializer and the alternatives retained for reporting.
#[derive(Clone, Debug, Serialize)]
pub struct SelectedNpcInitializer {
    pub chosen: SelectedNpcOutfit,
    pub choices: Vec<SelectedNpcOutfit>,
}
impl Resource for SelectedNpcInitializer {
    const NAME: &'static str = "fromsoft.selected_npc_initializer";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
/// Required level policy; `PreferLow` preserves the previous caller's stated preference and reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartLevel {
    PreferLow,
    Low,
    Full,
}
impl PartLevel {
    pub fn text(self) -> &'static str {
        match self {
            Self::PreferLow => "prefer-low",
            Self::Low => "low",
            Self::Full => "full",
        }
    }
}

/// The explicit shared part-level choice made by the registered initializer selector.
#[derive(Clone,Copy,Debug)]
pub struct PartLevelSelection(pub PartLevel);
impl Resource for PartLevelSelection {const NAME:&'static str="fromsoft.part_level_selection";}
