use primase_core::{Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_web::{Options, write};
use std::collections::BTreeMap;
fn plain() -> Options {
    Options {
        compress: false,
        ..Options::default()
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
        ..plain()
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
    let mut o = plain();
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
    assert!(write(&w, &plain()).is_err());
    w.insert(e, mesh());
    let o = Options {
        tile_size: 0.,
        ..plain()
    };
    assert!(write(&w, &o).is_err());
    let mut t = Transform::IDENTITY;
    t.cols[0] = [0.; 3];
    w.insert(e, t);
    let out = write(&w, &plain()).unwrap();
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
    let out = write(&w, &plain()).unwrap();
    let tile = &out.index.tiles[0];
    assert_eq!(tile.placements, 5);
    assert_eq!(tile.instanced_placements, 3);
    let bytes = &out.files[&tile.file];
    assert_eq!(*bytes, write(&w, &plain()).unwrap().files[&tile.file]);
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

#[test]
fn quantized_streams_decode_exactly_and_validate() {
    let mut w = World::new();
    let p = w.spawn();
    let mut m = mesh();
    m.primitives[0]
        .streams
        .insert(Semantic::Normal, Stream::F32x3(vec![[0., 1., 0.]; 3]));
    m.primitives[0]
        .streams
        .insert(Semantic::Tangent, Stream::F32x4(vec![[1., 0., 0., -1.]; 3]));
    m.primitives[0].streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0., 0.5], [1., 0.], [0., 1.]]),
    );
    m.primitives[0].streams.insert(
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
    let set = write(&w, &Options::default()).unwrap();
    assert_eq!(set.files, write(&w, &Options::default()).unwrap().files);
    let bytes = set.files.values().next().unwrap();
    let chunks = primase_gltf::glb::parse(bytes).unwrap();
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
    assert_eq!(values, vec![[0, 0, 0], [32767, 0, 0], [0, 0, 32767]]);
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
    if let Some(validator) = primase_core::gate::real_data("PRIMASE_GLTF_VALIDATOR") {
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
        ..Default::default()
    };
    let set = write(&World::new(), &options).unwrap();
    assert_eq!(set.index.tiles.len(), 1);
    let tile = &set.index.tiles[0];
    assert_eq!(tile.content, primase_web::TileContent::MetadataOnly);
    assert_eq!(
        (tile.col, tile.row, tile.viewer_items, tile.triangles),
        (-1, -2, 1, 0)
    );
    let chunks = primase_gltf::glb::parse(&set.files[&tile.file]).unwrap();
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
    let set = write(&w, &Options::default()).unwrap();
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
    assert!(write(&w, &Options::default()).is_err());
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
            ..Default::default()
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
    let collision_chunks = primase_gltf::glb::parse(bytes).unwrap();
    let collision_doc: serde_json::Value = serde_json::from_slice(collision_chunks.json).unwrap();
    assert_eq!(collision_doc["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(collision_doc["accessors"][2]["count"], 1);
    assert_eq!(
        &collision_chunks.bin.unwrap()[36..48],
        &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(collision_chunks.bin.unwrap()[48], 0);
    let chunks = primase_gltf::glb::parse(&set.files[&tile.file]).unwrap();
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
    let set = write(&w, &Options::default()).unwrap();
    let chunks = primase_gltf::glb::parse(set.files.values().next().unwrap()).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(chunks.json).unwrap();
    assert!(doc["nodes"][0].get("matrix").is_none_or(|m| m.is_array()));
    assert!(doc["nodes"][0]["scale"].is_array() || doc["nodes"][0]["matrix"].is_array());
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
    m.primitives[0].material = Some(mat);
    m.primitives[0].streams.insert(
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
    let set = write(&w, &Options::default()).unwrap();
    assert_eq!(set.index.tiles[0].instanced_placements, 3);
    let chunks = primase_gltf::glb::parse(&set.files[&set.index.tiles[0].file]).unwrap();
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
