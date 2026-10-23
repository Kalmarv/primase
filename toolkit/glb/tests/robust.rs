//! Hostile numbers in a document are errors that name the item, never a panic, a wrap or a gigabyte: regression inputs
//! of the robustness census (a byteOffset of 2^64-1, an accessor with no view and a huge count, a long chain of nodes,
//! duplicate scene roots), and the typed accessor element of the encoder.

#![allow(clippy::unwrap_used)]

use primase_glb::{Buffers, ElementType, Gltf, MAX_MATERIALISED_ELEMENTS, assemble};
use serde_json::{Value, json};

fn gltf(doc: &Value, bin: &[u8]) -> Result<Gltf, primase_glb::Error> {
    Gltf::from_glb(&assemble(&serde_json::to_vec(doc).unwrap(), bin).unwrap())
}

/// One float VEC3 POSITION accessor over a 36-byte view, with the accessor's fields given.
fn positions(accessor: Value) -> Value {
    let mut a = json!({"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"});
    a.as_object_mut()
        .unwrap()
        .extend(accessor.as_object().unwrap().clone());
    json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": 36}],
        "bufferViews": [{"buffer": 0, "byteLength": 36}],
        "accessors": [a],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
    })
}

fn read_error(doc: &Value) -> String {
    gltf(doc, &[0; 36])
        .and_then(|g| g.read_mesh(0))
        .unwrap_err()
        .to_string()
}

/// A byteOffset of 2^64-1 on a one-byte-component accessor passed the alignment test, wrapped `offset + elem` past the
/// bound and then read an empty slice (census: InterpolationTest.glb with accessors[6].byteOffset = u64::MAX).
#[test]
fn a_byte_offset_that_overflows_is_refused() {
    let mut doc = positions(json!({"componentType": 5121, "type": "SCALAR", "byteOffset": u64::MAX}));
    doc["meshes"][0]["primitives"][0] = json!({"attributes": {"POSITION": 0}});
    let e = read_error(&doc);
    assert!(e.contains("accessors[0]"), "{e}");
    // and on the accessors the reader takes whole
    let g = gltf(&doc, &[0; 36]).unwrap();
    let e = g.accessor_data(0).unwrap_err().to_string();
    assert!(
        e.contains("runs past its buffer view") || e.contains("size overflows"),
        "{e}"
    );
}

/// An accessor without a bufferView is zeros, `count` of them: the count is the file's word, so it is bounded.
#[test]
fn a_viewless_accessor_with_a_huge_count_is_refused_before_it_allocates() {
    let doc = json!({
        "asset": {"version": "2.0"},
        "accessors": [{"componentType": 5126, "count": 100_000_000, "type": "VEC3"}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
    });
    let e = gltf(&doc, &[])
        .and_then(|g| g.read_mesh(0))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("accessors[0]") && e.contains("materialises at most"),
        "{e}"
    );
    // the ceiling itself: one past it is refused, a small one is zeros
    let over = json!({"accessors": [{"componentType": 5126, "count": MAX_MATERIALISED_ELEMENTS + 1, "type": "SCALAR"}]});
    let mut doc = doc.clone();
    doc["accessors"] = over["accessors"].clone();
    let g = gltf(&doc, &[]).unwrap();
    assert!(
        g.accessor_data(0)
            .unwrap_err()
            .to_string()
            .contains("materialises at most")
    );
    doc["accessors"][0]["count"] = json!(5);
    let data = gltf(&doc, &[]).unwrap().accessor_data(0).unwrap();
    assert_eq!(data.bytes, vec![0u8; 20]);
}

fn chain(n: usize, close: bool) -> Value {
    let nodes: Vec<Value> = (0..n)
        .map(|i| {
            if i + 1 < n {
                json!({"children": [i + 1]})
            } else if close {
                json!({"children": [0]})
            } else {
                json!({})
            }
        })
        .collect();
    json!({"asset": {"version": "2.0"}, "nodes": nodes, "scenes": [{"nodes": [0]}]})
}

/// The forest check walked up to n parents from every node: 80,000 chained nodes took 4.4 s, a million minutes. It is
/// linear now; the bound is a hundred times what that takes.
#[test]
fn a_long_chain_of_nodes_is_checked_in_linear_time() {
    let g = gltf(&chain(200_000, false), &[]).unwrap();
    let t = std::time::Instant::now();
    let nodes = g.nodes().unwrap();
    assert_eq!(nodes.len(), 200_000);
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

#[test]
fn a_cycle_of_children_is_still_refused() {
    let g = gltf(&chain(5, true), &[]).unwrap();
    let e = g.nodes().unwrap_err().to_string();
    assert!(e.contains("cycle") || e.contains("also a child"), "{e}");
    // a node under two parents, and a node its own child
    let two = json!({"asset": {"version": "2.0"}, "nodes": [{"children": [2]}, {"children": [2]}, {}]});
    assert!(
        gltf(&two, &[])
            .unwrap()
            .nodes()
            .unwrap_err()
            .to_string()
            .contains("also a child")
    );
    let own = json!({"asset": {"version": "2.0"}, "nodes": [{"children": [0]}]});
    assert!(
        gltf(&own, &[])
            .unwrap()
            .nodes()
            .unwrap_err()
            .to_string()
            .contains("own child")
    );
    // a tree that is not a chain is accepted whole
    let tree =
        json!({"asset": {"version": "2.0"}, "nodes": [{"children": [1, 2]}, {"children": [3]}, {}, {}]});
    assert_eq!(gltf(&tree, &[]).unwrap().nodes().unwrap().len(), 4);
}

/// `scene.nodes` [0, 0] read as two roots, so a reader placed the node twice; duplicate children were already refused.
#[test]
fn a_root_listed_twice_is_refused() {
    let doc = json!({"asset": {"version": "2.0"}, "nodes": [{}, {}], "scenes": [{"nodes": [0, 1, 0]}]});
    let e = gltf(&doc, &[]).unwrap().scene_roots().unwrap_err().to_string();
    assert!(
        e.contains("scenes[0]") && e.contains("root node 0 is listed twice"),
        "{e}"
    );
    let doc = json!({"asset": {"version": "2.0"}, "nodes": [{}, {}], "scenes": [{"nodes": [0, 1]}]});
    assert_eq!(gltf(&doc, &[]).unwrap().scene_roots().unwrap(), Some(vec![0, 1]));
}

/// The encoder takes the accessor's element as a type, so there is no string to get wrong.
#[test]
fn a_float_accessor_is_named_by_its_element_type() {
    let mut b = Buffers::default();
    let mat = b.float_accessor(&[0.0; 32], ElementType::Mat4, None, false);
    let vec2 = b.float_accessor(&[0.0; 6], ElementType::Vec2, None, true);
    let one = b.float_accessor(
        &[0.0; 5],
        ElementType::Scalar,
        Some((vec![0.0], vec![1.0])),
        false,
    );
    assert_eq!(
        (
            b.accessors[mat]["type"].clone(),
            b.accessors[mat]["count"].clone()
        ),
        (json!("MAT4"), json!(2))
    );
    assert_eq!(
        (
            b.accessors[vec2]["type"].clone(),
            b.accessors[vec2]["count"].clone()
        ),
        (json!("VEC2"), json!(3))
    );
    assert_eq!(b.accessors[one]["type"], json!("SCALAR"));
    for (e, name, width) in [
        (ElementType::Scalar, "SCALAR", 1),
        (ElementType::Vec2, "VEC2", 2),
        (ElementType::Vec3, "VEC3", 3),
        (ElementType::Vec4, "VEC4", 4),
        (ElementType::Mat4, "MAT4", 16),
    ] {
        assert_eq!((e.name(), e.components()), (name, width));
    }
}
