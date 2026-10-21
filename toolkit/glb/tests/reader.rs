//! Reader strictness: a small hand-built GLB reads, and the same document as a `.gltf` with its buffer supplied;
//! each change that could alter the geometry, or that the reader does not understand, is an error naming the item.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use primase_core::{Semantic, Stream, Topology};
use primase_glb::{AccessorData, ComponentType, ElementType, Gltf};
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
    primase_glb::assemble(&serde_json::to_vec(doc).unwrap(), bin).unwrap()
}

fn read(doc: &Value) -> Result<primase_glb::ReadMesh, String> {
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
            "sparse without indices",
            |d| d["accessors"][0]["sparse"] = json!({"count": 1}),
            "no indices",
        ),
        (
            "morph target of another kind",
            |d| d["meshes"][0]["primitives"][0]["targets"] = json!([{"TEXCOORD_0": 2}]),
            "unsupported morph target attribute TEXCOORD_0",
        ),
        (
            "skin attribute of the wrong type",
            |d| d["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(0),
            "not allowed for Joints(0)",
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

/// The base document as a `.gltf`: its buffer named by a uri.
fn gltf_doc() -> Value {
    let mut d = doc();
    d["buffers"][0]["uri"] = json!("tri.bin");
    d
}

fn from_gltf(d: &Value, bin: Vec<u8>) -> Result<Gltf, String> {
    let mut bin = Some(bin);
    Gltf::from_gltf(&serde_json::to_vec(d).unwrap(), |i, uri| {
        assert_eq!((i, uri), (0, "tri.bin"));
        bin.take().ok_or_else(|| "asked twice".to_owned())
    })
    .map_err(|e| e.to_string())
}

/// A `.gltf` whose buffer the caller supplies reads as the same document in a GLB does.
#[test]
fn a_gltf_reads_with_the_buffer_the_caller_supplies() {
    let g = from_gltf(&gltf_doc(), bin()).unwrap();
    assert_eq!(g.read_mesh(0).unwrap(), read(&doc()).unwrap());
    assert_eq!(g.buffer(0).unwrap(), bin().as_slice());
    assert!(g.buffer(1).is_err());
}

/// A `.gltf`'s buffers: each needs a uri and exactly its byteLength bytes; the caller's error names the uri.
#[test]
fn a_gltf_buffer_is_checked() {
    let mut d = gltf_doc();
    d["buffers"][0].as_object_mut().unwrap().remove("uri");
    let e = from_gltf(&d, bin()).unwrap_err();
    assert!(e.contains("buffers[0]") && e.contains("no uri"), "{e}");
    let e = from_gltf(&gltf_doc(), bin()[..48].to_vec()).unwrap_err();
    assert!(
        e.contains("byteLength 52 does not match the 48-byte buffer tri.bin"),
        "{e}"
    );
    let e = Gltf::from_gltf(&serde_json::to_vec(&gltf_doc()).unwrap(), |_, _| {
        Err("not in the archive".to_owned())
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("tri.bin: not in the archive"), "{e}");
    let mut d = gltf_doc();
    d["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    assert!(from_gltf(&d, bin()).unwrap_err().contains("extensionsRequired"));
    // used, not required: the caller reads it from the JSON
    let mut d = gltf_doc();
    d["extensionsUsed"] = json!(["KHR_materials_specular"]);
    d["materials"][0]["extensions"] = json!({"KHR_materials_specular": {"specularFactor": 0.5}});
    assert!(from_gltf(&d, bin()).unwrap().read_mesh(0).is_ok());
}

/// Views pick their buffer: positions in a second buffer read from it, and a view naming no buffer is an error.
#[test]
fn views_read_the_buffer_they_name() {
    let mut d = gltf_doc();
    d["buffers"] = json!([{"byteLength": 52, "uri": "tri.bin"}, {"byteLength": 36, "uri": "far.bin"}]);
    d["bufferViews"][0] = json!({"buffer": 1, "byteOffset": 0, "byteLength": 36});
    let mut far = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0] {
        far.extend_from_slice(&v.to_le_bytes());
    }
    d["accessors"][0]["max"] = json!([2, 2, 0]);
    let json = serde_json::to_vec(&d).unwrap();
    let g = Gltf::from_gltf(&json, |i, _| Ok(if i == 0 { bin() } else { far.clone() })).unwrap();
    assert_eq!(
        g.read_mesh(0).unwrap().primitives[0]
            .primitive
            .streams
            .get(&Semantic::Position),
        Some(&Stream::F32x3(vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 2.0, 0.0]
        ]))
    );
    d["bufferViews"][0]["buffer"] = json!(2);
    let json = serde_json::to_vec(&d).unwrap();
    let e = Gltf::from_gltf(&json, |i, _| Ok(if i == 0 { bin() } else { far.clone() }))
        .unwrap()
        .read_mesh(0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("buffer 2 does not exist"), "{e}");
}

/// An application-specific attribute (`_...`) is returned as stored, its view's stride removed; its count must be
/// POSITION's, and a normalized float is not a glTF accessor.
#[test]
fn application_specific_attributes_are_returned_as_stored() {
    let mut d = doc();
    d["meshes"][0]["primitives"][0]["attributes"]["_COLOR_SHIFT"] = json!(2);
    let m = read(&d).unwrap();
    assert_eq!(
        m.primitives[0].custom["_COLOR_SHIFT"],
        AccessorData {
            component: ComponentType::U8,
            normalized: true,
            element: ElementType::Vec2,
            count: 3,
            bytes: vec![0, 0, 255, 0, 0, 255],
        }
    );
    // the same accessor still serves TEXCOORD_0; the custom attribute is not a stream
    assert_eq!(m.primitives[0].primitive.streams.len(), 2);
    let mut short = d.clone();
    short["accessors"][2]["count"] = json!(2);
    short["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("TEXCOORD_0");
    let e = read(&short).unwrap_err();
    assert!(e.contains("_COLOR_SHIFT count differs from POSITION's 3"), "{e}");
    let mut float = d.clone();
    float["accessors"].as_array_mut().unwrap().push(json!(
        {"bufferView": 0, "componentType": 5126, "normalized": true, "count": 3, "type": "VEC3"}
    ));
    float["meshes"][0]["primitives"][0]["attributes"]["_COLOR_SHIFT"] = json!(3);
    let e = read(&float).unwrap_err();
    assert!(
        e.contains("_COLOR_SHIFT") && e.contains("F32 is normalized"),
        "{e}"
    );
}
