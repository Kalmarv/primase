//! Reading a DDS file: its header as DirectXTex's `DecodeDDSHeader` reads it, then its payload, exactly.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::formats::{FOURCC, PixelFormat, from_dxgi, legacy_format};
use crate::{AlphaMode, Dds, Error, HeaderKind, Parsed, Result, Shape, extent, invalid};

fn word(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..)
        .and_then(<[u8]>::first_chunk::<4>)
        .map(|w| u32::from_le_bytes(*w))
        .ok_or_else(|| invalid(o, "truncated header"))
}

/// The bytes of one mip chain or volume level, as `u64` so that a header's sizes can't overflow before they're
/// compared with the file's length.
fn level_bytes(format: primase_core::TexFormat, width: u32, height: u32, level: u32) -> Result<u64> {
    format
        .checked_level_size(extent(width, level), extent(height, level))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| invalid(16, "level size overflow"))
}

/// The payload a shape needs: every item's whole mip chain (1D, 2D and cube textures), or every level's depth slices
/// (volumes).
fn payload_bytes(
    shape: Shape,
    width: u32,
    height: u32,
    format: primase_core::TexFormat,
    mips: u32,
) -> Result<u64> {
    let overflow = || invalid(16, "payload size overflow");
    let mut total = 0_u64;
    for level in 0..mips {
        let bytes = level_bytes(format, width, height, level)?;
        let images = shape.images_at(level);
        total = bytes
            .checked_mul(images)
            .and_then(|b| total.checked_add(b))
            .ok_or_else(overflow)?;
    }
    Ok(total)
}

/// The largest mip count an image of this size has: down to 1 x 1 (x 1).
fn max_levels(largest: u32) -> u32 {
    largest.checked_ilog2().unwrap_or(0).saturating_add(1)
}

/// Read a DDS file. See the crate documentation for what is read as DirectXTex reads it and what is stricter.
pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    if bytes.len() < 128 {
        return Err(invalid(
            bytes.len(),
            "truncated header (a DDS header is 128 bytes)",
        ));
    }
    if bytes.get(..4) != Some(b"DDS ") {
        return Err(invalid(0, "bad magic"));
    }
    let size = word(bytes, 4)?;
    if size != 124 {
        return Err(invalid(4, format!("header size {size}, not 124")));
    }
    let pf_size = word(bytes, 76)?;
    if pf_size != 32 {
        return Err(invalid(76, format!("pixel format size {pf_size}, not 32")));
    }
    let flags = word(bytes, 8)?;
    let (height, width, depth) = (word(bytes, 12)?, word(bytes, 16)?, word(bytes, 24)?);
    let mips = word(bytes, 28)?.max(1);
    let fourcc = bytes
        .get(84..88)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .ok_or_else(|| invalid(84, "truncated header"))?;
    let pf = PixelFormat {
        flags: word(bytes, 80)?,
        fourcc,
        bits: word(bytes, 88)?,
        masks: [
            word(bytes, 92)?,
            word(bytes, 96)?,
            word(bytes, 100)?,
            word(bytes, 104)?,
        ],
    };
    let caps2 = word(bytes, 112)?;
    const DEPTH: u32 = 0x80_0000;

    let (shape, height, format, color_space, alpha_mode, header, start) =
        if pf.flags & FOURCC != 0 && &pf.fourcc == b"DX10" {
            if bytes.len() < 148 {
                return Err(invalid(bytes.len(), "truncated DX10 header"));
            }
            let (format, color_space) = from_dxgi(word(bytes, 128)?)?;
            let dimension = word(bytes, 132)?;
            let misc = word(bytes, 136)?;
            let array = word(bytes, 140)?.max(1);
            let alpha_mode = AlphaMode::from_misc2(word(bytes, 144)?)?;
            if misc & !4 != 0 {
                return Err(Error::Unsupported(format!(
                    "DX10 misc flags {misc:#x} beyond TEXTURECUBE"
                )));
            }
            let (shape, height) = match dimension {
                // D3DX writes 1D textures with a height of 1; DirectXTex refuses any other stated height.
                2 => {
                    if flags & 2 != 0 && height != 1 {
                        return Err(invalid(12, format!("1D texture with height {height}")));
                    }
                    (Shape::D1 { layers: array }, 1)
                }
                3 if misc & 4 != 0 => (Shape::Cube { cubes: array }, height),
                3 => (Shape::D2 { layers: array }, height),
                4 => {
                    if flags & DEPTH == 0 {
                        return Err(invalid(8, "3D texture without the DDSD_DEPTH flag"));
                    }
                    if array > 1 {
                        return Err(Error::Unsupported(format!(
                            "3D texture arrays (arraySize {array})"
                        )));
                    }
                    (Shape::D3 { depth }, height)
                }
                d => return Err(invalid(132, format!("resource dimension {d}"))),
            };
            (
                shape,
                height,
                format,
                color_space,
                alpha_mode,
                HeaderKind::Dx10,
                148_usize,
            )
        } else {
            let nvtt = bytes.get(68..72) == Some(b"NVTT");
            let (format, color_space, alpha_mode) = legacy_format(&pf, nvtt)?;
            let shape = if flags & DEPTH != 0 {
                Shape::D3 { depth }
            } else if caps2 & 0x200 != 0 {
                if caps2 & 0xfe00 != 0xfe00 {
                    return Err(Error::Unsupported(format!(
                        "cube map without all six faces (caps2 {caps2:#x})"
                    )));
                }
                Shape::Cube { cubes: 1 }
            } else {
                Shape::D2 { layers: 1 }
            };
            (
                shape,
                height,
                format,
                color_space,
                alpha_mode,
                HeaderKind::Legacy(pf),
                128_usize,
            )
        };

    if width == 0 || height == 0 {
        return Err(invalid(12, format!("{width} x {height} pixels")));
    }
    let largest = match shape {
        Shape::D3 { depth: 0 } => return Err(invalid(24, "zero depth")),
        Shape::D3 { depth } => width.max(height).max(depth),
        _ => width.max(height),
    };
    if mips > max_levels(largest) {
        return Err(invalid(
            28,
            format!("{mips} mip levels for {width} x {height} {shape:?}"),
        ));
    }

    let remaining = u64::try_from(bytes.len().saturating_sub(start)).unwrap_or(u64::MAX);
    let needed = payload_bytes(shape, width, height, format, mips)?;
    let mut shape = shape;
    let mut faces_counted = false;
    if needed != remaining {
        // A DX10 cube whose arraySize counts faces (6 per cube), not cubes: DirectXTex's permissive load reads it
        // as arraySize / 6 cubes when the stated count overruns the file. Here the smaller count has to account
        // for the payload exactly, so the two readings can't both fit.
        let recount = match shape {
            Shape::Cube { cubes } if header == HeaderKind::Dx10 && needed > remaining && cubes % 6 == 0 => {
                Some(Shape::Cube { cubes: cubes / 6 })
            }
            _ => None,
        };
        match recount {
            Some(s) if payload_bytes(s, width, height, format, mips)? == remaining => {
                shape = s;
                faces_counted = true;
            }
            _ if needed > remaining => {
                return Err(invalid(
                    bytes.len(),
                    format!("truncated payload: {shape:?} needs {needed} bytes, the file has {remaining}"),
                ));
            }
            _ => {
                return Err(invalid(
                    start.saturating_add(usize::try_from(needed).unwrap_or(usize::MAX)),
                    format!("{} trailing bytes", remaining.saturating_sub(needed)),
                ));
            }
        }
    }

    let payload = bytes.get(start..).unwrap_or_default();
    let mut at = 0_usize;
    let mut take = |len: u64| -> Result<Vec<u8>> {
        let len = usize::try_from(len).map_err(|_| invalid(start, "image exceeds memory"))?;
        let end = at
            .checked_add(len)
            .ok_or_else(|| invalid(start, "payload offset overflow"))?;
        let image = payload
            .get(at..end)
            .ok_or_else(|| invalid(start.saturating_add(at), "truncated payload"))?;
        at = end;
        Ok(image.to_vec())
    };
    let mut levels: Vec<Vec<Vec<u8>>> = (0..mips).map(|_| Vec::new()).collect();
    match shape {
        Shape::D3 { .. } => {
            for (level, images) in (0_u32..).zip(levels.iter_mut()) {
                let bytes = level_bytes(format, width, height, level)?;
                for _ in 0..shape.images_at(level) {
                    images.push(take(bytes)?);
                }
            }
        }
        // 1D, 2D and cube textures store each item (array layer, or cube face +X, -X, +Y, -Y, +Z, -Z) with its mips.
        _ => {
            for _ in 0..shape.images_at(0) {
                for (level, images) in (0_u32..).zip(levels.iter_mut()) {
                    images.push(take(level_bytes(format, width, height, level)?)?);
                }
            }
        }
    }
    Ok(Parsed {
        dds: Dds {
            shape,
            width,
            height,
            format,
            color_space,
            alpha_mode,
            levels,
        },
        header,
        faces_counted,
    })
}
