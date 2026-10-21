//! What the writer cannot represent it leaves out and reports, without failing on it: placements of a prototype with
//! levels of detail (the writer chooses no form), and entities it neither writes nor counts otherwise (a marker, a
//! standalone collision soup). A selection is the caller's choice: what it leaves out is not reported.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Component, Entity, Instance, LodForm, Lods, Marker, Mesh, Name, Primitive, Semantic, Stream,
    Topology, Transform, World,
};
use primase_gltf::{GroupBy, Selected, Selection, WriteOptions, WriteReport};
use serde_json::Value;

fn quad(material: Option<Entity>) -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [1., 0., -1.], [0., 0., -1.]]),
        )]),
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        material,
    }])
}

fn place(w: &mut World, prototype: Entity, name: &str, x: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, 0., 0.];
    w.insert(e, t);
    w.insert(e, Name(name.into()));
    e
}

fn written(w: &World, selection: Option<Selection>) -> (Value, WriteReport) {
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
    (serde_json::from_slice(chunks.json).unwrap(), report)
}

fn node_names(doc: &Value) -> Vec<&str> {
    doc["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(|n| n["name"].as_str().unwrap()).collect())
        .unwrap_or_default()
}

fn select(placements: &[Entity]) -> Selection {
    Selection {
        placements: placements.iter().map(|&e| (e, Selected::default())).collect(),
        ..Selection::default()
    }
}

/// Plain prototype A placed once, prototype B with two other forms placed once.
fn plain_and_lods() -> (World, Entity, Entity) {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let plain = w.spawn();
    w.insert(plain, quad(Some(m)));
    w.insert(plain, Name("plain".into()));
    let lod = w.spawn();
    w.insert(lod, quad(Some(m)));
    w.insert(lod, Name("detailed".into()));
    let forms: Vec<LodForm> = (1..=2)
        .map(|detail| {
            let form = w.spawn();
            w.insert(form, quad(Some(m)));
            LodForm { detail, form }
        })
        .collect();
    w.insert(
        lod,
        Lods {
            detail: 0,
            forms,
            default: None,
        },
    );
    let a = place(&mut w, plain, "a", 0.);
    let b = place(&mut w, lod, "b", 10.);
    assert!(w.verify_components().is_empty());
    (w, a, b)
}

#[test]
fn a_selection_beside_an_unselected_prototype_with_levels_of_detail_is_written() {
    let (w, a, _) = plain_and_lods();
    let (doc, report) = written(&w, Some(select(&[a])));
    assert_eq!(node_names(&doc), ["a"]);
    assert_eq!(report.placements, 1);
    assert_eq!(report.placements_with_lods, 0);
    assert_eq!(doc["meshes"].as_array().unwrap().len(), 1);
}

#[test]
fn a_selected_placement_with_levels_of_detail_is_left_out_and_reported() {
    let (w, a, b) = plain_and_lods();
    let (doc, report) = written(&w, Some(select(&[a, b])));
    assert_eq!(node_names(&doc), ["a"]);
    assert_eq!(report.placements, 1);
    assert_eq!(report.placements_with_lods, 1, "{report}");
    assert_eq!(doc["meshes"].as_array().unwrap().len(), 1);
    // a selection's choice: nothing else is counted
    assert_eq!(report.entities_unwritten, 0);
}

#[test]
fn without_a_selection_levels_of_detail_are_reported_and_their_forms_are_not_prototypes() {
    let (w, _, _) = plain_and_lods();
    let (doc, report) = written(&w, None);
    assert_eq!(node_names(&doc), ["a"]);
    assert_eq!(report.placements_with_lods, 1);
    assert_eq!(report.prototypes_unplaced, 0, "{report}");
    // the forms go with the placement left out, under its cause
    assert_eq!(report.entities_unwritten, 0, "{report}");
    assert!(report.components_unwritten.is_empty());
}

#[test]
fn a_lods_without_other_forms_has_nothing_to_choose() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, quad(None));
    w.insert(
        proto,
        Lods {
            detail: 1,
            forms: Vec::new(),
            default: None,
        },
    );
    place(&mut w, proto, "only", 0.);
    let (doc, report) = written(&w, None);
    assert_eq!(node_names(&doc), ["only"]);
    assert_eq!(report.placements_with_lods, 0);
    assert!(doc["meshes"][0]["extras"]["primase"][Lods::NAME].is_object());
}

fn marker(w: &mut World) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Marker {
            kind: "spawn".into(),
            ground: None,
            yaw: None,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [1., 2., 3.];
    w.insert(e, t);
    w.insert(e, Name("start".into()));
    e
}

#[test]
fn a_world_of_only_a_marker_reports_it() {
    let mut w = World::new();
    marker(&mut w);
    let (doc, report) = written(&w, None);
    assert!(node_names(&doc).is_empty());
    assert_eq!(report.entities_unwritten, 1, "{report}");
    assert_eq!(
        report.components_unwritten,
        BTreeMap::from([
            (Marker::NAME.to_owned(), 1),
            (Name::NAME.to_owned(), 1),
            (Transform::NAME.to_owned(), 1),
        ])
    );
}

#[test]
fn a_mesh_beside_a_marker_and_a_collision_soup_reports_them() {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, quad(Some(m)));
    let a = place(&mut w, proto, "a", 0.);
    marker(&mut w);
    let soup = w.spawn();
    w.insert(
        soup,
        Collision {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        },
    );
    let (doc, report) = written(&w, None);
    assert_eq!(node_names(&doc), ["a"]);
    assert_eq!(report.placements, 1);
    assert_eq!(report.materials, 1);
    assert_eq!(report.entities_unwritten, 2, "{report}");
    assert_eq!(report.components_unwritten[Marker::NAME], 1);
    assert_eq!(report.components_unwritten[Collision::NAME], 1);
    // the written placement, prototype and material are not among them
    assert_eq!(report.components_unwritten[Transform::NAME], 1);
    assert!(!report.components_unwritten.contains_key(Mesh::NAME));
    assert!(!report.components_unwritten.contains_key(PbrMaterial::NAME));

    // a selection writes what it names; the rest is its caller's
    let (doc, report) = written(&w, Some(select(&[a])));
    assert_eq!(node_names(&doc), ["a"]);
    assert_eq!(report.entities_unwritten, 0);
    assert!(report.components_unwritten.is_empty());
}

/// A texture whose format an 8-bit PNG cannot hold exactly (half floats: 2.0 is beyond 0-1) is left out and reported
/// by format, its slot left empty; once `lower-textures-rgba8` converts it, it is written.
#[test]
fn a_texture_an_8_bit_png_cannot_hold_is_left_out_until_converted() {
    use primase_core::pass::{Pass, PassLog};
    use primase_core::pbr::TextureRef;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let mut w = World::new();
    let texture = w.spawn();
    w.insert(
        texture,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba16Float,
            levels: vec![
                [0x4000u16, 0, 0x3400, 0x3C00]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            ],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = w.spawn();
    w.insert(
        material,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(texture)),
            ..PbrMaterial::default()
        },
    );
    // the quad with the texture coordinates its material samples
    let mut primitive = quad(Some(material)).primitives()[0].clone();
    primitive.streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]]),
    );
    let mesh = Mesh::new(vec![primitive]);
    let prototype = w.spawn();
    w.insert(prototype, mesh);
    place(&mut w, prototype, "lit", 0.);
    let (doc, report) = written(&w, None);
    assert_eq!(
        report.textures_unrepresentable,
        BTreeMap::from([("Rgba16Float".to_owned(), 1)])
    );
    assert_eq!((report.texture_slots_dropped, report.images), (1, 0));
    assert!(doc.get("images").is_none());
    primase_image::LowerTexturesRgba8::default()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    let (_, report) = written(&w, None);
    assert!(report.textures_unrepresentable.is_empty());
    assert_eq!((report.texture_slots_dropped, report.images), (0, 1));
}
