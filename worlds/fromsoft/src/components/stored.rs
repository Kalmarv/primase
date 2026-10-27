//! Records `fromsoft-formats` reads, kept on entities as they are stored ([`Stored`]).
use fromsoft_formats::msb;
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, Geometry, Linear, world::Linearity};

/// A format record held as a component: the record as read, its JSON its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct Stored<T>(pub T);

impl<T> std::ops::Deref for Stored<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Stored<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// `fromsoft.shadow_flags`: a placement's raw shadow controls ([`msb::ShadowFlags`]).
pub type ShadowFlags = Stored<msb::ShadowFlags>;
/// `fromsoft.region`: a map layout region as stored ([`msb::Region`]).
pub type Region = Stored<msb::Region>;
/// `fromsoft.event`: a map layout event as stored ([`msb::Event`]).
pub type Event = Stored<msb::Event>;
/// `fromsoft.unplaced_part`: a map layout part the area read does not place as a model ([`msb::UnplacedPart`]).
pub type UnplacedPart = Stored<msb::UnplacedPart>;

impl Component for Stored<msb::ShadowFlags> {
    const NAME: &'static str = "fromsoft.shadow_flags";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_value(&self.0).map(Some)
    }
}

impl Component for Stored<msb::Region> {
    const NAME: &'static str = "fromsoft.region";
    const GEOMETRY: Geometry = Geometry::None;
    /// The stored place is the map's frame as stored: the entity's transform places the region in the world.
    fn apply_linear(&self, _l: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_value(&self.0).map(Some)
    }
}

impl Component for Stored<msb::Event> {
    const NAME: &'static str = "fromsoft.event";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_value(&self.0).map(Some)
    }
}

impl Component for Stored<msb::UnplacedPart> {
    const NAME: &'static str = "fromsoft.unplaced_part";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _l: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        to_value(&self.0).map(Some)
    }
}
