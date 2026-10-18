//! `core.vertex_color` `ignored`: a written primitive drawn with such a material has no `COLOR_0`, counted; a
//! prototype's `COLOR_0` accessor is still written for a variant whose material takes it, and not at all when no
//! material drawing the primitive does.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Instance, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, VertexColor,
    World,
};
use primase_gltf::{WriteOptions, write};
use serde_json::Value;

fn document(bytes: &[u8]) -> Value {
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + n]).unwrap()
}

fn material(w: &mut World, name: &str, ignores: bool) -> primase_core::Entity {
    let m = w.spawn();
    w.insert(m, Name(name.into()));
    w.insert(m, PbrMaterial::default());
    if ignores {
        w.insert(m, VertexColor::Ignored);
    }
    m
}

/// One coloured triangle drawn with `material`, placed once per variant.
fn world(variant: Option<bool>) -> World {
    let mut w = World::new();
    let ignores = material(&mut w, "standard", true);
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                ),
                (Semantic::Color(0), Stream::U8x4(vec![[255, 0, 0, 255]; 3])),
            ]),
            indices: None,
            material: Some(ignores),
        }]),
    );
    let mut variants = vec![0];
    if let Some(takes) = variant {
        let v = material(&mut w, "variant", !takes);
        w.insert(proto, MaterialVariants(vec![BTreeMap::from([(1, v)])]));
        variants.push(1);
    }
    for variant in variants {
        let at = w.spawn();
        w.insert(
            at,
            Instance {
                prototype: proto,
                variant,
            },
        );
        w.insert(at, Transform::IDENTITY);
    }
    w
}

fn color0(doc: &Value) -> Vec<bool> {
    doc["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["primitives"][0]["attributes"].get("COLOR_0").is_some())
        .collect()
}

#[test]
fn an_ignoring_material_is_drawn_without_color0() {
    let (bytes, report) = write(&world(None), &WriteOptions::default()).unwrap();
    let doc = document(&bytes);
    assert_eq!(color0(&doc), [false]);
    assert_eq!(report.primitives_color0_ignored, 1);
    // nothing draws it: no accessor written (only POSITION's)
    assert_eq!(doc["accessors"].as_array().unwrap().len(), 1);
    assert_eq!(
        doc["materials"][0]["extras"]["primase"]["core.vertex_color"]["value"],
        "ignored"
    );
}

#[test]
fn a_variant_that_takes_it_keeps_color0() {
    let (bytes, report) = write(&world(Some(true)), &WriteOptions::default()).unwrap();
    let doc = document(&bytes);
    assert_eq!(color0(&doc), [false, true]);
    assert_eq!(report.primitives_color0_ignored, 1);
    assert_eq!(doc["accessors"].as_array().unwrap().len(), 2);
    let (bytes, report) = write(&world(Some(false)), &WriteOptions::default()).unwrap();
    let doc = document(&bytes);
    assert_eq!(color0(&doc), [false, false]);
    assert_eq!(report.primitives_color0_ignored, 2);
    assert_eq!(doc["accessors"].as_array().unwrap().len(), 1);
}
