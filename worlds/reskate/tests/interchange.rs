//! Synthetic scene folders (made here, no game data): round trips, chunked tables, edits, determinism, and every
//! kind of invalid input failing with its reason.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use primase_core::gate::compare_json;
use primase_core::{
    ColorSpace, Instance, NormalConvention, NormalGreen, NormalX, NormalZ, Source, TexFormat, TexRole,
    Texture, Transform, World,
};
use primase_reskate::components::{self, SkateLight, SkatePrototype, SkateScene};
use primase_reskate::interchange;
use primase_reskate::interchange::tables::{
    self, CurveRec, CurveTable, LightRec, PlacementRec, SoupTable, flags,
};
use serde_json::{Value, json};

/// A fresh folder under the target directory's tmp.
fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("skate-scene")
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

static NORMAL_PNG: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A w x h RGBA PNG of one colour (zlib stored blocks, no compression).
fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut raw = Vec::new();
    for _ in 0..h {
        raw.push(0);
        raw.extend(rgba.iter().cycle().take(4 * w as usize));
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01, 1];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let chunk = |out: &mut Vec<u8>, t: &[u8], d: &[u8]| {
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        let mut td = t.to_vec();
        td.extend_from_slice(d);
        out.extend_from_slice(&td);
        out.extend_from_slice(&crc32(&td).to_be_bytes());
    };
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A GLB whose meshes have these many primitives, each one triangle (shared accessors), no materials.
fn glb(meshes: &[usize]) -> Vec<u8> {
    let mut bin = Vec::new();
    for v in [[0f32, 0., 0.], [1., 0., 0.], [0., 0., -1.]] {
        for x in v {
            bin.extend_from_slice(&x.to_le_bytes());
        }
    }
    for _ in 0..3 {
        for x in [0f32, 1., 0.] {
            bin.extend_from_slice(&x.to_le_bytes());
        }
    }
    let prim = r#"{"attributes": {"POSITION": 0, "NORMAL": 1}, "mode": 4}"#;
    let ms: Vec<String> = meshes
        .iter()
        .map(|&n| format!(r#"{{"primitives": [{}]}}"#, vec![prim; n].join(",")))
        .collect();
    let mut js = format!(
        r#"{{"asset": {{"version": "2.0"}}, "buffers": [{{"byteLength": 72}}], "bufferViews": [{{"buffer": 0, "byteOffset": 0, "byteLength": 36}}, {{"buffer": 0, "byteOffset": 36, "byteLength": 36}}], "accessors": [{{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, -1], "max": [1, 0, 0]}}, {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}}], "meshes": [{}]}}"#,
        ms.join(",")
    )
    .into_bytes();
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + js.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&js);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

fn plac(proto: u32, variant: u32, x: f32, fl: u32, source: u32) -> PlacementRec {
    PlacementRec {
        proto,
        variant,
        m: [1., 0., 0., 0., 1., 0., 0., 0., 1., x, -0.0, -40.],
        tile: if fl & flags::TILE != 0 { [-1, 3] } else { [0, 0] },
        radius: if fl & flags::RADIUS != 0 { 2.5 } else { 0.0 },
        flags: fl,
        source,
    }
}

fn soup() -> SoupTable {
    SoupTable {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 0., -1.], [5., 1., 5.]],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        surfaces: vec![1, 0x80 | 2],
        reserved: 0,
    }
}

fn light(group: u32, source: u32) -> LightRec {
    LightRec {
        kind: 1,
        position: [1., 2., 3.],
        direction: [0., -1., 0.],
        tangent: [0.; 3],
        color: [1., 0.9, 0.8],
        energy: 300.,
        range: 15.,
        radius: 0.1,
        spot_size: 1.2,
        spot_blend: 0.3,
        time_of_day: 24,
        flags: flags::CAST_SHADOW,
        priority: 0.,
        group,
        source,
    }
}

const MANIFEST: &str = r#"{
  "format": "skate-scene", "version": 1, "generator": "fixture 1",
  "name": "fixture_a", "display_name": "Fixture A",
  "region": [-100, -100.5, 100, 0.30000001192092896],
  "surfaces": ["default", "concrete", "asphalt", "packed:524384"],
  "light_prefix": "fx_",
  "textures": {
    "wall_d": {"file": "textures/wall_d.png", "name": "wall_d", "source_format": "D3DFMT_DXT1", "source_size": [512, 256], "extras": {"k": 1.0}},
    "wall_n": {"file": "textures/wall_n.png", "green": "down", "z": "rebuild", "source": "tex:wall_n"}
  },
  "materials": {
    "wall": {"class": "opaque", "base": "wall_d", "normal": "wall_n", "cutoff": 0.5, "roughness": 0.6, "label": "Wall", "vertex_alpha": true},
    "trim_red": {"class": "cutout", "base_color": [200, 40, 40, 255], "alpha": "mask", "two_sided": true, "vertex_alpha": false},
    "glass": {"class": "glass", "metallic": 0.1, "wrap": "extend", "source": "mat:glass", "unknown_key": [1, 2], "cast_shadows": false}
  },
  "prototypes": [
    {"id": "wall_01", "name": "Wall", "key": 305419896, "glb": "prototypes/wall.glb", "source": "msb:wall_01",
     "parts": [{"material": "wall", "source": {"shader": "a.sps", "bucket": 2, "model": 0, "geometry": 4}},
               {"material": null, "variants": {"0": "trim_red", "3": "glass"}}],
     "collision": {"file": "collision/wall.bin", "mode": "hull"}},
    {"id": "shared", "glb": "prototypes/wall.glb", "mesh": 1, "foliage": true, "parts": [{"material": "glass"}]},
    {"id": "empty", "glb": null, "parts": []}
  ],
  "placements": "tables/placements.bin",
  "placement_sources": ["msb:a", "msb:b"],
  "collision": [{"file": "collision/world.bin", "ground": true, "region_cut": false}, "collision/other.bin"],
  "lights": "tables/lights.bin",
  "light_groups": ["", "interior"],
  "light_sources": ["l0"],
  "grind_curves": "tables/grind.bin",
  "water": [{"corners": [[0, 0], [10, 0], [10, 10], [0, 10]], "height": 1.5}],
  "markers": [
    {"kind": "spawn", "position": [0, 120, 0], "yaw": 0, "snap": "none"},
    {"kind": "travel", "name": "Main Gate", "position": [12, null, -40], "yaw": 90.5, "snap": "street", "shelter": false},
    {"kind": "trainer", "position": [80, 300, 12], "snap": {"mode": "ground", "above": 3, "rings": [4, 8]}},
    {"kind": "vfx", "name": "snow", "blueprint": "vfx/weather/snowfall", "m": [1,0,0, 0,1,0, 0,0,1, 0,140,0]}
  ],
  "streaming_distances": {"near": 50, "medium": 250, "far": 500},
  "min_height": {"height": 100.01, "stub_below": 250, "floor": -91.0},
  "max_render_edge": 48,
  "warnings": ["from the front end"],
  "stats": {"front_end": "fixture", "lift": 3, "f": 0.25},
  "extensions": {"fixture": {"x": [1, {"y": null}]}},
  "extras": {"note": "kept"}
}
"#;

/// The fixture folder: manifest, tables, files.
fn fixture(name: &str) -> PathBuf {
    fixture_with(name, MANIFEST, |_| {})
}

fn fixture_with(name: &str, manifest: &str, edit: impl FnOnce(&Path)) -> PathBuf {
    let d = tmp(name);
    put(&d, "scene.json", manifest.as_bytes());
    put(&d, "textures/wall_d.png", &png(2, 2, [200, 100, 50, 255]));
    put(
        &d,
        "textures/wall_n.png",
        NORMAL_PNG.get_or_init(|| png(1, 1, [128, 128, 255, 255])),
    );
    put(&d, "prototypes/wall.glb", &glb(&[2, 1]));
    put(
        &d,
        "tables/placements.bin",
        &tables::write_placements(&[
            plac(0, 0, 12., flags::TILE | flags::RADIUS, 0),
            plac(0, 3, 13., flags::CLIP_GIVEN | flags::NO_COLLISION, 1),
            plac(
                1,
                0,
                14.,
                flags::CLIP_GIVEN | flags::CLIP | flags::SPLIT,
                u32::MAX,
            ),
            plac(2, 0, 15., 0, u32::MAX),
        ]),
    );
    put(&d, "collision/wall.bin", &tables::write_soup(&soup()));
    put(&d, "collision/world.bin", &tables::write_soup(&soup()));
    put(
        &d,
        "collision/other.bin",
        &tables::write_soup(&SoupTable::default()),
    );
    put(
        &d,
        "tables/lights.bin",
        &tables::write_lights(&[light(0, 0), light(1, u32::MAX), light(1, 0)]),
    );
    put(
        &d,
        "tables/grind.bin",
        &tables::write_curves(&CurveTable {
            curves: vec![
                CurveRec {
                    first: 0,
                    count: 2,
                    flags: flags::CLOSED | (2 << 4),
                    radius: 0.05,
                    surface: 3,
                },
                CurveRec {
                    first: 2,
                    count: 3,
                    flags: 0,
                    radius: 0.0,
                    surface: 0,
                },
            ],
            points: vec![
                [0., 1., 0.],
                [1., 1., 0.],
                [2., 1., 0.],
                [3., 1., 1.],
                [4., 1., 2.],
            ],
            reserved: 0,
        }),
    );
    edit(&d);
    d
}

fn read(dir: &Path) -> (World, interchange::ReadReport) {
    let mut w = World::new();
    let rep = interchange::read(dir, &mut w).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    (w, rep)
}

/// [`read`], then every file the scene references loaded: what a world must hold to be written (the writer reads no
/// file).
fn read_loaded(dir: &Path) -> (World, interchange::ReadReport) {
    let (mut w, rep) = read(dir);
    interchange::load(dir, &mut w).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    (w, rep)
}

fn read_err(dir: &Path) -> String {
    let mut w = World::new();
    match interchange::read(dir, &mut w) {
        Ok(_) => panic!("{} read without error", dir.display()),
        Err(e) => e.to_string(),
    }
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("scene.json")).unwrap()).unwrap()
}

#[test]
fn layered_materials_round_trip_and_follow_edits() {
    let mut input: Value = serde_json::from_str(MANIFEST).unwrap();
    input["materials"]["ground"] = json!({"class":"layered", "layers":[
        {"material":"wall", "uv_transform":{"scale":[2,3]}, "extra":true},
        null, {"material":"trim_red","uv":0}, {"material":"glass"}],
        "weights":{"attribute":"COLOR_1","mapping":"rgb_product_4","extra":7}});
    let dir = fixture_with("layered", &input.to_string(), |_| {});
    let (mut w, _) = read_loaded(&dir);
    let (entity, material) = w.iter::<primase_core::LayeredMaterial>().next().unwrap();
    assert_eq!(material.layers.len(), 4);
    assert!(material.layers[1].is_none());
    let out = tmp("layered_out");
    interchange::write(&w, &out).unwrap();
    assert_eq!(manifest(&out)["materials"], input["materials"]);
    w.get_mut::<primase_core::LayeredMaterial>(entity).unwrap().layers[0]
        .as_mut()
        .unwrap()
        .texcoord = Some(2);
    let out = tmp("layered_edited");
    interchange::write(&w, &out).unwrap();
    assert_eq!(manifest(&out)["materials"]["ground"]["layers"][0]["uv"], 2);
    for (name, replacement) in [
        ("unknown", json!({"material":"absent"})),
        ("recursive", json!({"material":"ground"})),
    ] {
        input["materials"]["ground"]["layers"][0] = replacement;
        let dir = fixture_with(&format!("layered_{name}"), &input.to_string(), |_| {});
        assert!(read_err(&dir).contains("material"));
    }
}

#[test]
fn layered_cell_closure_keeps_textures_and_mask_lowering() {
    use primase_core::pass::{Pass, PassLog};
    use primase_core::{
        Layer, LayerWeights, LayeredMaterial, Mesh, Primitive, Semantic, Stream, Topology, WeightMapping,
    };
    let dir = fixture("layered_cell");
    let (mut world, _) = read(&dir);
    let ordinary = world
        .iter::<components::SkateMaterial>()
        .find(|(_, m)| m.key == "wall")
        .unwrap()
        .0;
    let base = world
        .get::<components::SkateMaterial>(ordinary)
        .unwrap()
        .base
        .unwrap();
    let mask = world.spawn();
    world.insert(
        mask,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![11, 22, 33, 44]],
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    );
    world.get_mut::<components::SkateMaterial>(ordinary).unwrap().mask = Some(mask);
    let layered = world.spawn();
    world.insert(
        layered,
        LayeredMaterial {
            layers: vec![Some(Layer {
                material: ordinary,
                texcoord: None,
                transform: None,
            })],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let prototype = world.spawn();
    world.insert(
        prototype,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            indices: Some(vec![0, 1, 2]),
            material: Some(layered),
            streams: std::collections::BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
                ),
                (Semantic::TexCoord(0), Stream::F32x2(vec![[0.; 2]; 3])),
                (Semantic::Color(1), Stream::F32x4(vec![[1., 0., 0., 0.]; 3])),
            ]),
        }]),
    );
    let closure = world.closure([prototype]);
    for entity in [layered, ordinary, base, mask] {
        assert!(closure.contains(&entity));
    }
    let mut cell = world.subset(&closure);
    primase_gltf::load_mesh_files(&mut cell, &dir).unwrap();
    primase_image::load_textures(&mut cell, &dir).unwrap();
    primase_reskate::passes::lower::LowerSkatePbr
        .run(&mut cell, &mut PassLog::default())
        .unwrap();
    primase_reskate::passes::LowerLayeredPbr
        .run(&mut cell, &mut PassLog::default())
        .unwrap();
    primase_image::normalize_normals(&mut cell).unwrap();
    let material = cell.get::<Mesh>(prototype).unwrap().primitives()[0]
        .material
        .unwrap();
    let pbr = cell.get::<primase_core::pbr::PbrMaterial>(material).unwrap();
    assert_eq!(pbr.base_color_texture.unwrap().texture, base);
    let orm = pbr.metallic_roughness_texture.unwrap().texture;
    assert_eq!(pbr.occlusion_texture.unwrap().texture, orm);
    assert_eq!(cell.get::<Texture>(orm).unwrap().levels[0], [33, 233, 11, 255]);
}

fn same_tree(a: &Path, b: &Path, rels: &[&str]) {
    for r in rels {
        assert_eq!(
            std::fs::read(a.join(r)).unwrap(),
            std::fs::read(b.join(r)).unwrap(),
            "{r}"
        );
    }
}

const ALL_FILES: &[&str] = &[
    "textures/wall_d.png",
    "textures/wall_n.png",
    "prototypes/wall.glb",
    "tables/placements.bin",
    "collision/wall.bin",
    "collision/world.bin",
    "collision/other.bin",
    "tables/lights.bin",
    "tables/grind.bin",
];

#[test]
fn round_trip_is_exact() {
    let src = fixture("rt_src");
    let (w, rep) = read_loaded(&src);
    assert_eq!(rep.tolerated.len(), 0, "{:?}", rep.tolerated);
    assert_eq!(
        rep.warnings,
        ["prototype 'empty' has no geometry; its placements draw nothing"]
    );
    let out = tmp("rt_out");
    std::fs::remove_dir(&out).unwrap();
    interchange::write(&w, &out).unwrap();
    same_tree(&src, &out, ALL_FILES);
    let c = compare_json(&manifest(&src), &manifest(&out), 0);
    assert!(c.differences.is_empty(), "{:?}", c.differences);
    // the region's 0.30000001192092896 comes back as its shortest f32 form, 0.3
    assert_eq!(c.f32_equal, 1);
    // writing again gives the same bytes, also the manifest
    let out2 = tmp("rt_out2");
    interchange::write(&w, &out2).unwrap();
    same_tree(&out, &out2, &[&["scene.json"], ALL_FILES].concat());
    // and reading the output gives a world that writes the same again
    let (w3, _) = read_loaded(&out);
    let out3 = tmp("rt_out3");
    interchange::write(&w3, &out3).unwrap();
    same_tree(&out, &out3, &[&["scene.json"], ALL_FILES].concat());
}

#[test]
fn the_world_holds_what_the_manifest_says() {
    let (w, _) = read(&fixture("content"));
    let s = w.resource::<SkateScene>().unwrap();
    assert_eq!(s.name, "fixture_a");
    assert_eq!(s.min_height.as_ref().unwrap().floor, Some(-91.0));
    assert_eq!(s.key_order.first().map(String::as_str), Some("format"));
    assert!(s.other.contains_key("extensions") && s.other.contains_key("extras"));
    let protos: Vec<_> = w.iter::<SkatePrototype>().collect();
    assert_eq!(protos.len(), 3);
    let placements: Vec<_> = w.iter::<Instance>().collect();
    assert_eq!(placements.len(), 4);
    assert_eq!(placements[1].1.variant, 3);
    assert_eq!(placements[1].1.prototype, protos[0].0);
    assert_eq!(w.get::<Source>(placements[1].0).unwrap().0, "msb:b");
    let t = w.get::<Transform>(placements[0].0).unwrap();
    assert_eq!(t.cols[3][1].to_bits(), (-0.0f32).to_bits(), "signed zero kept");
    let mf = w.get::<primase_core::MeshFile>(protos[0].0).unwrap();
    assert_eq!(mf.materials.len(), 2);
    assert!(mf.materials[0].is_some() && mf.materials[1].is_none());
    let mv = w.get::<primase_core::MaterialVariants>(protos[0].0).unwrap();
    assert_eq!(mv.0[1].len(), 2);
    let groups: Vec<&str> = w.iter::<SkateLight>().map(|(_, l)| l.group.as_str()).collect();
    assert_eq!(groups, ["", "interior", "interior"]);
    // components show as JSON
    let shown = w.components_json(protos[0].0);
    let shown = shown.unwrap();
    assert!(
        shown
            .iter()
            .any(|c| c.name == "skate.prototype" && c.value.is_some())
    );
    // the scene resource has a JSON view too
    assert!(
        w.resources_json()
            .unwrap()
            .iter()
            .any(|c| c.name == "skate.scene" && c.value.is_some())
    );
}

#[test]
fn markers_and_curves_are_core_facts_too() {
    use primase_core::{Curve, Marker};
    let (w, _) = read(&fixture("core_markers"));
    assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    let markers: Vec<_> = w
        .iter::<Marker>()
        .map(|(e, m)| (m.clone(), w.get::<Transform>(e).copied()))
        .collect();
    let at = |p: [f32; 3]| {
        let mut t = Transform::IDENTITY;
        t.cols[3] = p;
        Some(t)
    };
    let marker = |kind: &str, ground| Marker {
        kind: kind.into(),
        ground,
        yaw: None,
    };
    assert_eq!(
        markers,
        [
            // a height: placed, facing yaw 0 (+Z)
            (marker("spawn", None), at([0., 120., 0.])),
            // no height: a ground position (east, north), no transform, its yaw its own
            (
                Marker {
                    yaw: Some(90.5),
                    ..marker("travel", Some([12., 40.]))
                },
                None,
            ),
            (marker("trainer", None), at([80., 300., 12.])),
            // an effect: its matrix
            (marker("vfx", None), at([0., 140., 0.])),
        ]
    );
    let bits = |t: Transform| t.cols.map(|c| c.map(f32::to_bits));
    assert_eq!(
        bits(markers[0].1.unwrap()),
        bits(at([0., 120., 0.]).unwrap()),
        "yaw 0 is the identity, without -0"
    );
    let curves: Vec<_> = w.iter::<Curve>().map(|(_, c)| c.clone()).collect();
    assert_eq!(
        curves,
        [
            Curve {
                kind: "coping".into(),
                points: vec![[0., 1., 0.], [1., 1., 0.]],
                closed: true,
                tangents: None,
                up: None,
            },
            Curve {
                kind: "rail".into(),
                points: vec![[2., 1., 0.], [3., 1., 1.], [4., 1., 2.]],
                closed: false,
                tangents: None,
                up: None,
            },
        ]
    );
    // yaw 90 faces +X: the marker's local +Z turns to east
    let turned = MANIFEST.replace(
        r#""position": [0, 120, 0], "yaw": 0"#,
        r#""position": [0, 120, 0], "yaw": 90"#,
    );
    assert_ne!(turned, MANIFEST);
    let (w, _) = read(&fixture_with("core_markers_yaw", &turned, |_| {}));
    let (e, _) = w.iter::<Marker>().next().unwrap();
    let c = w.get::<Transform>(e).unwrap().cols;
    for (got, want) in c[2].iter().chain(&c[0]).zip([1., 0., 0., 0., 0., -1.]) {
        assert!((got - want).abs() < 1e-6, "{c:?}");
    }
}

#[test]
fn parts_follow_a_pass_that_rebuilds_the_primitives() {
    let (mut w, _) = read(&fixture("remap_parts"));
    let (proto, parts) = w
        .iter::<components::SkateParts>()
        .find(|(_, p)| p.0.len() == 2)
        .map(|(e, p)| (e, p.0.clone()))
        .unwrap();
    // the second primitive split in two ahead of the first
    w.remap_primitives(proto, &[1, 1, 0]).unwrap();
    let after = &w.get::<components::SkateParts>(proto).unwrap().0;
    assert_eq!(after, &vec![parts[1].clone(), parts[1].clone(), parts[0].clone()]);
    let err = w.remap_primitives(proto, &[3]).unwrap_err();
    assert!(
        err.starts_with("core.material_variants: ") || err.starts_with("skate.parts: "),
        "{err}"
    );
}

#[test]
fn edits_are_written() {
    let (mut w, _) = read_loaded(&fixture("edit_src"));
    // move a placement, give a light a new group and source, drop a prototype's placements' sources
    let (pe, _) = w.iter::<Instance>().next().unwrap();
    w.get_mut::<Transform>(pe).unwrap().cols[3] = [1., 2., 3.];
    let (le, _) = w.iter::<SkateLight>().next().unwrap();
    w.get_mut::<SkateLight>(le).unwrap().group = "street".into();
    w.insert(le, Source("new_light".into()));
    let out = tmp("edit_out");
    interchange::write(&w, &out).unwrap();
    let (w2, _) = read(&out);
    let (pe2, _) = w2.iter::<Instance>().next().unwrap();
    assert_eq!(w2.get::<Transform>(pe2).unwrap().cols[3], [1., 2., 3.]);
    let m = manifest(&out);
    // new strings appended, the old order kept
    assert_eq!(m.get("light_groups"), Some(&json!(["", "interior", "street"])));
    assert_eq!(m.get("light_sources"), Some(&json!(["l0", "new_light"])));
    let (le2, _) = w2.iter::<SkateLight>().next().unwrap();
    assert_eq!(w2.get::<SkateLight>(le2).unwrap().group, "street");
}

/// PLAC holds any 3x4 matrix: a mirrored and a sheared placement are written and read back bit for bit (the format
/// carries them; a target that needs rigid ones declares it and is given bake-nonrigid).
#[test]
fn mirrored_and_sheared_placements_round_trip() {
    let (mut w, _) = read_loaded(&fixture("nonrigid_src"));
    let placements: Vec<_> = w.iter::<Instance>().map(|(e, _)| e).take(2).collect();
    assert_eq!(placements.len(), 2, "the fixture has two placements");
    let mirrored = Transform {
        cols: [
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 2.0, 3.0],
        ],
    };
    let sheared = Transform {
        cols: [
            [1.0, 0.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.0, 0.25, 1.0],
            [-4.0, 0.0, 2.5],
        ],
    };
    w.insert(placements[0], mirrored);
    w.insert(placements[1], sheared);
    let out = tmp("nonrigid_out");
    interchange::write(&w, &out).unwrap();
    let (w2, _) = read(&out);
    let got: Vec<Transform> = w2
        .iter::<Instance>()
        .map(|(e, _)| *w2.get::<Transform>(e).unwrap())
        .take(2)
        .collect();
    let bits = |t: &Transform| t.cols.map(|c| c.map(f32::to_bits));
    assert_eq!(
        got.iter().map(bits).collect::<Vec<_>>(),
        [bits(&mirrored), bits(&sheared)]
    );
}

/// The oracle is the PLAC byte layout: flags are the little-endian u32 at
/// record offset 68, with priority occupying bits 8 through 11.
#[test]
fn placement_priority_round_trips_and_rejects_overflow() {
    let src = fixture("priority_src");
    let path = src.join("tables/placements.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    for (i, priority) in [0u32, 1, 3, 15].into_iter().enumerate() {
        let at = 16 + i * 76 + 68;
        let old = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let unknown = (1 << 6) | (1 << 7) | (1 << 12) | (1 << 31);
        bytes[at..at + 4].copy_from_slice(&((old & !0xF00) | unknown | (priority << 8)).to_le_bytes());
    }
    std::fs::write(&path, &bytes).unwrap();
    let (mut w, _) = read_loaded(&src);
    let priorities: Vec<_> = w.iter::<primase_core::Priority>().map(|(_, p)| p.0).collect();
    assert_eq!(priorities, [0, 1, 3, 15]);
    let out = tmp("priority_out");
    interchange::write(&w, &out).unwrap();
    assert_eq!(std::fs::read(out.join("tables/placements.bin")).unwrap(), bytes);
    let placed = w.iter::<Instance>().next().unwrap().0;
    w.insert(placed, primase_core::Priority(16));
    let error = interchange::write(&w, &tmp("priority_overflow"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("priority 16") && error.contains("0-15"), "{error}");
    w.insert(placed, primase_core::Priority(3));
    w.get_mut::<components::SkatePlacement>(placed)
        .unwrap()
        .unknown_flags |= 1 << 8;
    let error = interchange::write(&w, &tmp("priority_conflict"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("priority bits must be stored in core.priority"),
        "{error}"
    );
}

#[test]
fn normal_maps_with_x_in_alpha_are_refused() {
    let (mut w, _) = read(&fixture("alpha_x_src"));
    let (te, _) = w
        .iter::<primase_reskate::components::SkateTexture>()
        .next()
        .unwrap();
    w.insert(
        te,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255, 128, 255, 200]],
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(NormalConvention {
                green: NormalGreen::Up,
                z: NormalZ::Rebuild,
                x: NormalX::Alpha,
                basis: primase_core::NormalBasis::TangentBitangent,
            }),
        },
    );
    let e = interchange::write(&w, &tmp("alpha_x_out"))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("X outside red") && e.contains("normalize-normals"),
        "{e}"
    );
    w.get_mut::<Texture>(te).unwrap().normal = Some(NormalConvention {
        basis: primase_core::NormalBasis::BitangentTangent,
        ..NormalConvention::GLTF
    });
    let e = interchange::write(&w, &tmp("bitangent_first_out"))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("non-tangent/bitangent basis") && e.contains("normalize-normals"),
        "{e}"
    );
}

#[test]
fn chunked_tables_keep_their_chunks() {
    let m = MANIFEST
        .replace(
            r#""placements": "tables/placements.bin","#,
            r#""placements": ["tables/p0.bin", {"file": "tables/p1.bin"}],"#,
        )
        .replace(
            r#""collision/other.bin"]"#,
            r#"{"file": ["collision/c0.bin", "collision/c1.bin"], "mode": "water"}]"#,
        );
    let src = fixture_with("chunks_src", &m, |d| {
        put(
            d,
            "tables/p0.bin",
            &tables::write_placements(&[plac(0, 0, 1., 0, 0), plac(1, 0, 2., 0, 1)]),
        );
        put(
            d,
            "tables/p1.bin",
            &tables::write_placements(&[plac(2, 0, 3., 0, u32::MAX)]),
        );
        put(d, "collision/c0.bin", &tables::write_soup(&soup()));
        put(d, "collision/c1.bin", &tables::write_soup(&soup()));
    });
    let (w, rep) = read_loaded(&src);
    assert_eq!(w.iter::<Instance>().count(), 3);
    assert_eq!(rep.tolerated.len(), 1, "{:?}", rep.tolerated); // the list item {"file"} without sha256
    let out = tmp("chunks_out");
    interchange::write(&w, &out).unwrap();
    same_tree(
        &src,
        &out,
        &[
            "tables/p0.bin",
            "tables/p1.bin",
            "collision/c0.bin",
            "collision/c1.bin",
        ],
    );
    // the merged soup indexes the second chunk's vertices rebased
    let soups: Vec<_> = w.iter::<primase_core::Collision>().collect();
    let merged = soups.iter().find(|(_, s)| s.triangles.len() == 4).unwrap().1;
    assert_eq!(merged.triangles[2], [4, 5, 6]);
}

#[test]
fn sha256_is_checked_and_rewritten() {
    use sha2::Digest;
    let h: String = sha2::Sha256::digest(NORMAL_PNG.get_or_init(|| png(1, 1, [128, 128, 255, 255])))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let m = MANIFEST.replace(
        r#""wall_n": {"file": "textures/wall_n.png","#,
        &format!(
            r#""wall_n": {{"file": "textures/wall_n.png", "sha256": "{}","#,
            h.to_uppercase()
        ),
    );
    let src = fixture_with("sha_src", &m, |_| {});
    let (w, _) = read_loaded(&src);
    let out = tmp("sha_out");
    interchange::write(&w, &out).unwrap();
    let got = manifest(&out);
    assert_eq!(
        got.get("textures").unwrap().get("wall_n").unwrap().get("sha256"),
        Some(&json!(h))
    );
    let bad = fixture_with("sha_bad", &m.replace(&h.to_uppercase(), &"0".repeat(64)), |_| {});
    assert!(read_err(&bad).contains("sha256"), "{}", read_err(&bad));
}

/// A case name, the manifest, an edit of the folder, and the reason expected.
type Case = (&'static str, String, Box<dyn Fn(&Path)>, &'static str);

/// One invalid change and the reason the reader gives (the reference validator's wording).
#[test]
fn invalid_input_fails_with_its_reason() {
    let cases: Vec<Case> = vec![
        (
            "format",
            MANIFEST.replace(r#""format": "skate-scene""#, r#""format": "other""#),
            Box::new(|_| {}),
            "format is not \"skate-scene\"",
        ),
        (
            "version",
            MANIFEST.replace(r#""version": 1"#, r#""version": 2"#),
            Box::new(|_| {}),
            "version is not 1",
        ),
        (
            "name",
            MANIFEST.replace("fixture_a", "fixture a"),
            Box::new(|_| {}),
            "name 'fixture a' is not [A-Za-z0-9_-]+",
        ),
        (
            "region",
            MANIFEST.replace("[-100, -100.5, 100, 0.30000001192092896]", "[1, 0, 0, 1]"),
            Box::new(|_| {}),
            "region is empty",
        ),
        (
            "surface",
            MANIFEST.replace("\"asphalt\"", "\"lava\""),
            Box::new(|_| {}),
            "surfaces[2]: unknown surface 'lava'",
        ),
        (
            "packed",
            MANIFEST.replace("packed:524384", "packed:7"),
            Box::new(|_| {}),
            "surfaces[3]: 'packed:7' is not a Studio packed material (n << 6 | 32)",
        ),
        (
            "tex dup",
            MANIFEST.replace(r#""green": "down""#, r#""name": "WALL_D", "green": "down""#),
            Box::new(|_| {}),
            "texture 'wall_n': name 'WALL_D' is used twice (one name, one image)",
        ),
        (
            "tex missing",
            MANIFEST.replace("textures/wall_n.png", "textures/none.png"),
            Box::new(|_| {}),
            "texture 'wall_n': 'textures/none.png' does not exist",
        ),
        (
            "outside",
            MANIFEST.replace("textures/wall_n.png", "../wall_n.png"),
            Box::new(|_| {}),
            "texture 'wall_n': '../wall_n.png' is outside the scene folder",
        ),
        (
            "backslash",
            MANIFEST.replace("textures/wall_n.png", "textures\\\\wall_n.png"),
            Box::new(|_| {}),
            "is not a relative path with forward slashes",
        ),
        (
            "class",
            MANIFEST.replace(r#""class": "glass""#, r#""class": "metal""#),
            Box::new(|_| {}),
            "material 'glass': class 'metal' unknown",
        ),
        (
            "mat case",
            MANIFEST.replace(r#""glass": {"class""#, r#""WALL": {"class""#),
            Box::new(|_| {}),
            "material keys 'WALL' and another differ only in case",
        ),
        (
            "role",
            MANIFEST.replace(r#""metallic": 0.1"#, r#""mask": "wall_d""#),
            Box::new(|_| {}),
            "texture 'wall_d' is used as color and as mask",
        ),
        (
            "tex unknown",
            MANIFEST.replace(r#""normal": "wall_n""#, r#""normal": "nope""#),
            Box::new(|_| {}),
            "material 'wall': normal texture 'nope' unknown",
        ),
        (
            "bytes",
            MANIFEST.replace("[200, 40, 40, 255]", "[200, 40, 400, 255]"),
            Box::new(|_| {}),
            "material 'trim_red': 'base_color' is not 4 integers 0-255",
        ),
        (
            "range",
            MANIFEST.replace(r#""roughness": 0.6"#, r#""roughness": 1.5"#),
            Box::new(|_| {}),
            "material 'wall': 'roughness' 1.5 is not in [0, 1]",
        ),
        (
            "proto dup",
            MANIFEST.replace(r#""id": "shared""#, r#""id": "wall_01""#),
            Box::new(|_| {}),
            "prototype id 'wall_01' twice",
        ),
        (
            "key dup",
            MANIFEST.replace(r#""foliage": true"#, r#""key": 305419896"#),
            Box::new(|_| {}),
            "prototypes 'wall_01' and 'shared' have the same key 305419896",
        ),
        (
            "part mat",
            MANIFEST.replace(r#"[{"material": "glass"}]"#, r#"[{"material": "nope"}]"#),
            Box::new(|_| {}),
            "prototype 'shared': material 'nope' unknown",
        ),
        (
            "variant key",
            MANIFEST.replace(r#""3": "glass""#, r#""x": "glass""#),
            Box::new(|_| {}),
            "prototype 'wall_01': variant 'x' is not a number",
        ),
        (
            "col mode",
            MANIFEST.replace(r#""mode": "hull""#, r#""mode": "lava""#),
            Box::new(|_| {}),
            "prototype 'wall_01': collision mode 'lava'",
        ),
        (
            "water convex",
            MANIFEST.replace(
                "[[0, 0], [10, 0], [10, 10], [0, 10]]",
                "[[0, 0], [10, 0], [1, 1], [0, 10]]",
            ),
            Box::new(|_| {}),
            "water[0]: the outline is not convex",
        ),
        (
            "water few",
            MANIFEST.replace("[[0, 0], [10, 0], [10, 10], [0, 10]]", "[[0, 0], [10, 0]]"),
            Box::new(|_| {}),
            "water[0]: fewer than 3 corners",
        ),
        (
            "kind",
            MANIFEST.replace(r#""kind": "trainer""#, r#""kind": "boss""#),
            Box::new(|_| {}),
            "markers[2]: kind 'boss'",
        ),
        (
            "blueprint",
            MANIFEST.replace(r#""blueprint": "vfx/weather/snowfall", "#, ""),
            Box::new(|_| {}),
            "markers[3]: an effect needs a blueprint",
        ),
        (
            "snap none",
            MANIFEST.replace(r#""snap": "street""#, r#""snap": "none""#),
            Box::new(|_| {}),
            "markers[1]: snap none needs a height",
        ),
        (
            "mirror m",
            MANIFEST.replace(
                "[1,0,0, 0,1,0, 0,0,1, 0,140,0]",
                "[-1,0,0, 0,1,0, 0,0,1, 0,140,0]",
            ),
            Box::new(|_| {}),
            "markers[3].m: a mirrored or zero-scale basis",
        ),
        (
            "json",
            MANIFEST.replace(r#""light_prefix": "fx_","#, r#""light_prefix": "fx_",,"#),
            Box::new(|_| {}),
            "scene.json: key must be a string at line 6 column 25",
        ),
        (
            "streaming",
            MANIFEST.replace(r#""far": 500"#, r#""far": -5"#),
            Box::new(|_| {}),
            "streaming_distances: 'far' -5 is not in [0, 1000000]",
        ),
        // tables
        (
            "magic",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut b = std::fs::read(d.join("tables/lights.bin")).unwrap();
                b[0] = b'X';
                put(d, "tables/lights.bin", &b);
            }),
            "tables/lights.bin: not an SKST table",
        ),
        (
            "size",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut b = std::fs::read(d.join("tables/placements.bin")).unwrap();
                b.pop();
                put(d, "tables/placements.bin", &b);
            }),
            "tables/placements.bin: 319 bytes, the header says 320",
        ),
        (
            "proto index",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                put(
                    d,
                    "tables/placements.bin",
                    &tables::write_placements(&[plac(7, 0, 1., 0, u32::MAX)]),
                );
            }),
            "placement 0: prototype 7 of 3",
        ),
        (
            "source index",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                put(
                    d,
                    "tables/placements.bin",
                    &tables::write_placements(&[plac(0, 0, 1., 0, 2)]),
                );
            }),
            "placement 0: source 2 of 2",
        ),
        (
            "variant material",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                put(
                    d,
                    "tables/placements.bin",
                    &tables::write_placements(&[plac(0, 5, 1., 0, 0)]),
                );
            }),
            "prototype 'wall_01' part 1: no material for variant 5",
        ),
        (
            "soup vertex",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut s = soup();
                s.triangles[1][1] = 9;
                put(d, "collision/world.bin", &tables::write_soup(&s));
            }),
            "collision/world.bin: triangle 1 indexes vertex 9 of 4",
        ),
        (
            "soup surface",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut s = soup();
                s.surfaces[0] = 4;
                put(d, "collision/world.bin", &tables::write_soup(&s));
            }),
            "collision/world.bin: triangle 0 has surface 4 of 4",
        ),
        (
            "light group",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                put(d, "tables/lights.bin", &tables::write_lights(&[light(2, 0)]));
            }),
            "light 0: group 2 of 2",
        ),
        (
            "light type",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut l = light(0, 0);
                l.kind = 2;
                put(d, "tables/lights.bin", &tables::write_lights(&[l]));
            }),
            "light 0: type 2",
        ),
        (
            "nan",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                let mut l = light(0, 0);
                l.energy = f32::INFINITY;
                put(d, "tables/lights.bin", &tables::write_lights(&[l]));
            }),
            "tables/lights.bin: a non-finite float at byte 68",
        ),
        (
            "curve range",
            MANIFEST.into(),
            Box::new(|d: &Path| {
                put(
                    d,
                    "tables/grind.bin",
                    &tables::write_curves(&CurveTable {
                        curves: vec![CurveRec {
                            first: 1,
                            count: 2,
                            flags: 0,
                            radius: 0.,
                            surface: 0,
                        }],
                        points: vec![[0.; 3], [1.; 3]],
                        reserved: 0,
                    }),
                );
            }),
            "tables/grind.bin: curve 0 indexes points 1+2 of 2",
        ),
    ];
    let validator = primase_testdata::real_data("PRIMASE_SCENE_SKATE");
    let mut differ = Vec::new();
    for (name, m, edit, want) in cases {
        let d = fixture_with(&format!("bad_{}", name.replace(' ', "_")), &m, edit);
        let got = read_err(&d);
        assert!(got.contains(want), "{name}: got {got:?}, want {want:?}");
        // the reference validator refuses it too, for the same reason
        if let Some(v) = &validator {
            match reference(v, &d) {
                Ok(()) => panic!("{name}: the reference validator accepts it"),
                // table messages: the reference names the file by its full path, we by the scene-relative one
                Err(msg) if !norm(&msg).contains(want) && name != "json" => {
                    differ.push(format!("{name}: ours {got:?}, reference {msg:?}"))
                }
                Err(_) => {}
            }
        }
    }
    assert!(
        differ.is_empty(),
        "causes differ:
{}",
        differ.join(
            "
"
        )
    );
}

/// A reference message with the fixture folder's path taken out of file names.
fn norm(msg: &str) -> String {
    let m = msg.replace('\\', "/");
    match m.find("/skate-scene/bad_") {
        Some(i) => {
            let rest = &m[i + "/skate-scene/bad_".len()..];
            let head = m[..i].rfind(' ').map_or("", |j| &m[..j + 1]);
            format!("{head}{}", rest.split_once('/').map_or(rest, |(_, r)| r))
        }
        None => m,
    }
}

/// The reference validator on a folder: Ok, or its error message.
fn reference(v: &Path, dir: &Path) -> Result<(), String> {
    let mut cmd = if v.extension().is_some_and(|e| e == "dll") {
        let mut c = std::process::Command::new("dotnet");
        c.arg(v);
        c
    } else {
        std::process::Command::new(v)
    };
    let out = cmd.arg("validate").arg(dir).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(())
    } else {
        Err(text.trim().to_owned())
    }
}

#[test]
fn the_reference_validator_accepts_the_fixture_and_our_output() {
    let Some(v) = primase_testdata::real_data("PRIMASE_SCENE_SKATE") else {
        return;
    };
    let src = fixture("oracle_src");
    reference(&v, &src).unwrap();
    let (w, _) = read_loaded(&src);
    let out = tmp("oracle_out");
    interchange::write(&w, &out).unwrap();
    reference(&v, &out).unwrap();
}

#[test]
fn writer_refuses_unsafe_outputs() {
    let (w, _) = read(&fixture("refuse_src"));
    let out = tmp("refuse_out");
    put(&out, "stale.txt", b"x");
    assert!(
        interchange::write(&w, &out)
            .unwrap_err()
            .to_string()
            .contains("is not empty")
    );
    let mut w2 = World::new();
    w2.spawn();
    assert!(
        interchange::write(&w2, &tmp("refuse_out2"))
            .unwrap_err()
            .to_string()
            .contains("no skate.scene")
    );
}

/// lower-skate-pbr: class defaults, colours to linear factors, texture roles and normal convention set on loaded
/// textures, the skate material kept.
#[test]
fn lowering_to_pbr() {
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::pbr::{AlphaMode, PbrMaterial, Wrap};
    use primase_core::{ColorSpace, NormalGreen, NormalZ, TexFormat, TexRole, Texture};
    use primase_reskate::components::{SkateMaterial, SkateTexture};

    let (mut w, _) = read(&fixture("lower"));
    let key = |w: &World, k: &str| w.iter::<SkateMaterial>().find(|(_, m)| m.key == k).unwrap().0;
    let tex = |w: &World, k: &str| w.iter::<SkateTexture>().find(|(_, t)| t.key == k).unwrap().0;
    // wall_n is loaded (as a texture pass would), wall_d stays a file
    let wall_n = tex(&w, "wall_n");
    w.insert(
        wall_n,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![128, 128, 255, 255]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Other,
            normal: None,
        },
    );
    let mut reg = PassRegistry::new();
    primase_reskate::register(&mut reg);
    let m = PassManager::new(reg);
    let mut report = RunReport::default();
    m.run(&mut w, &m.registry.parse("lower-skate-pbr").unwrap(), &mut report)
        .unwrap();
    assert_eq!(report.steps[0].log.counts.get("materials"), Some(&3));

    let wall = w.get::<PbrMaterial>(key(&w, "wall")).unwrap();
    assert_eq!(
        wall.base_color_texture.map(|t| t.texture),
        Some(tex(&w, "wall_d"))
    );
    assert_eq!(
        wall.normal_texture.map(|t| t.texture),
        Some(wall_n),
        "loaded: kept for the normal pass"
    );
    assert_eq!(
        (wall.metallic, wall.roughness, wall.alpha_mode),
        (0.0, 0.6, AlphaMode::Blend),
        "vertex_alpha: blended so COLOR_0's alpha fades it"
    );
    assert_eq!(
        report.steps[0].log.counts.get("vertex-alpha materials blended"),
        Some(&1)
    );
    let t = w.get::<Texture>(wall_n).unwrap();
    assert_eq!((t.role, t.color_space), (TexRole::Normal, ColorSpace::Linear));
    assert_eq!(
        t.normal.map(|n| (n.green, n.z)),
        Some((NormalGreen::Down, NormalZ::Rebuild))
    );

    let trim = w.get::<PbrMaterial>(key(&w, "trim_red")).unwrap();
    // [200, 40, 40] sRGB -> linear
    assert_eq!(trim.base_color[3], 1.0);
    assert!(
        (trim.base_color[0] - 0.577_580_3).abs() < 1e-6 && (trim.base_color[1] - 0.021_219_01).abs() < 1e-6
    );
    assert_eq!(
        trim.alpha_mode,
        AlphaMode::Mask { cutoff: 0.5 },
        "vertex_alpha false"
    );
    assert!(trim.double_sided, "cutout is two-sided by default");
    // a vertex-alpha cutout is blended too, its cutoff counted as dropped
    let mut faded = w.get::<SkateMaterial>(key(&w, "trim_red")).unwrap().clone();
    faded.vertex_alpha = Some(true);
    let mut log = primase_core::pass::PassLog::default();
    let lowered = primase_reskate::passes::lower::lower_material(&faded, None, &w, &mut log);
    assert_eq!(lowered.alpha_mode, AlphaMode::Blend);
    assert_eq!(
        log.approximations
            .get("vertex-alpha cutout materials blended, cutoff dropped"),
        Some(&1)
    );

    let glass = w.get::<PbrMaterial>(key(&w, "glass")).unwrap();
    assert_eq!((glass.roughness, glass.alpha_mode), (0.15, AlphaMode::Blend));
    assert_eq!(glass.cast_shadows, Some(false), "a stated false is kept");
    assert_eq!(wall.cast_shadows, None, "unknown stays unknown");
    assert_eq!(glass.base_color_texture, None);
    let _ = Wrap::Clamp;
    // lower and keep
    assert_eq!(w.iter::<SkateMaterial>().count(), 3);
}

#[test]
fn soups_become_core_collision() {
    use primase_core::{Collision, Surfaces};
    let (w, _) = read(&fixture("collision_core"));
    // the scene's surfaces are named in the world's table; bit 7 is skate.rock
    let names: Vec<&str> = w
        .resource::<Surfaces>()
        .unwrap()
        .0
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["default", "concrete", "asphalt", "packed:524384"]);
    let soups: Vec<_> = w.iter::<Collision>().collect();
    assert!(!soups.is_empty());
    for (e, c) in soups.iter().filter(|(_, c)| !c.triangles.is_empty()) {
        // fixture soups: surface byte 1, then rock | 2
        assert_eq!(c.surfaces[..2], [1, 2], "{e:?}");
        let rock = w.get::<components::SkateRock>(*e).unwrap();
        assert_eq!(rock.0[..2], [false, true]);
    }
    assert_eq!(w.verify_components(), Vec::new());
}

#[test]
fn collision_placed_by_an_entity_is_a_world_soup() {
    use primase_core::{Collision, Surfaces};
    let (mut w, _) = read_loaded(&fixture("collision_self"));
    let before = w.iter::<components::SkateCollision>().count();
    let asphalt = w.resource_mut::<Surfaces>().unwrap().index("asphalt").unwrap();
    let e = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        &[[0, 1, 2]],
        asphalt,
        |p| p,
    );
    w.insert(e, c);
    let mut t = Transform::IDENTITY;
    t.cols[3] = [10.0, 2.0, -3.0];
    w.insert(e, t);
    let out = tmp("collision_self_out");
    interchange::write(&w, &out).unwrap();
    let m = manifest(&out);
    let soups = m["collision"].as_array().unwrap();
    assert_eq!(soups.len(), before + 1);
    // a reference with nothing but its file is written as the file name
    let file = soups[before].as_str().or(soups[before]["file"].as_str()).unwrap();
    assert_eq!(file, format!("collision/world_{before}.bin"));
    let t = tables::read_soup(&std::fs::read(out.join(file)).unwrap(), 4).unwrap();
    assert_eq!(
        t.positions[1],
        [11.0, 2.0, -3.0],
        "in scene space through the transform"
    );
    assert_eq!(t.surfaces, [2], "asphalt is the scene's surface 2");
    // a surface the scene has no entry for is refused
    let lava = w.resource_mut::<Surfaces>().unwrap().index("lava").unwrap();
    w.get_mut::<Collision>(e).unwrap().surfaces[0] = lava;
    let err = interchange::write(&w, &tmp("collision_self_bad"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("'lava' is not one of the scene's surfaces"), "{err}");
}

#[test]
fn mirrored_collision_keeps_its_fronts() {
    use primase_core::{Collision, Surfaces};
    let (mut w, _) = read_loaded(&fixture("collision_mirror"));
    let before = w.iter::<components::SkateCollision>().count();
    let concrete = w.resource_mut::<Surfaces>().unwrap().index("concrete").unwrap();
    // a triangle facing up (+y), placed through a mirror in x
    let e = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        &[[0, 1, 2]],
        concrete,
        |p| p,
    );
    w.insert(e, c);
    let mut t = Transform::IDENTITY;
    t.cols[0] = [-1.0, 0.0, 0.0];
    w.insert(e, t);
    let out = tmp("collision_mirror_out");
    interchange::write(&w, &out).unwrap();
    let m = manifest(&out);
    let entry = &m["collision"][before];
    let file = entry.as_str().or(entry["file"].as_str()).unwrap();
    let s = tables::read_soup(&std::fs::read(out.join(file)).unwrap(), 4).unwrap();
    let [a, b, c] = s.triangles[0].map(|i| s.positions[i as usize]);
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let ny = u[2] * v[0] - u[0] * v[2];
    assert!(ny > 0.0, "the front still faces up after the mirror: {ny}");
    assert_eq!(s.positions[1], [-1.0, 0.0, 0.0]);
}

/// The fixture with levels of detail: `wall_01` (mesh 0, two parts) also has mesh 1 at level 1 (one part with
/// variants and a key of its own) and a level 3 that draws nothing, taking level 1 by default; `shared` has only a
/// level of its own.
fn lods_manifest() -> Value {
    let mut m: Value = serde_json::from_str(MANIFEST).unwrap();
    m["prototypes"][0]["lods"] = json!([
        {"detail": 1, "mesh": 1, "parts": [{"material": null, "variants": {"0": "trim_red", "3": "glass"},
            "source": "lod1", "extra": 2}], "note": "kept"},
        {"detail": 3, "mesh": null, "parts": []}
    ]);
    m["prototypes"][0]["lod_default"] = json!(1);
    m["prototypes"][1]["detail"] = json!(2);
    m
}

#[test]
fn levels_of_detail_round_trip_exactly_as_forms() {
    use primase_core::{LodForm, Lods, MaterialVariants, MeshFile};
    let input = lods_manifest();
    let src = fixture_with("lods_src", &input.to_string(), |_| {});
    let (w, rep) = read_loaded(&src);
    assert_eq!(rep.tolerated.len(), 0, "{:?}", rep.tolerated);
    // the forms come after every other entity: the rest keep the ids of the scene without them
    let (plain, _) = read(&fixture("lods_plain"));
    let ids = |w: &World| w.entities().take(plain.entity_count()).collect::<Vec<_>>();
    assert_eq!(ids(&w), ids(&plain));
    assert_eq!(w.entity_count(), plain.entity_count() + 2);
    let protos: Vec<_> = w.iter::<SkatePrototype>().map(|(e, _)| e).collect();
    let lods = w.get::<Lods>(protos[0]).unwrap();
    let [coarse, nothing] = [lods.forms[0].form, lods.forms[1].form];
    assert_eq!((lods.detail, lods.default, lods.forms.len()), (0, Some(1), 2));
    assert_eq!(
        lods.forms[1],
        LodForm {
            detail: 3,
            form: nothing
        }
    );
    let file = w.get::<MeshFile>(coarse).unwrap();
    assert_eq!(
        (file.mesh, &file.path),
        (1, &w.get::<MeshFile>(protos[0]).unwrap().path)
    );
    assert_eq!(w.get::<MaterialVariants>(coarse).unwrap().0[0].len(), 2);
    assert_eq!(
        w.get::<components::SkateParts>(coarse).unwrap().0[0].source,
        Some(json!("lod1"))
    );
    assert_eq!(
        w.get::<components::SkateForm>(coarse).unwrap().other["note"],
        "kept"
    );
    assert!(!w.has::<MeshFile>(nothing) && w.get::<components::SkateParts>(nothing).unwrap().0.is_empty());
    assert_eq!(
        w.get::<Lods>(protos[1]).unwrap(),
        &Lods {
            detail: 2,
            forms: vec![],
            default: Some(0)
        }
    );
    assert!(w.get::<Lods>(protos[2]).is_none());
    assert_eq!(w.verify_components(), vec![]);
    let out = tmp("lods_out");
    std::fs::remove_dir(&out).unwrap();
    interchange::write(&w, &out).unwrap();
    same_tree(&src, &out, ALL_FILES);
    let c = compare_json(&input, &manifest(&out), 0);
    assert!(c.differences.is_empty(), "{:?}", c.differences);
    // a world whose levels of detail have no default form is refused: the format always names one
    let mut none = w.clone();
    none.get_mut::<Lods>(protos[0]).unwrap().default = None;
    let err = interchange::write(&none, &tmp("lods_no_default"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("without a default form"), "{err}");
    // a prototype's closure keeps its forms with it
    let keep = w.closure([protos[0]]);
    assert!(keep.contains(&coarse) && keep.contains(&nothing));
}

#[test]
fn a_selected_form_is_written_as_the_prototype() {
    use primase_core::pass::{Pass, PassLog};
    let src = fixture_with("lods_select", &lods_manifest().to_string(), |_| {});
    let (mut w, _) = read_loaded(&src);
    let mut log = PassLog::default();
    primase_core::SelectLod::default().run(&mut w, &mut log).unwrap();
    assert_eq!(log.counts["prototypes drawn with another form"], 1);
    assert_eq!(log.counts["prototypes drawn with their own form"], 1);
    assert_eq!(log.counts["forms left out"], 1);
    let out = tmp("lods_select_out");
    interchange::write(&w, &out).unwrap();
    let p = &manifest(&out)["prototypes"];
    let want = lods_manifest()["prototypes"][0]["lods"][0]["parts"].clone();
    assert_eq!((&p[0]["mesh"], &p[0]["parts"]), (&json!(1), &want));
    assert!(p[0].get("lods").is_none() && p[0].get("lod_default").is_none());
    assert!(p[1].get("detail").is_none());
    // the finest instead: the prototypes as they are without levels of detail
    let (mut w, _) = read_loaded(&src);
    primase_core::SelectLod {
        choice: primase_core::LodChoice::Finest,
    }
    .run(&mut w, &mut PassLog::default())
    .unwrap();
    let out = tmp("lods_finest_out");
    interchange::write(&w, &out).unwrap();
    let want = serde_json::from_str::<Value>(MANIFEST).unwrap()["prototypes"].clone();
    assert_eq!(manifest(&out)["prototypes"], want);
}

#[test]
fn levels_of_detail_are_checked_as_the_validator_does() {
    let with = |name: &str, edit: &dyn Fn(&mut Value)| {
        let mut m = lods_manifest();
        edit(&mut m);
        fixture_with(name, &m.to_string(), |_| {})
    };
    type Case<'a> = (&'a str, &'a dyn Fn(&mut Value), &'a str);
    let cases: [Case; 7] = [
        (
            "lods_no_glb",
            &|m| m["prototypes"][2]["lods"] = json!([{"detail": 1, "mesh": 0, "parts": []}]),
            "prototype 'empty': lods without a glb",
        ),
        (
            "lods_default_range",
            &|m| m["prototypes"][0]["lod_default"] = json!(3),
            "prototype 'wall_01': lod_default 3 of 3 forms",
        ),
        (
            "lods_default_negative",
            &|m| m["prototypes"][1]["lod_default"] = json!(-1),
            "prototype 'shared': lod_default -1 of 1 forms",
        ),
        (
            "lods_null_mesh_parts",
            &|m| m["prototypes"][0]["lods"][1]["parts"] = json!([{"material": "glass"}]),
            "prototype 'wall_01': a lod without a mesh has parts",
        ),
        (
            "lods_detail_range",
            &|m| m["prototypes"][0]["lods"][0]["detail"] = json!(256),
            "out of range",
        ),
        (
            "lods_unknown_material",
            &|m| m["prototypes"][0]["lods"][0]["parts"][0]["variants"]["0"] = json!("absent"),
            "prototype 'wall_01': material 'absent' unknown",
        ),
        // the form taken by default needs a material for every variant placed (wall_01: 0 and 3)
        (
            "lods_default_variant",
            &|m| m["prototypes"][0]["lods"][0]["parts"][0]["variants"] = json!({"0": "glass"}),
            "prototype 'wall_01' part 0: no material for variant 3",
        ),
    ];
    for (name, edit, want) in cases {
        let err = read_err(&with(name, edit));
        assert!(err.contains(want), "{name}: {err}");
    }
    // a form not taken by default is not checked for the variants placed, as by the validator
    let dir = with("lods_other_variant", &|m| {
        m["prototypes"][0]["lods"][0]["parts"][0]["variants"] = json!({"0": "glass"});
        m["prototypes"][0]["lod_default"] = json!(0);
    });
    let (_, rep) = read(&dir);
    assert_eq!(rep.tolerated["explicit \"lod_default\": 0 (written absent)"], 1);
    // a default form that draws nothing: its placements draw nothing
    let dir = with("lods_default_nothing", &|m| {
        m["prototypes"][0]["lod_default"] = json!(2)
    });
    let (_, rep) = read(&dir);
    assert!(
        rep.warnings
            .contains(&"prototype 'wall_01' has no geometry; its placements draw nothing".to_owned()),
        "{:?}",
        rep.warnings
    );
}

#[test]
fn levels_of_detail_defaults_are_tolerated_and_written_in_full() {
    let mut m = lods_manifest();
    m["prototypes"][0]["detail"] = json!(0);
    m["prototypes"][0]["lods"][1] = json!({"parts": []});
    m["prototypes"][1]["lods"] = json!([]);
    let dir = fixture_with("lods_tolerated", &m.to_string(), |_| {});
    let (w, rep) = read_loaded(&dir);
    for t in [
        "explicit \"detail\": 0 (written absent)",
        "lod without \"detail\" (written 0)",
        "lod without \"mesh\" (written null)",
        "explicit empty \"lods\" (written absent)",
    ] {
        assert_eq!(rep.tolerated.get(t), Some(&1), "{t}: {:?}", rep.tolerated);
    }
    let out = tmp("lods_tolerated_out");
    interchange::write(&w, &out).unwrap();
    let p = &manifest(&out)["prototypes"];
    assert!(p[0].get("detail").is_none() && p[1].get("lods").is_none());
    assert_eq!(p[0]["lods"][1], json!({"detail": 0, "mesh": null, "parts": []}));
}

/// The fixture with a pause-map picture (`ui/pause.png`, 2 x 2).
fn fixture_with_pause(name: &str) -> PathBuf {
    let manifest = MANIFEST.replace(
        r#""max_render_edge": 48,"#,
        r#""max_render_edge": 48, "pause_map": {"image": "ui/pause.png", "bounds": [-10, -10, 10, 10]},"#,
    );
    fixture_with(name, &manifest, |d| {
        put(d, "ui/pause.png", &png(2, 2, [10, 20, 30, 255]))
    })
}

/// The writer reads no file and the loaded payloads are the authority: with the source folder gone, an unchanged
/// world writes every file as it was read, byte for byte; an edited texture, mesh and pause picture are written as
/// edited, though their file references still name the originals; a scene read but not loaded is refused.
#[test]
fn loaded_payloads_are_the_authority() {
    let src = fixture_with_pause("authority_src");
    let files: Vec<&str> = [&["ui/pause.png"], ALL_FILES].concat();
    let original: std::collections::BTreeMap<&str, Vec<u8>> = files
        .iter()
        .map(|f| (*f, std::fs::read(src.join(f)).unwrap()))
        .collect();
    let (unloaded, _) = read(&src);
    let (mut w, _) = read_loaded(&src);
    std::fs::remove_dir_all(&src).unwrap();
    // read but not loaded: refused, whatever the folder holds
    let e = interchange::write(&unloaded, &tmp("authority_unloaded")).unwrap_err();
    assert!(e.to_string().contains("load the scene before writing it"), "{e}");
    // unchanged, the source gone: every file as it was read
    let out = tmp("authority_same");
    let report = interchange::write(&w, &out).unwrap();
    for (f, bytes) in &original {
        assert_eq!(&std::fs::read(out.join(f)).unwrap(), bytes, "{f}");
    }
    assert!(
        report.copied.iter().any(|f| f == "ui/pause.png"),
        "{:?}",
        report.copied
    );
    // edited: a texture's pixel, a mesh's vertex, the picture's pixel
    let (te, _) = w
        .iter::<primase_core::TextureFile>()
        .find(|(_, f)| f.path.ends_with("wall_d.png"))
        .unwrap();
    w.get_mut::<primase_core::Texture>(te).unwrap().levels[0][0] = 7;
    let (me, _) = w
        .iter::<primase_core::MeshFile>()
        .find(|(_, f)| f.mesh == 1)
        .unwrap();
    {
        let mesh = w.get_mut::<primase_core::Mesh>(me).unwrap();
        let mut first = mesh.primitive_mut(0).unwrap();
        if let Some(primase_core::Stream::F32x3(p)) =
            first.streams_mut().get_mut(&primase_core::Semantic::Position)
        {
            p[0][0] += 1.0;
        }
    }
    w.resource_mut::<components::SkatePauseImage>().unwrap().rgba[0] = 99;
    let out = tmp("authority_edit");
    interchange::write(&w, &out).unwrap();
    let (mut back, _) = read(&out);
    interchange::load(&out, &mut back).unwrap();
    let texture = back
        .iter::<primase_core::TextureFile>()
        .find(|(_, f)| f.path.ends_with("wall_d.png"))
        .map(|(e, _)| back.get::<primase_core::Texture>(e).unwrap())
        .unwrap();
    assert_eq!(texture.levels[0][0], 7);
    assert_ne!(
        std::fs::read(out.join("textures/wall_d.png")).unwrap(),
        original["textures/wall_d.png"]
    );
    assert_eq!(
        back.resource::<components::SkatePauseImage>().unwrap().rgba[0],
        99
    );
    // the edited mesh drew from a GLB another prototype shares: it gets a file of its own beside it (its mesh 0), and
    // the shared file passes through for the other prototype
    let m = manifest(&out);
    let proto = |id: &str| {
        m["prototypes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    let shared = proto("shared");
    assert_eq!(shared["glb"], json!("prototypes/wall.shared.glb"), "{shared}");
    assert!(shared.get("mesh").is_none(), "{shared}");
    assert_eq!(proto("wall_01")["glb"], json!("prototypes/wall.glb"));
    assert_eq!(
        std::fs::read(out.join("prototypes/wall.glb")).unwrap(),
        original["prototypes/wall.glb"]
    );
    let (shared_e, _) = back
        .iter::<primase_core::MeshFile>()
        .find(|(_, f)| f.path.ends_with("wall.shared.glb"))
        .unwrap();
    let pos = &back.get::<primase_core::Mesh>(shared_e).unwrap().primitives()[0].streams
        [&primase_core::Semantic::Position];
    let primase_core::Stream::F32x3(pos) = pos else {
        panic!("{pos:?}")
    };
    assert_eq!(
        Some(pos[0]),
        match &w.get::<primase_core::Mesh>(me).unwrap().primitives()[0].streams
            [&primase_core::Semantic::Position]
        {
            primase_core::Stream::F32x3(p) => p.first().copied(),
            _ => None,
        }
    );
}
