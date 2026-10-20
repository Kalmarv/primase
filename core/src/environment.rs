//! What a source says about its scene's environment (`core.environment`, a world resource).

use serde::Serialize;
use serde_json::Value;

use crate::{Entity, JsonError, Light, LightKind, Resource, World};

/// The scene's environment as the source authored it. `sun`: the entity whose directional `core.light` the source
/// names as its sun (a scene setting naming one light, not the brightest light a writer picks); `None` when the source
/// names none, or in a part of a world that does not hold the sun (a subset or a cell). `ambient`: the scene's ambient
/// light ([`Ambient`]); `None` when the source states none or a lowering has not stated its meaning. Sky and fog are
/// not here until their meaning and units are established for a source.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Environment {
    pub sun: Option<Entity>,
    pub ambient: Option<Ambient>,
}

/// Ambient light: light every surface receives from no particular place, by the direction it faces alone, unshadowed.
/// Its colours are linear RGB, each scaled by `intensity`: a colour of 1 at intensity 1 lights a surface as a
/// directional `core.light` of intensity 1 (lux) shining straight onto it does. A source whose ambient is relative (to
/// its sun, say) states it at that scale and reports the approximation; this is not a measured illuminance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ambient {
    pub intensity: f32,
    pub light: AmbientLight,
}

/// How [`Ambient`]'s light varies with the direction a surface faces.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum AmbientLight {
    /// The same colour whatever a surface faces.
    Flat([f32; 3]),
    /// A gradient over the direction a surface faces: `sky` facing up (+Y), `equator` facing sideways, `ground` facing
    /// down.
    Gradient {
        sky: [f32; 3],
        equator: [f32; 3],
        ground: [f32; 3],
    },
}

impl AmbientLight {
    /// Its colours: the flat one, or the gradient's sky, equator and ground.
    pub fn colours(&self) -> Vec<[f32; 3]> {
        match self {
            AmbientLight::Flat(c) => vec![*c],
            AmbientLight::Gradient { sky, equator, ground } => vec![*sky, *equator, *ground],
        }
    }
}

impl Ambient {
    /// Finite and not negative, its intensity and every colour.
    fn verify(&self) -> Result<(), String> {
        let ok = |x: f32| x.is_finite() && x >= 0.0;
        if !ok(self.intensity) {
            return Err(format!(
                "ambient intensity {} is not finite and at least 0",
                self.intensity
            ));
        }
        match self
            .light
            .colours()
            .into_iter()
            .find(|c| !c.iter().all(|&x| ok(x)))
        {
            Some(c) => Err(format!("ambient colour {c:?} is not finite and at least 0")),
            None => Ok(()),
        }
    }
}

impl Resource for Environment {
    const NAME: &'static str = "core.environment";
    /// 2: `ambient`.
    const VERSION: u32 = 2;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// The sun is a directional light; the ambient is finite and not negative.
    fn verify(&self, w: &World) -> Result<(), String> {
        match self.sun {
            Some(e) if !matches!(w.get::<Light>(e), Some(l) if l.kind == LightKind::Directional) => {
                return Err(format!("sun {e:?} is not a directional core.light"));
            }
            _ => {}
        }
        self.ambient.as_ref().map_or(Ok(()), Ambient::verify)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.sun);
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        if self.sun.is_some_and(|e| !keep(e)) {
            self.sun = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;
    use std::collections::BTreeSet;

    /// A sun, a point light and a mesh-less placement-like entity; the environment names the sun.
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let sun = w.spawn();
        w.insert(
            sun,
            Light {
                kind: LightKind::Directional,
                color: [1.0, 0.95, 0.9],
                intensity: 3.0,
                range: None,
                cast_shadows: Some(true),
            },
        );
        w.insert(sun, Transform::IDENTITY);
        let lamp = w.spawn();
        w.insert(
            lamp,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 10.0,
                range: Some(5.0),
                cast_shadows: None,
            },
        );
        w.insert_resource(Environment {
            sun: Some(sun),
            ambient: None,
        });
        (w, sun, lamp)
    }

    fn sun_of(w: &World) -> Option<Entity> {
        w.resource::<Environment>().unwrap().sun
    }

    #[test]
    fn the_sun_survives_a_subset_that_keeps_it_and_is_forgotten_by_one_that_does_not() {
        let (w, sun, lamp) = world();
        assert!(w.verify_resources().is_empty());
        let mut out = Vec::new();
        Resource::references(w.resource::<Environment>().unwrap(), &mut out);
        assert_eq!(out, [sun]);
        let with = w.subset(&w.closure([sun]));
        assert_eq!(sun_of(&with), Some(sun));
        let without = w.subset(&BTreeSet::from([lamp]));
        assert_eq!(sun_of(&without), None, "no reference to an entity the part lacks");
        assert!(without.verify_resources().is_empty());
        assert_eq!(sun_of(&w), Some(sun), "the source world keeps its sun");
        // a split part as a subset; absorbing it back restores the world as it was
        let mut split = w.clone();
        let part = split.split_off(&BTreeSet::from([lamp]));
        assert_eq!((sun_of(&part), sun_of(&split)), (None, Some(sun)));
        let part = split.split_off(&BTreeSet::from([sun]));
        assert_eq!(sun_of(&part), Some(sun));
        split.absorb(part);
        assert!(split.verify_resources().is_empty());
    }

    #[test]
    fn despawning_or_dropping_the_sun_forgets_it() {
        let (mut w, sun, lamp) = world();
        w.retain(&BTreeSet::from([sun, lamp]));
        assert_eq!(sun_of(&w), Some(sun));
        w.retain(&BTreeSet::from([lamp]));
        assert_eq!(sun_of(&w), None);
        let (mut w, sun, _) = world();
        w.despawn(sun);
        assert_eq!(sun_of(&w), None);
        assert!(w.verify_resources().is_empty());
    }

    #[test]
    fn the_sun_must_be_a_directional_light() {
        let (mut w, _, lamp) = world();
        w.insert_resource(Environment {
            sun: Some(lamp),
            ambient: None,
        });
        assert_eq!(
            w.verify_resources(),
            [(
                "core.environment",
                format!("sun {lamp:?} is not a directional core.light")
            )]
        );
        assert_eq!(
            Environment {
                sun: Some(lamp),
                ambient: None
            }
            .to_json()
            .unwrap(),
            Some(serde_json::json!({"sun": lamp.index(), "ambient": null}))
        );
    }

    #[test]
    fn the_ambient_is_kept_by_every_part_and_must_be_finite_and_not_negative() {
        let (mut w, sun, lamp) = world();
        let gradient = Ambient {
            intensity: 1.0,
            light: AmbientLight::Gradient {
                sky: [0.3, 0.3, 0.4],
                equator: [0.5; 3],
                ground: [0.25; 3],
            },
        };
        w.insert_resource(Environment {
            sun: Some(sun),
            ambient: Some(gradient.clone()),
        });
        assert!(w.verify_resources().is_empty());
        // a scene-wide fact: a part without the sun keeps it
        let part = w.subset(&BTreeSet::from([lamp]));
        assert_eq!(
            part.resource::<Environment>().unwrap().ambient,
            Some(gradient.clone())
        );
        assert_eq!(gradient.light.colours(), [[0.3, 0.3, 0.4], [0.5; 3], [0.25; 3]]);
        assert_eq!(AmbientLight::Flat([0.2; 3]).colours(), [[0.2; 3]]);
        assert_eq!(
            w.resource::<Environment>().unwrap().to_json().unwrap(),
            Some(
                serde_json::json!({"sun": sun.index(), "ambient": {"intensity": 1.0, "light": {"Gradient": {
                "sky": [0.30000001192092896, 0.30000001192092896, 0.4000000059604645],
                "equator": [0.5, 0.5, 0.5], "ground": [0.25, 0.25, 0.25]}}}})
            )
        );
        for (intensity, colour, bad) in [
            (
                -1.0,
                [0.5; 3],
                "ambient intensity -1 is not finite and at least 0",
            ),
            (
                f32::NAN,
                [0.5; 3],
                "ambient intensity NaN is not finite and at least 0",
            ),
            (
                1.0,
                [0.5, -0.1, 0.5],
                "ambient colour [0.5, -0.1, 0.5] is not finite and at least 0",
            ),
            (
                1.0,
                [f32::INFINITY; 3],
                "ambient colour [inf, inf, inf] is not finite and at least 0",
            ),
        ] {
            w.insert_resource(Environment {
                sun: None,
                ambient: Some(Ambient {
                    intensity,
                    light: AmbientLight::Flat(colour),
                }),
            });
            assert_eq!(w.verify_resources(), [("core.environment", bad.to_owned())]);
        }
    }
}
