//! The effect lights of three areas read through both routes (`Options::effect_lights`): Ashina Castle (`m11_01_00_00`),
//! the Academy of Raya Lucaria (`m14_00_00_00`, a legacy dungeon) and the Shaded Castle tile (`m60_39_54_00`, with its
//! parent tiles' parts), against an independent census (a Python walk of the decoded layouts, the Smithbox-typed param dumps
//! and the files of the effect binders, reading nothing of the Rust crates): the SFX regions and the lights each reaches
//! through the effects it spawns, and for the placed objects and assets the rows they name, their slots and the lights those
//! reach, with the reasons a reference placed nothing. The lights a part's row reaches that the read could not place (a
//! model the archive lacks, a model of no vertex, a dummy polygon the model lacks) are counted by reason, and the counts
//! must add up: reached = placed + lost.
//!
//! The independent census does not read the models: how many dummy polygons of a model carry a slot's reference id (so how
//! many times a slot's lights are placed) is the crate's FLVER reader's, checked in `fromsoft-formats`; the placed counts
//! are asserted as measured, and their relation to the census by the accounting above.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}` and `tools.oodle6`; a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::pass::RunReport;
use primase_core::{Instance, Source, Transform, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{
    self, EffectLightsReport,
    source::{EffectLight, EffectPlace},
};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// What the independent census (`oracle_expect.py`) counts of an area, and what the read placed of it.
struct Area {
    game: Game,
    data: inputs::Game,
    map: &'static str,
    parents: sekiro::Parents,
    // the census: SFX regions, those whose effects reach a light, the lights they reach
    regions: usize,
    regions_with_light: usize,
    region_lights: usize,
    // the census: parts with a row, their used slots, the lights the slots reach, parts naming no row, Sekiro's unread rows
    parts_with_row: usize,
    slots: usize,
    slot_lights: usize,
    parts_naming_no_row: usize,
    unread_rows: usize,
    // the census: references that place nothing, by reason
    absent_rows: usize,
    // as read: lights lost because the model is not read, and the lights placed
    lost: &'static [(&'static str, usize)],
    attachments: usize,
    model_lights: usize,
}

const AREAS: [Area; 3] = [
    Area {
        game: Game::Sekiro,
        data: inputs::Game::Sekiro,
        map: "m11_01_00_00",
        parents: sekiro::Parents::None,
        regions: 89,
        regions_with_light: 7,
        region_lights: 44,
        parts_with_row: 278,
        slots: 278,
        slot_lights: 328,
        parts_naming_no_row: 1_396,
        unread_rows: 364,
        absent_rows: 0,
        lost: &[],
        attachments: 278,
        model_lights: 328,
    },
    Area {
        game: Game::EldenRing,
        data: inputs::Game::Elden,
        map: "m14_00_00_00",
        parents: sekiro::Parents::None,
        regions: 53,
        regions_with_light: 36,
        region_lights: 36,
        parts_with_row: 443,
        slots: 434,
        slot_lights: 145,
        parts_naming_no_row: 7_602,
        unread_rows: 0,
        absent_rows: 12,
        lost: &[],
        attachments: 452,
        model_lights: 146,
    },
    Area {
        game: Game::EldenRing,
        data: inputs::Game::Elden,
        map: "m60_39_54_00",
        parents: sekiro::Parents::Include,
        regions: 0,
        regions_with_light: 0,
        region_lights: 0,
        parts_with_row: 69,
        slots: 69,
        slot_lights: 70,
        parts_naming_no_row: 2_064,
        unread_rows: 0,
        absent_rows: 1,
        lost: &[("model uses no vertex", 2)],
        attachments: 67,
        model_lights: 68,
    },
];

fn read(
    area: &Area,
    decoder: &Decoder<'_>,
    install: &fromsoft_formats::Install,
) -> Result<(World, sekiro::Report)> {
    let options = sekiro::Options {
        parents: area.parents,
        effect_lights: true,
        ..Default::default()
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(install, area.map, decoder, &options, &mut world)?;
    Ok((world, report))
}

fn check_counts(area: &Area, r: &EffectLightsReport) {
    let map = area.map;
    assert!(r.asked && r.param_unresolved.is_none(), "{map}: {r:?}");
    assert_eq!(
        (
            r.regions,
            r.regions_outside,
            r.regions_with_light,
            r.region_lights
        ),
        (area.regions, 0, area.regions_with_light, area.region_lights),
        "{map}: SFX regions"
    );
    assert_eq!(
        (
            r.parts_with_row,
            r.slots,
            r.slot_lights,
            r.parts_naming_no_row,
            r.unread_rows
        ),
        (
            area.parts_with_row,
            area.slots,
            area.slot_lights,
            area.parts_naming_no_row,
            area.unread_rows
        ),
        "{map}: the model route's census"
    );
    assert_eq!(r.parts_without_row_name, 0, "{map}");
    // the references that place nothing, by reason
    let mut refused = BTreeMap::new();
    if area.absent_rows > 0 {
        refused.insert(
            "model effect row named but absent from the param".to_owned(),
            area.absent_rows,
        );
    }
    assert_eq!(r.refused, refused, "{map}: refusals");
    // reached = placed + lost, by reason
    let lost: usize = r.lost_lights.values().sum();
    assert_eq!(r.slot_lights, r.placed_slot_lights + lost, "{map}: accounting");
    let want_lost: BTreeMap<String, usize> = area.lost.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect();
    assert_eq!(r.lost_lights, want_lost, "{map}: lost lights");
    assert_eq!(
        (r.attachments, r.model_lights),
        (area.attachments, area.model_lights),
        "{map}: placed"
    );
    assert!(r.model_lights >= r.placed_slot_lights.min(1));
    assert_eq!(r.dummies_left_out, 0, "{map}");
}

#[test]
fn the_effect_lights_of_three_areas_are_placed_as_the_census_counts() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for area in &AREAS {
        let (Some(root), Some(keys)) = (inputs::install(area.data), inputs::keys(area.data)) else {
            continue;
        };
        let keys = Keys::from_folder(keys, area.game, area.game.param_key_sha256())?;
        let install = common::install(root, area.game, keys);
        let (mut world, report) = read(area, &decoder, &install)?;
        let r = &report.effect_lights;
        println!("{}: {r:?}", area.map);
        check_counts(area, r);

        // one entity per placed light, with a place, a source and a record that says where it came from
        let lights: Vec<_> = world.iter::<EffectLight>().map(|(e, l)| (e, l.clone())).collect();
        assert_eq!(lights.len(), r.lights(), "{}", area.map);
        let regions = lights
            .iter()
            .filter(|(_, l)| matches!(l.place, EffectPlace::Region { .. }))
            .count();
        assert_eq!(regions, r.region_lights);
        let mut placements: BTreeMap<String, [f32; 3]> = BTreeMap::new();
        for (e, _) in world.iter::<Instance>() {
            placements.insert(
                world.get::<Source>(e).ok_or("source")?.0.clone(),
                world.get::<Transform>(e).ok_or("transform")?.cols[3],
            );
        }
        let (mut farthest, mut measured) = (0f32, 0);
        for (e, l) in &lights {
            let t = world
                .get::<Transform>(*e)
                .ok_or("an effect light without a place")?;
            assert!(t.cols.iter().flatten().all(|x| x.is_finite()));
            assert!(
                world
                    .get::<Source>(*e)
                    .is_some_and(|s| s.0.starts_with(&format!("{}:", area.map)))
            );
            // a model's light stands near its model: its dummy polygon is on the model
            if let EffectPlace::Model { part, .. } = &l.place {
                let key = placements.keys().find(|k| k.ends_with(&format!(":{part}")));
                if let Some(origin) = key.and_then(|k| placements.get(k)) {
                    farthest = farthest.max(
                        t.cols[3]
                            .iter()
                            .zip(origin)
                            .map(|(a, b)| (a - b) * (a - b))
                            .sum::<f32>()
                            .sqrt(),
                    );
                    measured += 1;
                }
            }
        }
        println!(
            "{}: {measured} model lights measured, the farthest {farthest:.1} m from its model's origin",
            area.map
        );
        assert!(measured > 0 && farthest < 60., "{}: {farthest}", area.map);

        // the lowering: every light is lowered or counted by reason
        let m = common::manager().with_conservation(primase_core::ledger::Conservation::Enforce);
        let pipeline = "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15}";
        let mut run = RunReport::default();
        m.run(&mut world, &m.registry.parse(pipeline)?, &mut run)?;
        let log = &run.steps[0].log;
        let counted = |reason: &str| log.counts.get(reason).copied().unwrap_or(0) as usize;
        let out = counted("effect lights of regions that start disabled, left out")
            + counted("effect lights of a config the effect's first state does not choose, left out")
            + counted("effect lights dark at the lowered value, left out");
        assert_eq!(
            counted("effect lights lowered") + out,
            lights.len(),
            "{}: {:?}",
            area.map,
            log.counts
        );
        println!(
            "{}: {} effect lights lowered, {out} left out",
            area.map,
            counted("effect lights lowered")
        );
    }
    Ok(())
}
