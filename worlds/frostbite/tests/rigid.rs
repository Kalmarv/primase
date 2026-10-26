//! Gates of the rigid mesh builder (`primase_frostbite::rigid`) and the tangent frame codec (`tangent`).
//!
//! - Synthetic: frames encode and decode back within the codec's steps (normal 0.2 degrees, tangent 0.4); a frame on
//!   an axis, a tangent along the normal, both signs.
//! - `the_games_one_lod_quick_drop_meshes_rebuild` (`installs.skate/Data`, `tools.oodle9`): each one-LOD rigid
//!   MeshSet of `Win32/buildkititems.toc` with the builder's declaration (60 of 73; the others add a sub-material
//!   stream) rebuilt from its decoded geometry (positions, UV0, the tangent frames
//!   decoded, triangles) with its own name, material and chunk GUID: the geometry chunk is byte-identical but for
//!   tangent frames, which decode to the stored frames (a frame on the edge of an angle step or between two normal
//!   axes has two encodings); the written MeshSet and its metadata are byte-identical once the
//!   measured fields are set aside (bounds, surface area, UV density), and bounds and area agree within the
//!   half-float precision of the positions and 1%.
//! - `measures_agree_with_the_games_quick_drop_sections`: the measures of every rigid Quick Drop section (1,400 of
//!   them, all LODs) against the stored ones: bounds, area (1st..99th percentile within 1%) and UV density (25th..75th
//!   within 1%; the `default_mat` sections, whose UVs fold many triangles to nothing, fall outside).

#![allow(clippy::unwrap_used)]

use primase_compress::oodle::{Oodle, Version};
use primase_core::{Semantic, Stream};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::{MESHSET, geometry, meshset, rigid, tangent};
use primase_testdata::{Game, Tool, install_file, tool};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn angle(a: [f32; 3], b: [f32; 3]) -> f64 {
    let d: f64 = (0..3).map(|k| f64::from(a[k]) * f64::from(b[k])).sum();
    let l = |v: [f32; 3]| (0..3).map(|k| f64::from(v[k]).powi(2)).sum::<f64>().sqrt();
    (d / (l(a) * l(b))).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn frames_round_trip_within_their_steps() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let (mut worst_n, mut worst_t) = (0.0f64, 0.0f64);
    for i in 0..20_000 {
        let n = [next(), next(), next()].map(|v| v as f32);
        let r = [next(), next(), next()].map(|v| v as f32);
        if angle(n, r) < 1.0 || angle(n, r) > 179.0 {
            continue;
        }
        // the tangent: r made perpendicular to n
        let nn: f32 = n.iter().map(|v| v * v).sum::<f32>().sqrt();
        let u = n.map(|v| v / nn);
        let d: f32 = (0..3).map(|k| u[k] * r[k]).sum();
        let t = [0, 1, 2].map(|k| r[k] - u[k] * d);
        let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
        let f = tangent::decode(tangent::encode(n, t, sign).unwrap());
        worst_n = worst_n.max(angle(f.normal, n));
        worst_t = worst_t.max(angle(f.tangent, t));
        assert_eq!(f.sign, sign);
    }
    assert!(
        worst_n < 0.2 && worst_t < 0.4,
        "worst normal {worst_n} deg, tangent {worst_t} deg"
    );
    // on an axis, every axis and direction
    for k in 0..3 {
        for s in [1.0f32, -1.0] {
            let mut n = [0.0f32; 3];
            n[k] = s;
            let mut t = [0.0f32; 3];
            t[(k + 1) % 3] = 1.0;
            let f = tangent::decode(tangent::encode(n, t, 1.0).unwrap());
            assert!(
                angle(f.normal, n) < 0.01 && angle(f.tangent, t) < 0.2,
                "{n:?} {t:?}: {f:?}"
            );
        }
    }
    // a tangent along the normal has no direction to keep: angle 0; a normal without length is refused
    assert!(tangent::encode([0.0, 1.0, 0.0], [0.0, 2.0, 0.0], 1.0).is_some());
    assert!(tangent::encode([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0).is_none());
}

fn oodle() -> Option<Oodle> {
    let p = tool(Tool::Oodle9)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let o = unsafe { Oodle::load(p, Version::V9) };
    Some(o.expect("tools.oodle9 loads"))
}

/// The measured fields set aside: header bounds, and each section's UV densities, area and bounds.
fn without_measures(mut m: meshset::MeshSet) -> meshset::MeshSet {
    m.header[0x10..0x30].fill(0);
    for s in m.lods.iter_mut().flat_map(|l| &mut l.sections) {
        s.record[0x58..0x60].fill(0);
        s.record[0x14c..0x170].fill(0);
    }
    m
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

#[test]
fn the_games_one_lod_quick_drop_meshes_rebuild() -> R {
    let (Some(data), Some(oodle)) = (install_file(Game::Skate, "Data"), oodle()) else {
        return Ok(());
    };
    let folder = Folder::open(&data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let mut seen = std::collections::BTreeSet::new();
    let (mut meshes, mut other, mut densities, mut frames, mut same) =
        (0usize, 0usize, Vec::new(), 0usize, 0usize);
    for b in &src.listing("Win32/buildkititems.toc")?.bundles {
        for (i, a) in b.manifest.res.iter().enumerate() {
            let e = b.resource(i);
            if a.rtype != MESHSET || !seen.insert(e.name.clone()) {
                continue;
            }
            let stored = meshset::read_resource(&src.payload(e.file, e.size as usize)?, &a.rmeta)?;
            if stored.header.len() != meshset::HEADER || stored.lods.len() != 1 {
                continue;
            }
            let lod = &stored.lods[0];
            // those with the sub-material stream (usage 51) are not what the builder writes
            if lod.sections.iter().any(|s| s.decl != rigid::declaration()) {
                other += 1;
                continue;
            }
            let (file, size) = b.chunk(&src, &lod.chunk)?;
            let chunk = src.payload(file, size)?;
            let mut parts = Vec::new();
            for s in &lod.sections {
                let d = geometry::decode(&chunk, lod, s)?;
                let (Some(Stream::F32x3(p)), Some(Stream::F32x2(uv))) = (
                    d.streams.get(&Semantic::Position),
                    d.streams.get(&Semantic::TexCoord(0)),
                ) else {
                    panic!("{}: no positions or UV0", e.name);
                };
                let frames: Vec<tangent::Frame> = d
                    .raw
                    .iter()
                    .find(|r| r.usage == meshset::usage::TANGENT_SPACE)
                    .unwrap()
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&q| tangent::decode(q))
                    .collect();
                parts.push(rigid::Part {
                    material: s.name.clone(),
                    key: s.key,
                    slot: s.record[0x1c],
                    positions: p.clone(),
                    normals: frames.iter().map(|f| f.normal).collect(),
                    tangents: frames
                        .iter()
                        .map(|f| [f.tangent[0], f.tangent[1], f.tangent[2], f.sign])
                        .collect(),
                    uv: uv.clone(),
                    triangles: d.indices.as_chunks::<3>().0.to_vec(),
                });
            }
            let built = rigid::build(&stored.name, lod.chunk, &parts)?;
            // the chunk: every byte the same but the tangent frames, which decode to the same frames
            assert_eq!(built.chunk.len(), chunk.len(), "{}: chunk length", e.name);
            let mut frame_bytes = vec![false; chunk.len()];
            for s in &built.meshset.lods[0].sections {
                let at = s.stream_offset(2);
                frame_bytes[at..at + 4 * s.vertices as usize].fill(true);
            }
            for (k, (x, y)) in built.chunk.iter().zip(&chunk).enumerate() {
                assert!(
                    frame_bytes[k] || x == y,
                    "{}: chunk byte {k}: {x} built, {y} stored",
                    e.name
                );
            }
            for s in &built.meshset.lods[0].sections {
                let at = s.stream_offset(2);
                for v in 0..s.vertices as usize {
                    let (x, y) = (
                        &built.chunk[at + 4 * v..at + 4 * v + 4],
                        &chunk[at + 4 * v..at + 4 * v + 4],
                    );
                    frames += 1;
                    if x == y {
                        same += 1;
                        continue;
                    }
                    let (a, b) = (tangent::decode(x.try_into()?), tangent::decode(y.try_into()?));
                    assert!(
                        angle(a.normal, b.normal) < 0.3
                            && angle(a.tangent, b.tangent) < 0.5
                            && a.sign == b.sign,
                        "{}: vertex {v}: frame {x:?} built, {y:?} stored: {a:?} / {b:?}",
                        e.name
                    );
                }
            }
            // every record field but the measured ones
            let (w, wm) = meshset::write(&without_measures(built.meshset.clone()))?;
            let (s, sm) = meshset::write(&without_measures(stored.clone()))?;
            if w != s {
                let at = w.iter().zip(&s).position(|(x, y)| x != y);
                panic!("{}: records differ at {:x?}", e.name, at);
            }
            assert_eq!(wm, sm, "{}", e.name);
            // the measured fields
            let near = |a: f32, b: f32| (a - b).abs() <= 2e-3 * b.abs().max(1.0);
            for k in (0..3).chain(4..7) {
                let (x, y) = (
                    f32_at(&built.meshset.header, 0x10 + 4 * k),
                    f32_at(&stored.header, 0x10 + 4 * k),
                );
                assert!(near(x, y), "{}: bound {k}: {x} built, {y} stored", e.name);
            }
            for (bs, ss) in built.meshset.lods[0].sections.iter().zip(&lod.sections) {
                for k in (0..3).chain(4..7) {
                    let (x, y) = (
                        f32_at(&bs.record, 0x150 + 4 * k),
                        f32_at(&ss.record, 0x150 + 4 * k),
                    );
                    assert!(near(x, y), "{}: section bound {k}: {x} built, {y} stored", e.name);
                }
                let (x, y) = (f32_at(&bs.record, 0x14c), f32_at(&ss.record, 0x14c));
                assert!(
                    y < 1e-4 || (x / y - 1.0).abs() < 0.01,
                    "{}: area {x} built, {y} stored",
                    e.name
                );
                assert_eq!(
                    f32_at(&bs.record, 0x5c),
                    f32_at(&ss.record, 0x5c),
                    "{}: UV1 density",
                    e.name
                );
                densities.push(f32_at(&bs.record, 0x58) / f32_at(&ss.record, 0x58));
            }
            meshes += 1;
        }
    }
    densities.sort_by(f32::total_cmp);
    let q = |p: f64| densities[((densities.len() - 1) as f64 * p) as usize];
    eprintln!(
        "{meshes} one-LOD Quick Drop meshes rebuilt ({other} with another declaration); {same} of {frames} tangent frames byte-identical; UV density built / stored: 5% {} 25% {} 50% {} 75% {} 95% {}",
        q(0.05),
        q(0.25),
        q(0.5),
        q(0.75),
        q(0.95)
    );
    assert!(meshes >= 60, "{meshes}");
    Ok(())
}

/// The quantiles `ps` of `v`, sorted.
fn quantiles(v: &mut [f32], ps: &[f64]) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    ps.iter()
        .map(|&p| v[((v.len() - 1) as f64 * p) as usize])
        .collect()
}

/// [`rigid::measure`] of every rigid Quick Drop section (all LODs, every declaration) against what the game stores:
/// the area within 1% from the 1st to the 99th percentile, the UV density within 1% from the 25th to the 75th, the
/// bounds within the half-float precision of the decoded positions.
#[test]
fn measures_agree_with_the_games_quick_drop_sections() -> R {
    let (Some(data), Some(oodle)) = (install_file(Game::Skate, "Data"), oodle()) else {
        return Ok(());
    };
    let folder = Folder::open(&data)?;
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let mut seen = std::collections::BTreeSet::new();
    let (mut areas, mut densities) = (Vec::new(), Vec::new());
    for b in &src.listing("Win32/buildkititems.toc")?.bundles {
        for (i, a) in b.manifest.res.iter().enumerate() {
            let e = b.resource(i);
            if a.rtype != MESHSET || !seen.insert(e.name.clone()) {
                continue;
            }
            let m = meshset::read(&src.payload(e.file, e.size as usize)?)?;
            if m.header.len() != meshset::HEADER {
                continue;
            }
            for lod in &m.lods {
                let (file, size) = b.chunk(&src, &lod.chunk)?;
                let chunk = src.payload(file, size)?;
                for s in &lod.sections {
                    let d = geometry::decode(&chunk, lod, s)?;
                    let (Some(Stream::F32x3(p)), Some(Stream::F32x2(uv))) = (
                        d.streams.get(&Semantic::Position),
                        d.streams.get(&Semantic::TexCoord(0)),
                    ) else {
                        continue;
                    };
                    let part = rigid::Part {
                        positions: p.clone(),
                        uv: uv.clone(),
                        triangles: d.indices.as_chunks::<3>().0.to_vec(),
                        ..rigid::Part::default()
                    };
                    let got = rigid::measure(&part);
                    let near = |a: f32, b: f32| (a - b).abs() <= 2e-3 * b.abs().max(1.0);
                    for k in 0..3 {
                        assert!(
                            near(got.min[k], f32_at(&s.record, 0x150 + 4 * k)),
                            "{}: minimum {k}",
                            e.name
                        );
                        assert!(
                            near(got.max[k], f32_at(&s.record, 0x160 + 4 * k)),
                            "{}: maximum {k}",
                            e.name
                        );
                    }
                    let stored_area = f32_at(&s.record, 0x14c);
                    if stored_area > 1e-4 {
                        areas.push(got.area / stored_area);
                    }
                    densities.push(got.uv_density / f32_at(&s.record, 0x58));
                }
            }
        }
    }
    let a = quantiles(&mut areas, &[0.01, 0.5, 0.99]);
    let d = quantiles(&mut densities, &[0.05, 0.25, 0.5, 0.75, 0.95]);
    eprintln!(
        "{} sections: area measured / stored 1% {} 50% {} 99% {}; UV density 5% {} 25% {} 50% {} 75% {} 95% {}",
        densities.len(),
        a[0],
        a[1],
        a[2],
        d[0],
        d[1],
        d[2],
        d[3],
        d[4]
    );
    assert!(densities.len() > 1000);
    assert!(a.iter().all(|r| (r - 1.0).abs() < 0.01), "{a:?}");
    assert!(d[1..4].iter().all(|r| (r - 1.0).abs() < 0.01), "{d:?}");
    Ok(())
}
