//! 3D layout descriptions: NVIDIA's NV_texture_compression_vtc (2004, revision4),
//! and RPCS3's image_utils.hpp/TextureUtils.cpp at979a52415003fd7a74ddd216e1a5782b3731281f.
//! <https://registry.khronos.org/OpenGL/extensions/NV/NV_texture_compression_vtc.txt>
//! Uncompressed swizzle interleaves available X,Y,Z bits, skipping exhausted axes.
//! Compressed POT volumes group four S3TC Z planes per spatial block (VTC),
//! reducing to two or one at the end of a mip chain. Layouts were described by
//! those references; no RPCS3 implementation code is copied here.
use super::{CellGcmTexture, Error, Result, add, expand_level, morton3, mul, validate_remap};
use primase_core::{ColorSpace, TexFormat, TexRole};

/// A decoded 3D texture, distinct from core's 2D `Texture`. No slice selection or
/// flattening is implied. Identity BC1/BC3 remain compressed, per 2D slice;
/// other formats/remaps become straight RGBA8, without colour-space conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub format: TexFormat,
    pub color_space: ColorSpace,
    pub role: TexRole,
    pub levels: Vec<VolumeLevel>,
}

/// One mip, with tightly packed top-first rows, consecutive slices in increasing
/// Z order. Length is `format.level_size(width,height) * depth`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeLevel {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeSubresource {
    pub mip: u8,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub offset: usize,
    pub size: usize,
    /// Encoded row pitch; packed for swizzled/VTC, base pitch for LN.
    pub pitch: usize,
}

impl CellGcmTexture {
    /// Full volume or separately stored mip tail. Each mip shrinks all three axes.
    /// POT swizzled ARGB/B8 and POT VTC BC1/BC3 are supported. Linear uncompressed
    /// volumes use constant base pitch. Linear compressed and non-POT swizzled
    /// volumes are refused: their RSX storage has not been validated here.
    pub fn volume_layout(self, first_mip: u8) -> Result<Vec<VolumeSubresource>> {
        let source = self.source_format()?;
        if self.dimension != 3 || self.depth == 0 {
            return Err(Error::Dimension {
                dimension: self.dimension,
                depth: self.depth,
            });
        }
        if self.cubemap != 0 {
            return Err(Error::Invalid("a volume cannot be a cubemap"));
        }
        if self.width == 0 || self.height == 0 {
            return Err(Error::Invalid("zero volume extent"));
        }
        let largest = self.width.max(self.height).max(self.depth);
        if self.mipmap == 0 || u32::from(self.mipmap) > largest.ilog2() + 1 || first_mip >= self.mipmap {
            return Err(Error::Invalid("volume mip count or first mip"));
        }
        if !self.linear()
            && (!self.width.is_power_of_two()
                || !self.height.is_power_of_two()
                || !self.depth.is_power_of_two())
        {
            return Err(Error::Invalid("swizzled volume extents must be power of two"));
        }
        if self.linear() && source.is_block_compressed() {
            return Err(Error::Invalid("linear compressed volume storage is unproven"));
        }
        validate_remap(self.remap)?;
        let (bw, bh) = source.block_dims();
        let bytes = source.block_bytes() as usize;
        let base_row = mul(u32::from(self.width).div_ceil(bw) as usize, bytes)?;
        if self.linear() && ((self.pitch as usize) < base_row || !(self.pitch as usize).is_multiple_of(bytes))
        {
            return Err(Error::Invalid(
                "volume base pitch smaller than row or not a whole texel",
            ));
        }
        let mut levels = Vec::new();
        let mut offset = 0;
        for mip in first_mip..self.mipmap {
            let width = u32::from(self.width >> mip).max(1);
            let height = u32::from(self.height >> mip).max(1);
            let depth = u32::from(self.depth >> mip).max(1);
            let pitch = if self.linear() {
                self.pitch as usize
            } else {
                mul(width.div_ceil(bw) as usize, bytes)?
            };
            let size = mul(mul(pitch, height.div_ceil(bh) as usize)?, depth as usize)?;
            levels.push(VolumeSubresource {
                mip,
                width,
                height,
                depth,
                offset,
                size,
                pitch,
            });
            offset = add(offset, size)?;
        }
        Ok(levels)
    }

    /// Decode a volume explicitly, preserving depth and every requested mip. GPU
    /// address metadata is ignored as in `decode`; bytes must end at the last mip.
    pub fn decode_volume(
        self,
        data: &[u8],
        first_mip: u8,
        color_space: ColorSpace,
        role: TexRole,
    ) -> Result<Volume> {
        let layout = self.volume_layout(first_mip)?;
        let last = layout.last().ok_or(Error::Invalid("empty volume layout"))?;
        let expected = add(last.offset, last.size)?;
        if data.len() != expected {
            return Err(Error::Length {
                expected,
                actual: data.len(),
            });
        }
        let source = self.source_format()?;
        let format = if source.is_block_compressed() && self.remap == 0xaae4 {
            source
        } else {
            TexFormat::Rgba8Unorm
        };
        let mut levels = Vec::new();
        for l in layout {
            let src = &data[l.offset..l.offset + l.size];
            let plane = source
                .checked_level_size(l.width, l.height)
                .ok_or(Error::Overflow)?;
            let mut packed = crate::buffer(mul(plane, l.depth as usize)?)?;
            let bytes = source.block_bytes() as usize;
            if source.is_block_compressed() {
                let blocks = plane / bytes;
                for z in 0..l.depth as usize {
                    let group_start = z / 4 * 4;
                    let group_depth = (l.depth as usize - group_start).min(4);
                    for block in 0..blocks {
                        let from = (group_start * blocks + block * group_depth + z - group_start) * bytes;
                        let to = z * plane + block * bytes;
                        packed[to..to + bytes].copy_from_slice(&src[from..from + bytes]);
                    }
                }
            } else if self.linear() {
                let row = l.width as usize * bytes;
                for z in 0..l.depth as usize {
                    for y in 0..l.height as usize {
                        let from = (z * l.height as usize + y) * l.pitch;
                        let to = z * plane + y * row;
                        packed[to..to + row].copy_from_slice(&src[from..from + row]);
                    }
                }
            } else {
                for z in 0..l.depth {
                    for y in 0..l.height {
                        for x in 0..l.width {
                            let from = morton3([x, y, z], [l.width, l.height, l.depth]) * bytes;
                            let to =
                                z as usize * plane + (y as usize * l.width as usize + x as usize) * bytes;
                            packed[to..to + bytes].copy_from_slice(&src[from..from + bytes]);
                        }
                    }
                }
            }
            let output_plane = format
                .checked_level_size(l.width, l.height)
                .ok_or(Error::Overflow)?;
            let mut output = crate::buffer(mul(output_plane, l.depth as usize)?)?;
            for z in 0..l.depth as usize {
                let mut pixels = crate::buffer(plane)?;
                pixels.copy_from_slice(&packed[z * plane..(z + 1) * plane]);
                let pixels = expand_level(source, l.width, l.height, pixels, self.remap)?;
                output[z * output_plane..(z + 1) * output_plane].copy_from_slice(&pixels);
            }
            levels.push(VolumeLevel {
                width: l.width,
                height: l.height,
                depth: l.depth,
                data: output,
            });
        }
        Ok(Volume {
            format,
            color_space,
            role,
            levels,
        })
    }
}
