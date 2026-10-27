//! Identity and an optional known length of a clip plan entity. No decoder recipe is World data.
use crate::world::Linearity;
use crate::{Component, Entity, Geometry, JsonError, Linear, World};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `core.clip_key`: the reader's opaque stable identity, on a listed or loaded clip.
/// A listed clip is a `core.asset` member with `Role::Clip` without `core.clip`.
/// Its source descriptor belongs to the reader; loading preserves this entity and key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipKey(pub String);

/// `core.clip_length`: seconds, present only when the listing knows the length.
/// Absence means unknown, not zero. Once loaded, `core.clip.duration` is authoritative.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipLength(pub f32);

impl Component for ClipKey {
    const NAME: &'static str = "core.clip_key";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _entity: Entity, _world: &World) -> Result<(), String> {
        if self.0.trim().is_empty() {
            return Err("clip key is empty".into());
        }
        Ok(())
    }
    fn apply_linear(&self, _linear: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
}

impl Component for ClipLength {
    const NAME: &'static str = "core.clip_length";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _entity: Entity, _world: &World) -> Result<(), String> {
        if !self.0.is_finite() || self.0 < 0.0 {
            return Err("clip length must be finite and non-negative seconds".into());
        }
        Ok(())
    }
    fn apply_linear(&self, _linear: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_identity_and_known_length_roundtrip_before_loading() {
        let mut world = World::new();
        let entity = world.spawn();
        let key = ClipKey(r#"{"member":"walk"}"#.into());
        let length = ClipLength(2.5);
        world.insert(entity, key.clone());
        world.insert(entity, length);
        assert!(world.verify_components().is_empty());
        assert_eq!(
            serde_json::from_value::<ClipKey>(key.to_json().unwrap().unwrap()).unwrap(),
            key
        );
        assert_eq!(
            serde_json::from_value::<ClipLength>(length.to_json().unwrap().unwrap()).unwrap(),
            length
        );
        let part = world.subset(&[entity].into());
        assert_eq!(part.get::<ClipKey>(entity), Some(&key));
        assert_eq!(part.get::<ClipLength>(entity), Some(&length));
        assert!(!world.has::<crate::Clip>(entity));
        assert!(key.remap_primitives(&[1, 0]).is_none());
        assert!(length.remap_primitives(&[1, 0]).is_none());
        assert!(matches!(
            key.apply_linear(&Linear::of(&crate::Transform::IDENTITY)),
            Linearity::Invariant
        ));
        assert!(matches!(
            length.apply_linear(&Linear::of(&crate::Transform::IDENTITY)),
            Linearity::Invariant
        ));
    }
    #[test]
    fn invalid_identity_and_length_refuse() {
        let mut world = World::new();
        let e = world.spawn();
        assert!(ClipKey(" ".into()).verify(e, &world).is_err());
        for seconds in [f32::NAN, f32::INFINITY, -0.01] {
            assert!(ClipLength(seconds).verify(e, &world).is_err());
        }
        assert!(ClipLength(0.0).verify(e, &world).is_ok());
    }
}
