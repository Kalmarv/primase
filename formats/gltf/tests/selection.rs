//! A [`Selection`] writes part of the world without changing it: only the selected placements, each with its
//! prototype's mesh or a piece of its own and extra node extras, only the selected lights, and the scene's extras
//! without the omitted resources.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Environment, Instance, Light, LightKind, Mesh, Name, Opaque, OpaqueData, Primitive, Resource, Semantic,
    Stream, Topology, Transform, World,
};
use primase_gltf::{GroupBy, Selected, Selection, WriteOptions};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn quad(material: Option<primase_core::Entity>, indices: Vec<u32>) -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [1., 0., -1.], [0., 0., -1.]]),
        )]),
        indices: Some(indices),
        material,
    }])
}

fn document(w: &World, selection: Option<Selection>) -> Value {
    let (bytes, report) = primase_gltf::write(
        w,
        &WriteOptions {
            group_by: GroupBy::None,
            selection,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let chunks = primase_gltf::glb::parse(&bytes).unwrap();
    let doc: Value = serde_json::from_slice(chunks.json).unwrap();
    assert_eq!(
        report.lights as usize,
        doc["extensions"]["KHR_lights_punctual"]["lights"]
            .as_array()
            .map_or(0, Vec::len)
    );
    doc
}

fn names(doc: &Value, key: &str) -> Vec<String> {
    doc[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_selection_writes_its_placements_pieces_and_lights() {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, quad(Some(m), vec![0, 1, 2, 0, 2, 3]));
    w.insert(proto, Name("crate".into()));
    let opaque = |v: Value| Opaque {
        version: 1,
        data: OpaqueData::Json(v),
    };
    w.insert_opaque(proto, "test.prototype_note", opaque(json!("prototype")));
    let placed: Vec<_> = (0..3)
        .map(|k| {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [k as f32 * 10., 0., 0.];
            w.insert(e, t);
            w.insert(e, Name(format!("placed_{k}")));
            e
        })
        .collect();
    w.insert_opaque(placed[1], "test.z_after", opaque(json!("own")));
    let lights: Vec<_> = (0..2)
        .map(|k| {
            let e = w.spawn();
            w.insert(
                e,
                Light {
                    kind: if k == 0 {
                        LightKind::Point
                    } else {
                        LightKind::Directional
                    },
                    color: [1.; 3],
                    intensity: 1.,
                    range: None,
                    cast_shadows: None,
                },
            );
            w.insert(e, Name(format!("light_{k}")));
            e
        })
        .collect();
    w.insert_resource(Environment { sun: Some(lights[1]) });

    // everything
    let all = document(&w, None);
    assert_eq!(
        names(&all, "nodes"),
        ["placed_0", "placed_1", "placed_2", "light_0", "light_1"]
    );
    assert!(all["scenes"][0]["extras"]["primase"][Environment::NAME].is_object());

    // placement 0 whole, placement 1 as a one-triangle piece with an extra, placement 2 and the sun left out
    let selection = Selection {
        placements: BTreeMap::from([
            (placed[0], Selected::default()),
            (
                placed[1],
                Selected {
                    piece: Some((quad(Some(m), vec![0, 2, 3]), "crate.piece".into())),
                    extras: vec![("test.key".into(), 1, json!(7))],
                },
            ),
        ]),
        lights: [lights[0]].into(),
        omit_resources: [Environment::NAME.to_owned()].into(),
    };
    let before = format!("{:?}", w.components_json(placed[1]).unwrap());
    let doc = document(&w, Some(selection.clone()));
    assert_eq!(
        before,
        format!("{:?}", w.components_json(placed[1]).unwrap()),
        "the world is unchanged"
    );
    assert_eq!(names(&doc, "nodes"), ["placed_0", "placed_1", "light_0"]);
    assert_eq!(names(&doc, "meshes"), ["crate", "crate.piece"]);
    // the prototype's mesh carries the prototype's components; the piece none of them
    assert_eq!(
        doc["meshes"][0]["extras"]["primase"]["test.prototype_note"]["value"],
        "prototype"
    );
    assert!(doc["meshes"][1].get("extras").is_none());
    let piece = &doc["meshes"][1]["primitives"][0];
    assert_eq!(
        doc["accessors"][piece["indices"].as_u64().unwrap() as usize]["count"],
        3
    );
    assert_eq!(piece["material"], doc["meshes"][0]["primitives"][0]["material"]);
    // the extra among the placement's own components, in name order
    let extras = doc["nodes"][1]["extras"]["primase"].as_object().unwrap();
    assert_eq!(extras.keys().collect::<Vec<_>>(), ["test.key", "test.z_after"]);
    assert_eq!(extras["test.key"], json!({"version": 1, "value": 7}));
    assert!(doc["nodes"][0].get("extras").is_none());
    assert!(doc["scenes"][0].get("extras").is_none());

    // a selected entity that is not a placement is refused
    let mut wrong = selection;
    wrong.placements.insert(lights[0], Selected::default());
    let err = primase_gltf::write(
        &w,
        &WriteOptions {
            selection: Some(wrong),
            ..WriteOptions::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("not a placement"), "{err}");
}
