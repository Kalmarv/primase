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
use unity_formats::Result;
use unity_formats::texture::{TextureFormat, USAGE_NORMAL_DXT5NM, USAGE_NORMAL_PLAIN, UnityTexture};

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

/// `t` as a world texture, rows top first: normal maps (by usage mode) linear with their convention, others colour
/// (sRGB) or other (linear) until a material says more. RGB24 is widened to RGBA8 (alpha 255).
pub fn texture(t: &UnityTexture) -> Result<Texture> {
    let (stored, stored_levels) = t.levels()?;
    use TexFormat::*;
    let format = match stored {
        TextureFormat::Alpha8 => A8Unorm,
        TextureFormat::Argb4444 => B4g4r4a4Unorm,
        TextureFormat::Rgb24 | TextureFormat::Rgba32 => Rgba8Unorm,
        TextureFormat::Dxt1 => Bc1,
        TextureFormat::Dxt5 => Bc3,
        TextureFormat::Bc6h => Bc6hUfloat,
        TextureFormat::Bc7 => Bc7,
        TextureFormat::Bc4 => Bc4Unorm,
        TextureFormat::Bc5 => Bc5Unorm,
        TextureFormat::R8 => R8Unorm,
    };
    let levels = stored_levels
        .into_iter()
        .map(|bytes| match stored {
            TextureFormat::Rgb24 => bytes
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [r, g, b, 255])
                .collect(),
            _ => bytes.to_vec(),
        })
        .collect();
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
        }
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
