//! `TexFormat`: the one enum for how a texture's bytes are laid out, for every game and target.
//!
//! Only the layout lives here (block size, bytes per block or pixel). Colour space is not part of the format: a
//! DXGI `_SRGB` variant holds the same bits as its linear twin, so `Texture::color_space` carries it. Decoders and
//! encoders live in `primase-texcodec`; conversions from DXGI, D3DFMT FourCCs and game enums live in the crates that
//! read those (DDS in `primase-dds`, a game's own enum in its world crate).

/// How a texture's bytes are laid out.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum TexFormat {
    // ---- uncompressed, 8 bits per channel
    R8Unorm,
    R8Snorm,
    A8Unorm,
    Rg8Unorm,
    Rg8Snorm,
    Rgba8Unorm,
    Rgba8Snorm,
    Bgra8Unorm,
    /// B8G8R8X8: alpha byte ignored (decodes to 255).
    Bgrx8Unorm,
    // ---- uncompressed, packed
    /// B5G6R5 in a little-endian u16 (blue in the low bits).
    B5g6r5Unorm,
    /// B5G5R5A1 in a little-endian u16 (blue in the low bits, alpha the top bit).
    B5g5r5a1Unorm,
    /// B4G4R4A4 in a little-endian u16.
    B4g4r4a4Unorm,
    /// R10G10B10A2 in a little-endian u32 (red in the low bits).
    Rgb10a2Unorm,
    // ---- uncompressed, 16 and 32 bits per channel
    R16Unorm,
    R16Float,
    Rg16Unorm,
    Rg16Snorm,
    Rg16Float,
    Rgba16Unorm,
    Rgba16Float,
    R32Float,
    Rg32Float,
    Rgba32Float,
    // ---- BCn (4x4 blocks)
    /// BC1 (DXT1). 3-colour blocks (c0 <= c1) decode index 3 as transparent black (D3D10).
    Bc1,
    /// BC2 (DXT3): explicit 4-bit alpha.
    Bc2,
    /// BC3 (DXT5): interpolated alpha.
    Bc3,
    Bc4Unorm,
    Bc4Snorm,
    Bc5Unorm,
    Bc5Snorm,
    Bc6hUfloat,
    Bc6hSfloat,
    Bc7,
    // ---- mobile/console block formats (decoded only behind a gate)
    Etc1,
    Etc2Rgb,
    Etc2Rgba,
    Etc2RgbA1,
    EacR11Unorm,
    EacRg11Unorm,
    /// ASTC LDR with the given block footprint (4x4 to 12x12).
    Astc {
        block_w: u8,
        block_h: u8,
    },
}

impl TexFormat {
    /// Block footprint in pixels: (1, 1) for uncompressed formats.
    pub fn block_dims(self) -> (u32, u32) {
        use TexFormat::*;
        match self {
            Bc1 | Bc2 | Bc3 | Bc4Unorm | Bc4Snorm | Bc5Unorm | Bc5Snorm | Bc6hUfloat | Bc6hSfloat | Bc7
            | Etc1 | Etc2Rgb | Etc2Rgba | Etc2RgbA1 | EacR11Unorm | EacRg11Unorm => (4, 4),
            Astc { block_w, block_h } => (block_w as u32, block_h as u32),
            _ => (1, 1),
        }
    }

    /// Bytes per block (per pixel for uncompressed formats).
    pub fn block_bytes(self) -> u32 {
        use TexFormat::*;
        match self {
            R8Unorm | R8Snorm | A8Unorm => 1,
            Rg8Unorm | Rg8Snorm | B5g6r5Unorm | B5g5r5a1Unorm | B4g4r4a4Unorm | R16Unorm | R16Float => 2,
            Rgba8Unorm | Rgba8Snorm | Bgra8Unorm | Bgrx8Unorm | Rgb10a2Unorm | Rg16Unorm | Rg16Snorm
            | Rg16Float | R32Float => 4,
            Rgba16Unorm | Rgba16Float | Rg32Float => 8,
            Rgba32Float => 16,
            Bc1 | Bc4Unorm | Bc4Snorm | Etc1 | Etc2Rgb | Etc2RgbA1 | EacR11Unorm => 8,
            Bc2
            | Bc3
            | Bc5Unorm
            | Bc5Snorm
            | Bc6hUfloat
            | Bc6hSfloat
            | Bc7
            | Etc2Rgba
            | EacRg11Unorm
            | Astc { .. } => 16,
        }
    }

    pub fn is_block_compressed(self) -> bool {
        self.block_dims() != (1, 1)
    }

    /// Size in bytes of one level of `width` x `height` pixels: whole blocks, at least one per axis.
    pub fn level_size(self, width: u32, height: u32) -> usize {
        let (bw, bh) = self.block_dims();
        let blocks_x = width.div_ceil(bw).max(1) as usize;
        let blocks_y = height.div_ceil(bh).max(1) as usize;
        blocks_x * blocks_y * self.block_bytes() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::TexFormat::*;

    #[test]
    fn level_sizes_round_up_to_whole_blocks() {
        assert_eq!(Bc1.level_size(1, 1), 8);
        assert_eq!(Bc1.level_size(5, 3), 2 * 8);
        assert_eq!(Bc7.level_size(1024, 512), 256 * 128 * 16);
        assert_eq!(Rgba8Unorm.level_size(3, 2), 24);
        assert_eq!(
            (Astc {
                block_w: 6,
                block_h: 5
            })
            .level_size(13, 10),
            3 * 2 * 16
        );
    }
}
