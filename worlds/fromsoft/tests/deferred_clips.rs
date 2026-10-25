//! Existing character fixture: name-only listing and one selected decode, without reading a model/material.
use primase_core::World;
use primase_fromsoft::{Install, dcx::Decoder, sekiro};
use primase_havok::world::HavokAnimation;
use primase_testdata::{Game, Tool, install, oodle};
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn an_existing_character_lists_every_clip_and_decodes_only_one() -> Result<()> {
    let (Some(root), Some(library)) = (install(Game::Sekiro), oodle(Tool::Oodle6)) else { return Ok(()); };
    let codec = common::oodle(library)?;
    let decoder = Decoder { oodle: Some(&codec), max_output: 1 << 30 };
    let archive = Install::sekiro(root).open()?;
    let listing = sekiro::list_clips(&archive, &decoder, "c1010")?.ok_or("c1010 animation binder")?;
    let chosen = listing.clips.iter().find(|c| c.name == "a000_003000").ok_or("existing a000_003000 fixture")?;
    let mut world = World::new();
    let skeleton = sekiro::read_clip_skeleton(&archive, &decoder, "c1010", &mut world)?.ok_or("skeleton binder")?;
    assert!(skeleton.skeleton.is_some());
    assert_eq!(skeleton.report.animations, 0);
    assert_eq!(skeleton.report.tae_files, 0);
    assert_eq!(world.iter::<primase_core::Skeleton>().count(), 1);
    assert_eq!(world.iter::<HavokAnimation>().count(), 0);
    assert!(listing.clips.len() > 1, "the fixture must distinguish one selected clip from the whole binder");
    let decoded = sekiro::decode_clip(&archive, &decoder, "c1010", &chosen.key, &mut world)?.ok_or("animation binder")?;
    assert_eq!(decoded.animations.len(), 1, "{:?}", decoded.report);
    assert_eq!(world.iter::<HavokAnimation>().count(), 1);
    assert_eq!(world.iter::<primase_core::Skeleton>().count(), 1, "decode reuses the listed source's skeleton");
    assert_eq!(world.iter::<primase_core::Mesh>().count(), 0);
    assert_eq!(world.iter::<primase_core::Texture>().count(), 0);
    assert_eq!(world.get::<primase_core::Name>(decoded.animations[0]).map(|n| n.0.as_str()), Some(chosen.name.as_str()));
    println!("FS_CLIP_DECODE={{\"character\":\"c1010\",\"listed\":{},\"decoded\":1}}", listing.clips.len());
    Ok(())
}
