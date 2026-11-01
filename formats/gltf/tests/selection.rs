//! A [`Selection`] writes part of the world without changing it: only the selected placements, each with its
//! prototype's mesh or a piece of its own, and only the selected lights.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Environment, Instance, Light, LightKind, Mesh, Name, Opaque, OpaqueData, Primitive, Resource, Semantic,
    Stream, Topology, Transform, World,
};
use primase_gltf::{GroupBy, Piece, Selected, Selection, WriteOptions};
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
    let chunks = primase_glb::parse(&bytes).unwrap();
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
    w.insert_opaque(proto, "test.prototype_note", opaque(json!("prototype")))
        .unwrap();
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
    w.insert_opaque(placed[1], "test.z_after", opaque(json!("own")))
        .unwrap();
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
                    radius: None,
                    specular: None,
                },
            );
            w.insert(e, Name(format!("light_{k}")));
            e
        })
        .collect();
    w.insert_resource(Environment {
        sun: Some(lights[1]),
        ambient: None,
    });

    // everything
    let all = document(&w, None);
    assert_eq!(
        names(&all, "nodes"),
        ["placed_0", "placed_1", "placed_2", "light_0", "light_1"]
    );
    assert!(all["scenes"][0].get("extras").is_none());

    // placement 0 whole, placement 1 as a one-triangle piece, placement 2 and the sun left out
    let selection = Selection {
        placements: BTreeMap::from([
            (placed[0], Selected::default()),
            (
                placed[1],
                Selected {
                    piece: Some(Piece {
                        mesh: quad(Some(m), vec![0, 2, 3]),
                        name: "crate.piece".into(),
                        from: vec![0],
                    }),
                },
            ),
        ]),
        lights: [lights[0]].into(),
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
    // the prototype's components glTF has no form for are on neither the prototype's mesh nor the piece cut from it
    for mesh in [0, 1] {
        assert!(doc["meshes"][mesh].get("extras").is_none());
    }
    let piece = &doc["meshes"][1]["primitives"][0];
    assert_eq!(
        doc["accessors"][piece["indices"].as_u64().unwrap() as usize]["count"],
        3
    );
    assert_eq!(piece["material"], doc["meshes"][0]["primitives"][0]["material"]);
    for node in doc["nodes"].as_array().unwrap() {
        assert!(node.get("extras").is_none());
    }
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

/// A piece states, for each of its primitives, the prototype primitive it is cut from: one that does not is refused.
#[test]
fn a_piece_states_where_each_of_its_primitives_came_from() {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, quad(Some(m), vec![0, 1, 2, 0, 2, 3]));
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placed, Transform::IDENTITY);
    let selected = |from: Vec<usize>| Selection {
        placements: BTreeMap::from([(
            placed,
            Selected {
                piece: Some(Piece {
                    mesh: quad(Some(m), vec![0, 2, 3]),
                    name: "slab.piece".into(),
                    from,
                }),
            },
        )]),
        ..Selection::default()
    };
    let write = |selection| {
        primase_gltf::write(
            &w,
            &WriteOptions {
                selection: Some(selection),
                ..WriteOptions::default()
            },
        )
    };
    assert!(write(selected(vec![0])).is_ok());
    for (from, says) in [(vec![], "1 primitives with 0 sources"), (vec![1], "primitive 1")] {
        let err = write(selected(from)).unwrap_err().to_string();
        assert!(err.contains(says), "{says}: {err}");
    }
}

/// The environment's ambient has no glTF light: it is counted as not lit, and the environment as a resource without a
/// glTF form.
#[test]
fn an_ambient_is_counted_unlit_and_not_written() {
    let mut w = World::new();
    let options = WriteOptions {
        group_by: GroupBy::None,
        ..WriteOptions::default()
    };
    assert_eq!(primase_gltf::write(&w, &options).unwrap().1.ambient_unlit, 0);
    w.insert_resource(Environment {
        sun: None,
        ambient: Some(primase_core::Ambient {
            intensity: 1.0,
            light: primase_core::AmbientLight::Flat([0.25, 0.5, 0.75]),
        }),
    });
    let (bytes, report) = primase_gltf::write(&w, &options).unwrap();
    assert_eq!(report.ambient_unlit, 1);
    assert!(report.to_string().contains("ambient not lit (no glTF form) 1"));
    assert_eq!(report.resources_without_form.get(Environment::NAME), Some(&1));
    let doc: Value = serde_json::from_slice(primase_glb::parse(&bytes).unwrap().json).unwrap();
    assert!(doc["scenes"][0].get("extras").is_none());
}
