//! Which texture formats 8-bit RGBA holds exactly, and `lower-textures-rgba8`, the pass that converts the others,
//! each conversion named and counted.
//!
//! [`texture_level_rgba8`](crate::texture_level_rgba8) decodes every format to RGBA8: values wider than 8 bits are
//! rounded, HDR values clamped to 0-1, signed values mapped to `v * 0.5 + 0.5`. For the formats [`rgba8_exact`]
//! accepts, that decode is the format's meaning; for the others it is an approximation, which a writer embedding
//! 8-bit images does not make on its own: a program converts them first with this pass.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, Name, TexFormat, TexRole, Texture, World};
use primase_texcodec::{Decoder, TexDecoder};

use crate::texture::level_dimensions;

/// Whether every texel of `format` decodes to 8-bit RGBA without changing its value: 8-bit unsigned normalized
/// channels (missing ones read as 0, a missing alpha as 1); 4-bit ones (`v * 17`); BC1-BC5 unsigned and BC7, decoded
/// to 8 bits per the D3D specification, as the GPU samples them; ETC1 and ETC2's 8-bit colour. Not exact: wider
/// channels (16-bit, 10-bit, the 5- and 6-bit ones of packed 16-bit formats, EAC's 11-bit, ASTC), floating point
/// (16- and 32-bit, BC6H: values outside 0-1 and finer than 1/255) and signed formats (values below 0).
pub fn rgba8_exact(format: TexFormat) -> bool {
    use TexFormat::*;
    match format {
        R8Unorm | A8Unorm | Rg8Unorm | Rgba8Unorm | Bgra8Unorm | Bgrx8Unorm | B4g4r4a4Unorm => true,
        Bc1 | Bc2 | Bc3 | Bc4Unorm | Bc5Unorm | Bc7 => true,
        Etc1 | Etc2Rgb | Etc2Rgba | Etc2RgbA1 => true,
        R8Snorm | Rg8Snorm | Rgba8Snorm | Rg16Snorm | Bc4Snorm | Bc5Snorm => false,
        R16Float | Rg16Float | Rgba16Float | R32Float | Rg32Float | Rgba32Float | Bc6hUfloat | Bc6hSfloat => {
            false
        }
        B5g6r5Unorm | B5g5r5a1Unorm | Rgb10a2Unorm | R16Unorm | Rg16Unorm | Rgba16Unorm => false,
        EacR11Unorm | EacRg11Unorm | Astc { .. } => false,
        // a format added later is not exact until shown to be
        _ => false,
    }
}

/// How a format not exact in 8-bit RGBA is converted to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conversion {
    /// Unsigned values wider than 8 bits, rounded.
    Rounded,
    /// Floating point: clamped to 0-1, then rounded.
    Clamped,
    /// Signed: `v * 0.5 + 0.5`, then rounded (glTF's encoding of a normal map).
    Signed,
}

fn conversion(format: TexFormat) -> Option<Conversion> {
    use TexFormat::*;
    if rgba8_exact(format) {
        return None;
    }
    Some(match format {
        R8Snorm | Rg8Snorm | Rgba8Snorm | Rg16Snorm | Bc4Snorm | Bc5Snorm => Conversion::Signed,
        R16Float | Rg16Float | Rgba16Float | R32Float | Rg32Float | Rgba32Float | Bc6hUfloat | Bc6hSfloat => {
            Conversion::Clamped
        }
        _ => Conversion::Rounded,
    })
}

/// What `lower-textures-rgba8` does with a signed texture that is not a normal map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignedData {
    /// Refuse it: its values below 0 have no 0-1 form.
    #[default]
    Refuse,
    /// Remap it as a normal map is, `v * 0.5 + 0.5`: what reads it must undo that.
    Remap,
}

/// `lower-textures-rgba8{[signed-data=remap]}`: every texture whose format [`rgba8_exact`] refuses, converted to
/// [`TexFormat::Rgba8Unorm`] at every stored level, its colour space, role and normal convention kept: unsigned
/// values wider than 8 bits rounded, floating point clamped to 0-1 and rounded (each texel with a channel outside
/// 0-1 counted), signed normal maps in glTF's unsigned normal encoding (`v * 0.5 + 0.5`). A signed texture that is
/// not a normal map is refused unless `signed-data=remap` asks for the same remap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerTexturesRgba8 {
    pub signed_data: SignedData,
}

impl LowerTexturesRgba8 {
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut pass = LowerTexturesRgba8::default();
        for (k, v) in o {
            match (k.as_str(), v.as_str()) {
                ("signed-data", "remap") => pass.signed_data = SignedData::Remap,
                ("signed-data", "refuse") => pass.signed_data = SignedData::Refuse,
                ("signed-data", v) => return Err(format!("signed-data={v} is not remap or refuse")),
                (k, _) => return Err(format!("unknown option {k}")),
            }
        }
        Ok(pass)
    }
}

impl Pass for LowerTexturesRgba8 {
    fn name(&self) -> &'static str {
        "lower-textures-rgba8"
    }
    fn options(&self) -> Options {
        match self.signed_data {
            SignedData::Refuse => Options::new(),
            SignedData::Remap => Options::from([("signed-data".to_owned(), "remap".to_owned())]),
        }
    }
    // no `requires`: a world (or part) without textures has nothing to convert
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    fn approximates(&self) -> &'static str {
        "textures converted to 8-bit RGBA: wider values rounded, floating point clamped to 0-1, signed normal maps in \
         glTF's unsigned encoding"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let jobs: Vec<(Entity, Conversion)> = w
            .iter::<Texture>()
            .filter_map(|(e, t)| conversion(t.format).map(|c| (e, c)))
            .collect();
        let mut done = Vec::with_capacity(jobs.len());
        for (e, c) in jobs {
            let t = w.get::<Texture>(e).expect("listed");
            let what = || w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone());
            if c == Conversion::Signed && t.role != TexRole::Normal && self.signed_data == SignedData::Refuse
            {
                return Err(format!(
                    "{}: a signed {:?} texture that is not a normal map has no 0-1 form; signed-data=remap stores \
                     v * 0.5 + 0.5",
                    what(),
                    t.format
                ));
            }
            let mut levels = Vec::with_capacity(t.levels.len());
            let mut clamped = 0;
            for (i, data) in t.levels.iter().enumerate() {
                let (width, height) = level_dimensions(t, i).map_err(|err| format!("{}: {err}", what()))?;
                if c == Conversion::Clamped {
                    let values = Decoder::default()
                        .decode_rgba32f(t.format, width, height, data)
                        .map_err(|err| format!("{}: level {i}: {err}", what()))?;
                    clamped += values
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|p| p.iter().any(|v| !(0.0..=1.0).contains(v)))
                        .count() as u64;
                }
                levels.push(
                    Decoder::default()
                        .decode_rgba8(t.format, width, height, data)
                        .map_err(|err| format!("{}: level {i}: {err}", what()))?,
                );
            }
            match c {
                Conversion::Rounded => {
                    log.approximate("textures with channels wider than 8 bits, rounded to 8", 1)
                }
                Conversion::Clamped => {
                    log.approximate("floating-point textures clamped to 0-1 and rounded to 8 bits", 1);
                    if clamped > 0 {
                        log.approximate(
                            "floating-point texels with a channel outside 0-1, clamped",
                            clamped,
                        );
                    }
                }
                Conversion::Signed if t.role == TexRole::Normal => {
                    log.approximate("signed normal maps stored in glTF's unsigned normal encoding", 1)
                }
                Conversion::Signed => {
                    log.approximate("signed textures remapped to v * 0.5 + 0.5 (signed-data=remap)", 1)
                }
            }
            if t.role == TexRole::Data {
                log.approximate("data textures (read as numbers) converted to 8 bits", 1);
            }
            done.push((
                e,
                Texture {
                    format: TexFormat::Rgba8Unorm,
                    levels,
                    ..t.clone()
                },
            ));
        }
        for (e, t) in done {
            w.insert(e, t);
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use primase_core::ColorSpace;

    fn texture(format: TexFormat, role: TexRole, levels: Vec<Vec<u8>>, size: u32) -> Texture {
        Texture {
            width: size,
            height: size,
            format,
            levels,
            color_space: ColorSpace::Linear,
            role,
            normal: None,
        }
    }

    fn run(pass: LowerTexturesRgba8, t: Texture) -> Result<(Texture, PassLog), String> {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, t);
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log)?;
        Ok((w.get::<Texture>(e).unwrap().clone(), log))
    }

    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// 16-bit values rounded: 0x8000 is 32768 / 65535 * 255 = 127.502, so 128; 0x8080 is 128 * 257, exactly 128.
    /// Every stored level is converted.
    #[test]
    fn wider_values_are_rounded_at_every_level() {
        let level0 = u16s(&[0x8000, 0, 0xFFFF, 0x8080].repeat(4));
        let level1 = u16s(&[0, 0xFFFF, 0, 0xFFFF]);
        let (t, log) = run(
            LowerTexturesRgba8::default(),
            texture(TexFormat::Rgba16Unorm, TexRole::Color, vec![level0, level1], 2),
        )
        .unwrap();
        assert_eq!(t.format, TexFormat::Rgba8Unorm);
        assert_eq!(t.levels, [[128, 0, 255, 128].repeat(4), vec![0, 255, 0, 255]]);
        assert_eq!(
            log.approximations["textures with channels wider than 8 bits, rounded to 8"],
            1
        );
    }

    /// Half floats 2, -0.5, 0.25 and 1 clamp to 255 and 0, and 0.25 * 255 = 63.75 rounds to 64: one texel clamped.
    #[test]
    fn floating_point_is_clamped_and_counted() {
        let texel = u16s(&[0x4000, 0xB800, 0x3400, 0x3C00]);
        let (t, log) = run(
            LowerTexturesRgba8::default(),
            texture(TexFormat::Rgba16Float, TexRole::Color, vec![texel], 1),
        )
        .unwrap();
        assert_eq!(t.levels, [[255, 0, 64, 255]]);
        assert_eq!(
            log.approximations["floating-point texels with a channel outside 0-1, clamped"],
            1
        );
    }

    /// Snorm 127, -127 (0x81), 0 and 127 are 1, -1, 0 and 1; v * 0.5 + 0.5 of them is 255, 0, 127.5 (to even: 128)
    /// and 255. A normal map is remapped; another signed texture only with signed-data=remap.
    #[test]
    fn signed_normal_maps_take_gltfs_encoding_and_other_signed_data_is_refused() {
        let texel = vec![vec![127, 0x81, 0, 127]];
        let (t, log) = run(
            LowerTexturesRgba8::default(),
            texture(TexFormat::Rgba8Snorm, TexRole::Normal, texel.clone(), 1),
        )
        .unwrap();
        assert_eq!(t.levels, [[255, 0, 128, 255]]);
        assert_eq!(t.role, TexRole::Normal);
        assert_eq!(
            log.approximations["signed normal maps stored in glTF's unsigned normal encoding"],
            1
        );
        let mask = texture(TexFormat::Rgba8Snorm, TexRole::Mask, texel, 1);
        let refused = run(LowerTexturesRgba8::default(), mask.clone()).unwrap_err();
        assert!(refused.contains("signed-data=remap"), "{refused}");
        let remap = LowerTexturesRgba8 {
            signed_data: SignedData::Remap,
        };
        let (t, log) = run(remap, mask).unwrap();
        assert_eq!(t.levels, [[255, 0, 128, 255]]);
        assert_eq!(
            log.approximations["signed textures remapped to v * 0.5 + 0.5 (signed-data=remap)"],
            1
        );
    }

    /// A format 8-bit RGBA holds exactly is left as stored, nothing counted.
    #[test]
    fn exact_formats_are_left_as_stored() {
        let stored = texture(TexFormat::Rg8Unorm, TexRole::Mask, vec![vec![1, 2]], 1);
        let (t, log) = run(LowerTexturesRgba8::default(), stored.clone()).unwrap();
        assert_eq!(t, stored);
        assert!(log.approximations.is_empty());
    }

    #[test]
    fn options_round_trip() {
        for pass in [
            LowerTexturesRgba8::default(),
            LowerTexturesRgba8 {
                signed_data: SignedData::Remap,
            },
        ] {
            assert_eq!(LowerTexturesRgba8::from_options(&pass.options()).unwrap(), pass);
        }
        assert!(
            LowerTexturesRgba8::from_options(&Options::from([("signed".to_owned(), "x".to_owned())]))
                .is_err()
        );
    }
}
