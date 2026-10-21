//! BC7 encoding through `block_compression` (a pure-Rust CPU port of Intel's ISPC texture compressor: all BC7 modes,
//! no native code), behind our own entry point. Input is straight RGBA8, rows top first, as the decoders output; no
//! colour transfer is applied.
//!
//! Partial edge blocks (a side that is not a multiple of 4) are padded by repeating the last row/column of texels,
//! here, because the crate only accepts sizes that are multiples of 4.
//!
//! Quality: the crate's `slow` settings (all mode groups, extra refinement). A surface with any texel of alpha below
//! 255 uses `BC7Settings::alpha_slow` (alpha is part of the error); a fully opaque one uses `opaque_slow` (alpha is not
//! fitted and stays 255).
//!
//! Determinism: the encoder is plain Rust arithmetic without CPU-specific dispatch, so the same input gives the same
//! bytes (the tests encode twice and compare).
use crate::{Error, Result};
use block_compression::{BC7Settings, CompressionVariant, encode::compress_rgba8};
use primase_core::TexFormat;

/// Encodes `rgba` (straight RGBA8, `width * height * 4` bytes, rows top first) as BC7: `ceil(w/4) * ceil(h/4)`
/// blocks of 16 bytes, row-major. See the module documentation for the settings and the padding.
fn encode_bc7(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let bad = || Error::Dimensions { width, height };
    if width == 0 || height == 0 {
        return Err(bad());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(bad)?;
    if rgba.len() != expected {
        return Err(Error::Length {
            expected,
            actual: rgba.len(),
        });
    }
    let (pw, ph) = (
        width.div_ceil(4).checked_mul(4).ok_or_else(bad)?,
        height.div_ceil(4).checked_mul(4).ok_or_else(bad)?,
    );
    let padded_len = (pw as usize)
        .checked_mul(ph as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(bad)?;
    let mut padded = Vec::new();
    padded
        .try_reserve_exact(padded_len)
        .map_err(|_| Error::Allocation)?;
    for y in 0..ph as usize {
        let src = y.min(height as usize - 1) * width as usize * 4;
        let row = &rgba[src..src + width as usize * 4];
        padded.extend_from_slice(row);
        let edge = &row[row.len() - 4..];
        for _ in width..pw {
            padded.extend_from_slice(edge);
        }
    }
    let settings = if rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255) {
        BC7Settings::alpha_slow()
    } else {
        BC7Settings::opaque_slow()
    };
    let variant = CompressionVariant::BC7(settings);
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(variant.blocks_byte_size(pw, ph))
        .map_err(|_| Error::Allocation)?;
    blocks.resize(variant.blocks_byte_size(pw, ph), 0);
    compress_rgba8(variant, &padded, &mut blocks, pw, ph, pw * 4);
    Ok(blocks)
}

/// Encodes one tightly packed level from straight RGBA8, rows top first. The counterpart of
/// [`TexDecoder`](crate::TexDecoder): the encoder library behind it can be swapped without touching callers.
pub trait TexEncoder {
    /// `rgba` is `width * height * 4` bytes; the result is the level in `format`'s block layout. Formats the
    /// encoder does not write are refused with [`Error::Unsupported`].
    fn encode_rgba8(&self, format: TexFormat, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>>;
}

/// Encodes BC7 (all modes, through `block_compression`); every other format is refused.
#[derive(Debug, Clone, Copy, Default)]
pub struct Encoder {
    _private: (),
}
impl TexEncoder for Encoder {
    fn encode_rgba8(&self, format: TexFormat, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
        match format {
            TexFormat::Bc7 => encode_bc7(width, height, rgba),
            other => Err(Error::Unsupported(other)),
        }
    }
}
