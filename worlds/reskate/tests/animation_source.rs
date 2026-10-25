//! Light source proof using the existing off-board idle fixture: list all, decode one, then map onto render bones.
use primase_core::{Clip, JointMap, Mesh, Skeleton, Texture, World};
use primase_core::pass::{Pass, PassLog};
use primase_compress::oodle::{Oodle, Version};
use primase_frostbite::folder::{Folder, Source};
use primase_reskate::{game::{clip::{LowerSkateClips, SkateClip}, rig::render_map}, read::animation};
use primase_testdata::{Game, Tool, install_file, tool};

#[test]
#[ignore = "installed skate clip proof; default pair fixtures need no game files or native decoder"]
fn light_source_lists_then_decodes_one_without_meshes_or_images() -> Result<(), Box<dyn std::error::Error>> {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Err("explicit installed proof needs skate Data and Oodle9; no game data was measured".into());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the configuration supplies the game's trusted decoder.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source { folder: &folder, base: Some(&folder), oodle: Some(&oodle) };
    let mut world = World::new();
    let skeletons = animation::read_skeletons(&src, &mut world)?;
    let listing = animation::list_clips(&src, "Win32/levels/game/dingolevel_mpr/dingolevel_mpr.toc")?;
    assert!(listing.len() > 1);
    let entry = listing.iter().find(|c| c.name.eq_ignore_ascii_case("animation/dingo/c_proto_ofbnb_idle_lfoot")).ok_or("existing idle controller")?;
    assert_eq!(entry.duration, Some(12.0));
    assert_eq!(world.iter::<Clip>().count(), 0);
    assert_eq!(world.iter::<SkateClip>().count(), 0);
    let clip = animation::decode_clip(&src, entry, skeletons.animation, &mut world)?;
    assert_eq!(world.iter::<SkateClip>().count(), 1);
    let mut log = PassLog::default();
    LowerSkateClips.run(&mut world, &mut log)?;
    let pairs = render_map(world.get::<Skeleton>(skeletons.animation).ok_or("animation skeleton")?,
        world.get::<Skeleton>(skeletons.render).ok_or("render skeleton")?)?;
    let map = world.spawn();
    world.insert(map, JointMap { from: skeletons.animation, to: skeletons.render, pairs });
    let mut registry = primase_core::pass::PassRegistry::new();
    primase_animation::register(&mut registry);
    let manager = primase_core::pass::PassManager::new(registry);
    manager.run(&mut world, &manager.registry.parse("map-clip{unmapped=drop,rests=source,reparented=refuse}")?, &mut Default::default())?;
    assert_eq!(world.get::<Clip>(clip).ok_or("core clip")?.duration, 12.0);
    assert_eq!(world.iter::<Mesh>().count(), 0); assert_eq!(world.iter::<Texture>().count(), 0);
    assert!(world.verify_components().is_empty(), "{:?}", world.verify_components());
    println!("SKATE_CLIP_SOURCE={}", serde_json::json!({"listed":listing.len(),"decoded":1,"skeletons":2,"meshes":0,"textures":0,"key":entry.key}));
    Ok(())
}

/// Synthetic stored tracks and authored DOF map: the default fixture exercises lowering and the animation ->
/// render correspondence with no install, native codec, network, image payload or implicit world defaults.
#[test]
fn synthetic_skate_tracks_lower_and_map_to_render_without_an_install() -> Result<(), Box<dyn std::error::Error>> {
    use primase_core::{ClipTarget, Extrapolation, Joint, JointId, Property, Rest};
    use primase_reskate::game::{clip::{SkateTrack, insert_clip}, rig::{DofTarget, SkateAnimRig, SkateDof}};
    let bones = |base: u32| Skeleton { joints: vec![
        Joint { id: JointId(base), name: "Hips".into(), parent: None, rest: Rest::Trs {
            translation: [0.0, 1.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] } },
        Joint { id: JointId(base + 1), name: "Head".into(), parent: Some(JointId(base)), rest: Rest::Trs {
            translation: [0.0, 1.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] } },
    ] };
    let mut world = World::new();
    let animation = world.spawn(); world.insert(animation, bones(0));
    world.insert(animation, SkateAnimRig { name: "synthetic rig".into(), other_defaults: Vec::new(), dofs: vec![
        SkateDof { id: 1, name: "Hips.t".into(), target: DofTarget::Translation(JointId(0)) },
        SkateDof { id: 2, name: "Head.q".into(), target: DofTarget::Rotation(JointId(1)) },
    ] });
    let render = world.spawn(); world.insert(render, bones(10));
    let clip = insert_clip(SkateClip { name: "synthetic idle".into(), skeleton: animation,
        fps: 60.0, end_frame: 60, looping: true, tracks: vec![
            SkateTrack { dof: 1, width: 3, frames: vec![0, 60], values: vec![0.0, 1.0, 0.0, 1.0, 1.0, 0.0] },
            SkateTrack { dof: 2, width: 4, frames: vec![0, 60], values: [0.0, 0.0, 0.0, 1.0].repeat(2) },
        ] }, &mut world);
    assert!(world.verify_components().is_empty(), "{:?}", world.verify_components());
    let mut log = PassLog::default(); LowerSkateClips.run(&mut world, &mut log)?;
    let pairs = render_map(world.get::<Skeleton>(animation).ok_or("animation")?, world.get::<Skeleton>(render).ok_or("render")?)?;
    let map = world.spawn(); world.insert(map, JointMap { from: animation, to: render, pairs });
    let mut registry = primase_core::pass::PassRegistry::new(); primase_animation::register(&mut registry);
    let manager = primase_core::pass::PassManager::new(registry).with_pass_checks(true);
    manager.run(&mut world, &manager.registry.parse("map-clip{unmapped=drop,rests=source,reparented=refuse}")?, &mut Default::default())?;
    let lowered = world.get::<Clip>(clip).ok_or("lowered clip")?;
    assert_eq!(lowered.duration, 1.0); assert_eq!(lowered.extrapolation, Extrapolation::Loop);
    assert_eq!(lowered.channels.len(), 2);
    assert!(lowered.channels.iter().all(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == render)));
    let move_root = lowered.channels.iter().find(|c| c.property == Property::Translation).ok_or("root channel")?;
    assert_eq!(move_root.target, ClipTarget::Joint { skeleton: render, joint: JointId(10) });
    assert_eq!(move_root.times, [0.0, 1.0]);
    assert_eq!(move_root.values, [0.0, 1.0, 0.0, 1.0, 1.0, 0.0]);
    assert_eq!(world.iter::<SkateClip>().count(), 0);
    assert_eq!(world.iter::<Mesh>().count(), 0); assert_eq!(world.iter::<Texture>().count(), 0);
    assert!(world.verify_components().is_empty(), "{:?}", world.verify_components());
    assert!(!log.approximations.is_empty(), "the interpolation choice must stay reported");
    Ok(())
}
