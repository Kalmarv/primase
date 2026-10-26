//! Closed Studio 20261012 fixture observations: see the pass documentation for the independently compiled cases.
#![allow(clippy::unwrap_used)]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::components::{BlendOpacity, MaterialClass, SkateMaterial};
use std::collections::BTreeMap;

const CROSSING: [[f32; 2]; 3] = [
    [-2.0941606, 1.5334065],
    [-2.0486684, 1.5344927],
    [-2.0486684, 0.6637857],
];
const INSIDE: [[f32; 2]; 3] = [[0.1, 0.1], [0.9, 0.1], [0.1, 0.9]];

fn world(area: f32, uv: [[f32; 2]; 3], scale: f32) -> (World, Entity, Entity) {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(m, Name("measured_overlay".into()));
    w.insert(m, primase_core::pbr::PbrMaterial::default());
    let e = w.spawn();
    w.insert(e, Name("measured_triangle".into()));
    w.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 2. * area, 0.]]),
                ),
                (Semantic::Normal, Stream::F32x3(vec![[0., 0., 1.]; 3])),
                (Semantic::TexCoord(0), Stream::F32x2(uv.to_vec())),
            ]),
            indices: Some(vec![0, 1, 2]),
            material: Some(m),
        }]),
    );
    w.insert(e, BlendOpacity(vec![Some(vec![0.2, 0.4, 0.8])]));
    let place = w.spawn();
    w.insert(
        place,
        Instance {
            prototype: e,
            variant: 0,
        },
    );
    w.insert(
        place,
        Transform {
            cols: [[scale, 0., 0.], [0., scale, 0.], [0., 0., scale], [0.; 3]],
        },
    );
    let marker = w.spawn();
    w.insert(
        marker,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: None,
            yaw: None,
        },
    );
    w.insert(marker, Transform::IDENTITY);
    let mut reg = PassRegistry::new();
    primase_reskate::register(&mut reg);
    let pipe = reg
        .parse("prepare-reskate-laid-layers, raise-skate{name=measured,light_scale=standard,unclassified=leave-out}")
        .unwrap();
    PassManager::new(reg)
        .run(&mut w, &pipe, &mut RunReport::default())
        .unwrap();
    w.get_mut::<SkateMaterial>(m).unwrap().class = MaterialClass::Decal;
    (w, e, place)
}
fn run(w: &mut World) -> Result<RunReport, String> {
    let mut reg = PassRegistry::new();
    primase_reskate::register(&mut reg);
    let pipe = reg
        .parse("prepare-reskate-repeat-clipping{max_render_edge=0}")
        .unwrap();
    let mut report = RunReport::default();
    PassManager::new(reg)
        .run(w, &pipe, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}
fn triangles(w: &World, _: Entity) -> usize {
    let rec = w
        .resource::<primase_reskate::components::SkateRepeatClipping>()
        .unwrap();
    let changed: Vec<_> = rec
        .meshes
        .values()
        .flat_map(|m| m.replacements.values())
        .collect();
    if changed.is_empty() {
        return 1;
    }
    changed
        .into_iter()
        .map(|p| p.as_ref().map_or(0, |p| p.idx.len() / 3))
        .sum()
}

#[test]
fn measured_threshold_depends_on_world_piece_area_and_uv_extent() {
    for (area, uv, scale, expected) in [
        (3.2e-11, CROSSING, 1., 0),
        (6.4e-11, CROSSING, 1., 2),
        (1e-10, CROSSING, 1., 2),
        // High-area pieces are also prepared, preserving all corners Studio actually retains.
        (1e-6, CROSSING, 1., 3),
        (1e-10, CROSSING, 0.001, 0),
        (1e-10, CROSSING, 1000., 3),
        (1e-10, [[-0.5, -0.5], [1.5, -0.5], [-0.5, 1.5]], 1., 0),
        (1e-12, INSIDE, 1., 1),
        (0., INSIDE, 1., 1),
        (1e-12, [[-1.9, 0.1], [-1.1, 0.1], [-1.9, 0.9]], 1., 1),
        (0., CROSSING, 1., 0),
    ] {
        let (mut w, e, _) = world(area, uv, scale);
        let report = run(&mut w).unwrap();
        assert_eq!(
            triangles(&w, e),
            expected,
            "area {area}, scale {scale}, UV {uv:?}"
        );
        if expected == 0 {
            assert!(
                report.steps[0]
                    .log
                    .omitted
                    .keys()
                    .any(|s| s.contains("Studio repeat-clipped fan triangles"))
            );
        }
    }
}
#[test]
fn preparation_keeps_core_data_and_handles_shared_scales() {
    let (mut w, e, place) = world(1e-10, CROSSING, 1.);
    let other = w.duplicate(place).unwrap();
    w.insert(
        other,
        Transform {
            cols: [[0.001, 0., 0.], [0., 0.001, 0.], [0., 0., 0.001], [10., 0., 0.]],
        },
    );
    let before = w.get::<Mesh>(e).unwrap().clone();
    let opacity = w.get::<BlendOpacity>(e).unwrap().clone();
    run(&mut w).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
    assert_eq!(w.get::<BlendOpacity>(e).unwrap(), &opacity);
    assert!(w.verify_components().is_empty());
    let rec = w
        .resource::<primase_reskate::components::SkateRepeatClipping>()
        .unwrap();
    assert!(!rec.discarded.is_empty());
    let changed = rec
        .meshes
        .values()
        .flat_map(|m| m.replacements.values())
        .flatten()
        .next()
        .unwrap();
    assert_eq!(changed.idx.len() / 3, 2);
    assert!(
        changed
            .blend
            .as_ref()
            .unwrap()
            .iter()
            .all(|x| (0.2..=0.8).contains(x))
    );
}
#[test]
fn final_coordinate_rounding_and_stale_records_are_checked() {
    // A visible-area sliver whose tiny UV slice collapses only at these stored f32 coordinates.
    let (mut w, e, _) = world(1., CROSSING, 1.);
    let mut mesh = w.get::<Mesh>(e).unwrap().clone();
    let mut part = mesh.primitive_mut(0).unwrap();
    part.streams_mut().insert(
        Semantic::Position,
        Stream::F32x3(vec![
            [22.436964, -1.0787257, -75.71633],
            [22.436964, -1.0787257, -72.45642],
            [22.437044, -1.0787257, -72.456345],
        ]),
    );
    part.streams_mut().insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![
            [-7.1020103, -3.996503],
            [-7.1020103, -4.4004426],
            [-7.101996, -4.400452],
        ]),
    );
    w.insert(e, mesh);
    run(&mut w).unwrap();
    let rec = w
        .resource::<primase_reskate::components::SkateRepeatClipping>()
        .unwrap();
    assert!(!rec.discarded.is_empty());
    let mut mesh = w.get::<Mesh>(e).unwrap().clone();
    mesh.primitive_mut(0)
        .unwrap()
        .streams_mut()
        .insert(Semantic::TexCoord(0), Stream::F32x2(INSIDE.to_vec()));
    w.insert(e, mesh);
    let path = std::env::temp_dir().join(format!("primase-repeat-stale-{}", std::process::id()));
    let error = primase_reskate::write_map(
        &w,
        std::path::Path::new(""),
        &primase_reskate::EncodingOptions {
            out_root: path,
            max_render_edge: Some(0.),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("stale repeat-clipping"), "{error}");
}

#[test]
fn shared_prototype_clips_once_in_local_coordinates() {
    let (mut w, e, place) = world(1e-10, CROSSING, 1.);
    let other = w.duplicate(place).unwrap();
    w.insert(
        other,
        Transform {
            cols: [[0.001, 0., 0.], [0., 0.001, 0.], [0., 0., 0.001], [10., 0., 0.]],
        },
    );
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let pipeline = registry
        .parse("prepare-reskate-repeat-clipping{min_instances=2,min_instance_tris=0,max_render_edge=0}")
        .unwrap();
    PassManager::new(registry)
        .run(&mut w, &pipeline, &mut RunReport::default())
        .unwrap();
    let rec = w
        .resource::<primase_reskate::components::SkateRepeatClipping>()
        .unwrap();
    assert_eq!(rec.meshes.len(), 1);
    assert!(rec.meshes.keys().next().unwrap().starts_with("p_"));
    assert_eq!(triangles(&w, e), 2);
}

#[test]
fn tile_margin_excludes_a_corner_even_without_area_loss() {
    let (mut w, e, _) = world(1., [[-0.2, 0.98], [-0.2, 0.94], [1.0000069, 0.94]], 1.);
    let low = 0.0015526774;
    w.insert(e, BlendOpacity(vec![Some(vec![0.5, 0.7, low])]));
    run(&mut w).unwrap();
    let rec = w
        .resource::<primase_reskate::components::SkateRepeatClipping>()
        .unwrap();
    let prepared = rec
        .meshes
        .values()
        .flat_map(|m| m.replacements.values())
        .flatten()
        .next()
        .unwrap();
    assert_eq!(
        rec.source_triangles, 0,
        "the measured case loses no whole triangle"
    );
    assert!(
        prepared
            .uv
            .as_ref()
            .unwrap()
            .as_chunks::<2>()
            .0
            .iter()
            .all(|uv| uv[0] <= 1.)
    );
    let min = prepared
        .blend
        .as_ref()
        .unwrap()
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    assert!(
        min > low + 1e-7,
        "the clipped-away corner's weight must not be recorded: {min}"
    );
    assert_eq!(
        w.get::<Mesh>(e).unwrap().primitives()[0].vertex_count(),
        3,
        "core data stays authored"
    );
}
