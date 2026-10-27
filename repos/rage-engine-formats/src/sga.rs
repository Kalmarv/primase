//! Enhanced's buffer formats (`sga::BufferFormat`), one byte, used by textures and vertex elements. The codes
//! follow DXGI's order for the first twenty values, then their own numbering (`B10G10R10A2_UNORM` is 0x15 where DXGI
//! has 24; `BC1_UNORM` is 0x47 like DXGI's 71). The names below are the codes the game's files use; the
//! correspondence is checked by census (every texture's data size from its format, size and levels equals its stored
//! block count times block size).

/// A buffer format code, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferFormat(pub u8);

/// How a texture of some format lays out its pixels: whole blocks of `block_width` x `block_height` texels, each
/// `block_bytes` bytes (one texel per block for the uncompressed formats).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Texel {
    pub block_width: u32,
    pub block_height: u32,
    pub block_bytes: u32,
    /// The colour values are sRGB-encoded (the `_SRGB` codes); otherwise linear.
    pub srgb: bool,
}

impl Texel {
    const fn pixel(bytes: u32) -> Texel {
        Texel {
            block_width: 1,
            block_height: 1,
            block_bytes: bytes,
            srgb: false,
        }
    }

    const fn block(bytes: u32) -> Texel {
        Texel {
            block_width: 4,
            block_height: 4,
            block_bytes: bytes,
            srgb: false,
        }
    }

    const fn srgb(self) -> Texel {
        Texel { srgb: true, ..self }
    }

    /// The size in bytes of one level of `width` x `height` texels: whole blocks, at least one per axis. `None` when
    /// it does not fit in a `usize`.
    pub fn level_size(self, width: u32, height: u32) -> Option<usize> {
        let blocks_x = usize::try_from(width.div_ceil(self.block_width).max(1)).ok()?;
        let blocks_y = usize::try_from(height.div_ceil(self.block_height).max(1)).ok()?;
        blocks_x
            .checked_mul(blocks_y)?
            .checked_mul(usize::try_from(self.block_bytes).ok()?)
    }
}

/// A code, its name, and the texel layout where a texture can hold it.
type FormatEntry = (u8, &'static str, Option<Texel>);

const FORMATS: &[FormatEntry] = {
    use Texel as T;
    &[
        (0x02, "R32G32B32A32_FLOAT", Some(T::pixel(16))),
        (0x06, "R32G32B32_FLOAT", None),
        (0x0A, "R16G16B16A16_FLOAT", Some(T::pixel(8))),
        (0x0B, "R16G16B16A16_UNORM", Some(T::pixel(8))),
        (0x10, "R32G32_FLOAT", Some(T::pixel(8))),
        (0x18, "R10G10B10A2_UNORM", Some(T::pixel(4))),
        (0x1C, "R8G8B8A8_UNORM", Some(T::pixel(4))),
        (0x1D, "R8G8B8A8_UNORM_SRGB", Some(T::pixel(4).srgb())),
        (0x1E, "R8G8B8A8_UINT", None),
        (0x1F, "R8G8B8A8_SNORM", Some(T::pixel(4))),
        (0x22, "R16G16_FLOAT", Some(T::pixel(4))),
        (0x23, "R16G16_UNORM", Some(T::pixel(4))),
        (0x25, "R16G16_SNORM", Some(T::pixel(4))),
        (0x29, "R32_FLOAT", Some(T::pixel(4))),
        (0x31, "R8G8_UNORM", Some(T::pixel(2))),
        (0x33, "R8G8_SNORM", Some(T::pixel(2))),
        (0x36, "R16_FLOAT", Some(T::pixel(2))),
        (0x38, "R16_UNORM", Some(T::pixel(2))),
        (0x3D, "R8_UNORM", Some(T::pixel(1))),
        (0x3F, "R8_SNORM", Some(T::pixel(1))),
        (0x41, "A8_UNORM", Some(T::pixel(1))),
        (0x47, "BC1_UNORM", Some(T::block(8))),
        (0x48, "BC1_UNORM_SRGB", Some(T::block(8).srgb())),
        (0x4A, "BC2_UNORM", Some(T::block(16))),
        (0x4B, "BC2_UNORM_SRGB", Some(T::block(16).srgb())),
        (0x4D, "BC3_UNORM", Some(T::block(16))),
        (0x4E, "BC3_UNORM_SRGB", Some(T::block(16).srgb())),
        (0x50, "BC4_UNORM", Some(T::block(8))),
        (0x51, "BC4_SNORM", Some(T::block(8))),
        (0x53, "BC5_UNORM", Some(T::block(16))),
        (0x54, "BC5_SNORM", Some(T::block(16))),
        (0x55, "B5G6R5_UNORM", Some(T::pixel(2))),
        (0x56, "B5G5R5A1_UNORM", Some(T::pixel(2))),
        (0x57, "B8G8R8A8_UNORM", Some(T::pixel(4))),
        (0x5B, "B8G8R8A8_UNORM_SRGB", Some(T::pixel(4).srgb())),
        (0x5F, "BC6H_UF16", Some(T::block(16))),
        (0x60, "BC6H_SF16", Some(T::block(16))),
        (0x62, "BC7_UNORM", Some(T::block(16))),
        (0x63, "BC7_UNORM_SRGB", Some(T::block(16).srgb())),
        (0x73, "B4G4R4A4_UNORM", Some(T::pixel(2))),
    ]
};

impl BufferFormat {
    fn entry(self) -> Option<&'static FormatEntry> {
        FORMATS.iter().find(|f| f.0 == self.0)
    }

    /// The format's name, for codes the files are known to use.
    pub fn name(self) -> Option<&'static str> {
        self.entry().map(|f| f.1)
    }

    /// The pixel layout, for codes a texture can hold.
    pub fn texel(self) -> Option<Texel> {
        self.entry().and_then(|f| f.2)
    }

    /// The size of one vertex element of this format, for the codes vertex declarations use.
    pub fn element_size(self) -> Option<usize> {
        Some(match self.0 {
            0x02 => 16,
            0x06 => 12,
            0x0A | 0x10 => 8,
            0x18 | 0x1C | 0x1E | 0x22 => 4,
            _ => return None,
        })
    }
}
