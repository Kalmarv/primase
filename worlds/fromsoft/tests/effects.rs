//! The effects of three areas read through the two routes (`Options::effects`: Ashina Castle `m11_01_00_00`, the Academy of
//! Raya Lucaria `m14_00_00_00` and the Shaded Castle tile `m60_39_54_00` with its parent tiles' parts) and lowered by
//! `lower-fromsoft-effects`. The routes' counts are the independent census's of `effect_lights_census.rs` (a Python walk of
//! the decoded layouts, the typed param dumps and the effect binders): the SFX regions, and the attachments the used slots of
//! the placed models' rows make. Every placed effect has its file in the world, its textures are in the world, and the pass
//! gives each a system or counts why not.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}` and `tools.oodle6`; a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::pass::RunReport;
use primase_core::{Texture, Transform, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{
    self,
    effect_data::{Effect, EffectFile, EffectResource},
};
use std::collections::BTreeSet;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Area {
    game: Game,
    data: inputs::Game,
    map: &'static str,
    parents: sekiro::Parents,
    /// The census: SFX regions, and the attachments the used slots of the placed models make.
    regions: usize,
    attachments: usize,
}

const AREAS: [Area; 3] = [
    Area {
        game: Game::Sekiro,
        data: inputs::Game::Sekiro,
        map: "m11_01_00_00",
        parents: sekiro::Parents::None,
        regions: 89,
        attachments: 278,
    },
    Area {
        game: Game::EldenRing,
        data: inputs::Game::Elden,
        map: "m14_00_00_00",
        parents: sekiro::Parents::None,
        regions: 53,
        attachments: 452,
    },
    Area {
        game: Game::EldenRing,
        data: inputs::Game::Elden,
        map: "m60_39_54_00",
        parents: sekiro::Parents::Include,
        regions: 0,
        attachments: 67,
    },
];

#[test]
fn the_effects_of_three_areas_are_placed_with_their_files_and_lowered() -> Result<()> {
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
        let options = sekiro::Options {
            parents: area.parents,
            effects: true,
            ..Default::default()
        };
        let mut world = World::default();
        let report = sekiro::read_game_area(&install, area.map, &decoder, &options, &mut world)?;
        let r = &report.effects;
        println!("{}: {r:?}", area.map);
        let map = area.map;
        assert!(r.asked && r.param_unresolved.is_none(), "{map}: {r:?}");
        assert_eq!((r.regions, r.regions_outside), (area.regions, 0), "{map}: SFX regions");
        assert_eq!(r.region_effects, area.regions, "{map}");
        assert_eq!(r.model_effects, area.attachments, "{map}: model attachments");

        // one entity per placed effect, at a place, with its file in the world
        let placed: Vec<_> = world.iter::<Effect>().map(|(e, p)| (e, p.clone())).collect();
        assert_eq!(placed.len() + r.unplaced, r.region_effects + r.model_effects, "{map}: placed + unplaced = reached");
        assert!(!placed.is_empty(), "{map}");
        let files: BTreeSet<i32> = world.iter::<EffectFile>().map(|(_, f)| f.effect).collect();
        assert_eq!(files.len(), r.files, "{map}: effect files");
        for (e, p) in &placed {
            assert!(files.contains(&p.effect), "{map}: effect {} placed without its file", p.effect);
            let t = world.get::<Transform>(*e).ok_or("an effect without a place")?;
            assert!(t.cols.iter().flatten().all(|x| x.is_finite()));
        }
        // the textures the effects name are core textures, one per kind and number
        let resources: Vec<_> = world.iter::<EffectResource>().map(|(e, r)| (e, *r)).collect();
        assert_eq!(resources.len(), r.textures, "{map}: textures");
        assert!(resources.iter().all(|(e, _)| world.has::<Texture>(*e)));
        let distinct: BTreeSet<_> = resources.iter().map(|(_, r)| (r.kind, r.id)).collect();
        assert_eq!(distinct.len(), resources.len(), "{map}: a texture twice");

        // the lowering: each placed effect gets a system, or the pass counts why not
        let m = common::manager().with_conservation(primase_core::ledger::Conservation::Enforce);
        let mut run = RunReport::default();
        m.run(
            &mut world,
            &m.registry.parse("lower-fromsoft-effects{candela-per-power=1}")?,
            &mut run,
        )?;
        let log = &run.steps[0].log;
        let lowered = log.counts.get("placed effects lowered").copied().unwrap_or(0) as usize;
        let disabled = log
            .omitted
            .get("effects of regions that start disabled, left out")
            .copied()
            .unwrap_or(0) as usize;
        assert_eq!(lowered + disabled, placed.len(), "{map}: lowered + left out = placed");
        println!("{map}: {lowered} placed effects lowered, {disabled} left out as starting disabled");
    }
    Ok(())
}
