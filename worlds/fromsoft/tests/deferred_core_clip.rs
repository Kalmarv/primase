//! Opt-in installed proof of read-plan/load-one. Never launches a game executable.
mod inputs;

use primase_core::convert::Reader;
use primase_core::{Asset, Clip, ClipKey, Name, Role};
use primase_fromsoft::sekiro;
use primase_havok::world::HavokAnimation;
mod common;

#[test]
#[ignore = "opens an installed character and its authored animation binders"]
#[allow(unsafe_code)]
fn a_plan_loads_one_clip_as_read_on_its_original_entity()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(root) = inputs::install(inputs::Game::Sekiro) else {
        return Ok(());
    };
    let Some(root) = common::sekiro(&root).map_err(|e| e.to_string())? else {
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
    let mut reader = sekiro::CharacterReader::new(root, "c1010", decoder).with_animations("c1010");
    let (plan, _) = reader.read()?;
    assert!(plan.iter::<ClipKey>().count() > 1);
    assert_eq!(plan.iter::<HavokAnimation>().count(), 0);
    assert_eq!(plan.iter::<Clip>().count(), 0);
    let (entity, key) = plan
        .iter::<ClipKey>()
        .find(|(e, _)| plan.get::<Name>(*e).is_some_and(|n| n.0 == "a000_003000"))
        .ok_or("fixture clip not listed")?;
    assert!(
        plan.iter::<Asset>()
            .any(|(_, a)| a.members.contains(&(entity, Role::Clip)))
    );
    let (loaded, report) = reader.load(&plan, &[entity].into())?;
    assert_eq!(loaded.iter::<HavokAnimation>().count(), 1);
    assert!(loaded.has::<HavokAnimation>(entity));
    assert_eq!(loaded.get::<ClipKey>(entity), Some(key));
    assert!(loaded.has::<sekiro::character_data::ClipEvents>(entity));
    assert_eq!(
        loaded.iter::<Clip>().count(),
        0,
        "load does not lower or choose a map policy"
    );
    assert!(report.iter().any(|(_, line)| line.contains("no lowering")));
    assert!(loaded.verify_components().is_empty());
    let closure = loaded.closure([entity]);
    assert!(loaded.entities().all(|e| closure.contains(&e)));
    assert_eq!(
        plan.iter::<HavokAnimation>().count(),
        0,
        "load leaves the plan unchanged"
    );
    let (again, report) = reader.load(&loaded, &[entity].into())?;
    assert!(report.is_empty());
    assert_eq!(
        again.get::<HavokAnimation>(entity).unwrap().skeleton,
        loaded.get::<HavokAnimation>(entity).unwrap().skeleton
    );
    assert_eq!(
        again.get::<HavokAnimation>(entity).unwrap().track_to_bone,
        loaded.get::<HavokAnimation>(entity).unwrap().track_to_bone
    );
    Ok(())
}
