//! The texture format and colour space a RAGE buffer format code holds, in the world's terms.
use primase_core::components::ColorSpace;
use primase_core::texformat::TexFormat;
use rage_engine_formats::sga::BufferFormat;

/// The texture format and colour space, for the codes a texture can hold.
pub fn texture_format(format: BufferFormat) -> Option<(TexFormat, ColorSpace)> {
    use ColorSpace::{Linear, Srgb};
    use TexFormat::*;
    Some(match format.0 {
        0x02 => (Rgba32Float, Linear),
        0x0A => (Rgba16Float, Linear),
        0x0B => (Rgba16Unorm, Linear),
        0x10 => (Rg32Float, Linear),
        0x18 => (Rgb10a2Unorm, Linear),
        0x1C => (Rgba8Unorm, Linear),
        0x1D => (Rgba8Unorm, Srgb),
        0x1F => (Rgba8Snorm, Linear),
        0x22 => (Rg16Float, Linear),
        0x23 => (Rg16Unorm, Linear),
        0x25 => (Rg16Snorm, Linear),
        0x29 => (R32Float, Linear),
        0x31 => (Rg8Unorm, Linear),
        0x33 => (Rg8Snorm, Linear),
        0x36 => (R16Float, Linear),
        0x38 => (R16Unorm, Linear),
        0x3D => (R8Unorm, Linear),
        0x3F => (R8Snorm, Linear),
        0x41 => (A8Unorm, Linear),
        0x47 => (Bc1, Linear),
        0x48 => (Bc1, Srgb),
        0x4A => (Bc2, Linear),
        0x4B => (Bc2, Srgb),
        0x4D => (Bc3, Linear),
        0x4E => (Bc3, Srgb),
        0x50 => (Bc4Unorm, Linear),
        0x51 => (Bc4Snorm, Linear),
        0x53 => (Bc5Unorm, Linear),
        0x54 => (Bc5Snorm, Linear),
        0x55 => (B5g6r5Unorm, Linear),
        0x56 => (B5g5r5a1Unorm, Linear),
        0x57 => (Bgra8Unorm, Linear),
        0x5B => (Bgra8Unorm, Srgb),
        0x5F => (Bc6hUfloat, Linear),
        0x60 => (Bc6hSfloat, Linear),
        0x62 => (Bc7, Linear),
        0x63 => (Bc7, Srgb),
        0x73 => (B4g4r4a4Unorm, Linear),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format crate's pixel layout of every code and this mapping agree: the same block size and bytes, the same
    /// colour space; and the codes a texture can hold are exactly the ones mapped.
    #[test]
    fn the_mapping_agrees_with_the_formats_pixel_layout() {
        for code in 0..=u8::MAX {
            let format = BufferFormat(code);
            let texel = format.texel();
            let mapped = texture_format(format);
            assert_eq!(texel.is_some(), mapped.is_some(), "{code:#x}");
            if let (Some(t), Some((f, space))) = (texel, mapped) {
                assert_eq!((t.block_width, t.block_height), f.block_dims(), "{code:#x}");
                assert_eq!(t.block_bytes, f.block_bytes(), "{code:#x}");
                assert_eq!(t.srgb, space == ColorSpace::Srgb, "{code:#x}");
            }
        }
    }
}
