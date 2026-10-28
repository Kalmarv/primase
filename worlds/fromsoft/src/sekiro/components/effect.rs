//! What the effect read keeps for `lower-fromsoft-effects`: the effect files as stored (`fromsoft.effect_file`), the places
//! effects stand (`fromsoft.effect`) and the files effects name by number (`fromsoft.effect_resource`). The read selects and
//! places; the pass interprets.
use super::source::EffectPlace;
use fromsoft_formats::Game;
use fromsoft_formats::fxr::Fxr;
use fromsoft_formats::fxr::resources::TextureKind;
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, Geometry};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// An effect file of the install, as stored, on an entity of its own (not placed): the lowering of an effect and of the
/// effects it spawns finds its files here by their numbers. `game` is the layout the file's actions are read in.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectFile {
    pub effect: i32,
    pub game: Game,
    pub file: Arc<Fxr>,
}
impl Component for EffectFile {
    const NAME: &'static str = "fromsoft.effect_file";
    const GEOMETRY: Geometry = Geometry::None;
    /// The effect's number and the sizes of its tree, not the tree.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "effect": self.effect,
            "game": self.game,
            "actions": self.file.actions().len(),
            "states": self.file.states.len(),
        })))
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// An effect standing where an entity's transform places it (`fromsoft.effect`): which effect, and the route that placed it.
/// `starts_disabled` is an SFX region's flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Effect {
    pub effect: i32,
    pub place: EffectPlace,
    pub starts_disabled: bool,
}
impl Component for Effect {
    const NAME: &'static str = "fromsoft.effect";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// What an effect names by number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ResourceKind {
    /// A colour texture.
    Albedo,
    /// A normal map.
    Normal,
    /// A specular parameter map.
    Specular,
    /// A detail map.
    Detail,
    /// A mesh-particle model.
    Model,
    /// A model animation.
    Animation,
}

impl ResourceKind {
    /// The texture kind of a texture resource.
    pub fn texture(self) -> Option<TextureKind> {
        match self {
            ResourceKind::Albedo => Some(TextureKind::Albedo),
            ResourceKind::Normal => Some(TextureKind::Normal),
            ResourceKind::Specular => Some(TextureKind::Specular),
            ResourceKind::Detail => Some(TextureKind::Detail),
            ResourceKind::Model | ResourceKind::Animation => None,
        }
    }

    /// The resource kind of a texture kind.
    pub fn of_texture(kind: TextureKind) -> ResourceKind {
        match kind {
            TextureKind::Albedo => ResourceKind::Albedo,
            TextureKind::Normal => ResourceKind::Normal,
            TextureKind::Specular => ResourceKind::Specular,
            TextureKind::Detail => ResourceKind::Detail,
        }
    }
}

/// A file effects name by number (`fromsoft.effect_resource`), on the entity that holds it (a `core.texture` for a texture):
/// its kind and number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EffectResource {
    pub kind: ResourceKind,
    pub id: i32,
}
impl Component for EffectResource {
    const NAME: &'static str = "fromsoft.effect_resource";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_kinds_round_trip_through_texture_kinds() {
        for kind in [TextureKind::Albedo, TextureKind::Normal, TextureKind::Specular, TextureKind::Detail] {
            assert_eq!(ResourceKind::of_texture(kind).texture(), Some(kind));
        }
        assert_eq!(ResourceKind::Model.texture(), None);
    }
}
