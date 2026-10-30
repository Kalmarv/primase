//! Map reader-typed source events to core in identical order. This pass parses no bytes.
use crate::sekiro::character_data::ClipEvents as SourceEvents;
use fromsoft_formats::Game;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Clip, ClipEvent, ClipEvents, Component, World};
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
        &[ClipEvents::NAME]
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
                if !event.start.is_finite() || !event.end.is_finite() || event.end < event.start {
                    return Err(format!(
                        "{} event {index}: invalid source interval {}..{}",
                        source.file, event.start, event.end
                    ));
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
            world.insert(entity, events);
        }
        Ok(())
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
}
