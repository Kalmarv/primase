//! Reader strictness: a small hand-built GLB reads; each change that could alter the geometry, or that the reader
//! does not understand, is an error naming the item.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use primase_core::{Semantic, Stream, Topology};
use primase_gltf::{Gltf, glb};
use serde_json::{Value, json};

/// Positions of one triangle, then unsigned-byte indices, then a unorm8 VEC2 texcoord with a 4-byte stride.
fn bin() -> Vec<u8> {
    let mut b = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&[0, 1, 2, 0]);
    b.extend_from_slice(&[0, 0, 9, 9, 255, 0, 9, 9, 0, 255, 9, 9]);
    b
}

fn doc() -> Value {
    json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": 52}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962},
            {"buffer": 0, "byteOffset": 36, "byteLength": 3, "target": 34963},
            {"buffer": 0, "byteOffset": 40, "byteLength": 12, "byteStride": 4},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
            {"bufferView": 1, "componentType": 5121, "count": 3, "type": "SCALAR"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2",
             "byteOffset": 0},
        ],
        "materials": [{"name": "m0"}],
        "meshes": [{"name": "tri", "primitives": [
            {"attributes": {"POSITION": 0, "TEXCOORD_0": 2}, "indices": 1, "material": 0, "mode": 4}
        ]}],
    })
}

fn glb_of(doc: &Value, bin: &[u8]) -> Vec<u8> {
    glb::assemble(&serde_json::to_vec(doc).unwrap(), bin).unwrap()
}

fn read(doc: &Value) -> Result<primase_gltf::ReadMesh, String> {
    Gltf::from_glb(&glb_of(doc, &bin()))
        .and_then(|g| g.read_mesh(0))
        .map_err(|e| e.to_string())
}

#[test]
fn reads_the_base_document() {
    let m = read(&doc()).unwrap();
    assert_eq!(m.name.as_deref(), Some("tri"));
    let p = &m.primitives[0];
    assert_eq!(p.material_name.as_deref(), Some("m0"));
    assert_eq!(p.primitive.topology, Topology::Triangles);
    assert_eq!(p.primitive.indices, Some(vec![0, 1, 2]));
    // unorm8 texcoords dequantize to c / 255 (the byte-strided view is read with its stride).
    assert_eq!(
        p.primitive.streams.get(&Semantic::TexCoord(0)),
        Some(&Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]))
    );
}

/// Each mutation of the base document, and the text its error must contain.
#[test]
fn rejects_what_it_does_not_read() {
    type Mutation = fn(&mut Value);
    let cases: Vec<(&str, Mutation, &str)> = vec![
        (
            "sparse",
            |d| d["accessors"][0]["sparse"] = json!({"count": 1}),
            "sparse",
        ),
        (
            "morph targets",
            |d| d["meshes"][0]["primitives"][0]["targets"] = json!([{}]),
            "targets",
        ),
        (
            "skin attribute",
            |d| d["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(0),
            "JOINTS_0",
        ),
        (
            "custom attribute",
            |d| d["meshes"][0]["primitives"][0]["attributes"]["_ID"] = json!(0),
            "_ID",
        ),
        (
            "aliasing set name",
            |d| d["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_01"] = json!(2),
            "TEXCOORD_01",
        ),
        (
            "strip",
            |d| d["meshes"][0]["primitives"][0]["mode"] = json!(5),
            "mode 5",
        ),
        (
            "draco",
            |d| d["meshes"][0]["primitives"][0]["extensions"] = json!({"KHR_draco_mesh_compression": {}}),
            "extensions",
        ),
        (
            "required extension",
            |d| d["extensionsRequired"] = json!(["EXT_meshopt_compression"]),
            "extensionsRequired",
        ),
        (
            "external buffer",
            |d| d["buffers"][0]["uri"] = json!("data.bin"),
            "uri",
        ),
        (
            "two buffers",
            |d| d["buffers"] = json!([{"byteLength": 52}, {"byteLength": 4}]),
            "more than one buffer",
        ),
        (
            "buffer length",
            |d| d["buffers"][0]["byteLength"] = json!(40),
            "byteLength 40",
        ),
        (
            "accessor past view",
            |d| d["accessors"][0]["count"] = json!(4),
            "runs past",
        ),
        (
            "wrong bounds",
            |d| d["accessors"][0]["max"] = json!([1, 1, 1]),
            "min/max",
        ),
        (
            "float indices",
            |d| d["accessors"][1]["componentType"] = json!(5120),
            "index component type 5120",
        ),
        (
            "strided indices",
            |d| {
                d["bufferViews"][1]["byteStride"] = json!(4);
                d["bufferViews"][1]["byteLength"] = json!(4);
                d["accessors"][1]["count"] = json!(1);
            },
            "must not have a byteStride",
        ),
        (
            "normalized float",
            |d| d["accessors"][0]["normalized"] = json!(true),
            "not allowed for Position",
        ),
        (
            "vec2 position",
            |d| d["accessors"][0]["type"] = json!("VEC2"),
            "not allowed for Position",
        ),
        (
            "unknown property",
            |d| d["accessors"][0]["bogus"] = json!(1),
            "bogus",
        ),
        (
            "no position",
            |d| {
                d["meshes"][0]["primitives"][0]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("POSITION");
            },
            "no POSITION",
        ),
        (
            "count mismatch",
            |d| d["accessors"][2]["count"] = json!(2),
            "count differs",
        ),
        (
            "version",
            |d| d["asset"]["version"] = json!("1.0"),
            "asset.version",
        ),
        (
            "missing material",
            |d| d["meshes"][0]["primitives"][0]["material"] = json!(3),
            "materials[3]",
        ),
        (
            "implicit zeros",
            |d| {
                d["accessors"][2].as_object_mut().unwrap().remove("bufferView");
            },
            "no bufferView",
        ),
        (
            "misaligned",
            |d| d["accessors"][0]["byteOffset"] = json!(2),
            "misaligned",
        ),
    ];
    for (name, mutate, needle) in cases {
        let mut d = doc();
        mutate(&mut d);
        match read(&d) {
            Ok(_) => panic!("{name}: read without error"),
            Err(e) => assert!(e.contains(needle), "{name}: {e}"),
        }
    }
}

#[test]
fn rejects_bad_indices_and_non_finite_values() {
    let mut b = bin();
    b[38] = 7;
    let e = Gltf::from_glb(&glb_of(&doc(), &b))
        .unwrap()
        .read_mesh(0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("index 2 is 7, 3 vertices"), "{e}");
    let mut b = bin();
    b[0..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let mut d = doc();
    d["accessors"][0].as_object_mut().unwrap().remove("min");
    let e = Gltf::from_glb(&glb_of(&d, &b))
        .unwrap()
        .read_mesh(0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not finite"), "{e}");
}

/// The buffer's byteLength is the file's word: one near the largest size, which padding to 4 bytes would overflow,
/// and lengths that disagree with the BIN chunk are errors, never a panic; an exact or padded length reads.
#[test]
fn buffer_length_is_checked() {
    let parse = |len: u64, bin: &[u8]| {
        let mut d = doc();
        d["buffers"][0]["byteLength"] = json!(len);
        Gltf::from_glb(&glb_of(&d, bin)).map(|_| ())
    };
    let b = bin();
    assert!(parse(52, &b).is_ok());
    assert!(parse(49, &b).is_ok(), "49 bytes pad to the 52-byte chunk");
    for len in [u64::MAX, u64::MAX - 1, u64::MAX - 2, u64::MAX - 3, 53, 48, 0] {
        let err = parse(len, &b).unwrap_err().to_string();
        assert!(
            err.contains("byteLength") || err.contains("too large"),
            "{len}: {err}"
        );
    }
    // an empty BIN chunk with a huge declared length
    assert!(parse(u64::MAX, &[]).is_err());
    // an empty buffer: whatever the accessors make of it, the length check doesn't panic
    let _ = parse(0, &[]);
}
