//! A spot light's volume where the source's spot is a pyramid, not glTF's circular cone.
use serde::Serialize;

use crate::{Component, Entity, Geometry, JsonError, Light, LightKind, World};

/// A spot light whose volume is a rectangular pyramid with a near plane: from its apex it opens by
/// `horizontal_half_angle` about its local +X and `vertical_half_angle` about its local +Y (radians, from the beam),
/// and it lights only past `near` along the beam (metres), to its [`Light::range`]. Attached to the same entity as a
/// spot [`Light`], by a lowering whose source states such a spot (the source's angles and near plane, exactly). The
/// light's own cone ([`LightKind::Spot`]) is then the portable approximation the lowering states (glTF's circle has
/// one angle and no near plane); a consumer that does not read this component takes that cone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpotFrustum {
    pub horizontal_half_angle: f32,
    pub vertical_half_angle: f32,
    pub near: f32,
}

impl Component for SpotFrustum {
    const NAME: &'static str = "core.spot_frustum";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Half-angles above 0 and below pi / 2, a finite non-negative near plane, on a spot light whose range, where it
    /// states one, lies past the near plane.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let half = 0.0..std::f32::consts::FRAC_PI_2;
        for (axis, angle) in [
            ("horizontal", self.horizontal_half_angle),
            ("vertical", self.vertical_half_angle),
        ] {
            if !(half.contains(&angle) && angle > 0.0) {
                return Err(format!(
                    "a spot frustum's {axis} half-angle {angle} not in (0, pi / 2)"
                ));
            }
        }
        if !(self.near.is_finite() && self.near >= 0.0) {
            return Err(format!(
                "a spot frustum's near plane {} not finite and non-negative",
                self.near
            ));
        }
        match w.get::<Light>(e) {
            Some(Light {
                kind: LightKind::Spot { .. },
                range,
                ..
            }) => match range {
                Some(r) if *r <= self.near => Err(format!(
                    "a spot frustum's near plane {} not before the light's range {r}",
                    self.near
                )),
                _ => Ok(()),
            },
            _ => Err("a spot frustum on an entity with no spot light".into()),
        }
    }
}
