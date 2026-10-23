//! The reader refuses by name whatever the world's own verifier would refuse, and a reference it cannot follow; `extras` it
//! keeps nowhere are counted. Regression inputs of the robustness census (a skin slot outside its palette, a negative weight,
//! a rotation that is not a unit quaternion) and of the review (a sampler or texture that is not there).

#![allow(clippy::unwrap_used)]

use primase_core::World;
use primase_gltf::read_glb;
use serde_json::{Value, json};

/// A skinned triangle: three float positions, JOINTS_0 (u8x4 per vertex) and WEIGHTS_0 (f32x4 per vertex), one joint node.
fn skinned(joints: [u8; 4], weights: [f32; 4]) -> (Value, Vec<u8>) {
    let mut bin = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bin.extend(v.to_le_bytes());
    }
    for _ in 0..3 {
        bin.extend(joints);
    }
    for _ in 0..3 {
        for w in weights {
            bin.extend(w.to_le_bytes());
        }
    }
    let doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0, 1]}],
        "nodes": [{"mesh": 0, "skin": 0}, {"name": "root"}],
        "skins": [{"joints": [1]}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2}}]}],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 12},
            {"buffer": 0, "byteOffset": 48, "byteLength": 48},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
            {"bufferView": 1, "componentType": 5121, "count": 3, "type": "VEC4"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
        ],
    });
    (doc, bin)
}

fn read(doc: &Value, bin: &[u8]) -> Result<primase_gltf::ReadReport, String> {
    let glb = primase_glb::assemble(&serde_json::to_vec(doc).unwrap(), bin).unwrap();
    read_glb(&glb, "t.glb", &mut World::new()).map_err(|e| e.to_string())
}

#[test]
fn a_valid_skinned_document_reads() {
    let (doc, bin) = skinned([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    let report = read(&doc, &bin).unwrap();
    assert_eq!((report.bindings, report.joints), (1, 1));
}

/// JOINTS_0 naming slot 5 of a palette of one: the glb reader does not check joint indices ("a reader reads faithfully"), the
/// world's verifier refuses it, and `read` used to hand it back (269 of the census's mutants).
#[test]
fn a_joint_outside_its_skins_palette_is_refused() {
    let (doc, bin) = skinned([5, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    let e = read(&doc, &bin).unwrap_err();
    assert!(
        e.contains("core.skin") && e.contains("does not verify") && e.contains("slot 5"),
        "{e}"
    );
}

#[test]
fn a_negative_weight_is_refused() {
    let (doc, bin) = skinned([0, 0, 0, 0], [-1.0, 2.0, 0.0, 0.0]);
    let e = read(&doc, &bin).unwrap_err();
    assert!(e.contains("core.skin") && e.contains("negative"), "{e}");
}

/// A joint node whose rotation is not a unit quaternion (glTF requires one).
#[test]
fn a_joint_rotation_that_is_not_a_unit_quaternion_is_refused() {
    let (mut doc, bin) = skinned([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    doc["nodes"][1]["rotation"] = json!([0, 0, 0, 2]);
    let e = read(&doc, &bin).unwrap_err();
    assert!(
        e.contains("core.skeleton") && e.contains("not a unit quaternion") && e.contains("skeleton root"),
        "{e}"
    );
}

/// A texture, image or sampler the document names and does not have is an error, not a slot quietly left bare.
#[test]
fn a_texture_reference_that_leads_nowhere_is_refused() {
    let base = |texture: Value, extra: Value| {
        let mut doc = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
            "buffers": [{"byteLength": 36}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [0, 0, 0]}],
            "images": [{"uri": "a.png"}],
            "textures": [texture],
        });
        doc.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        doc
    };
    let bin = vec![0u8; 36];
    // control: a texture with its image and no sampler reads (the image is refused by name, no resolver)
    assert!(read(&base(json!({"source": 0}), json!({})), &bin).is_ok());
    let e = read(&base(json!({"source": 7}), json!({})), &bin).unwrap_err();
    assert!(
        e.contains("textures[0]") && e.contains("image 7 does not exist"),
        "{e}"
    );
    let e = read(
        &base(json!({"source": 0, "sampler": 3}), json!({"samplers": [{}]})),
        &bin,
    )
    .unwrap_err();
    assert!(e.contains("sampler 3 does not exist"), "{e}");
    let e = read(
        &base(
            json!({"source": 0, "sampler": 0}),
            json!({"samplers": [{"minFilter": 1}]}),
        ),
        &bin,
    )
    .unwrap_err();
    assert!(
        e.contains("samplers[0]") && e.contains("minification filter"),
        "{e}"
    );
    let mut doc = base(json!({"source": 0}), json!({}));
    doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["index"] = json!(4);
    let e = read(&doc, &bin).unwrap_err();
    assert!(e.contains("texture 4 does not exist"), "{e}");
}

/// Extras are not dropped silently: the reader counts those it keeps nowhere, and a mesh's `targetNames` are read, not
/// counted. An object with no home for them (an accessor) refuses them.
#[test]
fn extras_are_counted_or_refused() {
    let (mut doc, bin) = skinned([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    doc["scenes"][0]["extras"] = json!({"a": 1});
    doc["nodes"][0]["extras"] = json!({"b": 2});
    doc["skins"][0]["extras"] = json!({"c": 3});
    doc["meshes"][0]["primitives"][0]["extras"] = json!({"d": 4});
    doc["meshes"][0]["extras"] = json!({"targetNames": []});
    assert_eq!(read(&doc, &bin).unwrap().extras_not_read, 4);
    doc["meshes"][0]["extras"] = json!({"targetNames": [], "other": true});
    assert_eq!(read(&doc, &bin).unwrap().extras_not_read, 5);
    doc["accessors"][0]["extras"] = json!({"e": 5});
    let e = read(&doc, &bin).unwrap_err();
    assert!(
        e.contains("accessors[0]") && e.contains("unsupported property \"extras\""),
        "{e}"
    );
}
