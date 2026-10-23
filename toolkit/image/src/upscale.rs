//! `upscale-nearest{factor=N | size=S, textures=<name>;...}`: the selected textures enlarged by nearest neighbour, so a
//! source drawn with nearest sampling (pixel art, sharp-edged UI-style textures) keeps its look on a target that
//! samples linearly: each texel becomes an N x N block, and the target's linear filter then blends only across the
//! block edges. `factor=N` (at least 2) is the enlargement; `size=S` is the smallest whole factor that brings the
//! texture's larger side to at least S (a texture already that large is left as it is, counted). The mip levels below
//! the enlarged one are made again by the 2 x 2 box filter, down to 1 x 1; the texture is stored as RGBA8 in its colour
//! space and role. The selection is the program's, by texture name (`core.name`); a name matching no texture is an
//! error. A texture whose pixels are not loaded, and a normal map, are left as they are, counted. An enlargement past
//! 16384 texels on a side is an error. A program that also caps textures (`cap-textures`) runs this after it.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Name, TexFormat, TexRole, Texture, TextureFile, World};

use crate::{Image, Samples};

/// The largest side an enlargement may reach.
pub const MAX_UPSCALED: u32 = 16384;

/// How much [`UpscaleNearest`] enlarges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscaleBy {
    /// By this whole factor (at least 2).
    Factor(u32),
    /// By the smallest whole factor bringing the larger side to at least this.
    Size(u32),
}

/// See the module documentation. Made by [`UpscaleNearest::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpscaleNearest {
    by: UpscaleBy,
    textures: BTreeSet<String>,
}

impl UpscaleNearest {
    /// The textures named `textures` (at least one, none empty) enlarged `by`.
    pub fn new(by: UpscaleBy, textures: BTreeSet<String>) -> Result<UpscaleNearest, String> {
        match by {
            UpscaleBy::Factor(n) if n < 2 => return Err(format!("factor {n}: at least 2")),
            UpscaleBy::Size(0) => return Err("size 0: at least 1".into()),
            _ => {}
        }
        if textures.is_empty() || textures.iter().any(String::is_empty) {
            return Err("textures names no texture (names separated by ;)".into());
        }
        Ok(UpscaleNearest { by, textures })
    }

    pub fn by(&self) -> UpscaleBy {
        self.by
    }

    pub fn textures(&self) -> &BTreeSet<String> {
        &self.textures
    }

    /// From the options [`UPSCALE_NEAREST`] declares: `factor` or `size` (one of them), and `textures`.
    fn from_given(g: &Given) -> Result<UpscaleNearest, String> {
        let number = |k: &str| -> Result<Option<u32>, String> {
            g.optional(k)
                .map(|v| {
                    v.parse::<u32>()
                        .map_err(|_| format!("{k}={v} is not a whole number"))
                })
                .transpose()
        };
        let by = match (number("factor")?, number("size")?) {
            (Some(n), _) => UpscaleBy::Factor(n),
            (None, Some(s)) => UpscaleBy::Size(s),
            (None, None) => unreachable!("the declaration's check gives one of factor and size"),
        };
        let textures = g.required("textures").split(';').map(str::to_owned).collect();
        UpscaleNearest::new(by, textures)
    }

    /// The whole factor a `w` x `h` texture is enlarged by (1: left as it is).
    pub fn factor_for(&self, w: u32, h: u32) -> u32 {
        match self.by {
            UpscaleBy::Factor(n) => n,
            UpscaleBy::Size(s) => s.div_ceil(w.max(h).max(1)).max(1),
        }
    }
}

/// `upscale-nearest`'s declaration ([`primase_core::pass::PassRegistry::add`]): which textures are enlarged, and how
/// much, are the program's.
pub const UPSCALE_NEAREST: PassSpec = PassSpec {
    name: "upscale-nearest",
    options: &[
        OptionSpec::one_of("by", "factor", "2", "the whole enlargement, at least 2"),
        OptionSpec::one_of(
            "by",
            "size",
            "256",
            "the larger side to reach, in texels: the least whole factor that does",
        ),
        OptionSpec::required(
            "textures",
            "pixels",
            "the names of the textures to enlarge, `;` between",
        ),
    ],
    make: |g| Ok(Box::new(UpscaleNearest::from_given(g)?)),
};

/// `rgba` (`w` x `h`) with each texel an `k` x `k` block.
pub fn enlarge_nearest(w: u32, h: u32, rgba: &[u8], k: u32) -> Vec<u8> {
    let (w, h, k) = (w as usize, h as usize, k as usize);
    let mut out = vec![0u8; w * k * h * k * 4];
    for y in 0..h * k {
        for x in 0..w * k {
            let s = ((y / k) * w + x / k) * 4;
            let d = (y * w * k + x) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    out
}

impl Pass for UpscaleNearest {
    fn name(&self) -> &'static str {
        "upscale-nearest"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([(
            "textures".to_owned(),
            self.textures.iter().cloned().collect::<Vec<_>>().join(";"),
        )]);
        match self.by {
            UpscaleBy::Factor(n) => o.insert("factor".to_owned(), n.to_string()),
            UpscaleBy::Size(s) => o.insert("size".to_owned(), s.to_string()),
        };
        o
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    /// The file link of an enlarged texture: the file no longer holds its pixels.
    fn removes(&self) -> &[&'static str] {
        &[TextureFile::NAME]
    }
    /// Only the textures: the file links go as a side effect.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "nearest-sampled textures enlarged so linear filtering blends only across their texel blocks; lower mip levels \
         remade by the box filter"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let named: BTreeMap<Entity, String> = w
            .iter::<Name>()
            .filter(|(e, n)| {
                self.textures.contains(&n.0)
                    && (w.has::<Texture>(*e) || w.has::<primase_core::TextureFile>(*e))
            })
            .map(|(e, n)| (e, n.0.clone()))
            .collect();
        let found: BTreeSet<&String> = named.values().collect();
        if let Some(missing) = self.textures.iter().find(|n| !found.contains(n)) {
            return Err(format!("upscale-nearest: no texture is named {missing}"));
        }
        for (e, name) in named {
            let Some(t) = w.get::<Texture>(e) else {
                log.count(
                    &format!("named textures whose pixels are not loaded, left as they are: {name}"),
                    1,
                );
                continue;
            };
            if t.role == TexRole::Normal {
                log.count(&format!("normal maps left as they are: {name}"), 1);
                continue;
            }
            let k = self.factor_for(t.width, t.height);
            if k == 1 {
                log.count(
                    &format!("textures already as large as the size, left as they are: {name}"),
                    1,
                );
                continue;
            }
            let (ew, eh) = (t.width.saturating_mul(k), t.height.saturating_mul(k));
            if ew.max(eh) > MAX_UPSCALED {
                return Err(format!(
                    "upscale-nearest: {name} ({} x {}) enlarged {k} times would be {ew} x {eh}, past {MAX_UPSCALED}",
                    t.width, t.height
                ));
            }
            let top =
                crate::texture_level_rgba8(t, 0).map_err(|err| format!("upscale-nearest: {name}: {err}"))?;
            let Samples::U8(rgba) = top.samples else {
                unreachable!("RGBA8 decoded");
            };
            let mut levels = vec![enlarge_nearest(t.width, t.height, &rgba, k)];
            let (mut lw, mut lh) = (ew, eh);
            while lw > 1 || lh > 1 {
                let half = crate::half(&Image {
                    width: lw,
                    height: lh,
                    samples: Samples::U8(levels.last().expect("one").clone()),
                })
                .map_err(|err| format!("upscale-nearest: {name}: {err}"))?;
                let Samples::U8(px) = half.samples else {
                    unreachable!("8-bit in, 8-bit out");
                };
                levels.push(px);
                (lw, lh) = ((lw / 2).max(1), (lh / 2).max(1));
            }
            let enlarged = Texture {
                width: ew,
                height: eh,
                format: TexFormat::Rgba8Unorm,
                levels,
                color_space: t.color_space,
                role: t.role,
                normal: t.normal,
            };
            w.insert(e, enlarged);
            w.remove::<TextureFile>(e);
            log.count(
                &format!("textures enlarged {k} times by nearest neighbour: {name}"),
                1,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::ColorSpace;

    fn texture(w: &mut World, name: &str, width: u32, height: u32, role: TexRole) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            Texture {
                width,
                height,
                format: TexFormat::Rgba8Unorm,
                levels: vec![(0..width * height).flat_map(|i| [i as u8, 0, 0, 255]).collect()],
                color_space: ColorSpace::Srgb,
                role,
                normal: None,
            },
        );
        e
    }

    /// Each texel becomes a k x k block.
    #[test]
    fn a_texel_becomes_a_block() {
        let px = [1, 0, 0, 255, 2, 0, 0, 255];
        let out = enlarge_nearest(2, 1, &px, 2);
        let reds: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert_eq!(reds, [1, 1, 2, 2, 1, 1, 2, 2]);
    }

    /// The named texture is enlarged with its levels remade down to 1 x 1; another is not; a normal map is counted;
    /// size picks the smallest whole factor; a name matching nothing and an enlargement past the limit are errors.
    #[test]
    fn the_named_textures_are_enlarged() {
        let mut w = World::new();
        let sign = texture(&mut w, "sign", 2, 2, TexRole::Color);
        let wall = texture(&mut w, "wall", 2, 2, TexRole::Color);
        let bump = texture(&mut w, "bump", 2, 2, TexRole::Normal);
        let pass = UpscaleNearest::new(UpscaleBy::Factor(4), ["sign".into(), "bump".into()].into()).unwrap();
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        let t = w.get::<Texture>(sign).unwrap();
        assert_eq!((t.width, t.height, t.levels.len()), (8, 8, 4));
        assert_eq!(t.levels[0][..4], [0, 0, 0, 255]);
        assert_eq!(t.levels[0][4 * 4..4 * 4 + 4], [1, 0, 0, 255]);
        assert_eq!(w.get::<Texture>(wall).unwrap().width, 2);
        assert_eq!(w.get::<Texture>(bump).unwrap().width, 2);
        assert_eq!(log.counts["normal maps left as they are: bump"], 1);
        assert_eq!(
            log.counts["textures enlarged 4 times by nearest neighbour: sign"],
            1
        );
        // size: 2 px to at least 7 is 4 times; already 8 px: left
        let by = |s| UpscaleNearest::new(UpscaleBy::Size(s), ["sign".into()].into()).unwrap();
        assert_eq!(by(7).factor_for(2, 2), 4);
        assert_eq!(by(7).factor_for(8, 8), 1);
        let e = UpscaleNearest::new(UpscaleBy::Factor(2), ["sing".into()].into())
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(e.contains("no texture is named sing"), "{e}");
        let e = UpscaleNearest::new(UpscaleBy::Factor(9000), ["sign".into()].into())
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(e.contains("past 16384"), "{e}");
    }

    /// One of factor or size, and textures, are required; the options print the pass again.
    #[test]
    fn the_choices_are_required_and_printed() {
        let o =
            |kv: &[(&str, &str)]| -> Options { kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect() };
        let make = |kv: &[(&str, &str)]| UPSCALE_NEAREST.instance(&o(kv));
        assert!(make(&[("textures", "a")]).is_err());
        assert!(make(&[("factor", "2")]).is_err());
        assert!(make(&[("factor", "2"), ("size", "64"), ("textures", "a")]).is_err());
        assert!(make(&[("factor", "1"), ("textures", "a")]).is_err());
        let p = make(&[("size", "256"), ("textures", "a;b")]).unwrap();
        assert_eq!(
            UPSCALE_NEAREST.instance(&p.options()).unwrap().options(),
            p.options()
        );
    }
}
