//! PEAK's script fields, decoded from the bytes a MonoBehaviour stores after its base fields
//! ([`unity_formats::scene::UnityBehaviour::data`]; the player stores no type tree for them). Each layout is the
//! script's serialized fields in declaration order (the game's own `Assembly-CSharp`), named for the player's Unity
//! version it was measured on. Bytes that are not that layout (another length) are refused with what was found, never
//! guessed at; another build's layout is a new named layout.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::{Error, Result};

/// The Unity version of the PEAK player these layouts were measured on (its level4): `Peak.FogShaderParams`,
/// `LightVolume` and `FogConfig`.
pub const MEASURED: &str = "6000.3.15f1";

fn refuse(what: &str, version: &str, detail: String) -> Error {
    Error::Layout(format!(
        "{what}: {detail}: not the layout measured on Unity {MEASURED} (this player: {version})"
    ))
}

/// An object reference as a script stores it: the file index (0 this file, else one past its index in the file's
/// externals) and the path id; a path id of 0 is no object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PPtr {
    pub file_id: i32,
    pub path_id: i64,
}

/// A stored `PPtr` at the start of `bytes`: a 32-bit file id and a 64-bit path id, little-endian.
fn pptr(bytes: &[u8]) -> Option<PPtr> {
    let file_id = i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let path_id = i64::from_le_bytes(bytes.get(4..12)?.try_into().ok()?);
    Some(PPtr { file_id, path_id })
}

/// `Peak.FogShaderParams.fogTexture`, from its script bytes: on Unity [`MEASURED`] its one serialized field, a
/// `Texture` reference (12 bytes); `Start` hands it to every shader as `FogTexture`.
pub fn fog_shader_params(version: &str, data: &[u8]) -> Result<PPtr> {
    const WHAT: &str = "FogShaderParams";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    if data.len() != 12 {
        return Err(refuse(
            WHAT,
            version,
            format!("{} script bytes, not 12", data.len()),
        ));
    }
    pptr(data).ok_or_else(|| refuse(WHAT, version, "no texture reference".into()))
}

/// Little-endian fields read in order from a script's bytes; `None` past the end.
struct Fields<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let b = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(b)
    }

    fn word(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
    }

    fn f32(&mut self) -> Option<f32> {
        self.word().map(f32::from_le_bytes)
    }

    fn i32(&mut self) -> Option<i32> {
        self.word().map(i32::from_le_bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.word().map(u32::from_le_bytes)
    }

    fn f32x3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }

    /// A `bool`: one byte, 0 or 1, padded to four.
    fn bool(&mut self) -> Option<bool> {
        let b = self.take(4)?;
        match b.first()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn pptr(&mut self) -> Option<PPtr> {
        pptr(self.take(12)?)
    }

    /// An array of references: a 32-bit count, then the references; a negative count is none.
    fn pptrs(&mut self) -> Option<Vec<PPtr>> {
        let n = usize::try_from(self.i32()?).ok()?;
        // no more than the bytes left can hold
        if n > self.bytes.len().checked_sub(self.at)? / 12 {
            return None;
        }
        (0..n).map(|_| self.pptr()).collect()
    }
}

/// `Peak.CompressableLightMap`, the baked light volume a `LightVolume` holds, as stored: the volume cut into z slices,
/// each a 2D texture for the colour and one for the alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct CompressableLightMap {
    /// `hdrDecompressScalar`: what `Decompress` multiplies the colour by when the colour slices are not BC6H.
    pub hdr_decompress_scalar: f32,
    /// `rgbMapSlices`: one Texture2D a z slice, its colour.
    pub rgb_slices: Vec<PPtr>,
    /// `alphaChannelSlices`: one Texture2D a z slice, whose red is the volume's alpha.
    pub alpha_slices: Vec<PPtr>,
    /// `gridRes`: the volume's size in cells.
    pub grid_res: [i32; 3],
    /// `gridOffset`: the world position of the volume's corner.
    pub grid_offset: [f32; 3],
    /// `gridCenter`.
    pub grid_center: [f32; 3],
}

/// `LightVolume`'s fields that its `Start` hands every shader, and its light map.
#[derive(Clone, Debug, PartialEq)]
pub struct LightVolume {
    pub brightness: f32,
    pub ambience_strength: f32,
    pub ambience_min: f32,
    /// `raySpacing`: the size of a cell in world units.
    pub ray_spacing: f32,
    /// `gridRes` as the component stores it (`Start` replaces it with the light map's).
    pub grid_res: [u32; 3],
    /// `gridOffset` as the component stores it (`Start` replaces it with the light map's).
    pub grid_offset: [f32; 3],
    /// `lightMapData`.
    pub light_map: CompressableLightMap,
}

/// `LightVolume`, from its script bytes: on Unity [`MEASURED`] its serialized fields in declaration order
/// (`showVolumeGizmos`, `brightness`, `ambienceStrength`, `ambienceMin`, `skyColor`, `filterModeRT`, `textureFormat`,
/// `rayCount`, `raySpacing`, `occluderMask`, `blurRadius`, `gridRes`, `gridOffset`, `gridCenter`, four references
/// (`sceneParent` and the bake's three shaders), then `lightMapData`'s `hdrDecompressScalar`, `rgbMapSlices`,
/// `alphaChannelSlices`, `gridRes`, `gridOffset`, `gridCenter`): 188 bytes and 12 for each slice reference, every
/// byte read.
pub fn light_volume(version: &str, data: &[u8]) -> Result<LightVolume> {
    const WHAT: &str = "LightVolume";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    let mut f = Fields { bytes: data, at: 0 };
    let read = |f: &mut Fields| -> Option<LightVolume> {
        f.bool()?; // showVolumeGizmos
        let (brightness, ambience_strength, ambience_min) = (f.f32()?, f.f32()?, f.f32()?);
        f.take(16)?; // skyColor
        f.take(12)?; // filterModeRT, textureFormat, rayCount
        let ray_spacing = f.f32()?;
        f.take(8)?; // occluderMask, blurRadius
        let grid_res = [f.u32()?, f.u32()?, f.u32()?];
        let grid_offset = f.f32x3()?;
        f.f32x3()?; // gridCenter
        for _ in 0..4 {
            f.pptr()?;
        }
        let light_map = CompressableLightMap {
            hdr_decompress_scalar: f.f32()?,
            rgb_slices: f.pptrs()?,
            alpha_slices: f.pptrs()?,
            grid_res: [f.i32()?, f.i32()?, f.i32()?],
            grid_offset: f.f32x3()?,
            grid_center: f.f32x3()?,
        };
        Some(LightVolume {
            brightness,
            ambience_strength,
            ambience_min,
            ray_spacing,
            grid_res,
            grid_offset,
            light_map,
        })
    };
    let volume = read(&mut f).ok_or_else(|| {
        refuse(
            WHAT,
            version,
            format!(
                "{} script bytes end inside a field (or a bool is not 0 or 1)",
                data.len()
            ),
        )
    })?;
    if f.at != data.len() {
        return Err(refuse(
            WHAT,
            version,
            format!(
                "{} script bytes, {} past the fields",
                data.len(),
                data.len().saturating_sub(f.at)
            ),
        ));
    }
    Ok(volume)
}

/// `FogConfig`'s wind texture, from its script bytes: on Unity [`MEASURED`] its serialized fields in declaration order
/// (`windSkyBrightnessValue`, `windTextureInfluence`, `windTint`, `windTexture`, `windSphereScale`, `windSpeed`,
/// `windFogDensity`, `WindFogTextureDensity`, `windMixInfluence`, `maxVal`, `straightDown`, `zone`,
/// `useWindChillZoneIntensity`; the private `sinceSet` is not stored): 84 bytes, every byte read. Its `SetFog` hands
/// the wind fields to every shader; its `Start` sets `_WeatherBlend` to 0.
pub fn fog_config(version: &str, data: &[u8]) -> Result<PPtr> {
    const WHAT: &str = "FogConfig";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    let mut f = Fields { bytes: data, at: 0 };
    let read = |f: &mut Fields| -> Option<PPtr> {
        f.take(8)?; // windSkyBrightnessValue, windTextureInfluence
        f.take(16)?; // windTint
        let texture = f.pptr()?;
        f.take(4)?; // windSphereScale
        f.take(8)?; // windSpeed
        f.take(16)?; // windFogDensity, WindFogTextureDensity, windMixInfluence, maxVal
        f.bool()?; // straightDown
        f.pptr()?; // zone
        f.bool()?; // useWindChillZoneIntensity
        Some(texture)
    };
    let texture = read(&mut f).ok_or_else(|| {
        refuse(
            WHAT,
            version,
            format!(
                "{} script bytes end inside a field (or a bool is not 0 or 1)",
                data.len()
            ),
        )
    })?;
    if f.at != data.len() {
        return Err(refuse(
            WHAT,
            version,
            format!(
                "{} script bytes, {} past the fields",
                data.len(),
                data.len().saturating_sub(f.at)
            ),
        ));
    }
    Ok(texture)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;

    /// The one field is the texture reference; any other length or Unity version is refused, saying which.
    #[test]
    fn fog_shader_params_is_one_texture_reference() {
        let mut data = 3i32.to_le_bytes().to_vec();
        data.extend(621_203i64.to_le_bytes());
        assert_eq!(
            fog_shader_params(MEASURED, &data).unwrap(),
            PPtr {
                file_id: 3,
                path_id: 621_203
            }
        );
        let short = fog_shader_params(MEASURED, &data[..8]).unwrap_err().to_string();
        assert!(short.contains("8 script bytes, not 12"), "{short}");
        let mut long = data.clone();
        long.extend([0; 4]);
        assert!(fog_shader_params(MEASURED, &long).is_err());
        let other = fog_shader_params("6000.3.17f1", &data).unwrap_err().to_string();
        assert!(
            other.contains("another Unity version") && other.contains("6000.3.17f1"),
            "{other}"
        );
    }

    /// A light volume of two slices, its fields at their places: every byte read; a byte more or less, a bool that is
    /// not 0 or 1, or a count past the bytes is refused.
    #[test]
    fn light_volume_is_its_fields_then_its_light_map() {
        let mut d = Vec::new();
        let f = |d: &mut Vec<u8>, v: f32| d.extend(v.to_le_bytes());
        let i = |d: &mut Vec<u8>, v: i32| d.extend(v.to_le_bytes());
        let p = |d: &mut Vec<u8>, path: i64| {
            d.extend(0i32.to_le_bytes());
            d.extend(path.to_le_bytes());
        };
        d.extend([1, 0, 0, 0]); // showVolumeGizmos
        for v in [3.0, 1.25, 0.15] {
            f(&mut d, v);
        }
        for v in [0.8, 0.8, 0.8, 1.0] {
            f(&mut d, v);
        }
        for v in [1, 17, 256] {
            i(&mut d, v);
        }
        f(&mut d, 5.0);
        i(&mut d, -1);
        i(&mut d, 2);
        for v in [4, 8, 2] {
            i(&mut d, v);
        }
        for v in [-1.0, -2.0, -3.0, 9.0, 9.0, 9.0] {
            f(&mut d, v);
        }
        for path in [7, 0, 0, 0] {
            p(&mut d, path);
        }
        f(&mut d, 1.0);
        for slices in [[10, 11], [20, 21]] {
            i(&mut d, 2);
            for path in slices {
                p(&mut d, path);
            }
        }
        for v in [4, 8, 2] {
            i(&mut d, v);
        }
        for v in [-1.5, -2.5, -3.5, 8.0, 8.0, 8.0] {
            f(&mut d, v);
        }
        assert_eq!(d.len(), 188 + 4 * 12);
        let v = light_volume(MEASURED, &d).unwrap();
        assert_eq!(
            (v.brightness, v.ambience_strength, v.ambience_min),
            (3.0, 1.25, 0.15)
        );
        assert_eq!(
            (v.ray_spacing, v.grid_res, v.grid_offset),
            (5.0, [4, 8, 2], [-1.0, -2.0, -3.0])
        );
        let slice = |path_id| PPtr { file_id: 0, path_id };
        assert_eq!(
            v.light_map,
            CompressableLightMap {
                hdr_decompress_scalar: 1.0,
                rgb_slices: vec![slice(10), slice(11)],
                alpha_slices: vec![slice(20), slice(21)],
                grid_res: [4, 8, 2],
                grid_offset: [-1.5, -2.5, -3.5],
                grid_center: [8.0; 3],
            }
        );
        let mut long = d.clone();
        long.push(0);
        let e = light_volume(MEASURED, &long).unwrap_err().to_string();
        assert!(e.contains("1 past the fields"), "{e}");
        let e = light_volume(MEASURED, &d[..d.len() - 1]).unwrap_err().to_string();
        assert!(e.contains("end inside a field"), "{e}");
        let mut not_bool = d.clone();
        not_bool[0] = 2;
        assert!(light_volume(MEASURED, &not_bool).is_err());
        // the colour slices' count, past what the bytes hold
        let mut count = d.clone();
        count[144..148].copy_from_slice(&1000i32.to_le_bytes());
        assert!(light_volume(MEASURED, &count).is_err());
        assert!(light_volume("2022.3.1f1", &d).is_err());
    }

    /// The wind texture is the fourth field; 84 bytes are read to the last, and a byte more or less is refused.
    #[test]
    fn fog_config_is_its_wind_fields() {
        let mut d = vec![0u8; 24];
        d.extend(0i32.to_le_bytes());
        d.extend(60i64.to_le_bytes());
        d.extend([0u8; 28]); // windSphereScale, windSpeed and four floats
        d.extend([1, 0, 0, 0]);
        d.extend(0i32.to_le_bytes());
        d.extend(0i64.to_le_bytes());
        d.extend([0, 0, 0, 0]);
        assert_eq!(d.len(), 84);
        assert_eq!(
            fog_config(MEASURED, &d).unwrap(),
            PPtr {
                file_id: 0,
                path_id: 60
            }
        );
        let mut long = d.clone();
        long.extend([0; 4]);
        assert!(
            fog_config(MEASURED, &long)
                .unwrap_err()
                .to_string()
                .contains("4 past the fields")
        );
        assert!(fog_config(MEASURED, &d[..80]).is_err());
    }
}
