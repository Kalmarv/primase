//! A listed animation's source identity. Loading does not replace this descriptor.
use primase_core::{ClipKey, Component, Entity, Geometry, JsonError, Linear, World};
use serde::Serialize;
use serde_json::Value;

/// Transitional name-only listing for consumers migrating to plan entities. It holds
/// no decoder recipe and is not published by CharacterReader. Removed with that API
/// once consumers query Role::Clip asset members directly.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipListing {
    pub character: String,
    pub clips: Vec<ClipSource>,
    pub companions: Vec<String>,
}
impl primase_core::Resource for ClipListing {
    const NAME:&'static str="fromsoft.clip_listing";
    fn to_json(&self)->Result<Option<Value>,JsonError> {primase_core::json::to_value(self).map(Some)}
}

/// `fromsoft.clip_source` on each clip plan entity. Keys are opaque to consumers;
/// the reader validates them against the authored binder listing before loading.
/// BND headers do not state duration, so it normally remains unknown until HKX load.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClipSource {
    pub character: String,
    pub key: String,
    pub name: String,
    pub binder: String,
    pub member: String,
    /// Index in the binder's stored file table, distinguishing repeated member paths.
    pub member_index: usize,
    pub duration: Option<f64>,
}
impl Component for ClipSource {
    const NAME:&'static str="fromsoft.clip_source";
    const GEOMETRY:Geometry=Geometry::None;
    fn to_json(&self)->Result<Option<Value>,JsonError> {primase_core::json::to_value(self).map(Some)}
    fn verify(&self,entity:Entity,world:&World)->Result<(),String> {
        if [&self.character,&self.key,&self.name,&self.binder,&self.member].iter().any(|s|s.trim().is_empty()) {
            return Err("clip source has an empty identity field".into());
        }
        if world.get::<ClipKey>(entity).is_none_or(|key|key.0!=self.key) {
            return Err("clip source and core.clip_key differ".into());
        }
        if self.duration.is_some_and(|d|!d.is_finite() || d<0.) {
            return Err("clip source duration is not finite non-negative seconds".into());
        }
        Ok(())
    }
    fn apply_linear(&self,_linear:&Linear)->primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}
