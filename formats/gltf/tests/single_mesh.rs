//! primase-glb's single-mesh encoder writes what the writer writes for an unplaced prototype whose primitives have no
//! material, apart from the generator, which names the code that wrote the file. Covered: every stream kind, lines
//! and points, no indices, 32-bit indices, skin streams left out, and the refusals.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::{
    Binding, BindingId, Deformation, Influences, Joint, JointId, Mesh, Name, Primitive, Rest, Semantic,
    Skeleton, Skin, Slot, Stream, Topology, Transform, WeightRule, World,
};
use primase_glb::{Document, encode_mesh};
use primase_gltf::{WriteOptions, write};
use serde_json::Value;

mod common;

/// The writer's GLB of `mesh` as the only, unplaced prototype, named `name`.
fn written(name: &str, mesh: &Mesh) -> primase_gltf::Result<Vec<u8>> {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, mesh.clone());
    w.insert(e, Name(name.to_owned()));
    if mesh.primitives().iter().any(primase_core::skin::has_skin_streams) {
        let skin = skin_of(&mut w, mesh);
        w.insert(e, skin);
    }
    let options = WriteOptions {
        include_unplaced: true,
        ..Default::default()
    };
    write(&w, &options).map(|(bytes, _)| bytes)
}

/// A skin for `mesh` over a one-joint skeleton (all its vertices name slot 0): joint and weight streams are a mesh's
/// only with a skin, which the writer's entry check requires of the world.
fn skin_of(w: &mut World, mesh: &Mesh) -> Skin {
    let skeleton = w.spawn();
    w.insert(
        skeleton,
        Skeleton {
            joints: vec![Joint {
                id: JointId(1),
                name: "root".into(),
                parent: None,
                rest: Rest::Affine(Transform::IDENTITY),
            }],
        },
    );
    let slot = Slot {
        joint: JointId(1),
        inverse_bind: Transform::IDENTITY,
    };
    let influences: Vec<Option<Influences>> = mesh
        .primitives()
        .iter()
        .map(|p| {
            match (
                p.streams.contains_key(&Semantic::Joints(0)),
                p.streams.contains_key(&Semantic::Weights(0)),
            ) {
                (true, true) => Some(Influences::Streams { sets: 1 }),
                (true, false) => Some(Influences::SingleIndex),
                _ => None,
            }
        })
        .collect();
    let palettes = influences.iter().map(|i| i.map(|_| vec![slot])).collect();
    Skin {
        influences,
        weights: WeightRule::SumsToOne,
        bindings: vec![Binding {
            id: BindingId(0),
            skeleton,
            palettes,
            deformation: Deformation::Linear,
        }],
    }
}

/// The document's generator, replaced by a mask.
fn generator(doc: &mut Document) -> Value {
    std::mem::replace(&mut doc.json["asset"]["generator"], Value::from("masked"))
}

fn assert_same(name: &str, mesh: &Mesh) {
    let mut ours = Document::parse(&encode_mesh(name, mesh).unwrap()).unwrap();
    let mut theirs = Document::parse(&written(name, mesh).unwrap()).unwrap();
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(generator(&mut ours), format!("primase-glb {version}"));
    assert_eq!(generator(&mut theirs), format!("primase-gltf {version}"));
    // the writer keeps a prototype's skin in its node's extras; the encoder takes a mesh alone and has none
    for node in theirs.json["nodes"].as_array_mut().into_iter().flatten() {
        if let Some(extras) = node
            .get_mut("extras")
            .and_then(|e| e.get_mut("primase"))
            .and_then(Value::as_object_mut)
        {
            extras.remove("core.skin");
        }
    }
    // the JSON in its key order, then the buffer
    assert_eq!(
        serde_json::to_string(&ours.json).unwrap(),
        serde_json::to_string(&theirs.json).unwrap(),
        "{name}"
    );
    assert_eq!(ours.bin, theirs.bin, "{name}");
}

fn primitive(topology: Topology, positions: Vec<[f32; 3]>, indices: Option<Vec<u32>>) -> Primitive {
    Primitive {
        topology,
        streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(positions))]),
        indices,
        material: None,
    }
}

#[test]
fn the_single_mesh_encoder_writes_what_the_writer_writes_but_its_generator() {
    // every stream kind (u8 and f32 colours), two primitives sharing the buffer
    let mut u16_colour = common::rich_primitive(None, 2.0);
    u16_colour.streams.insert(
        Semantic::Color(0),
        Stream::U16x4(vec![
            [0, 1, 2, 3],
            [65535, 0, 0, 65535],
            [9, 9, 9, 9],
            [1, 1, 1, 1],
        ]),
    );
    u16_colour.streams.insert(
        Semantic::Color(2),
        Stream::F32x4(vec![[0.5; 4], [0.25; 4], [1.0; 4], [0.0; 4]]),
    );
    assert_same(
        "rich",
        &Mesh::new(vec![common::rich_primitive(None, 0.0), u16_colour]),
    );
    // lines and points; no indices
    let square = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    assert_same(
        "lines and points",
        &Mesh::new(vec![
            primitive(
                Topology::Lines,
                square.clone(),
                Some(vec![0, 1, 1, 2, 2, 3, 3, 0]),
            ),
            primitive(Topology::Points, square.clone(), None),
            primitive(Topology::Triangles, square[..3].to_vec(), None),
        ]),
    );
    // over 65535 vertices: 32-bit indices
    let many: Vec<[f32; 3]> = (0..65_538).map(|i| [i as f32, (i % 7) as f32, 0.0]).collect();
    assert_same(
        "big",
        &Mesh::new(vec![primitive(
            Topology::Triangles,
            many,
            Some(vec![0, 65_536, 65_537]),
        )]),
    );
    // skin streams are left out by both
    let mut skinned = primitive(Topology::Triangles, square[..3].to_vec(), None);
    skinned
        .streams
        .insert(Semantic::Joints(0), Stream::U8x4(vec![[0; 4]; 3]));
    skinned
        .streams
        .insert(Semantic::Weights(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 0.0]; 3]));
    assert_same("skinned", &Mesh::new(vec![skinned]));
}

#[test]
fn the_single_mesh_encoder_refuses_what_the_writer_refuses() {
    let square = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    let bad = [
        (
            "no primitives",
            Mesh::new(Vec::new()),
            "without primitives",
            "without primitives",
        ),
        (
            "out of range",
            Mesh::new(vec![primitive(
                Topology::Triangles,
                square.clone(),
                Some(vec![0, 1, 4]),
            )]),
            "index 4 of 4 vertices",
            "index 4 of 4 vertices",
        ),
        (
            "not whole",
            Mesh::new(vec![primitive(Topology::Triangles, square.clone(), None)]),
            "4 vertices is not whole Triangles",
            "4 corners are not whole Triangles",
        ),
        (
            "non-finite",
            Mesh::new(vec![primitive(
                Topology::Points,
                vec![[f32::NAN, 0.0, 0.0]],
                None,
            )]),
            "Position has a non-finite value",
            "not finite",
        ),
    ];
    // the encoder's own words and the world's: the writer refuses a mesh at its entry, in the world's check
    for (what, mesh, ours_reason, theirs_reason) in bad {
        let ours = encode_mesh(what, &mesh).unwrap_err().to_string();
        let theirs = written(what, &mesh).unwrap_err().to_string();
        assert!(
            ours.contains(ours_reason) && theirs.contains(theirs_reason),
            "{what}: {ours} / {theirs}"
        );
    }
}
