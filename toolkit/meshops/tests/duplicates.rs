//! `drop-duplicate-triangles` through the pass manager: a repeat of a triangle (any rotation, every stream equal) is
//! left out and declared; a reversed one, one that differs in any stream, and one in another primitive stay.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::{Conservation, Measure};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Mesh, MeshFile, Primitive, Semantic, Stream, Topology, World};
use primase_meshops::DropDuplicateTriangles;

fn run(w: &mut World) -> RunReport {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(DropDuplicateTriangles)],
            },
            &mut report,
        )
        .unwrap();
    report
}

/// Four corners: `a`, `b`, `c` and `d`, which sits where `c` does but with another texture coordinate.
fn primitive(indices: Option<Vec<u32>>) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]]),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.5, 0.5]]),
            ),
        ]),
        indices,
        material: None,
    }
}

fn indices(w: &World, e: primase_core::Entity, i: usize) -> Vec<u32> {
    w.get::<Mesh>(e).unwrap().primitives()[i].indices.clone().unwrap()
}

#[test]
fn a_repeat_goes_and_what_differs_stays() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(
        e,
        Mesh::new(vec![
            // 0 1 2; rotated; again; reversed; one corner with another texture coordinate
            primitive(Some(vec![0, 1, 2, 1, 2, 0, 0, 1, 2, 0, 2, 1, 0, 1, 3])),
            // the same triangle in another primitive
            primitive(Some(vec![0, 1, 2])),
        ]),
    );
    w.insert(
        e,
        MeshFile {
            path: "prototype.glb".into(),
            mesh: 0,
            materials: vec![None; 2],
        },
    );
    let report = run(&mut w);
    assert_eq!(indices(&w, e, 0), vec![0, 1, 2, 0, 2, 1, 0, 1, 3]);
    assert_eq!(indices(&w, e, 1), vec![0, 1, 2]);
    assert!(!w.has::<MeshFile>(e), "a changed mesh is no longer its file's");
    let dropped: u64 = report
        .steps
        .iter()
        .flat_map(|s| s.log.drops.iter())
        .filter(|((m, _), _)| *m == Measure::Triangles)
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(dropped, 2);
}

#[test]
fn a_corner_that_is_the_same_vertex_twice_over_counts_as_one() {
    // vertices 2 and 3 differ only by index in a primitive whose streams agree there
    let mut w = World::new();
    let e = w.spawn();
    let mut p = primitive(Some(vec![0, 1, 2, 0, 1, 3]));
    p.streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0]]),
    );
    w.insert(e, Mesh::new(vec![p]));
    run(&mut w);
    assert_eq!(indices(&w, e, 0), vec![0, 1, 2]);
}

#[test]
fn a_primitive_without_indices_is_read_in_order_and_one_without_repeats_is_untouched() {
    let mut w = World::new();
    let (a, b) = (w.spawn(), w.spawn());
    let mut p = primitive(None);
    for s in p.streams.values_mut() {
        match s {
            Stream::F32x3(v) => *v = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].repeat(2),
            Stream::F32x2(v) => *v = [[0.0; 2], [1.0, 0.0], [0.0, 1.0]].repeat(2),
            _ => unreachable!(),
        }
    }
    w.insert(a, Mesh::new(vec![p]));
    w.insert(b, Mesh::new(vec![primitive(Some(vec![0, 1, 2, 0, 1, 3]))]));
    w.insert(
        b,
        MeshFile {
            path: "kept.glb".into(),
            mesh: 0,
            materials: vec![None],
        },
    );
    run(&mut w);
    assert_eq!(indices(&w, a, 0), vec![0, 1, 2]);
    assert_eq!(indices(&w, b, 0), vec![0, 1, 2, 0, 1, 3]);
    assert!(w.has::<MeshFile>(b), "an unchanged mesh keeps its file link");
}
