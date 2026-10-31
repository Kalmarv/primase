//! Source-stated incoming transition timing, independent of any event numbering or engine rule.
use crate::{Component, Entity, Geometry, JsonError, Linear, World};
use serde::{Deserialize, Serialize};

/// A source pass's timing override for transitions into a clip.
/// `None` retains the caller's authored duration; a refusal must never be treated as an absent override.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IncomingTransition {
    pub duration: Option<f64>,
    pub refusal: Option<String>,
}

/// A caller's authored duration and the optional source override that resolved it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectBlend {
    pub authored: f64,
    pub event_end: Option<f64>,
}
impl EffectBlend {
    pub fn seconds(&self) -> f64 {
        self.event_end.unwrap_or(self.authored)
    }
}
impl IncomingTransition {
    /// Resolve a stated duration using only this typed source result, never by interpreting clip events.
    pub fn resolve(&self, authored: f64) -> Result<EffectBlend, String> {
        if !authored.is_finite() || authored < 0.0 {
            return Err(format!(
                "the effect's authored duration {authored} is not seconds"
            ));
        }
        if let Some(reason) = &self.refusal {
            return Err(reason.clone());
        }
        Ok(EffectBlend {
            authored,
            event_end: self.duration,
        })
    }
}
impl Component for IncomingTransition {
    const NAME: &'static str = "core.incoming_transition";
    const GEOMETRY: Geometry = Geometry::None;
    fn verify(&self, _: Entity, _: &World) -> Result<(), String> {
        if self
            .duration
            .is_some_and(|value| !value.is_finite() || value < 0.)
        {
            return Err("incoming transition duration must be finite and non-negative".into());
        }
        if self
            .refusal
            .as_ref()
            .is_some_and(|reason| reason.trim().is_empty())
        {
            return Err("empty incoming transition refusal".into());
        }
        if self.duration.is_some() && self.refusal.is_some() {
            return Err("incoming transition cannot be both resolved and refused".into());
        }
        Ok(())
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn apply_linear(&self, _: &Linear) -> crate::Linearity<Self> {
        crate::Linearity::Invariant
    }
}
