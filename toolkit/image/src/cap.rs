//! `cap-textures{max=N}`: textures no larger than the size a program asks for, taken from the stored mip levels.

use std::collections::BTreeMap;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Component, Entity, Name, NormalX, NormalZ, TexFormat, TexRole, Texture, TextureFile, World,
};
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
    /// `height`, in stored values (sRGB textures are filtered as stored; normal maps through [`resize_normals`], or
    /// by the same box as colour with [`NormalDownscale::Box`]).
    Downscaled { width: u32, height: u32 },
    /// No stored level was small enough, and the texture is a normal map whose X is not in red or whose Z is not
    /// stored, which averaging RGB vectors cannot filter: left at its smallest stored level.
    LeftLarge { level: usize, width: u32, height: u32 },
    /// No stored level was small enough and the caller keeps stored levels only ([`Beyond::Keep`]): left at its
    /// smallest stored level, exactly.
    Smallest { level: usize, width: u32, height: u32 },
}

/// What [`cap_texture`] does with a texture none of whose stored levels is within the cap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Beyond {
    /// Decode the smallest stored level and box-filter it down to the cap (an approximation, reported).
    #[default]
    Filter,
    /// Keep the smallest stored level, larger than the cap (exact; reported).
    Keep,
}

/// How a normal map is filtered down when it is ([`Beyond::Filter`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NormalDownscale {
    /// Each vector decoded, averaged and renormalized ([`resize_normals`]); a normal map whose RGB is not X, Y, Z is
    /// left at its smallest stored level.
    #[default]
    Renormalize,
    /// The 2 x 2 box in stored values, as for colour (the vectors' mean, not renormalized; any convention).
    Box,
}

/// How [`cap_texture`] reaches the cap; the defaults filter beyond the stored levels and renormalize normal maps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapMode {
    pub beyond: Beyond,
    pub normals: NormalDownscale,
}

/// Cap one texture to `max` pixels on its larger side (see [`CapTextures`]).
pub fn cap_texture(t: &mut Texture, max: u32, mode: CapMode) -> Result<Capped> {
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
    if mode.beyond == Beyond::Keep {
        t.levels.drain(..level);
        (t.width, t.height) = (width, height);
        return Ok(Capped::Smallest { level, width, height });
    }
    let renormalize = t.role == TexRole::Normal && mode.normals == NormalDownscale::Renormalize;
    // averaging the RGB vectors is right whatever the axes' order and signs, as long as RGB holds X, Y, Z
    let averageable = t
        .normal
        .is_none_or(|n| n.x == NormalX::Red && n.z == NormalZ::Stored);
    if renormalize && !averageable {
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
    image = if renormalize {
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

/// A role [`CapTextures`] can cap apart from the others (data textures are never capped).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapRole {
    Color,
    Normal,
    Mask,
}

/// `cap-textures{max=N[,color=N][,normal=N][,mask=N][,names=NAME:N;...][,beyond=filter|keep][,normals=renormalize|box]}`
/// (N > 0; `max` required): every texture no larger than its cap on its larger side, so passes that derive textures
/// from it (PBR maps, normal maps) work at that size. The composing program chooses the caps; readers read every
/// stored level. A texture's cap is its name's (`names`, its `core.name` in any case), else its role's (`color`,
/// `normal`, `mask`), else `max`.
/// - A stored mip level no larger than the cap: that level and the smaller ones are kept, exactly; the larger ones
///   dropped.
/// - None: by default (`beyond=filter`) the smallest stored level is decoded to RGBA8 and box-filtered down (an
///   approximation, named per texture); normal maps are renormalized (`normals=renormalize`, and a normal map whose
///   RGB does not hold X, Y, Z is left at its smallest level instead, counted) or box-filtered as colour is
///   (`normals=box`). With `beyond=keep` the smallest stored level is kept as it is, counted.
/// - Data textures (`TexRole::Data`: palettes, lookups, distance fields) are kept whole.
///
/// Every texture is counted in one of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapTextures {
    max: u32,
    color: Option<u32>,
    normal: Option<u32>,
    mask: Option<u32>,
    names: BTreeMap<String, u32>,
    mode: CapMode,
}

/// A cap: a positive number of pixels.
fn positive(what: &str, cap: u32) -> std::result::Result<u32, String> {
    if cap == 0 {
        return Err(format!("{what}: a cap is a positive number of pixels, not 0"));
    }
    Ok(cap)
}

/// A cap's option value: a positive number of pixels.
fn pixels(k: &str, v: &str) -> std::result::Result<u32, String> {
    let n = v
        .parse::<u32>()
        .map_err(|_| format!("{k}={v} is not a number of pixels"))?;
    positive(&format!("{k}={v}"), n)
}

impl CapTextures {
    /// Every texture no larger than `max` pixels on its larger side (positive), by the default mode.
    pub fn new(max: u32) -> std::result::Result<CapTextures, String> {
        Ok(CapTextures {
            max: positive("max", max)?,
            color: None,
            normal: None,
            mask: None,
            names: BTreeMap::new(),
            mode: CapMode::default(),
        })
    }

    /// Textures of `role` no larger than `cap` (positive), over `max`.
    pub fn with_role_cap(mut self, role: CapRole, cap: u32) -> std::result::Result<CapTextures, String> {
        let cap = Some(positive("role cap", cap)?);
        match role {
            CapRole::Color => self.color = cap,
            CapRole::Normal => self.normal = cap,
            CapRole::Mask => self.mask = cap,
        }
        Ok(self)
    }

    /// The texture named `name` (its `core.name`, in any case) no larger than `cap` (positive), over its role's cap
    /// and `max`.
    pub fn with_name_cap(mut self, name: &str, cap: u32) -> std::result::Result<CapTextures, String> {
        self.names.insert(name.to_lowercase(), positive(name, cap)?);
        Ok(self)
    }

    /// How the caps are reached.
    pub fn with_mode(mut self, mode: CapMode) -> CapTextures {
        self.mode = mode;
        self
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    /// The caps by name, lower case.
    pub fn name_caps(&self) -> &BTreeMap<String, u32> {
        &self.names
    }

    pub fn mode(&self) -> CapMode {
        self.mode
    }

    /// From the options [`CAP_TEXTURES`] declares, through [`CapTextures::new`] and the same setters.
    fn from_given(g: &Given) -> std::result::Result<CapTextures, String> {
        let mut pass = CapTextures::new(pixels("max", g.required("max"))?)?;
        for (k, role) in [
            ("color", CapRole::Color),
            ("normal", CapRole::Normal),
            ("mask", CapRole::Mask),
        ] {
            if let Some(v) = g.optional(k) {
                pass = pass.with_role_cap(role, pixels(k, v)?)?;
            }
        }
        if let Some(v) = g.optional("names") {
            for pair in v.split(';').filter(|p| !p.is_empty()) {
                let (name, cap) = pair
                    .rsplit_once(':')
                    .ok_or_else(|| format!("names={v}: each entry is NAME:N"))?;
                pass = pass.with_name_cap(name, pixels("names", cap)?)?;
            }
        }
        let mut mode = CapMode::default();
        if let Some(beyond) = g.optional_choice::<Beyond>("beyond") {
            mode.beyond = beyond;
        }
        if let Some(normals) = g.optional_choice::<NormalDownscale>("normals") {
            mode.normals = normals;
        }
        Ok(pass.with_mode(mode))
    }

    /// The cap of texture `t` named `name` (lower case).
    fn cap(&self, t: &Texture, name: Option<&str>) -> u32 {
        if let Some(&n) = name.and_then(|n| self.names.get(n)) {
            return n;
        }
        let role = match t.role {
            TexRole::Color => self.color,
            TexRole::Normal => self.normal,
            TexRole::Mask => self.mask,
            TexRole::Data | TexRole::Other => None,
        };
        role.unwrap_or(self.max)
    }
}

impl Choice for Beyond {
    const VALUES: &'static [(&'static str, Self)] = &[("filter", Beyond::Filter), ("keep", Beyond::Keep)];
}

impl Choice for NormalDownscale {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("renormalize", NormalDownscale::Renormalize),
        ("box", NormalDownscale::Box),
    ];
}

/// `cap-textures`' declaration ([`primase_core::pass::PassRegistry::add`]): the cap is the program's, required.
pub const CAP_TEXTURES: PassSpec = PassSpec {
    name: "cap-textures",
    options: &[
        OptionSpec::required("max", "256", "pixels: the largest side a texture keeps"),
        OptionSpec::optional("color", "512", "pixels: the cap of colour textures", "max"),
        OptionSpec::optional("normal", "512", "pixels: the cap of normal maps", "max"),
        OptionSpec::optional("mask", "512", "pixels: the cap of mask textures", "max"),
        OptionSpec::optional(
            "names",
            "sky:1024",
            "NAME:N entries, `;` between: the cap of the textures so named (lower case), before their role's",
            "no texture capped by its name",
        ),
        OptionSpec::optional_choice::<Beyond>(
            "beyond",
            "a texture whose smallest stored level is over its cap: that level box-filtered down to the cap, or \
             kept",
            "filter",
        ),
        OptionSpec::optional_choice::<NormalDownscale>(
            "normals",
            "how a normal map is filtered down: each vector renormalized, or the stored values boxed",
            "renormalize",
        ),
    ],
    make: |g| Ok(Box::new(CapTextures::from_given(g)?)),
};

impl Pass for CapTextures {
    fn name(&self) -> &'static str {
        "cap-textures"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        for (k, v) in [
            ("max", Some(self.max)),
            ("color", self.color),
            ("normal", self.normal),
            ("mask", self.mask),
        ] {
            if let Some(v) = v {
                o.insert(k.to_owned(), v.to_string());
            }
        }
        if !self.names.is_empty() {
            let names: Vec<String> = self.names.iter().map(|(n, c)| format!("{n}:{c}")).collect();
            o.insert("names".to_owned(), names.join(";"));
        }
        if self.mode.beyond == Beyond::Keep {
            o.insert("beyond".to_owned(), Beyond::Keep.text().to_owned());
        }
        if self.mode.normals == NormalDownscale::Box {
            o.insert("normals".to_owned(), NormalDownscale::Box.text().to_owned());
        }
        o
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    /// The file link of a texture whose levels it dropped or remade: the file no longer holds them.
    fn removes(&self) -> &[&'static str] {
        &[TextureFile::NAME]
    }
    /// Only the textures: the file links go as a side effect.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "textures with no stored level within the cap: box-filtered from their smallest level, decoded to RGBA8"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        // the names the program caps by, looked up before the textures are borrowed
        let names: BTreeMap<Entity, String> = if self.names.is_empty() {
            BTreeMap::new()
        } else {
            w.iter::<Texture>()
                .filter_map(|(e, _)| w.get::<Name>(e).map(|n| (e, n.0.to_lowercase())))
                .collect()
        };
        let Some(mut textures) = w.column_mut::<Texture>() else {
            return Ok(());
        };
        // the textures whose levels changed: their file links no longer match
        let mut changed = Vec::new();
        for (e, t) in textures.iter_mut() {
            let (from_w, from_h) = (t.width, t.height);
            let cap = self.cap(t, names.get(&e).map(String::as_str));
            match cap_texture(t, cap, self.mode).map_err(|err| format!("{e:?}: {err}"))? {
                Capped::Within => log.count("textures within the cap", 1),
                Capped::Data => log.count("data textures kept whole", 1),
                Capped::Level { level, .. } => {
                    changed.push(e);
                    log.count(&format!("textures: stored level {level} kept"), 1)
                }
                Capped::Downscaled { width, height } => {
                    changed.push(e);
                    log.approximate(
                        &format!("{e:?}: {from_w}x{from_h}, no stored level within {cap}, box-filtered to {width}x{height}"),
                        1,
                    )
                }
                Capped::LeftLarge { level, width, height } => {
                    changed.push(e);
                    log.count(
                        &format!("{e:?}: normal map with X not in red or Z not stored, left at stored level {level} ({width}x{height})"),
                        1,
                    )
                }
                Capped::Smallest { level, width, height } => {
                    changed.push(e);
                    log.count(
                        &format!("{e:?}: no stored level within {cap}, left at its smallest, stored level {level} ({width}x{height})"),
                        1,
                    )
                }
            }
        }
        for e in changed {
            w.remove::<TextureFile>(e);
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

    /// A texture whose levels the pass changed no longer matches its file; one it left whole keeps the link.
    #[test]
    fn a_changed_texture_loses_its_file_link() {
        let mut w = World::new();
        let (changed, within) = (w.spawn(), w.spawn());
        w.insert(changed, chain());
        w.insert(within, rgba8(2, 2, vec![vec![0; 16], vec![0; 4]], TexRole::Color));
        for e in [changed, within] {
            w.insert(e, TextureFile { path: "a.png".into() });
        }
        CapTextures::new(4)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert!(!w.has::<TextureFile>(changed));
        assert!(w.has::<TextureFile>(within));
    }

    #[test]
    fn a_stored_level_within_the_cap_is_kept_exactly() {
        let mut t = chain();
        let got = cap_texture(&mut t, 2, CapMode::default()).unwrap();
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
            cap_texture(&mut wide, 4, CapMode::default()).unwrap(),
            Capped::Level {
                level: 1,
                width: 4,
                height: 2
            }
        );
        assert_eq!(wide.levels, vec![vec![1; 32], vec![2; 8], vec![3; 4]]);
        // within the cap already: untouched
        let mut t = chain();
        assert_eq!(
            cap_texture(&mut t, 8, CapMode::default()).unwrap(),
            Capped::Within
        );
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
            cap_texture(&mut t, 2, CapMode::default()).unwrap(),
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
        assert_eq!(cap_texture(&mut t, 2, CapMode::default()).unwrap(), Capped::Data);
        assert_eq!(t, data);
        // X in alpha (DXT5nm): averaging RGB would be wrong; its one level stays
        let mut normal = rgba8(4, 4, vec![vec![128; 64]], TexRole::Normal);
        normal.normal = Some(NormalConvention {
            x: NormalX::Alpha,
            ..NormalConvention::GLTF
        });
        let before = normal.clone();
        assert_eq!(
            cap_texture(&mut normal, 2, CapMode::default()).unwrap(),
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
        r.add(&CAP_TEXTURES);
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
        // max has no default: a pipeline without it does not parse, nor does a struct with none exist
        let err = m.registry.parse("cap-textures").unwrap_err();
        assert!(err.to_string().contains("max is required: "), "{err}");
        assert!(CapTextures::new(0).is_err());
        assert!(
            CapTextures::new(4)
                .unwrap()
                .with_role_cap(CapRole::Normal, 0)
                .is_err()
        );
        assert!(m.registry.parse("cap-textures{max=0}").is_err());
    }

    /// `beyond=keep`: a texture with no stored level within the cap keeps its smallest, exactly; one with such a level
    /// is capped as ever.
    #[test]
    fn keep_leaves_the_smallest_stored_level() {
        let keep = CapMode {
            beyond: Beyond::Keep,
            ..CapMode::default()
        };
        // two stored levels, 8 and 4 px, capped to 2
        let mut t = rgba8(8, 8, vec![vec![1; 256], vec![2; 64]], TexRole::Color);
        assert_eq!(
            cap_texture(&mut t, 2, keep).unwrap(),
            Capped::Smallest {
                level: 1,
                width: 4,
                height: 4
            }
        );
        assert_eq!((t.width, t.height, t.levels.clone()), (4, 4, vec![vec![2; 64]]));
        let mut t = chain();
        assert_eq!(
            cap_texture(&mut t, 2, keep).unwrap(),
            Capped::Level {
                level: 2,
                width: 2,
                height: 2
            }
        );
    }

    /// `normals=box`: a normal map is box-filtered in stored values as colour is, whatever its convention; by default
    /// it is renormalized, so the two differ on vectors that do not average to unit length.
    #[test]
    fn normals_box_filters_like_colour() {
        // a 2 x 1 normal map: +X and +Y, (255,128,128) and (128,255,128): the box mean is (192,192,128)
        let pixels = vec![255, 128, 128, 255, 128, 255, 128, 255];
        let mut normal = rgba8(2, 1, vec![pixels.clone()], TexRole::Normal);
        normal.normal = Some(NormalConvention {
            x: NormalX::Alpha,
            ..NormalConvention::GLTF
        });
        let boxed = CapMode {
            normals: NormalDownscale::Box,
            ..CapMode::default()
        };
        let mut b = normal.clone();
        assert_eq!(
            cap_texture(&mut b, 1, boxed).unwrap(),
            Capped::Downscaled { width: 1, height: 1 }
        );
        assert_eq!(b.levels, vec![vec![192, 192, 128, 255]]);
        // the same map with X in alpha cannot be renormalized: left at its level by default
        let mut r = normal.clone();
        assert!(matches!(
            cap_texture(&mut r, 1, CapMode::default()).unwrap(),
            Capped::LeftLarge { .. }
        ));
        // in glTF's convention it is renormalized, not boxed
        let mut g = normal;
        g.normal = Some(NormalConvention::GLTF);
        cap_texture(&mut g, 1, CapMode::default()).unwrap();
        assert_ne!(g.levels, vec![vec![192, 192, 128, 255]]);
    }

    /// A texture's cap is its name's, else its role's, else `max`; the options print back as given.
    #[test]
    fn names_and_roles_choose_the_cap() {
        let mut r = PassRegistry::new();
        r.add(&CAP_TEXTURES);
        let m = PassManager::new(r);
        let pipeline = m
            .registry
            .parse("cap-textures{max=4,normal=2,names=Wall:1,beyond=keep,normals=box}")
            .unwrap();
        assert_eq!(
            pipeline.to_string(),
            "cap-textures{beyond=keep,max=4,names=wall:1,normal=2,normals=box}"
        );
        let mut w = World::new();
        let (colour, normal, named) = (w.spawn(), w.spawn(), w.spawn());
        w.insert(colour, chain());
        w.insert(normal, rgba8(8, 8, chain().levels, TexRole::Normal));
        w.insert(named, chain());
        w.insert(named, Name("WALL".into()));
        m.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
        let side = |e| w.get::<Texture>(e).unwrap().width;
        assert_eq!((side(colour), side(normal), side(named)), (4, 2, 1));
        for bad in [
            "cap-textures{max=4,names=wall}",
            "cap-textures{max=4,beyond=crop}",
            "cap-textures{max=4,normal=0}",
        ] {
            assert!(m.registry.parse(bad).is_err(), "{bad}");
        }
    }
}
