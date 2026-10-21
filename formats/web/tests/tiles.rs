//! Web tiles: placement, pivots, instancing, compression and quantization.
// helpers outside #[test] functions unwrap too
#![allow(clippy::unwrap_used)]
use primase_core::{Entity, Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_web::{Options, write};
use std::collections::BTreeMap;
fn plain() -> Options {
    Options {
        compress: false,
        ..Options::new(primase_web::TileSize::new(200.).unwrap())
    }
}
#[test]
fn asset_names_hash_their_own_bytes_and_reject_stale_content() {
    use sha2::{Digest, Sha256};
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, mesh());
    let placement = world.spawn();
    world.insert(
        placement,
        Instance {
            prototype: entity,
            variant: 0,
        },
    );
    world.insert(placement, Transform::IDENTITY);
    let mut set = write(&world, &plain()).unwrap();
    for (name, bytes) in &set.files {
        let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        assert!(name.ends_with(&format!(".{hash}.glb")));
    }
    let name = set.index.tiles[0].file.clone();
    set.files.get_mut(&name).unwrap()[0] ^= 1;
    let destination = std::env::temp_dir().join("primase-rejected-stale-tile");
    assert!(set.write_to(&destination).is_err());
}
fn mesh() -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
        )]),
        indices: Some(vec![0, 1, 2]),
        material: None,
    }])
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
        ..plain()
    };
    let a = write(&w, &options).unwrap();
    assert_eq!(serde_json::to_value(&a.index).unwrap()["format_version"], "2.0");
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
        let g = primase_glb::Gltf::from_glb(&a.files[&t.file]).unwrap();
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
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(
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
    *m.primitive_mut(0).expect("a listed primitive").indices_mut() = Some(vec![0, 1, 2, 3, 4, 5]);
    w.insert(e, m);
    w.insert(e, Transform::IDENTITY);
    let mut o = plain();
    o.split_by_tile.insert(e);
    let tiles = write(&w, &o).unwrap();
    assert_eq!(tiles.index.tiles.len(), 2);
    assert_eq!(tiles.index.tiles[1].min[0], 199.);
    assert_eq!(tiles.index.tiles[1].max[0], 203.);
    for t in &tiles.index.tiles {
        assert_eq!(t.triangles, 1);
        let g = primase_glb::Gltf::from_glb(&tiles.files[&t.file]).unwrap();
        let mesh = g.read_mesh(0).unwrap().to_mesh();
        assert_eq!(mesh.primitives()[0].vertex_count(), 3);
        assert_eq!(mesh.primitives()[0].indices, Some(vec![0, 1, 2]));
    }
}
#[test]
fn bad_meshes_and_options_are_errors() {
    let mut w = World::new();
    let e = w.spawn();
    let mut m = mesh();
    *m.primitive_mut(0).expect("a listed primitive").indices_mut() = Some(vec![0, 1, 9]);
    w.insert(e, m);
    w.insert(e, Transform::IDENTITY);
    assert!(write(&w, &plain()).is_err());
    w.insert(e, mesh());
    // a tile size is finite and above 0, checked where it is made
    assert!(primase_web::TileSize::new(0.).is_err() && primase_web::TileSize::new(f64::NAN).is_err());
    let mut t = Transform::IDENTITY;
    t.cols[0] = [0.; 3];
    w.insert(e, t);
    let out = write(&w, &plain()).unwrap();
    assert!(out.index.tiles.is_empty());
    assert_eq!(out.index.skipped_degenerate, 1);
}

#[test]
fn validator_and_index_file() {
    let Some(validator) = primase_testdata::real_data("PRIMASE_GLTF_VALIDATOR") else {
        return;
    };
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, mesh());
    w.insert(e, Transform::IDENTITY);
    let set = write(&w, &plain()).unwrap();
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
fn instancing_trs_and_baked_mirrors_shears() {
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
    // the writer takes rigid placements: the mirror and the shear are baked first
    let refused = write(&w, &plain()).unwrap_err().to_string();
    assert!(refused.contains("bake-nonrigid"), "{refused}");
    primase_meshops::bake_nonrigid(&mut w, &primase_meshops::BakeNonrigid::default());
    let out = write(&w, &plain()).unwrap();
    let tile = &out.index.tiles[0];
    assert_eq!(tile.placements, 5);
    assert_eq!(tile.instanced_placements, 3);
    let bytes = &out.files[&tile.file];
    assert_eq!(*bytes, write(&w, &plain()).unwrap().files[&tile.file]);
    let chunks = primase_glb::parse(bytes).unwrap();
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
    if let Some(validator) = primase_testdata::real_data("PRIMASE_GLTF_VALIDATOR") {
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

#[test]
fn quantized_streams_decode_exactly_and_validate() {
    let mut w = World::new();
    let p = w.spawn();
    let mut m = mesh();
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(Semantic::Normal, Stream::F32x3(vec![[0., 1., 0.]; 3]));
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(Semantic::Tangent, Stream::F32x4(vec![[1., 0., 0., -1.]; 3]));
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(
            Semantic::TexCoord(0),
            Stream::F32x2(vec![[0., 0.5], [1., 0.], [0., 1.]]),
        );
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(
            Semantic::TexCoord(1),
            Stream::F32x2(vec![[0., 0.], [2., 0.], [0., 2.]]),
        );
    w.insert(p, m);
    for x in [0., 10.] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3][0] = x;
        w.insert(e, t);
    }
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    assert_eq!(
        set.files,
        write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap()))
            .unwrap()
            .files
    );
    let bytes = set.files.values().next().unwrap();
    let chunks = primase_glb::parse(bytes).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let a = &doc["accessors"][doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize];
    assert_eq!(a["componentType"], 5122);
    let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let ext = &view["extensions"]["EXT_meshopt_compression"];
    let start = ext["byteOffset"].as_u64().unwrap() as usize;
    let end = start + ext["byteLength"].as_u64().unwrap() as usize;
    let decoded = meshopt::decode_vertex_buffer::<[u8; 8]>(&chunks.bin.unwrap()[start..end], 3).unwrap();
    let values: Vec<_> = decoded
        .iter()
        .map(|v| {
            [
                i16::from_le_bytes([v[0], v[1]]),
                i16::from_le_bytes([v[2], v[3]]),
                i16::from_le_bytes([v[4], v[5]]),
            ]
        })
        .collect();
    // bounds x 0..1, y 0, z 0..1: centre (0.5, 0, 0.5), one step 0.5/32767 (rounded up to reach the bounds)
    assert_eq!(
        values,
        vec![[-32767, 0, -32767], [32767, 0, -32767], [-32767, 0, 32767]]
    );
    // the two placements, instanced, carry the frame: translation by the centre, scale by the step
    let node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n.get("mesh").is_some())
        .unwrap();
    let streams = &node["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
    let bin = chunks.bin.unwrap();
    let translation: Vec<[f32; 3]> = stream(&doc, bin, streams["TRANSLATION"].as_u64().unwrap());
    let scale: Vec<[f32; 3]> = stream(&doc, bin, streams["SCALE"].as_u64().unwrap());
    assert_eq!(translation, vec![[0.5, 0., 0.5], [10.5, 0., 0.5]]);
    let step = scale[0][0];
    assert_eq!(scale, vec![[step; 3]; 2]);
    assert!(f64::from(step) * 32767. >= 0.5 && f64::from(step.next_down()) * 32767. < 0.5);
    let q = &doc["asset"]["extras"]["primase.quantization"];
    assert_eq!(q["position_centre"], "bounds");
    assert_eq!(q["meshes_float_positions"], 0);
    assert!(q["position_max_world_error"].as_f64().unwrap() < 1e-6, "{q}");
    let attrs = &doc["meshes"][0]["primitives"][0]["attributes"];
    for (name, component) in [
        ("NORMAL", 5122),
        ("TANGENT", 5122),
        ("TEXCOORD_0", 5123),
        ("TEXCOORD_1", 5126),
    ] {
        let a = &doc["accessors"][attrs[name].as_u64().unwrap() as usize];
        assert_eq!(a["componentType"], component, "{name}");
        let e = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize]["extensions"]["EXT_meshopt_compression"];
        let start = e["byteOffset"].as_u64().unwrap() as usize;
        let end = start + e["byteLength"].as_u64().unwrap() as usize;
        if name == "NORMAL" {
            let v = meshopt::decode_vertex_buffer::<[i16; 4]>(&chunks.bin.unwrap()[start..end], 3).unwrap();
            assert_eq!(v, vec![[0, 32767, 0, 0]; 3]);
        } else if name == "TANGENT" {
            let v = meshopt::decode_vertex_buffer::<[i16; 4]>(&chunks.bin.unwrap()[start..end], 3).unwrap();
            assert_eq!(v, vec![[32767, 0, 0, -32767]; 3]);
        } else if name == "TEXCOORD_0" {
            let v = meshopt::decode_vertex_buffer::<[u16; 2]>(&chunks.bin.unwrap()[start..end], 3).unwrap();
            assert_eq!(v, vec![[0, 32768], [65535, 0], [0, 65535]]);
        } else {
            let v = meshopt::decode_vertex_buffer::<[f32; 2]>(&chunks.bin.unwrap()[start..end], 3).unwrap();
            assert_eq!(v, vec![[0., 0.], [2., 0.], [0., 2.]]);
        }
    }
    // what the encoding approximated is reported: the two instanced placements, the one quantized position stream,
    // and NORMAL, TANGENT and TEXCOORD_0 (TEXCOORD_1 repeats, kept f32)
    let approximated: Vec<(String, u64)> = set
        .notices
        .iter()
        .filter(|n| n.kind == primase_core::convert::NoticeKind::Approximation)
        .map(|n| (n.reason.split(' ').take(2).collect::<Vec<_>>().join(" "), n.count))
        .collect();
    assert_eq!(
        approximated,
        [
            ("normal, tangent".to_owned(), 3),
            ("placements instanced".to_owned(), 2),
            ("position streams".to_owned(), 1),
        ]
    );
    if let Some(validator) = primase_testdata::real_data("PRIMASE_GLTF_VALIDATOR") {
        let path = std::env::temp_dir().join(format!("primase-web-compressed-{}.glb", std::process::id()));
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

/// The values of a meshopt-coded f32 accessor.
fn stream<T: Default + Clone>(doc: &serde_json::Value, bin: &[u8], accessor: u64) -> Vec<T> {
    let a = &doc["accessors"][accessor as usize];
    let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let e = &view["extensions"]["EXT_meshopt_compression"];
    let start = e["byteOffset"].as_u64().unwrap() as usize;
    let end = start + e["byteLength"].as_u64().unwrap() as usize;
    meshopt::decode_vertex_buffer::<T>(&bin[start..end], a["count"].as_u64().unwrap() as usize).unwrap()
}

/// One self-placed mesh with `positions` (triangles 0 1 2 and 0 2 3), its tile's document and BIN.
fn one_mesh(positions: Vec<[f32; 3]>) -> (serde_json::Value, Vec<u8>) {
    let mut w = World::new();
    let e = w.spawn();
    let mut m = mesh();
    *m.primitive_mut(0).expect("a listed primitive").indices_mut() = Some(vec![0, 1, 2, 0, 2, 3]);
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(Semantic::Position, Stream::F32x3(positions));
    w.insert(e, m);
    w.insert(e, Transform::IDENTITY);
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    let bytes = set.files.values().next().unwrap();
    let chunks = primase_glb::parse(bytes).unwrap();
    (
        serde_json::from_slice(chunks.json).unwrap(),
        chunks.bin.unwrap().to_vec(),
    )
}

#[test]
fn positions_far_from_the_origin_are_centred_and_a_mesh_over_budget_keeps_f32() {
    // 2 km out, 1 m across: centred on (2000.5, 0, 0.5), step 0.5/32767 (15 um), so the node is moved to the centre
    // and scaled by the step; origin-centred it would have been 2001/32767 (61 mm)
    let (doc, bin) = one_mesh(vec![
        [2000., 0., 0.],
        [2001., 0., 0.],
        [2001., 0., 1.],
        [2000., 0., 1.],
    ]);
    let node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n.get("mesh").is_some())
        .unwrap();
    assert_eq!(node["translation"], serde_json::json!([2000.5, 0., 0.5]));
    let step = node["scale"][0].as_f64().unwrap();
    assert!(
        step * 32767. >= 0.5 && step < 0.5 / 32767. * (1. + 1e-6),
        "{step}"
    );
    let position = doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap();
    assert_eq!(doc["accessors"][position as usize]["componentType"], 5122);
    let q: Vec<[i16; 4]> = stream(&doc, &bin, position);
    assert_eq!(
        q,
        vec![
            [-32767, 0, -32767, 0],
            [32767, 0, -32767, 0],
            [32767, 0, 32767, 0],
            [-32767, 0, 32767, 0]
        ]
    );
    let extras = &doc["asset"]["extras"]["primase.quantization"];
    assert!(
        extras["position_max_world_error"].as_f64().unwrap() < 1e-6,
        "{extras}"
    );

    // 100 m across: the step is 50/32767 (1.526 mm), and the vertex 0.6 mm along x decodes to 0, 0.6 mm from where
    // it was, over the 0.25 mm budget, so the positions stay f32 and the node as it was
    let (doc, bin) = one_mesh(vec![
        [0., 0., 0.],
        [100., 0., 0.],
        [0., 0., 100.],
        [0.0006, 0., 0.],
    ]);
    let position = doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap();
    assert_eq!(doc["accessors"][position as usize]["componentType"], 5126);
    let p: Vec<[f32; 3]> = stream(&doc, &bin, position);
    assert_eq!(
        p,
        vec![[0., 0., 0.], [100., 0., 0.], [0., 0., 100.], [0.0006, 0., 0.]]
    );
    let node = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n.get("mesh").is_some())
        .unwrap();
    assert!(node.get("translation").is_none() && node.get("scale").is_none() && node.get("matrix").is_none());
    let extras = &doc["asset"]["extras"]["primase.quantization"];
    assert_eq!(extras["meshes_float_positions"], 1);
    assert_eq!(extras["position_budget"], 0.00025);
}

#[test]
fn viewer_records_make_metadata_only_tiles_and_keep_payloads() {
    let item = primase_web::ViewerItem {
        name: "spawn / test".into(),
        kind: "spawn".into(),
        anchor: [-1., 4., 201.],
        data: serde_json::json!({"yaw":1.5,"snap":"street"}),
    };
    let options = Options {
        viewer_items: vec![item.clone()],
        ..Options::new(primase_web::TileSize::new(200.).unwrap())
    };
    let set = write(&World::new(), &options).unwrap();
    assert_eq!(set.index.tiles.len(), 1);
    let tile = &set.index.tiles[0];
    assert_eq!(tile.content, primase_web::TileContent::MetadataOnly);
    assert_eq!(
        (tile.col, tile.row, tile.viewer_items, tile.triangles),
        (-1, -2, 1, 0)
    );
    let chunks = primase_glb::parse(&set.files[&tile.file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    assert_eq!(doc["nodes"][0]["name"], "Viewer_spawn___test_0");
    assert!(doc.get("meshes").is_none());
    assert!(doc.get("buffers").is_none());
    assert!(chunks.bin.is_none_or(|b| b.is_empty()));
    assert_eq!(
        doc["nodes"][0]["extras"]["primase"]["web.viewer"]["value"]["data"],
        item.data
    );
    assert_eq!(set.files, write(&World::new(), &options).unwrap().files);
}

#[test]
fn the_worlds_curves_and_markers_are_viewer_records() {
    use primase_core::{Curve, Marker, Name, Opaque, OpaqueData};
    let mut w = World::new();
    let rail = w.spawn();
    w.insert(
        rail,
        Curve {
            kind: "rail".into(),
            points: vec![[5., 1., -5.], [6., 1., -5.]],
            closed: false,
            tangents: None,
            up: None,
        },
    );
    w.insert(rail, Name("rail a".into()));
    w.insert_opaque(
        rail,
        "test.dialect",
        Opaque {
            version: 3,
            data: OpaqueData::Json(serde_json::json!({"surface": 4})),
        },
    );
    let spawn = w.spawn();
    let mut t = Transform::IDENTITY;
    t.cols[3] = [10., 2., -20.];
    w.insert(spawn, t);
    w.insert(
        spawn,
        Marker {
            kind: "spawn".into(),
            ground: None,
            yaw: None,
        },
    );
    let stop = w.spawn();
    w.insert(
        stop,
        Marker {
            kind: "travel".into(),
            ground: Some([12., 40.]),
            yaw: None,
        },
    );
    let options = Options {
        viewer_items: vec![primase_web::ViewerItem {
            name: "own".into(),
            kind: "marker".into(),
            anchor: [1., 0., -1.],
            data: serde_json::json!(null),
        }],
        ..Options::new(primase_web::TileSize::new(200.).unwrap())
    };
    // counted as the records are written, from each cell's world
    let mut plan = primase_web::plan(&w, &options).unwrap();
    assert_eq!(plan.ground_markers(), 0);
    for cell in plan.cells() {
        plan.write_cell(cell, &w.subset(&w.closure(plan.roots(cell))))
            .unwrap();
    }
    assert_eq!(plan.ground_markers(), 1);
    let set = write(&w, &options).unwrap();
    let mut records = Vec::new();
    for tile in &set.index.tiles {
        let chunks = primase_glb::parse(&set.files[&tile.file]).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
        for node in doc["nodes"].as_array().unwrap() {
            records.push((node["name"].clone(), node["extras"]["primase"].clone()));
        }
    }
    records.sort_by_key(|(name, _)| name.as_str().unwrap().to_owned());
    let viewer = |kind: &str, name: &str, anchor: [f64; 3], data: serde_json::Value| serde_json::json!({"version": 1, "value": {"name": name, "kind": kind, "anchor": anchor, "data": data}});
    assert_eq!(
        records,
        [
            (
                "Viewer_Entity_1_2".into(),
                serde_json::json!({"web.viewer": viewer("spawn", "Entity_1", [10., 2., -20.], serde_json::json!({"kind": "spawn", "ground": null}))})
            ),
            (
                "Viewer_Entity_2_3".into(),
                serde_json::json!({"web.viewer": viewer("marker", "Entity_2", [12., 0., -40.], serde_json::json!({"kind": "travel", "ground": [12., 40.]}))})
            ),
            (
                "Viewer_own_0".into(),
                serde_json::json!({"web.viewer": viewer("marker", "own", [1., 0., -1.], serde_json::json!(null))})
            ),
            (
                "Viewer_rail_a_1".into(),
                serde_json::json!({
                    "web.viewer": viewer("grind_curve", "rail a", [5., 1., -5.], serde_json::json!({"kind": "rail", "points": [[5., 1., -5.], [6., 1., -5.]], "closed": false})),
                    "test.dialect": {"version": 3, "value": {"surface": 4}},
                })
            ),
        ]
    );
}

#[test]
fn empty_prototypes_are_counted_but_unloaded_files_are_errors() {
    let mut w = World::new();
    let p = w.spawn();
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype: p,
            variant: 0,
        },
    );
    w.insert(e, Transform::IDENTITY);
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    assert_eq!(set.index.placements_without_mesh, 1);
    assert!(set.files.is_empty());
    w.insert(
        p,
        primase_core::MeshFile {
            path: "missing.glb".into(),
            mesh: 0,
            materials: Vec::new(),
        },
    );
    assert!(write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).is_err());
}

#[test]
fn collision_payload_is_external_and_referenced() {
    let payload = serde_json::json!({"positions":[[0.,0.,0.],[1.,0.,0.],[0.,1.,0.]],"triangles":[[0,1,2]],"surfaces":[0]});
    let set = write(
        &World::new(),
        &Options {
            viewer_items: vec![primase_web::ViewerItem {
                name: "collision".into(),
                kind: "collision".into(),
                anchor: [0.; 3],
                data: payload.clone(),
            }],
            ..Options::new(primase_web::TileSize::new(200.).unwrap())
        },
    )
    .unwrap();
    let tile = &set.index.tiles[0];
    let collision = tile.collision.as_ref().unwrap();
    let bytes = &set.collision_files[&collision.file];
    use sha2::{Digest, Sha256};
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    assert!(collision.file.ends_with(&format!(".{hash}.glb")));
    assert_eq!(bytes.len() as u64, collision.bytes);
    let collision_chunks = primase_glb::parse(bytes).unwrap();
    let collision_doc: serde_json::Value = serde_json::from_slice(collision_chunks.json).unwrap();
    assert_eq!(collision_doc["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(collision_doc["accessors"][2]["count"], 1);
    assert_eq!(
        &collision_chunks.bin.unwrap()[36..48],
        &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(collision_chunks.bin.unwrap()[48], 0);
    let chunks = primase_glb::parse(&set.files[&tile.file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let record = &doc["nodes"][0]["extras"]["primase"]["web.viewer"]["value"];
    assert_eq!(record["kind"], "collision_reference");
    assert_eq!(record["data"]["file"], collision.file);
    assert!(record["data"].get("positions").is_none());
    assert!(doc.get("meshes").is_none());
}

#[test]
fn ground_north_matches_shared_frame() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, mesh());
    let mut t = Transform::IDENTITY;
    t.cols[3][2] = -250.;
    w.insert(e, t);
    let set = write(&w, &plain()).unwrap();
    assert_eq!((set.index.tiles[0].col, set.index.tiles[0].row), (0, 1));
    assert_eq!(primase_core::frame::ground(t.cols[3]), [0., 250.]);
}

#[test]
fn compressed_plain_nodes_omit_absent_matrix() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, mesh());
    w.insert(e, Transform::IDENTITY);
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    let chunks = primase_glb::parse(set.files.values().next().unwrap()).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let node = &doc["nodes"][0];
    // 1 m across: the frame is centred on (0.5, 0, 0.5) with step 0.5/32767, the smallest f32 at or above it
    // (1.5259257e-05, widening to 1.525925654277671e-05). A compressed node keeps translation and scale; it never
    // gains the matrix the source transform did not have.
    assert!(node.get("matrix").is_none(), "{node}");
    assert_eq!(node["translation"], serde_json::json!([0.5, 0.0, 0.5]));
    let step = f64::from(1.525_925_7e-5_f32);
    assert_eq!(node["scale"], serde_json::json!([step, step, step]));
}

#[test]
fn texture_transforms_survive_instancing_and_compression() {
    use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let mut w = World::new();
    let tex = w.spawn();
    w.insert(
        tex,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![200, 100, 50, 255]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let mat = w.spawn();
    w.insert(
        mat,
        PbrMaterial {
            base_color_texture: Some(TextureRef {
                transform: Some(UvTransform {
                    offset: [0.5, 0.25],
                    scale: [2.0, 2.0],
                    rotation: 0.0,
                }),
                ..TextureRef::new(tex)
            }),
            ..PbrMaterial::default()
        },
    );
    let p = w.spawn();
    let mut m = mesh();
    *m.primitive_mut(0).expect("a listed primitive").material_mut() = Some(mat);
    m.primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(
            Semantic::TexCoord(0),
            Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
        );
    w.insert(p, m);
    for x in [0., 1., 2.] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3][0] = x;
        w.insert(e, t);
    }
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    assert_eq!(set.index.tiles[0].instanced_placements, 3);
    let chunks = primase_glb::parse(&set.files[&set.index.tiles[0].file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let used: Vec<&str> = doc["extensionsUsed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for e in [
        "KHR_texture_transform",
        "EXT_mesh_gpu_instancing",
        "EXT_meshopt_compression",
    ] {
        assert!(used.contains(&e), "{e} not in {used:?}");
    }
    assert!(
        !doc["extensionsRequired"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "KHR_texture_transform"),
        "a transform is optional"
    );
    assert_eq!(
        doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"],
        serde_json::json!({"offset": [0.5, 0.25], "scale": [2.0, 2.0]})
    );
}

#[test]
fn world_collision_is_placed_and_named() {
    use primase_core::{Collision, Surfaces};
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let (snow, ice) = (surfaces.index("Snow").unwrap(), surfaces.index("Ice").unwrap());
    w.insert_resource(surfaces);
    let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];
    // a prototype's collision, placed twice
    let p = w.spawn();
    let mut c = Collision::default();
    c.extend(&tri, &[[0, 1, 2]], ice, |q| q);
    w.insert(p, c);
    for x in [0.0, 5.0] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 1.0, 0.0];
        w.insert(e, t);
    }
    // an entity's own collision, through its transform
    let s = w.spawn();
    let mut c = Collision::default();
    c.extend(&tri, &[[0, 1, 2]], snow, |q| q);
    w.insert(s, c);
    let mut t = Transform::IDENTITY;
    t.cols[3] = [2.0, 0.0, 0.0];
    w.insert(s, t);
    let set = write(&w, &plain()).unwrap();
    let mut firsts = Vec::new();
    for tile in &set.index.tiles {
        let Some(c) = &tile.collision else { continue };
        let chunks = primase_glb::parse(&set.collision_files[&c.file]).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
        let bin = chunks.bin.unwrap();
        for m in doc["meshes"].as_array().unwrap() {
            let prim = &m["primitives"][0];
            let ext = &prim["extras"]["primase.collision_surfaces"];
            assert_eq!(ext["version"], 2);
            assert_eq!(ext["names"], serde_json::json!(["Snow", "Ice"]));
            let a = &doc["accessors"][ext["accessor"].as_u64().unwrap() as usize];
            assert_eq!(a["componentType"], 5123, "u16 surfaces");
            let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
            let off = v["byteOffset"].as_u64().unwrap() as usize;
            let surface = u16::from_le_bytes([bin[off], bin[off + 1]]);
            let pa = &doc["accessors"][prim["attributes"]["POSITION"].as_u64().unwrap() as usize];
            let pv = &doc["bufferViews"][pa["bufferView"].as_u64().unwrap() as usize];
            let po = pv["byteOffset"].as_u64().unwrap() as usize;
            let x = f32::from_le_bytes(bin[po..po + 4].try_into().unwrap());
            let y = f32::from_le_bytes(bin[po + 4..po + 8].try_into().unwrap());
            firsts.push((x, y, surface));
        }
    }
    firsts.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(firsts, [(0.0, 1.0, ice), (2.0, 0.0, snow), (5.0, 1.0, ice)]);
}

/// The first triangle's normal (counter-clockwise front) and surface of each collision mesh in the set.
#[allow(clippy::unwrap_used)]
fn collision_fronts(set: &primase_web::TileSet) -> Vec<([f32; 3], u16)> {
    let mut out = Vec::new();
    for tile in &set.index.tiles {
        let Some(c) = &tile.collision else { continue };
        let chunks = primase_glb::parse(&set.collision_files[&c.file]).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
        let bin = chunks.bin.unwrap();
        let view = |a: &serde_json::Value| {
            let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
            v["byteOffset"].as_u64().unwrap() as usize
        };
        for m in doc["meshes"].as_array().unwrap() {
            let prim = &m["primitives"][0];
            let pos = view(&doc["accessors"][prim["attributes"]["POSITION"].as_u64().unwrap() as usize]);
            let idx = view(&doc["accessors"][prim["indices"].as_u64().unwrap() as usize]);
            let ext = &prim["extras"]["primase.collision_surfaces"];
            let sur = view(&doc["accessors"][ext["accessor"].as_u64().unwrap() as usize]);
            let u32_at = |o: usize| u32::from_le_bytes(bin[o..o + 4].try_into().unwrap());
            let p = |i: u32| -> [f32; 3] {
                let o = pos + 12 * i as usize;
                std::array::from_fn(|k| f32::from_le_bytes(bin[o + 4 * k..o + 4 * k + 4].try_into().unwrap()))
            };
            let [a, b, c] = [0, 1, 2].map(|k| p(u32_at(idx + 4 * k)));
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            out.push((n, u16::from_le_bytes([bin[sur], bin[sur + 1]])));
        }
    }
    out
}

#[test]
fn mirrored_collision_keeps_its_fronts_and_wide_surfaces() {
    use primase_core::{Collision, Surfaces};
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    for i in 0..300 {
        surfaces.index(&format!("s{i}")).unwrap();
    }
    let wide = surfaces.index("s299").unwrap();
    w.insert_resource(surfaces);
    // a triangle facing up (+y, counter-clockwise from above)
    let p = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        &[[0, 1, 2]],
        wide,
        |q| q,
    );
    w.insert(p, c);
    for mirror in [false, true] {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: p,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        if mirror {
            t.cols[0] = [-1.0, 0.0, 0.0];
        }
        t.cols[3] = [if mirror { 5.0 } else { 0.0 }, 0.0, 0.0];
        w.insert(e, t);
    }
    primase_meshops::bake_nonrigid(&mut w, &primase_meshops::BakeNonrigid::default());
    let fronts = collision_fronts(&write(&w, &plain()).unwrap());
    assert_eq!(fronts.len(), 2);
    for (n, s) in fronts {
        assert!(n[1] > 0.0, "the front faces up, mirrored or not: {n:?}");
        assert_eq!(s, 299, "a surface above 255 survives");
    }
}

#[test]
fn invalid_collision_records_are_refused() {
    let item = |data: serde_json::Value| Options {
        viewer_items: vec![primase_web::ViewerItem {
            name: "c".into(),
            kind: "collision".into(),
            anchor: [0.0; 3],
            data,
        }],
        ..plain()
    };
    let tri = serde_json::json!([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]);
    for (what, data) in [
        (
            "surface past the names",
            serde_json::json!({"positions": tri, "triangles": [[0, 1, 2]], "surfaces": [3], "names": ["a"]}),
        ),
        (
            "a surface per triangle",
            serde_json::json!({"positions": tri, "triangles": [[0, 1, 2]], "surfaces": []}),
        ),
        (
            "index past the positions",
            serde_json::json!({"positions": tri, "triangles": [[0, 1, 9]], "surfaces": [0]}),
        ),
        (
            "surface out of u16",
            serde_json::json!({"positions": tri, "triangles": [[0, 1, 2]], "surfaces": [70000]}),
        ),
    ] {
        assert!(write(&World::new(), &item(data)).is_err(), "{what}");
    }
}
#[test]
fn lights_are_punctual_nodes_in_their_cell_and_bump_the_minor_version() {
    use primase_core::{Light, LightKind};
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, mesh());
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placement, Transform::IDENTITY);
    assert_eq!(write(&w, &plain()).unwrap().index.format_version, "2.0");
    for (x, kind) in [
        (5.0f32, LightKind::Point),
        (
            450.0,
            LightKind::Spot {
                inner_cone: 0.2,
                outer_cone: 0.4,
            },
        ),
    ] {
        let e = w.spawn();
        w.insert(
            e,
            Light {
                kind,
                color: [1.0, 0.5, 0.25],
                intensity: 7.0,
                range: Some(12.0),
                cast_shadows: None,
            },
        );
        w.insert(
            e,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, 3.0, 0.0]],
            },
        );
    }
    let set = write(&w, &plain()).unwrap();
    assert_eq!(set.index.format_version, "2.0");
    let tiles: Vec<_> = set
        .index
        .tiles
        .iter()
        .map(|t| (t.col, t.row, t.lights, t.triangles > 0))
        .collect();
    assert_eq!(tiles, [(0, 0, 1, true), (2, 0, 1, false)]);
    let index = serde_json::to_value(&set.index).unwrap();
    assert_eq!(index["tiles"][1]["content"], "metadata_only");
    for (tile, kind) in set.index.tiles.iter().zip(["point", "spot"]) {
        let bytes = &set.files[&tile.file];
        let doc: serde_json::Value = serde_json::from_slice(primase_glb::parse(bytes).unwrap().json).unwrap();
        assert!(
            doc["extensionsUsed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u == "KHR_lights_punctual")
        );
        assert!(
            doc.get("extensionsRequired").is_none_or(|r| !r
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u == "KHR_lights_punctual"))
        );
        let lights = doc["extensions"]["KHR_lights_punctual"]["lights"]
            .as_array()
            .unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0]["type"], kind);
        assert_eq!(lights[0]["intensity"], 7.0);
        assert_eq!(lights[0]["range"], 12.0);
        let node = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n.get("extensions").is_some())
            .unwrap();
        assert_eq!(node["extensions"]["KHR_lights_punctual"]["light"], 0);
        assert_eq!(node["matrix"][13], 3.0);
        assert!(tile.min[1] <= 3.0 && tile.max[1] >= 3.0);
    }
}
#[test]
fn emissive_strength_survives_compression_and_instancing() {
    use primase_core::pbr::PbrMaterial;
    let mut w = World::new();
    let mat = w.spawn();
    w.insert(
        mat,
        PbrMaterial {
            emissive: [1.0, 0.5, 0.0],
            emissive_strength: 3.0,
            ..PbrMaterial::default()
        },
    );
    let proto = w.spawn();
    let mut m = mesh();
    *m.primitive_mut(0).expect("a listed primitive").material_mut() = Some(mat);
    w.insert(proto, m);
    for x in [0.0f32, 2.0] {
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
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    assert_eq!(set.index.tiles[0].instanced_placements, 2);
    let chunks = primase_glb::parse(&set.files[&set.index.tiles[0].file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    assert_eq!(
        doc["materials"][0]["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
        3.0
    );
    assert!(
        doc["extensionsUsed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_emissive_strength")
    );
}
#[test]
fn shadows_split_instance_groups_and_stay_on_their_nodes() {
    use primase_core::{ShadowCast, Shadows};
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, mesh());
    let proxy = Shadows {
        cast: Some(ShadowCast::Only),
        receive: Some(false),
    };
    for (x, shadows) in [
        (0.0f32, Some(proxy)),
        (2.0, None),
        (4.0, Some(proxy)),
        (6.0, None),
    ] {
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
        if let Some(s) = shadows {
            w.insert(e, s);
        }
    }
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    assert_eq!(set.index.tiles[0].instanced_placements, 4);
    let chunks = primase_glb::parse(&set.files[&set.index.tiles[0].file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let instanced: Vec<_> = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["extensions"].get("EXT_mesh_gpu_instancing").is_some())
        .collect();
    assert_eq!(instanced.len(), 2, "one instanced node per shadow setting");
    let mut seen: Vec<_> = instanced
        .iter()
        .map(|n| n["extras"].get("primase.shadows").cloned())
        .collect();
    seen.sort_by_key(|v| v.is_some());
    assert_eq!(
        seen,
        vec![
            None,
            Some(serde_json::json!({"version": 1, "cast": "only", "receive": false}))
        ]
    );
    let uninstanced = Options {
        gpu_instancing: false,
        ..plain()
    };
    let plain = write(&w, &uninstanced).unwrap();
    let chunks = primase_glb::parse(&plain.files[&plain.index.tiles[0].file]).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    let with: usize = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["extras"].get("primase.shadows").is_some())
        .count();
    assert_eq!(with, 2, "uninstanced: on each placement's node");
}
#[test]
fn vertex_colours_survive_compression_and_instancing() {
    use primase_core::pbr::{AlphaMode, PbrMaterial};
    // GTA's decal parts: white, the fade in alpha (normalised u8)
    let colours = vec![[255, 255, 255, 0], [255, 255, 255, 11], [255, 255, 255, 50]];
    for compress in [false, true] {
        let mut w = World::new();
        let mat = w.spawn();
        w.insert(
            mat,
            PbrMaterial {
                alpha_mode: AlphaMode::Blend,
                ..PbrMaterial::default()
            },
        );
        let proto = w.spawn();
        let mut m = mesh();
        *m.primitive_mut(0).expect("a listed primitive").material_mut() = Some(mat);
        m.primitive_mut(0)
            .expect("a listed primitive")
            .streams_mut()
            .insert(Semantic::Color(0), Stream::U8x4(colours.clone()));
        w.insert(proto, m);
        for x in [0.0f32, 2.0] {
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
            compress,
            ..Options::new(primase_web::TileSize::new(200.).unwrap())
        };
        let set = write(&w, &options).unwrap();
        assert_eq!(set.index.tiles[0].instanced_placements, 2);
        let chunks = primase_glb::parse(&set.files[&set.index.tiles[0].file]).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
        let prim = &doc["meshes"][0]["primitives"][0];
        let a = &doc["accessors"][prim["attributes"]["COLOR_0"].as_u64().unwrap() as usize];
        assert_eq!(
            (&a["componentType"], &a["type"], &a["normalized"], &a["count"]),
            (&5121.into(), &"VEC4".into(), &true.into(), &3.into()),
            "compress={compress}"
        );
        let view = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let bin = chunks.bin.unwrap();
        let decoded: Vec<[u8; 4]> = match view["extensions"].get("EXT_meshopt_compression") {
            Some(ext) => {
                let start = ext["byteOffset"].as_u64().unwrap() as usize;
                let end = start + ext["byteLength"].as_u64().unwrap() as usize;
                meshopt::decode_vertex_buffer::<[u8; 4]>(&bin[start..end], 3).unwrap()
            }
            None => {
                let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
                    + a["byteOffset"].as_u64().unwrap_or(0) as usize;
                bin[start..start + 12].as_chunks::<4>().0.to_vec()
            }
        };
        assert_eq!(
            view["extensions"].get("EXT_meshopt_compression").is_some(),
            compress
        );
        assert_eq!(decoded, colours, "COLOR_0 exact (compress={compress})");
        assert_eq!(
            doc["materials"][prim["material"].as_u64().unwrap() as usize]["alphaMode"],
            "BLEND"
        );
    }
}
#[test]
fn layered_materials_survive_tiling_and_compression() {
    use primase_core::pbr::PbrMaterial;
    use primase_core::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
    for compress in [false, true] {
        let mut w = World::new();
        let layer = |w: &mut World, r: f32| {
            let e = w.spawn();
            w.insert(
                e,
                PbrMaterial {
                    base_color: [r, 0.5, 0.25, 1.0],
                    ..PbrMaterial::default()
                },
            );
            Some(Layer {
                material: e,
                texcoord: None,
                transform: None,
            })
        };
        let layers = vec![layer(&mut w, 0.1), layer(&mut w, 0.2), None, layer(&mut w, 0.4)];
        let host = w.spawn();
        w.insert(
            host,
            LayeredMaterial {
                layers,
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
            },
        );
        let proto = w.spawn();
        let mut m = mesh();
        *m.primitive_mut(0).expect("a listed primitive").material_mut() = Some(host);
        m.primitive_mut(0)
            .expect("a listed primitive")
            .streams_mut()
            .insert(
                Semantic::Color(1),
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0.5, 1., 1.]]),
            );
        // the layers are sampled with set 0
        m.primitive_mut(0)
            .expect("a listed primitive")
            .streams_mut()
            .insert(
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]),
            );
        w.insert(proto, m);
        let place = w.spawn();
        w.insert(
            place,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(place, Transform::IDENTITY);
        let options = Options {
            compress,
            ..Options::new(primase_web::TileSize::new(200.).unwrap())
        };
        let set = write(&w, &options).unwrap();
        assert_eq!(set.index.format_version, "2.0");
        assert_eq!(set.index.tiles[0].layered_materials, 1);
        let index = serde_json::to_value(&set.index).unwrap();
        assert_eq!(index["tiles"][0]["layered_materials"], 1);
        let bytes = &set.files[&set.index.tiles[0].file];
        let doc: serde_json::Value = serde_json::from_slice(primase_glb::parse(bytes).unwrap().json).unwrap();
        assert!(
            doc["extensionsUsed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "PRIMASE_layered_material")
        );
        assert!(
            !doc["extensionsRequired"]
                .as_array()
                .is_some_and(|r| r.iter().any(|e| e == "PRIMASE_layered_material"))
        );
        let hosts: Vec<_> = doc["materials"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["extensions"].get("PRIMASE_layered_material").is_some())
            .collect();
        assert_eq!(hosts.len(), 1);
        let ext = &hosts[0]["extensions"]["PRIMASE_layered_material"];
        assert_eq!(ext["weights"]["attribute"], "COLOR_1");
        assert!(ext["layers"][2].is_null());
        assert_eq!(ext["layers"].as_array().unwrap().len(), 4);
        let prim = &doc["meshes"][0]["primitives"][0];
        assert!(
            prim["attributes"].get("COLOR_1").is_some(),
            "weights kept (compress={compress})"
        );
    }
}
#[test]
fn the_sun_is_indexed_scene_wide_and_bumps_the_minor_version() {
    use primase_core::{Environment, Light, LightKind, Name};
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, mesh());
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(placement, Transform::IDENTITY);
    let sun = w.spawn();
    w.insert(
        sun,
        Light {
            kind: LightKind::Directional,
            color: [1.0, 0.9, 0.8],
            intensity: 4.0,
            range: None,
            cast_shadows: Some(true),
        },
    );
    w.insert(sun, Name("Sun".into()));
    // pointing down and north: local -Z is (0, -1, -1) / sqrt 2; placed far from the geometry, in a cell of its own
    let h = std::f32::consts::FRAC_1_SQRT_2;
    w.insert(
        sun,
        Transform {
            cols: [[1., 0., 0.], [0., h, -h], [0., h, h], [900.0, 50.0, -900.0]],
        },
    );
    // without the environment the sun is a light of the cell holding its position
    let set = write(&w, &plain()).unwrap();
    assert_eq!(set.index.format_version, "2.0");
    assert_eq!(set.index.tiles.iter().map(|t| t.lights).sum::<u64>(), 1);
    assert!(
        serde_json::to_value(&set.index)
            .unwrap()
            .get("environment")
            .is_none()
    );
    w.insert_resource(Environment {
        sun: Some(sun),
        ambient: None,
    });
    let set = write(&w, &plain()).unwrap();
    assert_eq!(set.index.format_version, "2.1");
    let index = serde_json::to_value(&set.index).unwrap();
    let s = &index["environment"]["sun"];
    assert_eq!(
        (&s["name"], &s["intensity"], &s["cast"]),
        (
            &serde_json::json!("Sun"),
            &serde_json::json!(4.0),
            &serde_json::json!("on")
        )
    );
    let d: Vec<f64> = s["direction"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(d[0], 0.0);
    assert!(
        (d[1] + 0.5f64.sqrt()).abs() < 1e-6 && (d[2] + 0.5f64.sqrt()).abs() < 1e-6,
        "{d:?}"
    );
    assert!((d.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() < 1e-6);
    // not also a tile light, and no tile carries the environment
    assert_eq!(set.index.tiles.len(), 1);
    assert_eq!(set.index.tiles[0].lights, 0);
    for bytes in set.files.values() {
        let doc: serde_json::Value = serde_json::from_slice(primase_glb::parse(bytes).unwrap().json).unwrap();
        assert!(!doc.to_string().contains("core.environment"));
        assert!(
            doc.get("extensions")
                .is_none_or(|x| x.get("KHR_lights_punctual").is_none())
        );
    }
    // a sun that is not a directional light is refused
    w.get_mut::<Light>(sun).unwrap().kind = LightKind::Point;
    let err = write(&w, &plain()).unwrap_err().to_string();
    assert!(err.contains("not a directional core.light"), "{err}");
}

#[test]
fn a_starting_camera_goes_to_the_index_as_2_2() {
    let camera = primase_web::Camera {
        eye: [10., 50., 20.],
        target: [0., 0., -100.],
    };
    let with = |camera| Options {
        camera,
        viewer_items: vec![primase_web::ViewerItem {
            name: "a".into(),
            kind: "marker".into(),
            anchor: [0.; 3],
            data: serde_json::json!(null),
        }],
        ..Options::new(primase_web::TileSize::new(200.).unwrap())
    };
    let index = write(&World::new(), &with(Some(camera))).unwrap().index;
    assert_eq!(index.format_version, "2.2");
    let json = serde_json::to_value(&index).unwrap();
    assert_eq!(
        json["camera"],
        serde_json::json!({"eye": [10., 50., 20.], "target": [0., 0., -100.]})
    );
    // without one: 2.0 and no field
    let index = write(&World::new(), &with(None)).unwrap().index;
    assert_eq!(index.format_version, "2.0");
    assert!(serde_json::to_value(&index).unwrap().get("camera").is_none());
    // eye on the target, or not finite: refused
    for bad in [
        primase_web::Camera {
            eye: [1.; 3],
            target: [1.; 3],
        },
        primase_web::Camera {
            eye: [f64::NAN, 0., 0.],
            target: [0.; 3],
        },
    ] {
        let err = write(&World::new(), &with(Some(bad))).unwrap_err().to_string();
        assert!(err.contains("camera"), "{err}");
    }
}

/// A line-list prototype through `points` (consecutive points joined), placed at each of `xs` (metres east).
fn lines(w: &mut World, points: Vec<[f32; 3]>, xs: &[f32]) -> Entity {
    let segments = (1..points.len() as u32).flat_map(|i| [i - 1, i]).collect();
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Lines,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(points))]),
            indices: Some(segments),
            material: None,
        }]),
    );
    for &x in xs {
        place(w, proto, x);
    }
    proto
}
fn place(w: &mut World, prototype: Entity, x: f32) -> Entity {
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
    e
}
/// Each tile's glTF JSON.
fn docs(set: &primase_web::TileSet) -> Vec<serde_json::Value> {
    set.index
        .tiles
        .iter()
        .map(|t| serde_json::from_slice(primase_glb::parse(&set.files[&t.file]).unwrap().json).unwrap())
        .collect()
}
/// Each mesh node's primitive modes (absent: triangles, 4).
fn modes(doc: &serde_json::Value) -> Vec<Vec<u64>> {
    doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["mesh"].as_u64())
        .map(|m| {
            doc["meshes"][m as usize]["primitives"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["mode"].as_u64().unwrap_or(4))
                .collect()
        })
        .collect()
}

#[test]
fn a_tile_of_lines_alone_is_written_and_its_placements_are_not_instanced() {
    let mut w = World::new();
    lines(&mut w, vec![[0., 0., 0.], [1., 0., 0.]], &[10., 20.]);
    // compressed and instancing on, as by default
    let set = write(&w, &Options::new(primase_web::TileSize::new(200.).unwrap())).unwrap();
    let [tile] = &set.index.tiles[..] else {
        panic!("one tile: {:?}", set.index.tiles)
    };
    assert_eq!(tile.content, primase_web::TileContent::Geometry);
    assert_eq!(
        (tile.triangles, tile.placements, tile.instanced_placements),
        (0, 2, 0)
    );
    assert_eq!((tile.min[0], tile.max[0]), (10., 21.));
    // two line nodes: three.js instances triangle meshes alone
    assert_eq!(modes(&docs(&set)[0]), [vec![1], vec![1]]);
}

#[test]
fn lines_beside_triangles_are_written_with_them() {
    let mut w = World::new();
    let triangle = w.spawn();
    w.insert(triangle, mesh());
    place(&mut w, triangle, 10.);
    lines(&mut w, vec![[0., 0., 0.], [1., 0., 0.]], &[20.]);
    let set = write(&w, &plain()).unwrap();
    let [tile] = &set.index.tiles[..] else {
        panic!("one tile: {:?}", set.index.tiles)
    };
    assert_eq!((tile.triangles, tile.placements), (1, 2));
    let mut modes = modes(&docs(&set)[0]);
    modes.sort();
    assert_eq!(modes, [vec![1], vec![4]]);
}

#[test]
fn lines_filed_by_element_centre_go_to_the_cells_of_their_segments() {
    let mut w = World::new();
    // segments centred at 50 m and 200 m: cells 0 and 1
    let proto = lines(&mut w, vec![[0., 0., 0.], [100., 0., 0.], [300., 0., 0.]], &[]);
    let e = place(&mut w, proto, 0.);
    let options = Options {
        split_by_tile: [e].into(),
        ..plain()
    };
    let set = write(&w, &options).unwrap();
    let cells: Vec<_> = set
        .index
        .tiles
        .iter()
        .map(|t| (t.col, t.min[0], t.max[0]))
        .collect();
    assert_eq!(cells, [(0, 0., 100.), (1, 100., 300.)]);
    for doc in docs(&set) {
        assert_eq!(modes(&doc), [vec![1]]);
    }
}

#[test]
fn omissions_are_counted_once_per_published_tile_not_per_budget_retry() {
    use primase_core::convert::NoticeKind;
    let mut w = World::new();
    // a material of a source's own, not lowered to pbr.material, on a triangle placed twice in one cell
    let material = w.spawn();
    w.insert(material, primase_core::Name("unlowered".into()));
    let proto = w.spawn();
    let mut m = mesh();
    *m.primitive_mut(0).unwrap().material_mut() = Some(material);
    w.insert(proto, m);
    place(&mut w, proto, 10.);
    place(&mut w, proto, 150.);
    // every file over the budget: the cell is written to measure it, split, and each subcell written and kept
    let options = Options {
        tile_byte_budget: Some(1),
        ..plain()
    };
    let set = write(&w, &options).unwrap();
    assert_eq!(set.splits.len(), 1);
    assert_eq!(set.index.tiles.len(), 2);
    let counts: Vec<(NoticeKind, u64)> = set
        .notices
        .iter()
        .filter(|n| n.reason.starts_with("materials without pbr.material"))
        .map(|n| (n.kind, n.count))
        .collect();
    assert_eq!(counts, [(NoticeKind::Unsupported, 2)]);
}

/// The viewer has no ambient light: the index carries only the environment's sun and the tiles leave the environment
/// out, so an ambient is reported once per tile set as not written, with a named sun and without one (where nothing of
/// the environment is written at all), and nothing is reported without an ambient.
#[test]
fn the_environments_ambient_is_reported_once_as_not_written() {
    use primase_core::{Ambient, AmbientLight, Environment, Light, LightKind};
    let ambient_notice = |set: &primase_web::TileSet| {
        set.notices
            .iter()
            .filter(|n| {
                n.reason
                    .starts_with("the environment's ambient light, not written")
            })
            .map(|n| n.count)
            .collect::<Vec<_>>()
    };
    let mut w = World::new();
    // placements in two cells, so the tile set has more than one tile
    for x in [0.0, 500.0] {
        let proto = w.spawn();
        w.insert(proto, mesh());
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(
            placement,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, 0., 0.]],
            },
        );
    }
    let sun = w.spawn();
    w.insert(
        sun,
        Light {
            kind: LightKind::Directional,
            color: [1.0; 3],
            intensity: 2.0,
            range: None,
            cast_shadows: Some(true),
        },
    );
    w.insert(sun, Transform::IDENTITY);
    let ambient = Ambient {
        intensity: 1.0,
        light: AmbientLight::Flat([0.2, 0.25, 0.3]),
    };
    // no ambient: nothing to report
    w.insert_resource(Environment {
        sun: Some(sun),
        ambient: None,
    });
    let set = write(&w, &plain()).unwrap();
    assert!(set.index.tiles.len() > 1);
    assert!(ambient_notice(&set).is_empty());
    // an ambient beside a named sun: the sun is indexed, the ambient reported once
    w.insert_resource(Environment {
        sun: Some(sun),
        ambient: Some(ambient.clone()),
    });
    let set = write(&w, &plain()).unwrap();
    assert!(set.index.environment.is_some());
    assert_eq!(ambient_notice(&set), [1]);
    let n = set
        .notices
        .iter()
        .find(|n| n.reason.starts_with("the environment's ambient light"))
        .unwrap();
    assert_eq!(n.kind, primase_core::convert::NoticeKind::Unsupported);
    // an ambient alone: no environment in the index, the ambient reported once, and no tile carries it
    w.insert_resource(Environment {
        sun: None,
        ambient: Some(ambient),
    });
    let set = write(&w, &plain()).unwrap();
    assert!(set.index.environment.is_none());
    assert_eq!(ambient_notice(&set), [1]);
    for bytes in set.files.values() {
        let doc: serde_json::Value = serde_json::from_slice(primase_glb::parse(bytes).unwrap().json).unwrap();
        assert!(!doc.to_string().contains("core.environment"));
    }
}

/// A world of only an environment with an ambient has no cell to write: the plan sees the ambient itself, so it is
/// still reported once, before any cell is written and in the written set.
#[test]
fn an_environment_only_world_still_reports_its_ambient() {
    use primase_core::{Ambient, AmbientLight, Environment};
    let mut w = World::new();
    w.insert_resource(Environment {
        sun: None,
        ambient: Some(Ambient {
            intensity: 1.0,
            light: AmbientLight::Gradient {
                sky: [0.3; 3],
                equator: [0.2; 3],
                ground: [0.1; 3],
            },
        }),
    });
    let reported = |notices: &[primase_core::convert::Notice]| {
        notices
            .iter()
            .filter(|n| {
                n.reason
                    .starts_with("the environment's ambient light, not written")
            })
            .map(|n| (n.kind, n.count))
            .collect::<Vec<_>>()
    };
    let plan = primase_web::plan(&w, &plain()).unwrap();
    assert_eq!(plan.cells().len(), 0);
    assert_eq!(
        reported(&plan.notices()),
        [(primase_core::convert::NoticeKind::Unsupported, 1)]
    );
    let set = write(&w, &plain()).unwrap();
    assert!(set.index.tiles.is_empty() && set.files.is_empty());
    assert!(set.index.environment.is_none());
    assert_eq!(
        reported(&set.notices),
        [(primase_core::convert::NoticeKind::Unsupported, 1)]
    );
}

/// Planned with an ambient, written from parts whose passes consumed it (`fill-ambient-lights` takes it out of the
/// environment, its lights standing for it): the written cells decide, so nothing is reported; written with the
/// ambient kept, it is reported once.
#[test]
fn an_ambient_consumed_after_planning_is_not_reported() {
    use primase_core::{Ambient, AmbientLight, Environment};
    let mut w = World::new();
    for x in [0.0, 500.0] {
        let proto = w.spawn();
        w.insert(proto, mesh());
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(
            placement,
            Transform {
                cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [x, 0., 0.]],
            },
        );
    }
    w.insert_resource(Environment {
        sun: None,
        ambient: Some(Ambient {
            intensity: 1.0,
            light: AmbientLight::Flat([0.2; 3]),
        }),
    });
    let ambient_notices = |plan: &primase_web::TilePlan| {
        plan.notices()
            .iter()
            .filter(|n| {
                n.reason
                    .starts_with("the environment's ambient light, not written")
            })
            .map(|n| (n.kind, n.count))
            .collect::<Vec<_>>()
    };
    // each part as its passes leave it: `consumed`, the ambient taken out (the program's choice of fill lights)
    let write_parts = |consumed: bool| {
        let mut plan = primase_web::plan(&w, &plain()).unwrap();
        for e in plan.unsplit() {
            plan.split(&w, e).unwrap();
        }
        // before any cell is written, the planning world's ambient is what there is to report
        assert_eq!(ambient_notices(&plan).len(), 1);
        let mut part = w.clone();
        if consumed {
            part.insert_resource(Environment {
                sun: None,
                ambient: None,
            });
        }
        assert!(plan.cells().len() > 1);
        for cell in plan.cells() {
            let cw = part.subset(&part.closure(plan.roots(cell).into_iter().chain(plan.shared(cell))));
            plan.write_cell(cell, &cw).unwrap();
        }
        plan
    };
    assert!(ambient_notices(&write_parts(true)).is_empty());
    assert_eq!(
        ambient_notices(&write_parts(false)),
        [(primase_core::convert::NoticeKind::Unsupported, 1)]
    );
}

/// What the tiles have no form for is left out and reported, once each, not refused: a placement of a prototype with
/// levels of detail (not drawn, not counted as a placement), a parametric collider on a placed prototype and one of its
/// own, and a Bezier curve (no viewer record). The plain placement and the polyline are written.
#[test]
fn levels_of_detail_colliders_and_bezier_curves_are_left_out_and_reported() {
    use primase_core::convert::NoticeKind;
    use primase_core::{Collider, Curve, LodForm, Lods, Name, Shape, Surface, Surfaces};
    let mut w = World::new();
    w.insert_resource(Surfaces(vec![Surface::new("default")]));
    let with_forms = w.spawn();
    w.insert(with_forms, mesh());
    let form = w.spawn();
    w.insert(form, mesh());
    w.insert(
        with_forms,
        Lods {
            detail: 0,
            forms: vec![LodForm { detail: 1, form }],
            default: None,
        },
    );
    let lod_placement = place(&mut w, with_forms, 10.);
    w.insert(lod_placement, Name("chosen later".into()));
    let solid = w.spawn();
    w.insert(solid, mesh());
    w.insert(
        solid,
        Collider {
            shape: Shape::Sphere {
                center: [0.; 3],
                radius: 1.,
            },
            surface: 0,
        },
    );
    let drawn = place(&mut w, solid, 20.);
    w.insert(drawn, Name("drawn".into()));
    let own = w.spawn();
    w.insert(
        own,
        Collider {
            shape: Shape::Box {
                center: [0.; 3],
                half_extents: [1.; 3],
            },
            surface: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [30., 0., 0.];
    w.insert(own, t);
    w.insert(own, Name("box".into()));
    let curve = |tangents| Curve {
        kind: "rail".into(),
        points: vec![[40., 0., 0.], [41., 0., 0.]],
        closed: false,
        tangents,
        up: None,
    };
    let bezier = w.spawn();
    w.insert(bezier, curve(Some(vec![[[40., 0., 0.]; 2], [[41., 0., 0.]; 2]])));
    w.insert(bezier, Name("bezier".into()));
    let polyline = w.spawn();
    w.insert(polyline, curve(None));
    let set = write(&w, &plain()).unwrap();
    let tile = &set.index.tiles[0];
    assert_eq!(
        (set.index.tiles.len(), tile.placements, tile.viewer_items),
        (1, 1, 1)
    );
    let unsupported: Vec<(String, u64, Vec<String>)> = set
        .notices
        .iter()
        .filter(|n| n.kind == NoticeKind::Unsupported)
        .map(|n| {
            (
                n.reason.split(' ').next().unwrap().to_owned(),
                n.count,
                n.examples.clone(),
            )
        })
        .collect();
    assert_eq!(
        unsupported,
        [
            ("Bezier".to_owned(), 1, vec!["bezier".to_owned()]),
            (
                "parametric".to_owned(),
                2,
                vec!["drawn".to_owned(), "box".to_owned()]
            ),
            ("placements".to_owned(), 1, vec!["chosen later".to_owned()]),
        ]
    );
}
