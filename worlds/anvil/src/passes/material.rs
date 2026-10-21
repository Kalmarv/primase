//! `lower-anvil-material{normal-green=up|down,alpha=opaque|mask[,alpha-cutoff=X],unset=white}`: Anvil Materials
//! ([`AnvilMaterial`]) become metal-rough PBR, for the templates whose pixel shaders the Riders Republic notes
//! evaluated (docs/material-spec.md §4: the game's own shaders run on WARP against these formulas, within 5.5e-7):
//!
//! - **MAD_Fake** (and MAD_CHR_Basic): base colour the texture at t75; the normal map t76's red and green, z rebuilt;
//!   roughness t76's alpha, metalness its blue (a metallic-roughness texture made of them); two-sided.
//! - **MAD_Tex1, MAD_Tex1_Wind, MAD_Decals1**: base colour t76, tinted where its mask (t77, the Material's TextureSet
//!   slot 7; none on Decals1) has red, by `Hue`, `Saturation` and `Luminosity` in HSV; roughness `S(sat(a + Roughness
//!   Offset))` and metalness `b * Metalness` of the normal map t75 (`S(x) = x x (3 - 2 x)`); occlusion
//!   `1 + OnOffAO (mask.b - 1)`. The vertex-colour factor (`Vertex Color Intensity`) needs the mesh's vertex colour,
//!   which isn't read: a Material with it other than 0 is lowered without it, reported.
//!
//! A texture's colour space follows its TextureMap's u32 at 25 (1: sampled as sRGB, the notes' rule from the game's
//! texture-creation records; not a role's); values are made linear as the shader samples them before any arithmetic,
//! and a made texture is linear except a tinted base colour, sRGB. The Material's UV parameters (8 f32) aren't
//! applied: how the game makes the shader's UV rows of them isn't established; a Material whose parameters aren't
//! `1, 1, 0, 0, 0, 0, 0, 0` is reported. Every option is the program's: the normal map's green sign against glTF's
//! isn't established, nor the alpha test's reference (alpha=mask states one), nor what the game binds for a texture a
//! Material leaves unset (unset=white: the notes find the masks' settings make sense only so). Other templates are
//! drawn with a neutral placeholder, reported by name. The Materials are consumed.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Component, Entity, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat,
    TexRole, Texture, World,
};
use primase_image::{Rgba8Texel, TextureMetadata, map_rgba8_levels, srgb_to_linear};

use crate::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialValue};

/// How the alpha channel is drawn: the program's statement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaChoice {
    Opaque,
    Mask(f32),
}

/// The pass (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMaterial {
    pub normal_green: NormalGreen,
    pub alpha: AlphaChoice,
}

impl LowerAnvilMaterial {
    /// From its options: `normal-green`, `alpha` (and `alpha-cutoff` with `mask`), `unset=white`, all required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilMaterial, String> {
        let known = ["normal-green", "alpha", "alpha-cutoff", "unset"];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let usage = "normal-green=up|down, alpha=opaque|mask (with alpha-cutoff=X), unset=white";
        let get = |k: &str| {
            o.get(k)
                .map(String::as_str)
                .ok_or_else(|| format!("{k} is required: {usage}"))
        };
        let normal_green = match get("normal-green")? {
            "up" => NormalGreen::Up,
            "down" => NormalGreen::Down,
            v => return Err(format!("normal-green={v} is not up or down")),
        };
        let alpha = match get("alpha")? {
            "opaque" => AlphaChoice::Opaque,
            "mask" => {
                let c: f32 = get("alpha-cutoff")?
                    .parse()
                    .map_err(|_| "alpha-cutoff is a number".to_owned())?;
                if !(0.0..=1.0).contains(&c) {
                    return Err(format!("alpha-cutoff {c}: within 0 and 1"));
                }
                AlphaChoice::Mask(c)
            }
            v => return Err(format!("alpha={v} is not opaque or mask")),
        };
        match get("unset")? {
            "white" => {}
            v => return Err(format!("unset={v}: only white is read")),
        }
        Ok(LowerAnvilMaterial { normal_green, alpha })
    }
}

/// `S(x) = x x (3 - 2 x)`.
fn smooth(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

fn sat(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// The notes' HSV tint `T(c; h, s, v)`: to HSV, hue shifted (wrapping), saturation and value offset (clamped), back.
fn tint(c: [f64; 3], dh: f64, ds: f64, dv: f64) -> [f64; 3] {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d) / 6.0
    } else if max == g {
        (2.0 + (b - r) / d) / 6.0
    } else {
        (4.0 + (r - g) / d) / 6.0
    };
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = (h + dh).rem_euclid(1.0);
    let (s, v) = (sat(s + ds), sat(max + dv));
    let hue = [
        (6.0 * h - 3.0).abs() - 1.0,
        2.0 - (6.0 * h - 2.0).abs(),
        2.0 - (6.0 * h - 4.0).abs(),
    ];
    hue.map(|x| ((sat(x) - 1.0) * s + 1.0) * v)
}

/// A texel's channels as the shader samples them: red, green and blue made linear where the texture is sampled as
/// sRGB, alpha as stored.
fn sampled(rgba: [u8; 4], srgb: bool) -> [f64; 4] {
    let f = |x: u8| f64::from(x) / 255.0;
    let c = |x: u8| if srgb { srgb_to_linear(f(x)) } else { f(x) };
    [c(rgba[0]), c(rgba[1]), c(rgba[2]), f(rgba[3])]
}

fn byte(x: f64) -> u8 {
    (sat(x) * 255.0).round() as u8
}

/// What lowering one Material reads of it.
struct Inputs<'m> {
    m: &'m AnvilMaterial,
}

impl Inputs<'_> {
    /// The texture its template binds at shader slot `t(75 + slot)`, from the Material's property of that key.
    fn texture(&self, slot: u32) -> Option<Entity> {
        let key = self
            .m
            .bindings
            .iter()
            .find(|(_, b)| *b == MaterialBinding::Texture { slot })?
            .0;
        self.m.values.iter().find_map(|(k, v)| match v {
            MaterialValue::Texture { texture, .. } if *k == key => *texture,
            _ => None,
        })
    }

    /// The scalar its template binds at the constant buffer's vec4 `vec4`; 0 where the Material sets none.
    fn scalar(&self, vec4: u32) -> f64 {
        let Some(&(key, _)) = self
            .m
            .bindings
            .iter()
            .find(|(_, b)| *b == MaterialBinding::Scalar { vec4 })
        else {
            return 0.0;
        };
        self.m
            .values
            .iter()
            .find_map(|(k, v)| match v {
                MaterialValue::Float(x) if *k == key => Some(f64::from(*x)),
                _ => None,
            })
            .unwrap_or(0.0)
    }

    /// Whether its UV parameters are other than `1, 1, 0, 0, 0, 0, 0, 0`.
    fn uv_set(&self) -> bool {
        let uv_keys: Vec<u32> = self
            .m
            .bindings
            .iter()
            .filter(|(_, b)| matches!(b, MaterialBinding::UvTransform { .. }))
            .map(|(k, _)| *k)
            .collect();
        self.m.values.iter().any(|(k, v)| match v {
            MaterialValue::Floats(f) if uv_keys.contains(k) => {
                f.as_slice() != [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
            }
            _ => false,
        })
    }
}

/// Whether the texture `e`'s TextureMap is sampled as sRGB (its u32 at 25 is 1).
fn srgb(w: &World, e: Entity) -> bool {
    w.get::<AnvilTextureMap>(e).is_some_and(|t| t.field_25 == 1)
}

/// A texture made from `source`'s every stored level, each texel by `map` of its sampled channels.
fn derived(
    w: &World,
    source: Entity,
    role: TexRole,
    color_space: ColorSpace,
    mut map: impl FnMut(&Rgba8Texel, [f64; 4]) -> [u8; 4],
) -> Result<Texture, String> {
    let t = w
        .get::<Texture>(source)
        .ok_or("a texture entity without a texture")?;
    let srgb = srgb(w, source);
    map_rgba8_levels(
        t,
        0..t.levels.len(),
        TextureMetadata {
            color_space,
            role,
            normal: None,
        },
        |texel| Ok(map(&texel, sampled(texel.rgba, srgb))),
    )
    .map_err(|e| e.to_string())
}

impl Pass for LowerAnvilMaterial {
    fn name(&self) -> &'static str {
        "lower-anvil-material"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            (
                "normal-green".to_owned(),
                match self.normal_green {
                    NormalGreen::Up => "up",
                    NormalGreen::Down => "down",
                }
                .to_owned(),
            ),
            ("unset".to_owned(), "white".to_owned()),
        ]);
        match self.alpha {
            AlphaChoice::Opaque => {
                o.insert("alpha".to_owned(), "opaque".to_owned());
            }
            AlphaChoice::Mask(c) => {
                o.insert("alpha".to_owned(), "mask".to_owned());
                o.insert("alpha-cutoff".to_owned(), c.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Texture::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "Riders Republic materials as metal-rough PBR: no vertex-colour factor, no UV parameters, other templates a placeholder"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<Entity> = w.iter::<AnvilMaterial>().map(|(e, _)| e).collect();
        let mut white: Option<Entity> = None;
        let alpha_mode = match self.alpha {
            AlphaChoice::Opaque => AlphaMode::Opaque,
            AlphaChoice::Mask(cutoff) => AlphaMode::Mask { cutoff },
        };
        let normal = NormalConvention {
            green: self.normal_green,
            z: NormalZ::Rebuild,
            x: NormalX::Red,
            basis: NormalBasis::TangentBitangent,
        };
        // made textures, per (source, what, parameters' bits)
        let mut made: BTreeMap<(Entity, &'static str, [u64; 3]), Entity> = BTreeMap::new();
        for e in materials {
            let Some(m) = w.remove::<AnvilMaterial>(e) else {
                continue;
            };
            let name = m.template_name.clone().unwrap_or_default();
            let i = Inputs { m: &m };
            let pbr = match name.as_str() {
                "MAD_Fake" | "MAD_CHR_Basic" => {
                    let albedo = i.texture(0);
                    let normals = i.texture(1);
                    if let Some(a) = albedo {
                        set_colour(w, a, TexRole::Color);
                    }
                    let mr = match normals {
                        Some(n) => Some(made_texture(w, &mut made, (n, "fake mr", [0; 3]), |w| {
                            derived(w, n, TexRole::Mask, ColorSpace::Linear, |_, c| {
                                [255, byte(c[3]), byte(c[2]), 255]
                            })
                        })?),
                        None => None,
                    };
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    log.count("MAD_Fake Materials lowered", 1);
                    PbrMaterial {
                        base_color_texture: albedo.map(TextureRef::new),
                        metallic: 1.0,
                        roughness: 1.0,
                        metallic_roughness_texture: mr.map(TextureRef::new),
                        normal_texture: normals.map(TextureRef::new),
                        alpha_mode,
                        double_sided: true,
                        ..PbrMaterial::default()
                    }
                }
                "MAD_Tex1" | "MAD_Tex1_Wind" | "MAD_Decals1" => {
                    let masked = name != "MAD_Decals1";
                    let normals = i.texture(0);
                    let diffuse = i.texture(1);
                    let mask = if masked {
                        Some(match m.texture_set.get(7).copied().flatten() {
                            Some(t) => t,
                            None => {
                                log.count("Tex1 masks unset: white (stated)", 1);
                                *white.get_or_insert_with(|| white_texture(w))
                            }
                        })
                    } else {
                        None
                    };
                    let [metal, hue, saturation, luminosity, rough_offset, ao_on, vci] =
                        [1, 2, 3, 4, 5, 6, 7].map(|v| i.scalar(v));
                    if vci != 0.0 {
                        log.omit(
                            "Tex1 Materials' vertex-colour factor (the mesh's vertex colour isn't read)",
                            1,
                        );
                    }
                    if i.uv_set() {
                        log.omit(
                            "Materials' UV parameters (how the game applies them isn't established)",
                            1,
                        );
                    }
                    let tinted = (hue, saturation, luminosity) != (0.0, 0.0, 0.0);
                    let base = match (diffuse, mask) {
                        (Some(d), Some(k)) if tinted => {
                            let key = (
                                d,
                                "tint",
                                [hue.to_bits(), saturation.to_bits(), luminosity.to_bits()],
                            );
                            let mask_srgb = srgb(w, k);
                            let mask_image = w
                                .get::<Texture>(k)
                                .map(|t| {
                                    let meta = TextureMetadata {
                                        color_space: ColorSpace::Linear,
                                        role: TexRole::Mask,
                                        normal: None,
                                    };
                                    map_rgba8_levels(t, 0..1, meta, |texel| Ok(texel.rgba))
                                })
                                .transpose()
                                .map_err(|e| e.to_string())?;
                            Some(made_texture(w, &mut made, key, |w| {
                                derived(w, d, TexRole::Color, ColorSpace::Srgb, |t, c| {
                                    let m = mask_image
                                        .as_ref()
                                        .map_or(1.0, |img| sampled(nearest(img, t), mask_srgb)[0]);
                                    let tc = tint([c[0], c[1], c[2]], hue, saturation, luminosity);
                                    let out = [0, 1, 2].map(|k| c[k] + (tc[k] - c[k]) * m);
                                    let enc = |x: f64| primase_image::linear_to_srgb8(x);
                                    [enc(out[0]), enc(out[1]), enc(out[2]), byte(c[3])]
                                })
                            })?)
                        }
                        (Some(d), _) => {
                            set_colour(w, d, TexRole::Color);
                            Some(d)
                        }
                        (None, _) => None,
                    };
                    let mr = match normals {
                        Some(n) => {
                            let key = (n, "tex1 mr", [metal.to_bits(), rough_offset.to_bits(), 0]);
                            Some(made_texture(w, &mut made, key, |w| {
                                derived(w, n, TexRole::Mask, ColorSpace::Linear, |_, c| {
                                    [
                                        255,
                                        byte(smooth(sat(c[3] + rough_offset))),
                                        byte(c[2] * metal),
                                        255,
                                    ]
                                })
                            })?)
                        }
                        None => None,
                    };
                    let occlusion = match mask {
                        Some(k) if ao_on != 0.0 => {
                            let key = (k, "tex1 ao", [ao_on.to_bits(), 0, 0]);
                            Some(made_texture(w, &mut made, key, |w| {
                                derived(w, k, TexRole::Mask, ColorSpace::Linear, |_, c| {
                                    let ao = byte(1.0 + ao_on * (c[2] - 1.0));
                                    [ao, ao, ao, 255]
                                })
                            })?)
                        }
                        _ => None,
                    };
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    log.count(&format!("{name} Materials lowered"), 1);
                    PbrMaterial {
                        base_color_texture: base.map(TextureRef::new),
                        metallic: 1.0,
                        roughness: 1.0,
                        metallic_roughness_texture: mr.map(TextureRef::new),
                        occlusion_texture: occlusion.map(TextureRef::new),
                        normal_texture: normals.map(TextureRef::new),
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                other => {
                    log.omit(
                        &format!(
                            "Materials of template {other:?} (not lowered: a neutral placeholder drawn)"
                        ),
                        1,
                    );
                    PbrMaterial {
                        base_color: [0.5, 0.5, 0.5, 1.0],
                        metallic: 0.0,
                        roughness: 1.0,
                        ..PbrMaterial::default()
                    }
                }
            };
            w.insert(e, pbr);
        }
        Ok(())
    }
}

/// The texel of `img` (an RGBA8 texture's level 0) nearest the texel `t` of another texture.
fn nearest(img: &Texture, t: &Rgba8Texel) -> [u8; 4] {
    let x = (u64::from(t.x) * u64::from(img.width) / u64::from(t.width.max(1))) as usize;
    let y = (u64::from(t.y) * u64::from(img.height) / u64::from(t.height.max(1))) as usize;
    let i = (y * img.width as usize + x) * 4;
    let level = img.levels.first().map_or(&[][..], Vec::as_slice);
    [0, 1, 2, 3].map(|k| level.get(i + k).copied().unwrap_or(255))
}

/// The made texture `key` names, made once by `make`.
fn made_texture(
    w: &mut World,
    made: &mut BTreeMap<(Entity, &'static str, [u64; 3]), Entity>,
    key: (Entity, &'static str, [u64; 3]),
    make: impl FnOnce(&World) -> Result<Texture, String>,
) -> Result<Entity, String> {
    if let Some(&e) = made.get(&key) {
        return Ok(e);
    }
    let t = make(w)?;
    let e = w.spawn();
    w.insert(e, t);
    made.insert(key, e);
    Ok(e)
}

/// A 1 x 1 white texture (the stated `unset=white`).
fn white_texture(w: &mut World) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255; 4]],
            color_space: ColorSpace::Linear,
            role: TexRole::Mask,
            normal: None,
        },
    );
    e
}

/// The texture `e` marked a colour texture in the colour space its u32 at 25 states.
fn set_colour(w: &mut World, e: Entity, role: TexRole) {
    let space = if srgb(w, e) {
        ColorSpace::Srgb
    } else {
        ColorSpace::Linear
    };
    if let Some(t) = w.get_mut::<Texture>(e) {
        t.role = role;
        t.color_space = space;
    }
}

/// The texture `e` marked a normal map of convention `n`, in the colour space its u32 at 25 states.
fn set_normal(w: &mut World, e: Entity, n: NormalConvention) {
    set_colour(w, e, TexRole::Normal);
    if let Some(t) = w.get_mut::<Texture>(e) {
        t.normal = Some(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tint_with_no_offsets_returns_the_colour() {
        for c in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0]] {
            let t = tint(c, 0.0, 0.0, 0.0);
            assert!((0..3).all(|k| (t[k] - c[k]).abs() < 1e-12), "{c:?} -> {t:?}");
        }
        // a full turn of hue is no change; pure red a third of a turn on is pure green
        let t = tint([1.0, 0.0, 0.0], 1.0 / 3.0, 0.0, 0.0);
        assert!(
            (t[0]).abs() < 1e-12 && (t[1] - 1.0).abs() < 1e-12 && t[2].abs() < 1e-12,
            "{t:?}"
        );
    }
}
