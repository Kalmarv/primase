//! What a source says about its scene's environment (`core.environment`, a world resource).

use serde::Serialize;
use serde_json::Value;

use crate::{Entity, JsonError, Light, LightKind, Resource, World};

/// The scene's environment as the source authored it. `sun`: the entity whose directional `core.light` the source
/// names as its sun (a scene setting naming one light, not the brightest light a writer picks); `None` when the source
/// names none, or in a part of a world that does not hold the sun (a subset or a cell). Ambient light, sky and fog are
/// not here until their meaning and units are established for a source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Environment {
    pub sun: Option<Entity>,
}

impl Resource for Environment {
    const NAME: &'static str = "core.environment";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// The sun is a directional light.
    fn verify(&self, w: &World) -> Result<(), String> {
        match self.sun {
            Some(e) if !matches!(w.get::<Light>(e), Some(l) if l.kind == LightKind::Directional) => {
                Err(format!("sun {e:?} is not a directional core.light"))
            }
            _ => Ok(()),
        }
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
        w.insert_resource(Environment { sun: Some(sun) });
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
        w.insert_resource(Environment { sun: Some(lamp) });
        assert_eq!(
            w.verify_resources(),
            [(
                "core.environment",
                format!("sun {lamp:?} is not a directional core.light")
            )]
        );
        assert_eq!(
            Environment { sun: Some(lamp) }.to_json().unwrap(),
            Some(serde_json::json!({"sun": lamp.index()}))
        );
    }
}
