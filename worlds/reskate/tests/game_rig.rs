//! skate.'s animation rig read from the game's data (`installs.skate/Data`, `tools.oodle9`): the
//! `AnimBase_Default_Rig` with its 395-joint `AntSkeletonAsset`, its DOFs and default pose. Every joint has a rotation,
//! translation and scale DOF with a default (the skeleton's rests). The render skeleton is the rig's: every one of its
//! 386 bones is a joint of the animation skeleton with the same parent ([`render_map`]), and the rig's default pose,
//! composed into the model's space, is the render skeleton's stored model pose for every bone within 1e-5 but
//! `AITrajectory` (the animation rig turns it a quarter turn about y, the render skeleton turns its children instead;
//! their model poses agree).

use primase_animation::pose::{affine, locals, model_transforms};
use primase_compress::oodle::{Oodle, Version};
use primase_core::{Skeleton, World};
use primase_frostbite::folder::{Folder, Source};
use primase_reskate::game::rig::{DofTarget, SkateAnimRig, insert_rig, render_map};
use primase_reskate::game::skeleton::{SkateSkeletonAsset, read_skeleton};
use primase_reskate::read::game::read_rig;
use primase_testdata::{Game, Tool, install_file, tool};

type R = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_animation_rig_reads_and_the_render_skeleton_is_its() -> R {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return Ok(());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let rig = read_rig(
        &src,
        "Win32/globals.toc",
        "win32/configurations/gameconfigurations/delmargameconfiguration",
        "animation/dingo/animbase_default_rig",
    )?;
    assert_eq!(rig.name, "Animation/Dingo/AnimBase_Default_Rig");
    assert_eq!(rig.skeleton_name, "Animation/Dingo/AnimBase_Default_Skeleton");
    assert_eq!((rig.joints.len(), rig.dofs.len()), (395, 2721));

    let mut w = World::new();
    let anim = insert_rig(&rig, "animation/dingo/animbase_default_rig", &mut w)?;
    let (_, doc) = src.ebx(
        "Win32/levels/game/bam_levelroot/bam_levelroot.toc",
        "characters/maincharacters/common/animbase/animbase_default_renderskeleton",
    )?;
    let render = read_skeleton(&doc, "render", &mut w)?;
    assert_eq!(w.verify_components(), vec![]);
    let a = w.get::<Skeleton>(anim).unwrap();
    let joint_dofs = w
        .get::<SkateAnimRig>(anim)
        .unwrap()
        .dofs
        .iter()
        .filter(|d| !matches!(d.target, DofTarget::Other(_)))
        .count();
    assert_eq!(joint_dofs, 395 * 3);

    // the render skeleton is the rig's: names and parents
    let r = w.get::<Skeleton>(render).unwrap();
    let pairs = render_map(a, r)?;
    assert_eq!(pairs.len(), 386);

    // and its rest: the default pose in the model's space against the render skeleton's model poses
    let model = model_transforms(a, &locals(a, None)).map_err(|e| e.0)?;
    let asset = w.get::<SkateSkeletonAsset>(render).unwrap();
    let mut worst = (0.0f64, String::new());
    let mut off = Vec::new();
    for &(ja, jr) in &pairs {
        let i = r.joints.iter().position(|j| j.id == jr).unwrap();
        let d = affine(&asset.model_pose[i])
            .iter()
            .flatten()
            .zip(model[&ja].iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max);
        if d > 1e-5 {
            off.push(r.joints[i].name.clone());
        } else if d > worst.0 {
            worst = (d, r.joints[i].name.clone());
        }
    }
    println!(
        "default pose = render model pose within {:e} (at {}); apart: {off:?}",
        worst.0, worst.1
    );
    assert_eq!(off, ["AITrajectory"]);
    Ok(())
}
