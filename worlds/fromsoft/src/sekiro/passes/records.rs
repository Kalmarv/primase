//! `lower-fromsoft-map-records{leave-out=...}`: the map layout's regions and events the reader kept
//! (`fromsoft.map_records`) as entities a viewer lists and places. Each region of a type but the spawn point (the extras'
//! travel markers) is a `core.marker` whose kind is the word for its type (`sound`, `sfx`, `environment_map_point`,
//! `weather_override` ..., `type_<n>` for a code SoulsFormats does not name) at the region's stored place, with the stored
//! record (`fromsoft.region`: type, shape and its dimensions, entity ID, the type's data as hexadecimal bytes) on the
//! same entity. Each part of a kind the area read does not place as a model (an enemy, a player start, a dummy object,
//! asset or enemy, a connect collision) is a `core.marker` of the part's kind word (`enemy`, `player_start`, ...) at its
//! stored place with its record (`fromsoft.unplaced_part`). Each event is an entity of its own named as stored with its
//! record (`fromsoft.event`: type, event ID, the part and region it names, entity ID, the type's data as bytes).
//! Nothing in a type's data is decoded. `leave-out` takes region type words and part kind words.
use crate::msb::{Event, Region, UnplacedPart, euler_transform};
use crate::sekiro::source::{MapRecords, ReadScope};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Marker, Name, Record, Resource, Source, Transform, World};
use std::collections::BTreeSet;

/// `lower-fromsoft-map-records`: the choice of region kinds to leave out is the program's, stated (empty for none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerFromSoftMapRecords {
    pub leave_out: BTreeSet<String>,
}

/// The pass's declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LOWER_FROMSOFT_MAP_RECORDS: PassSpec = PassSpec {
    name: "lower-fromsoft-map-records",
    options: &[OptionSpec::required(
        "leave-out",
        "",
        "region kinds (the words of the region types: sound, sfx, wind_area, ...) not lowered, joined by + (empty for none)",
    )],
    make: |g| Ok(Box::new(LowerFromSoftMapRecords::from_given(g)?)),
};

impl LowerFromSoftMapRecords {
    fn from_given(g: &Given) -> std::result::Result<Self, String> {
        let mut leave_out = BTreeSet::new();
        for word in g.required("leave-out").split('+').filter(|w| !w.is_empty()) {
            if !primase_core::kind_is_valid(word) {
                return Err(format!("leave-out: {word:?} is not a region kind (a word of a-z, 0-9 and _)"));
            }
            leave_out.insert(word.to_owned());
        }
        Ok(Self { leave_out })
    }
}

/// The word for a region's kind: the type's name, else `type_<code>`.
fn kind(region: &Region) -> String {
    region
        .type_name
        .map_or_else(|| format!("type_{}", region.type_code), str::to_owned)
}

impl Pass for LowerFromSoftMapRecords {
    fn name(&self) -> &'static str {
        "lower-fromsoft-map-records"
    }
    fn options(&self) -> Options {
        Options::from([(
            "leave-out".to_owned(),
            self.leave_out.iter().cloned().collect::<Vec<_>>().join("+"),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[MapRecords::NAME]
    }
    /// A world read without the records (`map_records` off) has none: the step is recorded as skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Marker::NAME,
            Transform::NAME,
            Name::NAME,
            Source::NAME,
            Region::NAME,
            Event::NAME,
            UnplacedPart::NAME,
            Record::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[MapRecords::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a region's rotation read as a part's (degrees about X, then Z, then Y), its shape not drawn: the marker is the region's place; an event has no place"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let records = world.remove_resource::<MapRecords>().ok_or("no fromsoft.map_records")?;
        let area = world.resource::<ReadScope>().map(|s| s.area.clone());
        let label = |name: &str| match &area {
            Some(area) => format!("{area}:{name}"),
            None => name.to_owned(),
        };
        for region in records.regions {
            let word = kind(&region);
            if Some(region.type_code) == records.spawn_region {
                log.count("spawn regions left to lower-fromsoft-extras", 1);
                continue;
            }
            if self.leave_out.contains(&word) {
                log.count(&format!("{word} regions left out"), 1);
                continue;
            }
            let transform = euler_transform(region.position, region.rotation_degrees, [1.; 3], region.index)
                .map_err(|e| e.to_string())?;
            let e = world.spawn();
            world.insert(
                e,
                Marker {
                    kind: word.clone(),
                    ground: None,
                    yaw: None,
                },
            );
            world.insert(e, transform);
            world.insert(e, Name(region.name.clone()));
            world.insert(e, Source(label(&region.name)));
            world.insert(e, Record {category:"region".into(),kind:word.clone()});
            world.insert(e, region);
            log.place(e);
            log.count(&format!("{word} regions"), 1);
            log.approximate(
                "region rotations read as a part's Euler angles; shapes not drawn",
                1,
            );
        }
        for part in records.parts {
            if self.leave_out.contains(part.kind) {
                log.count(&format!("{} parts left out", part.kind), 1);
                continue;
            }
            let transform = euler_transform(part.position, part.rotation_degrees, [1.; 3], part.index)
                .map_err(|e| e.to_string())?;
            let e = world.spawn();
            world.insert(
                e,
                Marker {
                    kind: part.kind.to_owned(),
                    ground: None,
                    yaw: None,
                },
            );
            world.insert(e, transform);
            world.insert(e, Name(part.name.clone()));
            world.insert(e, Source(label(&part.name)));
            world.insert(e, Record {category:"part".into(),kind:part.kind.into()});
            log.count(&format!("{} parts", part.kind), 1);
            world.insert(e, part);
            log.place(e);
        }
        for event in records.events {
            let e = world.spawn();
            let word = event
                .type_name
                .map_or_else(|| format!("type_{}", event.type_code), str::to_owned);
            world.insert(e, Name(event.name.clone()));
            world.insert(e, Source(label(&event.name)));
            world.insert(e, Record {category:"event".into(),kind:word.clone()});
            world.insert(e, event);
            log.place(e);
            log.count(&format!("{word} events"), 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msb::RegionShape;

    fn region(index: usize, name: &str, code: u32, name_word: Option<&'static str>) -> Region {
        Region {
            index,
            name: name.to_owned(),
            type_code: code,
            type_name: name_word,
            shape: RegionShape::Sphere { radius: 2. },
            position: [1., 2., 3.],
            rotation_degrees: [0., 90., 0.],
            entity_id: 7,
            type_data: vec![1, 2],
        }
    }

    fn world() -> World {
        let mut w = World::default();
        w.insert_resource(MapRecords {
            regions: vec![
                region(0, "Spawn", 8, Some("spawn_point")),
                region(1, "Birds", 4, Some("sound")),
                region(2, "Gust", 6, Some("wind_sfx")),
                region(3, "Odd", 77, None),
            ],
            events: vec![Event {
                index: 0,
                name: "Chest".to_owned(),
                event_id: 5,
                type_code: 4,
                type_name: Some("treasure"),
                part: Some("o1".to_owned()),
                region: None,
                entity_id: 9,
                type_data: vec![3],
            }],
            parts: vec![UnplacedPart {
                index: 4,
                name: "c1000_0000".to_owned(),
                kind: "enemy",
                model: Some("c1000".to_owned()),
                npc_param: Some(100),
                position: [5., 6., 7.],
                rotation_degrees: [0.; 3],
                scale: [1.; 3],
                entity_id: 3,
                entity_group_ids: [0; 8],
            }],
            spawn_region: Some(8),
        });
        w
    }

    fn pass(leave_out: &str) -> Box<dyn Pass> {
        LOWER_FROMSOFT_MAP_RECORDS
            .instance(&Options::from([("leave-out".to_owned(), leave_out.to_owned())]))
            .unwrap()
    }

    /// Regions become markers of their type's word at their place with their record, the spawn regions are left to the
    /// extras, an unnamed type is `type_<n>`, events are named entities with their record and no place.
    #[test]
    fn regions_become_markers_events_become_named_records_and_spawns_are_left_to_the_extras() {
        let mut w = world();
        let mut log = PassLog::default();
        pass("").run(&mut w, &mut log).unwrap();
        assert!(w.resource::<MapRecords>().is_none());
        let kinds: Vec<_> = w.iter::<Marker>().map(|(_, m)| m.kind.clone()).collect();
        assert_eq!(kinds, ["sound", "wind_sfx", "type_77", "enemy"]);
        assert_eq!(log.counts["enemy parts"], 1);
        let (enemy, part) = w.iter::<UnplacedPart>().next().unwrap();
        assert_eq!((part.npc_param, part.model.as_deref()), (Some(100), Some("c1000")));
        assert_eq!(w.get::<Transform>(enemy).unwrap().cols[3], [5., 6., -7.]);
        assert_eq!(log.counts["spawn regions left to lower-fromsoft-extras"], 1);
        let (e, _) = w.iter::<Marker>().next().unwrap();
        assert_eq!(w.get::<Name>(e).unwrap().0, "Birds");
        assert_eq!(w.get::<Region>(e).unwrap().entity_id, 7);
        // the transform is the part rule's: the position through the core frame (z reflected)
        assert_eq!(w.get::<Transform>(e).unwrap().cols[3], [1., 2., -3.]);
        let (chest, event) = w.iter::<Event>().next().unwrap();
        assert_eq!((event.type_name, event.part.as_deref()), (Some("treasure"), Some("o1")));
        assert!(!w.has::<Transform>(chest) && !w.has::<Marker>(chest));
        assert_eq!(w.get::<Name>(chest).unwrap().0, "Chest");
        assert_eq!(w.get::<Record>(chest),Some(&Record {category:"event".into(),kind:"treasure".into()}));
        assert_eq!(w.get::<Record>(enemy),Some(&Record {category:"part".into(),kind:"enemy".into()}));
        assert_eq!(w.get::<Record>(e),Some(&Record {category:"region".into(),kind:"sound".into()}));
        assert_eq!(log.counts["treasure events"], 1);
    }

    /// A kind left out by choice is counted and not lowered; a word that is no kind is refused.
    #[test]
    fn a_kind_left_out_is_counted_and_a_bad_word_is_refused() {
        let mut w = world();
        let mut log = PassLog::default();
        pass("wind_sfx+sound+enemy").run(&mut w, &mut log).unwrap();
        assert_eq!(w.iter::<Marker>().count(), 1);
        assert_eq!(log.counts["enemy parts left out"], 1);
        assert_eq!(log.counts["wind_sfx regions left out"], 1);
        assert_eq!(log.counts["sound regions left out"], 1);
        let o = pass("sound+wind_sfx").options();
        assert_eq!(o["leave-out"], "sound+wind_sfx");
        for bad in ["Sound", "wind sfx", "a:b"] {
            assert!(
                LOWER_FROMSOFT_MAP_RECORDS
                    .instance(&Options::from([("leave-out".to_owned(), bad.to_owned())]))
                    .is_err(),
                "{bad}"
            );
        }
    }
}
