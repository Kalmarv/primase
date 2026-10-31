//! Map reader-typed source events to core in identical order. This pass parses no bytes.
//!
//! An event core cannot carry (a time that is not finite, or an end before its start: the installs store 2 such events in
//! Sekiro and 13 in Elden Ring) is left out by name, and the clip's other events are lowered; how the game plays such an
//! event is not read. The refusals name source event indices; [`core_index`] gives the index an event has in
//! `core.clip_events`.
use crate::sekiro::character_data::ClipEvents as SourceEvents;
use fromsoft_formats::Game;
use fromsoft_formats::tae::Event;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Clip, ClipEvent, ClipEvents, Component, IncomingTransition, World};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerFromSoftClipEvents {
    pub game: Game,
}
pub(crate) fn game_option(g: &Given) -> Result<Game, String> {
    match g.required("game") {
        "sekiro" => Ok(Game::Sekiro),
        "elden" => Ok(Game::EldenRing),
        game => Err(format!("game={game}: expected sekiro or elden")),
    }
}
pub(crate) fn game_text(game: Game) -> &'static str {
    match game {
        Game::Sekiro => "sekiro",
        Game::EldenRing => "elden",
    }
}
/// Why a source event has no entry in `core.clip_events`, or `None` when it is lowered: core carries an interval whose
/// start is finite and whose end, where it has one, is finite and not before the start. A start below 0 is carried.
pub fn left_out(event: &Event) -> Option<&'static str> {
    if !event.start.is_finite() || !event.end.is_finite() {
        Some("a time that is not finite")
    } else if event.end < event.start {
        Some("it ends before it starts, and how the game plays that is not read")
    } else {
        None
    }
}

/// The index in the clip's `core.clip_events` of source event `index` of `events`, or `None` where it is left out
/// ([`left_out`]) or past the end.
pub fn core_index(events: &[Event], index: usize) -> Option<u32> {
    let event = events.get(index)?;
    if left_out(event).is_some() {
        return None;
    }
    let before = events[..index].iter().filter(|e| left_out(e).is_none()).count();
    u32::try_from(before).ok()
}

pub const LOWER_FROMSOFT_CLIP_EVENTS: PassSpec = PassSpec {
    name: "lower-fromsoft-clip-events",
    options: &[OptionSpec::required(
        "game",
        "sekiro",
        "source game: sekiro or elden; template decoding belongs to its reader",
    )],
    make: |g| {
        Ok(Box::new(LowerFromSoftClipEvents {
            game: game_option(g)?,
        }))
    },
};
impl Pass for LowerFromSoftClipEvents {
    fn name(&self) -> &'static str {
        "lower-fromsoft-clip-events"
    }
    fn options(&self) -> Options {
        Options::from([("game".into(), game_text(self.game).into())])
    }
    fn requires(&self) -> &[&'static str] {
        &[SourceEvents::NAME, Clip::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[ClipEvents::NAME, IncomingTransition::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut staged = Vec::new();
        for (entity, source) in world.iter::<SourceEvents>() {
            if !world.has::<Clip>(entity) {
                continue;
            }
            source.verify(entity, world)?;
            let mut lowered = ClipEvents {
                origin: Some(format!("{} animation {}", source.file, source.animation)),
                ..Default::default()
            };
            for (index, event) in source.events.iter().enumerate() {
                // a start below 0 is stored and kept as it is (core.clip_events carries it unchanged)
                if let Some(reason) = left_out(event) {
                    lowered.refusals.push(format!(
                        "source event {index} TAE {} at {}..{}: left out of core.clip_events, {reason}",
                        event.kind, event.start, event.end
                    ));
                    continue;
                }
                let typed = source.typed.get(index);
                if let Some(typed) = typed {
                    lowered.refusals.extend(
                        typed
                            .refusals
                            .iter()
                            .map(|reason| format!("event {index} TAE {}: {reason}", event.kind)),
                    );
                } else {
                    lowered.refusals.push(format!(
                        "event {index} TAE {}: typed fields were not read",
                        event.kind
                    ));
                }
                lowered.events.push(ClipEvent {
                    kind: i64::from(event.kind),
                    name: typed.and_then(|e| e.name.clone()),
                    naming: typed.and_then(|e| e.naming.clone()),
                    start: event.start,
                    end: (!event.open_ended()).then_some(event.end),
                    values: typed.map_or_else(Vec::new, |e| e.values.clone()),
                    bytes: event.parameters.clone(),
                });
            }
            staged.push((entity, lowered));
        }
        for (entity, events) in staged {
            for reason in &events.refusals {
                log.omit(reason, 1);
            }
            log.count("source-ordered core clip events", events.events.len() as u64);
            if self.game == Game::EldenRing {
                let timing = incoming_transition(&events.events);
                if let Some(reason) = &timing.refusal {
                    log.omit(reason, 1);
                }
                world.insert(entity, timing);
            }
            world.insert(entity, events);
        }
        Ok(())
    }
}

/// Resolve the established incoming-clip event rule while lowering source events, before a client sees core.
fn incoming_transition(events: &[ClipEvent]) -> IncomingTransition {
    const TRANSITION_EVENT: i32 = fromsoft_formats::tae::ELDEN_TRANSITION_END;
    let mut ends = Vec::new();
    for (i, event) in events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == i64::from(TRANSITION_EVENT))
    {
        let Some(end) = event.end else {
            return IncomingTransition {
                duration: None,
                refusal: Some(format!(
                    "event {i} (type {TRANSITION_EVENT}) runs to the clip's end: its stored end is not kept"
                )),
            };
        };
        ends.push(end);
    }
    let positive: Vec<f32> = ends.iter().copied().filter(|&e| e > 0.0).collect();
    match positive.as_slice() {
        [] => IncomingTransition::default(),
        [first, rest @ ..] if rest.iter().all(|e| e == first) => IncomingTransition {
            duration: Some(f64::from(*first)),
            refusal: None,
        },
        _ => IncomingTransition {
            duration: None,
            refusal: Some(format!(
                "the clip has {} type {TRANSITION_EVENT} events with different ends {positive:?}: which one the game keeps \
             is not traced",
                positive.len()
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sekiro::character_data::DecodedEvent;
    #[test]
    fn lowering_maps_reader_fields_without_interpreting_opaque_bytes() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            Clip {
                name: "clip".into(),
                duration: 1.,
                extrapolation: primase_core::Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let event = fromsoft_formats::tae::Event {
            kind: 198,
            unk04: 0,
            start: 0.,
            end: 2.,
            parameters: vec![0xFE],
        };
        let source = SourceEvents {
            file: "fixture.tae".into(),
            animation: 1,
            header: fromsoft_formats::tae::Header::None,
            file_name: None,
            authored_events: vec![event.clone()],
            events: vec![event],
            typed: vec![DecodedEvent {
                name: Some("Hand-authored fact".into()),
                naming: Some(primase_core::NameOrigin::Game),
                values: vec![primase_core::EventValue {
                    name: "material_ex.row_id".into(),
                    value: primase_core::EventValueData::I64(10300),
                }],
                refusals: vec!["envelope not established".into()],
            }],
        };
        world.insert(e, source.clone());
        LowerFromSoftClipEvents {
            game: Game::EldenRing,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        let lowered = world.get::<ClipEvents>(e).unwrap();
        assert_eq!(lowered.events[0].bytes, [0xFE]);
        assert_eq!(lowered.events[0].values, source.typed[0].values);
        assert_eq!(lowered.events[0].end, Some(2.));
        assert!(lowered.refusals[0].contains("envelope"));
        assert_eq!(world.get::<SourceEvents>(e), Some(&source));
    }

    #[test]
    fn an_event_that_begins_before_its_clip_lowers_with_its_stored_start() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            Clip {
                name: "clip".into(),
                duration: 1.,
                extrapolation: primase_core::Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let event = fromsoft_formats::tae::Event {
            kind: 16,
            unk04: 0,
            start: -1. / 30.,
            end: 0.3,
            parameters: vec![],
        };
        world.insert(
            e,
            SourceEvents {
                file: "fixture.tae".into(),
                animation: 4020,
                header: fromsoft_formats::tae::Header::None,
                file_name: None,
                authored_events: vec![event.clone()],
                events: vec![event],
                typed: vec![DecodedEvent::default()],
            },
        );
        LowerFromSoftClipEvents {
            game: Game::EldenRing,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        let lowered = world.get::<ClipEvents>(e).unwrap();
        assert_eq!(
            (lowered.events[0].start, lowered.events[0].end),
            (-1. / 30., Some(0.3))
        );
        assert!(lowered.verify(e, &world).is_ok());
    }
    /// An event that ends before it starts (c4310's `a000_008046` holds a type 129 at 0..-1/30) is left out by name; the
    /// clip's other events lower, and the ones after it move down one index in core, as `core_index` says.
    #[test]
    fn an_event_that_ends_before_it_starts_is_left_out_by_name_and_the_rest_lower() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            Clip {
                name: "clip".into(),
                duration: 1.,
                extrapolation: primase_core::Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let event = |kind, start, end| fromsoft_formats::tae::Event {
            kind,
            unk04: 0,
            start,
            end,
            parameters: vec![],
        };
        let events = vec![event(16, 0., 0.1), event(129, 0., -1. / 30.), event(96, 0.2, 0.3)];
        world.insert(
            e,
            SourceEvents {
                file: "c4310.tae".into(),
                animation: 8046,
                header: fromsoft_formats::tae::Header::None,
                file_name: None,
                authored_events: events.clone(),
                typed: vec![DecodedEvent::default(); 3],
                events: events.clone(),
            },
        );
        LowerFromSoftClipEvents {
            game: Game::EldenRing,
        }
        .run(&mut world, &mut PassLog::default())
        .unwrap();
        let lowered = world.get::<ClipEvents>(e).unwrap();
        assert_eq!(
            lowered.events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [16, 96]
        );
        assert!(lowered.verify(e, &world).is_ok());
        assert!(
            lowered
                .refusals
                .iter()
                .any(|r| r.starts_with("source event 1 TAE 129 at 0..-0.033")
                    && r.contains("ends before it starts")),
            "{:?}",
            lowered.refusals
        );
        assert_eq!(
            (0..4).map(|i| core_index(&events, i)).collect::<Vec<_>>(),
            [Some(0), None, Some(1), None]
        );
    }
    fn timing_event(kind: i64, start: f32, end: Option<f32>) -> ClipEvent {
        ClipEvent {
            kind,
            name: None,
            naming: None,
            start,
            end,
            values: Vec::new(),
            bytes: Vec::new(),
        }
    }

    #[test]
    fn an_effects_duration_is_the_incoming_clips_event_16_end_else_its_authored_value() {
        let made = |authored: f64, events: &[ClipEvent]| {
            incoming_transition(events).resolve(authored).map(|b| b.seconds())
        };
        // a000_004020: event 16 from -1/30 s to 0.3 s gives its end, not its length 1/3 (its other events around it)
        let fixture = [
            timing_event(0, 0.0, Some(1.0)),
            timing_event(16, -1.0 / 30.0, Some(0.3)),
            timing_event(717, 0.1, Some(0.2)),
        ];
        assert_eq!(made(0.5, &fixture), Ok(f64::from(0.3f32)));
        // as observed in the game: a standing jump's W_Jump_N (authored 0.5) into a010_202000 (event 16 0..0.2) took 0.2;
        // its landing (authored 0.2) into a010_202100 (0..0.1333) took 0.1333
        assert_eq!(
            made(0.5, &[timing_event(16, 0.0, Some(0.2))]),
            Ok(f64::from(0.2f32))
        );
        assert_eq!(
            made(0.2, &[timing_event(16, 0.0, Some(0.133_333_34))]),
            Ok(f64::from(0.133_333_34f32))
        );
        // no event 16, or one ending at 0: the authored value, not floored at 0.05
        assert_eq!(made(0.03, &[timing_event(717, 0.0, Some(0.1))]), Ok(0.03));
        assert_eq!(made(0.03, &[timing_event(16, 0.0, Some(0.0))]), Ok(0.03));
        assert_eq!(made(0.0, &[]), Ok(0.0));
        // two event 16s with one end are one value; with two ends which the game keeps is not traced
        assert_eq!(
            made(
                0.5,
                &[timing_event(16, 0.0, Some(0.2)), timing_event(16, 0.1, Some(0.2))]
            ),
            Ok(f64::from(0.2f32))
        );
        let refused =
            |authored: f64, events: &[ClipEvent]| incoming_transition(events).resolve(authored).unwrap_err();
        assert!(
            refused(
                0.5,
                &[timing_event(16, 0.0, Some(0.2)), timing_event(16, 0.0, Some(0.3))]
            )
            .contains("not traced")
        );
        assert!(refused(0.5, &[timing_event(16, 0.0, None)]).contains("end is not kept"));
        assert!(refused(-0.1, &[]).contains("not seconds"));
        assert!(refused(f64::NAN, &[]).contains("not seconds"));
        let blend = incoming_transition(&fixture).resolve(0.5).unwrap();
        assert_eq!(
            blend,
            primase_core::EffectBlend {
                authored: 0.5,
                event_end: Some(f64::from(0.3f32))
            }
        );
    }
}
