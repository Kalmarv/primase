//! What is known of shadows is written as `extras["primase.shadows"]` (version 1): a placement's `core.shadows` on
//! its node, a material's and a light's `cast_shadows` on the material and the light's node; unknown is absent.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Light, LightKind, Mesh, ShadowCast, Shadows, Transform, World};
use serde_json::{Value, json};

mod common;

fn document(w: &World) -> Value {
    let (bytes, _) = primase_gltf::write(w, &primase_gltf::WriteOptions::default()).unwrap();
    let chunks = primase_glb::parse(&bytes).unwrap();
    serde_json::from_slice(chunks.json).unwrap()
}

/// A prototype drawn with a material that does not cast, placed twice: once as a shadow proxy that does not receive,
/// once with nothing known; and a light that casts.
fn world() -> World {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            cast_shadows: Some(false),
            ..PbrMaterial::default()
        },
    );
    let proto = w.spawn();
    w.insert(proto, Mesh::new(vec![common::rich_primitive(Some(m), 0.0)]));
    for (x, shadows) in [
        (
            0.0,
            Some(Shadows {
                cast: Some(ShadowCast::Only),
                receive: Some(false),
            }),
        ),
        (2.0, None),
    ] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3][0] = x;
        w.insert(e, t);
        if let Some(s) = shadows {
            w.insert(e, s);
        }
    }
    let light = w.spawn();
    w.insert(
        light,
        Light {
            kind: LightKind::Point,
            color: [1.0; 3],
            intensity: 10.0,
            range: None,
            cast_shadows: Some(true),
        },
    );
    w.insert(light, Transform::IDENTITY);
    w
}

#[test]
fn shadows_are_node_and_material_extras() {
    let doc = document(&world());
    let nodes = doc["nodes"].as_array().unwrap();
    let placed: Vec<&Value> = nodes.iter().filter(|n| n.get("mesh").is_some()).collect();
    assert_eq!(placed.len(), 2);
    assert_eq!(
        placed[0]["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "only", "receive": false})
    );
    assert!(
        placed[1]
            .get("extras")
            .is_none_or(|x| x.get("primase.shadows").is_none()),
        "unknown is absent"
    );
    // consumed: not also a generic component in the extras
    assert!(placed[0]["extras"]["primase"].get("core.shadows").is_none());
    assert_eq!(
        doc["materials"][0]["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "off"})
    );
    let light = nodes
        .iter()
        .find(|n| n["extensions"].get("KHR_lights_punctual").is_some())
        .unwrap();
    assert_eq!(
        light["extras"]["primase.shadows"],
        json!({"version": 1, "cast": "on"})
    );
}

#[test]
fn nothing_known_writes_nothing() {
    let mut w = world();
    let ids: Vec<_> = w.iter::<Shadows>().map(|(e, _)| e).collect();
    for e in ids {
        w.remove::<Shadows>(e);
    }
    let materials: Vec<_> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
    for e in materials {
        w.get_mut::<PbrMaterial>(e).unwrap().cast_shadows = None;
    }
    let lights: Vec<_> = w.iter::<Light>().map(|(e, _)| e).collect();
    for e in lights {
        w.get_mut::<Light>(e).unwrap().cast_shadows = None;
    }
    assert!(
        !serde_json::to_string(&document(&w))
            .unwrap()
            .contains("primase.shadows")
    );
}
