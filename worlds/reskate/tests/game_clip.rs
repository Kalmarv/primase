//! skate. clips read from the game's data (`PRIMASE_SKATE_DATA`, `PRIMASE_OODLE9`), lowered and moved onto the render
//! skeleton: the off-board idle read as stored (`skate.clip`, verified against the rig), lowered to a `core.clip`
//! (linear between keys, the stated choice), then mapped onto the render skeleton by the rig's own correspondence
//! (`render_map`, `map-clip{rests=source}`). Exact: at every time checked, each render bone composes to the model-space
//! transform of its animation joint under the same clip, within 1e-5.

use std::collections::BTreeMap;

use primase_animation::pose::{evaluate, locals, model_transforms};
use primase_compress::oodle::{Oodle, Version};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Clip, JointMap, Skeleton, World};
use primase_frostbite::folder::{Folder, Source};
use primase_reskate::game::clip::SkateClip;
use primase_reskate::game::rig::{insert_rig, render_map};
use primase_reskate::game::skeleton::read_skeleton;
use primase_reskate::read::game::{read_clip, read_rig};
use primase_testdata::real_data;

type R = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_skate_clip_drives_the_render_skeleton_as_it_drives_the_rig() -> R {
    let (Some(data), Some(dll)) = (real_data("PRIMASE_SKATE_DATA"), real_data("PRIMASE_OODLE9")) else {
        return Ok(());
    };
    let folder = Folder::open(data)?;
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let mut w = World::new();
    let rig = read_rig(
        &src,
        "Win32/globals.toc",
        "win32/configurations/gameconfigurations/delmargameconfiguration",
        "animation/dingo/animbase_default_rig",
    )?;
    let anim = insert_rig(&rig, "animation/dingo/animbase_default_rig", &mut w)?;
    let (_, doc) = src.ebx(
        "Win32/levels/game/bam_levelroot/bam_levelroot.toc",
        "characters/maincharacters/common/animbase/animbase_default_renderskeleton",
    )?;
    let render = read_skeleton(&doc, "render", &mut w)?;
    let clip = read_clip(
        &src,
        "Win32/levels/game/dingolevel_mpr/dingolevel_mpr.toc",
        "animation/dingo/c_proto_ofbnb_idle_lfoot",
        anim,
        &mut w,
    )?;
    {
        let c = w.get::<SkateClip>(clip).unwrap();
        assert_eq!(
            (c.fps, c.end_frame, c.looping, c.tracks.len()),
            (60.0, 720, true, 84)
        );
    }
    assert_eq!(w.verify_components(), vec![]);

    let mut reg = PassRegistry::new();
    primase_core::register(&mut reg);
    primase_animation::register(&mut reg);
    primase_reskate::register(&mut reg);
    let m = PassManager::new(reg).with_pass_checks(true);
    let mut report = RunReport::default();
    m.run(
        &mut w,
        &m.registry.parse("lower-skate-clips{interpolation=linear}")?,
        &mut report,
    )?;
    let lowered = w.get::<Clip>(clip).unwrap().clone();
    assert_eq!(lowered.duration, 12.0);
    println!(
        "lowered: {:?} {:?}",
        report.steps[0].log.counts, report.steps[0].log.omitted
    );

    // the animation rig's pose under the clip, before mapping
    let times = [0.0, 1.5, 4.25, 9.0];
    let a = w.get::<Skeleton>(anim).unwrap().clone();
    let before: Vec<_> = times
        .iter()
        .map(|&t| {
            let e = evaluate(&lowered, anim, &a, None, t).unwrap();
            model_transforms(&a, &locals(&a, Some(&e.pose))).unwrap()
        })
        .collect();

    // onto the render skeleton by the rig's own correspondence
    let pairs = render_map(&a, w.get::<Skeleton>(render).unwrap())?;
    let map = w.spawn();
    w.insert(
        map,
        JointMap {
            from: anim,
            to: render,
            pairs: pairs.clone(),
        },
    );
    let mut report = RunReport::default();
    m.run(
        &mut w,
        &m.registry.parse("map-clip{unmapped=drop,rests=source}")?,
        &mut report,
    )?;
    println!(
        "mapped: {:?} {:?}",
        report.steps[0].log.counts, report.steps[0].log.omitted
    );
    let r = w.get::<Skeleton>(render).unwrap().clone();
    let mapped = w.get::<Clip>(clip).unwrap().clone();
    let pairs: BTreeMap<_, _> = pairs.into_iter().collect();
    let mut worst = 0.0f64;
    for (k, &t) in times.iter().enumerate() {
        let e = evaluate(&mapped, render, &r, None, t).map_err(|e| e.0)?;
        let after = model_transforms(&r, &locals(&r, Some(&e.pose))).map_err(|e| e.0)?;
        for (ja, jr) in &pairs {
            let d = before[k][ja]
                .iter()
                .flatten()
                .zip(after[jr].iter().flatten())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f64::max);
            worst = worst.max(d);
        }
    }
    println!("render bones vs animation joints, model space: within {worst:e}");
    assert!(worst <= 1e-5, "{worst}");
    assert_eq!(w.verify_components(), vec![]);
    Ok(())
}
