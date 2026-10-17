//! A mesh file's box from its GLB's JSON alone: the POSITION accessors' bounds of every primitive of that mesh.

#![allow(clippy::unwrap_used)]

use primase_core::{Mesh, MeshFile, Primitive, Semantic, Stream, Topology, Transform, World};
use std::collections::BTreeMap;

mod common;

fn triangle(points: [[f32; 3]; 3]) -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(points.to_vec()))]),
        indices: Some(vec![0, 1, 2]),
        material: None,
    }
}

#[test]
fn a_mesh_files_box_is_read_from_its_json() {
    let dir = common::scratch("mesh-box");
    let mut w = World::new();
    let e = w.spawn();
    w.insert(
        e,
        Mesh::new(vec![
            triangle([[0., 0., 0.], [1., 0., 0.], [0., 2., 0.]]),
            triangle([[-3., 0., 0.], [0., 0., 5.], [0., 0., -1.]]),
        ]),
    );
    w.insert(e, Transform::IDENTITY);
    primase_gltf::write_file(&w, &primase_gltf::WriteOptions::default(), &dir.join("a.glb")).unwrap();
    let file = |path: &str| MeshFile {
        path: path.into(),
        mesh: 0,
        materials: vec![None, None],
    };
    assert_eq!(
        primase_gltf::mesh_file_box(&dir, &file("a.glb")).unwrap(),
        Some([[-3., 0., -1.], [1., 2., 5.]])
    );
    // a mesh the file lacks, a file that is not a GLB, a JSON chunk longer than the file: errors naming the file
    let missing = MeshFile {
        mesh: 7,
        ..file("a.glb")
    };
    let err = primase_gltf::mesh_file_box(&dir, &missing)
        .unwrap_err()
        .to_string();
    assert!(err.contains("a.glb") && err.contains("meshes[7]"), "{err}");
    std::fs::write(dir.join("b.glb"), b"not a glb file at all").unwrap();
    let err = primase_gltf::mesh_file_box(&dir, &file("b.glb"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a GLB"), "{err}");
    let mut cut = std::fs::read(dir.join("a.glb")).unwrap();
    cut.truncate(40);
    std::fs::write(dir.join("c.glb"), &cut).unwrap();
    let err = primase_gltf::mesh_file_box(&dir, &file("c.glb"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("past the end"), "{err}");
}
