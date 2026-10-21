//! The glTF reader. A character written by the writer reads back to the same motion: posed by the clip at several
//! times, every vertex where the original's is (the writer's node tree read as a skeleton with the rig's root above
//! it). Against the Khronos glTF-Sample-Assets (`PRIMASE_KHRONOS_GLTF`, third-party files with pygltflib's reading of
//! each in `expected.json`): every model reads and verifies; skins' joints and inverse bind matrices are the
//! document's; each channel on a joint is a clip channel, each `weights` channel one per morph target, the rest
//! counted; mesh default weights are the morph targets' defaults.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{
    Binding, BindingId, Channel, Clip, ClipBinding, ClipTarget, Deformation, Deltas, Extrapolation,
    Influences, Instance, Interpolation, Joint, JointId, Mesh, MorphFrame, MorphId, MorphTarget,
    MorphWeights, Morphs, Primitive, Property, Rest, RigInstance, Semantic, Skeleton, Skin, SkinInstance,
    Slot, Stream, Topology, Transform, WeightRule, World,
};
use primase_gltf::{WriteOptions, read_glb, write};
use serde_json::Value;

const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn translate(t: [f32; 3]) -> Transform {
    let mut m = Transform::IDENTITY;
    m.cols[3] = t;
    m
}

/// A two-joint chain (1 at the origin, 2 at y=1) under a rig at x=3; a triangle skinned by streams with a morph target;
/// a placement at the rig with the morph at 0.25; a clip turning joint 2 (linear), raising it (Hermite) and driving the
/// morph (step).
fn character() -> World {
    let mut w = World::new();
    let skeleton = w.spawn();
    w.insert(
        skeleton,
        Skeleton {
            joints: vec![
                Joint {
                    id: JointId(1),
                    name: "hip".into(),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                },
                Joint {
                    id: JointId(2),
                    name: "knee".into(),
                    parent: Some(JointId(1)),
                    rest: Rest::Trs {
                        translation: [0.0, 1.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                },
            ],
        },
    );
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: [
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 2.0, 0.0]]),
                ),
                (
                    Semantic::Joints(0),
                    Stream::U8x4(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]),
                ),
                (
                    Semantic::Weights(0),
                    Stream::F32x4(vec![
                        [1.0, 0.0, 0.0, 0.0],
                        [0.5, 0.5, 0.0, 0.0],
                        [1.0, 0.0, 0.0, 0.0],
                    ]),
                ),
            ]
            .into(),
            indices: None,
            material: None,
        }]),
    );
    w.insert(
        proto,
        Skin {
            influences: vec![Some(Influences::Streams { sets: 1 })],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton,
                palettes: vec![Some(vec![
                    Slot {
                        joint: JointId(1),
                        inverse_bind: Transform::IDENTITY,
                    },
                    Slot {
                        joint: JointId(2),
                        inverse_bind: translate([0.0, -1.0, 0.0]),
                    },
                ])],
                deformation: Deformation::Linear,
            }],
        },
    );
    w.insert(
        proto,
        Morphs {
            targets: vec![MorphTarget {
                id: MorphId(0),
                name: "bulge".into(),
                default_weight: 0.0,
                frames: vec![MorphFrame {
                    weight: 1.0,
                    primitives: vec![Some(Deltas {
                        indices: None,
                        positions: Some(vec![[0.0; 3], [0.5, 0.0, 0.0], [0.0; 3]]),
                        ..Deltas::default()
                    })],
                }],
            }],
        },
    );
    let rig = w.spawn();
    w.insert(rig, RigInstance { skeleton });
    w.insert(rig, translate([3.0, 0.0, 0.0]));
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, translate([3.0, 0.0, 0.0]));
    w.insert(
        placed,
        SkinInstance {
            rig,
            binding: BindingId(0),
        },
    );
    w.insert(placed, MorphWeights(vec![(MorphId(0), 0.25)]));
    let clip = w.spawn();
    w.insert(
        clip,
        Clip {
            name: "bend".into(),
            duration: 1.0,
            extrapolation: Extrapolation::Hold,
            channels: vec![
                Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(2),
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S],
                },
                Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(2),
                    },
                    property: Property::Translation,
                    interpolation: Interpolation::Hermite,
                    times: vec![0.0, 1.0],
                    values: vec![
                        0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 0.0, //
                        0.0, -0.5, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0,
                    ],
                },
                Channel {
                    target: ClipTarget::Morph {
                        geometry: proto,
                        morph: MorphId(0),
                    },
                    property: Property::Scalar,
                    interpolation: Interpolation::Step,
                    times: vec![0.0, 0.5],
                    values: vec![0.0, 1.0],
                },
            ],
            root_motion: None,
            additive: None,
        },
    );
    let binding = w.spawn();
    w.insert(
        binding,
        ClipBinding {
            clip,
            rig: Some(rig),
            meshes: vec![placed],
        },
    );
    assert_eq!(w.verify_components(), vec![]);
    w
}

fn manager() -> PassManager {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    primase_animation::register(&mut r);
    PassManager::new(r).with_pass_checks(true)
}

/// Every placed vertex in world space after the clips are evaluated at `t` and the skins posed: per placement (in
/// entity order), its prototype's positions through its placement.
fn posed(mut w: World, t: f32) -> Vec<Vec<[f32; 3]>> {
    let m = manager();
    let pipeline = m
        .registry
        .parse(&format!("evaluate-clip{{time={t},root=ignore}}, pose-skin"))
        .unwrap();
    m.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    w.iter::<Instance>()
        .map(|(e, i)| {
            let at = *w.get::<Transform>(e).unwrap();
            w.get::<Mesh>(i.prototype)
                .unwrap()
                .primitives()
                .iter()
                .flat_map(|p| match &p.streams[&Semantic::Position] {
                    Stream::F32x3(v) => v.iter().map(|&q| at.apply(q)).collect::<Vec<_>>(),
                    _ => panic!("positions"),
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_written_character_reads_back_to_the_same_motion() {
    let original = character();
    let (bytes, _) = write(&original, &WriteOptions::default()).unwrap();
    let mut read = World::new();
    let report = read_glb(&bytes, "bend.glb", &mut read).unwrap();
    assert_eq!(read.verify_components(), vec![]);
    assert_eq!(
        (
            report.skeletons,
            report.bindings,
            report.morph_targets,
            report.clips,
            report.channels
        ),
        (1, 1, 1, 1, 3)
    );
    // the writer's rig root node is a joint above the chain: three joints
    assert_eq!(report.joints, 3);
    let copy = || {
        let all = original.entities().collect();
        original.subset(&all)
    };
    for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let (want, got) = (
            posed(copy(), t),
            posed(read.subset(&read.entities().collect()), t),
        );
        assert_eq!(want.len(), got.len());
        for (a, b) in want.iter().flatten().zip(got.iter().flatten()) {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() <= 1e-5, "t={t}: {a:?} vs {b:?}");
            }
        }
    }
}

/// The model folders of the Khronos fixtures, if the variable is set.
fn khronos() -> Vec<std::path::PathBuf> {
    let Some(dir) = primase_testdata::real_data("PRIMASE_KHRONOS_GLTF") else {
        return Vec::new();
    };
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// The node `expected.json` lists for index `i`.
fn node(expected: &Value, i: u64) -> &Value {
    &expected["nodes"][i as usize]
}

#[test]
fn khronos_models_read_as_pygltflib_reads_them() {
    for dir in khronos() {
        let model = dir.file_name().unwrap().to_string_lossy().into_owned();
        // a GLB, else a .gltf with its buffers beside it
        let glb = dir.join(format!("{model}.glb"));
        let gltf = if glb.exists() {
            primase_glb::Gltf::from_glb(&std::fs::read(&glb).unwrap()).unwrap()
        } else {
            let json = std::fs::read(dir.join(format!("{model}.gltf"))).unwrap();
            primase_glb::Gltf::from_gltf(&json, |_, uri| {
                std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
            })
            .unwrap()
        };
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
        let mut w = World::new();
        let report =
            primase_gltf::read::read(&gltf, &model, &mut w).unwrap_or_else(|e| panic!("{model}: {e}"));
        assert_eq!(w.verify_components(), vec![], "{model}");
        // the skeleton nodes: every joint of a skin a node draws, and every node above one
        let parent = |i: u64| node(&expected, i)["parent"].as_u64();
        let drawn: BTreeSet<u64> = expected["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["skin"].as_u64())
            .collect();
        let mut skeleton_nodes = BTreeSet::new();
        for s in &drawn {
            for j in expected["skins"][*s as usize]["joints"].as_array().unwrap() {
                let mut at = j.as_u64();
                while let Some(n) = at {
                    skeleton_nodes.insert(n);
                    at = parent(n);
                }
            }
        }
        // skins: each drawn skin's binding has its joints and inverse binds, as stored
        let bindings: BTreeMap<u32, &Binding> = w
            .iter::<Skin>()
            .flat_map(|(_, s)| s.bindings.iter())
            .map(|b| (b.id.0, b))
            .collect();
        for s in &drawn {
            let skin = &expected["skins"][*s as usize];
            let b = bindings[&(*s as u32)];
            let palette = b.palettes.iter().flatten().next().unwrap();
            let joints: Vec<u64> = skin["joints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|j| j.as_u64().unwrap())
                .collect();
            assert_eq!(
                palette.iter().map(|s| u64::from(s.joint.0)).collect::<Vec<_>>(),
                joints,
                "{model} skin {s}"
            );
            if let Some(ibms) = skin["inverseBindMatrices"].as_array() {
                for (slot, m) in palette.iter().zip(ibms) {
                    let m: Vec<f32> = m
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_f64().unwrap() as f32)
                        .collect();
                    let cols: [[f32; 3]; 4] = std::array::from_fn(|c| [m[4 * c], m[4 * c + 1], m[4 * c + 2]]);
                    assert_eq!(
                        slot.inverse_bind.cols, cols,
                        "{model} skin {s} joint {:?}",
                        slot.joint
                    );
                }
            }
        }
        // joints' rests: each skeleton node's stored transform
        for (_, sk) in w.iter::<Skeleton>() {
            for j in &sk.joints {
                let n = node(&expected, u64::from(j.id.0));
                match j.rest {
                    Rest::Trs {
                        translation,
                        rotation,
                        scale,
                    } => {
                        let get = |k: &str, d: &[f32]| -> Vec<f32> {
                            n[k].as_array().map_or_else(
                                || d.to_vec(),
                                |a| a.iter().map(|x| x.as_f64().unwrap() as f32).collect(),
                            )
                        };
                        assert_eq!(translation.to_vec(), get("translation", &[0.0; 3]), "{model}");
                        assert_eq!(
                            rotation.to_vec(),
                            get("rotation", &[0.0, 0.0, 0.0, 1.0]),
                            "{model}"
                        );
                        assert_eq!(scale.to_vec(), get("scale", &[1.0; 3]), "{model}");
                    }
                    Rest::Affine(t) => {
                        let m: Vec<f32> = n["matrix"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|x| x.as_f64().unwrap() as f32)
                            .collect();
                        let cols: [[f32; 3]; 4] =
                            std::array::from_fn(|c| [m[4 * c], m[4 * c + 1], m[4 * c + 2]]);
                        assert_eq!(t.cols, cols, "{model}");
                    }
                }
            }
            assert!(
                sk.joints
                    .iter()
                    .all(|j| skeleton_nodes.contains(&u64::from(j.id.0)))
            );
        }
        // channels: on a skeleton node one clip channel; weights one a morph target; the rest counted
        let targets_of = |n: u64| -> usize {
            let mesh = node(&expected, n)["mesh"].as_u64().unwrap() as usize;
            expected["meshes"][mesh]["primitives"][0]["morphTargetCount"]
                .as_u64()
                .unwrap() as usize
        };
        let (mut want, mut off) = (0usize, 0usize);
        for a in expected["animations"].as_array().unwrap() {
            for c in a["channels"].as_array().unwrap() {
                let n = c["targetNode"].as_u64().unwrap();
                if c["path"] == "weights" {
                    want += targets_of(n);
                } else if skeleton_nodes.contains(&n) {
                    want += 1;
                } else {
                    off += 1;
                }
            }
        }
        let got: usize = w.iter::<Clip>().map(|(_, c)| c.channels.len()).sum();
        assert_eq!((got, report.channels_off_joints), (want, off), "{model}");
        // default weights
        for m in expected["meshes"].as_array().unwrap() {
            let defaults: Vec<f32> = m["defaultWeights"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect();
            if defaults.is_empty() {
                continue;
            }
            let name = m["name"].as_str();
            let found = w.iter::<Morphs>().any(|(e, morphs)| {
                name.is_none_or(|n| w.get::<primase_core::Name>(e).is_some_and(|x| x.0 == n))
                    && morphs
                        .targets
                        .iter()
                        .map(|t| t.default_weight)
                        .collect::<Vec<_>>()
                        == defaults
            });
            assert!(found, "{model}: default weights {defaults:?}");
        }
        println!("{model}: {report:?}");
    }
}

/// Every placement's posed vertices in world space after the clips are evaluated at `t`, by placement name.
fn posed_by_name(mut w: World, t: f32) -> BTreeMap<String, Vec<[f32; 3]>> {
    let m = manager();
    let pipeline = m
        .registry
        .parse(&format!("evaluate-clip{{time={t},root=ignore}}, pose-skin"))
        .unwrap();
    m.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    w.iter::<Instance>()
        .map(|(e, i)| {
            let at = *w.get::<Transform>(e).unwrap();
            let points = w
                .get::<Mesh>(i.prototype)
                .unwrap()
                .primitives()
                .iter()
                .flat_map(|p| match &p.streams[&Semantic::Position] {
                    Stream::F32x3(v) => v.iter().map(|&q| at.apply(q)).collect::<Vec<_>>(),
                    _ => panic!("positions"),
                })
                .collect();
            (w.get::<primase_core::Name>(e).unwrap().0.clone(), points)
        })
        .collect()
}

/// Oracle: Blender 4.3.2's importer and evaluator on the Khronos skin and morph models (`blender_pose.json`, written
/// by primase-oracles' `fixtures/khronos-gltf/blender_pose.py` at key times of each model's animation): read by us,
/// the clip evaluated at those times and the skins posed, every vertex where Blender puts it, by index, within 1e-4
/// m. Objects match placements by name, or one to one when each side has a single mesh (Blender names an unnamed
/// node itself); Blender's bone display shape (`Icosphere`) is not a glTF node.
#[test]
fn khronos_models_pose_as_blender_poses_them() {
    let mut checked = 0;
    for dir in khronos() {
        let model = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(text) = std::fs::read_to_string(dir.join("blender_pose.json")) else {
            continue;
        };
        let oracle: Value = serde_json::from_str(&text).unwrap();
        let glb = dir.join(format!("{model}.glb"));
        let gltf = if glb.exists() {
            primase_glb::Gltf::from_glb(&std::fs::read(&glb).unwrap()).unwrap()
        } else {
            let json = std::fs::read(dir.join(format!("{model}.gltf"))).unwrap();
            primase_glb::Gltf::from_gltf(&json, |_, uri| {
                std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
            })
            .unwrap()
        };
        let mut w = World::new();
        primase_gltf::read::read(&gltf, &model, &mut w).unwrap();
        for pose in oracle["poses"].as_array().unwrap() {
            let t = pose["time"].as_f64().unwrap() as f32;
            let ours = posed_by_name(w.subset(&w.entities().collect()), t);
            let theirs: BTreeMap<&str, Vec<[f32; 3]>> = pose["objects"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| k.as_str() != "Icosphere")
                .map(|(k, v)| {
                    let points = v
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| std::array::from_fn(|i| p[i].as_f64().unwrap() as f32))
                        .collect();
                    (k.as_str(), points)
                })
                .collect();
            let pairs: Vec<(&Vec<[f32; 3]>, &Vec<[f32; 3]>, String)> = if theirs.len() == 1 && ours.len() == 1
            {
                let (a, b) = (ours.iter().next().unwrap(), theirs.iter().next().unwrap());
                vec![(a.1, b.1, format!("{} / {}", a.0, b.0))]
            } else {
                theirs
                    .iter()
                    .map(|(k, v)| (&ours[*k], v, (*k).to_owned()))
                    .collect()
            };
            for (a, b, name) in pairs {
                assert_eq!(a.len(), b.len(), "{model} {name} at {t}: vertex counts");
                let worst = a
                    .iter()
                    .zip(b)
                    .map(|(p, q)| (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0f32, f32::max))
                    .fold(0.0f32, f32::max);
                assert!(worst <= 1e-4, "{model} {name} at {t}: off by {worst}");
                println!(
                    "{model} {name} at {t}: {} vertices, largest difference {worst:e}",
                    a.len()
                );
                checked += 1;
            }
        }
    }
    if primase_testdata::real_data("PRIMASE_KHRONOS_GLTF").is_some() {
        assert!(checked > 0, "no blender_pose.json beside the fixtures");
    }
}
