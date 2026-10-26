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
    let mut reg = PassRegistry::new();
    primase_reskate::register(&mut reg);
    let pipe = reg
        .parse("raise-skate{name=measured,light_scale=standard,unclassified=leave-out}")
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
    let pipe = reg.parse("prepare-reskate-repeat-clipping").unwrap();
    let mut report = RunReport::default();
    PassManager::new(reg)
        .run(w, &pipe, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}
fn triangles(w: &World, e: Entity) -> usize {
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3
}
#[test]
fn measured_threshold_depends_on_world_piece_area_and_uv_extent() {
    for (area, uv, scale, expected) in [
        (3.2e-11, CROSSING, 1., 0),
        (6.4e-11, CROSSING, 1., 2),
        (1e-10, CROSSING, 1., 2),
        // At high area no polygon is lost, so the source triangle stays for Studio to split.
        (1e-6, CROSSING, 1., 1),
        (1e-10, CROSSING, 0.001, 0),
        (1e-10, CROSSING, 1000., 1),
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
                    .counts
                    .keys()
                    .any(|s| s.starts_with("measured_overlay: discarded"))
            );
        }
    }
}
#[test]
fn retained_vertices_keep_interpolated_opacity_and_single_tile_uvs() {
    let (mut w, e, _) = world(1e-10, CROSSING, 1.);
    use primase_core::morphs::{Deltas, Frame, MorphId, Morphs, Target};
    w.insert(
        e,
        Morphs {
            targets: vec![Target {
                id: MorphId(0),
                name: "measured_delta".into(),
                default_weight: 0.,
                frames: vec![Frame {
                    weight: 1.,
                    primitives: vec![Some(Deltas {
                        positions: Some(vec![[0.2, 0., 0.], [0.4, 0., 0.], [0.8, 0., 0.]]),
                        ..Default::default()
                    })],
                }],
            }],
        },
    );
    run(&mut w).unwrap();
    assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
    let Stream::F32x3(pos) = &p.streams[&Semantic::Position] else {
        panic!()
    };
    let Stream::F32x2(uv) = &p.streams[&Semantic::TexCoord(0)] else {
        panic!()
    };
    let opacity = w.get::<BlendOpacity>(e).unwrap().0[0].as_ref().unwrap();
    assert_eq!(pos.len(), opacity.len());
    let deltas = w.get::<Morphs>(e).unwrap().targets[0].frames[0].primitives[0]
        .as_ref()
        .unwrap()
        .positions
        .as_ref()
        .unwrap();
    for (d, o) in deltas.iter().zip(opacity) {
        assert!((d[0] - o).abs() < 1e-6);
    }
    for (at, &o) in pos.iter().zip(opacity) {
        let b = at[0];
        let c = at[1] / 2e-10;
        assert!((o - (0.2 * (1. - b - c) + 0.4 * b + 0.8 * c)).abs() < 1e-6);
    }
    for tri in p.indices.as_ref().unwrap().as_chunks::<3>().0 {
        for axis in [0, 1] {
            let lo = tri
                .map(|i| uv[i as usize][axis])
                .into_iter()
                .fold(f32::INFINITY, f32::min);
            let hi = tri
                .map(|i| uv[i as usize][axis])
                .into_iter()
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((hi - 1e-5).floor() <= (lo + 1e-5).floor());
        }
    }
    let before = w.get::<Mesh>(e).unwrap().clone();
    run(&mut w).unwrap();
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before, "preparation is idempotent");
}
#[test]
fn opaque_materials_and_disagreeing_shared_placements_are_explicit() {
    let (mut w, e, place) = world(1e-10, CROSSING, 1.);
    let m = w.get::<Mesh>(e).unwrap().primitives()[0].material.unwrap();
    w.get_mut::<SkateMaterial>(m).unwrap().class = MaterialClass::Opaque;
    run(&mut w).unwrap();
    assert_eq!(triangles(&w, e), 1);
    w.get_mut::<SkateMaterial>(m).unwrap().class = MaterialClass::Decal;
    let other = w.duplicate(place).unwrap();
    w.insert(
        other,
        Transform {
            cols: [[0.001, 0., 0.], [0., 0.001, 0.], [0., 0., 0.001], [0.; 3]],
        },
    );
    let before = w.get::<Mesh>(e).unwrap().clone();
    let error = run(&mut w).unwrap_err();
    assert!(error.contains("shared placements disagree"), "{error}");
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
}

#[test]
fn unknown_vertex_data_is_refused_before_any_mesh_changes() {
    #[derive(Clone, Debug)]
    struct UnknownVertexData;
    impl primase_core::Component for UnknownVertexData {
        const NAME: &'static str = "test.unknown_vertex_data";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
        fn remap_primitives(&self, _: &[usize]) -> Option<Result<Self, String>> {
            Some(Err("unmodelled primitive data".into()))
        }
        fn remap_vertices(&self, _: usize, _: &[u32]) -> Option<Result<Self, String>> {
            Some(Err("unmodelled vertex data".into()))
        }
    }
    let (mut w, e, _) = world(1e-10, CROSSING, 1.);
    w.insert(e, UnknownVertexData);
    let before = w.get::<Mesh>(e).unwrap().clone();
    let error = run(&mut w).unwrap_err();
    assert!(error.contains("test.unknown_vertex_data"), "{error}");
    assert_eq!(w.get::<Mesh>(e).unwrap(), &before);
}
