//! Sparse accessors, accessors without a view, skin attributes, morph targets, `floats` and MAT4 accessors, on
//! hand-built documents: what is read as stored or dequantized, and each refusal with the text its error must contain.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use primase_core::{Semantic, Stream};
use primase_glb::{ComponentType, ElementType, Gltf, ReadMesh};
use serde_json::{Value, json};

const BYTE: u64 = 5120;
const UNSIGNED_BYTE: u64 = 5121;
const SHORT: u64 = 5122;
const UNSIGNED_SHORT: u64 = 5123;
const UNSIGNED_INT: u64 = 5125;
const FLOAT: u64 = 5126;

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn i16s(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A document under construction: one buffer, its views and accessors.
#[derive(Default)]
struct Doc {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Doc {
    /// A view over `bytes` (the buffer stays 4-aligned).
    fn view(&mut self, bytes: &[u8], stride: Option<usize>) -> usize {
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(s) = stride {
            v["byteStride"] = json!(s);
        }
        self.bin.extend_from_slice(bytes);
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    /// An accessor over `bytes` in a view of its own.
    fn acc(&mut self, bytes: &[u8], component: u64, kind: &str, count: usize, normalized: bool) -> usize {
        let view = self.view(bytes, None);
        let mut a = json!({"bufferView": view, "componentType": component, "type": kind, "count": count});
        if normalized {
            a["normalized"] = json!(true);
        }
        self.raw(a)
    }

    fn raw(&mut self, a: Value) -> usize {
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn gltf(&self, meshes: Value) -> Gltf {
        let doc = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": self.bin.len()}],
            "bufferViews": self.views,
            "accessors": self.accessors,
            "meshes": meshes,
        });
        Gltf::from_glb(&primase_glb::assemble(&serde_json::to_vec(&doc).unwrap(), &self.bin).unwrap())
            .unwrap()
    }

    /// Mesh 0, one points primitive with these attributes (and targets), read.
    fn read(&self, attributes: Value, targets: Option<Value>) -> Result<ReadMesh, String> {
        let mut p = json!({"attributes": attributes, "mode": 0});
        if let Some(t) = targets {
            p["targets"] = t;
        }
        self.gltf(json!([{"primitives": [p]}]))
            .read_mesh(0)
            .map_err(|e| e.to_string())
    }
}

fn position(m: &ReadMesh) -> &Stream {
    &m.primitives[0].primitive.streams[&Semantic::Position]
}

/// Four positions, dense: (0,0,0) (1,0,0) (2,0,0) (3,0,0).
fn dense_positions(d: &mut Doc) -> usize {
    let v = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 3.0, 0.0, 0.0]);
    d.view(&v, None)
}

fn sparse_json(count: usize, idx_view: usize, idx_type: u64, values_view: usize) -> Value {
    json!({
        "count": count,
        "indices": {"bufferView": idx_view, "componentType": idx_type},
        "values": {"bufferView": values_view},
    })
}

#[test]
fn a_sparse_accessor_substitutes_into_the_dense_data() {
    let mut d = Doc::default();
    let dense = dense_positions(&mut d);
    let idx = d.view(&[1, 3], None);
    let vals = d.view(&f32s(&[0.0, 5.0, 0.0, 0.0, 7.0, 0.0]), None);
    // min/max describe the substituted data: a reader that checked the dense data would refuse them.
    let a = d.raw(json!({
        "bufferView": dense, "componentType": FLOAT, "type": "VEC3", "count": 4,
        "min": [0.0, 0.0, 0.0], "max": [2.0, 7.0, 0.0],
        "sparse": sparse_json(2, idx, UNSIGNED_BYTE, vals),
    }));
    let m = d.read(json!({"POSITION": a}), None).unwrap();
    assert_eq!(
        position(&m),
        &Stream::F32x3(vec![[0.0; 3], [0.0, 5.0, 0.0], [2.0, 0.0, 0.0], [0.0, 7.0, 0.0]])
    );
    // accessor_data sees the same elements
    let data = d.gltf(json!([])).accessor_data(a).unwrap();
    assert_eq!(
        (data.count, data.element, data.component),
        (4, ElementType::Vec3, ComponentType::F32)
    );
    assert_eq!(data.bytes.len(), 48);
}

#[test]
fn a_sparse_accessor_over_a_strided_view_is_tightly_packed() {
    let mut d = Doc::default();
    // 16-byte stride: the vec3 and 4 bytes of something else
    let mut v = Vec::new();
    for k in 0..3 {
        v.extend(f32s(&[k as f32, 0.0, 0.0, 99.0]));
    }
    let dense = d.view(&v, Some(16));
    let idx = d.view(&u16s(&[2]), None);
    let vals = d.view(&f32s(&[9.0, 9.0, 9.0]), None);
    let a = d.raw(json!({
        "bufferView": dense, "componentType": FLOAT, "type": "VEC3", "count": 3,
        "sparse": sparse_json(1, idx, UNSIGNED_SHORT, vals),
    }));
    let m = d.read(json!({"POSITION": a}), None).unwrap();
    assert_eq!(
        position(&m),
        &Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [9.0; 3]])
    );
}

#[test]
fn an_accessor_without_a_view_is_zeros_and_sparse_substitutes_into_them() {
    let mut d = Doc::default();
    let zeros = d.raw(json!({"componentType": FLOAT, "type": "VEC3", "count": 3}));
    let idx = d.view(&1u32.to_le_bytes(), None);
    let vals = d.view(&f32s(&[4.0, 5.0, 6.0]), None);
    let sparse = d.raw(json!({
        "componentType": FLOAT, "type": "VEC3", "count": 3,
        "sparse": sparse_json(1, idx, UNSIGNED_INT, vals),
    }));
    let m = d.read(json!({"POSITION": zeros}), None).unwrap();
    assert_eq!(position(&m), &Stream::F32x3(vec![[0.0; 3]; 3]));
    let m = d.read(json!({"POSITION": sparse}), None).unwrap();
    assert_eq!(
        position(&m),
        &Stream::F32x3(vec![[0.0; 3], [4.0, 5.0, 6.0], [0.0; 3]])
    );
}

#[test]
fn sparse_accessors_are_checked() {
    // (what the case changes, the text the error must contain)
    let cases: [(&str, &[u8], usize, u64, &str); 6] = [
        ("equal indices", &[1, 1], 2, UNSIGNED_BYTE, "does not increase"),
        (
            "decreasing indices",
            &[2, 1],
            2,
            UNSIGNED_BYTE,
            "does not increase",
        ),
        ("index past the count", &[1, 4], 2, UNSIGNED_BYTE, "4 elements"),
        ("count of zero", &[1], 0, UNSIGNED_BYTE, "outside 1-4"),
        (
            "count above the accessor's",
            &[0, 1, 2, 3, 3],
            5,
            UNSIGNED_BYTE,
            "outside 1-4",
        ),
        ("indices run past their view", &[1], 2, UNSIGNED_BYTE, "runs past"),
    ];
    for (what, indices, count, idx_type, text) in cases {
        let mut d = Doc::default();
        let dense = dense_positions(&mut d);
        let idx = d.view(indices, None);
        let vals = d.view(&f32s(&[1.0; 15]), None);
        let a = d.raw(json!({
            "bufferView": dense, "componentType": FLOAT, "type": "VEC3", "count": 4,
            "sparse": sparse_json(count, idx, idx_type, vals),
        }));
        let e = d.read(json!({"POSITION": a}), None).unwrap_err();
        assert!(e.contains(text), "{what}: {e}");
        assert!(e.contains("accessors[0]"), "{what}: {e}");
    }

    // a stride on the index view or on the values view
    for (on_indices, name) in [(true, "indices"), (false, "values")] {
        let mut d = Doc::default();
        let dense = dense_positions(&mut d);
        let idx = d.view(&[0, 0, 0, 0, 1, 0, 0, 0], on_indices.then_some(4));
        let vals = d.view(&f32s(&[1.0; 6]), (!on_indices).then_some(12));
        let a = d.raw(json!({
            "bufferView": dense, "componentType": FLOAT, "type": "VEC3", "count": 4,
            "sparse": sparse_json(1, idx, UNSIGNED_BYTE, vals),
        }));
        let e = d.read(json!({"POSITION": a}), None).unwrap_err();
        assert!(e.contains("byteStride") && e.contains(name), "{name}: {e}");
    }

    // an unknown property of the sparse object, and a byteOffset with no view
    let mut d = Doc::default();
    let idx = d.view(&[0], None);
    let vals = d.view(&f32s(&[1.0; 3]), None);
    let mut sparse = sparse_json(1, idx, UNSIGNED_BYTE, vals);
    sparse["extra"] = json!(1);
    let a = d.raw(json!({"componentType": FLOAT, "type": "VEC3", "count": 1, "sparse": sparse}));
    let e = d.read(json!({"POSITION": a}), None).unwrap_err();
    assert!(e.contains("unsupported property"), "{e}");
    let a = d.raw(json!({"componentType": FLOAT, "type": "VEC3", "count": 1, "byteOffset": 4}));
    let e = d.read(json!({"POSITION": a}), None).unwrap_err();
    assert!(e.contains("byteOffset without a bufferView"), "{e}");
}

/// Four positions, plus the skin attributes `joints` and `weights` as given.
fn skinned(joints: (&[u8], u64, bool), weights: (&[u8], u64, bool)) -> Result<ReadMesh, String> {
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let j = d.acc(joints.0, joints.1, "VEC4", 2, joints.2);
    let w = d.acc(weights.0, weights.1, "VEC4", 2, weights.2);
    d.read(json!({"POSITION": pos, "JOINTS_0": j, "WEIGHTS_0": w}), None)
}

#[test]
fn skin_attributes_are_kept_as_stored() {
    let j8: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
    let j16 = u16s(&[0, 1, 2, 300, 4, 5, 6, 7]);
    let w32 = f32s(&[0.5, 0.25, 0.25, 0.0, 1.0, 0.0, 0.0, 0.0]);
    let w16 = u16s(&[32768, 16384, 16383, 0, 65535, 0, 0, 0]);
    let weights = [
        ((&w32[..], FLOAT, false), "f32"),
        ((j8, UNSIGNED_BYTE, true), "u8"),
        ((&w16[..], UNSIGNED_SHORT, true), "u16"),
    ];
    for (joints, jname) in [
        ((j8, UNSIGNED_BYTE, false), "u8"),
        ((&j16[..], UNSIGNED_SHORT, false), "u16"),
    ] {
        for (w, wname) in weights {
            let m = skinned(joints, w).unwrap_or_else(|e| panic!("joints {jname}, weights {wname}: {e}"));
            let streams = &m.primitives[0].primitive.streams;
            match (&streams[&Semantic::Joints(0)], jname) {
                (Stream::U8x4(v), "u8") => assert_eq!(v, &vec![[0, 1, 2, 3], [4, 5, 6, 7]]),
                (Stream::U16x4(v), "u16") => assert_eq!(v, &vec![[0, 1, 2, 300], [4, 5, 6, 7]]),
                (s, _) => panic!("joints {jname} read as {s:?}"),
            }
            match (&streams[&Semantic::Weights(0)], wname) {
                (Stream::F32x4(v), "f32") => {
                    assert_eq!(v, &vec![[0.5, 0.25, 0.25, 0.0], [1.0, 0.0, 0.0, 0.0]])
                }
                (Stream::U8x4(v), "u8") => assert_eq!(v, &vec![[0, 1, 2, 3], [4, 5, 6, 7]]),
                (Stream::U16x4(v), "u16") => assert_eq!(v, &vec![[32768, 16384, 16383, 0], [65535, 0, 0, 0]]),
                (s, _) => panic!("weights {wname} read as {s:?}"),
            }
        }
    }
}

#[test]
fn skin_attributes_are_checked() {
    let j8: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
    let w32 = f32s(&[0.5, 0.25, 0.25, 0.0, 1.0, 0.0, 0.0, 0.0]);
    let j32 = f32s(&[0.0; 8]);
    // normalized joints
    let e = skinned((j8, UNSIGNED_BYTE, true), (&w32, FLOAT, false)).unwrap_err();
    assert!(e.contains("JOINTS_0") && e.contains("not allowed"), "{e}");
    // float joints
    let e = skinned((&j32, FLOAT, false), (&w32, FLOAT, false)).unwrap_err();
    assert!(e.contains("not allowed"), "{e}");
    // unnormalized integer weights
    let e = skinned((j8, UNSIGNED_BYTE, false), (j8, UNSIGNED_BYTE, false)).unwrap_err();
    assert!(e.contains("WEIGHTS_0") && e.contains("not allowed"), "{e}");

    // a set without its partner, either way round
    for (attr, partner) in [("JOINTS_0", "Weights(0)"), ("WEIGHTS_0", "Joints(0)")] {
        let mut d = Doc::default();
        let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
        let a = if attr == "JOINTS_0" {
            d.acc(j8, UNSIGNED_BYTE, "VEC4", 2, false)
        } else {
            d.acc(&w32, FLOAT, "VEC4", 2, false)
        };
        let e = d.read(json!({"POSITION": pos, attr: a}), None).unwrap_err();
        assert!(e.contains(partner), "{attr}: {e}");
    }
    // JOINTS_1 pairs with WEIGHTS_1, not WEIGHTS_0
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let j = d.acc(j8, UNSIGNED_BYTE, "VEC4", 2, false);
    let w = d.acc(&w32, FLOAT, "VEC4", 2, false);
    let e = d
        .read(json!({"POSITION": pos, "JOINTS_1": j, "WEIGHTS_0": w}), None)
        .unwrap_err();
    assert!(e.contains("Joints(1)") && e.contains("Weights(1)"), "{e}");
    // a non-canonical spelling would alias JOINTS_1
    let e = d
        .read(json!({"POSITION": pos, "JOINTS_01": j, "WEIGHTS_01": w}), None)
        .unwrap_err();
    assert!(e.contains("unsupported attribute JOINTS_01"), "{e}");
    // a count that differs from POSITION's
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 9]), FLOAT, "VEC3", 3, false);
    let j = d.acc(j8, UNSIGNED_BYTE, "VEC4", 2, false);
    let w = d.acc(&w32, FLOAT, "VEC4", 2, false);
    let e = d
        .read(json!({"POSITION": pos, "JOINTS_0": j, "WEIGHTS_0": w}), None)
        .unwrap_err();
    assert!(e.contains("count differs from POSITION"), "{e}");
}

#[test]
fn morph_targets_read_float_and_normalized_deltas() {
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let fpos = d.acc(&f32s(&[0.5, -1.0, 2.0, 0.0, 0.0, 0.25]), FLOAT, "VEC3", 2, false);
    // 32767 -> 1, -32768 -> -1 (clamped, not -1.00003), 16384 -> 16384 / 32767, 0
    let spos = d.acc(
        &i16s(&[32767, -32768, 16384, 0, -32767, 1]),
        SHORT,
        "VEC3",
        2,
        true,
    );
    let bytes: Vec<u8> = [127i8, -128, 64, 0, -127, 1].iter().map(|b| *b as u8).collect();
    let bnorm = d.acc(&bytes, BYTE, "VEC3", 2, true);
    let tang = d.acc(&f32s(&[1.0; 6]), FLOAT, "VEC3", 2, false);
    let targets = json!([
        {"POSITION": fpos, "TANGENT": tang},
        {"POSITION": spos, "NORMAL": bnorm},
        {},
    ]);
    let m = d.read(json!({"POSITION": pos}), Some(targets)).unwrap();
    let t = &m.primitives[0].targets;
    assert_eq!(t.len(), 3);
    assert_eq!(t[0].position, Some(vec![[0.5, -1.0, 2.0], [0.0, 0.0, 0.25]]));
    assert_eq!(t[0].tangent, Some(vec![[1.0; 3]; 2]));
    assert_eq!(t[0].normal, None);
    let q = |c: i16| (f64::from(c) / 32767.0).max(-1.0) as f32;
    assert_eq!(
        t[1].position,
        Some(vec![[1.0, -1.0, q(16384)], [0.0, -1.0, q(1)]]),
        "snorm16 deltas"
    );
    let q = |c: i8| (f64::from(c) / 127.0).max(-1.0) as f32;
    assert_eq!(
        t[1].normal,
        Some(vec![[1.0, -1.0, q(64)], [0.0, -1.0, q(1)]]),
        "snorm8 deltas"
    );
    assert_eq!(t[2], primase_glb::ReadTarget::default());
    assert_eq!(m.weights, None);
}

#[test]
fn morph_targets_over_sparse_accessors() {
    // The usual writer's form: a target's delta is mostly zeros.
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 9]), FLOAT, "VEC3", 3, false);
    let idx = d.view(&[2], None);
    let vals = d.view(&f32s(&[0.0, 0.0, 3.0]), None);
    let delta = d.raw(json!({
        "componentType": FLOAT, "type": "VEC3", "count": 3,
        "sparse": sparse_json(1, idx, UNSIGNED_BYTE, vals),
    }));
    let m = d
        .read(json!({"POSITION": pos}), Some(json!([{"POSITION": delta}])))
        .unwrap();
    assert_eq!(
        m.primitives[0].targets[0].position,
        Some(vec![[0.0; 3], [0.0; 3], [0.0, 0.0, 3.0]])
    );
}

#[test]
fn morph_targets_are_checked() {
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let three = d.acc(&f32s(&[0.0; 9]), FLOAT, "VEC3", 3, false);
    let ok = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let vec2 = d.acc(&f32s(&[0.0; 4]), FLOAT, "VEC2", 2, false);
    let ushort = d.acc(&u16s(&[0; 6]), UNSIGNED_SHORT, "VEC3", 2, true);
    let short_raw = d.acc(&i16s(&[0; 6]), SHORT, "VEC3", 2, false);
    let attrs = json!({"POSITION": pos});
    let cases = [
        (
            "a delta count differing from POSITION's",
            json!([{"POSITION": three}]),
            "3 elements, POSITION 2",
        ),
        (
            "a TEXCOORD_0 delta",
            json!([{"TEXCOORD_0": ok}]),
            "unsupported morph target attribute TEXCOORD_0",
        ),
        (
            "an application-specific delta",
            json!([{"_X": ok}]),
            "unsupported morph target attribute _X",
        ),
        (
            "a VEC2 delta",
            json!([{"POSITION": vec2}]),
            "not allowed for a morph target delta",
        ),
        (
            "a normalized unsigned delta",
            json!([{"POSITION": ushort}]),
            "not allowed for a morph target delta",
        ),
        (
            "an unnormalized short delta",
            json!([{"POSITION": short_raw}]),
            "not allowed for a morph target delta",
        ),
        ("no targets", json!([]), "not a non-empty array"),
        ("a target that is no object", json!([1]), "not an object"),
    ];
    for (what, targets, text) in cases {
        let e = d.read(attrs.clone(), Some(targets)).unwrap_err();
        assert!(e.contains(text), "{what}: {e}");
    }
}

#[test]
fn morph_target_counts_and_default_weights_are_checked() {
    let mut d = Doc::default();
    let pos = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let delta = d.acc(&f32s(&[0.0; 6]), FLOAT, "VEC3", 2, false);
    let target = json!({"POSITION": delta});
    let prim = |targets: usize| {
        let mut p = json!({"attributes": {"POSITION": pos}, "mode": 0});
        if targets > 0 {
            p["targets"] = json!(vec![target.clone(); targets]);
        }
        p
    };
    let read = |prims: Vec<Value>, weights: Option<Value>| {
        let mut m = json!({"primitives": prims});
        if let Some(w) = weights {
            m["weights"] = w;
        }
        d.gltf(json!([m])).read_mesh(0).map_err(|e| e.to_string())
    };

    // the same number of targets on each primitive, and the default weights
    let m = read(vec![prim(2), prim(2)], Some(json!([0.5, 1.0]))).unwrap();
    assert_eq!(m.weights, Some(vec![0.5, 1.0]));
    assert!(m.primitives.iter().all(|p| p.targets.len() == 2));
    let m = read(vec![prim(0)], None).unwrap();
    assert_eq!((m.weights, m.primitives[0].targets.len()), (None, 0));

    let e = read(vec![prim(2), prim(1)], None).unwrap_err();
    assert!(
        e.contains("primitive 1 has 1 morph targets, primitive 0 has 2"),
        "{e}"
    );
    let e = read(vec![prim(0), prim(2)], None).unwrap_err();
    assert!(e.contains("primitive 1 has 2 morph targets"), "{e}");
    let e = read(vec![prim(2)], Some(json!([0.5]))).unwrap_err();
    assert!(e.contains("1 weights for 2 morph targets"), "{e}");
    let e = read(vec![prim(0)], Some(json!([0.5]))).unwrap_err();
    assert!(e.contains("1 weights for 0 morph targets"), "{e}");
    let e = read(vec![prim(1)], Some(json!(["x"]))).unwrap_err();
    assert!(e.contains("weights holds a non-number"), "{e}");
}

#[test]
fn floats_decodes_each_component_type() {
    let mut d = Doc::default();
    let f = d.acc(&f32s(&[0.5, -2.0, 3.25, 1e30]), FLOAT, "VEC2", 2, false);
    let u8n = d.acc(&[0, 51, 255, 5], UNSIGNED_BYTE, "SCALAR", 4, true);
    let u16n = d.acc(&u16s(&[0, 65535, 1000, 32768]), UNSIGNED_SHORT, "VEC2", 2, true);
    let i8n = d.acc(&[127u8, 0x80, 0x81, 64], BYTE, "SCALAR", 4, true);
    let i16n = d.acc(&i16s(&[32767, -32768, -32767, 16384]), SHORT, "VEC4", 1, true);
    let raw = d.acc(&[1, 2, 3, 4], UNSIGNED_BYTE, "SCALAR", 4, false);
    let raw_int = d.acc(&1u32.to_le_bytes(), UNSIGNED_INT, "SCALAR", 1, false);
    let nan = d.acc(&f32s(&[1.0, f32::NAN]), FLOAT, "SCALAR", 2, false);
    let mat = d.acc(
        &f32s(&(0..32).map(|i| i as f32).collect::<Vec<_>>()),
        FLOAT,
        "MAT4",
        2,
        false,
    );
    let mat8 = d.acc(&[0; 16], UNSIGNED_BYTE, "MAT4", 1, false);
    // a strided view is read with its stride
    let strided_view = d.view(&f32s(&[1.0, 9.0, 2.0, 9.0, 3.0, 9.0]), Some(8));
    let strided =
        d.raw(json!({"bufferView": strided_view, "componentType": FLOAT, "type": "SCALAR", "count": 3}));
    let g = d.gltf(json!([]));

    assert_eq!(
        g.floats(f).unwrap(),
        (ElementType::Vec2, vec![0.5, -2.0, 3.25, 1e30])
    );
    let (e, v) = g.floats(u8n).unwrap();
    assert_eq!(e, ElementType::Scalar);
    assert_eq!(v, [0.0, (51.0f64 / 255.0) as f32, 1.0, (5.0f64 / 255.0) as f32]);
    assert_eq!(v[1], 0.2);
    let (e, v) = g.floats(u16n).unwrap();
    assert_eq!(e, ElementType::Vec2);
    assert_eq!(
        v,
        [
            0.0,
            1.0,
            (1000.0f64 / 65535.0) as f32,
            (32768.0f64 / 65535.0) as f32
        ]
    );
    let (_, v) = g.floats(i8n).unwrap();
    assert_eq!(
        v,
        [1.0, -1.0, -1.0, (64.0f64 / 127.0) as f32],
        "-128 clamps to -1"
    );
    let (e, v) = g.floats(i16n).unwrap();
    assert_eq!(e, ElementType::Vec4);
    assert_eq!(v, [1.0, -1.0, -1.0, (16384.0f64 / 32767.0) as f32]);
    assert_eq!(g.floats(strided).unwrap().1, [1.0, 2.0, 3.0]);

    let (e, v) = g.floats(mat).unwrap();
    assert_eq!(e, ElementType::Mat4);
    assert_eq!(v, (0..32).map(|i| i as f32).collect::<Vec<_>>());
    let data = g.accessor_data(mat).unwrap();
    assert_eq!(
        (data.element, data.count, data.bytes.len()),
        (ElementType::Mat4, 2, 128)
    );

    for (acc, text) in [
        (raw, "no float form"),
        (raw_int, "no float form"),
        (nan, "not finite"),
        (mat8, "MAT4 is read as FLOAT only"),
    ] {
        let e = g.floats(acc).unwrap_err().to_string();
        assert!(
            e.contains(text) && e.contains(&format!("accessors[{acc}]")),
            "{e}"
        );
    }
    assert!(g.floats(99).unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn a_mat4_is_not_an_attribute() {
    let mut d = Doc::default();
    let m = d.acc(&f32s(&[0.0; 16]), FLOAT, "MAT4", 1, false);
    let e = d.read(json!({"POSITION": m}), None).unwrap_err();
    assert!(e.contains("MAT4") && e.contains("not allowed"), "{e}");
}
