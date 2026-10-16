//! Validated per-mip decoding and channel mapping; source recipes remain with callers.
use crate::{Error, Result, sample_count};
use primase_core::{ColorSpace, NormalConvention, TexFormat, TexRole, Texture};
use primase_texcodec::{Decoder, TexDecoder};
use std::ops::Range;

/// Metadata explicitly assigned to the mapped RGBA8 texture.
#[derive(Clone, Copy, Debug)]
pub struct TextureMetadata {
    pub color_space: ColorSpace,
    pub role: TexRole,
    pub normal: Option<NormalConvention>,
}

/// One decoded texel, with its original source mip and integer pixel coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Rgba8Texel {
    pub level: usize,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: [u8; 4],
}

fn dimensions(source: &Texture, level: usize) -> (u32, u32) {
    let shift = u32::try_from(level).unwrap_or(u32::MAX);
    (
        source.width.checked_shr(shift).unwrap_or(0).max(1),
        source.height.checked_shr(shift).unwrap_or(0).max(1),
    )
}

/// Decode and map exactly the selected source mips, in row-major order.
/// `0..1` selects only the base mip; `0..source.levels.len()` selects all stored mips.
/// The output's base dimensions correspond to `levels.start`. No resampling,
/// colour transfer, normal conversion or mip generation is implicit.
/// All selected dimensions and payload sizes are checked before invoking the mapper.
/// Errors return no partial texture and never mutate the source.
pub fn map_rgba8_levels(
    source: &Texture,
    levels: Range<usize>,
    metadata: TextureMetadata,
    mut map: impl FnMut(Rgba8Texel) -> Result<[u8; 4]>,
) -> Result<Texture> {
    sample_count(source.width, source.height)?;
    if levels.start > levels.end || levels.end > source.levels.len() {
        return Err(Error::Invalid("selected mip range outside texture".into()));
    }
    let mut total = 0usize;
    let (bw, bh) = source.format.block_dims();
    for level in levels.clone() {
        let (width, height) = dimensions(source, level);
        let samples = sample_count(width, height)?;
        total = total
            .checked_add(samples)
            .ok_or_else(|| Error::Invalid("mapped mip sizes overflow".into()))?;
        let expected = (width.div_ceil(bw) as usize)
            .checked_mul(height.div_ceil(bh) as usize)
            .and_then(|n| n.checked_mul(source.format.block_bytes() as usize))
            .ok_or_else(|| Error::Invalid(format!("mip {level} encoded size overflows")))?;
        if source.levels[level].len() != expected {
            return Err(Error::Invalid(format!(
                "mip {level}: expected {expected} encoded bytes, got {}",
                source.levels[level].len()
            )));
        }
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(levels.len())
        .map_err(|_| Error::Allocation)?;
    let (width, height) = dimensions(source, levels.start);
    for level in levels {
        let (w, h) = dimensions(source, level);
        let mut pixels = Decoder::default().decode_rgba8(source.format, w, h, &source.levels[level])?;
        for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *pixel = map(Rgba8Texel {
                level,
                x: (i % w as usize) as u32,
                y: (i / w as usize) as u32,
                width: w,
                height: h,
                rgba: *pixel,
            })?;
        }
        output.push(pixels);
    }
    Ok(Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels: output,
        color_space: metadata.color_space,
        role: metadata.role,
        normal: metadata.normal,
    })
}
