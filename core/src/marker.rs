//! What a source places for players and tools rather than for drawing: points of interest (`core.marker`) and
//! polylines (`core.curve`). Any reader that reads such data gives them; what only its dialect knows (how a marker
//! snaps to the ground, a rail's grind surface) stays in its dialect's component on the same entity.

use serde::Serialize;
use serde_json::Value;

use crate::{Component, Entity, JsonError, Transform, World};

/// `core.marker`: an author-placed point of interest: a spawn, a fast-travel stop, a trainer, an effect emitter, a
/// waypoint. Where: the entity's [`Transform`] (position, and orientation when the source gives one: the marker faces
/// its local +Z). A source that leaves the height to be found (snapped to the ground by its consumer) gives no
/// transform but [`Marker::ground`]. Name: the entity's `core.name`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Marker {
    /// A lower-case word from the source's vocabulary (see [`kind_is_valid`]); writers that do not know a kind still
    /// show the marker as a point.
    pub kind: String,
    /// The ground position `(east, north)`, metres ([`crate::frame::ground`]), when the source states no height: the
    /// entity then has no transform.
    pub ground: Option<[f32; 2]>,
    /// A ground marker's facing, which no transform gives it: degrees about +Y, 0 facing +Z and 90 facing +X, as a
    /// marker with a transform faces its local +Z. `None`: the source states none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
}

/// `core.curve`: an author-placed curve in world space: a grind rail, a ledge, a coping, a path, a chairlift's cable.
/// Points in order, metres; `closed`: the last point joins the first. Without `tangents` it is the polyline through
/// them; with them each pair of consecutive points is a cubic Bezier segment from a point by its out handle and the
/// next point's in handle (Catmull-Rom, Hermite and auto tangents convert to these handles exactly). A consumer that
/// needs a polyline runs `flatten-curves`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Curve {
    /// As [`Marker::kind`].
    pub kind: String,
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    /// Each point's in and out handles, absolute positions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tangents: Option<Vec<[[f32; 3]; 2]>>,
    /// Each point's up direction (a rail's or a cable car's roll), unit length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up: Option<Vec<[f32; 3]>>,
}

/// A kind is a nonempty word of `a-z`, `0-9` and `_`. The known ones are listed in the format documentation; any
/// other is a source's own.
pub fn kind_is_valid(kind: &str) -> bool {
    !kind.is_empty()
        && kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn check_kind(kind: &str) -> Result<(), String> {
    if kind_is_valid(kind) {
        Ok(())
    } else {
        Err(format!("kind {kind:?} is not a word of a-z, 0-9 and _"))
    }
}

impl Component for Marker {
    const NAME: &'static str = "core.marker";
    /// 2: a ground marker's `yaw`.
    const VERSION: u32 = 2;
    /// Placed by its entity's transform, which the pass keeps rigid; a ground position is world space.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// A valid kind, and exactly one of a transform and a finite ground position.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        check_kind(&self.kind)?;
        match (self.ground, w.has::<Transform>(e)) {
            (Some(_), true) => Err("both a ground position and a transform".into()),
            (None, false) => Err("neither a ground position nor a transform".into()),
            (Some(g), false) if !g.iter().all(|v| v.is_finite()) => {
                Err(format!("ground position {g:?} is not finite"))
            }
            _ if self.yaw.is_some() && self.ground.is_none() => {
                Err("a yaw without a ground position (a transform gives the facing)".into())
            }
            _ if self.yaw.is_some_and(|y| !y.is_finite()) => Err("a yaw that is not finite".into()),
            _ => Ok(()),
        }
    }
}

impl Component for Curve {
    const NAME: &'static str = "core.curve";
    /// 2: optional Bezier `tangents` and `up` per point.
    const VERSION: u32 = 2;
    /// World space: not in any entity's local space.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// A valid kind and at least two finite points; handles and up directions, when given, one per point and finite
    /// (up directions of unit length within 1e-3).
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        check_kind(&self.kind)?;
        let n = self.points.len();
        if n < 2 {
            return Err(format!("{n} points; a curve has at least 2"));
        }
        if let Some(i) = self.points.iter().position(|p| !p.iter().all(|v| v.is_finite())) {
            return Err(format!("point {i} {:?} is not finite", self.points[i]));
        }
        if let Some(t) = &self.tangents {
            if t.len() != n {
                return Err(format!("{} tangent pairs for {n} points", t.len()));
            }
            if let Some(i) = t.iter().position(|h| !h.iter().flatten().all(|v| v.is_finite())) {
                return Err(format!("point {i}'s handles {:?} are not finite", t[i]));
            }
        }
        if let Some(up) = &self.up {
            if up.len() != n {
                return Err(format!("{} up directions for {n} points", up.len()));
            }
            let unit = |u: &[f32; 3]| {
                let l = u.iter().map(|v| v * v).sum::<f32>().sqrt();
                l.is_finite() && (l - 1.0).abs() <= 1e-3
            };
            if let Some(i) = up.iter().position(|u| !unit(u)) {
                return Err(format!("point {i}'s up {:?} is not a unit direction", up[i]));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_lower_case_words() {
        for good in ["spawn", "trigger_effect", "rail2"] {
            assert!(kind_is_valid(good), "{good}");
        }
        for bad in ["", "Spawn", "fast travel", "a:b", "é"] {
            assert!(!kind_is_valid(bad), "{bad}");
        }
    }

    #[test]
    fn a_marker_is_placed_by_a_transform_or_a_ground_position() {
        let mut w = World::new();
        let placed = w.spawn();
        w.insert(placed, Transform::IDENTITY);
        w.insert(
            placed,
            Marker {
                kind: "spawn".into(),
                ground: None,
                yaw: None,
            },
        );
        let snapped = w.spawn();
        w.insert(
            snapped,
            Marker {
                kind: "travel".into(),
                ground: Some([12., 40.]),
                yaw: None,
            },
        );
        assert!(w.verify_components().is_empty());
        let both = w.spawn();
        w.insert(both, Transform::IDENTITY);
        w.insert(
            both,
            Marker {
                kind: "travel".into(),
                ground: Some([0., 0.]),
                yaw: None,
            },
        );
        let neither = w.spawn();
        w.insert(
            neither,
            Marker {
                kind: "travel".into(),
                ground: None,
                yaw: None,
            },
        );
        let nan = w.spawn();
        w.insert(
            nan,
            Marker {
                kind: "travel".into(),
                ground: Some([f32::NAN, 0.]),
                yaw: None,
            },
        );
        let bad_kind = w.spawn();
        w.insert(
            bad_kind,
            Marker {
                kind: "Travel".into(),
                ground: Some([0., 0.]),
                yaw: None,
            },
        );
        let found: Vec<(Entity, String)> = w
            .verify_components()
            .into_iter()
            .map(|(_, e, m)| (e, m))
            .collect();
        assert_eq!(found.len(), 4, "{found:?}");
        assert_eq!(found[0], (both, "both a ground position and a transform".into()));
        assert_eq!(
            found[1],
            (neither, "neither a ground position nor a transform".into())
        );
        assert!(found[2].1.contains("not finite"), "{found:?}");
        assert!(found[3].1.contains("is not a word"), "{found:?}");
    }

    #[test]
    fn a_curve_has_two_finite_points() {
        let mut w = World::new();
        let curve = |points: Vec<[f32; 3]>| Curve {
            kind: "rail".into(),
            points,
            closed: false,
            tangents: None,
            up: None,
        };
        let good = w.spawn();
        w.insert(good, curve(vec![[0.; 3], [1., 0., 0.]]));
        let short = w.spawn();
        w.insert(short, curve(vec![[0.; 3]]));
        let nan = w.spawn();
        w.insert(nan, curve(vec![[0.; 3], [f32::INFINITY, 0., 0.]]));
        let found: Vec<Entity> = w.verify_components().into_iter().map(|(_, e, _)| e).collect();
        assert_eq!(found, [short, nan]);
        assert_eq!(
            w.get::<Curve>(good).unwrap().to_json().unwrap(),
            Some(
                serde_json::json!({"kind": "rail", "points": [[0., 0., 0.], [1., 0., 0.]], "closed": false})
            )
        );
    }
}
