//! Which collision triangles are natural rock (`skate.rock`): scene-interchange v1's surface bit 7, beside the
//! entity's `core.collision` ([`crate::interchange::soup`] reads and writes it).

use primase_core::{Collision, Component, Entity, JsonError, World};
use serde_json::{Value, json};

/// Which triangles of the entity's `core.collision` are natural rock (v1's surface bit 7).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SkateRock(pub Vec<bool>);

impl Component for SkateRock {
    const NAME: &'static str = "skate.rock";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(
            json!({"triangles": self.0.len(), "rock": self.0.iter().filter(|&&r| r).count()}),
        ))
    }

    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        match w.get::<Collision>(e) {
            Some(c) if c.triangles.len() == self.0.len() => Ok(()),
            Some(c) => Err(format!(
                "{} rock flags for {} triangles",
                self.0.len(),
                c.triangles.len()
            )),
            None => Err("rock flags without core.collision".into()),
        }
    }
}
