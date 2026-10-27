//! A level decoded on several threads ([`Decoder::with_threads`], a row of blocks each) is the one decoded on one:
//! every block format, sizes that are and are not whole blocks, real BC7 blocks (encoded here from noise) and
//! arbitrary bytes alike, results and refusals.

#![allow(clippy::unwrap_used)]

use primase_core::TexFormat;
use primase_foundations::parallel::Threads;
use primase_texcodec::{Decoder, Encoder, TexDecoder, TexEncoder};

fn bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 24) as u8
        })
        .collect()
}

#[test]
fn every_thread_count_decodes_the_same_pixels() {
    let one = Decoder::default();
    let formats = [
        TexFormat::Bc1,
        TexFormat::Bc2,
        TexFormat::Bc3,
        TexFormat::Bc4Unorm,
        TexFormat::Bc5Unorm,
        TexFormat::Bc7,
        TexFormat::Rgba8Unorm,
    ];
    for (k, f) in formats.into_iter().enumerate() {
        for (w, h) in [(1, 1), (4, 4), (37, 29), (256, 64), (64, 257)] {
            let (bw, bh) = f.block_dims();
            let n = (w as usize).div_ceil(bw as usize)
                * (h as usize).div_ceil(bh as usize)
                * f.block_bytes() as usize;
            let data = bytes(n, 7 + k as u32 + w * 31 + h);
            let serial = one.decode_rgba8(f, w, h, &data).map_err(|e| e.to_string());
            for t in [2, 3, 8] {
                let many = Decoder::with_threads(Threads::new(t).unwrap())
                    .decode_rgba8(f, w, h, &data)
                    .map_err(|e| e.to_string());
                assert_eq!(many, serial, "{f:?} {w}x{h} on {t} threads");
            }
        }
    }
    // real BC7 (every block a valid encoding)
    let (w, h) = (128, 72);
    let blocks = Encoder::default()
        .encode_rgba8(TexFormat::Bc7, w, h, &bytes((w * h * 4) as usize, 99))
        .unwrap();
    let serial = one.decode_rgba8(TexFormat::Bc7, w, h, &blocks).unwrap();
    let many = Decoder::with_threads(Threads::new(8).unwrap())
        .decode_rgba8(TexFormat::Bc7, w, h, &blocks)
        .unwrap();
    assert_eq!(many, serial);
}
