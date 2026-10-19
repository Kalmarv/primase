//! A mesh file's box from its GLB's JSON alone: the POSITION accessors' bounds of every primitive of that mesh.

#![allow(clippy::unwrap_used)]

use primase_core::{Mesh, MeshFile, Primitive, Semantic, Stream, Topology};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A fresh, empty scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("primase-glb-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

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
    let dir = scratch("mesh-box");
    let mesh = Mesh::new(vec![
        triangle([[0., 0., 0.], [1., 0., 0.], [0., 2., 0.]]),
        triangle([[-3., 0., 0.], [0., 0., 5.], [0., 0., -1.]]),
    ]);
    std::fs::write(dir.join("a.glb"), primase_glb::encode_mesh("a", &mesh).unwrap()).unwrap();
    let file = |path: &str| MeshFile {
        path: path.into(),
        mesh: 0,
        materials: vec![None, None],
    };
    assert_eq!(
        primase_glb::mesh_file_box(&dir, &file("a.glb")).unwrap(),
        Some([[-3., 0., -1.], [1., 2., 5.]])
    );
    // a mesh the file lacks, a file that is not a GLB, a JSON chunk longer than the file: errors naming the file
    let missing = MeshFile {
        mesh: 7,
        ..file("a.glb")
    };
    let err = primase_glb::mesh_file_box(&dir, &missing)
        .unwrap_err()
        .to_string();
    assert!(err.contains("a.glb") && err.contains("meshes[7]"), "{err}");
    std::fs::write(dir.join("b.glb"), b"not a glb file at all").unwrap();
    let err = primase_glb::mesh_file_box(&dir, &file("b.glb"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a GLB"), "{err}");
    let mut cut = std::fs::read(dir.join("a.glb")).unwrap();
    cut.truncate(40);
    std::fs::write(dir.join("c.glb"), &cut).unwrap();
    let err = primase_glb::mesh_file_box(&dir, &file("c.glb"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("past the end"), "{err}");
}
