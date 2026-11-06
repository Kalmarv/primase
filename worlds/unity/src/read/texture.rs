//! Unity textures (`Texture2D`) as world textures: Unity's TextureFormat ids onto [`TexFormat`], colour space, how the
//! texture is used.
//!
//! Unity stores rows bottom first (row 0 is v = 0, its texture origin being bottom-left); the world's textures are top
//! first with their origin top-left (and the mesh reader stores v as 1 - v), so every texture is flipped
//! ([`primase_texcodec::flip_vertical`]: exact in place for uncompressed and BC1-5 whole-block levels, decoded
//! otherwise). That gives back the image as authored, which Unity's importer flipped.

use primase_core::{
    ColorSpace, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole, Texture,
};
use primase_texcodec::{Decoder, flip_vertical};
use unity_formats::texture::{TextureFormat, USAGE_NORMAL_DXT5NM, USAGE_NORMAL_PLAIN, UnityTexture};
use unity_formats::{Error, Result};

use crate::invalid;

/// The normal map convention a texture's usage mode says, if it is one: Unity's normal maps are green up with Z rebuilt
/// (URP unpacks X from red times alpha, which is alpha for DXT5nm and red for plain maps).
pub fn normal(t: &UnityTexture) -> Option<NormalConvention> {
    let x = match t.usage {
        USAGE_NORMAL_DXT5NM => NormalX::Alpha,
        USAGE_NORMAL_PLAIN => NormalX::Red,
        _ => return None,
    };
    Some(NormalConvention {
        green: NormalGreen::Up,
        z: NormalZ::Rebuild,
        x,
        basis: NormalBasis::TangentBitangent,
    })
}

/// How a stored level becomes the world format's bytes.
#[derive(Clone, Copy)]
enum Convert {
    /// The bytes as stored.
    Same,
    /// RGB24: an alpha of 255 added.
    Rgb8,
    /// RGB24_SIGNED: an alpha of 127 (1.0) added.
    Rgb8Snorm,
    /// ARGB32: A, R, G, B to R, G, B, A.
    Argb8,
    /// RGBA4444: the 16-bit value's nibbles R, G, B, A (R on top) to bytes.
    Rgba4,
    /// RGB48: an alpha of 65535 added.
    Rgb16,
    /// RGB9e5Float: the shared-exponent value to four 32-bit floats (alpha 1).
    Rgb9e5,
}

impl Convert {
    fn level(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Convert::Same => bytes.to_vec(),
            Convert::Rgb8 => bytes
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [r, g, b, 255])
                .collect(),
            Convert::Rgb8Snorm => bytes
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [r, g, b, 127])
                .collect(),
            Convert::Argb8 => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[a, r, g, b]| [r, g, b, a])
                .collect(),
            Convert::Rgba4 => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&p| {
                    let v = u16::from_le_bytes(p);
                    // a nibble n is n * 17 of 255
                    [12, 8, 4, 0].map(|shift| ((v >> shift) & 0xf) as u8 * 17)
                })
                .collect(),
            Convert::Rgb16 => bytes
                .as_chunks::<6>()
                .0
                .iter()
                .flat_map(|p| p.iter().copied().chain([0xff, 0xff]))
                .collect(),
            Convert::Rgb9e5 => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&p| {
                    let v = u32::from_le_bytes(p);
                    // each 9-bit mantissa times 2^(exponent - 15 - 9), exact in an f32
                    let scale = f32::from_bits(((v >> 27) + 127 - 24) << 23);
                    [0, 9, 18]
                        .map(|shift| ((v >> shift) & 0x1ff) as f32 * scale)
                        .into_iter()
                        .chain([1.0f32])
                        .flat_map(f32::to_le_bytes)
                })
                .collect(),
        }
    }
}

/// The world format of a stored one and how its bytes become it; `None` for a format the world has none for.
fn world_format(stored: TextureFormat) -> Option<(TexFormat, Convert)> {
    use Convert::*;
    use TexFormat::*;
    use TextureFormat as U;
    Some(match stored {
        U::Alpha8 => (A8Unorm, Same),
        U::Argb4444 => (B4g4r4a4Unorm, Same),
        U::Rgb24 => (Rgba8Unorm, Rgb8),
        U::Rgba32 => (Rgba8Unorm, Same),
        U::Argb32 => (Rgba8Unorm, Argb8),
        U::Rgb565 => (B5g6r5Unorm, Same),
        U::R16 => (R16Unorm, Same),
        U::Dxt1 => (Bc1, Same),
        U::Dxt5 => (Bc3, Same),
        U::Rgba4444 => (Rgba8Unorm, Rgba4),
        U::Bgra32 => (Bgra8Unorm, Same),
        U::RHalf => (R16Float, Same),
        U::RgHalf => (Rg16Float, Same),
        U::RgbaHalf => (Rgba16Float, Same),
        U::RFloat => (R32Float, Same),
        U::RgFloat => (Rg32Float, Same),
        U::RgbaFloat => (Rgba32Float, Same),
        U::Rgb9e5Float => (Rgba32Float, Rgb9e5),
        U::Bc6h => (Bc6hUfloat, Same),
        U::Bc7 => (Bc7, Same),
        U::Bc4 => (Bc4Unorm, Same),
        U::Bc5 => (Bc5Unorm, Same),
        U::EtcRgb4 => (Etc1, Same),
        U::EacR => (EacR11Unorm, Same),
        U::EacRg => (EacRg11Unorm, Same),
        U::Etc2Rgb => (Etc2Rgb, Same),
        U::Etc2Rgba1 => (Etc2RgbA1, Same),
        U::Etc2Rgba8 => (Etc2Rgba, Same),
        U::Astc { side, hdr: false } => {
            let side = u8::try_from(side).ok()?;
            (
                Astc {
                    block_w: side,
                    block_h: side,
                },
                Same,
            )
        }
        U::Rg16 => (Rg8Unorm, Same),
        U::R8 => (R8Unorm, Same),
        U::Rg32 => (Rg16Unorm, Same),
        U::Rgb48 => (Rgba16Unorm, Rgb16),
        U::Rgba64 => (Rgba16Unorm, Same),
        U::R8Signed => (R8Snorm, Same),
        U::Rg16Signed => (Rg8Snorm, Same),
        U::Rgb24Signed => (Rgba8Snorm, Rgb8Snorm),
        U::Rgba32Signed => (Rgba8Snorm, Same),
        U::Rg32Signed => (Rg16Snorm, Same),
        // YUY2 needs a colour matrix the file does not state; ATC, signed EAC, HDR ASTC and the signed 16-bit one- and
        // three-channel formats have no world format; `levels` gives crunched ones unpacked
        _ => return None,
    })
}

/// `t` as a world texture, rows top first: normal maps (by usage mode) linear with their convention, others colour
/// (sRGB) or other (linear) until a material says more. RGB24 and RGB48 are widened with an opaque alpha, ARGB32 and
/// RGBA4444 reordered to RGBA8, RGB9e5 decoded to RGBA32 floats (each exact). Refused: an empty texture (a dynamic
/// font's, no pixels until the game draws glyphs), and the formats the world has none for (YUY2, ATC, signed EAC, HDR
/// ASTC, R16 and RGB48 signed).
pub fn texture(t: &UnityTexture) -> Result<Texture> {
    let (stored, stored_levels) = t.levels()?;
    if stored_levels.is_empty() {
        return Err(Error::Unsupported(format!(
            "{}: an empty texture ({}x{}, no pixels)",
            t.name, t.width, t.height
        )));
    }
    let (format, convert) = world_format(stored).ok_or_else(|| {
        Error::Unsupported(format!("{}: texture format {stored:?} (id {})", t.name, t.format))
    })?;
    let levels = stored_levels.iter().map(|bytes| convert.level(bytes)).collect();
    let normal = normal(t);
    let bottom_first = Texture {
        width: t.width,
        height: t.height,
        format,
        levels,
        color_space: if t.srgb && normal.is_none() {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        },
        role: match (normal, t.srgb) {
            (Some(_), _) => TexRole::Normal,
            (None, true) => TexRole::Color,
            (None, false) => TexRole::Other,
        },
        normal,
    };
    flip_vertical(&bottom_first, &Decoder::default()).map_err(|e| invalid(format!("{}: {e}", t.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unity_texture(width: u32, height: u32, mips: u32, format: i64, data: Vec<u8>) -> UnityTexture {
        UnityTexture {
            name: "t".into(),
            width,
            height,
            format,
            mips,
            srgb: false,
            usage: 0,
            wrap: [0, 0],
            filter: 1,
            mip_bias: 0.0,
            aniso: 1,
            data,
            unity: unity_formats::UnityVersion::new(2021, 3, 45, 3, 2),
        }
    }

    /// The bytes of a converted 1x1 texture's level, before the flip (one row flips to itself).
    fn one_pixel(format: i64, data: Vec<u8>) -> (TexFormat, Vec<u8>) {
        let t = texture(&unity_texture(1, 1, 1, format, data)).unwrap();
        (t.format, t.levels[0].clone())
    }

    /// Each conversion keeps the stored values exactly: channels reordered to RGBA, nibbles widened by 17, RGB48 and
    /// RGB24_SIGNED given an opaque alpha, and RGB9e5's shared exponent applied (R 0x1ff of exponent 15 + 1 is
    /// 511 / 256, G 1 is 1 / 256, B 0).
    #[test]
    fn formats_convert_to_their_world_bytes() {
        assert_eq!(
            one_pixel(5, vec![9, 1, 2, 3]),
            (TexFormat::Rgba8Unorm, vec![1, 2, 3, 9])
        );
        // RGBA4444: R 0xa, G 0x5, B 0xf, A 0x0
        assert_eq!(
            one_pixel(13, 0xa5f0u16.to_le_bytes().to_vec()),
            (TexFormat::Rgba8Unorm, vec![170, 85, 255, 0])
        );
        assert_eq!(
            one_pixel(73, vec![1, 2, 3, 4, 5, 6]),
            (TexFormat::Rgba16Unorm, vec![1, 2, 3, 4, 5, 6, 255, 255])
        );
        assert_eq!(
            one_pixel(77, vec![1, 2, 3]),
            (TexFormat::Rgba8Snorm, vec![1, 2, 3, 127])
        );
        let e9 = 0x1ff | (1 << 9) | (16 << 27);
        let (format, bytes) = one_pixel(22, u32::to_le_bytes(e9).to_vec());
        let floats: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(
            (format, floats),
            (TexFormat::Rgba32Float, vec![511.0 / 256.0, 1.0 / 256.0, 0.0, 1.0])
        );
        for (id, format) in [
            (7, TexFormat::B5g6r5Unorm),
            (9, TexFormat::R16Unorm),
            (14, TexFormat::Bgra8Unorm),
        ] {
            let bytes = vec![7; format.level_size(1, 1)];
            assert_eq!(one_pixel(id, bytes.clone()), (format, bytes), "format {id}");
        }
    }

    /// A format the world has none for and an empty texture are refused by name.
    #[test]
    fn formats_without_a_world_format_are_refused() {
        let e = texture(&unity_texture(2, 1, 1, 21, vec![0; 4])).unwrap_err();
        assert!(
            matches!(e, Error::Unsupported(_)) && e.to_string().contains("Yuy2"),
            "{e}"
        );
        let e = texture(&unity_texture(0, 0, 1, 1, vec![])).unwrap_err();
        assert!(
            matches!(e, Error::Unsupported(_)) && e.to_string().contains("empty"),
            "{e}"
        );
    }

    /// Sizes the file states are held to its pixels: levels past 32, sizes that overflow and levels that cannot fit the
    /// data are errors naming them, not a panic or an allocation.
    #[test]
    fn stated_sizes_are_held_to_the_pixels() {
        let rgba = 4;
        assert!(texture(&unity_texture(2, 2, 1, rgba, vec![0; 16])).is_ok());
        assert!(texture(&unity_texture(2, 2, 2, rgba, vec![0; 20])).is_ok());
        let e = texture(&unity_texture(2, 2, 33, rgba, vec![0; 16]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("33 mip levels"), "{e}");
        let e = texture(&unity_texture(u32::MAX, u32::MAX, 1, rgba, vec![0; 16]))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("past the 16 bytes of pixels") || e.contains("does not fit"),
            "{e}"
        );
        // a block format with huge sides: the level size is checked, not expected
        assert!(texture(&unity_texture(u32::MAX, u32::MAX, 32, 12, vec![0; 16])).is_err());
        assert!(texture(&unity_texture(0, 4, 1, rgba, vec![])).is_err());
    }
}
