//! PEAK's baked light volume as its `LightVolume.Start` builds it for the shaders (`_LightMap`):
//! `Peak.CompressableLightMap.Decompress` reads each z slice's colour texture (BC6H) and alpha texture (BC4) with
//! `GetPixels`, writes the alpha slice's red into the colour's alpha, lays the slices one after another and writes
//! them with `SetPixels` into a `gridRes` RGBAHalf `Texture3D` without mips ([`assemble`]).
//!
//! The volume keeps the order Unity holds it in: a slice's rows as stored (the first row is the one a shader reads at
//! v = 0) and the slices by z. A 2D texture is flipped to rows top first for the world because its meshes' v is
//! flipped with it; the programs address this volume by world position, so nothing flips it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use half::f16;
use primase_core::{ColorSpace, TexFormat, TexRole, TextureLayers, TextureShape};
use primase_texcodec::{Decoder, TexDecoder};
use unity_formats::player::{ObjRef, Player};
use unity_formats::texture::{TextureFormat, UnityTexture};

use crate::read::scripts::{CompressableLightMap, PPtr};
use crate::{Error, Result};

fn refuse(detail: String) -> Error {
    Error::Refused(format!("light volume: {detail}"))
}

/// One slice texture's first level: the slice's stored reference resolved in `file`, held to `format` and the
/// volume's slice size.
fn slice(player: &Player, file: &str, p: PPtr, format: TextureFormat, size: (u32, u32)) -> Result<Vec<f32>> {
    let r: ObjRef = player.resolve(file, &super::pptr_value(p))?.ok_or_else(|| {
        refuse(format!(
            "a slice reference ({}, {}) names no object",
            p.file_id, p.path_id
        ))
    })?;
    let t = UnityTexture::read(player, &r)?;
    let (stored, levels) = t.levels()?;
    if stored != format || (t.width, t.height) != size {
        return Err(refuse(format!(
            "slice {}/{} is {stored:?} {} x {}, not the {format:?} {} x {} Decompress reads",
            r.file, r.path_id, t.width, t.height, size.0, size.1
        )));
    }
    let first = levels
        .first()
        .ok_or_else(|| refuse(format!("slice {}/{} holds no level", r.file, r.path_id)))?;
    let codec = match format {
        TextureFormat::Bc6h => TexFormat::Bc6hUfloat,
        _ => TexFormat::Bc4Unorm,
    };
    Decoder::default()
        .decode_rgba32f(codec, t.width, t.height, first)
        .map_err(|e| refuse(format!("slice {}/{}: {e}", r.file, r.path_id)))
}

/// A value as `SetPixels` stores it in an RGBAHalf texture: the nearest half, ties to even.
fn half_bytes(v: f32) -> [u8; 2] {
    f16::from_f32(v).to_bits().to_le_bytes()
}

/// The `_LightMap` volume `Decompress` builds from `light_map` (a `LightVolume`'s in `file` of `player`): RGBAHalf,
/// `gridRes` cells, one level. Refused where the stored light map is not what it reads to a whole volume: slice
/// counts other than `gridRes.z` (a missing slice would be left black, one more is an error in the game), slices not
/// BC6H colour and BC4 alpha (`Decompress` then scales the colour, a path the game logs as unfinished) or not
/// `gridRes.x` x `gridRes.y`.
pub fn assemble(player: &Player, file: &str, light_map: &CompressableLightMap) -> Result<TextureLayers> {
    let size = light_map.grid_res.map(u32::try_from);
    let [Ok(x), Ok(y), Ok(z)] = size else {
        return Err(refuse(format!("gridRes {:?}", light_map.grid_res)));
    };
    if x == 0 || y == 0 || z == 0 {
        return Err(refuse(format!("gridRes {:?}", light_map.grid_res)));
    }
    let depth = usize::try_from(z).map_err(|_| refuse(format!("gridRes {:?}", light_map.grid_res)))?;
    if light_map.rgb_slices.len() != depth || light_map.alpha_slices.len() != depth {
        return Err(refuse(format!(
            "{} colour and {} alpha slices for a depth of {depth}",
            light_map.rgb_slices.len(),
            light_map.alpha_slices.len()
        )));
    }
    let mut layers = Vec::new();
    layers
        .try_reserve_exact(depth)
        .map_err(|_| refuse("no memory for the slices".into()))?;
    for (&rgb, &alpha) in light_map.rgb_slices.iter().zip(&light_map.alpha_slices) {
        let colour = slice(player, file, rgb, TextureFormat::Bc6h, (x, y))?;
        let alpha = slice(player, file, alpha, TextureFormat::Bc4, (x, y))?;
        let mut layer = Vec::new();
        layer
            .try_reserve_exact(colour.len().saturating_mul(2))
            .map_err(|_| refuse("no memory for a slice".into()))?;
        for (c, a) in colour.as_chunks::<4>().0.iter().zip(alpha.as_chunks::<4>().0) {
            let [r, g, b, _] = *c;
            for v in [r, g, b, a[0]] {
                layer.extend(half_bytes(v));
            }
        }
        layers.push(layer);
    }
    Ok(TextureLayers {
        shape: TextureShape::Volume { depth: z },
        width: x,
        height: y,
        format: TexFormat::Rgba16Float,
        levels: vec![layers],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SetPixels` into RGBAHalf rounds to the nearest half, ties to even: the BC4 alpha's 1/255 and 128/255, a value
    /// past a half's range and 0.
    #[test]
    fn a_value_is_stored_as_its_nearest_half() {
        assert_eq!(half_bytes(0.0), [0, 0]);
        assert_eq!(half_bytes(1.0), 0x3c00u16.to_le_bytes());
        assert_eq!(half_bytes(1.0 / 255.0), 0x1c04u16.to_le_bytes());
        assert_eq!(half_bytes(128.0 / 255.0), 0x3804u16.to_le_bytes());
        assert_eq!(half_bytes(70000.0), 0x7c00u16.to_le_bytes());
    }
}
