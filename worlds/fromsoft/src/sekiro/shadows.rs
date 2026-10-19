//! Placement shadow controls. The raw MSB flags stay on the placement beside
//! their common meaning; point-light, static and cascade-specific renderer
//! controls remain source data only.
use crate::msb::ShadowFlags;
use primase_core::{Entity, ShadowCast, Shadows, World, pass::PassLog};

/// Attach a selected placement's source controls and their core meaning.
/// Explicit false stays false. A shadow-only flag with casting disabled is
/// contradictory: casting is left unknown and counted, never resolved.
pub(crate) fn attach(world: &mut World, placement: Entity, source: ShadowFlags, log: &mut PassLog) {
    let cast = match (source.cast, source.only) {
        (false, false) => Some(ShadowCast::Off),
        (true, false) => Some(ShadowCast::On),
        (true, true) => Some(ShadowCast::Only),
        (false, true) => {
            log.count("contradictory source cast/only controls", 1);
            None
        }
    };
    world.insert(placement, source);
    world.insert(
        placement,
        Shadows {
            cast,
            receive: Some(source.receive),
        },
    );
    log.count("placements with source shadow controls", 1);
    log.approximate("point-light, static and cascade shadow controls retained only", 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Instance, Mesh};

    #[test]
    fn preserves_explicit_false_and_raw_renderer_controls_without_guessing_contradictions() {
        let mut world = World::new();
        let prototype = world.spawn();
        world.insert(prototype, Mesh::default());
        let mut log = PassLog::default();
        let mut placed = Vec::new();
        for (cast, only, receive, expected) in [
            (false, false, false, Some(ShadowCast::Off)),
            (true, false, true, Some(ShadowCast::On)),
            (true, true, false, Some(ShadowCast::Only)),
            (false, true, true, None),
        ] {
            let e = world.spawn();
            world.insert(
                e,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            world.insert(e, primase_core::Transform::IDENTITY);
            let raw = ShadowFlags {
                cast,
                receive,
                only,
                point_light_cast: !receive,
                static_cast: 19,
                cascade3_cast: 241,
            };
            attach(&mut world, e, raw, &mut log);
            placed.push((e, raw, expected));
        }
        let untouched = world.spawn();
        world.insert(
            untouched,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(untouched, primase_core::Transform::IDENTITY);
        for (e, raw, expected) in placed {
            assert_eq!(world.get::<ShadowFlags>(e), Some(&raw));
            assert_eq!(
                world.get::<Shadows>(e),
                Some(&Shadows {
                    cast: expected,
                    receive: Some(raw.receive)
                })
            );
        }
        assert!(world.get::<Shadows>(untouched).is_none());
        assert!(world.get::<ShadowFlags>(untouched).is_none());
        assert_eq!(log.counts["contradictory source cast/only controls"], 1);
        assert_eq!(log.counts["placements with source shadow controls"], 4);
        assert_eq!(
            log.approximations["point-light, static and cascade shadow controls retained only"],
            4
        );
        assert!(world.verify_components().is_empty());
    }
}
