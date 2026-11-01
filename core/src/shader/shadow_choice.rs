//! Which local lights a consumer shadows: the choice as machinery with options the consumer states
//! ([`LocalShadowChoice`]), so that every consumer that draws a source's local shadow atlas chooses by the same rule and
//! reports what it chose. The rule is a stated choice, not the source's: no game's own selection of shadowed lights is
//! established.
use serde::{Deserialize, Serialize};

use super::{LightTableKind, ShadowedLight};
use crate::{Entity, Light, Transform, World};

/// The options of the choice: off (no light shadowed, every light lit unshadowed), or the lights of the shadowed table's
/// kind whose `core.light` says they cast shadows and that have a range, whose range sphere meets the view's frustum
/// and, where `max_distance` is set, comes within that many metres of the eye, taken in `order` up to the atlas's
/// capacity and, where it is set, `max_lights`. Each option changes what is drawn, so each is the consumer's to state:
/// there is no default, and deserialising requires every field, `null` for no limit included.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalShadowChoice {
    pub enabled: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub max_distance: Option<f32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub max_lights: Option<usize>,
    pub order: ShadowOrder,
}

/// Which of the lights in view a choice takes first when there are more than it shadows. No game's order is
/// established: a recorded frame shows the lights the game shadowed, not the ones it passed over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowOrder {
    /// Nearest the eye first: by the distance from the eye to the light's range sphere (0 with the eye inside it), then
    /// by the distance from the eye to its centre, then by entity.
    NearestEye,
}

/// The view a choice is made for, in world space: its view-projection (rows: `clip = m * [p, 1]`, depth 0 to 1 in
/// either direction) and its eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowView {
    pub view_projection: [[f32; 4]; 4],
    pub eye: [f32; 3],
}

/// What a choice chose for a view: the lights, in the choice's order, with their world centres, and how many it
/// considered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChosenShadows {
    pub lights: Vec<ShadowedLight>,
    /// The scene's lights of the kind that cast shadows and have a range.
    pub casters: usize,
    /// Of those, the ones in view (and within `max_distance`).
    pub in_view: usize,
}

/// The world's lights of `kind` that cast shadows and have a finite range above 0, with their world centre and range.
fn casters(world: &World, kind: LightTableKind) -> impl Iterator<Item = (Entity, [f32; 3], f32)> + '_ {
    world.iter::<Light>().filter_map(move |(e, l)| {
        let range = l.range.filter(|r| *r > 0.0 && r.is_finite())?;
        let t = world.get::<Transform>(e)?;
        (kind.holds(l) && l.cast_shadows == Some(true)).then_some((e, t.cols[3], range))
    })
}

impl LocalShadowChoice {
    /// The most lights this choice can shadow in `world` with an atlas of `capacity`: 0 when off, else the capacity,
    /// no more than the world's shadow-casting lights of `kind` with a range, nor `max_lights`.
    pub fn most(&self, world: &World, kind: LightTableKind, capacity: usize) -> usize {
        if !self.enabled {
            return 0;
        }
        capacity
            .min(casters(world, kind).count())
            .min(self.max_lights.unwrap_or(usize::MAX))
    }

    /// The choice in words, for a consumer's report.
    pub fn describe(&self, capacity: usize) -> String {
        if !self.enabled {
            return "local shadows: off; every shadow-casting light is lit without one".into();
        }
        format!(
            "local shadows: lights with cast_shadows and a range, their range sphere in the view's frustum{}, {}, up to {}",
            self.max_distance
                .map(|d| format!(" and within {d} m of the eye"))
                .unwrap_or_default(),
            match self.order {
                ShadowOrder::NearestEye => "nearest the eye first",
            },
            match self.max_lights {
                Some(n) if n < capacity => format!("{n} (the atlas holds {capacity})"),
                _ => format!("the atlas's capacity, {capacity}"),
            }
        )
    }

    /// The lights of `kind` in `world` this choice shadows for `view`, at most `capacity`, in the choice's `order`.
    pub fn choose(
        &self,
        world: &World,
        kind: LightTableKind,
        view: &ShadowView,
        capacity: usize,
    ) -> ChosenShadows {
        let mut chosen = ChosenShadows::default();
        if !self.enabled {
            return chosen;
        }
        let planes = frustum(&view.view_projection);
        let mut found = Vec::new();
        for (light, centre, range) in casters(world, kind) {
            chosen.casters += 1;
            let to_centre = distance(centre, view.eye);
            let gap = (to_centre - range).max(0.0);
            if !meets(&planes, centre, range) || self.max_distance.is_some_and(|d| gap > d) {
                continue;
            }
            chosen.in_view += 1;
            found.push((gap, to_centre, light, centre, range));
        }
        match self.order {
            ShadowOrder::NearestEye => {
                found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
            }
        }
        let limit = self.max_lights.map_or(capacity, |n| n.min(capacity));
        chosen.lights = found
            .into_iter()
            .take(limit)
            .map(|(_, _, light, centre, range)| ShadowedLight { light, centre, range })
            .collect();
        chosen
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The six planes of a view-projection's frustum (Gribb and Hartmann, from its rows, depth 0 to 1), each normalised:
/// a point `p` is inside a plane where `n . p + d >= 0`.
fn frustum(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let row = |r: usize| m[r];
    let add = |a: [f32; 4], b: [f32; 4], s: f32| std::array::from_fn::<f32, 4, _>(|k| a[k] + s * b[k]);
    let (x, y, z, w) = (row(0), row(1), row(2), row(3));
    [
        add(w, x, 1.0),
        add(w, x, -1.0),
        add(w, y, 1.0),
        add(w, y, -1.0),
        z,
        add(w, z, -1.0),
    ]
    .map(|p| {
        let length = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        if length > 0.0 { p.map(|v| v / length) } else { p }
    })
}

/// Whether the sphere of `centre` and `radius` may meet the frustum: false only when it lies wholly outside one plane.
fn meets(planes: &[[f32; 4]; 6], centre: [f32; 3], radius: f32) -> bool {
    planes
        .iter()
        .all(|p| p[0] * centre[0] + p[1] * centre[1] + p[2] * centre[2] + p[3] >= -radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LightKind;

    /// Shadows on, with no distance or count limit, nearest the eye first.
    const ALL: LocalShadowChoice = LocalShadowChoice {
        enabled: true,
        max_distance: None,
        max_lights: None,
        order: ShadowOrder::NearestEye,
    };

    /// Every option is stated: a choice that leaves one out, `null` limits included, does not deserialise.
    #[test]
    fn every_option_is_required() -> Result<(), serde_json::Error> {
        let all: LocalShadowChoice = serde_json::from_str(
            r#"{"enabled":true,"max_distance":null,"max_lights":null,"order":"nearest_eye"}"#,
        )?;
        assert_eq!(all, ALL);
        let limited: LocalShadowChoice = serde_json::from_str(
            r#"{"enabled":true,"max_distance":20.0,"max_lights":4,"order":"nearest_eye"}"#,
        )?;
        assert_eq!((limited.max_distance, limited.max_lights), (Some(20.0), Some(4)));
        for partial in [
            r#"{"max_distance":null,"max_lights":null,"order":"nearest_eye"}"#,
            r#"{"enabled":true,"max_lights":null,"order":"nearest_eye"}"#,
            r#"{"enabled":true,"max_distance":null,"order":"nearest_eye"}"#,
            r#"{"enabled":true,"max_distance":null,"max_lights":null}"#,
            r#"{"enabled":true,"max_distance":null,"max_lights":null,"order":"brightest"}"#,
            r#"{}"#,
        ] {
            assert!(
                serde_json::from_str::<LocalShadowChoice>(partial).is_err(),
                "{partial}"
            );
        }
        Ok(())
    }

    /// A world of point lights at `z` metres down -z, each of range 2 and `cast_shadows` as given.
    fn world(lights: &[(f32, Option<bool>)]) -> (World, Vec<Entity>) {
        let mut w = World::new();
        let mut out = Vec::new();
        for &(z, casts) in lights {
            let e = w.spawn();
            let mut t = Transform::IDENTITY;
            t.cols[3] = [0.0, 0.0, -z];
            w.insert(e, t);
            w.insert(
                e,
                Light {
                    kind: LightKind::Point,
                    color: [1.0; 3],
                    intensity: 1.0,
                    range: Some(2.0),
                    cast_shadows: casts,
                    radius: None,
                    specular: None,
                },
            );
            out.push(e);
        }
        (w, out)
    }

    /// A camera at the origin looking down -z, 90 degrees square, 0.1 to 100 m, depth 0 to 1, rows.
    fn view() -> ShadowView {
        let (n, f) = (0.1f32, 100.0f32);
        ShadowView {
            view_projection: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, f / (n - f), n * f / (n - f)],
                [0.0, 0.0, -1.0, 0.0],
            ],
            eye: [0.0; 3],
        }
    }

    /// Only lights that cast shadows and are in view are chosen, nearest the eye first, and no more than asked.
    #[test]
    fn the_choice_takes_the_nearest_shadow_casting_lights_in_view() {
        // a caster at 30 m, one behind the eye, one that casts none, one of unknown casting, casters at 10 and 20 m
        let (w, e) = world(&[
            (30.0, Some(true)),
            (-10.0, Some(true)),
            (5.0, Some(false)),
            (6.0, None),
            (10.0, Some(true)),
            (20.0, Some(true)),
        ]);
        let choice = ALL;
        let chosen = choice.choose(&w, LightTableKind::Point, &view(), 8);
        assert_eq!((chosen.casters, chosen.in_view), (4, 3));
        assert_eq!(
            chosen.lights.iter().map(|l| l.light).collect::<Vec<_>>(),
            [e[4], e[5], e[0]]
        );
        assert_eq!(chosen.lights[0].range, 2.0);
        assert_eq!(chosen.lights[0].centre, [0.0, 0.0, -10.0]);
        assert_eq!(
            choice.choose(&w, LightTableKind::Point, &view(), 2).lights.len(),
            2
        );
        let near = LocalShadowChoice {
            max_distance: Some(20.0),
            ..choice
        };
        assert_eq!(near.choose(&w, LightTableKind::Point, &view(), 8).lights.len(), 2);
        let one = LocalShadowChoice {
            max_lights: Some(1),
            ..choice
        };
        assert_eq!(one.choose(&w, LightTableKind::Point, &view(), 8).lights.len(), 1);
        let off = LocalShadowChoice {
            enabled: false,
            ..choice
        };
        assert!(
            off.choose(&w, LightTableKind::Point, &view(), 8)
                .lights
                .is_empty()
        );
        assert!(
            choice
                .choose(&w, LightTableKind::Spot, &view(), 8)
                .lights
                .is_empty()
        );
    }

    /// A light whose range sphere reaches into the frustum is in view though its centre is not.
    #[test]
    fn a_light_whose_range_reaches_the_view_is_in_view() {
        let (mut w, e) = world(&[(1.0, Some(true))]);
        // beside the 90-degree view at 1 m deep: its centre at x = 2.5 lies 1.06 m outside the edge plane, within its
        // range of 2; at x = 5, 2.83 m outside, beyond it
        let mut t = Transform::IDENTITY;
        t.cols[3] = [2.5, 0.0, -1.0];
        w.insert(e[0], t);
        assert_eq!(ALL.choose(&w, LightTableKind::Point, &view(), 8).in_view, 1);
        t.cols[3] = [5.0, 0.0, -1.0];
        w.insert(e[0], t);
        assert_eq!(ALL.choose(&w, LightTableKind::Point, &view(), 8).in_view, 0);
    }

    /// The most a choice can shadow: the atlas's capacity, no more than the scene's casters or the stated limit; none
    /// when off.
    #[test]
    fn the_most_a_choice_shadows_is_bounded_by_the_atlas_the_casters_and_the_limit() {
        let (w, _) = world(&[(10.0, Some(true)), (20.0, Some(true)), (30.0, Some(false))]);
        let choice = ALL;
        assert_eq!(choice.most(&w, LightTableKind::Point, 170), 2);
        assert_eq!(choice.most(&w, LightTableKind::Point, 1), 1);
        assert_eq!(
            LocalShadowChoice {
                max_lights: Some(1),
                ..choice
            }
            .most(&w, LightTableKind::Point, 170),
            1
        );
        assert_eq!(
            LocalShadowChoice {
                enabled: false,
                ..choice
            }
            .most(&w, LightTableKind::Point, 170),
            0
        );
    }
}
