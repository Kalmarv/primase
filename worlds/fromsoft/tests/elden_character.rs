//! An Elden Ring character read by itself (`installs.elden`, `keys.elden`, `tools.oodle6`): the one character reader of
//! the crate, given the game's install, finds the model in `chr/<id>.chrbnd`, its textures in the high and low
//! resolution texture binders (`chr/<id>_h.texbnd`, `chr/<id>_l.texbnd`) and resolves its materials against the game's
//! material binaries, as it does Sekiro's against MTDs.
//!
//! - E1: `c2010` reads: its FLVER's 356 bones, 16 meshes and 16 materials (a census of every character's FLVER read
//!   them all as version 0x2001A), the textures its materials bind found in the texture binders, an asset holding its
//!   skeleton as the rig and its prototype as the part.
//! - E2: the notices say what the read left out (the Havok skeleton and animation binder when animations are not
//!   asked for).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_core::{Asset, Role, Skeleton, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::sekiro::{CharacterOptions, read_character_with};
use primase_fromsoft::{Game, Install, Keys};
use primase_testdata::{Game as Data, Tool, install, keys, tool};

mod common;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn an_elden_ring_character_reads_through_the_one_character_reader() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), tool(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = Install::new(dir, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
    let mut w = World::new();
    let report = read_character_with(&install, "c2010", &decoder, CharacterOptions::default(), &mut w)?;
    println!(
        "{}: {} bones, {} meshes, {} materials, {} textures; unread {:?}; missing shader bundles {:?}",
        report.id,
        report.bones,
        report.meshes,
        report.materials,
        report.textures,
        report.unread,
        report.missing_shader_bundles
    );
    // E1
    assert_eq!((report.bones, report.meshes), (356, 16));
    assert!(
        report.materials > 0 && report.materials <= 16,
        "{} materials",
        report.materials
    );
    assert!(report.textures > 0, "no texture was found in the texture binders");
    let (root, asset) = w
        .iter::<Asset>()
        .next()
        .map(|(e, a)| (e, a.members.clone()))
        .unwrap();
    let _ = root;
    assert_eq!(asset.iter().filter(|(_, role)| *role == Role::Rig).count(), 1);
    assert_eq!(asset.iter().filter(|(_, role)| *role == Role::Part).count(), 1);
    let (rig, _) = asset.iter().find(|(_, role)| *role == Role::Rig).unwrap();
    assert_eq!(w.get::<Skeleton>(*rig).unwrap().joints.len(), 356);
    // E2
    assert!(
        report.unread.iter().any(|f| f.ends_with(".anibnd")),
        "{:?}",
        report.unread
    );
    Ok(())
}

/// `c8002` (a creature) has two helper joints at its root scaled to nothing (`MoveSampling_...`,
/// `MasterRig_MoveSampling_UpperLimit`): their rests have no inverse. No vertex weights them, so the geometry lowers
/// with the identity held as their inverse bind, counted.
#[test]
fn joints_scaled_to_nothing_hold_the_identity_when_no_vertex_weights_them() -> Result<()> {
    let (Some(dir), Some(keys), Some(dll)) = (install(Data::Elden), keys(Data::Elden), tool(Tool::Oodle6))
    else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let install = Install::new(dir, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
    let mut w = World::new();
    read_character_with(&install, "c8002", &decoder, CharacterOptions::default(), &mut w)?;
    let report = common::run(
        &mut w,
        "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    )?;
    let counts = &report.steps[0].log.counts;
    assert_eq!(
        counts["joints whose rest has no inverse (a bone scaled to nothing), holding the identity as inverse bind: no vertex weights them"],
        2
    );
    Ok(())
}
