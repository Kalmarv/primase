//! Writer gates: write the synthetic world, read the GLB back with our strict reader, compare everything with the
//! world (geometry bit for bit, transforms bit for bit, baked shear against independent properties, materials,
//! textures, extras, report counts); determinism; the Khronos validator when `PRIMASE_GLTF_VALIDATOR` is set.

// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use common::{Built, build, scratch};
use primase_core::{Entity, Mesh, MeshFile, Semantic, Stream, Topology, World};
use primase_gltf::{Gltf, GroupBy, WriteOptions, WriteReport, load_mesh_files, write};
use serde_json::{Value, json};

fn options(b: &Built) -> WriteOptions {
    WriteOptions {
        base_dir: b.dir.clone(),
        scene_name: Some("test".into()),
        ..WriteOptions::default()
    }
}

fn written(b: &Built) -> (Gltf, WriteReport, Vec<u8>) {
    let (bytes, report) = write(&b.world, &options(b)).unwrap();
    (Gltf::from_glb(&bytes).unwrap(), report, bytes)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map_or(&[], Vec::as_slice)
}

/// The node whose extras carry this `core.source`.
fn node_by_source<'a>(doc: &'a Value, source: &str) -> Option<&'a Value> {
    arr(doc, "nodes")
        .iter()
        .find(|n| n["extras"]["primase"]["core.source"]["value"] == source)
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// Streams equal bit for bit (`PartialEq` on f32 would let -0.0 equal 0.0).
fn same_streams(a: &BTreeMap<Semantic, Stream>, b: &BTreeMap<Semantic, Stream>) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

fn matrix_of(b: &Built, e: Entity) -> Vec<f32> {
    let c = b.world.get::<primase_core::Transform>(e).unwrap().cols;
    vec![
        c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1], c[2][2], 0.0,
        c[3][0], c[3][1], c[3][2], 1.0,
    ]
}

#[test]
fn round_trip_geometry_and_transforms() {
    let b = build(&scratch("rt-geom"));
    let (gltf, report, _) = written(&b);
    let doc = gltf.json();
    let rich = b.world.get::<Mesh>(b.proto_rich).unwrap();

    // Every mesh the writer made reads back through the strict reader.
    for i in 0..gltf.mesh_count() {
        gltf.read_mesh(i).unwrap();
    }

    // Regular and mirrored: the transform exactly, the prototype's mesh shared, geometry bit-exact.
    let regular = node_by_source(doc, "plac:0").unwrap();
    let mirrored = node_by_source(doc, "plac:1").unwrap();
    assert_eq!(bits(&f32s(&regular["matrix"])), bits(&matrix_of(&b, b.regular)));
    assert_eq!(bits(&f32s(&mirrored["matrix"])), bits(&matrix_of(&b, b.mirrored)));
    assert_eq!(regular["mesh"], mirrored["mesh"]);
    assert_eq!(regular["name"], "rich placed");
    assert_eq!(mirrored["name"], "rich");
    let m = gltf
        .read_mesh(regular["mesh"].as_u64().unwrap() as usize)
        .unwrap();
    assert_eq!(m.name.as_deref(), Some("rich"));
    assert_eq!(m.primitives.len(), 3);
    for (got, want) in m.primitives.iter().zip(&rich.primitives) {
        assert!(same_streams(&got.primitive.streams, &want.streams));
        assert_eq!(got.primitive.indices, want.indices);
        assert_eq!(got.primitive.topology, Topology::Triangles);
    }
    assert_eq!(m.primitives[0].material_name.as_deref(), Some("stone"));
    assert_eq!(m.primitives[1].material_name.as_deref(), Some("fence"));
    assert_eq!(m.primitives[2].material, None);

    // Variant 1: its own mesh with the overriding material, sharing the accessors.
    let v1 = node_by_source(doc, "plac:5").unwrap();
    assert_ne!(v1["mesh"], regular["mesh"]);
    assert!(
        v1.get("matrix").is_none() && v1.get("translation").is_none(),
        "identity writes no transform"
    );
    let mv = &arr(doc, "meshes")[v1["mesh"].as_u64().unwrap() as usize];
    let mr = &arr(doc, "meshes")[regular["mesh"].as_u64().unwrap() as usize];
    assert_eq!(mv["name"], "rich.v1");
    for k in 0..3 {
        assert_eq!(
            mv["primitives"][k]["attributes"],
            mr["primitives"][k]["attributes"]
        );
        assert_eq!(mv["primitives"][k]["indices"], mr["primitives"][k]["indices"]);
    }
    let mat_v = mv["primitives"][1]["material"].as_u64().unwrap() as usize;
    assert_eq!(
        arr(doc, "materials")[mat_v]["name"],
        format!("material_e{}", b.mat_variant.index())
    );
    assert_eq!(arr(doc, "materials")[mat_v]["alphaMode"], "BLEND");

    // Sheared: baked, translation on the node.
    for (src, entity, mirrored) in [("plac:2", b.sheared, false), ("plac:3", b.sheared_mirrored, true)] {
        let node = node_by_source(doc, src).unwrap();
        assert!(node.get("matrix").is_none());
        let tr = b.world.get::<primase_core::Transform>(entity).unwrap().cols;
        match node.get("translation") {
            Some(t) => assert_eq!(bits(&f32s(t)), bits(&tr[3])),
            None => assert_eq!(tr[3], [0.0; 3]),
        }
        let baked = gltf.read_mesh(node["mesh"].as_u64().unwrap() as usize).unwrap();
        assert!(baked.name.as_deref().unwrap().starts_with("rich.baked.e"));
        check_baked(&baked.to_mesh(), rich, tr, mirrored);
    }
    assert!(
        node_by_source(doc, "plac:4").is_none(),
        "degenerate placement skipped"
    );
    assert!(
        node_by_source(doc, "plac:8").is_none(),
        "placement of a prototype without geometry"
    );

    // Big: unsigned int indices; a sequential primitive keeps no indices.
    let big = node_by_source(doc, "plac:6").unwrap();
    assert_eq!(bits(&f32s(&big["translation"])), bits(&[-0.0, 1.0, 2.0]));
    let big_mesh = gltf.read_mesh(big["mesh"].as_u64().unwrap() as usize).unwrap();
    let want = b.world.get::<Mesh>(b.proto_big).unwrap();
    assert_eq!(big_mesh.to_mesh().primitives.len(), 2);
    for (got, want) in big_mesh.primitives.iter().zip(&want.primitives) {
        let pos = |p: &primase_core::Primitive| format!("{:?}", p.streams.get(&Semantic::Position));
        assert_eq!(pos(&got.primitive), pos(want));
        assert_eq!(got.primitive.indices, want.indices);
    }
    assert!(same_streams(
        &big_mesh.primitives[0].primitive.streams,
        &want.primitives[0].streams
    ));
    // The second primitive's material samples sets 0 and 1, which it lacks: zeros, from one shared accessor.
    let filled = &big_mesh.primitives[1].primitive.streams;
    for set in [0, 1] {
        assert_eq!(
            filled.get(&Semantic::TexCoord(set)),
            Some(&Stream::F32x2(vec![[0.0; 2]; 9]))
        );
    }
    let big_json = &arr(doc, "meshes")[big["mesh"].as_u64().unwrap() as usize];
    let ia = big_json["primitives"][0]["indices"].as_u64().unwrap() as usize;
    assert_eq!(arr(doc, "accessors")[ia]["componentType"], 5125);
    let ia = mr["primitives"][0]["indices"].as_u64().unwrap() as usize;
    assert_eq!(arr(doc, "accessors")[ia]["componentType"], 5123);

    // Lines with a unorm16 colour; the skin attribute is skipped and counted.
    let rail = node_by_source(doc, "plac:7").unwrap();
    let rail_mesh = gltf.read_mesh(rail["mesh"].as_u64().unwrap() as usize).unwrap();
    let p = &rail_mesh.primitives[0].primitive;
    assert_eq!(p.topology, Topology::Lines);
    assert_eq!(
        p.streams.get(&Semantic::Color(0)),
        Some(&Stream::U16x4(vec![[65535, 0, 0, 65535]; 3]))
    );
    assert!(!p.streams.contains_key(&Semantic::Joints(0)));

    // Self-placed soup.
    let soup = arr(doc, "nodes")
        .iter()
        .find(|n| n["name"] == "terrain" && n.get("mesh").is_some())
        .unwrap();
    assert_eq!(f32s(&soup["translation"]), [100.0, 0.0, -50.0]);

    assert_eq!(report.placements, 8, "{report}");
    assert_eq!(report.placements_mirrored, 1);
    assert_eq!(report.placements_sheared_baked, 2);
    assert_eq!(report.placements_degenerate_skipped, 1);
    assert_eq!(report.placements_without_mesh, 1);
    assert_eq!(report.prototypes_unplaced, 1);
    assert_eq!(report.meshes, 7, "rich, rich.v1, 2 baked, big, rail, terrain");
    assert_eq!(
        report.streams_skipped,
        BTreeMap::from([("JOINTS_0".to_owned(), 1)])
    );
    assert_eq!(
        report.primitives_without_material, 5,
        "rich's third primitive in 4 meshes, the rail"
    );
    assert_eq!(report.primitives_texcoords_zero_filled, 1);
    assert_eq!(report.triangles, 4 * 3 * 2 + 70_002 / 3 + 3 + 2);
    println!("{report}");
}

/// The baked mesh of a sheared placement against properties that hold for any correct bake: positions are `L p`
/// (independent f64 evaluation), each normal is perpendicular to the transformed tangent plane and on the side of
/// `L n`, tangents are `L t` normalized with w flipped under a mirror, and triangles still face their normals
/// (winding reversed under a mirror). Streams the bake does not touch are bit-identical.
fn check_baked(baked: &Mesh, proto: &Mesh, tr: [[f32; 3]; 4], mirrored: bool) {
    let l = |v: [f32; 3]| -> [f64; 3] {
        std::array::from_fn(|r| (0..3).map(|c| f64::from(tr[c][r]) * f64::from(v[c])).sum())
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let f64v = |v: [f32; 3]| v.map(f64::from);
    for (bp, pp) in baked.primitives.iter().zip(&proto.primitives) {
        let (Some(Stream::F32x3(bpos)), Some(Stream::F32x3(ppos))) = (
            bp.streams.get(&Semantic::Position),
            pp.streams.get(&Semantic::Position),
        ) else {
            panic!("positions")
        };
        for (got, src) in bpos.iter().zip(ppos) {
            let want = l(*src);
            for k in 0..3 {
                assert!((f64::from(got[k]) - want[k]).abs() <= 1e-6 * want[k].abs().max(1.0));
            }
        }
        let (Some(Stream::F32x3(bn)), Some(Stream::F32x3(pn))) = (
            bp.streams.get(&Semantic::Normal),
            pp.streams.get(&Semantic::Normal),
        ) else {
            panic!("normals")
        };
        for (got, src) in bn.iter().zip(pn) {
            let g = f64v(*got);
            assert!((dot(g, g) - 1.0).abs() < 1e-6, "unit normal");
            let n = f64v(*src);
            for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
                let v = cross(n, axis);
                let lv = l(v.map(|x| x as f32));
                assert!(
                    dot(lv, g).abs() < 1e-6,
                    "normal perpendicular to the transformed tangent plane"
                );
            }
            assert!(dot(l(*src), g) > 0.0, "normal on the side of L n");
        }
        let (Some(Stream::F32x4(bt)), Some(Stream::F32x4(pt))) = (
            bp.streams.get(&Semantic::Tangent),
            pp.streams.get(&Semantic::Tangent),
        ) else {
            panic!("tangents")
        };
        for (got, src) in bt.iter().zip(pt) {
            let want = l([src[0], src[1], src[2]]);
            let len = dot(want, want).sqrt();
            for k in 0..3 {
                assert!((f64::from(got[k]) - want[k] / len).abs() < 1e-6);
            }
            assert_eq!(got[3], if mirrored { -src[3] } else { src[3] });
        }
        for s in [
            Semantic::TexCoord(0),
            Semantic::TexCoord(1),
            Semantic::Color(0),
            Semantic::Color(1),
        ] {
            assert_eq!(
                format!("{:?}", bp.streams.get(&s)),
                format!("{:?}", pp.streams.get(&s))
            );
        }
        let idx = bp.indices.as_ref().unwrap();
        if !mirrored {
            assert_eq!(bp.indices, pp.indices);
        }
        for t in idx.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| f64v(bpos[t[k] as usize]));
            let face = cross(
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            assert!(
                dot(face, f64v(bn[t[0] as usize])) > 0.0,
                "triangle faces its normal"
            );
        }
    }
}

#[test]
fn round_trip_materials_textures_and_extras() {
    let b = build(&scratch("rt-mat"));
    let (gltf, report, _) = written(&b);
    let doc = gltf.json();
    let mats = arr(doc, "materials");
    let by_name = |n: &str| mats.iter().find(|m| m["name"] == n).unwrap();
    let image_bytes = |tex: &Value| -> Vec<u8> {
        let t = &arr(doc, "textures")[tex["index"].as_u64().unwrap() as usize];
        let img = &arr(doc, "images")[t["source"].as_u64().unwrap() as usize];
        assert_eq!(img["mimeType"], "image/png");
        gltf.buffer_view(img["bufferView"].as_u64().unwrap() as usize)
            .unwrap()
            .to_vec()
    };

    let stone = by_name("stone");
    let pmr = &stone["pbrMetallicRoughness"];
    assert_eq!(f32s(&pmr["baseColorFactor"]), [0.9, 0.8, 0.7, 1.0]);
    assert_eq!(
        f32s(&json!([pmr["metallicFactor"], pmr["roughnessFactor"]])),
        [0.0, 0.6]
    );
    assert_eq!(
        image_bytes(&pmr["baseColorTexture"]),
        std::fs::read(b.dir.join("textures/base.png")).unwrap()
    );
    let normal = &stone["normalTexture"];
    assert_eq!(
        (normal["texCoord"].as_u64(), normal["scale"].as_f64()),
        (Some(1), Some(0.5))
    );
    // the slot's transform as KHR_texture_transform (rotation 0 left out), declared used
    assert_eq!(
        normal["extensions"]["KHR_texture_transform"],
        json!({"offset": [0.25, -0.5], "scale": [2.0, 4.0]})
    );
    assert_eq!(doc["extensionsUsed"], json!(["KHR_texture_transform"]));
    assert!(pmr["baseColorTexture"].get("extensions").is_none());
    assert_eq!(
        image_bytes(normal),
        std::fs::read(b.dir.join("textures/normal.png")).unwrap()
    );
    let nt = &arr(doc, "textures")[normal["index"].as_u64().unwrap() as usize];
    let sampler = &arr(doc, "samplers")[nt["sampler"].as_u64().unwrap() as usize];
    assert_eq!(
        (sampler["wrapS"].as_u64(), sampler["wrapT"].as_u64()),
        (Some(33071), Some(33648))
    );
    assert_eq!(
        nt["extras"]["primase"]["core.source"],
        json!({"version": 1, "value": "tex:normal"})
    );
    assert_eq!(f32s(&stone["emissiveFactor"]), [0.1, 0.0, 0.0]);
    assert_eq!(stone["doubleSided"], true);
    assert_eq!(
        stone["extras"],
        json!({"primase": {"skate.material": {"version": 1,
            "value": {"class": "opaque", "base": "stone_d", "tint": [200, 40, 40]}}}})
    );

    let fence = by_name("fence");
    assert_eq!(fence["alphaMode"], "MASK");
    assert_eq!(fence["alphaCutoff"].as_f64().map(|x| x as f32), Some(0.33));
    assert!(
        fence["pbrMetallicRoughness"]
            .get("metallicRoughnessTexture")
            .is_none(),
        "DDS slot left empty"
    );
    assert_eq!(
        image_bytes(&fence["pbrMetallicRoughness"]["baseColorTexture"]),
        image_bytes(&pmr["baseColorTexture"]),
    );

    let plain = by_name("unlowered");
    assert_eq!(
        f32s(&plain["pbrMetallicRoughness"]["baseColorFactor"]),
        [0.8, 0.8, 0.8, 1.0]
    );
    assert!(plain.get("extras").is_none());

    assert_eq!(
        report.images, 2,
        "base.png and its byte-identical copy embedded once"
    );
    assert_eq!(report.textures, 3);
    assert_eq!(report.materials, 4);
    assert_eq!(report.materials_fallback, 1);
    assert_eq!(report.textures_skipped, BTreeMap::from([("dds".to_owned(), 1)]));
    assert_eq!(report.texture_slots_dropped, 1);
    assert_eq!(
        report.extras_unencoded,
        BTreeMap::from([("fromsoft.layered_material".to_owned(), 1)]),
        "opaque bytes have no JSON form"
    );

    // Placement, prototype and scene extras.
    let regular = node_by_source(doc, "plac:0").unwrap();
    assert_eq!(
        regular["extras"],
        json!({"primase": {"core.source": {"version": 1, "value": "plac:0"},
            "core.tile": {"version": 1, "value": {"col": 3, "row": -1}}}})
    );
    let mesh = &arr(doc, "meshes")[regular["mesh"].as_u64().unwrap() as usize];
    assert_eq!(
        mesh["extras"],
        json!({"primase": {"core.source": {"version": 1, "value": "msb:c1100/rich"}}})
    );
    assert_eq!(
        doc["scenes"][0]["extras"],
        json!({"primase": {"skate.scene": {"version": 1, "value": {"name": "test_scene", "region": [-10, -10, 10, 10]}}}})
    );
    assert_eq!(doc["scenes"][0]["name"], "test");
}

#[test]
fn groups_are_empty_nodes_per_prototype_or_tile() {
    let b = build(&scratch("groups"));
    let (gltf, report, _) = written(&b);
    let doc = gltf.json();
    let roots: Vec<&Value> = doc["scenes"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &arr(doc, "nodes")[i.as_u64().unwrap() as usize])
        .collect();
    let names: Vec<&str> = roots.iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["rich", "big", "rail", "terrain"]);
    for r in &roots {
        assert!(r.get("mesh").is_none());
        assert_eq!(r["extras"]["primase.collection"], r["name"]);
    }
    let rich_children: Vec<&Value> = roots[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(
            |i| &arr(doc, "nodes")[i.as_u64().unwrap() as usize]["extras"]["primase"]["core.source"]["value"],
        )
        .collect();
    assert_eq!(rich_children, ["plac:0", "plac:1", "plac:2", "plac:3", "plac:5"]);
    assert_eq!(report.group_nodes, 4);

    let mut o = options(&b);
    o.group_by = GroupBy::Tile;
    let (bytes, _) = write(&b.world, &o).unwrap();
    let doc = Gltf::from_glb(&bytes).unwrap().json().clone();
    let roots: Vec<&str> = doc["scenes"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            arr(&doc, "nodes")[i.as_u64().unwrap() as usize]["name"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(roots, ["tile_3_-1", "untiled"]);

    o.group_by = GroupBy::None;
    o.include_unplaced = true;
    let (bytes, report) = write(&b.world, &o).unwrap();
    let doc = Gltf::from_glb(&bytes).unwrap().json().clone();
    assert_eq!(
        doc["scenes"][0]["nodes"].as_array().unwrap().len(),
        9,
        "8 placements and the unplaced prototype"
    );
    assert_eq!((report.group_nodes, report.prototypes_unplaced), (0, 1));
}

/// Same world, same bytes: twice from one world, and from a world built again in another folder (texture paths
/// do not reach the output).
#[test]
fn output_is_deterministic() {
    let a = build(&scratch("det-a"));
    let (one, _) = write(&a.world, &options(&a)).unwrap();
    let (two, _) = write(&a.world, &options(&a)).unwrap();
    assert_eq!(one, two);
    let b = build(&scratch("det-b"));
    let (three, _) = write(&b.world, &options(&b)).unwrap();
    assert_eq!(one, three);
    let cloned = a.world.clone();
    let (four, _) = write(&cloned, &options(&a)).unwrap();
    assert_eq!(one, four);
}

#[test]
fn bad_worlds_are_errors() {
    let b = build(&scratch("bad"));
    let mut w = b.world.clone();
    w.get_mut::<Mesh>(b.proto_rich).unwrap().primitives[0].indices = Some(vec![0, 1, 9]);
    let err = write(&w, &options(&b)).unwrap_err().to_string();
    assert!(err.contains("index 9 of 4 vertices"), "{err}");

    let mut w = b.world.clone();
    w.get_mut::<Mesh>(b.proto_rich).unwrap().primitives[0]
        .streams
        .insert(Semantic::Normal, Stream::F32x3(vec![[0.0; 3]; 3]));
    assert!(
        write(&w, &options(&b))
            .unwrap_err()
            .to_string()
            .contains("Normal has 3 values")
    );

    let mut w = b.world.clone();
    w.get_mut::<Mesh>(b.proto_rich).unwrap().primitives[0]
        .streams
        .insert(Semantic::Position, Stream::F32x3(vec![[f32::NAN; 3]; 4]));
    assert!(
        write(&w, &options(&b))
            .unwrap_err()
            .to_string()
            .contains("non-finite")
    );

    let mut w = b.world.clone();
    w.remove::<primase_core::Transform>(b.regular);
    assert!(
        write(&w, &options(&b))
            .unwrap_err()
            .to_string()
            .contains("without core.transform")
    );

    std::fs::write(b.dir.join("textures/normal.png"), b"not a png").unwrap();
    assert!(
        write(&b.world, &options(&b))
            .unwrap_err()
            .to_string()
            .contains("not a PNG")
    );
}

/// The load pass: prototypes pointing into a GLB (two meshes of one file) get their meshes back bit-exact, with
/// the materials from `core.mesh_file`.
#[test]
fn load_mesh_files_restores_meshes() {
    let b = build(&scratch("load"));
    let (gltf, _, bytes) = written(&b);
    std::fs::write(b.dir.join("all.glb"), &bytes).unwrap();
    let mesh_index = |name: &str| {
        arr(gltf.json(), "meshes")
            .iter()
            .position(|m| m["name"] == name)
            .unwrap() as u32
    };
    let mut w = World::new();
    let m1 = w.spawn();
    let p1 = w.spawn();
    let p2 = w.spawn();
    w.insert(
        p1,
        MeshFile {
            path: "all.glb".into(),
            mesh: mesh_index("rich"),
            materials: vec![Some(m1), None, None],
        },
    );
    w.insert(
        p2,
        MeshFile {
            path: "all.glb".into(),
            mesh: mesh_index("big"),
            materials: vec![None, Some(m1)],
        },
    );
    let report = load_mesh_files(&mut w, &b.dir).unwrap();
    assert_eq!((report.meshes, report.files, report.primitives), (2, 1, 5));
    let got = w.get::<Mesh>(p1).unwrap();
    let want = b.world.get::<Mesh>(b.proto_rich).unwrap();
    for (g, x) in got.primitives.iter().zip(&want.primitives) {
        assert!(same_streams(&g.streams, &x.streams));
        assert_eq!(g.indices, x.indices);
    }
    assert_eq!(
        got.primitives.iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(m1), None, None]
    );
    assert_eq!(w.get::<Mesh>(p2).unwrap().primitives[1].material, Some(m1));

    let mut w2 = World::new();
    let p = w2.spawn();
    w2.insert(
        p,
        MeshFile {
            path: "all.glb".into(),
            mesh: mesh_index("rich"),
            materials: vec![None],
        },
    );
    let err = load_mesh_files(&mut w2, &b.dir).unwrap_err().to_string();
    assert!(
        err.contains("has 3 primitives, core.mesh_file lists 1 materials"),
        "{err}"
    );
    w2.insert(
        p,
        MeshFile {
            path: "all.glb".into(),
            mesh: 99,
            materials: vec![],
        },
    );
    let err = load_mesh_files(&mut w2, &b.dir).unwrap_err().to_string();
    assert!(err.contains("all.glb") && err.contains("meshes[99]"), "{err}");
}

/// Oracle: the Khronos glTF-Validator (2.0.0-dev.3.10) on the writer's output: no errors, and no warnings except
/// `MESH_PRIMITIVE_GENERATED_TANGENT_SPACE` (a normal-mapped primitive without tangents, which the fixture has on
/// purpose: readers generate them, and game meshes often have none).
#[test]
fn khronos_validator_accepts_output() {
    let Some(validator) = primase_core::gate::real_data("PRIMASE_GLTF_VALIDATOR") else {
        return;
    };
    let dir = scratch("validator");
    let b = build(&dir);
    let (bytes, _) = write(&b.world, &options(&b)).unwrap();
    let path = dir.join("world.glb");
    std::fs::write(&path, bytes).unwrap();
    let out = std::process::Command::new(validator)
        .arg("-o")
        .arg(&path)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let issues = &report["issues"];
    println!("{}", serde_json::to_string_pretty(issues).unwrap());
    assert_eq!(issues["numErrors"], 0);
    let warnings: Vec<&str> = issues["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["severity"] == 1)
        .map(|m| m["code"].as_str().unwrap())
        .collect();
    assert!(
        warnings
            .iter()
            .all(|c| *c == "MESH_PRIMITIVE_GENERATED_TANGENT_SPACE"),
        "{warnings:?}"
    );
}

#[test]
fn memory_textures_embed_exact_pixels_and_require_normal_conversion() {
    use primase_core::{
        ColorSpace, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole, Texture, TextureFile,
    };
    let mut b = build(&scratch("memory-textures"));
    let pixels = vec![128, 64, 0, 255];
    let t = Texture {
        width: 1,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![pixels.clone()],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    };
    let entities: Vec<_> = b.world.iter::<TextureFile>().map(|(e, _)| e).collect();
    for e in entities {
        b.world.insert(e, t.clone());
    }
    let (g, r, bytes) = written(&b);
    assert!(r.textures_skipped.is_empty());
    assert_eq!(r.images, 1);
    let view = g.json()["images"][0]["bufferView"].as_u64().unwrap() as usize;
    assert_eq!(
        primase_image::read_png(g.buffer_view(view).unwrap())
            .unwrap()
            .image
            .samples,
        primase_image::Samples::U8(pixels)
    );
    assert_eq!(bytes, written(&b).2);
    let n = b.world.get_mut::<Texture>(b.tex_normal).unwrap();
    n.role = TexRole::Normal;
    n.normal = Some(NormalConvention {
        green: NormalGreen::Down,
        z: NormalZ::Rebuild,
        x: NormalX::Red,
    });
    assert!(
        write(&b.world, &options(&b))
            .unwrap_err()
            .to_string()
            .contains("normalize-normals")
    );
    primase_image::normalize_normals(&mut b.world).unwrap();
    assert!(written(&b).1.textures_skipped.is_empty());
}

/// A component whose JSON view would lose a value (a NaN written as null) is an error naming it, never a quiet null.
#[test]
fn extras_never_lose_a_value() {
    let mut b = build(&scratch("nan_extras"));
    b.world.insert(
        b.regular,
        primase_core::Bounds {
            min: [0.0; 3],
            max: [1.0, f32::NAN, 1.0],
        },
    );
    let err = write(&b.world, &options(&b)).unwrap_err().to_string();
    assert!(err.contains("core.bounds") && err.contains("NaN"), "{err}");
}
