//! The effects of three areas read through the two routes (`Options::effects`: Ashina Castle `m11_01_00_00`, the Academy of
//! Raya Lucaria `m14_00_00_00` and the Shaded Castle tile `m60_39_54_00` with its parent tiles' parts) and lowered by
//! `lower-fromsoft-effects`. The routes' counts are the independent census's of `effect_lights_census.rs` (a Python walk of
//! the decoded layouts, the typed param dumps and the effect binders): the SFX regions, and the attachments the used slots of
//! the placed models' rows make. Every placed effect has its file in the world, its textures and the models its mesh
//! particles draw are in the world, and the pass gives each a system or counts why not, and each model a mesh or counts why
//! not.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}` and `tools.oodle6`; a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::pass::RunReport;
use primase_core::{Mesh, Texture, Transform, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{
    self,
    effect_data::{Effect, EffectFile, EffectModel, EffectResource, ResourceKind},
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
        assert_eq!(
            (r.regions, r.regions_outside),
            (area.regions, 0),
            "{map}: SFX regions"
        );
        assert_eq!(r.region_effects, area.regions, "{map}");
        assert_eq!(r.model_effects, area.attachments, "{map}: model attachments");

        // one entity per placed effect, at a place, with its file in the world
        let placed: Vec<_> = world.iter::<Effect>().map(|(e, p)| (e, p.clone())).collect();
        assert_eq!(
            placed.len() + r.unplaced,
            r.region_effects + r.model_effects,
            "{map}: placed + unplaced = reached"
        );
        assert!(!placed.is_empty(), "{map}");
        let files: BTreeSet<i32> = world.iter::<EffectFile>().map(|(_, f)| f.effect).collect();
        assert_eq!(files.len(), r.files, "{map}: effect files");
        for (e, p) in &placed {
            assert!(
                files.contains(&p.effect),
                "{map}: effect {} placed without its file",
                p.effect
            );
            let t = world.get::<Transform>(*e).ok_or("an effect without a place")?;
            assert!(t.cols.iter().flatten().all(|x| x.is_finite()));
        }
        // the textures the effects name, one entity per kind and number: a core texture each that the read gave, a
        // core.texture_absent each that it could not (counted in the report's missing); the models, one entity each that
        // the read gave (one it could not is counted only)
        let resources: Vec<_> = world.iter::<EffectResource>().map(|(e, r)| (e, *r)).collect();
        let (models, textures): (Vec<_>, Vec<_>) = resources
            .iter()
            .filter(|(_, r)| r.kind != ResourceKind::Animation)
            .partition(|(_, r)| r.kind == ResourceKind::Model);
        let read = textures.iter().filter(|(e, _)| world.has::<Texture>(*e)).count();
        let absent = textures
            .iter()
            .filter(|(e, _)| world.has::<primase_core::TextureAbsent>(*e))
            .count();
        assert_eq!(read, r.textures, "{map}: textures");
        let missing = |models: bool| {
            r.missing
                .iter()
                .filter(|(why, _)| why.starts_with("Model ") == models)
                .map(|(_, n)| n)
                .sum::<usize>()
        };
        assert_eq!(absent, missing(false), "{map}: absent textures");
        assert_eq!(
            read + absent,
            textures.len(),
            "{map}: each texture resource a texture or absent, not both"
        );
        assert_eq!(models.len(), r.models, "{map}: models");
        assert!(
            models.iter().all(|(e, _)| world.has::<EffectModel>(*e)),
            "{map}: a model resource without its model"
        );
        let distinct: BTreeSet<_> = resources.iter().map(|(_, r)| (r.kind, r.id)).collect();
        assert_eq!(distinct.len(), resources.len(), "{map}: a resource twice");

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
        assert_eq!(
            lowered + disabled,
            placed.len(),
            "{map}: lowered + left out = placed"
        );
        println!("{map}: {lowered} placed effects lowered, {disabled} left out as starting disabled");
        // each model a mesh, or counted as left out
        let meshes = models.iter().filter(|(e, _)| world.has::<Mesh>(*e)).count();
        let left_out: u64 = log
            .omitted
            .iter()
            .filter(|(why, _)| why.starts_with("effect models ") && why.ends_with(", left out"))
            .map(|(_, n)| n)
            .sum();
        assert_eq!(
            log.counts.get("effect models lowered").copied().unwrap_or(0) as usize,
            meshes,
            "{map}: effect models lowered"
        );
        assert_eq!(
            meshes + left_out as usize,
            models.len(),
            "{map}: models meshed + left out"
        );
        println!(
            "{map}: {} models read ({} missing), {meshes} meshed",
            models.len(),
            missing(true)
        );
    }
    Ok(())
}

/// Two of Elden Ring's spells read alone as a viewer reads an effect (`read_effect`: the shard spell 523022 and the pebble
/// spell 523002, from `sfxbnd_commoneffects`) and lowered: every model their mesh particles name is read (none missing) and
/// made a `core.mesh` whose materials are unlit `pbr.material`s, every mesh particle draws one, and a material whose
/// binding names an effect texture draws that texture as read.
#[test]
fn the_mesh_particles_of_two_spells_draw_their_models() -> Result<()> {
    use primase_core::particles::{Draw, ParticleSystem};
    use primase_core::pbr::{PbrMaterial, Shading};
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let (Some(root), Some(keys)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let game = Game::EldenRing;
    let keys = Keys::from_folder(keys, game, game.param_key_sha256())?;
    let archive = common::install(root, game, keys).open()?;
    for effect in [523022, 523002] {
        let mut world = World::default();
        let binders = sekiro::binders_for_area(game, "m60_39_54_00");
        let (report, placed) = sekiro::read_effect(&archive, &decoder, game, binders, effect, &mut world)?;
        let placed = placed.ok_or("the effect is not placed")?;
        println!("{effect}: {report:?}");
        assert!(report.models > 0, "{effect}: no model read");
        assert!(
            !report.missing.keys().any(|why| why.starts_with("Model ")),
            "{effect}: {:?}",
            report.missing
        );

        let m = common::manager().with_conservation(primase_core::ledger::Conservation::Enforce);
        let mut run = RunReport::default();
        m.run(
            &mut world,
            &m.registry.parse("lower-fromsoft-effects{candela-per-power=1}")?,
            &mut run,
        )?;
        let log = &run.steps[0].log;
        assert_eq!(
            log.counts.get("effect models lowered").copied().unwrap_or(0) as usize,
            report.models,
            "{effect}: {log:?}"
        );
        assert!(
            !log.omitted
                .contains_key("a model particle whose model is not read"),
            "{effect}: {:?}",
            log.omitted
        );
        // every system the effect is made of: the placed one and the ones it spawns
        let systems: Vec<&ParticleSystem> = world.iter::<ParticleSystem>().map(|(_, s)| s).collect();
        assert!(world.has::<ParticleSystem>(placed));
        let meshes: Vec<_> = systems
            .iter()
            .flat_map(|s| &s.emitters)
            .filter_map(|e| match &e.draw {
                Some(Draw::Mesh(m)) => Some(m),
                _ => None,
            })
            .collect();
        assert!(!meshes.is_empty(), "{effect}: no mesh particle");
        let mut textured = 0;
        for m in &meshes {
            let mesh = world
                .get::<Mesh>(m.mesh)
                .ok_or("a mesh particle's model without a mesh")?;
            for p in mesh.primitives() {
                let material = world
                    .get::<PbrMaterial>(p.material.ok_or("a primitive without a material")?)
                    .ok_or("a material that is not a pbr.material")?;
                assert_eq!(material.shading, Shading::Unlit);
                if let Some(t) = &material.base_color_texture {
                    assert!(
                        world.has::<Texture>(t.texture),
                        "{effect}: a colour texture not read"
                    );
                    textured += 1;
                }
            }
        }
        println!(
            "{effect}: {} mesh particles, {textured} textured primitives",
            meshes.len()
        );
        assert!(textured > 0, "{effect}: no mesh particle draws a texture");
    }
    Ok(())
}
