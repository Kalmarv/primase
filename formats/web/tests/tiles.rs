use primase_core::{Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_web::{Options, write};
use std::collections::BTreeMap;
fn mesh() -> Mesh {
    Mesh {
        primitives: vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
            )]),
            indices: Some(vec![0, 1, 2]),
            material: None,
        }],
    }
}
#[test]
fn pivot_tiles_negative_coordinates_and_determinism() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, mesh());
    for x in [-0.1, 0., 199.9, 200.] {
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
    }
    let a = write(&w, &Options::default()).unwrap();
    assert_eq!(
        a.index
            .tiles
            .iter()
            .map(|t| (t.col, t.row, t.placements, t.triangles))
            .collect::<Vec<_>>(),
        vec![(-1, 0, 1, 1), (0, 0, 2, 2), (1, 0, 1, 1)]
    );
    assert_eq!(a.files, write(&w, &Options::default()).unwrap().files);
    for t in &a.index.tiles {
        let g = primase_gltf::Gltf::from_glb(&a.files[&t.file]).unwrap();
        assert_eq!(g.mesh_count(), 1);
        assert_eq!(g.json()["nodes"].as_array().unwrap().len() as u64, t.placements);
        assert_eq!(a.files[&t.file].len() as u64, t.bytes);
    }
}
#[test]
fn triangle_centres_keep_whole_faces() {
    let mut w = World::new();
    let e = w.spawn();
    let mut m = mesh();
    m.primitives[0].streams.insert(
        Semantic::Position,
        Stream::F32x3(vec![
            [198., 0., 0.],
            [199., 0., 0.],
            [198., 0., 1.],
            [199., 0., 0.],
            [203., 0., 0.],
            [201., 0., 1.],
        ]),
    );
    m.primitives[0].indices = Some(vec![0, 1, 2, 3, 4, 5]);
    w.insert(e, m);
    w.insert(e, Transform::IDENTITY);
    let mut o = Options::default();
    o.split_by_tile.insert(e);
    let tiles = write(&w, &o).unwrap();
    assert_eq!(tiles.index.tiles.len(), 2);
    assert_eq!(tiles.index.tiles[1].min[0], 199.);
    assert_eq!(tiles.index.tiles[1].max[0], 203.);
    for t in &tiles.index.tiles {
        assert_eq!(t.triangles, 1);
        let g = primase_gltf::Gltf::from_glb(&tiles.files[&t.file]).unwrap();
        let mesh = g.read_mesh(0).unwrap().to_mesh();
        assert_eq!(mesh.primitives[0].vertex_count(), 3);
        assert_eq!(mesh.primitives[0].indices, Some(vec![0, 1, 2]));
    }
}
#[test]
fn bad_meshes_and_options_are_errors() {
    let mut w = World::new();
    let e = w.spawn();
    let mut m = mesh();
    m.primitives[0].indices = Some(vec![0, 1, 9]);
    w.insert(e, m);
    w.insert(e, Transform::IDENTITY);
    assert!(write(&w, &Options::default()).is_err());
    w.insert(e, mesh());
    let o = Options {
        tile_size: 0.,
        ..Options::default()
    };
    assert!(write(&w, &o).is_err());
    let mut t = Transform::IDENTITY;
    t.cols[0] = [0.; 3];
    w.insert(e, t);
    let out = write(&w, &Options::default()).unwrap();
    assert!(out.index.tiles.is_empty());
    assert_eq!(out.index.skipped_degenerate, 1);
}

#[test]
fn validator_and_index_file() {
    let Some(validator) = primase_core::gate::real_data("PRIMASE_GLTF_VALIDATOR") else {
        return;
    };
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, mesh());
    w.insert(e, Transform::IDENTITY);
    let set = write(&w, &Options::default()).unwrap();
    let dir = std::env::temp_dir().join(format!("primase-web-{}", std::process::id()));
    set.write_to(&dir).unwrap();
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("tiles.json")).unwrap()).unwrap();
    assert_eq!(index["tiles"][0]["triangles"], 1);
    for name in set.files.keys() {
        let path = dir.join(name);
        let out = std::process::Command::new(&validator)
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["issues"]["numErrors"], 0);
    }
    let mut invalid = set.clone();
    invalid.index.tiles[0].file = "../escape.glb".into();
    assert!(invalid.write_to(&dir).is_err());
}
