//! The effects a clip starts, read as the clip loads (`CharacterReader::load` with clip effects) and lowered: the load keeps
//! the effect files and textures it read with the clip it returns, so `lower-fromsoft-effects` on the loaded clip alone
//! makes its `core.clip_effects`. Sekiro's player, the Wolf's first light attack with the katana (`a050_300000`, the
//! katana's motion category 50): event 96 starts effect 400 at the character's dummy polygon 85 from 0.8333 s, and effect
//! 600030 at dummy polygon 10301 (polygon 301 of the right hand's first weapon) under a spawn condition, which is counted;
//! with Kusabimaru held, each attack event draws its row's trail.
//!
//! Needs `installs.sekiro`, `keys.sekiro` and `tools.oodle6`; skipped when they are not configured.
mod common;
mod inputs;

use primase_core::convert::Reader;
use primase_core::particles::ParticleSystem;
use primase_core::pass::RunReport;
use primase_core::{ClipEffects, ClipKey, Name};
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::effect_data::{
    ClipEffectUses, EffectFile, EquippedWeapons, SPAWN_CONDITION, UseRoute,
};

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
    let read = |weapons: Option<i32>| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
        let mut reader =
            sekiro::CharacterReader::animations(install.clone(), "c0000", decoder).with_clip_effects();
        if let Some(row) = weapons {
            reader = reader.with_equipped_weapons(EquippedWeapons::right_hand(row));
        }
        let (plan, _) = reader.read()?;
        let (entity, _) = plan
            .iter::<ClipKey>()
            .find(|(e, _)| plan.get::<Name>(*e).is_some_and(|n| n.0 == "a050_300000"))
            .ok_or("a050_300000 not listed")?;
        let (loaded, _) = reader.load(&plan, &[entity].into())?;
        Ok((loaded, entity))
    };
    let mut registry = primase_core::pass::PassRegistry::new();
    sekiro::passes::register(&mut registry);
    primase_havok::register(&mut registry);
    let m = primase_core::pass::PassManager::new(registry);
    let lower = |loaded: &mut primase_core::World| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut run = RunReport::default();
        m.run(
            loaded,
            &m.registry.parse(
                "lower-havok-clips{absent=neutral,rate=1,root-motion=leave,clips=a050_300000},                  lower-fromsoft-clip-events{game=sekiro}, lower-fromsoft-effects{candela-per-power=1}",
            )?,
            &mut run,
        )?;
        Ok(())
    };

    // no weapon held: the uses, and the files read for them, are in what the load returns; 600030's event names a spawn
    // condition (counted, not used)
    let (mut loaded, entity) = read(None)?;
    let uses = loaded
        .get::<ClipEffectUses>(entity)
        .ok_or("no effect uses on the clip")?;
    let started: Vec<(i32, Option<i32>)> = uses.uses.iter().map(|u| (u.effect, u.dummy)).collect();
    assert_eq!(started, [(400, Some(85))]);
    assert!(
        uses.lost.contains(&(SPAWN_CONDITION.to_owned(), 1)),
        "{:?}",
        uses.lost
    );
    let files: std::collections::BTreeSet<i32> = loaded.iter::<EffectFile>().map(|(_, f)| f.effect).collect();
    assert!(files.contains(&400), "{files:?}");
    assert!(loaded.verify_components().is_empty());
    // lowered on the loaded clip alone (frames at 30 per second: 25 is 0.8333 s)
    lower(&mut loaded)?;
    let effects = loaded.get::<ClipEffects>(entity).ok_or("no core.clip_effects")?;
    let spawns: Vec<(Option<&str>, i32, bool)> = effects
        .spawns
        .iter()
        .map(|s| (s.socket.as_deref(), (s.start * 30.0).round() as i32, s.follow))
        .collect();
    assert_eq!(spawns, [(Some("dummy 85"), 25, true)]);
    assert!(
        effects
            .spawns
            .iter()
            .all(|s| loaded.has::<ParticleSystem>(s.system))
    );

    // Kusabimaru held (row 8000000, its trace slot 0: 401000 between its polygons 300 and 301): each attack event draws the
    // trail for its window, at the right hand's first weapon's polygon 300 (10300)
    let (mut loaded, entity) = read(Some(8_000_000))?;
    // the attack events under no special-effect state (State Info 0): one of a050_300000's three, 0.767 to 0.833 s
    let events = loaded
        .get::<sekiro::character_data::ClipEvents>(entity)
        .ok_or("no events")?;
    let attacks: Vec<(f32, f32)> = events
        .events
        .iter()
        .zip(&events.typed)
        .filter(|(e, t)| {
            e.kind == 1
                && t.values
                    .iter()
                    .any(|v| v.name == "State Info" && v.value == primase_core::EventValueData::I64(0))
        })
        .map(|(e, _)| (e.start, e.end))
        .collect();
    assert_eq!(attacks.len(), 1);
    assert_eq!(events.events.iter().filter(|e| e.kind == 1).count(), 3);
    let uses = loaded.get::<ClipEffectUses>(entity).ok_or("no effect uses")?;
    let traces: Vec<_> = uses
        .uses
        .iter()
        .filter(|u| {
            u.route
                == UseRoute::WeaponTrace {
                    row: 8_000_000,
                    slot: 0,
                }
        })
        .map(|u| (u.effect, u.dummy, u.tail, u.start, u.end.unwrap_or(f32::NAN)))
        .collect();
    let expected: Vec<_> = attacks
        .iter()
        .map(|&(start, end)| (401_000, Some(10_300), Some(10_301), start, end))
        .collect();
    assert_eq!(traces, expected);
    lower(&mut loaded)?;
    let effects = loaded.get::<ClipEffects>(entity).ok_or("no core.clip_effects")?;
    let trails = effects
        .spawns
        .iter()
        .filter(|s| s.socket.as_deref() == Some("dummy 10300") && s.follow && s.end.is_some())
        .count();
    assert_eq!(trails, attacks.len());
    // the trail's strips fade in and out over fractions of their length: 401000's tracers store 80 and 20, 50 and 20 (the
    // fxr project: percentages)
    let trail = effects
        .spawns
        .iter()
        .find(|s| s.socket.as_deref() == Some("dummy 10300"))
        .ok_or("no trail spawn")?;
    let fades: Vec<(f32, f32)> = loaded
        .get::<ParticleSystem>(trail.system)
        .ok_or("the trail is no system")?
        .emitters
        .iter()
        .filter_map(|e| match &e.draw {
            Some(primase_core::particles::Draw::Trail(t)) => Some((
                t.fade_start.curve.keys[0].value[0],
                t.fade_end.curve.keys[0].value[0],
            )),
            _ => None,
        })
        .collect();
    let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
    assert!(fades.len() >= 2, "{fades:?}");
    assert!(
        fades
            .iter()
            .all(|&(a, b)| (near(a, 0.8) || near(a, 0.5)) && near(b, 0.2)),
        "{fades:?}"
    );
    Ok(())
}
