//! `core.clip_effects`: the particle systems a clip starts while it plays, and where and when. A clip's events say that
//! something happens at a time ([`ClipEvents`](crate::ClipEvents)); a source's lowering that knows an event starts an effect
//! says it here, in terms a player can follow without knowing the source: which system, from when, standing where.

use crate::particles::ParticleSystem;
use crate::world::Linearity;
use crate::{Clip, Component, Entity, Geometry, JsonError, Linear, Transform, World};
use serde::Serialize;
use serde_json::Value;

/// `core.clip_effects`, on a clip.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipEffects {
    /// Where the record was made from (a provenance string), if the lowering states it.
    pub origin: Option<String>,
    /// The systems the clip starts, in source order.
    pub spawns: Vec<ClipSpawn>,
    /// What the lowering could not carry, by reason, with how many: accounting, never state.
    pub lost: Vec<(String, u32)>,
}

/// One system a clip starts.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClipSpawn {
    /// The system: an entity holding a [`ParticleSystem`] and no [`Transform`] (it is placed by this record).
    pub system: Entity,
    /// The clip time it starts at, seconds.
    pub start: f32,
    /// The clip time it is stopped at, if the source stops it; `None` lets it play out.
    pub end: Option<f32>,
    /// The name of the socket of the clip's rig the system stands at (a socket's name is the source's: provenance, but the one
    /// handle a clip has on a rig's sockets); `None` stands it at the rig's origin. A name the rig's skeleton does not hold
    /// is looked for on the rigs attached to it (`core.attachment`: what the rig holds, a weapon in a hand), the attached
    /// rig standing where its attachment places it and in its own pose.
    pub socket: Option<String>,
    /// A second socket of the rig, for a system that runs between two (a weapon's trail from the hilt to the tip): the
    /// system stands midway between `socket` and this one, in `socket`'s frame. `None` stands it at `socket`.
    pub toward: Option<String>,
    /// Whether the system follows its socket as the clip moves it, else it stays where it started.
    pub follow: bool,
    /// The index in the clip's `core.clip_events` of the event that starts it, where the source says.
    pub event: Option<u32>,
}

impl Component for ClipEffects {
    const NAME: &'static str = "core.clip_effects";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.spawns.iter().map(|spawn| spawn.system));
    }
    /// On a clip; each spawn names a system that is not placed, a finite start that is not negative, an end that is not before
    /// it and a socket name that is not empty.
    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        if !world.has::<Clip>(entity) {
            return Err("core.clip_effects must be on a core.clip".into());
        }
        if self
            .origin
            .as_ref()
            .is_some_and(|origin| origin.trim().is_empty())
        {
            return Err("clip effects origin is empty".into());
        }
        if self.lost.iter().any(|(reason, _)| reason.trim().is_empty()) {
            return Err("a lost count without a reason".into());
        }
        for (i, spawn) in self.spawns.iter().enumerate() {
            if !world.has::<ParticleSystem>(spawn.system) {
                return Err(format!("spawn {i}: {:?} is not a particle system", spawn.system));
            }
            if world.has::<Transform>(spawn.system) {
                return Err(format!(
                    "spawn {i}: the system {:?} is placed; a clip places the systems it starts",
                    spawn.system
                ));
            }
            if !spawn.start.is_finite() || spawn.start < 0.0 {
                return Err(format!("spawn {i}: a start of {} seconds", spawn.start));
            }
            if spawn.end.is_some_and(|end| !end.is_finite() || end < spawn.start) {
                return Err(format!("spawn {i}: an end before its start or not finite"));
            }
            if spawn
                .socket
                .as_ref()
                .is_some_and(|socket| socket.trim().is_empty())
            {
                return Err(format!("spawn {i}: a socket name that is empty"));
            }
            if spawn.toward.is_some() && spawn.socket.is_none() {
                return Err(format!("spawn {i}: a second socket with no first"));
            }
            if spawn.toward.as_ref().is_some_and(|t| t.trim().is_empty()) {
                return Err(format!("spawn {i}: a second socket name that is empty"));
            }
        }
        Ok(())
    }
    fn apply_linear(&self, _linear: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Extrapolation;
    use crate::particles::Termination;

    fn system() -> ParticleSystem {
        ParticleSystem {
            seed: 0,
            emitters: Vec::new(),
            inputs: Vec::new(),
            states: 1,
            warmup: 0.0,
            time_scale: None,
            max_view_distance: None,
            termination: Termination::Natural,
            end: None,
            lost: Vec::new(),
        }
    }

    fn fixture() -> (World, Entity, Entity, ClipEffects) {
        let mut world = World::new();
        let clip = world.spawn();
        world.insert(
            clip,
            Clip {
                name: "clip".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let spark = world.spawn();
        world.insert(spark, system());
        let effects = ClipEffects {
            origin: Some("source event record".into()),
            spawns: vec![ClipSpawn {
                system: spark,
                start: 0.25,
                end: Some(0.5),
                socket: Some("dummy 100".into()),
                toward: None,
                follow: true,
                event: Some(3),
            }],
            lost: vec![("an effect named through a param row".into(), 2)],
        };
        (world, clip, spark, effects)
    }

    #[test]
    fn a_spawn_of_an_unplaced_system_on_a_clip_verifies_and_names_its_system() {
        let (world, clip, spark, effects) = fixture();
        assert_eq!(effects.verify(clip, &world), Ok(()));
        let mut referenced = Vec::new();
        effects.references(&mut referenced);
        assert_eq!(referenced, [spark]);
        let json = effects.to_json().unwrap().unwrap();
        assert_eq!(json["spawns"][0]["socket"], "dummy 100");
        assert_eq!(json["lost"][0][1], 2);
    }

    #[test]
    fn what_is_not_a_clip_a_system_or_a_time_is_refused() {
        let (mut world, clip, spark, effects) = fixture();
        // not on a clip
        assert!(effects.verify(spark, &world).is_err());
        // a spawn of something that is not a system, and of a system that is placed
        let mut not_a_system = effects.clone();
        not_a_system.spawns[0].system = clip;
        assert!(not_a_system.verify(clip, &world).is_err());
        world.insert(spark, Transform::IDENTITY);
        assert!(effects.verify(clip, &world).is_err());
        world.remove::<Transform>(spark);
        for (start, end) in [
            (f32::NAN, None),
            (-0.5, None),
            (0.5, Some(0.25)),
            (0.0, Some(f32::INFINITY)),
        ] {
            let mut bad = effects.clone();
            bad.spawns[0].start = start;
            bad.spawns[0].end = end;
            assert!(bad.verify(clip, &world).is_err(), "{start} {end:?}");
        }
        let mut bad = effects.clone();
        bad.spawns[0].socket = Some(" ".into());
        assert!(bad.verify(clip, &world).is_err());
        // a second socket with no first, or an empty one
        let mut bad = effects.clone();
        bad.spawns[0].toward = Some("dummy 101".into());
        assert_eq!(bad.verify(clip, &world), Ok(()));
        bad.spawns[0].socket = None;
        assert!(bad.verify(clip, &world).is_err());
        bad.spawns[0].socket = Some("dummy 100".into());
        bad.spawns[0].toward = Some(String::new());
        assert!(bad.verify(clip, &world).is_err());
        let mut bad = effects;
        bad.lost.push((String::new(), 1));
        assert!(bad.verify(clip, &world).is_err());
    }
}
