//! Enhanced's buffer formats (`rage::sga::BufferFormat`), one byte, used by textures and vertex elements. The codes
//! follow DXGI's order for the first twenty values, then their own numbering (`B10G10R10A2_UNORM` is 0x15 where DXGI
//! has 24; `BC1_UNORM` is 0x47 like DXGI's 71). The names below are the codes the game's files use; the
//! correspondence is checked by census (every texture's data size from its format, size and levels equals its stored
//! block count times block size).
use primase_core::components::ColorSpace;
use primase_core::texformat::TexFormat;

/// A buffer format code, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferFormat(pub u8);

/// A code, its name, and the texture format and colour space it holds where one exists.
type FormatEntry = (u8, &'static str, Option<(TexFormat, ColorSpace)>);

const FORMATS: &[FormatEntry] = {
    use ColorSpace::{Linear as L, Srgb as S};
    use TexFormat::*;
    &[
        (0x02, "R32G32B32A32_FLOAT", Some((Rgba32Float, L))),
        (0x06, "R32G32B32_FLOAT", None),
        (0x0A, "R16G16B16A16_FLOAT", Some((Rgba16Float, L))),
        (0x0B, "R16G16B16A16_UNORM", Some((Rgba16Unorm, L))),
        (0x10, "R32G32_FLOAT", Some((Rg32Float, L))),
        (0x18, "R10G10B10A2_UNORM", Some((Rgb10a2Unorm, L))),
        (0x1C, "R8G8B8A8_UNORM", Some((Rgba8Unorm, L))),
        (0x1D, "R8G8B8A8_UNORM_SRGB", Some((Rgba8Unorm, S))),
        (0x1E, "R8G8B8A8_UINT", None),
        (0x1F, "R8G8B8A8_SNORM", Some((Rgba8Snorm, L))),
        (0x22, "R16G16_FLOAT", Some((Rg16Float, L))),
        (0x23, "R16G16_UNORM", Some((Rg16Unorm, L))),
        (0x25, "R16G16_SNORM", Some((Rg16Snorm, L))),
        (0x29, "R32_FLOAT", Some((R32Float, L))),
        (0x31, "R8G8_UNORM", Some((Rg8Unorm, L))),
        (0x33, "R8G8_SNORM", Some((Rg8Snorm, L))),
        (0x36, "R16_FLOAT", Some((R16Float, L))),
        (0x38, "R16_UNORM", Some((R16Unorm, L))),
        (0x3D, "R8_UNORM", Some((R8Unorm, L))),
        (0x3F, "R8_SNORM", Some((R8Snorm, L))),
        (0x41, "A8_UNORM", Some((A8Unorm, L))),
        (0x47, "BC1_UNORM", Some((Bc1, L))),
        (0x48, "BC1_UNORM_SRGB", Some((Bc1, S))),
        (0x4A, "BC2_UNORM", Some((Bc2, L))),
        (0x4B, "BC2_UNORM_SRGB", Some((Bc2, S))),
        (0x4D, "BC3_UNORM", Some((Bc3, L))),
        (0x4E, "BC3_UNORM_SRGB", Some((Bc3, S))),
        (0x50, "BC4_UNORM", Some((Bc4Unorm, L))),
        (0x51, "BC4_SNORM", Some((Bc4Snorm, L))),
        (0x53, "BC5_UNORM", Some((Bc5Unorm, L))),
        (0x54, "BC5_SNORM", Some((Bc5Snorm, L))),
        (0x55, "B5G6R5_UNORM", Some((B5g6r5Unorm, L))),
        (0x56, "B5G5R5A1_UNORM", Some((B5g5r5a1Unorm, L))),
        (0x57, "B8G8R8A8_UNORM", Some((Bgra8Unorm, L))),
        (0x5B, "B8G8R8A8_UNORM_SRGB", Some((Bgra8Unorm, S))),
        (0x5F, "BC6H_UF16", Some((Bc6hUfloat, L))),
        (0x60, "BC6H_SF16", Some((Bc6hSfloat, L))),
        (0x62, "BC7_UNORM", Some((Bc7, L))),
        (0x63, "BC7_UNORM_SRGB", Some((Bc7, S))),
        (0x73, "B4G4R4A4_UNORM", Some((B4g4r4a4Unorm, L))),
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

    /// The texture format and colour space, for codes a texture can hold.
    pub fn texture(self) -> Option<(TexFormat, ColorSpace)> {
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
