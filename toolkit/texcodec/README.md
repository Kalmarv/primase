# primase-texcodec

A Rust library that decodes and encodes GPU block-compressed textures (BC1 to BC7) for primase. It also decodes the
uncompressed formats of `primase_core::TexFormat` and lays out PS3 (RSX) texture storage.

## Features

- Decodes BC1 to BC7 and uncompressed formats to RGBA8 or RGBA32F, BC4 and BC5 in UNORM and SNORM, BC6H in UF16 and SF16
- Encodes BC1 to BC7, BC4 and BC5 in UNORM and SNORM and BC6H in UF16 and SF16, with its own encoders
- Searches all eight BC7 modes and every partition, rotation and index selection for each block
- Takes RGBA8 or RGBA32F input and fills partial edge blocks as DirectXTex fills them
- Ports DirectXTex's BC6H encoder, with three of its errors fixed
- Encodes each test image at least as well as texconv's CPU encoder (measurements below)
- Flips textures vertically, in block space where every level allows it (`flip_vertical`)
- Decodes PS3 GCM texture layouts, cubes and volumes included (`gcm`)
- Offers `Bcdec`, an adapter to bcdec_rs 0.2.0, as an independent decoder for checks
- Builds for `wasm32-unknown-unknown`; no native code
- Gives the same bytes for the same input on any machine and any thread count
- Returns errors, not panics, for unknown formats, wrong lengths, overflowing sizes and failed allocations

## Getting Started

### Installation

The crate is part of primase and is not on crates.io. Depend on it from Git, pinned to a commit:

```toml
[dependencies]
primase-core = { git = "https://github.com/Kalmarv/primase", rev = "<commit>" }
primase-texcodec = { git = "https://github.com/Kalmarv/primase", rev = "<commit>" }
```

### Usage

```rust
use primase_core::TexFormat;
use primase_texcodec::{Decoder, Encoder, TexDecoder, TexEncoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // a 6x5 gradient with alpha: the edge blocks are partial
    let (w, h) = (6, 5);
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i * 8) as u8, 128, 255 - (i * 8) as u8, 200]).collect();
    let blocks = Encoder::default().encode_rgba8(TexFormat::Bc3, w, h, &rgba)?;
    assert_eq!(blocks.len(), 4 * 16); // 2x2 blocks of 16 bytes
    let decoded = Decoder::default().decode_rgba8(TexFormat::Bc3, w, h, &blocks)?;
    let worst = rgba.iter().zip(&decoded).map(|(a, b)| a.abs_diff(*b)).max();
    println!("largest difference: {worst:?} levels");
    Ok(())
}
```

`Encoder::with_threads` and `Decoder::with_threads` work a level's rows of blocks on several threads. This example
needs no game files.

Pixels are straight RGBA, rows top first, with no colour transfer either way. Encoder input bytes are `v / 255`; for
an SNORM format they are biased as the decoder biases them (`v / 255 * 2 - 1`). Floats are clamped to the format's
range (0 to 1, -1 to 1, or the finite halves for BC6H), and NaN encodes as 0. BC1 makes a texel transparent when its
alpha is below 0.5. BC7 fits alpha only when some texel's alpha is below 1.

The decoder leaves missing colour channels at zero and missing alpha at one; BC4 is red only. Float output keeps
signed formats signed and does not clamp HDR values. Byte output clamps to 0 to 1 and biases signed channels.
BC1 to BC5 follow D3D's arithmetic; BC6H and BC7 decode through bcdec_rs.

DirectXTex's BC6H encoder (MIT, Microsoft) has three errors this port fixes. In two-region modes it fits region 0's
endpoints to all 16 texels; here each region gets its own texels. For SF16 its endpoint search never moves a negative
endpoint. At 16-bit precision (mode 0x0f) its endpoints decode to 31/64 of the target. The endpoint search here checks
every palette entry.

### Tests

```sh
cargo test -p primase-texcodec --test encode --test threads --test gates
```

`tests/directxtex.rs` measures every encoder against texconv from DirectXTex may2026, at `-nogpu -bc u` and at its
default settings, and BC7 also at `-bc ux` (all modes). It needs `tools.texconv` and `tools.directxtex_dump` in the file
`GAME_DATA` names ([tests/inputs](tests/inputs/README.md)). Every block written here decodes the same here, in bcdec_rs
and in DirectXTex's library. BC6H decodes exactly the same in all three, BC7 within 2e-5 of a level, and BC1 to BC5
within 0.72 of a level, since DirectXTex does not round its palettes.

The mean PSNR in dB is over the stored channels, with BC1 measured opaque on RGB; higher is better. For BC6H the table
gives the RMS of the difference in half-float bit patterns; lower is better. The synthetic set has 8 LDR and 7 HDR
images. The Sekiro set is a sample of the install's own DDS textures, decoded and encoded again: 34 files, up to
256x256, 138 encodes.

| Format | Synthetic, here | texconv `-bc u` | texconv default | Sekiro, here | texconv `-bc u` | texconv default |
| --- | --- | --- | --- | --- | --- | --- |
| BC1 | 32.64 | 30.95 | 29.97 | 34.06 | 33.19 | 32.29 |
| BC2 | 30.91 | 30.45 | 29.61 | 30.52 | 29.97 | 28.68 |
| BC3 | 33.77 | 32.11 | 31.16 | 43.23 | 38.75 | 37.92 |
| BC4 UNORM | 45.42 | 44.83 | 44.83 | 82.00 | 53.70 | 53.70 |
| BC4 SNORM | 46.90 | 45.88 | 45.88 | 67.64 | 52.31 | 52.31 |
| BC5 UNORM | 45.96 | 45.32 | 45.32 | 60.30 | 47.56 | 47.56 |
| BC5 SNORM | 47.36 | 46.38 | 46.38 | 41.72 | 40.76 | 40.76 |
| BC7 | 38.20 | 34.15 | 34.15 | 59.28 | 44.04 | 44.04 |
| BC6H UF16 (RMS) | 552.39 | 621.64 | 621.64 | 16.40 | 19.35 | 19.35 |
| BC6H SF16 (RMS) | 656.96 | 807.23 | 807.23 | | | |

An exact encode has infinite PSNR and its row is left out of the mean, so the synthetic BC7 mean skips the `blocks`
image, which BC7 encodes exactly here. texconv's `-bc ux` gives 34.15 dB on the synthetic BC7 set and 46.38 dB on the
Sekiro one.

Image by image, no encode here falls below any texconv setting measured. The closest is the 3x2 image, where both
give the same PSNR. The Sekiro BC6H column covers 48 faces of 8 of the install's 2,232 BC6H environment cubes
(`worlds/fromsoft/tests/environment_dds.rs`), with the largest ratio to texconv at 0.961.

`tests/encode.rs` covers, without outside tools, exact one-colour blocks, all 14 BC6H modes, edge-block filling,
clamping and NaN, BC1's alpha threshold, thread counts, and floors taken from texconv's encodes. The decoders' oracles
live in `tests/oracles`: compile `native.c` against the public [bcdec](https://github.com/iOrange/bcdec) header (SHA256
`867d2f6b13ec64c9fd807e3c7709c113feeff2a67d30d9ee333895cabb2ee7db`), run `pillow.py` (Pillow 11.0.0), then
`modes.py <native-executable>`. All of their data is synthetic. The decoders also meet real data in the readers' tests
(`worlds/unity/tests/texture.rs`, `worlds/anvil/tests/texture.rs`, `worlds/fromsoft/tests/tpf_pixels.rs`).

Licensed under GPL-3.0-or-later, as primase.
