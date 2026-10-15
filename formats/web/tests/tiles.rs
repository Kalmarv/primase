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
    let options = Options {
        gpu_instancing: false,
        ..Options::default()
    };
    let a = write(&w, &options).unwrap();
    assert_eq!(
        a.index
            .tiles
            .iter()
            .map(|t| (t.col, t.row, t.placements, t.triangles))
            .collect::<Vec<_>>(),
        vec![(-1, 0, 1, 1), (0, 0, 2, 2), (1, 0, 1, 1)]
    );
    assert_eq!(a.files, write(&w, &options).unwrap().files);
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

#[test]
fn instancing_trs_and_plain_mirrors_shears() {
    let mut w = World::new();
    let p = w.spawn();
    w.insert(p, mesh());
    let mut transforms = Vec::new();
    for i in 0..5 {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [10. + i as f32, 2., 10.];
        if i == 2 {
            t.cols[0] = [0., 2., 0.];
            t.cols[1] = [-3., 0., 0.];
            t.cols[2] = [0., 0., 4.];
        }
        if i == 3 {
            t.cols[0][0] = -1.;
        }
        if i == 4 {
            t.cols[1][0] = 0.5;
        }
        w.insert(e, t);
        transforms.push(t);
    }
    let out = write(&w, &Options::default()).unwrap();
    let tile = &out.index.tiles[0];
    assert_eq!(tile.placements, 5);
    assert_eq!(tile.instanced_placements, 3);
    let bytes = &out.files[&tile.file];
    assert_eq!(*bytes, write(&w, &Options::default()).unwrap().files[&tile.file]);
    let chunks = primase_gltf::glb::parse(bytes).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let nodes = doc["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 3);
    let node = nodes
        .iter()
        .find(|n| n["extensions"]["EXT_mesh_gpu_instancing"].is_object())
        .unwrap();
    let attrs = &node["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
    let read = |name: &str, components: usize| {
        let a = &doc["accessors"][attrs[name].as_u64().unwrap() as usize];
        assert_eq!(a["count"], 3);
        let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let off = view["byteOffset"].as_u64().unwrap() as usize;
        chunks.bin.unwrap()[off..off + 3 * components * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect::<Vec<_>>()
    };
    let translation = read("TRANSLATION", 3);
    let rotation = read("ROTATION", 4);
    let scale = read("SCALE", 3);
    for i in 0..3 {
        assert_eq!(&translation[i * 3..i * 3 + 3], &transforms[i].cols[3]);
        let q = &rotation[i * 4..i * 4 + 4];
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let cols = [
            [
                1. - 2. * (y * y + z * z),
                2. * (x * y + z * w),
                2. * (x * z - y * w),
            ],
            [
                2. * (x * y - z * w),
                1. - 2. * (x * x + z * z),
                2. * (y * z + x * w),
            ],
            [
                2. * (x * z + y * w),
                2. * (y * z - x * w),
                1. - 2. * (x * x + y * y),
            ],
        ];
        for (c, column) in cols.iter().enumerate() {
            for (r, value) in column.iter().enumerate() {
                assert!((*value * scale[i * 3 + c] - transforms[i].cols[c][r]).abs() < 1e-5);
            }
        }
    }
    assert_eq!(
        node["extras"]["primase"]["web.instances"]["value"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    if let Some(validator) = primase_core::gate::real_data("PRIMASE_GLTF_VALIDATOR") {
        let path = std::env::temp_dir().join(format!("primase-web-instance-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let output = std::process::Command::new(validator)
            .arg("-o")
            .arg(path)
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["issues"]["numErrors"], 0, "{report}");
    }
}
