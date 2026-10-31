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
