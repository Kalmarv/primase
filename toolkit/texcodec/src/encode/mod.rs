//! Block encoders behind [`TexEncoder`]: BC1, BC2 and BC3 (colour: [`color`]; BC3's alpha: [`single`]), BC4 and BC5
//! UNORM and SNORM ([`single`]), BC6H UF16 and SF16 ([`bc6h`], DirectXTex's encoder ported), all here in Rust; BC7
//! through `block_compression` (a pure-Rust CPU port of Intel's ISPC texture compressor: all BC7 modes, no native code).
//!
//! Input is straight RGBA, rows top first, as the decoders output: bytes (`v / 255`) or floats. No colour transfer is
//! applied. UNORM formats clamp floats to 0..=1 and SNORM ones to -1..=1; bytes for an SNORM format are biased as the
//! decoders bias them (`v / 255 * 2 - 1`). BC6H takes floats as they are (clamped to its range; bytes are `v / 255`).
//! NaN encodes as 0. BC1 makes a texel of alpha below 0.5 (a byte below 128) transparent, in a three-colour block.
//!
//! A partial block at the right or bottom edge (a side that is not a multiple of 4) is filled as DirectXTex fills it:
//! the missing columns (rows) repeat the block's first and second, so its texels are all texels of the image.
//!
//! A level is encoded on [`Encoder::threads`] threads, a row of blocks at a time; the bytes are the same on any count.
//! Every encoder is plain Rust arithmetic without CPU-specific dispatch: the same input gives the same bytes.
//!
//! BC7 uses the crate's `slow` settings (all mode groups, extra refinement): `alpha_slow` when any texel's alpha is below
//! 1 (alpha is part of the error), else `opaque_slow` (alpha is not fitted and stays 1).
mod bc6h;
mod color;
mod single;

use crate::{Error, Result, buffer};
use block_compression::{BC7Settings, CompressionVariant, encode::compress_rgba8};
use primase_core::TexFormat;
use primase_foundations::parallel::{Threads, rows_mut};

/// Encodes one tightly packed level from straight RGBA, rows top first. The counterpart of
/// [`TexDecoder`](crate::TexDecoder).
pub trait TexEncoder {
    /// `rgba` is `width * height * 4` bytes; the result is the level in `format`'s block layout. Formats the encoder
    /// does not write are refused with [`Error::Unsupported`].
    fn encode_rgba8(&self, format: TexFormat, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>>;
    /// As [`TexEncoder::encode_rgba8`], from `width * height * 4` floats: signed for SNORM formats, HDR for BC6H.
    fn encode_rgba32f(&self, format: TexFormat, width: u32, height: u32, rgba: &[f32]) -> Result<Vec<u8>>;
}

/// Encodes BC1-BC7 (see the module documentation); every other format is refused. A level's rows of blocks are
/// encoded on [`Encoder::threads`] threads.
#[derive(Debug, Clone, Copy)]
pub struct Encoder {
    threads: Threads,
}

impl Default for Encoder {
    /// One thread.
    fn default() -> Encoder {
        Encoder {
            threads: Threads::ONE,
        }
    }
}

impl Encoder {
    /// An encoder working each level's rows of blocks on `threads` threads (at most that many rows at once).
    pub fn with_threads(threads: Threads) -> Encoder {
        Encoder { threads }
    }

    pub fn threads(&self) -> Threads {
        self.threads
    }
}

/// A level's texels.
#[derive(Clone, Copy)]
enum Source<'a> {
    Bytes(&'a [u8]),
    Floats(&'a [f32]),
}

impl Source<'_> {
    fn len(self) -> usize {
        match self {
            Source::Bytes(b) => b.len(),
            Source::Floats(f) => f.len(),
        }
    }

    /// Texel `i` as floats; bytes are `v / 255`, or biased to -1..=1 when `signed`.
    fn texel(self, i: usize, signed: bool) -> [f32; 4] {
        match self {
            Source::Bytes(b) => std::array::from_fn(|c| {
                let v = f32::from(b[i * 4 + c]) / 255.;
                if signed && c < 2 { v * 2. - 1. } else { v }
            }),
            Source::Floats(f) => std::array::from_fn(|c| f[i * 4 + c]),
        }
    }
}

/// The texel of a block that position `s` (0..4) of a partial block of `n` real ones repeats: DirectXTex's
/// `{0, 0, 0, 1}`, applied until it lands on a real texel.
fn fill(s: u32, n: u32) -> u32 {
    if s < n {
        s
    } else {
        fill([0, 0, 0, 1][s as usize], n)
    }
}

/// The level's byte count in `format`, checking `source` holds `width * height` texels.
fn sizes(format: TexFormat, width: u32, height: u32, source: Source) -> Result<usize> {
    let bad = || Error::Dimensions { width, height };
    if width == 0 || height == 0 {
        return Err(bad());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(bad)?;
    if source.len() != expected {
        return Err(Error::Length {
            expected,
            actual: source.len(),
        });
    }
    (width.div_ceil(4) as usize)
        .checked_mul(height.div_ceil(4) as usize)
        .and_then(|n| n.checked_mul(format.block_bytes() as usize))
        .ok_or_else(bad)
}

fn unorm(v: f32) -> f32 {
    if v.is_nan() { 0. } else { v.clamp(0., 1.) * 255. }
}

fn snorm(v: f32) -> f32 {
    if v.is_nan() { 0. } else { v.clamp(-1., 1.) * 127. }
}

/// The colour block of `texels` (0..=1 floats), BC1's transparent texels left out of the fit.
fn colour(texels: &[[f32; 4]; 16], palettes: color::Palettes) -> [u8; 8] {
    let transparent = if palettes == color::Palettes::Bc1 {
        // NaN alpha is 0: transparent
        (0..16).fold(0u16, |m, i| {
            m | u16::from(texels[i][3].is_nan() || texels[i][3] < 0.5) << i
        })
    } else {
        0
    };
    color::encode(
        &color::Texels {
            rgb: texels.map(|t| [unorm(t[0]), unorm(t[1]), unorm(t[2])]),
            transparent,
        },
        palettes,
    )
}

/// One block of `texels` in `format` (one of BC1-BC6H); the block's bytes are the first `format.block_bytes()`.
fn encode_block(format: TexFormat, texels: &[[f32; 4]; 16]) -> [u8; 16] {
    use TexFormat::*;
    let mut out = [0u8; 16];
    let channel =
        |c: usize, scale: fn(f32) -> f32| -> [f32; 16] { std::array::from_fn(|i| scale(texels[i][c])) };
    match format {
        Bc1 => out[..8].copy_from_slice(&colour(texels, color::Palettes::Bc1)),
        Bc2 => {
            // four bits of alpha per texel, nearest
            let alpha = (0..16).fold(0u64, |bits, i| {
                bits | ((unorm(texels[i][3]) / 17.).round() as u64) << (4 * i)
            });
            out[..8].copy_from_slice(&alpha.to_le_bytes());
            out[8..].copy_from_slice(&colour(texels, color::Palettes::Four));
        }
        Bc3 => {
            out[..8].copy_from_slice(&single::encode(&channel(3, unorm), false));
            out[8..].copy_from_slice(&colour(texels, color::Palettes::Four));
        }
        Bc4Unorm => out[..8].copy_from_slice(&single::encode(&channel(0, unorm), false)),
        Bc4Snorm => out[..8].copy_from_slice(&single::encode(&channel(0, snorm), true)),
        Bc5Unorm => {
            out[..8].copy_from_slice(&single::encode(&channel(0, unorm), false));
            out[8..].copy_from_slice(&single::encode(&channel(1, unorm), false));
        }
        Bc5Snorm => {
            out[..8].copy_from_slice(&single::encode(&channel(0, snorm), true));
            out[8..].copy_from_slice(&single::encode(&channel(1, snorm), true));
        }
        Bc6hUfloat | Bc6hSfloat => {
            out = bc6h::encode(&texels.map(|t| [t[0], t[1], t[2]]), format == Bc6hSfloat);
        }
        _ => {}
    }
    out
}

/// Encodes a level in BC1-BC6H, a row of blocks per task.
fn encode_blocks(
    threads: Threads,
    format: TexFormat,
    width: u32,
    height: u32,
    source: Source,
) -> Result<Vec<u8>> {
    let size = sizes(format, width, height, source)?;
    let signed = matches!(format, TexFormat::Bc4Snorm | TexFormat::Bc5Snorm);
    let block_bytes = format.block_bytes() as usize;
    let mut out = buffer(size)?;
    rows_mut(
        threads,
        &mut out,
        width.div_ceil(4) as usize * block_bytes,
        |by, row| {
            let by = by as u32;
            let rows = (height - by * 4).min(4);
            for (bx, block) in row.chunks_exact_mut(block_bytes).enumerate() {
                let bx = bx as u32;
                let columns = (width - bx * 4).min(4);
                let texels: [[f32; 4]; 16] = std::array::from_fn(|i| {
                    let (x, y) = (fill(i as u32 % 4, columns), fill(i as u32 / 4, rows));
                    source.texel(((by * 4 + y) * width + bx * 4 + x) as usize, signed)
                });
                block.copy_from_slice(&encode_block(format, &texels)[..block_bytes]);
            }
        },
    )?;
    Ok(out)
}

/// Encodes a level as BC7 through `block_compression`, a row of blocks per task, partial blocks filled here (the crate
/// takes whole blocks only).
fn encode_bc7(threads: Threads, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let size = sizes(TexFormat::Bc7, width, height, Source::Bytes(rgba))?;
    let settings = if rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255) {
        BC7Settings::alpha_slow()
    } else {
        BC7Settings::opaque_slow()
    };
    let variant = CompressionVariant::BC7(settings);
    let blocks_x = width.div_ceil(4);
    let mut out = buffer(size)?;
    rows_mut(threads, &mut out, blocks_x as usize * 16, |by, row| {
        let by = by as u32;
        let rows = (height - by * 4).min(4);
        // the row of blocks as whole blocks: four rows of `blocks_x * 4` texels
        let mut strip = Vec::with_capacity(blocks_x as usize * 64);
        for y in 0..4 {
            let source = (by * 4 + fill(y, rows)) * width;
            for x in 0..blocks_x * 4 {
                let (bx, s) = (x / 4, x % 4);
                let column = bx * 4 + fill(s, (width - bx * 4).min(4));
                let i = (source + column) as usize * 4;
                strip.extend_from_slice(&rgba[i..i + 4]);
            }
        }
        compress_rgba8(variant, &strip, row, blocks_x * 4, 4, blocks_x * 16);
    })?;
    Ok(out)
}

/// Rounds floats to bytes (0..=1 to 0..=255; NaN is 0) for the BC7 encoder.
fn to_bytes(rgba: &[f32]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(rgba.len()).map_err(|_| Error::Allocation)?;
    out.extend(rgba.iter().map(|&v| unorm(v).round() as u8));
    Ok(out)
}

/// The formats encoded here, BC7 apart.
fn encodes(format: TexFormat) -> bool {
    use TexFormat::*;
    matches!(
        format,
        Bc1 | Bc2 | Bc3 | Bc4Unorm | Bc4Snorm | Bc5Unorm | Bc5Snorm | Bc6hUfloat | Bc6hSfloat
    )
}

impl TexEncoder for Encoder {
    fn encode_rgba8(&self, format: TexFormat, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
        match format {
            TexFormat::Bc7 => encode_bc7(self.threads, width, height, rgba),
            f if encodes(f) => encode_blocks(self.threads, f, width, height, Source::Bytes(rgba)),
            other => Err(Error::Unsupported(other)),
        }
    }

    fn encode_rgba32f(&self, format: TexFormat, width: u32, height: u32, rgba: &[f32]) -> Result<Vec<u8>> {
        match format {
            TexFormat::Bc7 => {
                sizes(format, width, height, Source::Floats(rgba))?;
                encode_bc7(self.threads, width, height, &to_bytes(rgba)?)
            }
            f if encodes(f) => encode_blocks(self.threads, f, width, height, Source::Floats(rgba)),
            other => Err(Error::Unsupported(other)),
        }
    }
}
