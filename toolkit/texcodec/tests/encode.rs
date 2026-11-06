//! The encoders without outside tools: exact cases (one colour, one value), the quality on a smooth image against floors
//! taken from texconv's encode of it (tests/directxtex.rs measures every image against texconv itself), partial blocks
//! filled as DirectXTex fills them, the input conventions (clamping, NaN, SNORM bias, BC1's alpha threshold), the same
//! bytes on any thread count, every block decoded alike here and by bcdec, and refusals.
#![allow(clippy::unwrap_used)]
mod images;

use images::{RARE_MODES, Random, ldr};
use primase_core::TexFormat;
use primase_foundations::parallel::Threads;
use primase_texcodec::{Bcdec, Decoder, Encoder, Error, TexDecoder, TexEncoder};

const LDR: [TexFormat; 8] = [
    TexFormat::Bc1,
    TexFormat::Bc2,
    TexFormat::Bc3,
    TexFormat::Bc4Unorm,
    TexFormat::Bc4Snorm,
    TexFormat::Bc5Unorm,
    TexFormat::Bc5Snorm,
    TexFormat::Bc7,
];

fn encode(format: TexFormat, w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    Encoder::default().encode_rgba8(format, w, h, rgba).unwrap()
}

fn encode_f(format: TexFormat, w: u32, h: u32, rgba: &[f32]) -> Vec<u8> {
    Encoder::default().encode_rgba32f(format, w, h, rgba).unwrap()
}

fn decode(format: TexFormat, w: u32, h: u32, blocks: &[u8]) -> Vec<u8> {
    Decoder::default().decode_rgba8(format, w, h, blocks).unwrap()
}

fn decode_f(format: TexFormat, w: u32, h: u32, blocks: &[u8]) -> Vec<f32> {
    Decoder::default().decode_rgba32f(format, w, h, blocks).unwrap()
}

/// The channels a format stores, in RGBA order.
fn stored(format: TexFormat) -> usize {
    match format {
        TexFormat::Bc4Unorm | TexFormat::Bc4Snorm => 1,
        TexFormat::Bc5Unorm | TexFormat::Bc5Snorm => 2,
        TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat => 3,
        _ => 4,
    }
}

/// PSNR over the first `channels` channels, in 8-bit levels.
fn psnr(a: &[u8], b: &[u8], channels: usize) -> f64 {
    let (sum, n) = a
        .iter()
        .zip(b)
        .enumerate()
        .filter(|(i, _)| i % 4 < channels)
        .fold((0f64, 0usize), |(s, n), (_, (x, y))| {
            (s + (f64::from(*x) - f64::from(*y)).powi(2), n + 1)
        });
    10. * (255. * 255. / (sum / n as f64)).log10()
}

#[test]
fn one_value_is_exact_in_bc4_and_bc5() {
    for v in 0..=255u8 {
        let rgba: Vec<u8> = (0..16).flat_map(|_| [v, 255 - v, 0, 255]).collect();
        let d = decode(
            TexFormat::Bc4Unorm,
            4,
            4,
            &encode(TexFormat::Bc4Unorm, 4, 4, &rgba),
        );
        assert!(d.chunks(4).all(|t| t[0] == v), "BC4 {v}");
        let d = decode(
            TexFormat::Bc5Unorm,
            4,
            4,
            &encode(TexFormat::Bc5Unorm, 4, 4, &rgba),
        );
        assert!(d.chunks(4).all(|t| t[0] == v && t[1] == 255 - v), "BC5 {v}");
    }
    // SNORM: every one of the 255 values, from floats
    for q in -127..=127i32 {
        let v = q as f32 / 127.;
        let rgba: Vec<f32> = (0..16).flat_map(|_| [v, -v, 0., 1.]).collect();
        let d = decode_f(
            TexFormat::Bc4Snorm,
            4,
            4,
            &encode_f(TexFormat::Bc4Snorm, 4, 4, &rgba),
        );
        assert!(d.chunks(4).all(|t| t[0] == v), "BC4 SNORM {q}");
        let d = decode_f(
            TexFormat::Bc5Snorm,
            4,
            4,
            &encode_f(TexFormat::Bc5Snorm, 4, 4, &rgba),
        );
        assert!(d.chunks(4).all(|t| t[0] == v && t[1] == -v), "BC5 SNORM {q}");
    }
}

#[test]
fn one_colour_is_within_a_level_in_bc1_bc2_and_bc3() {
    let mut random = Random::new(17);
    for _ in 0..300 {
        let c: [u8; 4] = std::array::from_fn(|_| (random.next() >> 24) as u8);
        let c = [c[0], c[1], c[2], 255];
        let rgba: Vec<u8> = (0..16).flat_map(|_| c).collect();
        for format in [TexFormat::Bc1, TexFormat::Bc2, TexFormat::Bc3] {
            let d = decode(format, 4, 4, &encode(format, 4, 4, &rgba));
            for t in d.chunks(4) {
                for ch in 0..3 {
                    assert!(t[ch].abs_diff(c[ch]) <= 1, "{format:?} {c:?}: {t:?}");
                }
            }
        }
    }
}

#[test]
fn one_colour_is_exact_in_bc6h() {
    for v in [0., 1., 0.5, 3.140625, 1e-5, 65504., 0.000061035156, 1000.] {
        for signed in [false, true] {
            let format = if signed {
                TexFormat::Bc6hSfloat
            } else {
                TexFormat::Bc6hUfloat
            };
            let c = if signed {
                [v, -v * 0.5, v * 0.25, 1.]
            } else {
                [v, v * 0.5, v * 0.25, 1.]
            };
            let rgba: Vec<f32> = (0..16).flat_map(|_| c).collect();
            let d = decode_f(format, 4, 4, &encode_f(format, 4, 4, &rgba));
            let expected = c.map(|x| half::f16::from_f32(x).to_f32());
            for t in d.chunks(4) {
                assert_eq!(&t[..3], &expected[..3], "{format:?} {c:?}");
            }
        }
    }
}

/// Floors from texconv's own encode of the smooth image (`-nogpu -bc u`, DirectXTex may2026): the encodes here are at
/// least as good.
#[test]
fn a_smooth_image_encodes_at_least_as_well_as_texconv_did() {
    let images = ldr();
    let smooth = images.iter().find(|i| i.name == "smooth").unwrap();
    let opaque: Vec<u8> = smooth
        .texels
        .iter()
        .enumerate()
        .map(|(i, &v)| if i % 4 == 3 { 255 } else { v })
        .collect();
    for (format, floor) in [
        (TexFormat::Bc1, 33.90),
        (TexFormat::Bc2, 34.02),
        (TexFormat::Bc3, 34.83),
        (TexFormat::Bc4Unorm, 46.80),
        (TexFormat::Bc4Snorm, 47.00),
        (TexFormat::Bc5Unorm, 45.76),
        (TexFormat::Bc5Snorm, 45.97),
    ] {
        // BC1's one bit of alpha is not measured: it encodes the image opaque, and three channels
        let (source, channels) = if format == TexFormat::Bc1 {
            (&opaque, 3)
        } else {
            (&smooth.texels, stored(format))
        };
        let d = decode(format, 64, 64, &encode(format, 64, 64, source));
        let p = psnr(source, &d, channels);
        assert!(p >= floor, "{format:?}: {p:.2} dB, texconv {floor}");
    }
}

#[test]
fn every_block_decodes_alike_here_and_in_bcdec() {
    for image in ldr() {
        for format in &LDR[..7] {
            let blocks = encode(*format, image.width, image.height, &image.texels);
            if matches!(format, TexFormat::Bc4Snorm | TexFormat::Bc5Snorm) {
                // an entry of exactly 0 is a tie when biased to bytes: compared as floats
                let here = decode_f(*format, image.width, image.height, &blocks);
                let bcdec = Bcdec
                    .decode_rgba32f(*format, image.width, image.height, &blocks)
                    .unwrap();
                let d = here
                    .iter()
                    .zip(&bcdec)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0., f32::max);
                assert!(d <= 1e-6, "{format:?} {}: {d}", image.name);
            } else {
                assert_eq!(
                    decode(*format, image.width, image.height, &blocks),
                    Bcdec
                        .decode_rgba8(*format, image.width, image.height, &blocks)
                        .unwrap(),
                    "{format:?} {}",
                    image.name
                );
            }
        }
    }
}

/// A partial block is filled as DirectXTex fills it: the missing columns (rows) repeat columns 0, 0, 1 (rows alike), so
/// a 1, 2 or 3 texels wide image encodes as the 4 wide one holding those repeats.
#[test]
fn partial_blocks_repeat_the_texels_directxtex_repeats() {
    let fill = |s: usize, n: usize| -> usize {
        let mut s = s;
        while s >= n {
            s = [0, 0, 0, 1][s];
        }
        s
    };
    let mut random = Random::new(23);
    let full: Vec<u8> = (0..4 * 4 * 4).map(|_| (random.next() >> 24) as u8).collect();
    for format in LDR {
        for (w, h) in [(1, 1), (2, 3), (3, 2), (3, 4), (4, 1)] {
            let part: Vec<u8> = (0..h * w)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    full[(y * 4 + x) * 4..][..4].to_vec()
                })
                .collect();
            let filled: Vec<u8> = (0..16)
                .flat_map(|i| {
                    let (x, y) = (fill(i % 4, w), fill(i / 4, h));
                    part[(y * w + x) * 4..][..4].to_vec()
                })
                .collect();
            assert_eq!(
                encode(format, w as u32, h as u32, &part),
                encode(format, 4, 4, &filled),
                "{format:?} {w}x{h}"
            );
        }
    }
}

#[test]
fn inputs_are_clamped_and_nan_is_zero() {
    // a ramp in [0, 1] with every third texel's colour set to `v`: `outside` clamps to `edge`
    let block = |v: f32| -> Vec<f32> {
        (0..16)
            .flat_map(|i| {
                let t = i as f32 / 15.;
                if i % 3 == 0 {
                    [v, v, v, 1.]
                } else {
                    [t, 1. - t, t * 0.5, 1.]
                }
            })
            .collect()
    };
    for (format, outside, edge) in [
        (TexFormat::Bc4Unorm, 3., 1.),
        (TexFormat::Bc4Unorm, -2., 0.),
        (TexFormat::Bc4Snorm, -5., -1.),
        (TexFormat::Bc5Snorm, 9., 1.),
        (TexFormat::Bc3, 2., 1.),
        (TexFormat::Bc6hUfloat, 1e6, 65504.),
        (TexFormat::Bc6hUfloat, -4., 0.),
        (TexFormat::Bc6hSfloat, -1e9, -65504.),
    ] {
        assert_eq!(
            encode_f(format, 4, 4, &block(outside)),
            encode_f(format, 4, 4, &block(edge)),
            "{format:?} {outside}"
        );
    }
    for format in LDR
        .into_iter()
        .chain([TexFormat::Bc6hUfloat, TexFormat::Bc6hSfloat])
    {
        let nan: Vec<f32> = (0..64).map(|i| if i % 4 == 3 { 1. } else { f32::NAN }).collect();
        let zero: Vec<f32> = (0..64).map(|i| if i % 4 == 3 { 1. } else { 0. }).collect();
        assert_eq!(
            encode_f(format, 4, 4, &nan),
            encode_f(format, 4, 4, &zero),
            "{format:?}"
        );
    }
}

/// Bytes for an SNORM format are biased as the decoders bias them: `v / 255 * 2 - 1`.
#[test]
fn bytes_for_snorm_are_biased() {
    let mut random = Random::new(5);
    let bytes: Vec<u8> = (0..8 * 8 * 4).map(|_| (random.next() >> 24) as u8).collect();
    let floats: Vec<f32> = bytes
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if i % 4 < 2 {
                f32::from(v) / 255. * 2. - 1.
            } else {
                f32::from(v) / 255.
            }
        })
        .collect();
    for format in [TexFormat::Bc4Snorm, TexFormat::Bc5Snorm] {
        assert_eq!(
            encode(format, 8, 8, &bytes),
            encode_f(format, 8, 8, &floats),
            "{format:?}"
        );
    }
}

/// BC1 makes a texel transparent when its alpha is below 0.5 (a byte below 128), in a three-colour block.
#[test]
fn bc1_alpha_threshold() {
    for (alpha, transparent) in [(0u8, true), (127, true), (128, false), (255, false)] {
        let rgba: Vec<u8> = (0..16)
            .flat_map(|i| [200, 40, 90, if i == 5 { alpha } else { 255 }])
            .collect();
        let d = decode(TexFormat::Bc1, 4, 4, &encode(TexFormat::Bc1, 4, 4, &rgba));
        assert_eq!(d[5 * 4 + 3] == 0, transparent, "alpha {alpha}");
        assert!(d.chunks(4).enumerate().all(|(i, t)| i == 5 || t[3] == 255));
    }
    for (alpha, transparent) in [(0.49999, true), (0.5, false)] {
        let rgba: Vec<f32> = (0..16)
            .flat_map(|i| [0.8, 0.2, 0.4, if i == 0 { alpha } else { 1. }])
            .collect();
        let d = decode(TexFormat::Bc1, 4, 4, &encode_f(TexFormat::Bc1, 4, 4, &rgba));
        assert_eq!(d[3] == 0, transparent, "alpha {alpha}");
    }
    // all transparent
    let d = decode(TexFormat::Bc1, 4, 4, &encode(TexFormat::Bc1, 4, 4, &[0; 64]));
    assert!(d.iter().all(|&v| v == 0));
}

#[test]
fn every_thread_count_encodes_the_same_bytes() {
    let mut random = Random::new(41);
    let (w, h) = (37, 29);
    let bytes: Vec<u8> = (0..w * h * 4).map(|_| (random.next() >> 24) as u8).collect();
    let floats: Vec<f32> = (0..w * h * 4).map(|_| random.unit() * 300. - 100.).collect();
    for format in LDR
        .into_iter()
        .chain([TexFormat::Bc6hUfloat, TexFormat::Bc6hSfloat])
    {
        let one = Encoder::default();
        let four = Encoder::with_threads(Threads::new(4).unwrap());
        assert_eq!(
            one.encode_rgba8(format, w, h, &bytes).unwrap(),
            four.encode_rgba8(format, w, h, &bytes).unwrap(),
            "{format:?} bytes"
        );
        if format.is_block_compressed() && format != TexFormat::Bc7 {
            assert_eq!(
                one.encode_rgba32f(format, w, h, &floats).unwrap(),
                four.encode_rgba32f(format, w, h, &floats).unwrap(),
                "{format:?} floats"
            );
        }
    }
}

/// A half float's bit pattern as a signed integer: BC6H fits endpoints in this space.
fn half_pattern(v: f32) -> f64 {
    let h = half::f16::from_f32(v).to_bits();
    if h & 0x8000 != 0 {
        -f64::from(h & 0x7fff)
    } else {
        f64::from(h)
    }
}

/// Every one of BC6H's 14 modes is written, and decodes near its block: a wrong bit layout would not. (bcdec decodes
/// BC6H here; tests/directxtex.rs holds the blocks to DirectXTex's decoder.) The blocks: random ones on one colour line
/// and on two (left and right halves), each a base colour plus a spread from wide to narrow at magnitudes from 2^-10
/// to 2^14 (signed: some negative, some with a line of each sign), and the found blocks of the rare modes 0x06 and 0x0a.
#[test]
fn bc6h_writes_every_mode() {
    let mut random = Random::new(3);
    for signed in [false, true] {
        let format = if signed {
            TexFormat::Bc6hSfloat
        } else {
            TexFormat::Bc6hUfloat
        };
        let mut blocks: Vec<Vec<f32>> = (0..300)
            .map(|k| {
                let scale = (random.unit() * 24. - 10.).exp2();
                let spread = (-random.unit() * 10.).exp2();
                let mut colour = || -> [f32; 3] { std::array::from_fn(|_| scale * random.unit()) };
                let (bases, directions) = ([colour(), colour()], [colour(), colour()]);
                (0..16)
                    .flat_map(|i| {
                        let line = usize::from(k % 2 == 1 && i % 4 >= 2);
                        let t = (i / 4) as f32 / 3.;
                        // signed: some blocks negative, and some with one line of each sign
                        let sign = if signed && (k % 8 == 3 || k % 8 == 7 && line == 1) {
                            -1.
                        } else {
                            1.
                        };
                        let c: [f32; 3] = std::array::from_fn(|ch| {
                            sign * (bases[line][ch] + spread * t * directions[line][ch])
                        });
                        [c[0], c[1], c[2], 1.]
                    })
                    .collect()
            })
            .collect();
        blocks.extend(RARE_MODES[usize::from(signed)].iter().map(|patterns| {
            patterns
                .chunks(3)
                .flat_map(|c| {
                    let [r, g, b] = [c[0], c[1], c[2]].map(|p| half::f16::from_bits(p).to_f32());
                    [r, g, b, 1.]
                })
                .collect()
        }));
        let mut modes = std::collections::BTreeSet::new();
        for (k, rgba) in blocks.iter().enumerate() {
            let block = encode_f(format, 4, 4, rgba);
            // modes 0 and 1 are two bits, the rest five
            modes.insert(if block[0] & 2 == 0 {
                block[0] & 3
            } else {
                block[0] & 0x1f
            });
            let decoded = decode_f(format, 4, 4, &block);
            let rgb = |v: &[f32]| -> Vec<f64> {
                v.iter()
                    .enumerate()
                    .filter(|(i, _)| i % 4 < 3)
                    .map(|(_, &x)| half_pattern(x))
                    .collect()
            };
            let (input, output) = (rgb(rgba), rgb(&decoded));
            let range =
                input.iter().fold(f64::MIN, |a, &b| a.max(b)) - input.iter().fold(f64::MAX, |a, &b| a.min(b));
            let rms = (input
                .iter()
                .zip(&output)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                / 48.)
                .sqrt();
            // within 5% of the block's range (the worst of 20,000 such blocks is 3.4%)
            assert!(
                rms <= 0.05 * (range + 1.),
                "{format:?} block {k}: RMS {rms:.1}, range {range}"
            );
        }
        assert_eq!(modes.len(), 14, "{format:?}: modes {modes:x?}");
    }
}

#[test]
fn bad_input_is_refused() {
    for format in LDR.into_iter().chain([TexFormat::Bc6hUfloat]) {
        let e = Encoder::default();
        assert!(matches!(
            e.encode_rgba8(format, 0, 4, &[]),
            Err(Error::Dimensions { .. })
        ));
        assert!(matches!(
            e.encode_rgba8(format, 4, 4, &[0; 63]),
            Err(Error::Length { .. })
        ));
        assert!(matches!(
            e.encode_rgba32f(format, 4, 0, &[]),
            Err(Error::Dimensions { .. })
        ));
        assert!(matches!(
            e.encode_rgba32f(format, 4, 4, &[0.; 65]),
            Err(Error::Length { .. })
        ));
    }
}

#[test]
fn other_formats_are_refused_by_name() {
    for format in [TexFormat::Rgba8Unorm, TexFormat::Etc1] {
        let e = Encoder::default()
            .encode_rgba8(format, 4, 4, &[0; 64])
            .expect_err("not written");
        assert!(matches!(e, Error::Unsupported(f) if f == format));
        assert!(e.to_string().contains(&format!("{format:?}")), "{e}");
    }
}

// ---- BC7

fn noise(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut random = Random::new(seed);
    (0..w * h * 4).map(|_| (random.next() >> 8) as u8).collect()
}

/// A ramp along x between two colours, the same on every row: one line in RGBA space, so a block can fit it.
fn ramp(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for _ in 0..h {
        for x in 0..w {
            let t = x as f64 / (w - 1) as f64;
            v.extend([
                (255. * t) as u8,
                (40. + 150. * t) as u8,
                (200. - 180. * t) as u8,
                (255. - 100. * t) as u8,
            ]);
        }
    }
    v
}

/// (max abs error, RMS error) per channel.
fn errors(a: &[u8], b: &[u8]) -> ([u32; 4], [f64; 4]) {
    assert_eq!(a.len(), b.len());
    let (mut max, mut sq) = ([0u32; 4], [0f64; 4]);
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let d = (*x as i32 - *y as i32).unsigned_abs();
        max[i % 4] = max[i % 4].max(d);
        sq[i % 4] += (d * d) as f64;
    }
    let n = (a.len() / 4) as f64;
    (max, sq.map(|s| (s / n).sqrt()))
}

#[test]
fn bc7_one_and_two_colour_blocks_are_exact() {
    for c in [
        [0, 0, 0, 0],
        [254, 100, 6, 250],
        [200, 200, 200, 200],
        [2, 4, 8, 16],
        [255, 255, 255, 255],
    ] {
        let rgba: Vec<u8> = (0..16).flat_map(|_| c).collect();
        assert_eq!(
            decode(TexFormat::Bc7, 4, 4, &encode(TexFormat::Bc7, 4, 4, &rgba)),
            rgba,
            "{c:?}"
        );
    }
    // odd values: an endpoint's four channels share one p-bit, so only same-parity colours are exact
    let rgba: Vec<u8> = (0..16).flat_map(|_| [101, 33, 7, 255]).collect();
    let (max, _) = errors(
        &decode(TexFormat::Bc7, 4, 4, &encode(TexFormat::Bc7, 4, 4, &rgba)),
        &rgba,
    );
    assert!(max.iter().all(|&m| m <= 1), "{max:?}");
    let pairs = [
        ([0, 0, 0, 0], [254, 254, 254, 254]),
        ([10, 200, 30, 254], [250, 20, 90, 0]),
        ([1, 3, 5, 255], [255, 255, 9, 1]),
        ([64, 64, 64, 64], [66, 66, 66, 66]),
    ];
    for (a, b) in pairs {
        for pattern in [0xAAAAu32, 0x00FF, 0x8001, 0x0001] {
            let rgba: Vec<u8> = (0..16)
                .flat_map(|i| if pattern >> i & 1 == 1 { b } else { a })
                .collect();
            let d = decode(TexFormat::Bc7, 4, 4, &encode(TexFormat::Bc7, 4, 4, &rgba));
            assert_eq!(d, rgba, "{a:?} {b:?} {pattern:#x}");
        }
    }
}

#[test]
fn bc7_ramp_and_noise_error() {
    let r = ramp(64, 64);
    let (_, rms) = errors(
        &r,
        &decode(TexFormat::Bc7, 64, 64, &encode(TexFormat::Bc7, 64, 64, &r)),
    );
    assert!(rms.iter().all(|&e| e < 2.), "{rms:?}");
    // uniform noise has a per-channel deviation of ~74 and no structure that 4-bit indices and two or three subsets
    // can follow; this encoder lands around 42
    for seed in [1, 2, 3] {
        let n = noise(64, 64, seed);
        let (_, rms) = errors(
            &n,
            &decode(TexFormat::Bc7, 64, 64, &encode(TexFormat::Bc7, 64, 64, &n)),
        );
        assert!(rms.iter().all(|&r| r < 50.), "{rms:?}");
    }
    // every block has a valid mode: mode n is n zero bits then a one, so an all-zero low byte is the reserved tag
    let enc = encode(TexFormat::Bc7, 64, 64, &noise(64, 64, 5));
    assert!(enc.as_chunks::<16>().0.iter().all(|b| b[0] != 0));
}

#[test]
fn bc7_tiny_images_and_floats() {
    let one = [8, 16, 24, 32];
    let enc = encode(TexFormat::Bc7, 1, 1, &one);
    assert_eq!(enc.len(), 16);
    assert_eq!(decode(TexFormat::Bc7, 1, 1, &enc), one);
    // floats are rounded to bytes first
    let floats: Vec<f32> = ramp(5, 3).iter().map(|&v| f32::from(v) / 255.).collect();
    assert_eq!(
        encode_f(TexFormat::Bc7, 5, 3, &floats),
        encode(TexFormat::Bc7, 5, 3, &ramp(5, 3))
    );
}
