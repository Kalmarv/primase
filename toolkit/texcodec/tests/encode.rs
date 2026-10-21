use primase_core::TexFormat;
use primase_texcodec::{Decoder, Encoder, Error, TexDecoder, TexEncoder};

fn encode_bc7(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, Error> {
    Encoder::default().encode_rgba8(TexFormat::Bc7, w, h, rgba)
}

fn decode(w: u32, h: u32, blocks: &[u8]) -> Vec<u8> {
    Decoder::default()
        .decode_rgba8(TexFormat::Bc7, w, h, blocks)
        .expect("every block decodes")
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
fn noise(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut s = seed;
    (0..w * h * 4)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 8) as u8
        })
        .collect()
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
/// Two independent directions: a plane in RGBA space, which one line per block cannot follow.
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            v.extend([
                (x * 255 / (w - 1)) as u8,
                (y * 255 / (h - 1)) as u8,
                ((x + y) * 255 / (w + h - 2)) as u8,
                255 - (x * 255 / (w - 1)) as u8 / 2,
            ]);
        }
    }
    v
}

#[test]
fn single_colour_blocks_are_exact() {
    for c in [
        [0, 0, 0, 0],
        [254, 100, 6, 250],
        [200, 200, 200, 200],
        [2, 4, 8, 16],
        [255, 255, 255, 255],
    ] {
        let rgba: Vec<u8> = (0..16).flat_map(|_| c).collect();
        let enc = encode_bc7(4, 4, &rgba).expect("encode");
        assert_eq!(enc.len(), 16);
        assert_eq!(decode(4, 4, &enc), rgba, "{c:?}");
    }
    // Odd values: an endpoint's four channels share one p-bit, so only same-parity colours are exact.
    let c = [255, 255, 255, 255];
    let rgba: Vec<u8> = (0..16).flat_map(|_| c).collect();
    assert_eq!(decode(4, 4, &encode_bc7(4, 4, &rgba).expect("encode")), rgba);
    let c = [101, 33, 7, 255];
    let rgba: Vec<u8> = (0..16).flat_map(|_| c).collect();
    let (max, _) = errors(&decode(4, 4, &encode_bc7(4, 4, &rgba).expect("encode")), &rgba);
    assert!(max.iter().all(|&m| m <= 1), "{max:?}");
}
#[test]
fn two_colour_blocks_are_exact() {
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
            let enc = encode_bc7(4, 4, &rgba).expect("encode");
            assert_eq!(decode(4, 4, &enc), rgba, "{a:?} {b:?} {pattern:#x}");
        }
    }
}
#[test]
fn gradient_and_noise_error() {
    let r = ramp(64, 64);
    let d = decode(64, 64, &encode_bc7(64, 64, &r).expect("encode"));
    let (max, rms) = errors(&r, &d);
    println!("ramp 64x64: max {max:?} rms {rms:?}");
    assert!(rms.iter().all(|&e| e < 2.), "{rms:?}");
    let g = gradient(64, 64);
    let d = decode(64, 64, &encode_bc7(64, 64, &g).expect("encode"));
    let (max, rms) = errors(&g, &d);
    println!("2-D gradient 64x64: max {max:?} rms {rms:?}");
    assert!(rms.iter().all(|&e| e < 3.), "{rms:?}");
    // Uniform noise has a per-channel deviation of ~74 and no structure that 4-bit indices and two or three subsets
    // can follow; this encoder lands around 42.
    for seed in [1, 2, 3] {
        let n = noise(64, 64, seed);
        let d = decode(64, 64, &encode_bc7(64, 64, &n).expect("encode"));
        let (max, rms) = errors(&n, &d);
        println!("noise 64x64 seed {seed}: max {max:?} rms {rms:?}");
        assert!(rms.iter().all(|&r| r < 50.), "{rms:?}");
    }
}
#[test]
fn deterministic() {
    let n = noise(33, 17, 9);
    assert_eq!(
        encode_bc7(33, 17, &n).expect("a"),
        encode_bc7(33, 17, &n).expect("b")
    );
}
#[test]
fn partial_edge_blocks_and_tiny_images() {
    // 5x3: 2x1 blocks; the texels outside the image repeat the edge.
    let g = ramp(5, 3);
    let enc = encode_bc7(5, 3, &g).expect("encode");
    assert_eq!(enc.len(), 2 * 16);
    let d = decode(5, 3, &enc);
    assert_eq!(d.len(), g.len());
    let (_, rms) = errors(&g, &d);
    assert!(rms.iter().all(|&r| r < 8.), "{rms:?}");
    // A solid even colour stays exact across the padded edge.
    let solid: Vec<u8> = (0..15).flat_map(|_| [40, 80, 120, 200]).collect();
    assert_eq!(decode(5, 3, &encode_bc7(5, 3, &solid).expect("encode")), solid);
    // 1x1.
    let one = [8, 16, 24, 32];
    let enc = encode_bc7(1, 1, &one).expect("encode");
    assert_eq!(enc.len(), 16);
    assert_eq!(decode(1, 1, &enc), one);
}
#[test]
fn bad_input_is_refused() {
    assert!(encode_bc7(0, 4, &[]).is_err());
    assert!(encode_bc7(4, 4, &[0; 63]).is_err());
}
#[test]
fn every_block_has_a_valid_mode() {
    // Mode n is signalled by n zero bits then a one: an all-zero low byte is the reserved tag.
    let enc = encode_bc7(64, 64, &noise(64, 64, 5)).expect("encode");
    assert!(enc.as_chunks::<16>().0.iter().all(|b| b[0] != 0));
}
#[test]
fn other_formats_are_refused_by_name() {
    let e = Encoder::default()
        .encode_rgba8(TexFormat::Bc1, 4, 4, &[0; 64])
        .expect_err("BC1 is not written");
    assert!(matches!(e, Error::Unsupported(TexFormat::Bc1)));
    assert!(e.to_string().contains("Bc1"), "{e}");
}
