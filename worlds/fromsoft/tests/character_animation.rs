//! A Sekiro character's animations (`installs.sekiro`, `tools.oodle6`): `c1010` read with its animation binder, three of
//! its animations lowered to clips (`lower-havok-clips`), the Havok skeleton made to correspond to the model's
//! (`correspond-skeletons`) and the clips mapped onto it (`map-clip`).
//!
//! - A1: the read holds the Havok skeleton (127 bones) and every spline compressed animation of the binder as a clip
//!   member of the asset; what it did not read is reported by reason (the interleaved animation, the event file).
//! - A2: the Havok skeleton corresponds to the model's skeleton at nearly all its bones (the measured rests equal), the
//!   mapped clips drive the model's skeleton, and every key of every channel is what `SplineAnimation::sample` gives at
//!   that time, converted to the world's frame (the lowering, the map and the keys' times are exact).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use primase_core::frame::{Axis, SourceFrame};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Asset, Clip, ClipTarget, JointMap, Name, Property, Rest, Role, Skeleton, World};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::sekiro::{CharacterOptions, read_character_with};
use primase_havok::spline::{Absent, Fallback};
use primase_havok::world::HavokAnimation;
use primase_testdata::{Game, Tool, install, tool};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CLIPS: [&str; 3] = ["a000_003000", "a200_000200", "a100_000220"];

#[allow(unsafe_code)]
#[test]
fn a_characters_animations_read_lower_and_map_onto_its_model() -> Result<()> {
    let (Some(install), Some(dll)) = (install(Game::Sekiro), tool(Tool::Oodle6)) else {
        return Ok(());
    };
    // SAFETY: explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut w = World::new();
    let report = read_character_with(
        &install,
        "c1010",
        &decoder,
        CharacterOptions { animations: true },
        &mut w,
    )?;
    let animations = report.animations.clone().expect("the animation binder was read");
    println!(
        "{} bones in the Havok skeleton, {} animations, refused {:?}, unread {:?}",
        animations.bones, animations.animations, animations.refused, animations.unread
    );
    // A1
    assert_eq!(animations.bones, 127);
    assert!(
        animations.animations > 300,
        "{} animations",
        animations.animations
    );
    let root = w.iter::<Asset>().next().map(|(e, _)| e).unwrap();
    let clip_members = w
        .get::<Asset>(root)
        .unwrap()
        .members
        .iter()
        .filter(|(_, role)| *role == Role::Clip)
        .count();
    assert_eq!(clip_members, animations.animations);
    assert_eq!(w.iter::<HavokAnimation>().count(), animations.animations);
    assert!(
        animations.unread.iter().any(|f| f.ends_with(".tae")),
        "{:?}",
        animations.unread
    );
    assert!(!report.notices().is_empty());

    // A2
    let mut registry = PassRegistry::new();
    primase_havok::register(&mut registry);
    primase_animation::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    let pipeline = manager.registry.parse(&format!(
        "lower-havok-clips{{absent=neutral,rate=1,clips={}}}, \
         correspond-skeletons{{from=c1010 havok skeleton,to=c1010 skeleton,tolerance=0.0001}}, \
         map-clip{{unmapped=drop,rests=target}}",
        CLIPS.join(";")
    ))?;
    let mut run = RunReport::default();
    manager.run(&mut w, &pipeline, &mut run)?;
    for step in &run.steps {
        println!(
            "{}: {:?} {:?}",
            step.pass, step.log.counts, step.log.approximations
        );
    }
    let correspond = &run.steps[1].log;
    let paired = correspond.counts["joints paired"];
    assert!(
        paired >= 100,
        "{paired} of 127 joints paired: {:?}",
        correspond.counts
    );
    let (map_entity, map) = w.iter::<JointMap>().next().map(|(e, m)| (e, m.clone())).unwrap();
    let model_skeleton = map.to;
    let _ = map_entity;
    assert_eq!(w.get::<Name>(model_skeleton).unwrap().0, "c1010 skeleton");

    let frame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);
    let absent = Absent {
        position: Fallback::Neutral,
        rotation: Fallback::Neutral,
        scale: Fallback::Neutral,
    };
    let mut keys_compared = 0;
    for name in CLIPS {
        let (e, havok) = w
            .iter::<HavokAnimation>()
            .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
            .map(|(e, h)| (e, h.clone()))
            .unwrap();
        let clip: &Clip = w.get(e).unwrap();
        assert_eq!(clip.name, name);
        assert!(!clip.channels.is_empty());
        let mut driven = 0;
        for channel in &clip.channels {
            let ClipTarget::Joint { skeleton, joint } = channel.target else {
                panic!("a joint channel")
            };
            assert_eq!(
                skeleton, model_skeleton,
                "{name}: mapped onto the model's skeleton"
            );
            // the Havok joint this model joint is
            let (havok_joint, _) = map.pairs.iter().find(|(_, to)| *to == joint).unwrap();
            let track = havok
                .track_to_bone
                .iter()
                .position(|&b| b as u32 == havok_joint.0)
                .unwrap();
            let reference = &havok.reference_pose[havok_joint.0 as usize];
            let width = match channel.property {
                Property::Rotation => 4,
                _ => 3,
            };
            for (k, &t) in channel.times.iter().enumerate() {
                let sample = havok.spline.sample(track, t, reference, absent)?;
                let Rest::Trs {
                    translation,
                    rotation,
                    scale,
                } = (Rest::Trs {
                    translation: [
                        sample.translation[0],
                        sample.translation[1],
                        sample.translation[2],
                    ],
                    rotation: sample.rotation,
                    scale: [sample.scale[0], sample.scale[1], sample.scale[2]],
                })
                .converted(&frame)
                else {
                    unreachable!()
                };
                let expected: Vec<f32> = match channel.property {
                    Property::Translation => translation.to_vec(),
                    Property::Scale => scale.to_vec(),
                    _ => rotation.to_vec(),
                };
                let got = &channel.values[k * width..(k + 1) * width];
                // a rotation's sign is the clip's (continuity), the rotation the same
                let same = if width == 4 {
                    let dot: f32 = got.iter().zip(&expected).map(|(a, b)| a * b).sum();
                    (dot.abs() - 1.0).abs() < 1e-5
                } else {
                    got.iter()
                        .zip(&expected)
                        .all(|(a, b)| (a - b).abs() <= 1e-5 * (1.0 + b.abs()))
                };
                assert!(
                    same,
                    "{name} joint {joint:?} {:?} key {k}: {got:?} against {expected:?}",
                    channel.property
                );
                keys_compared += 1;
            }
            driven += 1;
        }
        println!(
            "{name}: {} channels, {driven} on the model's skeleton",
            clip.channels.len()
        );
    }
    println!("{keys_compared} keys compared");
    let model: &Skeleton = w.get(model_skeleton).unwrap();
    assert_eq!(model.joints.len(), 400);
    Ok(())
}
