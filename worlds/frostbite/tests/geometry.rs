//! MeshSet geometry: a hand-laid section decoded to its streams, its raw elements and its triangles, the refusals; and,
//! on the game's data (the skate. install, its Oodle 9 library and ReSkate Studio's `reskate_cli`), the meshes of
//! `Win32/items.toc` and `Win32/customization.toc` read, each rigid mesh's level 0 compared with the reference reader
//! (`reskate_cli meshes`: its vertex and triangle counts) and its positions with the bounding box the MeshSet header
//! stores (0x10 minimum, 0x20 maximum), and every mesh of the two TOCs decoded without a refusal.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use frostbite_formats::MESHSET;
use frostbite_formats::bundle::Kind;
use frostbite_formats::folder::{Folder, Source};
use frostbite_formats::meshset::{self, Decl, Element, Lod, Section, format, usage};
use half::f16;
use primase_core::{Lods, Mesh, Semantic, Stream, World};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_frostbite::components::{FrostbiteMeshSet, FrostbiteVertexElements};
use primase_frostbite::geometry;
use primase_frostbite::read::entry::read_entry;
use primase_frostbite::read::geometry::read_geometry;
use primase_testdata::{Game, Tool, install, install_file, tool};

fn half(v: f32) -> [u8; 2] {
    f16::from_f32(v).to_bits().to_le_bytes()
}

/// A declaration of the given elements over streams of the given strides.
fn decl(elements: &[(u8, u8, u8)], strides: &[u8]) -> Decl {
    Decl {
        elements: elements
            .iter()
            .map(|&(usage, format, stream)| Element {
                usage,
                format,
                offset: 0,
                stream,
            })
            .collect(),
        strides: strides.to_vec(),
        bytes: [0; meshset::DECL_SIZE],
    }
}

/// A one-section LOD of 3 vertices and a triangle, 16-bit indices: Half4 positions, Half2 texture coordinates, UByte4N
/// colour, blend indices (four u16, one over 255) and weights, and a tangent space (usage 52) left raw. The chunk holds
/// the streams then the index buffer.
fn fixture() -> (Vec<u8>, Lod, Section) {
    let mut chunk = Vec::new();
    for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.5]] {
        for c in p {
            chunk.extend(half(c));
        }
        chunk.extend(half(1.0));
    }
    for uv in [[0.0f32, 0.0], [1.0, 0.0], [0.0, 0.5]] {
        chunk.extend(half(uv[0]));
        chunk.extend(half(uv[1]));
    }
    for c in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 128]] {
        chunk.extend(c);
    }
    for t in [[1u8, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]] {
        chunk.extend(t);
    }
    for j in [[101u16, 101, 101, 107], [0, 1, 2, 3], [300, 0, 0, 0]] {
        for lane in j {
            chunk.extend(lane.to_le_bytes());
        }
    }
    for w in [[0u8, 0, 0, 255], [10, 20, 30, 195], [255, 0, 0, 0]] {
        chunk.extend(w);
    }
    // 24 + 12 + 12 + 12 + 24 + 12 = 96 vertex bytes; then the three indices
    chunk.resize(96, 0);
    for i in [0u16, 1, 2] {
        chunk.extend(i.to_le_bytes());
    }
    let section = Section {
        at: 0,
        key: 0xABCD,
        decl: decl(
            &[
                (usage::POSITION, format::HALF4, 0),
                (usage::TEXCOORD0, format::HALF2, 1),
                (usage::COLOR0, format::UBYTE4N, 2),
                (usage::TANGENT_SPACE, format::UBYTE4N, 3),
                (usage::BLEND_INDICES, format::USHORT4, 4),
                (usage::BLEND_WEIGHTS, format::UBYTE4N, 5),
            ],
            &[8, 4, 4, 4, 8, 4],
        ),
        draw_decl: decl(&[], &[]),
        flags: 0xe,
        stride: 20,
        primitives: 1,
        start_index: 0,
        vertex_offset: 0,
        vertices: 3,
        ..Section::default()
    };
    let lod = Lod {
        at: 0,
        chunk: [0; 16],
        index_size: 2,
        index_bytes: 6,
        vertex_bytes: 96,
        sections: vec![section.clone()],
        ..Lod::default()
    };
    (chunk, lod, section)
}

#[test]
fn a_section_decodes_to_streams_raw_elements_and_triangles() {
    let (chunk, lod, section) = fixture();
    let d = geometry::decode(&chunk, &lod, &section).unwrap();
    assert_eq!(
        d.streams[&Semantic::Position],
        Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.5]])
    );
    assert_eq!(
        d.streams[&Semantic::TexCoord(0)],
        Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 0.5]])
    );
    assert_eq!(
        d.streams[&Semantic::Color(0)],
        Stream::U8x4(vec![[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 128]])
    );
    assert_eq!(d.indices, [0, 1, 2]);
    assert_eq!(d.raw.len(), 3);
    assert_eq!((d.raw[0].usage, d.raw[0].size), (usage::TANGENT_SPACE, 4));
    // the blend indices and weights stay raw, and read as four little-endian u16 and four bytes a vertex
    assert!(!d.streams.contains_key(&Semantic::Joints(0)) && !d.streams.contains_key(&Semantic::Weights(0)));
    assert_eq!(
        d.raw[1].joints().unwrap(),
        [[101, 101, 101, 107], [0, 1, 2, 3], [300, 0, 0, 0]]
    );
    assert_eq!(d.raw[1].weights(), None);
    assert_eq!(
        d.raw[2].weights().unwrap(),
        [[0, 0, 0, 255], [10, 20, 30, 195], [255, 0, 0, 0]]
    );
    assert_eq!(d.raw[2].joints(), None);
    assert_eq!(d.raw[0].joints(), None);
    assert_eq!(d.raw[0].data, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    // the raw data follows the vertices a pass reorders
    assert_eq!(
        d.raw[0].gather(&[2, 2, 0]).unwrap().data,
        [9, 10, 11, 12, 9, 10, 11, 12, 1, 2, 3, 4]
    );
    assert!(d.raw[0].gather(&[3]).is_err());
}

#[test]
fn a_section_that_does_not_fit_is_refused() {
    let (chunk, lod, section) = fixture();
    let err = |chunk: &[u8], lod: &Lod, section: &Section| {
        geometry::decode(chunk, lod, section).unwrap_err().to_string()
    };
    // a position's w lane is 1
    let mut bad = chunk.clone();
    bad[6..8].copy_from_slice(&half(0.5));
    assert!(err(&bad, &lod, &section).contains("position w"));
    // an index outside the section's vertices
    let mut bad = chunk.clone();
    bad[96 + 4..96 + 6].copy_from_slice(&3u16.to_le_bytes());
    assert!(err(&bad, &lod, &section).contains("index 2 is vertex 3 of 3"));
    // a chunk without the whole vertex stream, without the indices
    assert!(err(&chunk[..20], &lod, &section).contains("a vertex stream"));
    assert!(err(&chunk[..100], &lod, &section).contains("the index buffer"));
    // a usage stored in a format that is not one the decoder reads
    let mut odd = section.clone();
    odd.decl = decl(&[(usage::POSITION, format::HALF2, 0)], &[4]);
    assert!(err(&chunk, &lod, &odd).contains("usage 1 stored with format"));
    // no position at all
    let mut none = section;
    none.decl = decl(&[(usage::TEXCOORD0, format::HALF2, 0)], &[4]);
    assert!(err(&chunk, &lod, &none).contains("no position"));
}

struct Reference {
    vertices: usize,
    triangles: usize,
}

/// `reskate_cli meshes <game root>`: one line a decoded mesh, `<name> = <kind> LOD 0 (<n> vertices, <m> triangles)`.
fn reference(cli: &std::path::Path, root: &std::path::Path) -> BTreeMap<String, Reference> {
    let out = std::process::Command::new(cli)
        .arg("meshes")
        .arg(root)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mut meshes = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((name, what)) = line.split_once(" = ") else {
            continue;
        };
        let Some(inner) = what.split_once('(').and_then(|(_, r)| r.strip_suffix(')')) else {
            continue;
        };
        let mut numbers = inner
            .split(", ")
            .map(|p| p.split(' ').next().unwrap().parse::<usize>());
        if let (Some(Ok(vertices)), Some(Ok(triangles))) = (numbers.next(), numbers.next()) {
            meshes.insert(name.to_owned(), Reference { vertices, triangles });
        }
    }
    meshes
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[test]
fn the_stock_meshes_decode_and_agree_with_the_reference() {
    let (Some(data), Some(root), Some(dll), Some(cli)) = (
        install_file(Game::Skate, "Data"),
        install(Game::Skate),
        tool(Tool::Oodle9),
        tool(Tool::ReskateCli),
    ) else {
        return;
    };
    let folder = Folder::open(data).unwrap();
    // SAFETY: the configured Oodle 9 library is the game's own.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }.unwrap();
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let reference = reference(&cli, &root);
    assert!(!reference.is_empty(), "the reference reader decoded no mesh");
    let (mut decoded, mut compared, mut counts_equal, mut bounded, mut inside) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut worst = 0.0f32;
    let mut with_lods = 0usize;
    for toc in ["Win32/items.toc", "Win32/customization.toc"] {
        let listing = src.listing(toc).unwrap();
        for bundle in &listing.bundles {
            for entry in bundle
                .entries()
                .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
            {
                let mut w = World::new();
                let made = read_entry(&src, bundle, &entry, &mut w).unwrap();
                let id = made[0];
                // a mesh whose chunks the bundle and its TOC hold; the others are counted
                let forms =
                    read_geometry(&src, bundle, id, &mut w).unwrap_or_else(|e| panic!("{}: {e}", entry.name));
                decoded += 1;
                with_lods += usize::from(w.get::<Lods>(id).is_some());
                for &f in &forms {
                    let mesh = w.get::<Mesh>(f).unwrap();
                    let elements = w.get::<FrostbiteVertexElements>(f).unwrap();
                    assert_eq!(mesh.primitives().len(), elements.primitives.len());
                }
                let Some(r) = reference.get(&entry.name) else {
                    continue;
                };
                compared += 1;
                let mesh = w.get::<Mesh>(id).unwrap();
                let (vertices, triangles) = (mesh.primitives().iter()).fold((0, 0), |(v, t), p| {
                    (
                        v + p.vertex_count(),
                        t + p.indices.as_ref().map_or(0, Vec::len) / 3,
                    )
                });
                if (vertices, triangles) == (r.vertices, r.triangles) {
                    counts_equal += 1;
                } else {
                    println!(
                        "{}: {vertices} vertices {triangles} triangles, the reference {} and {}",
                        entry.name, r.vertices, r.triangles
                    );
                }
                // the MeshSet header's bounds against the decoded positions of level 0
                let payload = src.payload(entry.file, entry.size as usize).unwrap();
                let (lo, hi) = (
                    [0x10, 0x14, 0x18].map(|o| f32_at(&payload, o)),
                    [0x20, 0x24, 0x28].map(|o| f32_at(&payload, o)),
                );
                let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in mesh.primitives() {
                    for v in p.positions().unwrap() {
                        for k in 0..3 {
                            min[k] = min[k].min(v[k]);
                            max[k] = max[k].max(v[k]);
                        }
                    }
                }
                let off = (0..3)
                    .map(|k| (min[k] - lo[k]).abs().max((max[k] - hi[k]).abs()))
                    .fold(0.0f32, f32::max);
                worst = worst.max(off);
                bounded += usize::from(off <= 0.01);
                // the stored box holds every position, within the half precision
                inside += usize::from((0..3).all(|k| min[k] >= lo[k] - 0.001 && max[k] <= hi[k] + 0.001));
                let set = w.get::<FrostbiteMeshSet>(id).unwrap();
                assert!(!set.meshset.lods.is_empty());
            }
        }
    }
    println!(
        "{decoded} MeshSets decoded ({with_lods} with several levels); {compared} compared with the reference reader: counts equal in {counts_equal}; level 0 positions within 0.01 of the header's bounds in {bounded} (worst {worst}), inside the stored box in {inside}"
    );
    assert!(compared >= 5, "{compared} meshes compared");
    assert_eq!(
        counts_equal, compared,
        "the vertex and triangle counts differ from the reference reader's"
    );
    assert_eq!(
        bounded, compared,
        "level 0 positions are further than 0.01 from the MeshSet header's bounds"
    );
}
