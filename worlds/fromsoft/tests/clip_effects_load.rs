//! The effects a clip starts, read as the clip loads (`CharacterReader::load` with clip effects) and lowered: the load keeps
//! the effect files and textures it read with the clip it returns, so `lower-fromsoft-effects` on the loaded clip alone
//! makes its `core.clip_effects`. Sekiro's player, the Wolf's first light attack with the katana (`a050_300000`, the
//! katana's motion category 50): event 96 starts effect 600030 at dummy polygon 10301 (polygon 301 of the right hand's
//! first weapon) from 0.3 s, following it, and effect 400 at the character's dummy polygon 85 from 0.8333 s; both files are
//! in `sfxbnd_commoneffects`.
//!
//! Needs `installs.sekiro`, `keys.sekiro` and `tools.oodle6`; skipped when they are not configured.
mod common;
mod inputs;

use primase_core::convert::Reader;
use primase_core::particles::ParticleSystem;
use primase_core::pass::RunReport;
use primase_core::{ClipEffects, ClipKey, Name};
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::effect_data::{ClipEffectUses, EffectFile};

#[test]
#[allow(unsafe_code)]
fn a_loaded_clip_keeps_the_effect_files_it_read_and_lowers_to_its_spawns()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(root) = inputs::install(inputs::Game::Sekiro) else {
        return Ok(());
    };
    let Some(install) = common::sekiro(&root).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let Some(codec) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    // SAFETY: the test configuration supplies the trusted installed codec.
    let codec = unsafe {
        primase_foundations::compress::oodle::Oodle::open(
            codec.path(),
            primase_foundations::compress::oodle::Version::V6,
        )
    }?;
    let decoder = fromsoft_formats::dcx::Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let mut reader = sekiro::CharacterReader::animations(install, "c0000", decoder).with_clip_effects();
    let (plan, _) = reader.read()?;
    let (entity, _) = plan
        .iter::<ClipKey>()
        .find(|(e, _)| plan.get::<Name>(*e).is_some_and(|n| n.0 == "a050_300000"))
        .ok_or("a050_300000 not listed")?;
    let (mut loaded, _) = reader.load(&plan, &[entity].into())?;

    // the uses, and the files read for them, are in what the load returns
    let uses = loaded
        .get::<ClipEffectUses>(entity)
        .ok_or("no effect uses on the clip")?;
    let started: Vec<(i32, Option<i32>)> = uses.uses.iter().map(|u| (u.effect, u.dummy)).collect();
    assert_eq!(started, [(600030, Some(10301)), (400, Some(85))]);
    let files: std::collections::BTreeSet<i32> = loaded.iter::<EffectFile>().map(|(_, f)| f.effect).collect();
    assert!(files.contains(&600030) && files.contains(&400), "{files:?}");
    assert!(loaded.verify_components().is_empty());

    // lowered on the loaded clip alone
    let mut registry = primase_core::pass::PassRegistry::new();
    sekiro::passes::register(&mut registry);
    primase_havok::register(&mut registry);
    let m = primase_core::pass::PassManager::new(registry);
    let mut run = RunReport::default();
    m.run(
        &mut loaded,
        &m.registry.parse(
            "lower-havok-clips{absent=neutral,rate=1,root-motion=leave,clips=a050_300000}, \
             lower-fromsoft-clip-events{game=sekiro}, lower-fromsoft-effects{candela-per-power=1}",
        )?,
        &mut run,
    )?;
    let effects = loaded.get::<ClipEffects>(entity).ok_or("no core.clip_effects")?;
    let spawns: Vec<(Option<&str>, i32, bool)> = effects
        .spawns
        .iter()
        .map(|s| (s.socket.as_deref(), (s.start * 30.0).round() as i32, s.follow))
        .collect();
    // frames at 30 per second: 9 is 0.3 s, 25 is 0.8333 s
    assert_eq!(
        spawns,
        [(Some("dummy 10301"), 9, true), (Some("dummy 85"), 25, true)]
    );
    assert!(
        effects
            .spawns
            .iter()
            .all(|s| loaded.has::<ParticleSystem>(s.system))
    );
    Ok(())
}
