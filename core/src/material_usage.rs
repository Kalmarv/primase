//! What a material's surface is, as the source states it (`core.material_usage`, on material entities).

use serde::Serialize;
use serde_json::Value;

use crate::{Component, JsonError};

/// `core.material_usage`: what the surfaces drawn with a material are, as the source states it (its shader family),
/// for consumers that treat them differently whatever their shading: a target's material class, a viewer's wind or
/// two-sided lighting. A reader's lowering sets it from the source through a named table and counts what it does not
/// map; it is never guessed from names. A material without it is ordinary. Further kinds are added with a source
/// that states them, as a new VERSION.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialUsage {
    /// Vegetation: leaves, needles, grass and the cards drawn for them.
    Foliage,
    /// A water surface.
    Water,
}

impl Component for MaterialUsage {
    const NAME: &'static str = "core.material_usage";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    #[test]
    fn a_usage_shows_by_its_name() {
        let mut w = World::new();
        let leaves = w.spawn();
        w.insert(leaves, MaterialUsage::Foliage);
        let sea = w.spawn();
        w.insert(sea, MaterialUsage::Water);
        assert_eq!(
            w.get::<MaterialUsage>(leaves).unwrap().to_json().unwrap(),
            Some(serde_json::json!("foliage"))
        );
        assert_eq!(
            w.get::<MaterialUsage>(sea).unwrap().to_json().unwrap(),
            Some(serde_json::json!("water"))
        );
    }
}
