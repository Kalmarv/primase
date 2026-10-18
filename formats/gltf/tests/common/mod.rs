//! Synthetic worlds and fixtures for the gltf tests: everything generated here, no game data.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, Name, Opaque, OpaqueData, Primitive, Semantic, Source, Stream,
    TextureFile, Tile, Topology, Transform, World,
};
use serde_json::json;

/// A fresh, empty scratch directory for one test.
pub fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("primase-gltf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A minimal valid RGBA8 PNG (stored deflate blocks), `w` x `h`, pixels from `f(x, y)`.
pub fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.extend_from_slice(&f(x, y));
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &c in &raw {
        a = (a + u32::from(c)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    for (kind, data) in [(b"IHDR", ihdr), (b"IDAT", z), (b"IEND", Vec::new())] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(&data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    out
}

fn crc32(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &x in b {
        c ^= u32::from(x);
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

pub fn t(cols: [[f32; 3]; 4]) -> Transform {
    Transform { cols }
}

/// A box-ish prototype primitive with every stream kind the writer takes (Joints optional).
pub fn rich_primitive(material: Option<Entity>, offset: f32) -> Primitive {
    let pos = vec![
        [0.0 + offset, 0.0, 0.0],
        [1.0 + offset, 0.0, 0.0],
        [1.0 + offset, 1.0, 0.1],
        [0.0 + offset, 1.0, -0.25],
    ];
    let mut streams = BTreeMap::new();
    streams.insert(Semantic::Position, Stream::F32x3(pos));
    streams.insert(
        Semantic::Normal,
        Stream::F32x3(vec![
            [0.0, 0.0, 1.0],
            [0.0, 0.6, 0.8],
            [0.6, 0.0, 0.8],
            [0.0, 0.0, 1.0],
        ]),
    );
    streams.insert(
        Semantic::Tangent,
        Stream::F32x4(vec![
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, -1.0],
            [0.8, 0.0, -0.6, 1.0],
            [1.0, 0.0, 0.0, 1.0],
        ]),
    );
    streams.insert(
        Semantic::TexCoord(0),
        Stream::F32x2(vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.1, 0.0]]),
    );
    streams.insert(
        Semantic::TexCoord(1),
        Stream::F32x2(vec![[0.5, 0.5], [0.25, 0.75], [-0.0, 3.5], [1e-30, 7.0]]),
    );
    streams.insert(
        Semantic::Color(0),
        Stream::U8x4(vec![
            [255, 0, 0, 255],
            [0, 255, 0, 128],
            [0, 0, 255, 0],
            [1, 2, 3, 4],
        ]),
    );
    streams.insert(
        Semantic::Color(1),
        Stream::F32x3(vec![
            [0.1, 0.2, 0.3],
            [0.4, 0.5, 0.6],
            [0.7, 0.8, 0.9],
            [1.0, 1.0, 1.0],
        ]),
    );
    Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(vec![0, 1, 2, 0, 2, 3]),
        material,
    }
}

/// Everything the test world contains, by role.
#[derive(Debug)]
pub struct Built {
    pub world: World,
    pub dir: PathBuf,
    pub mat_pbr: Entity,
    pub mat_mask: Entity,
    pub mat_plain: Entity,
    pub mat_variant: Entity,
    pub tex_base: Entity,
    pub tex_normal: Entity,
    pub tex_dds: Entity,
    pub proto_rich: Entity,
    pub proto_big: Entity,
    pub proto_lines: Entity,
    pub proto_unplaced: Entity,
    pub proto_empty: Entity,
    pub soup: Entity,
    pub regular: Entity,
    pub mirrored: Entity,
    pub sheared: Entity,
    pub sheared_mirrored: Entity,
    pub degenerate: Entity,
    pub variant1: Entity,
    pub no_mesh: Entity,
    /// The transforms the mirrored and sheared placements were authored with: `build` runs `bake-nonrigid`, as every
    /// pipeline does before a writer, so the world holds their rigid remainders and baked geometry.
    pub authored: std::collections::BTreeMap<Entity, Transform>,
}

/// The synthetic world: PNG and DDS texture files under `dir`, three kinds of material, four prototypes and
/// placements of every transform kind, the mirrored and sheared ones baked by `bake-nonrigid`.
pub fn build(dir: &Path) -> Built {
    std::fs::create_dir_all(dir.join("textures")).unwrap();
    std::fs::write(
        dir.join("textures/base.png"),
        png(4, 2, |x, y| [x as u8 * 60, y as u8 * 100, 7, 255]),
    )
    .unwrap();
    std::fs::write(
        dir.join("textures/normal.png"),
        png(2, 2, |_, _| [128, 128, 255, 255]),
    )
    .unwrap();
    // Same bytes as base.png under another name: embedded once.
    std::fs::copy(dir.join("textures/base.png"), dir.join("textures/base_copy.png")).unwrap();
    std::fs::write(dir.join("textures/skipped.dds"), b"DDS not really").unwrap();

    let mut w = World::new();
    let tex = |w: &mut World, name: &str, file: &str| {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(e, TextureFile { path: file.into() });
        w.insert(e, Source(format!("tex:{name}")));
        e
    };
    let tex_base = tex(&mut w, "base", "textures/base.png");
    let tex_normal = tex(&mut w, "normal", "textures/normal.png");
    let tex_copy = tex(&mut w, "base_copy", "textures/base_copy.png");
    let tex_dds = tex(&mut w, "dds", "textures/skipped.dds");

    let mat_pbr = w.spawn();
    w.insert(mat_pbr, Name("stone".into()));
    w.insert(
        mat_pbr,
        PbrMaterial {
            base_color: [0.9, 0.8, 0.7, 1.0],
            base_color_texture: Some(TextureRef::new(tex_base)),
            metallic: 0.0,
            roughness: 0.6,
            normal_texture: Some(TextureRef {
                texture: tex_normal,
                texcoord: 1,
                wrap: [Wrap::Clamp, Wrap::Mirror],
                transform: Some(UvTransform {
                    offset: [0.25, -0.5],
                    scale: [2.0, 4.0],
                    rotation: 0.0,
                }),
            }),
            normal_scale: 0.5,
            emissive: [0.1, 0.0, 0.0],
            double_sided: true,
            ..PbrMaterial::default()
        },
    );
    w.insert_opaque(
        mat_pbr,
        "skate.material",
        Opaque {
            version: 1,
            data: OpaqueData::Json(json!({"class": "opaque", "base": "stone_d", "tint": [200, 40, 40]})),
        },
    );
    let mat_mask = w.spawn();
    w.insert(mat_mask, Name("fence".into()));
    w.insert(
        mat_mask,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(tex_copy)),
            metallic_roughness_texture: Some(TextureRef::new(tex_dds)),
            alpha_mode: AlphaMode::Mask { cutoff: 0.33 },
            ..PbrMaterial::default()
        },
    );
    let mat_plain = w.spawn();
    w.insert(mat_plain, Name("unlowered".into()));
    w.insert_opaque(
        mat_plain,
        "fromsoft.layered_material",
        Opaque {
            version: 2,
            data: OpaqueData::Bytes(vec![1, 2, 3]),
        },
    );
    let mat_variant = w.spawn();
    w.insert(
        mat_variant,
        PbrMaterial {
            base_color: [1.0, 0.0, 0.0, 0.5],
            alpha_mode: AlphaMode::Blend,
            ..PbrMaterial::default()
        },
    );

    let proto_rich = w.spawn();
    w.insert(proto_rich, Name("rich".into()));
    w.insert(proto_rich, Source("msb:c1100/rich".into()));
    w.insert(
        proto_rich,
        Mesh::new(vec![
            rich_primitive(Some(mat_pbr), 0.0),
            rich_primitive(Some(mat_mask), 2.0),
            rich_primitive(None, 4.0),
        ]),
    );
    w.insert(
        proto_rich,
        MaterialVariants(vec![
            BTreeMap::new(),
            BTreeMap::from([(1, mat_variant)]),
            BTreeMap::new(),
        ]),
    );

    // More than 65535 vertices: unsigned int indices. Sequential (no indices) on a second primitive.
    let proto_big = w.spawn();
    w.insert(proto_big, Name("big".into()));
    let n = 70_002usize;
    let big_pos: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            [
                (i % 300) as f32 * 0.5,
                (i / 300) as f32 * 0.25,
                ((i * 7919) % 13) as f32 * 0.01,
            ]
        })
        .collect();
    let big_idx: Vec<u32> = (0..n as u32).rev().collect();
    w.insert(
        proto_big,
        Mesh::new(vec![
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(big_pos.clone()))]),
                indices: Some(big_idx),
                material: Some(mat_plain),
            },
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(big_pos[..9].to_vec()))]),
                indices: None,
                material: Some(mat_pbr),
            },
        ]),
    );

    let proto_lines = w.spawn();
    w.insert(proto_lines, Name("rail".into()));
    w.insert(
        proto_lines,
        Mesh::new(vec![Primitive {
            topology: Topology::Lines,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [0.0, 0.0, -3.0], [1.0, 2.0, 3.0]]),
                ),
                (Semantic::Color(0), Stream::U16x4(vec![[65535, 0, 0, 65535]; 3])),
                (Semantic::Joints(0), Stream::U8x4(vec![[0; 4]; 3])),
            ]),
            indices: Some(vec![0, 1, 1, 2]),
            material: None,
        }]),
    );

    let proto_unplaced = w.spawn();
    w.insert(proto_unplaced, Name("unused".into()));
    w.insert(
        proto_unplaced,
        Mesh::new(vec![rich_primitive(Some(mat_pbr), 0.0)]),
    );
    let proto_empty = w.spawn();
    w.insert(proto_empty, Name("no geometry".into()));

    // A world-space soup placed as itself.
    let soup = w.spawn();
    w.insert(soup, Name("terrain".into()));
    w.insert(
        soup,
        t([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [100., 0., -50.]]),
    );
    w.insert(soup, Mesh::new(vec![rich_primitive(Some(mat_plain), 0.0)]));

    let place = |w: &mut World, proto: Entity, variant: u32, tr: Transform, src: &str| {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant,
            },
        );
        w.insert(e, tr);
        w.insert(e, Source(src.into()));
        e
    };
    let regular = place(
        &mut w,
        proto_rich,
        0,
        t([[0., 0., -2.], [0., 2., 0.], [2., 0., 0.], [10., 0.5, -3.25]]),
        "plac:0",
    );
    w.insert(regular, Tile { col: 3, row: -1 });
    w.insert(regular, Name("rich placed".into()));
    let mirrored = place(
        &mut w,
        proto_rich,
        0,
        t([[-1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]]),
        "plac:1",
    );
    let sheared = place(
        &mut w,
        proto_rich,
        0,
        t([[1., 0., 0.], [0.5, 1., 0.], [0., 0.25, 1.], [5., 6., 7.]]),
        "plac:2",
    );
    let sheared_mirrored = place(
        &mut w,
        proto_rich,
        0,
        t([[1., 0., 0.], [0.5, 1., 0.], [0., 0., -1.], [0.; 3]]),
        "plac:3",
    );
    let degenerate = place(
        &mut w,
        proto_rich,
        0,
        t([[1., 0., 0.], [0., 1., 0.], [0., 0., 0.], [0.; 3]]),
        "plac:4",
    );
    let variant1 = place(&mut w, proto_rich, 1, Transform::IDENTITY, "plac:5");
    place(
        &mut w,
        proto_big,
        0,
        t([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [-0.0, 1.0, 2.0]]),
        "plac:6",
    );
    place(&mut w, proto_lines, 0, Transform::IDENTITY, "plac:7");
    let no_mesh = place(&mut w, proto_empty, 0, Transform::IDENTITY, "plac:8");
    w.insert_opaque_resource(
        "skate.scene",
        Opaque {
            version: 1,
            data: OpaqueData::Json(json!({"name": "test_scene", "region": [-10, -10, 10, 10]})),
        },
    );

    // lights: a spot placed and turned, an unplaced directional sun
    let spot = w.spawn();
    w.insert(spot, Name("lamp".into()));
    w.insert(
        spot,
        primase_core::Light {
            kind: primase_core::LightKind::Spot {
                inner_cone: 0.25,
                outer_cone: 0.5,
            },
            color: [1.0, 0.5, 0.25],
            intensity: 40.0,
            range: Some(12.0),
            cast_shadows: None,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[0] = [0.0, 0.0, -1.0];
    t.cols[2] = [1.0, 0.0, 0.0];
    t.cols[3] = [2.0, 3.0, 4.0];
    w.insert(spot, t);
    let sun = w.spawn();
    w.insert(sun, Name("sun".into()));
    w.insert(
        sun,
        primase_core::Light {
            kind: primase_core::LightKind::Directional,
            color: [1.0; 3],
            intensity: 2.0,
            range: Some(10.0),
            cast_shadows: None,
        },
    );

    let authored = [mirrored, sheared, sheared_mirrored]
        .map(|e| (e, *w.get::<Transform>(e).unwrap()))
        .into_iter()
        .collect();
    let log = primase_meshops::bake_nonrigid(&mut w, &primase_meshops::BakeNonrigid::default());
    assert_eq!(
        log.counts.get("mirrored transforms made rigid"),
        Some(&1),
        "{:?}",
        log.counts
    );
    assert_eq!(
        log.counts.get("sheared transforms made rigid"),
        Some(&2),
        "{:?}",
        log.counts
    );
    Built {
        world: w,
        dir: dir.to_owned(),
        mat_pbr,
        mat_mask,
        mat_plain,
        mat_variant,
        tex_base,
        tex_normal,
        tex_dds,
        proto_rich,
        proto_big,
        proto_lines,
        proto_unplaced,
        proto_empty,
        soup,
        regular,
        mirrored,
        sheared,
        sheared_mirrored,
        degenerate,
        variant1,
        no_mesh,
        authored,
    }
}
