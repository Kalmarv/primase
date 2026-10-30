//! Source-ordered clip events for display and typed consumers. Reading an event does not execute it.

use crate::world::Linearity;
use crate::{Clip, Component, Entity, Geometry, JsonError, Linear, NameOrigin, World};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `core.clip_events`, on a clip. Refusals name source records a lowering could not
/// interpret; the lowering must not emit a partial behavior timeline for those records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipEvents {
    pub origin: Option<String>,
    pub events: Vec<ClipEvent>,
    pub refusals: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipEvent {
    pub kind: i64,
    /// A name and its naming origin travel together, or both are absent.
    pub name: Option<String>,
    pub naming: Option<NameOrigin>,
    /// Seconds from the clip's start, finite. It may be below 0: source events legitimately begin before their clip
    /// does, and core neither clamps nor discards them; what that means for playback is each consumer's to read.
    pub start: f32,
    /// None runs to the clip's end. A finite explicit end may exceed clip duration;
    /// source events legitimately do so and core neither clamps nor discards them.
    pub end: Option<f32>,
    /// Source order, including duplicate names: consumers address fields by index.
    pub values: Vec<EventValue>,
    /// Stored bytes stay available even when typed fields were decoded.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventValue {
    /// May repeat within an event; labels do not replace source-field identity.
    pub name: String,
    pub value: EventValueData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EventValueData {
    I64(i64),
    F64(f64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
}

impl Component for ClipEvents {
    const NAME: &'static str = "core.clip_events";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        if !world.has::<Clip>(entity) {
            return Err("core.clip_events must be on a core.clip".into());
        }
        if self.origin.as_ref().is_some_and(|s| s.trim().is_empty()) {
            return Err("clip event origin is empty".into());
        }
        if self.refusals.iter().any(|s| s.trim().is_empty()) {
            return Err("clip event refusal is empty".into());
        }
        for (i, event) in self.events.iter().enumerate() {
            if !event.start.is_finite() || event.end.is_some_and(|end| !end.is_finite() || end < event.start)
            {
                return Err(format!(
                    "event {i}: start must be finite, end finite and at least start"
                ));
            }
            if event.name.is_some() != event.naming.is_some()
                || event.name.as_ref().is_some_and(|s| s.trim().is_empty())
            {
                return Err(format!(
                    "event {i}: name and naming origin must be present together and nonempty"
                ));
            }
            if let Some(naming) = &event.naming {
                naming.verify().map_err(|reason| format!("event {i}: {reason}"))?;
            }
            for (at, field) in event.values.iter().enumerate() {
                if field.name.trim().is_empty() {
                    return Err(format!("event {i} field {at}: name is empty"));
                }
                if matches!(field.value,EventValueData::F64(v) if !v.is_finite()) {
                    return Err(format!("event {i} field {at}: numeric value is not finite"));
                }
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

    fn fixture() -> (World, Entity, ClipEvents) {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Clip {
                name: "clip".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![],
                root_motion: None,
                additive: None,
            },
        );
        let events = ClipEvents {
            origin: Some("source event record".into()),
            refusals: vec!["unknown behavior kept raw".into()],
            events: vec![ClipEvent {
                kind: 193,
                name: Some("Opacity".into()),
                naming: Some(NameOrigin::Community {
                    credit: "template author".into(),
                }),
                start: 0.25,
                end: Some(2.0),
                bytes: vec![0, 1, 255],
                values: vec![
                    EventValue {
                        name: "value".into(),
                        value: EventValueData::I64(-2),
                    },
                    EventValue {
                        name: "value".into(),
                        value: EventValueData::F64(0.5),
                    },
                    EventValue {
                        name: "enabled".into(),
                        value: EventValueData::Bool(true),
                    },
                    EventValue {
                        name: "label".into(),
                        value: EventValueData::Text("source".into()),
                    },
                    EventValue {
                        name: "raw".into(),
                        value: EventValueData::Bytes(vec![255, 0]),
                    },
                ],
            }],
        };
        (world, entity, events)
    }

    #[test]
    fn all_typed_values_and_duplicate_labels_roundtrip_in_source_order() {
        let (mut world, entity, events) = fixture();
        assert!(
            events.verify(entity, &world).is_ok(),
            "explicit end may outlast the clip"
        );
        let value = events.to_json().unwrap().unwrap();
        let copy: ClipEvents = serde_json::from_value(value).unwrap();
        assert_eq!(copy, events);
        assert_eq!(copy.events[0].values[0].name, copy.events[0].values[1].name);
        world.insert(entity, events.clone());
        let subset = world.subset(&[entity].into());
        assert_eq!(subset.get::<ClipEvents>(entity), Some(&events));
        assert!(
            events.remap_primitives(&[2, 0, 2]).is_none(),
            "events are not mesh-primitive data"
        );
        assert!(matches!(
            events.apply_linear(&Linear::mirror_x()),
            Linearity::Invariant
        ));
    }

    #[test]
    fn a_start_before_the_clip_is_kept_as_stored() {
        let (world, entity, events) = fixture();
        for (start, end) in [(-1.0 / 30.0, Some(0.3)), (-0.5, None), (-0.25, Some(-0.25))] {
            let mut e = events.clone();
            e.events[0].start = start;
            e.events[0].end = end;
            assert!(e.verify(entity, &world).is_ok(), "{start}..{end:?}");
            let copy: ClipEvents = serde_json::from_value(e.to_json().unwrap().unwrap()).unwrap();
            assert_eq!(copy.events[0].start, start);
        }
    }

    #[test]
    fn malformed_times_names_and_nonfinite_fields_refuse() {
        let (world, entity, events) = fixture();
        for (start, end) in [
            (f32::NAN, None),
            (f32::NEG_INFINITY, Some(0.25)),
            (0.5, Some(0.25)),
            (-0.1, Some(-0.2)),
            (0.0, Some(f32::INFINITY)),
        ] {
            let mut e = events.clone();
            e.events[0].start = start;
            e.events[0].end = end;
            assert!(e.verify(entity, &world).is_err());
        }
        let mut e = events.clone();
        e.events[0].naming = None;
        assert!(e.verify(entity, &world).is_err());
        let mut e = events.clone();
        e.events[0].name = None;
        assert!(e.verify(entity, &world).is_err());
        let mut e = events.clone();
        e.events[0].naming = Some(NameOrigin::Community { credit: " ".into() });
        assert!(e.verify(entity, &world).is_err());
        let mut e = events.clone();
        e.refusals.push(String::new());
        assert!(e.verify(entity, &world).is_err());
        let mut e = events.clone();
        e.origin = Some(String::new());
        assert!(e.verify(entity, &world).is_err());
        let mut e = events.clone();
        e.events[0].values[0].value = EventValueData::F64(f64::NAN);
        assert!(e.verify(entity, &world).is_err());
        assert!(e.to_json().is_err());
        let mut e = events.clone();
        e.events[0].end = None;
        assert!(e.verify(entity, &world).is_ok());
        assert!(e.verify(Entity::from_index(99), &world).is_err());
        let mut value = events.to_json().unwrap().unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ClipEvents>(value).is_err());
    }
}
