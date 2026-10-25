//! Characters in glTF: a rig as a node tree, a skinned placement with its skin (palettes unified, a whole-bound
//! primitive spelled out), morph targets and an occurrence's weights, a clip as an animation; what glTF has no form for
//! counted; weights that do not sum to 1 refused; the Khronos validator when `tools.gltf_validator` is set.

#![allow(clippy::unwrap_used)]

mod common;

use common::scratch;
use primase_core::{
    Binding, BindingId, Channel, Clip, ClipBinding, ClipTarget, Deformation, Deltas, Extrapolation,
    Influences, Instance, Interpolation, Joint, JointId, Mesh, MorphFrame, MorphId, MorphTarget,
    MorphWeights, Morphs, Primitive, Property, Rest, RigInstance, RigPose, Semantic, Skeleton, Skin,
    SkinInstance, Slot, Stream, Topology, Transform, WeightRule, World,
};
use primase_gltf::{WriteOptions, write};
use serde_json::Value;

const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn translate(t: [f32; 3]) -> Transform {
    let mut m = Transform::IDENTITY;
    m.cols[3] = t;
    m
}

fn joint(id: u32, parent: Option<u32>, y: f32) -> Joint {
    Joint {
        id: JointId(id),
        name: format!("joint{id}"),
        parent: parent.map(JointId),
        rest: Rest::Affine(translate([0.0, y, 0.0])),
    }
}

/// A three-joint chain (1 at the origin, 2 at y=1, 3 at y=2); a prototype of two primitives: a triangle skinned by
/// streams over palette [1, 2], and a triangle bound whole to slot 0 of palette [2, 3]; one morph target on the first
/// primitive; a rig at x=5 posed with joint 2 turned; a skinned placement of it with the morph at 0.25; a looping clip
/// turning joint 3 (linear) and driving the morph (step), plus a control channel and root motion.
fn world(weights: [u8; 4]) -> World {
    let mut w = World::new();
    let skeleton = w.spawn();
    w.insert(
        skeleton,
        Skeleton {
            joints: vec![
                joint(1, None, 0.0),
                joint(2, Some(1), 1.0),
                joint(3, Some(2), 1.0),
            ],
        },
    );
    let slot = |j: u32, y: f32| Slot {
        joint: JointId(j),
        inverse_bind: translate([0.0, -y, 0.0]),
    };
    let triangle = |extra: Vec<(Semantic, Stream)>| {
        let mut streams: std::collections::BTreeMap<_, _> = extra.into_iter().collect();
        streams.insert(
            Semantic::Position,
            Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 2.0, 0.0]]),
        );
        streams.insert(Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, 1.0]; 3]));
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: None,
            material: None,
        }
    };
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![
            triangle(vec![
                (
                    Semantic::Joints(0),
                    Stream::U8x4(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]),
                ),
                (
                    Semantic::Weights(0),
                    Stream::U8x4(vec![[255, 0, 0, 0], weights, [255, 0, 0, 0]]),
                ),
            ]),
            triangle(vec![]),
        ]),
    );
    w.insert(
        proto,
        Skin {
            influences: vec![
                Some(Influences::Streams { sets: 1 }),
                Some(Influences::Whole { slot: 0 }),
            ],
            weights: WeightRule::SumsToOne,
            bindings: vec![Binding {
                id: BindingId(0),
                skeleton,
                palettes: vec![
                    Some(vec![slot(1, 0.0), slot(2, 1.0)]),
                    Some(vec![slot(2, 1.0), slot(3, 2.0)]),
                ],
                deformation: Deformation::Linear,
            }],
        },
    );
    w.insert(
        proto,
        Morphs {
            targets: vec![MorphTarget {
                id: MorphId(4),
                name: "bulge".into(),
                default_weight: 0.0,
                frames: vec![MorphFrame {
                    weight: 1.0,
                    primitives: vec![
                        Some(Deltas {
                            indices: Some(vec![1]),
                            positions: Some(vec![[0.5, 0.0, 0.0]]),
                            ..Deltas::default()
                        }),
                        None,
                    ],
                }],
            }],
        },
    );
    let rig = w.spawn();
    w.insert(rig, RigInstance { skeleton });
    w.insert(rig, translate([5.0, 0.0, 0.0]));
    w.insert(
        rig,
        RigPose {
            joints: vec![
                (JointId(1), Rest::Affine(Transform::IDENTITY)),
                (
                    JointId(2),
                    Rest::Trs {
                        translation: [0.0, 1.0, 0.0],
                        rotation: [0.0, 0.0, S, S],
                        scale: [1.0; 3],
                    },
                ),
                (JointId(3), Rest::Affine(translate([0.0, 1.0, 0.0]))),
            ],
            root_applied: None,
        },
    );
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, translate([5.0, 0.0, 0.0]));
    w.insert(
        placed,
        SkinInstance {
            rig,
            binding: BindingId(0),
        },
    );
    w.insert(placed, MorphWeights(vec![(MorphId(4), 0.25)]));
    let clip = w.spawn();
    w.insert(
        clip,
        Clip {
            name: "wave".into(),
            duration: 1.0,
            extrapolation: Extrapolation::Loop,
            channels: vec![
                Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(3),
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S],
                },
                Channel {
                    target: ClipTarget::Morph {
                        geometry: proto,
                        morph: MorphId(4),
                    },
                    property: Property::Scalar,
                    interpolation: Interpolation::Step,
                    times: vec![0.0, 0.5],
                    values: vec![0.0, 1.0],
                },
                Channel {
                    target: ClipTarget::Control { name: "board".into() },
                    property: Property::Scalar,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0],
                    values: vec![1.0],
                },
            ],
            root_motion: Some(primase_core::RootMotion {
                space: primase_core::RootSpace::Rig,
                translation: Some((Interpolation::Linear, vec![0.0, 1.0], vec![0.0; 6])),
                rotation: None,
            }),
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

fn json(bytes: &[u8]) -> Value {
    primase_glb::Document::parse(bytes).unwrap().json
}

fn accessor_bytes(doc: &primase_glb::Document, a: usize) -> &[u8] {
    let acc = &doc.json["accessors"][a];
    let view = &doc.json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
    let off = view["byteOffset"].as_u64().unwrap() as usize;
    let len = view["byteLength"].as_u64().unwrap() as usize;
    &doc.bin[off..off + len]
}

/// Hand-worked expectations of the written glTF: the rig's root node at x=5 with joint nodes 1 > 2 > 3 under it,
/// joint 2's pose as a rotation; one skin whose joints are the nodes of joints 1, 2, 3 (palettes [1,2] and [2,3]
/// unified in first-use order) with their inverse binds; the first primitive's joints as stored (slots 0, 1 map to
/// 0, 1), the second's spelled out as joint 1 (slot 0 of [2,3] is the second joint of the list) with weight 1; the
/// skinned node without a transform, at the scene's root, its weights [0.25]; the morph as a target with the sparse
/// delta made dense; the clip as one animation with a rotation channel on joint 3's node and a weights channel on the
/// skinned node, marked as looping; the control channel and root motion counted.
#[test]
fn a_rig_skin_morphs_and_clip_as_gltf() {
    let w = world([128, 127, 0, 0]);
    let (bytes, report) = write(&w, &WriteOptions::default()).unwrap();
    let doc = primase_glb::Document::parse(&bytes).unwrap();
    let j = &doc.json;
    let nodes = j["nodes"].as_array().unwrap();
    let named = |n: &str| nodes.iter().position(|x| x["name"] == n).unwrap();
    let (j1, j2, j3) = (named("joint1"), named("joint2"), named("joint3"));
    let rig_root = nodes
        .iter()
        .position(|x| x["children"] == serde_json::json!([j1]))
        .unwrap();
    assert_eq!(nodes[rig_root]["matrix"][12], 5.0);
    assert_eq!(nodes[j1]["children"], serde_json::json!([j2]));
    assert_eq!(nodes[j2]["children"], serde_json::json!([j3]));
    let r = nodes[j2]["rotation"].as_array().unwrap();
    assert!((r[2].as_f64().unwrap() - f64::from(S)).abs() < 1e-6);
    // the skin
    assert_eq!(j["skins"].as_array().unwrap().len(), 1);
    let skin = &j["skins"][0];
    assert_eq!(skin["joints"], serde_json::json!([j1, j2, j3]));
    assert_eq!(skin["skeleton"], rig_root);
    let ibm: &[f32] = bytemuck::cast_slice(accessor_bytes(
        &doc,
        skin["inverseBindMatrices"].as_u64().unwrap() as usize,
    ));
    assert_eq!([ibm[13], ibm[16 + 13], ibm[32 + 13]], [0.0, -1.0, -2.0]);
    // the skinned node
    let skinned = nodes.iter().position(|x| x.get("skin").is_some()).unwrap();
    assert!(nodes[skinned].get("matrix").is_none() && nodes[skinned].get("translation").is_none());
    assert!(
        j["scenes"][0]["nodes"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(skinned))
    );
    assert_eq!(nodes[skinned]["weights"], serde_json::json!([0.25]));
    // the primitives' skin attributes
    let mesh = &j["meshes"][nodes[skinned]["mesh"].as_u64().unwrap() as usize];
    let prims = mesh["primitives"].as_array().unwrap();
    let joints0: &[u8] = accessor_bytes(
        &doc,
        prims[0]["attributes"]["JOINTS_0"].as_u64().unwrap() as usize,
    );
    assert_eq!(joints0, [0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]);
    let joints1: &[u8] = accessor_bytes(
        &doc,
        prims[1]["attributes"]["JOINTS_0"].as_u64().unwrap() as usize,
    );
    let joints1: Vec<[u8; 4]> = joints1.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    assert!(joints1.iter().all(|j| *j == [1, 0, 0, 0]), "{joints1:?}");
    let weights1: &[f32] = bytemuck::cast_slice(accessor_bytes(
        &doc,
        prims[1]["attributes"]["WEIGHTS_0"].as_u64().unwrap() as usize,
    ));
    assert_eq!(&weights1[..4], [1.0, 0.0, 0.0, 0.0]);
    // the morph target
    assert_eq!(mesh["weights"], serde_json::json!([0.0]));
    assert_eq!(mesh["extras"]["targetNames"], serde_json::json!(["bulge"]));
    let delta: &[f32] = bytemuck::cast_slice(accessor_bytes(
        &doc,
        prims[0]["targets"][0]["POSITION"].as_u64().unwrap() as usize,
    ));
    assert_eq!(delta, [0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert!(prims[1]["targets"][0]["POSITION"].is_number());
    // the animation
    let anim = &j["animations"][0];
    assert_eq!(anim["name"], "wave");
    assert_eq!(anim["extras"]["primase.loop"], true);
    let channels = anim["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(
        channels[0]["target"],
        serde_json::json!({"node": j3, "path": "rotation"})
    );
    assert_eq!(
        channels[1]["target"],
        serde_json::json!({"node": skinned, "path": "weights"})
    );
    assert_eq!(anim["samplers"][1]["interpolation"], "STEP");
    // what glTF has no form for
    assert_eq!((report.control_channels, report.root_motions), (1, 1));
    assert_eq!(
        (report.rigs, report.joint_nodes, report.skins, report.animations),
        (1, 3, 1, 1)
    );
    assert_eq!(
        report.components_unwritten,
        Default::default(),
        "{:?}",
        report.components_unwritten
    );
    let _ = json(&bytes);
}

/// glTF has no draw-group or named material-vector animation. Keep the drawable joint/morph
/// animation and report each omitted channel separately, rather than silently exporting a partial clip.
#[test]
fn display_channels_without_a_gltf_form_are_counted() {
    let mut w = world([128, 127, 0, 0]);
    let clip = w.iter::<Clip>().next().unwrap().0;
    let material = w.spawn();
    w.insert(material, primase_core::MaterialParameters(std::collections::BTreeMap::from([
        ("tint".into(), vec![0.25, 0.5]),
    ])));
    w.get_mut::<Clip>(clip).unwrap().channels.extend([
        Channel { target: ClipTarget::DrawGroup { scope: "character".into(), group: 4 },
            property: Property::Visibility, interpolation: Interpolation::Step,
            times: vec![0.0], values: vec![0.0] },
        Channel { target: ClipTarget::MaterialParameter { materials: vec![material], name: "tint".into() },
            property: Property::Values(2), interpolation: Interpolation::Linear,
            times: vec![0.0], values: vec![0.25, 0.5] },
    ]);
    let (bytes, report) = write(&w, &WriteOptions::default()).unwrap();
    assert_eq!((report.draw_group_channels, report.material_parameter_channels), (1, 1));
    assert_eq!((report.control_channels, report.animation_channels), (1, 2));
    let doc = json(&bytes);
    assert_eq!(doc["animations"][0]["channels"].as_array().unwrap().len(), 2);
    let text = report.to_string();
    assert!(text.contains("draw-group channels 1") && text.contains("material-parameter channels 1"), "{text}");
}

/// Weights that do not sum to 1 (bytes summing 253) are refused, naming the pass that normalises them; a skinned
/// prototype placed without a rig is drawn in its bind pose, without its skin, counted.
#[test]
fn unnormalised_weights_are_refused_and_a_rigless_placement_is_counted() {
    let w = world([128, 125, 0, 0]);
    let err = write(&w, &WriteOptions::default()).unwrap_err().to_string();
    assert!(err.contains("normalize-skin-weights"), "{err}");
    let mut w = world([128, 127, 0, 0]);
    let placed = w.iter::<SkinInstance>().next().unwrap().0;
    w.remove::<SkinInstance>(placed);
    let (bytes, report) = write(&w, &WriteOptions::default()).unwrap();
    assert_eq!(report.skinned_without_rig, 1);
    assert_eq!(report.skins, 0);
    let j = json(&bytes);
    assert!(
        j["meshes"][0]["primitives"][0]["attributes"]
            .get("JOINTS_0")
            .is_none()
    );
}

/// Oracle: the Khronos glTF-Validator (2.0.0-dev.3.10) on the character output: no errors and no warnings (padding
/// influences name joint 0, as it expects).
#[test]
fn khronos_validator_accepts_characters() {
    let Some(validator) = primase_testdata::tool(primase_testdata::Tool::GltfValidator) else {
        return;
    };
    let dir = scratch("characters-validator");
    let w = world([128, 127, 0, 0]);
    let (bytes, _) = write(&w, &WriteOptions::default()).unwrap();
    let path = dir.join("character.glb");
    std::fs::write(&path, bytes).unwrap();
    let out = std::process::Command::new(validator)
        .arg("-o")
        .arg(&path)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let issues = &report["issues"];
    println!("{}", serde_json::to_string_pretty(issues).unwrap());
    assert_eq!(issues["numErrors"], 0);
    let warnings: Vec<&str> = issues["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["severity"] == 1)
        .map(|m| m["code"].as_str().unwrap())
        .collect();
    assert!(warnings.is_empty(), "{warnings:?}");
}
