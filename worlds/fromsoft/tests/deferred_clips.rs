//! Existing character fixture: reader planning and one strict root load, without models/materials.
use primase_core::convert::Reader;
use primase_core::{ClipKey, Name};
use primase_fromsoft::{Install, dcx::Decoder, sekiro};
use primase_havok::world::HavokAnimation;
use primase_testdata::{Game, Tool, install, oodle};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn an_existing_character_lists_every_clip_and_decodes_only_one() -> Result<()> {
    let (Some(root), Some(library)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let mut reader = sekiro::CharacterReader::animations(Install::sekiro(root), "c1010", decoder);
    let (plans, _) = reader.read().map_err(|error| error.to_string())?;
    let chosen = plans
        .iter::<Name>()
        .find(|(e, name)| plans.has::<ClipKey>(*e) && name.0 == "a000_003000")
        .map(|(e, _)| e)
        .ok_or("existing a000_003000 fixture")?;
    let key = plans.get::<ClipKey>(chosen).unwrap().clone();
    assert!(
        plans.iter::<ClipKey>().count() > 1,
        "fixture must distinguish one root from the whole binder"
    );
    assert_eq!(plans.iter::<primase_core::Skeleton>().count(), 0);
    assert_eq!(plans.iter::<HavokAnimation>().count(), 0);
    let (world, _) = reader
        .load(&plans, &[chosen].into())
        .map_err(|error| error.to_string())?;
    assert_eq!(world.iter::<HavokAnimation>().count(), 1);
    assert_eq!(
        world.iter::<primase_core::Skeleton>().count(),
        1,
        "decode reuses the listed source's skeleton"
    );
    assert_eq!(world.iter::<primase_core::Mesh>().count(), 0);
    assert_eq!(world.iter::<primase_core::Texture>().count(), 0);
    assert!(world.has::<HavokAnimation>(chosen));
    assert_eq!(world.get::<ClipKey>(chosen), Some(&key));
    assert_eq!(
        world.get::<Name>(chosen).map(|n| n.0.as_str()),
        Some("a000_003000")
    );
    println!(
        "FS_CLIP_DECODE={{\"character\":\"c1010\",\"listed\":{},\"decoded\":1}}",
        plans.iter::<ClipKey>().count()
    );
    Ok(())
}
