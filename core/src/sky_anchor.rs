//! Where a placement of the sky stands relative to the view (`core.sky_anchor`, on placement entities).

use serde::Serialize;
use serde_json::Value;

use crate::{Component, Entity, JsonError, Light, LightKind, Transform, World};

/// `core.sky_anchor`: a placement of the sky (its materials in [`crate::DrawLayer::Sky`]) that the source moves with
/// the view every frame, and how. Its `core.transform` is where it stands for a view at the world's origin; a consumer
/// computes, every frame and for every view (each eye of a stereo view), the transform it draws it with
/// ([`SkyAnchor::placed`]). A consumer that does not move it draws it where its transform puts it, which is right for a
/// view at the origin only.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkyAnchor {
    /// It follows the view across the ground and keeps its height and its rotation: its transform with the view's x
    /// and z added to the translation (a sky dome, the far mountains around it).
    Horizontal,
    /// It stands `distance` from the view in the direction towards the directional light `light`, facing the view
    /// (a moon or a sun's disc): `Transform::facing(view + distance * towards, -towards)`, so that its local +Z points
    /// towards the light and its local +Y is the world's up made perpendicular to that, where `towards` is the light's
    /// local +Z (the opposite of the direction it shines). Its own transform is not used.
    TowardsLight { light: Entity, distance: f32 },
}

impl SkyAnchor {
    /// The transform a placement with this anchor and its own `transform` is drawn with for a view at `view` (a
    /// world position), `light` the transform of the anchor's light ([`SkyAnchor::TowardsLight`]; unused for
    /// [`SkyAnchor::Horizontal`]). `None` when the light's transform is missing or its +Z is zero or not finite.
    pub fn placed(
        &self,
        transform: &Transform,
        view: [f32; 3],
        light: Option<&Transform>,
    ) -> Option<Transform> {
        match *self {
            SkyAnchor::Horizontal => {
                let mut t = *transform;
                t.cols[3][0] += view[0];
                t.cols[3][2] += view[2];
                Some(t)
            }
            SkyAnchor::TowardsLight { distance, .. } => {
                let z = light?.cols[2];
                let n = z.iter().map(|x| x * x).sum::<f32>().sqrt();
                if !(n > 0.0 && n.is_finite()) {
                    return None;
                }
                let towards = z.map(|x| x / n);
                let position = std::array::from_fn(|i| view[i] + distance * towards[i]);
                Transform::facing(position, towards.map(|x| -x))
            }
        }
    }
}

impl Component for SkyAnchor {
    const NAME: &'static str = "core.sky_anchor";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry: where the placement stands, in the world.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        if let SkyAnchor::TowardsLight { light, .. } = self {
            out.push(*light);
        }
    }
    /// A placement (it has a `core.transform`); a light anchor's light is a directional `core.light` with a
    /// transform, and its distance finite and above 0.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Transform>(e) {
            return Err("a sky anchor on an entity without a core.transform".into());
        }
        if let SkyAnchor::TowardsLight { light, distance } = *self {
            if !(distance.is_finite() && distance > 0.0) {
                return Err(format!(
                    "sky anchor distance {distance} is not finite and above 0"
                ));
            }
            if !matches!(w.get::<Light>(light), Some(l) if l.kind == LightKind::Directional) {
                return Err(format!(
                    "sky anchor light {light:?} is not a directional core.light"
                ));
            }
            if !w.has::<Transform>(light) {
                return Err(format!("sky anchor light {light:?} has no core.transform"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Transform, b: [[f32; 3]; 4]) -> bool {
        a.cols
            .iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < 1e-4)
    }

    /// A Sekiro frame (a recorded frame of Ashina castle at 18:00, the D3D11 recorder's constant buffers): the camera
    /// at (-203.3207, -26.2268, 178.8822); the sky dome's object matrix the identity rotation at (-203.3207, 0,
    /// 178.8822); the moon's at (-203.3206, -15.6614, 156.2245) with rows (-1, 0, 0), (0, 0.9063, 0.4226),
    /// (0, 0.4226, -0.9063), its light set's direction towards the light (0, 0.4226, -0.9063), 25 units away. The
    /// values are the game's own, in the game's frame.
    #[test]
    fn the_sky_of_a_recorded_frame_stands_where_the_game_drew_it() {
        let view = [-203.3207, -26.2268, 178.8822];
        let dome = SkyAnchor::Horizontal
            .placed(&Transform::IDENTITY, view, None)
            .unwrap();
        assert!(close(
            dome,
            [
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 1.],
                [-203.3207, 0., 178.8822]
            ]
        ));
        // a light shining along -towards: its +Z is towards
        let towards = [0.0, 0.422618, -0.9063079];
        let light = Transform::facing([0.; 3], towards.map(|x| -x)).unwrap();
        let mut w = World::new();
        let moon = SkyAnchor::TowardsLight {
            light: w.spawn(),
            distance: 25.0,
        }
        .placed(&Transform::IDENTITY, view, Some(&light))
        .unwrap();
        // columns: the images of local x, y, z (the game's rows, transposed), then the translation
        assert!(close(
            moon,
            [
                [-1., 0., 0.],
                [0., 0.9063079, 0.422618],
                [0., 0.422618, -0.9063079],
                [-203.3207, -15.66135, 156.2245]
            ]
        ));
    }

    #[test]
    fn a_light_anchor_needs_a_directional_light_and_a_distance() {
        let mut w = World::new();
        let sun = w.spawn();
        w.insert(sun, Transform::IDENTITY);
        w.insert(
            sun,
            Light {
                kind: LightKind::Directional,
                color: [1.; 3],
                intensity: 1.,
                range: None,
                cast_shadows: None,
                radius: None,
            },
        );
        let moon = w.spawn();
        w.insert(moon, Transform::IDENTITY);
        let ok = SkyAnchor::TowardsLight {
            light: sun,
            distance: 25.,
        };
        assert_eq!(ok.verify(moon, &w), Ok(()));
        let far = SkyAnchor::TowardsLight {
            light: sun,
            distance: 0.,
        };
        assert!(far.verify(moon, &w).unwrap_err().contains("distance"));
        let lamp = SkyAnchor::TowardsLight {
            light: moon,
            distance: 25.,
        };
        assert!(lamp.verify(moon, &w).unwrap_err().contains("not a directional"));
        let loose = w.spawn();
        assert!(
            SkyAnchor::Horizontal
                .verify(loose, &w)
                .unwrap_err()
                .contains("core.transform")
        );
        let mut refs = Vec::new();
        ok.references(&mut refs);
        assert_eq!(refs, vec![sun]);
        assert_eq!(
            ok.to_json().unwrap(),
            Some(serde_json::json!({"towards_light": {"light": sun, "distance": 25.0}}))
        );
        assert_eq!(
            SkyAnchor::Horizontal.to_json().unwrap(),
            Some(serde_json::json!("horizontal"))
        );
    }
}
