//! Sony CellGcmTexture byte layout and RSX texture storage (no GPU memory access).
//!
//! Descriptor fields are big endian; DXT blocks retain their little-endian BCn encoding.
//! LN (0x20) selects constant base pitch; UN (0x40) selects *unnormalized* coordinates
//! and does not change pixel storage. Compressed levels are block-linear even without LN.
//! Cubes are +X, -X, +Y, -Y, +Z, -Z, each with its mip chain; non-LN faces start
//! on 128-byte boundaries. There is no required padding after the final face.
//!
//! Public references for libgcm's vocabulary and the consumer's layout:
//! <https://github.com/ps3dev/PSL1GHT/blob/master/ppu/include/rsx/gcm_sys.h>
//! (`gcmTexture`, `GCM_TEXTURE_REMAP_MODE`) and
//! <https://github.com/RPCS3/rpcs3/blob/979a52415003fd7a74ddd216e1a5782b3731281f/rpcs3/Emu/RSX/Common/TextureUtils.cpp>
//! (`get_subresources_layout_impl`). 3D swizzle is deliberately unsupported.

use crate::{Decoder, TexDecoder};
use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("CellGcmTexture requires 24 bytes, got {0}")]
    DescriptorLength(usize),
    #[error("unsupported GCM texture format 0x{0:02x}")]
    Format(u8),
    #[error("unsupported GCM texture dimension {dimension}, depth {depth} (3D swizzle unproven)")]
    Dimension { dimension: u8, depth: u16 },
    #[error("invalid GCM texture: {0}")]
    Invalid(&'static str),
    #[error("unsupported GCM remap 0x{0:08x}")]
    Remap(u32),
    #[error("GCM pixel length: expected {expected}, got {actual}")]
    Length { expected: usize, actual: usize },
    #[error("GCM layout overflows address space")]
    Overflow,
    #[error(transparent)]
    Decode(#[from] crate::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// The 24-byte descriptor, including GPU address metadata. `offset` and `location`
/// are preserved but never used to index the supplied texture payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGcmTexture {
    pub format: u8,
    pub mipmap: u8,
    pub dimension: u8,
    pub cubemap: u8,
    pub remap: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub location: u8,
    pub pad: u8,
    pub pitch: u32,
    pub offset: u32,
}

/// One encoded level's range, relative to the supplied payload's start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subresource {
    pub face: u8,
    pub mip: u8,
    pub width: u32,
    pub height: u32,
    pub offset: usize,
    pub size: usize,
    pub pitch: usize,
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or(Error::Overflow)
}
fn mul(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).ok_or(Error::Overflow)
}

impl CellGcmTexture {
    /// Parse exactly one descriptor as data; validate supported storage when decoding.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let b: &[u8; 24] = data.try_into().map_err(|_| Error::DescriptorLength(data.len()))?;
        let u16_at = |i| u16::from_be_bytes([b[i], b[i + 1]]);
        let u32_at = |i| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(Self {
            format: b[0],
            mipmap: b[1],
            dimension: b[2],
            cubemap: b[3],
            remap: u32_at(4),
            width: u16_at(8),
            height: u16_at(10),
            depth: u16_at(12),
            location: b[14],
            pad: b[15],
            pitch: u32_at(16),
            offset: u32_at(20),
        })
    }

    pub fn linear(self) -> bool {
        self.format & 0x20 != 0
    }
    pub fn unnormalized_coordinates(self) -> bool {
        self.format & 0x40 != 0
    }

    fn source_format(self) -> Result<TexFormat> {
        match self.format & !0x60 {
            0x81 => Ok(TexFormat::R8Unorm),
            0x85 => Ok(TexFormat::Rgba8Unorm),
            0x86 => Ok(TexFormat::Bc1),
            0x88 => Ok(TexFormat::Bc3),
            _ => Err(Error::Format(self.format)),
        }
    }

    /// Layout of a complete chain (`first_mip = 0`) or a separately stored tail.
    /// A tail starts at byte zero, retains the descriptor's base pitch for LN,
    /// and aligns its cube faces independently. No address relocation is applied.
    pub fn layout(self, first_mip: u8) -> Result<Vec<Subresource>> {
        let format = self.source_format()?;
        if self.dimension != 2 || self.depth > 1 {
            return Err(Error::Dimension {
                dimension: self.dimension,
                depth: self.depth,
            });
        }
        if self.width == 0 || self.height == 0 {
            return Err(Error::Invalid("zero extent"));
        }
        if self.cubemap > 1 || (self.cubemap == 1 && self.width != self.height) {
            return Err(Error::Invalid("cube flag or nonsquare cube"));
        }
        if self.mipmap == 0
            || u32::from(self.mipmap) > self.width.max(self.height).ilog2() + 1
            || first_mip >= self.mipmap
        {
            return Err(Error::Invalid("mip count or first mip"));
        }
        if !self.linear()
            && !format.is_block_compressed()
            && (!self.width.is_power_of_two() || !self.height.is_power_of_two())
        {
            return Err(Error::Invalid(
                "uncompressed swizzled extent must be power of two",
            ));
        }
        validate_remap(self.remap)?;
        let (bw, bh) = format.block_dims();
        let bytes = format.block_bytes() as usize;
        let base_row = mul(u32::from(self.width).div_ceil(bw) as usize, bytes)?;
        if self.linear() && ((self.pitch as usize) < base_row || !(self.pitch as usize).is_multiple_of(bytes))
        {
            return Err(Error::Invalid("base pitch smaller than row or not a whole block"));
        }
        let mut offset = 0;
        let mut levels = Vec::new();
        for face in 0..if self.cubemap == 1 { 6 } else { 1 } {
            if face > 0 && !self.linear() {
                offset = add(offset, 127)? & !127;
            }
            for mip in first_mip..self.mipmap {
                let width = u32::from(self.width >> mip).max(1);
                let height = u32::from(self.height >> mip).max(1);
                let row = mul(width.div_ceil(bw) as usize, bytes)?;
                let pitch = if self.linear() { self.pitch as usize } else { row };
                let size = mul(pitch, height.div_ceil(bh) as usize)?;
                levels.push(Subresource {
                    face,
                    mip,
                    width,
                    height,
                    offset,
                    size,
                    pitch,
                });
                offset = add(offset, size)?;
            }
        }
        Ok(levels)
    }

    /// Produce core textures (one per face) with top-first rows and straight RGBA.
    /// Caller supplies colour meaning; no colour transfer or normal reconstruction
    /// is done. The caller can attach its normal convention to the returned textures.
    /// Identity-remapped BC1/BC3 stay compressed; other remaps expand to RGBA8.
    /// The payload must end exactly after the final level (no final-face padding).
    pub fn decode(
        self,
        data: &[u8],
        first_mip: u8,
        color_space: ColorSpace,
        role: TexRole,
    ) -> Result<Vec<Texture>> {
        let layout = self.layout(first_mip)?;
        let last = layout.last().ok_or(Error::Invalid("empty layout"))?;
        let expected = add(last.offset, last.size)?;
        if data.len() != expected {
            return Err(Error::Length {
                expected,
                actual: data.len(),
            });
        }
        let source = self.source_format()?;
        let keep_bc = source.is_block_compressed() && self.remap == 0xaae4;
        let format = if keep_bc { source } else { TexFormat::Rgba8Unorm };
        let mut faces: Vec<Texture> = Vec::new();
        for level in &layout {
            if faces.len() == level.face as usize {
                faces.push(Texture {
                    width: level.width,
                    height: level.height,
                    format,
                    levels: Vec::new(),
                    color_space,
                    role,
                    normal: None,
                });
            }
            let src = &data[level.offset..level.offset + level.size];
            let mut packed = crate::buffer(source.level_size(level.width, level.height))?;
            let bytes = source.block_bytes() as usize;
            if !self.linear() && !source.is_block_compressed() {
                for y in 0..level.height {
                    for x in 0..level.width {
                        let from = morton(x, y, level.width, level.height) * bytes;
                        let to = (y as usize * level.width as usize + x as usize) * bytes;
                        packed[to..to + bytes].copy_from_slice(&src[from..from + bytes]);
                    }
                }
            } else {
                let row = level.width.div_ceil(source.block_dims().0) as usize * bytes;
                for (y, dst) in packed.chunks_exact_mut(row).enumerate() {
                    dst.copy_from_slice(&src[y * level.pitch..y * level.pitch + row]);
                }
            }
            let pixels = if keep_bc {
                packed
            } else {
                let mut rgba = if source.is_block_compressed() {
                    Decoder::default().decode_rgba8(source, level.width, level.height, &packed)?
                } else if bytes == 1 {
                    packed.iter().flat_map(|&b| [b, b, b, 255]).collect()
                } else {
                    packed
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|b| [b[1], b[2], b[3], b[0]])
                        .collect()
                };
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    *pixel = remap_pixel(self.remap, *pixel);
                }
                rgba
            };
            faces[level.face as usize].levels.push(pixels);
        }
        Ok(faces)
    }
}

// Interleave the available low x/y bits; a rectangular surface's remaining
// bits belong only to its longer axis (there are no holes in its storage).
fn morton(x: u32, y: u32, width: u32, height: u32) -> usize {
    let mut result = 0;
    let mut out = 0;
    for bit in 0..width.max(height).ilog2() {
        if (1 << bit) < width {
            result |= (((x >> bit) & 1) as usize) << out;
            out += 1;
        }
        if (1 << bit) < height {
            result |= (((y >> bit) & 1) as usize) << out;
            out += 1;
        }
    }
    result
}

fn validate_remap(remap: u32) -> Result<()> {
    // The order bit (16) affects two-component formats, none supported here.
    if remap & !0x1ffff != 0 || (0..4).any(|c| (remap >> (8 + c * 2)) & 3 == 3) {
        return Err(Error::Remap(remap));
    }
    Ok(())
}

/// libgcm remap: input selectors in bits 0..7, output controls in 8..15,
/// each two bits in A,R,G,B order. Inputs: A=0,R=1,G=2,B=3;
/// controls: ZERO=0, ONE=1, REMAP=2 (3 is reserved). Bit 16 selects
/// XYXY/XXXY ordering for two-component formats; it has no effect here.
/// Argument and result use core RGBA order. Identity is 0xAAE4.
pub fn apply_remap(remap: u32, rgba: [u8; 4]) -> Result<[u8; 4]> {
    validate_remap(remap)?;
    Ok(remap_pixel(remap, rgba))
}

fn remap_pixel(remap: u32, rgba: [u8; 4]) -> [u8; 4] {
    let argb = [rgba[3], rgba[0], rgba[1], rgba[2]];
    let mut out = [0; 4];
    for (channel, value) in out.iter_mut().enumerate() {
        *value = match (remap >> (8 + channel * 2)) & 3 {
            0 => 0,
            1 => 255,
            _ => argb[((remap >> (channel * 2)) & 3) as usize],
        };
    }
    [out[1], out[2], out[3], out[0]]
}
