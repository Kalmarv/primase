//! Real Meshes read into a world through a stated frame and lowered to core meshes, under the pass manager with its
//! checks on (Riders Republic build 25394987). Counts, splits and bounds as a separate read-only decode found them
//! (games/riders-republic docs/recon/mesh-second-fixture.md).
mod inputs;

use std::fs::File;
use std::path::Path;

use anvil_formats::archive::MAX_RECORD;
use anvil_formats::data::{Resource, read_entry};
use anvil_formats::forge::Forge;
use anvil_formats::mesh::{self, StoredMesh};
use inputs::{Game, install_file};
use primase_anvil::components::AnvilMesh;
use primase_anvil::passes::{LOWER_ANVIL_MESH, LowerAnvilMesh, Static16Normal};
use primase_anvil::read::meshes::{Error, read_meshes};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Options, Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Instance, Mesh, Semantic, Stream, World};

/// The entry's buffers as read in the diagnostic frame (the stored numbers unconverted, not a claim that they are
/// metres): the world, the prototype's buffers.
fn read(path: &Path, entry: usize) -> (World, AnvilMesh) {
    let mut w = World::new();
    let placed = read_meshes(path, &[entry], SourceFrame::WORLD, &mut w).expect("read");
    let prototype = w.get::<Instance>(placed[0]).expect("placed").prototype;
    let mesh = w.get::<AnvilMesh>(prototype).expect("buffers").clone();
    (w, mesh)
}

fn lower(w: &mut World) {
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![Box::new(LowerAnvilMesh {
            static16_normal: Static16Normal::P,
        })],
    };
    let mut report = RunReport::default();
    manager.run(w, &pipeline, &mut report).expect("lower");
}

/// The least and greatest of each coordinate.
fn extrema(positions: &[[f32; 3]]) -> [[f32; 3]; 2] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    [lo, hi]
}

/// A measured count, not a claim about the bytes' meaning: for how many triangles the sum of its vertices' bytes 8..11,
/// each read as `b / 127.5 - 1`, has a positive dot product with the triangle's winding normal (and for how many not).
/// It checks vertex indexing, not a normal reading: with the wrong indices of an earlier reader (c1f82990), Prada's
/// middle range measured mixed.
fn facing(m: &AnvilMesh) -> (usize, usize) {
    let stride = m.stride as usize;
    let n = |i: u32| {
        let b = &m.vertex_records[i as usize * stride + 8..];
        [b[0], b[1], b[2]].map(|b| f32::from(b) / 127.5 - 1.0)
    };
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let mut agree = (0, 0);
    for &[a, b, c] in &m.triangles {
        let p = |i: u32| m.positions[i as usize];
        let (u, v) = (sub(p(b), p(a)), sub(p(c), p(a)));
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let normal = [0, 1, 2].map(|i| n(a)[i] + n(b)[i] + n(c)[i]);
        if face.iter().zip(normal).map(|(f, n)| f * n).sum::<f32>() > 0.0 {
            agree.0 += 1;
        } else {
            agree.1 += 1;
        }
    }
    agree
}

#[test]
fn black_box_and_reversed_sphere_read_and_lower() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge") else {
        return;
    };
    for (entry, vertices, triangles) in [(161, 56, 46), (318, 559, 960)] {
        let (mut w, m) = read(&path, entry);
        assert_eq!(
            (m.entry as usize, m.positions.len(), m.triangles.len()),
            (entry, vertices, triangles)
        );
        assert_eq!(m.ranges, [0]);
        assert_eq!(m.materials.len(), 1);
        if entry == 161 {
            assert_eq!(m.materials, [2_008_283_988_509]);
        }
        ranges_tile(&m);
        let prototype = w.iter::<AnvilMesh>().map(|(e, _)| e).next().expect("one");
        lower(&mut w);
        assert!(!w.has::<AnvilMesh>(prototype));
        let p = &w.get::<Mesh>(prototype).expect("lowered").primitives()[0];
        assert_eq!(p.indices.as_ref().expect("indexed").len(), triangles * 3);
        let Some(Stream::F32x3(normals)) = p.streams.get(&Semantic::Normal) else {
            panic!("normals");
        };
        assert_eq!(normals.len(), vertices);
        let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(0)) else {
            panic!("texture coordinates");
        };
        assert_eq!(uv.len(), vertices);
        // the engine's declarations worked by hand on the first and last vertices (static24: the frame the
        // diagnostic one, so directions as stored): the normal (b - 127) / 127 at unit length, the tangent likewise
        // with the position word's sign, the colour bytes 16..19, the coordinates two snorm16 words times 16
        let Some(Stream::F32x4(tangents)) = p.streams.get(&Semantic::Tangent(0)) else {
            panic!("tangents");
        };
        let Some(Stream::U8x4(colours)) = p.streams.get(&Semantic::Color(0)) else {
            panic!("colours");
        };
        for i in [0, vertices - 1] {
            let r = &m.vertex_records[i * 24..(i + 1) * 24];
            let unit = |b: &[u8]| {
                let v = [b[0], b[1], b[2]].map(|c| (f32::from(c) - 127.0) / 127.0);
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                v.map(|c| c / len)
            };
            assert_eq!(normals[i], unit(&r[8..11]));
            let w = i16::from_le_bytes([r[6], r[7]]);
            let t = unit(&r[12..15]);
            assert_eq!(tangents[i], [t[0], t[1], t[2], if w < 0 { -1.0 } else { 1.0 }]);
            assert_eq!(colours[i], [r[16], r[17], r[18], r[19]]);
            let word =
                |at: usize| (f32::from(i16::from_le_bytes([r[at], r[at + 1]])) / 32767.0).max(-1.0) * 16.0;
            assert_eq!(uv[i], [word(20), word(22)]);
        }
    }
    // a Mesh entry is found by the record's type; the MeshShape a few entries on is refused
    let mut w = World::new();
    let not = (0..388)
        .map(|e| read_meshes(&path, &[e], SourceFrame::WORLD, &mut w))
        .find_map(|r| match r {
            Err(Error::NotAMesh { entry, .. }) => Some(entry),
            _ => None,
        });
    assert!(not.is_some(), "the base forge holds entries that aren't Meshes");
}

#[test]
fn stand_reads_its_two_ranges() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom_patch_01.forge") else {
        return;
    };
    let (mut w, m) = read(&path, 2);
    assert_eq!((m.positions.len(), m.triangles.len()), (1154, 864));
    // the second range (indices from 2016) counted from vertex 904
    assert_eq!(m.ranges, [0, 672]);
    assert_eq!(m.materials, [1_885_111_592_920, 1_797_334_957_893]);
    ranges_tile(&m);
    assert!(m.triangles[672..].iter().flatten().all(|&i| i >= 904));
    assert_eq!(facing(&m), (864, 0));
    lower(&mut w);
    let (_, mesh) = w.iter::<Mesh>().next().expect("lowered");
    let Some(Stream::F32x3(normals)) = mesh.primitives()[0].streams.get(&Semantic::Normal) else {
        panic!("normals")
    };
    assert!(
        normals
            .iter()
            .all(|n| ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) - 1.0).abs() < 1e-6)
    );
}

/// Every range's triangles name only its own vertices, from its base up to the next range's, and together they name
/// every vertex: observed on every fixture, not a format requirement (the reader reads unnamed vertices and empty
/// ranges).
fn ranges_tile(m: &AnvilMesh) {
    let mut used = vec![false; m.positions.len()];
    for t in m.triangles.iter().flatten() {
        used[*t as usize] = true;
    }
    assert!(used.iter().all(|&u| u), "a vertex no triangle names");
    let mut ends = m.ranges.iter().skip(1).map(|&r| r as usize).collect::<Vec<_>>();
    ends.push(m.triangles.len());
    let mut start = 0;
    let mut floor = 0;
    for end in ends {
        let range = &m.triangles[start..end];
        let lo = range.iter().flatten().min().copied().unwrap_or(floor);
        assert_eq!(lo, floor, "range from triangle {start} doesn't start at its base");
        floor = range.iter().flatten().max().map_or(floor, |&hi| hi + 1);
        start = end;
    }
}

#[test]
fn prada_barrier_reads_three_ranges() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT_patch_01.forge") else {
        return;
    };
    let (_, m) = read(&path, 136_194);
    assert_eq!((m.positions.len(), m.triangles.len()), (110, 60));
    // ranges from indices 0, 12 and 120, counted from vertices 0, 8 and 70; read with only the last record's base,
    // the middle range named the wrong vertices, and the bytes-8 sign agreement below was mixed on it
    assert_eq!(m.ranges, [0, 4, 40]);
    assert_eq!(
        m.materials,
        [1_885_111_687_947, 1_797_334_957_893, 2_022_154_151_195]
    );
    ranges_tile(&m);
    assert_eq!(facing(&m), (60, 0));
}

/// Large meshes of five and six ranges, and one of one: the regression for a reader that applied only the last
/// record's base (the bowls' middle ranges were drawn across the whole mesh).
#[test]
fn bowls_read_every_range() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    let mut w = World::new();
    let placed = read_meshes(&path, &[652_571, 644_520, 313_892], SourceFrame::WORLD, &mut w).expect("read");
    let fixtures = [
        (30_280, 35_229, vec![0, 6471, 29_915, 30_531, 31_835], (35_227, 2)),
        (
            25_347,
            30_704,
            vec![0, 7820, 26_083, 27_142, 30_226, 30_506],
            (30_694, 10),
        ),
        (11_897, 18_554, vec![0], (18_484, 70)),
    ];
    let bowl = w.get::<Instance>(placed[0]).expect("placed").prototype;
    // a witness from a separate decode: triangle 6490 (index 19470, in the range from 19413) stores 17265, 17264,
    // 19334, which named a 15-unit sliver until counted from that range's base 4863
    assert_eq!(
        w.get::<AnvilMesh>(bowl).expect("buffers").triangles[6490],
        [22_128, 22_127, 24_197]
    );
    let materials: [&[u64]; 3] = [
        &[
            2_151_033_353_301,
            2_151_033_348_977,
            2_151_033_446_414,
            2_151_033_349_440,
            2_151_033_348_398,
        ],
        &[
            1_973_853_693_640,
            2_022_154_269_566,
            2_022_154_269_555,
            2_022_154_274_721,
            2_022_154_273_024,
            2_022_154_269_303,
        ],
        &[1_957_570_587_744],
    ];
    for (p, materials) in placed.iter().zip(materials) {
        let m = w
            .get::<AnvilMesh>(w.get::<Instance>(*p).expect("placed").prototype)
            .expect("buffers");
        assert_eq!(m.materials, materials);
    }
    for (p, (vertices, triangles, ranges, faces)) in placed.into_iter().zip(fixtures) {
        let m = w
            .get::<AnvilMesh>(w.get::<Instance>(p).expect("placed").prototype)
            .expect("buffers");
        assert_eq!((m.positions.len(), m.triangles.len()), (vertices, triangles));
        assert_eq!(m.ranges, ranges);
        ranges_tile(m);
        // bytes 8..11 read as b / 127.5 - 1 agree in sign with the winding normal on these counts of triangles
        assert_eq!(facing(m), faces);
    }
}

#[test]
fn the_lowering_states_its_one_choice() {
    let o = |kv: &[(&str, &str)]| -> Options {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    let make = |kv: &[(&str, &str)]| LOWER_ANVIL_MESH.instance(&o(kv));
    for (word, static16_normal) in [("p", Static16Normal::P), ("omit", Static16Normal::Omit)] {
        assert_eq!(
            make(&[("static16-normal", word)]).unwrap().options(),
            LowerAnvilMesh { static16_normal }.options()
        );
    }
    assert!(make(&[]).is_err());
    assert!(make(&[("static16-normal", "q")]).is_err());
    // the readings the engine's declarations settled are no longer options
    assert!(make(&[("static16-normal", "p"), ("uv-divisor", "2048")]).is_err());
}

/// The entry's body parsed as stored.
fn stored(path: &Path, entry: usize) -> StoredMesh {
    let forge = Forge::open(path).expect("forge");
    let payload = forge
        .payload(&mut File::open(path).expect("open"), entry)
        .expect("payload");
    let block = read_entry(&payload, MAX_RECORD).expect("entry");
    let resource = Resource::parse(&block.resource.data).expect("resource");
    StoredMesh::parse(&resource.body).expect("mesh")
}

/// The decoded positions' extrema against the bounds block's stored bounds, on every fixture: within one quantization step
/// (measured: BlackBox 1.04e-4, ReversedSphere 1.14e-4, Stand 1.5e-5, PradaBarrier 9.7e-5). This is agreement of six
/// numbers per mesh, evidence for the position decode's scale and sign; it says nothing of any one vertex between them.
#[test]
fn decoded_extrema_meet_the_stored_bounds() {
    let base = || install_file(Game::Riders, "DataPC_MAD_WhiteRoom.forge");
    let patch = || install_file(Game::Riders, "DataPC_MAD_WhiteRoom_patch_01.forge");
    let usnat = || install_file(Game::Riders, "DataPC_MAD_USNAT.forge");
    let usnat_patch = || install_file(Game::Riders, "DataPC_MAD_USNAT_patch_01.forge");
    let fixtures = [
        (base(), 161),
        (base(), 318),
        (patch(), 2),
        (usnat_patch(), 136_194),
        (usnat(), 652_571),
        (usnat(), 644_520),
        (usnat(), 313_892),
        (usnat(), 651_945),
        (usnat(), 651_944),
    ];
    for (forge, entry) in fixtures {
        let Some(path) = forge else {
            continue;
        };
        let input = path.file_name().unwrap().to_string_lossy();
        let mesh = stored(&path, entry);
        let positions: Vec<[f32; 3]> = mesh.vertices.positions().collect();
        let found = extrema(&positions);
        let off = (0..2)
            .flat_map(|k| (0..3).map(move |i| (k, i)))
            .map(|(k, i)| (found[k][i] - mesh.bounds[k][i]).abs())
            .fold(0.0f32, f32::max);
        // one quantization step: the largest scale over 32768
        let step = mesh
            .vertices
            .ws()
            .map(|w| f32::from(w).abs() / 32768.0)
            .fold(0.0f32, f32::max);
        assert!(off <= step, "{input} {entry}: off by {off}, a step {step}");
    }
}

/// Meshes of the inline layout (`MeshData`'s CRC-32 at marker+13, 3 at marker+4; u32 indices): the Favela BigTurn B
/// and A LOD0s, counts as a separate read-only decode found them (games/riders-republic
/// docs/recon/layoutb-favela-observation.md, d099244). One range each, past 65535 vertices, its descriptor's count the
/// low 16 bits.
#[test]
fn inline_meshes_read_their_u32_indices() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    let mut w = World::new();
    let placed = read_meshes(&path, &[651_945, 651_944], SourceFrame::WORLD, &mut w).expect("read");
    for (p, (vertices, triangles)) in placed.into_iter().zip([(93_906, 64_211), (72_260, 46_142)]) {
        let m = w
            .get::<AnvilMesh>(w.get::<Instance>(p).expect("placed").prototype)
            .expect("buffers");
        assert_eq!((m.positions.len(), m.triangles.len()), (vertices, triangles));
        assert_eq!(m.ranges, [0]);
        assert_eq!(m.materials, [2_151_033_132_404]);
        assert!(m.triangles.iter().flatten().any(|&i| i > 65_535));
        assert!(m.positions.iter().flatten().all(|x| x.is_finite()));
        ranges_tile(m);
    }
    let stored = stored(&path, 651_945);
    assert!(matches!(stored.layout, mesh::Layout::Inline(_)));
    assert_eq!(stored.material_list.descriptors[0].vertex_count, 28_370);
}

/// The materials the bowls' tails name are Material resources of the same forge: each ClassID finds an entry whose
/// record's type (read from the record, not the name) is Material's.
#[test]
fn the_bowls_materials_resolve_to_materials() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_USNAT.forge") else {
        return;
    };
    let mut w = World::new();
    let placed = read_meshes(&path, &[652_571, 644_520], SourceFrame::WORLD, &mut w).expect("read");
    let forge = Forge::open(&path).expect("forge");
    let mut file = File::open(&path).expect("open");
    for p in placed {
        let m = w
            .get::<AnvilMesh>(w.get::<Instance>(p).expect("placed").prototype)
            .expect("buffers");
        for &id in &m.materials {
            let entry = forge.find(id).expect("in this forge");
            let payload = forge.payload(&mut file, entry).expect("payload");
            let block = read_entry(&payload, MAX_RECORD).expect("entry");
            let resource = Resource::parse(&block.resource.data).expect("resource");
            assert_eq!((resource.type_id, resource.class_id), (mesh::MATERIAL, id));
        }
    }
}
