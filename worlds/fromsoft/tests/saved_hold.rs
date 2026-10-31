//! The copied save's missing stowed Longbow is a named placement refusal, not a native visibility rule.
mod common;
mod inputs;

use fromsoft_formats::{Binder, Game, Install, Keys, dcx::Decoder, flver::Model, save::Save};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Asset, Attachment, Name, RigInstance, Role, Sockets, Source, World};
use primase_fromsoft::sekiro::effect_data::HeldWeapon;
use primase_fromsoft::sekiro::{self, CharacterOptions, attack_clip::PlayerState};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn the_saved_two_handed_hold_names_the_unresolved_stowed_longbow() -> Result<()> {
    let (Some(root), Some(folder), Some(library), Some(save)) = (
        install(Data::Elden),
        keys(Data::Elden),
        oodle(Tool::Oodle6),
        oracle(OracleSet::Saves, Data::Elden, "ER0000.sl2"),
    ) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    let installed = Install::new(
        root,
        Game::EldenRing,
        Keys::from_folder(folder, Game::EldenRing, None)?,
    );
    let archive = installed.open()?;
    let save = Save::read(&std::fs::read(save)?)?;
    let player = PlayerState::from_save(save.slot(0).ok_or("no slot zero")?)?;
    // The native attachment lookup can fall back to c0000/40, but socket existence alone is not visibility.
    for (path, has_40) in [
        ("/chr/c0000.chrbnd.dcx", true),
        ("/parts/bd_m_1361_l.partsbnd.dcx", false),
    ] {
        let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
        let file = binder
            .files
            .iter()
            .find(|f| f.name.as_deref().is_some_and(|n| n.ends_with(".flver")))
            .ok_or("no model")?;
        let model = Model::read(&decoder.decode_if_dcx(&file.bytes)?)?;
        assert_eq!(
            model.dummies.iter().any(|d| d.reference_id == 40),
            has_40,
            "{path}"
        );
    }
    let options = CharacterOptions {
        animations: Some("c0000".to_owned()),
        animation_clips: Some(Vec::new()),
        equipped_weapons: player.held(),
        ..CharacterOptions::default()
    };
    let mut world = World::new();
    sekiro::read_parts_with(
        &installed,
        &["hd_m_1360_l", "bd_m_1361_l", "am_m_1360_l", "lg_m_1360_l"],
        &decoder,
        &options,
        &mut world,
    )?;
    let bow = world
        .iter::<HeldWeapon>()
        .find(|(_, h)| h.slot.left && h.model == 0)
        .ok_or("no read Longbow")?;
    assert_eq!(
        world.get::<Name>(bow.0).map(|n| n.0.as_str()),
        Some("wp_a_1410_l")
    );
    assert_eq!(bow.1.holds.left_hang.map(|h| h.dummy), Some(2040));
    let sources: Vec<String> = world
        .iter::<HeldWeapon>()
        .filter_map(|(root, _)| world.get::<Source>(root).map(|s| s.0.clone()))
        .collect();
    let mut registry = PassRegistry::new();
    sekiro::passes::register(&mut registry);
    primase_animation::register(&mut registry);
    registry.add(&primase_core::PLACE_ASSETS);
    let manager = PassManager::new(registry);
    let pipeline = format!(
        "lower-fromsoft-geometry{{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}},assemble-character{{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled,leave-out={}}},place-assets,hold-fromsoft-weapons{{grip=two-handed-right}}",
        sources.join("+")
    );
    let mut run = RunReport::default();
    manager.run(&mut world, &manager.registry.parse(&pipeline)?, &mut run)?;
    let hold = &run.steps.last().ok_or("no hold pass")?.log;
    assert!(
        hold.omitted.iter().any(|(why, count)| *count == 1
            && why.contains("wp_a_1410_l")
            && why.contains("body dummy 2040")
            && why.contains("dummy 40 fallback")
            && why.contains("visibility is unresolved")),
        "{:?}",
        hold.omitted
    );
    assert!(
        hold.drops
            .keys()
            .all(|(_, why)| !why.contains("game does not draw"))
    );
    let mut attachments = Vec::new();
    for (root, held) in world.iter::<HeldWeapon>() {
        let asset = world.get::<Asset>(root).ok_or("no held asset")?;
        let skeleton = asset
            .members
            .iter()
            .find(|(_, role)| *role == Role::Rig)
            .ok_or("no held rig")?
            .0;
        let (rig, _) = world
            .iter::<RigInstance>()
            .find(|(_, r)| r.skeleton == skeleton)
            .ok_or("no held instance")?;
        let attachment = world.get::<Attachment>(rig);
        let socket = attachment.and_then(|a| {
            world
                .get::<RigInstance>(a.rig)
                .and_then(|r| world.get::<Sockets>(r.skeleton))
                .and_then(|s| s.0.iter().find(|s| s.id == a.socket))
        });
        attachments.push((held.slot.left, held.model, socket.map(|s| s.name.clone())));
    }
    attachments.sort();
    assert_eq!(
        attachments,
        vec![
            (false, 0, Some("dummy 24".to_owned())),
            (false, 1, Some("dummy 2069".to_owned()))
        ]
    );
    assert!(
        world.verify_components().is_empty(),
        "{:?}",
        world.verify_components()
    );
    Ok(())
}
