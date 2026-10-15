//! Vertical flips of whole textures, for games that store rows bottom first.
use crate::{Error, Result, TexDecoder};
use primase_core::{TexFormat, Texture};

/// Which sub-blocks of a BC1-5 block hold rows, and how: (offset, kind).
#[derive(Clone, Copy)]
enum Rows {
    /// BC1-style colour indices: one byte per row after the two endpoints.
    Colour,
    /// BC2 explicit alpha: two bytes (4 x 4 bits) per row.
    Explicit,
    /// BC3/BC4 interpolated alpha: 48 bits of 3-bit indices after the two endpoints, 12 bits per row.
    Interpolated,
}

fn parts(f: TexFormat) -> Option<&'static [(usize, Rows)]> {
    use TexFormat::*;
    Some(match f {
        Bc1 => &[(0, Rows::Colour)],
        Bc2 => &[(0, Rows::Explicit), (8, Rows::Colour)],
        Bc3 => &[(0, Rows::Interpolated), (8, Rows::Colour)],
        Bc4Unorm | Bc4Snorm => &[(0, Rows::Interpolated)],
        Bc5Unorm | Bc5Snorm => &[(0, Rows::Interpolated), (8, Rows::Interpolated)],
        _ => return None,
    })
}

/// Reverse the first `h` (1-4) pixel rows of one block, in place.
fn flip_block(block: &mut [u8], parts: &[(usize, Rows)], h: usize) {
    for &(o, kind) in parts {
        match kind {
            Rows::Colour => block[o + 4..o + 4 + h].reverse(),
            Rows::Explicit => {
                let mut rows: Vec<[u8; 2]> =
                    (0..h).map(|y| [block[o + 2 * y], block[o + 2 * y + 1]]).collect();
                rows.reverse();
                for (y, r) in rows.iter().enumerate() {
                    block[o + 2 * y..o + 2 * y + 2].copy_from_slice(r);
                }
            }
            Rows::Interpolated => {
                let bits = block[o + 2..o + 8]
                    .iter()
                    .enumerate()
                    .fold(0u64, |v, (i, &b)| v | (u64::from(b) << (8 * i)));
                let mut out = bits;
                for y in 0..h {
                    let row = (bits >> (12 * (h - 1 - y))) & 0xFFF;
                    out = (out & !(0xFFF << (12 * y))) | (row << (12 * y));
                }
                block[o + 2..o + 8].copy_from_slice(&out.to_le_bytes()[..6]);
            }
        }
    }
}

/// One level flipped without decoding, when that is exact: uncompressed rows reversed; BC1-5 block rows reversed and
/// the rows inside each block, for heights that are whole blocks or within one block row. `None` otherwise.
fn flip_level(f: TexFormat, w: u32, h: u32, data: &[u8]) -> Option<Vec<u8>> {
    let (bw, bh) = f.block_dims();
    let row = w.div_ceil(bw) as usize * f.block_bytes() as usize;
    let rows = h.div_ceil(bh) as usize;
    if data.len() != row * rows {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    for r in data.chunks_exact(row).rev() {
        out.extend_from_slice(r);
    }
    if !f.is_block_compressed() {
        return Some(out);
    }
    let parts = parts(f)?;
    let inside = match h {
        h if h % 4 == 0 => 4,
        h if h < 4 => h as usize,
        _ => return None,
    };
    for block in out.chunks_exact_mut(f.block_bytes() as usize) {
        flip_block(block, parts, inside);
    }
    Some(out)
}

/// Reverse the rows of a level's RGBA pixels, `w` pixels a row.
fn flip_pixels<T: Copy>(pixels: &[T], w: u32) -> Vec<T> {
    pixels
        .chunks_exact(w as usize * 4)
        .rev()
        .flatten()
        .copied()
        .collect()
}

/// `t` with the rows of every level in reverse order.
///
/// Exact in its own format when every level allows it: uncompressed formats always; BC1-5 when each level's height is a
/// multiple of 4 or below 4 (its single block row). Otherwise each level is decoded with `decoder` and flipped as
/// pixels: `Rgba8Unorm` for formats that decode to 8-bit unsigned values (BC1-3, BC4/BC5 unorm, BC7, ETC, ASTC),
/// exact; `Rgba32Float` for the rest (BC6H, signed BC4/BC5). The result's format tells which happened.
pub fn flip_vertical(t: &Texture, decoder: &impl TexDecoder) -> Result<Texture> {
    let dims = |i: usize| ((t.width >> i).max(1), (t.height >> i).max(1));
    let exact: Option<Vec<Vec<u8>>> = t
        .levels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let (w, h) = dims(i);
            flip_level(t.format, w, h, l)
        })
        .collect();
    if let Some(levels) = exact {
        return Ok(Texture { levels, ..t.clone() });
    }
    use TexFormat::*;
    let float = matches!(t.format, Bc6hUfloat | Bc6hSfloat | Bc4Snorm | Bc5Snorm);
    let mut levels = Vec::with_capacity(t.levels.len());
    for (i, l) in t.levels.iter().enumerate() {
        let (w, h) = dims(i);
        levels.push(if float {
            flip_pixels(&decoder.decode_rgba32f(t.format, w, h, l)?, w)
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect()
        } else {
            flip_pixels(&decoder.decode_rgba8(t.format, w, h, l)?, w)
        });
    }
    if levels.is_empty() {
        return Err(Error::Dimensions {
            width: t.width,
            height: t.height,
        });
    }
    Ok(Texture {
        format: if float { Rgba32Float } else { Rgba8Unorm },
        levels,
        ..t.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decoder;
    use primase_core::{ColorSpace, TexRole};

    fn noise(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (s >> 33) as u8
            })
            .collect()
    }

    fn texture(format: TexFormat, width: u32, height: u32) -> Texture {
        let levels = (0..)
            .map(|i| ((width >> i).max(1), (height >> i).max(1)))
            .take(1 + width.max(height).ilog2() as usize)
            .enumerate()
            .map(|(i, (w, h))| noise(format.level_size(w, h), i as u64 + 7))
            .collect();
        Texture {
            width,
            height,
            format,
            levels,
            color_space: ColorSpace::Linear,
            role: TexRole::Color,
            normal: None,
        }
    }

    /// Decoded pixels of every level, as RGBA8 or RGBA32F bits.
    fn pixels(t: &Texture) -> Vec<Vec<u32>> {
        let d = Decoder::default();
        t.levels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let (w, h) = ((t.width >> i).max(1), (t.height >> i).max(1));
                d.decode_rgba32f(t.format, w, h, l)
                    .unwrap()
                    .iter()
                    .map(|x| x.to_bits())
                    .collect()
            })
            .collect()
    }

    fn flipped_pixels(t: &Texture) -> Vec<Vec<u32>> {
        pixels(t)
            .iter()
            .enumerate()
            .map(|(i, p)| flip_pixels(p, (t.width >> i).max(1)))
            .collect()
    }

    #[test]
    fn exact_flips_keep_the_format_and_flip_the_pixels() {
        use TexFormat::*;
        for f in [
            Bc1,
            Bc2,
            Bc3,
            Bc4Unorm,
            Bc4Snorm,
            Bc5Unorm,
            Bc5Snorm,
            Rgba8Unorm,
            B5g6r5Unorm,
            R16Float,
        ] {
            for (w, h) in [(16, 16), (8, 4), (16, 2), (4, 1), (12, 7)] {
                let t = texture(f, w, h);
                let exact_blocks = !f.is_block_compressed() || h % 4 == 0 || h < 4;
                let flipped = flip_vertical(&t, &Decoder::default()).unwrap();
                if !exact_blocks {
                    assert_ne!(flipped.format, f, "{f:?} {w}x{h} cannot flip in blocks");
                    continue;
                }
                assert_eq!(flipped.format, f, "{f:?} {w}x{h}");
                assert_eq!(pixels(&flipped), flipped_pixels(&t), "{f:?} {w}x{h}");
                assert_eq!(
                    flip_vertical(&flipped, &Decoder::default()).unwrap(),
                    t,
                    "{f:?} {w}x{h} twice"
                );
            }
        }
    }

    #[test]
    fn other_flips_decode() {
        use TexFormat::*;
        for (f, to) in [
            (Bc7, Rgba8Unorm),
            (Bc1, Rgba8Unorm),
            (Bc5Snorm, Rgba32Float),
            (Bc6hUfloat, Rgba32Float),
        ] {
            let t = texture(f, 8, 6);
            let flipped = flip_vertical(&t, &Decoder::default()).unwrap();
            assert_eq!(flipped.format, to, "{f:?}");
            assert_eq!(pixels(&flipped), flipped_pixels(&t), "{f:?}");
        }
    }
}
