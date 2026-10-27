//! Evidence from the game: independent prefetch copies, neighbour-slice coherence
//! against a deterministic pixel-shuffled control, and RGB LUT axis affinity.
//! These content properties check this disc; the layout itself comes from the
//! published NVIDIA VTC description and RSX consumer, plus hand-worked fixtures.
mod inputs;

use inputs::{Depth, Game, Query, depth, expect_found, install, seed};
use primase_core::{ColorSpace, TexRole};
use primase_shred::read::texture;
use primase_texcodec::{Decoder, TexDecoder, gcm::Volume};
use rme_formats::{
    pak::Pak,
    texture::{Payload, Residency, TextureCatalog},
};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("entry").path();
        if p.is_dir() {
            archives(&p, out);
        } else if p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pak")) {
            out.push(p);
        }
    }
}

fn planes(volume: &Volume) -> Vec<Vec<u8>> {
    let l = &volume.levels[0];
    let plane = volume.format.level_size(l.width, l.height);
    l.data
        .chunks_exact(plane)
        .map(|b| {
            Decoder::default()
                .decode_rgba8(volume.format, l.width, l.height, b)
                .expect("slice decode")
        })
        .collect()
}

// Permute XY correspondences in the next slice, keeping its exact histogram.
// This controls for a shared average colour being mistaken for Z coherence.
fn coherence(planes: &[Vec<u8>]) -> (u64, u64, u64) {
    let n = planes[0].len() / 4;
    let mut shuffled: Vec<usize> = (0..n).collect();
    let mut state = 0x1234_5678_9abc_def0u64;
    for i in (1..n).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        shuffled.swap(i, state as usize % (i + 1));
    }
    let (mut adjacent, mut control, mut samples) = (0u64, 0u64, 0u64);
    for pair in planes.windows(2) {
        for (i, &j) in shuffled.iter().enumerate() {
            for c in 0..3 {
                adjacent += u64::from(pair[0][4 * i + c].abs_diff(pair[1][4 * i + c]));
                control += u64::from(pair[0][4 * i + c].abs_diff(pair[1][4 * j + c]));
                samples += 1;
            }
        }
    }
    (adjacent, control, samples)
}

// A grading LUT may change the ramp's shape, but the disc's RGB cube should be
// nearer the canonical RGB axis assignment than any of the other five orders.
fn lut_errors(planes: &[Vec<u8>]) -> [u64; 6] {
    let permutations = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let mut errors = [0; 6];
    for (z, plane) in planes.iter().enumerate() {
        for y in 0..32 {
            for x in 0..32 {
                let coords = [x, y, z];
                let pixel = &plane[(y * 32 + x) * 4..][..4];
                for (i, p) in permutations.iter().enumerate() {
                    for c in 0..3 {
                        let ideal = ((coords[p[c]] * 255 + 15) / 31) as u8;
                        let difference = u64::from(pixel[c].abs_diff(ideal));
                        errors[i] += difference * difference;
                    }
                }
            }
        }
    }
    errors
}

#[test]
fn coherence_metric_distinguishes_a_known_gradient_from_shuffled_correspondences() {
    let planes: Vec<Vec<u8>> = (0..4)
        .map(|z| {
            (0..16)
                .flat_map(|i| [(i % 4) * 60, (i / 4) * 60, z * 5, 255])
                .collect()
        })
        .collect();
    let (adjacent, control, samples) = coherence(&planes);
    assert_eq!(adjacent, 16 * 3 * 5);
    assert_eq!(samples, 16 * 3 * 3);
    assert!(adjacent * 5 < control);
}

#[test]
fn every_volume_and_its_stored_copy_and_spatial_coherence() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    let depth = depth();
    let seed = seed();
    if depth == Depth::Smoke {
        let n = paths.len();
        paths.rotate_left(seed as usize % n);
    }
    let (mut count, mut argb, mut compressed, mut copies, mut unique_xyz) = (0, 0, 0, 0, 0);
    let mut scores = [[0u64; 3]; 2];
    let mut lut_totals = [0u64; 6];
    let (mut visited, mut volume_archives, mut smoother, mut tied) = (0, 0, 0, 0);
    for path in &paths {
        if depth == Depth::Smoke && volume_archives >= 8 {
            break;
        }
        visited += 1;
        let pak = Pak::open(path).unwrap();
        let label = path.to_str().unwrap();
        let c = TextureCatalog::new(&[(label, &pak)]).unwrap();
        if c.records().iter().any(|r| r.header.gcm.dimension == 3) {
            volume_archives += 1;
        }
        for (index, r) in c
            .records()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.header.gcm.dimension == 3)
        {
            let v = texture::decode_volume(&c, index, Payload::Primary, ColorSpace::Linear, TexRole::Color)
                .unwrap_or_else(|e| panic!("{label} {:?}: {e}", String::from_utf8_lossy(&r.path)));
            count += 1;
            assert_eq!(v.levels.len(), 1);
            assert_eq!(v.levels[0].depth, u32::from(r.header.gcm.depth));
            let p = planes(&v);
            assert_eq!(p.len(), r.header.gcm.depth as usize);
            let group = usize::from(r.header.gcm.format & !0x60 == 0x85);
            let (a, b, n) = coherence(&p);
            scores[group][0] += a;
            scores[group][1] += b;
            scores[group][2] += n;
            if a < b {
                smoother += 1;
            } else if a == b {
                tied += 1;
            } else {
                eprintln!(
                    "coherence outlier {label} {:?}:adjacent={a} control={b}",
                    String::from_utf8_lossy(&r.path)
                );
            }
            if group == 1 {
                argb += 1;
                assert_eq!(
                    (v.levels[0].width, v.levels[0].height, v.levels[0].depth),
                    (32, 32, 32)
                );
                let errors = lut_errors(&p);
                eprintln!(
                    "LUT {:?} axis-errors XYZ,XZY,YXZ,YZX,ZXY,ZYX={errors:?}",
                    String::from_utf8_lossy(&r.path)
                );
                if errors[1..].iter().all(|&e| errors[0] < e) {
                    unique_xyz += 1;
                }
                for (sum, error) in lut_totals.iter_mut().zip(errors) {
                    *sum += error;
                }
            } else {
                compressed += 1;
            }
            if r.residency == Residency::Prefetch {
                for (full_index, _) in c
                    .records()
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| other.path == r.path && other.residency == Residency::Streamed)
                {
                    let full = texture::decode_volume(
                        &c,
                        full_index,
                        Payload::Primary,
                        ColorSpace::Linear,
                        TexRole::Color,
                    )
                    .unwrap();
                    assert_eq!(v, full, "volume prefetch {label}");
                    copies += 1;
                }
            }
        }
    }
    expect_found(Query::Install(Game::Shred, None), "3D volume headers", count);
    eprintln!(
        "SHRED volumes {depth:?} seed={seed}:PAKs={visited} volume_PAKs={volume_archives} volumes={count} compressed={compressed} ARGB={argb} copies={copies} smoother={smoother} tied={tied} unique_XYZ={unique_xyz} LUT_axis_errors={lut_totals:?}"
    );
    for (kind, s) in ["VTC", "ARGB"].into_iter().zip(scores) {
        eprintln!(
            "{kind} Z coherence: adjacent_sum={} shuffled_sum={} samples={} ratio={}",
            s[0],
            s[1],
            s[2],
            s[0] as f64 / s[1].max(1) as f64
        );
        if s[1] > 0 {
            assert!(
                s[0] * 5 < s[1] * 4,
                "{kind} neighbour slices are not substantially smoother than the shuffled control: {s:?}"
            );
        }
    }
    if depth == Depth::Full {
        assert_eq!((count, compressed, argb, copies), (818, 771, 47, 408));
        assert_eq!(
            unique_xyz, 47,
            "RGB axis affinity did not distinguish XYZ for every LUT"
        );
    }
}
