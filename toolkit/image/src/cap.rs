//! `cap-textures{max=N}`: textures no larger than the size a program asks for, taken from the stored mip levels.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{NormalX, NormalZ, TexFormat, TexRole, Texture, World};
use primase_texcodec::{Decoder, TexDecoder};

use crate::{Image, Result, Samples, half, pick_level, resize_normals};

/// What [`cap_texture`] did to one texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capped {
    /// Already no larger than the cap.
    Within,
    /// A data texture (`TexRole::Data`), kept whole whatever its size.
    Data,
    /// The stored level `level` and those below it kept, exactly; the larger ones dropped.
    Level { level: usize, width: u32, height: u32 },
    /// No stored level was small enough: the smallest one decoded to RGBA8 and box-filtered down to `width` x
    /// `height`, in stored values (sRGB textures are filtered as stored; normal maps through [`resize_normals`]).
    Downscaled { width: u32, height: u32 },
    /// No stored level was small enough, and the texture is a normal map whose X is not in red or whose Z is not
    /// stored, which averaging RGB vectors cannot filter: left at its smallest stored level.
    LeftLarge { level: usize, width: u32, height: u32 },
}

/// Cap one texture to `max` pixels on its larger side (see [`CapTextures`]).
pub fn cap_texture(t: &mut Texture, max: u32) -> Result<Capped> {
    if t.role == TexRole::Data {
        return Ok(Capped::Data);
    }
    if t.width.max(t.height) <= max {
        return Ok(Capped::Within);
    }
    let (level, width, height) = pick_level(t.width, t.height, t.levels.len(), max)?;
    if width.max(height) <= max {
        t.levels.drain(..level);
        (t.width, t.height) = (width, height);
        return Ok(Capped::Level { level, width, height });
    }
    // averaging the RGB vectors is right whatever the axes' order and signs, as long as RGB holds X, Y, Z
    let averageable = t
        .normal
        .is_none_or(|n| n.x == NormalX::Red && n.z == NormalZ::Stored);
    if t.role == TexRole::Normal && !averageable {
        t.levels.drain(..level);
        (t.width, t.height) = (width, height);
        return Ok(Capped::LeftLarge { level, width, height });
    }
    // decode the smallest stored level, halve it until it fits (normal maps: one renormalizing filter to the size)
    let rgba = Decoder::default().decode_rgba8(t.format, width, height, &t.levels[level])?;
    let mut image = Image {
        width,
        height,
        samples: Samples::U8(rgba),
    };
    let (mut w, mut h) = (width, height);
    while w.max(h) > max {
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    image = if t.role == TexRole::Normal {
        resize_normals(&image, w, h)?.image
    } else {
        while image.width.max(image.height) > max {
            image = half(&image)?;
        }
        image
    };
    let Samples::U8(pixels) = image.samples else {
        unreachable!("RGBA8 in, RGBA8 out");
    };
    t.format = TexFormat::Rgba8Unorm;
    t.levels = vec![pixels];
    (t.width, t.height) = (image.width, image.height);
    Ok(Capped::Downscaled {
        width: image.width,
        height: image.height,
    })
}

/// `cap-textures{max=N}` (N > 0, required): every texture no larger than N pixels on its larger side, so passes that
/// derive textures from it (PBR maps, normal maps) work at that size. The composing program chooses N; readers read
/// every stored level.
/// - A stored mip level no larger than N: that level and the smaller ones are kept, exactly; the larger ones dropped.
/// - None: the smallest stored level is decoded to RGBA8 and box-filtered down (an approximation, named per texture).
///   A normal map whose RGB does not hold X, Y, Z (X in alpha, Z rebuilt) is left at its smallest level instead
///   (counted).
/// - Data textures (`TexRole::Data`: palettes, lookups, distance fields) are kept whole.
///
/// Every texture is counted in one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapTextures {
    /// `None` only for the instance a registry makes with no options; running it is an error.
    pub max: Option<u32>,
}

impl CapTextures {
    pub fn from_options(o: &Options) -> std::result::Result<CapTextures, String> {
        let mut max = None;
        for (k, v) in o {
            match k.as_str() {
                "max" => {
                    max = Some(
                        v.parse::<u32>()
                            .ok()
                            .filter(|&m| m > 0)
                            .ok_or_else(|| format!("max={v} is not a positive number of pixels"))?,
                    )
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        // a registry makes every pass once with no options; a pipeline must name max (checked when it runs)
        Ok(CapTextures { max })
    }
}

/// [`CapTextures::from_options`] as a pass factory.
pub fn cap_textures(o: &Options) -> std::result::Result<Box<dyn Pass>, String> {
    Ok(Box::new(CapTextures::from_options(o)?))
}

impl Pass for CapTextures {
    fn name(&self) -> &'static str {
        "cap-textures"
    }
    fn options(&self) -> Options {
        match self.max {
            None => Options::new(),
            Some(m) => Options::from([("max".to_owned(), m.to_string())]),
        }
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn approximates(&self) -> &'static str {
        "textures with no stored level within the cap: box-filtered from their smallest level, decoded to RGBA8"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let max = self
            .max
            .ok_or("cap-textures needs max=N, the largest side in pixels the program wants")?;
        let Some(mut textures) = w.column_mut::<Texture>() else {
            return Ok(());
        };
        for (e, t) in textures.iter_mut() {
            let (from_w, from_h) = (t.width, t.height);
            match cap_texture(t, max).map_err(|err| format!("{e:?}: {err}"))? {
                Capped::Within => log.count("textures within the cap", 1),
                Capped::Data => log.count("data textures kept whole", 1),
                Capped::Level { level, .. } => log.count(&format!("textures: stored level {level} kept"), 1),
                Capped::Downscaled { width, height } => log.approximate(
                    &format!("{e:?}: {from_w}x{from_h}, no stored level within {max}, box-filtered to {width}x{height}"),
                    1,
                ),
                Capped::LeftLarge { level, width, height } => log.count(
                    &format!("{e:?}: normal map with X not in red or Z not stored, left at stored level {level} ({width}x{height})"),
                    1,
                ),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{ColorSpace, NormalConvention};

    fn rgba8(width: u32, height: u32, levels: Vec<Vec<u8>>, role: TexRole) -> Texture {
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels,
            color_space: ColorSpace::Srgb,
            role,
            normal: None,
        }
    }

    /// An 8 x 8 chain (8, 4, 2, 1 px), level i filled with 10 + i.
    fn chain() -> Texture {
        let levels = (0..4u8).map(|i| vec![10 + i; (8usize >> i).pow(2) * 4]).collect();
        rgba8(8, 8, levels, TexRole::Color)
    }

    #[test]
    fn a_stored_level_within_the_cap_is_kept_exactly() {
        let mut t = chain();
        let got = cap_texture(&mut t, 2).unwrap();
        assert_eq!(
            got,
            Capped::Level {
                level: 2,
                width: 2,
                height: 2
            }
        );
        assert_eq!(t.levels, vec![vec![12; 16], vec![13; 4]]);
        assert_eq!((t.width, t.height, t.format), (2, 2, TexFormat::Rgba8Unorm));
        // 8 x 4 capped to 4: the 4 x 2 level (32 bytes) and the two below it
        let mut wide = rgba8(
            8,
            4,
            vec![vec![0; 128], vec![1; 32], vec![2; 8], vec![3; 4]],
            TexRole::Mask,
        );
        assert_eq!(
            cap_texture(&mut wide, 4).unwrap(),
            Capped::Level {
                level: 1,
                width: 4,
                height: 2
            }
        );
        assert_eq!(wide.levels, vec![vec![1; 32], vec![2; 8], vec![3; 4]]);
        // within the cap already: untouched
        let mut t = chain();
        assert_eq!(cap_texture(&mut t, 8).unwrap(), Capped::Within);
        assert_eq!(t, chain());
    }

    #[test]
    fn no_stored_level_within_the_cap_is_box_filtered_and_named() {
        // one 4 x 4 level; each 2 x 2 block's red: {10, 20, 30, 40} -> (100 + 2) / 4 = 25, {0, 0, 0, 1} -> 0,
        // {255, 255, 255, 254} -> 255, {1, 2, 3, 4} -> 3 (round half up, in stored values); green 7, blue 9, alpha 255
        let red = [[10, 20, 0, 0], [30, 40, 0, 1], [255, 255, 1, 2], [255, 254, 3, 4]];
        let pixels: Vec<u8> = red.iter().flatten().flat_map(|&r| [r, 7, 9, 255]).collect();
        let mut t = rgba8(4, 4, vec![pixels], TexRole::Color);
        assert_eq!(
            cap_texture(&mut t, 2).unwrap(),
            Capped::Downscaled { width: 2, height: 2 }
        );
        let expected: Vec<u8> = [25, 0, 255, 3].iter().flat_map(|&r| [r, 7, 9, 255]).collect();
        assert_eq!(t.levels, vec![expected]);
        assert_eq!(
            (t.width, t.height, t.format, t.color_space),
            (2, 2, TexFormat::Rgba8Unorm, ColorSpace::Srgb)
        );
    }

    #[test]
    fn data_textures_and_unfilterable_normal_maps_are_left_as_they_are() {
        let data = rgba8(8, 8, vec![vec![5; 256]], TexRole::Data);
        let mut t = data.clone();
        assert_eq!(cap_texture(&mut t, 2).unwrap(), Capped::Data);
        assert_eq!(t, data);
        // X in alpha (DXT5nm): averaging RGB would be wrong; its one level stays
        let mut normal = rgba8(4, 4, vec![vec![128; 64]], TexRole::Normal);
        normal.normal = Some(NormalConvention {
            x: NormalX::Alpha,
            ..NormalConvention::GLTF
        });
        let before = normal.clone();
        assert_eq!(
            cap_texture(&mut normal, 2).unwrap(),
            Capped::LeftLarge {
                level: 0,
                width: 4,
                height: 4
            }
        );
        assert_eq!(normal, before);
    }

    #[test]
    fn the_pass_counts_every_texture_and_needs_max() {
        let mut r = PassRegistry::new();
        r.register("cap-textures", cap_textures);
        let m = PassManager::new(r);
        let mut w = World::new();
        let es: Vec<_> = (0..3).map(|_| w.spawn()).collect();
        w.insert(es[0], chain());
        w.insert(es[1], rgba8(8, 8, vec![vec![5; 256]], TexRole::Data));
        w.insert(es[2], rgba8(4, 4, vec![vec![0; 64]], TexRole::Color));
        let mut report = RunReport::default();
        m.run(
            &mut w,
            &m.registry.parse("cap-textures{max=2}").unwrap(),
            &mut report,
        )
        .unwrap();
        let log = &report.steps[0].log;
        assert_eq!(log.counts.get("textures: stored level 2 kept"), Some(&1));
        assert_eq!(log.counts.get("data textures kept whole"), Some(&1));
        assert_eq!(
            log.approximations.keys().collect::<Vec<_>>(),
            ["e2: 4x4, no stored level within 2, box-filtered to 2x2"]
        );
        assert_eq!(report.steps[0].pass, "cap-textures{max=2}");
        let err = m
            .run(
                &mut w,
                &m.registry.parse("cap-textures").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("needs max=N"), "{err}");
        assert!(m.registry.parse("cap-textures{max=0}").is_err());
    }
}
