//! An Elden Ring character's animations (`installs.elden`, `keys.elden`, `tools.oodle6`): its Havok clips lowered, its
//! Havok skeleton paired with the model's by name (`correspond-skeletons{parents=any}`) and the clips moved onto the
//! model by the game's own rule (`map-clip{reparented=model-space}`: the animated model-space pose of a Havok bone
//! replaces the model bone's, whatever the two rests and hierarchies are).
//!
//! - M1: `c8000` (a creature whose model parents most of its bones otherwise than its Havok skeleton: 13 of its 122
//!   pairs have the same parent) and `c2030` (a humanoid; its IK targets are roots in the model): the Havok skeleton
//!   pairs with the model's, the clips map.
//! - M2: the oracle, at every key time of every mapped clip: the model-space pose of every paired target joint equals
//!   the source's, the Havok skeleton's forward kinematics under the unmapped clip, to 1e-5.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_fromsoft::game::ParamKey;
use std::collections::BTreeMap;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Install, Keys};
use primase_animation::pose::{evaluate, model_transforms};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Clip, ClipTarget, Entity, JointId, JointMap, Name, Rest, Skeleton, World};
use primase_fromsoft::sekiro::{CharacterOptions, read_character_with};
use primase_testdata::{Game as Data, Tool, install, keys, oodle};

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn skeleton_named(w: &World, name: &str) -> (Entity, Skeleton) {
    w.iter::<Skeleton>()
        .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
        .map(|(e, s)| (e, s.clone()))
        .unwrap()
}

fn poses(
    clip: &Clip,
    entity: Entity,
    skeleton: &Skeleton,
    t: f32,
) -> Result<BTreeMap<JointId, primase_animation::pose::Affine>> {
    let e = evaluate(clip, entity, skeleton, None, f64::from(t))?;
    let locals: BTreeMap<JointId, Rest> = e.pose.joints.iter().copied().collect();
    Ok(model_transforms(skeleton, &locals)?)
}

/// Read `id`, lower `clips`, pair, map, and check every key time; returns (clips, key times, pairs, reparented).
fn check(
    id: &str,
    clips: &str,
    decoder: &Decoder<'_>,
    install: &Install,
) -> Result<(usize, usize, usize, u64)> {
    let mut w = World::new();
    read_character_with(
        install,
        id,
        decoder,
        CharacterOptions {
            animations: Some(id.to_owned()),
            ..CharacterOptions::default()
        },
        &mut w,
    )?;
    let mut registry = PassRegistry::new();
    primase_havok::register(&mut registry);
    primase_animation::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let mut run = RunReport::default();
    manager.run(
        &mut w,
        &manager.registry.parse(&format!(
            "lower-havok-clips{{absent=neutral,rate=1,root-motion=translation,clips={clips}}}, \
             correspond-skeletons{{from={id} havok skeleton,to={id} skeleton,tolerance=0.0001,parents=any}}"
        ))?,
        &mut run,
    )?;
    let source: Vec<(Entity, Clip)> = w.iter::<Clip>().map(|(e, c)| (e, c.clone())).collect();
    let mut mapped_run = RunReport::default();
    manager.run(
        &mut w,
        &manager
            .registry
            .parse("map-clip{unmapped=drop,rests=source,reparented=model-space}")?,
        &mut mapped_run,
    )?;
    let reparented = mapped_run.steps[0]
        .log
        .counts
        .iter()
        .find(|(k, _)| k.starts_with("joints moved by the source's model-space pose"))
        .map_or(0, |(_, n)| *n);
    let (hk_e, hk) = skeleton_named(&w, &format!("{id} havok skeleton"));
    let (md_e, md) = skeleton_named(&w, &format!("{id} skeleton"));
    let map = w.iter::<JointMap>().next().map(|(_, m)| m.clone()).unwrap();
    let (mut keys_checked, mut worst) = (0, 0f64);
    for (e, src) in &source {
        let mapped = w.get::<Clip>(*e).unwrap();
        let mut times: Vec<f32> = src
            .channels
            .iter()
            .filter(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == hk_e))
            .flat_map(|c| c.times.clone())
            .collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        for t in times {
            let (ms, mt) = (poses(src, hk_e, &hk, t)?, poses(mapped, md_e, &md, t)?);
            for (a, b) in &map.pairs {
                let d = ms[a]
                    .iter()
                    .flatten()
                    .zip(mt[b].iter().flatten())
                    .map(|(p, q)| (p - q).abs())
                    .fold(0.0, f64::max);
                worst = worst.max(d);
            }
            keys_checked += 1;
        }
    }
    println!(
        "{id}: {} clips, {keys_checked} key times, {} pairs, {reparented} reparented joints; worst model-space difference {worst:.2e}",
        source.len(),
        map.pairs.len()
    );
    assert!(worst < 1e-5, "{id}: {worst}");
    Ok((source.len(), keys_checked, map.pairs.len(), reparented))
}

#[test]
fn elden_ring_clips_move_onto_the_model_by_the_games_own_rule() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = Install::new(
        dir,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    // M1, M2: a creature whose hierarchy differs widely, and a humanoid
    let (clips, key_times, pairs, reparented) =
        check("c8000", "a000_000000;a000_003000;a000_020000", &decoder, &install)?;
    assert_eq!((clips, pairs), (3, 122));
    assert!(key_times > 100);
    assert!(reparented >= 100, "{reparented} reparented joints");
    let (clips, key_times, pairs, _) = check("c2030", "a000_000020;a000_041000", &decoder, &install)?;
    assert_eq!((clips, pairs), (2, 184));
    assert!(key_times > 100);
    // M3: a character whose own anibnd holds only the skeleton and the load lists of its divisions
    // (`c2010_div00.blf`, `c2010_div01.blf` name `c2010_div00.anibnd`, `c2010_div01.anibnd`: 82 and 86 animations)
    let (clips, key_times, pairs, _) = check("c2010", "a000_000000;a000_003010", &decoder, &install)?;
    assert_eq!((clips, pairs), (2, 126));
    assert!(key_times > 100);
    Ok(())
}

/// M3: the animation binders of a character are the ones its markers (`.txt`, or `.blf` in Elden Ring) name; the one
/// animation of the install with a NaN stored (`c2030/a001_009000`: the quantisation ranges of the scale of the bone
/// `R_Egg`, a leaf bone of the held item) is read with those three components absent, named.
#[test]
fn elden_ring_divisions_are_read_from_the_blf_markers_and_a_refusal_names_its_bone() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), oodle(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = Install::new(
        dir,
        Game::EldenRing,
        Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let mut w = World::new();
    let report = read_character_with(
        &install,
        "c2010",
        &decoder,
        CharacterOptions {
            animations: Some("c2010".into()),
            ..CharacterOptions::default()
        },
        &mut w,
    )?;
    let a = report.animations.unwrap();
    assert_eq!(a.companions, ["c2010_div00", "c2010_div01"]);
    assert_eq!((a.bones, a.animations), (126, 168));
    assert!(a.refused.is_empty(), "{:?}", a.refused);
    let mut w = World::new();
    let report = read_character_with(
        &install,
        "c2030",
        &decoder,
        CharacterOptions {
            animations: Some("c2030".into()),
            ..CharacterOptions::default()
        },
        &mut w,
    )?;
    let a = report.animations.unwrap();
    // the one animation whose scale range is NaN is read, that component as absent, and named
    assert!(a.refused.is_empty(), "{:?}", a.refused);
    assert_eq!(a.absent_components.len(), 3, "{:?}", a.absent_components);
    assert!(
        a.absent_components
            .iter()
            .all(|c| c.starts_with("scale ") && c.ends_with("of R_Egg in a001_009000")),
        "{:?}",
        a.absent_components
    );
    Ok(())
}
