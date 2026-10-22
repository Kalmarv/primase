//! Checked texture-level decoding. Rows are top first; channels are straight RGBA.
//! No colour-space transfer is performed. Missing channels are zero, alpha is one.
//! SNORM floats are signed; RGBA8 biases present signed channels from [-1,1] to
//! [0,255], rounding ties to even. BC4 remains red-only.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use primase_core::TexFormat;
use primase_core::parallel::{Panicked, Threads, rows_mut};
use thiserror::Error;
mod blocks;
mod encode;
mod flip;
mod pixels;

pub use encode::{Encoder, TexEncoder};
pub use flip::flip_vertical;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported texture format {0:?}")]
    Unsupported(TexFormat),
    #[error("invalid or overflowing texture dimensions {width}x{height}")]
    Dimensions { width: u32, height: u32 },
    #[error("level byte length: expected {expected}, got {actual}")]
    Length { expected: usize, actual: usize },
    #[error("could not allocate decoded pixels")]
    Allocation,
    #[error("decoding a row of blocks: {0}")]
    Worker(#[from] Panicked),
}
pub type Result<T> = std::result::Result<T, Error>;

pub trait TexDecoder {
    fn decode_rgba8(&self, format: TexFormat, width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>>;
    /// Signed formats remain signed; HDR values are not clamped.
    fn decode_rgba32f(&self, format: TexFormat, width: u32, height: u32, data: &[u8]) -> Result<Vec<f32>>;
}

/// Independent Rust BC1-5 implementation; BC6H/BC7 use the checked bcdec adapter. A block-compressed level is decoded
/// on [`Decoder::threads`] threads, a row of blocks at a time (the same pixels on any count).
#[derive(Debug, Clone, Copy)]
pub struct Decoder {
    threads: Threads,
}

impl Default for Decoder {
    /// One thread.
    fn default() -> Decoder {
        Decoder {
            threads: Threads::ONE,
        }
    }
}

impl Decoder {
    /// A decoder working each level's rows of blocks on `threads` threads (at most that many rows at once).
    pub fn with_threads(threads: Threads) -> Decoder {
        Decoder { threads }
    }

    pub fn threads(&self) -> Threads {
        self.threads
    }
}
/// Adapter to bcdec_rs 0.2.0, useful as an independent gate oracle for BC1-5.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bcdec;

fn validate(f: TexFormat, w: u32, h: u32, data: &[u8]) -> Result<usize> {
    let (bw, bh) = f.block_dims();
    let bad = || Error::Dimensions { width: w, height: h };
    if w == 0 || h == 0 || bw == 0 || bh == 0 {
        return Err(bad());
    }
    let size = (w.div_ceil(bw) as usize)
        .checked_mul(h.div_ceil(bh) as usize)
        .and_then(|n| n.checked_mul(f.block_bytes() as usize))
        .ok_or_else(bad)?;
    if data.len() != size {
        return Err(Error::Length {
            expected: size,
            actual: data.len(),
        });
    }
    (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(bad)
}
fn buffer<T: Default + Clone>(n: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::Allocation)?;
    v.resize(n, T::default());
    Ok(v)
}
fn signed(f: TexFormat) -> bool {
    use TexFormat::*;
    matches!(
        f,
        R8Snorm | Rg8Snorm | Rgba8Snorm | Rg16Snorm | Bc4Snorm | Bc5Snorm
    )
}
fn channels(f: TexFormat) -> usize {
    use TexFormat::*;
    match f {
        R8Snorm | Bc4Snorm => 1,
        Rg8Snorm | Rg16Snorm | Bc5Snorm => 2,
        _ => 4,
    }
}
fn to_byte(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255.).round_ties_even() as u8
}

fn decode8(oracle: bool, threads: Threads, f: TexFormat, w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>> {
    let n = validate(f, w, h, data)?;
    if !f.is_block_compressed() || signed(f) || matches!(f, TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat) {
        let floats = decode_float(oracle, f, w, h, data)?;
        return Ok(floats
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                to_byte(if signed(f) && i % 4 < channels(f) {
                    v * 0.5 + 0.5
                } else {
                    v
                })
            })
            .collect());
    }
    let mut out = buffer(n)?;
    // each row of blocks fills its own four rows of pixels (fewer at the bottom edge); the first failing row's error
    let failed: std::sync::Mutex<Option<(u32, Error)>> = std::sync::Mutex::new(None);
    let block_row = 4 * w as usize * 4;
    rows_mut(threads, &mut out, block_row, |by, rows| {
        let by = by as u32;
        if let Err(e) = decode_block_row(oracle, f, w, by, data, rows)
            && let Ok(mut first) = failed.lock()
            && first.as_ref().is_none_or(|(at, _)| by < *at)
        {
            *first = Some((by, e));
        }
    })?;
    if let Some((_, e)) = failed.into_inner().ok().flatten() {
        return Err(e);
    }
    Ok(out)
}

/// Block row `by` of a level `w` texels wide into `rows`, its pixel rows (four, or what is left of the level).
fn decode_block_row(oracle: bool, f: TexFormat, w: u32, by: u32, data: &[u8], rows: &mut [u8]) -> Result<()> {
    let height = (rows.len() / (w as usize * 4)) as u32;
    for bx in 0..w.div_ceil(4) {
        let offset = (by as usize * w.div_ceil(4) as usize + bx as usize) * f.block_bytes() as usize;
        let block = blocks::decode(f, &data[offset..offset + f.block_bytes() as usize], oracle)?;
        for y in 0..height {
            for x in 0..4 {
                if bx * 4 + x < w {
                    let dst = ((y as usize * w as usize) + (bx * 4 + x) as usize) * 4;
                    let src = (y * 4 + x) as usize * 4;
                    rows[dst..dst + 4].copy_from_slice(&block[src..src + 4]);
                }
            }
        }
    }
    Ok(())
}
fn decode_float(oracle: bool, f: TexFormat, w: u32, h: u32, data: &[u8]) -> Result<Vec<f32>> {
    let n = validate(f, w, h, data)?;
    if !f.is_block_compressed() {
        return pixels::decode(f, data, n);
    }
    if !matches!(
        f,
        TexFormat::Bc4Snorm | TexFormat::Bc5Snorm | TexFormat::Bc6hUfloat | TexFormat::Bc6hSfloat
    ) {
        return Ok(decode8(oracle, Threads::ONE, f, w, h, data)?
            .iter()
            .map(|&v| v as f32 / 255.)
            .collect());
    }
    let mut out = buffer(n)?;
    for by in 0..h.div_ceil(4) {
        for bx in 0..w.div_ceil(4) {
            let offset = (by as usize * w.div_ceil(4) as usize + bx as usize) * f.block_bytes() as usize;
            let b = &data[offset..offset + f.block_bytes() as usize];
            let mut block = [0.; 64];
            for p in block.as_chunks_mut::<4>().0 {
                p[3] = 1.;
            }
            match f {
                TexFormat::Bc4Snorm | TexFormat::Bc5Snorm => {
                    for c in 0..channels(f) {
                        let mut values = [0.; 16];
                        if oracle {
                            bcdec_rs::bc4_float(&b[c * 8..], &mut values, 4, true);
                        } else {
                            values = blocks::snorm(&b[c * 8..]);
                        }
                        for i in 0..16 {
                            block[i * 4 + c] = values[i];
                        }
                    }
                }
                _ => {
                    let mut rgb = [0.; 48];
                    bcdec_rs::bc6h_float(b, &mut rgb, 12, f == TexFormat::Bc6hSfloat);
                    for i in 0..16 {
                        block[i * 4..i * 4 + 3].copy_from_slice(&rgb[i * 3..i * 3 + 3]);
                    }
                }
            }
            for y in 0..4 {
                for x in 0..4 {
                    if bx * 4 + x < w && by * 4 + y < h {
                        let dst = (((by * 4 + y) as usize * w as usize) + (bx * 4 + x) as usize) * 4;
                        let src = (y * 4 + x) as usize * 4;
                        out[dst..dst + 4].copy_from_slice(&block[src..src + 4]);
                    }
                }
            }
        }
    }
    Ok(out)
}
impl TexDecoder for Decoder {
    fn decode_rgba8(&self, f: TexFormat, w: u32, h: u32, d: &[u8]) -> Result<Vec<u8>> {
        decode8(false, self.threads, f, w, h, d)
    }
    fn decode_rgba32f(&self, f: TexFormat, w: u32, h: u32, d: &[u8]) -> Result<Vec<f32>> {
        decode_float(false, f, w, h, d)
    }
}
impl TexDecoder for Bcdec {
    fn decode_rgba8(&self, f: TexFormat, w: u32, h: u32, d: &[u8]) -> Result<Vec<u8>> {
        decode8(true, Threads::ONE, f, w, h, d)
    }
    fn decode_rgba32f(&self, f: TexFormat, w: u32, h: u32, d: &[u8]) -> Result<Vec<f32>> {
        decode_float(true, f, w, h, d)
    }
}
