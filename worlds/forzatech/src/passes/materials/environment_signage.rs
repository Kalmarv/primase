//! # `environment_signage`
//!
//! Evidence, over the 19 GBuffer permutations of the family in the Tokyo track's `bin.zip` (each render target channel
//! followed back to its texture reads and parameter rows through the family's `CBMP` and `TXMP` maps; the texture
//! indices of this family's parameter buffer start at dword 236, one row before the constants' end, which is the base
//! that makes the normal's reads land on `NormHeightTex_A`):
//! - the metalness (target 0 W) is `RoughMetAoPorTex_A.g`, the roughness (target 1 W) `saturate(RoughMetAoPorTex_A.r)`
//!   with wetness off, and the normal (target 2) the standard decode of `NormHeightTex_A.rg`, all read at `uv0`
//!   (`TEXCOORD0`) with the environment family's tilings: the signage shaders are the environment shaders with a
//!   sign face added;
//! - the albedo (target 0 XYZ) of every `_Signage*` permutation is the sign: `Signage_DiffTex` read at the cell
//!   `((min(columns - 1, AtlasIndex.x - 1) + uv3.x) / columns, (min(rows - 1, AtlasIndex.y - 1) + uv3.y) / rows)` of an
//!   atlas of `AtlasImages` (columns, rows) cells on `TEXCOORD3`, the sign face, through a Catmull-Rom filter (the
//!   `BiCubic` permutations; bilinear otherwise). The sample is taken through the format's own view, raised to 1/2.2,
//!   darkened by wetness and raised to 2.2: a dry surface returns the linear sample. Without `ColorMask` (the
//!   permutations without `_CM`) the sample is the colour; with it the sample is a mask of up to four channels and
//!   the colour is `c = lerp(BackgroundColor, RColor, m.r)`, then with `Signage_UseFiveColorMask` also
//!   `lerp(c, GColor, m.g)`, `lerp(c, BColor, m.b)`, `lerp(c, AColor, m.a)`, each `m` first `saturate((m - 0.4) * 5)`
//!   when `Signage_UseSteppedMask`; the result is saturated. The `Signage*Mural*` permutations lerp the wall
//!   (`DiffAlphaTex_A`, `uv0`) to that colour by `MuralTexture.r` (the same cell). The albedo is then multiplied by a
//!   factor of `AoWetBlendContextTex.r` (`uv1`, tiled by `MacroAOWetBlendMaskTiling`) and scene constants;
//! - the only `discard` compares a constant of the draw with one: the surface is opaque;
//! - the emissive term of the `_SignageEmissive*` permutations (target 1 XYZ) is
//!   `EmissiveTexture.rgb * EmissiveTint.rgb * (the sign colour above)` times `GIEmissiveScale`, an instance-variation
//!   factor (`LightEmisVariationOnOff`) and the distance encoding `g(d)`; `EmissiveTexture` is read in the same cell of
//!   the same `uv3` atlas, or at `uv4` when `EmissiveAltUV` is on;
//! - the scrolling, rotating and scaling of the four mask channels (`Red_` ... `Alpha_`), `RandomiseIndexOnOff` and
//!   `_AnimatedSign` are time-driven and off in every material of the Shibuya Scramble box.
//!
//! The rule. A material is sampled through one coordinate set, and `first-texcoord-set` makes that set the first
//! when every slot of every material of a group samples the same one, so the sign samples only `TEXCOORD3`: the
//! base colour is `Signage_DiffTex` as it is when it is a colour atlas stored sRGB, or the atlas painted with the
//! material's colours (every texel through the lerps above, saturated) when `ColorMask` is on, and the emission is
//! `EmissiveTexture * the sign colour` baked into one atlas of the same cell grid, with `EmissiveTint` as the
//! emissive factor (normalised to 0-1, the rest in `emissive_strength`). Each is read through the cell as a texture
//! transform (`scale = 1 / (columns, rows)`, `offset = cell / (columns, rows)`) clamped at its edge. The material is
//! opaque, dielectric and rough. A `Signage = 0` mural material takes the environment rule (its maps are all on
//! `TEXCOORD0`).
//!
//! What this costs, per material and per triangle. The normal, roughness and metalness maps (`TEXCOORD0`) and the
//! ambient-occlusion map (`TEXCOORD1`) are left out: they sample other coordinate sets than the sign face, and could
//! only come back by re-baking them into the sign face's texture space, a texture bake and not a lowering. Also left
//! out, named: emission of an `EmissiveAltUV` material (its texture is on `TEXCOORD4`), `GIEmissiveScale` and the
//! distance encoding, the instance-variation factor, modulate, the lightmap, the LED screen effect, retro-reflection,
//! the bicubic filter (the renderer's own filter is used), wetness and lighting (run-time), and the mural blend. A
//! material one of whose primitives lacks `TEXCOORD3` takes its background colour and no texture.
use super::{Binding, Lowering, Params, approximate, left_out, seen};
use crate::components::{ForzaMaterial, MaterialTextures};
use primase_core::pass::PassLog;
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{ColorSpace, Entity, Name, TexRole, Texture, World};
use primase_foundations::color::linear_to_srgb;
use primase_image::{Image, Samples, TextureMetadata, map_rgba8_levels};

/// The coordinate set of the sign face.
const SIGN_FACE: u8 = 3;

/// The colours a colour-masked sign is painted with (linear: background, R, G, B, A) and how its mask is read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Paint {
    colours: [[f32; 3]; 5],
    five: bool,
    stepped: bool,
}

impl Paint {
    fn key(&self) -> Vec<u32> {
        let mut key: Vec<u32> = self.colours.iter().flatten().map(|c| c.to_bits()).collect();
        key.push(u32::from(self.five));
        key.push(u32::from(self.stepped));
        key
    }

    /// The colour of a texel whose mask channels (as a shader reads them) are `m`.
    fn colour(&self, m: [f64; 4]) -> [f64; 3] {
        let step = |x: f64| {
            if self.stepped {
                ((x - 0.4) * 5.0).clamp(0.0, 1.0)
            } else {
                x
            }
        };
        let mut c = self.colours[0].map(f64::from);
        let layers = if self.five { 4 } else { 1 };
        for (k, mask) in m.iter().take(layers).enumerate() {
            let t = step(*mask);
            let target = self.colours[k + 1];
            for (c, target) in c.iter_mut().zip(target) {
                *c += (f64::from(target) - *c) * t;
            }
        }
        c.map(|x| x.clamp(0.0, 1.0))
    }
}

/// Bilinear sample of an 8-bit image at normalised `(u, v)`, clamped at the edge; channels as stored.
fn sample(image: &Image, u: f64, v: f64) -> [f64; 4] {
    let Samples::U8(data) = &image.samples else {
        return [0.0; 4];
    };
    let (w, h) = (image.width as usize, image.height as usize);
    let x = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
    let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let at = |x: usize, y: usize, c: usize| f64::from(data.get((y * w + x) * 4 + c).copied().unwrap_or(0));
    let mut out = [0.0; 4];
    for (c, out) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        *out = top * (1.0 - fy) + bottom * fy;
    }
    out
}

/// Level 0 of `t` as RGBA8, as stored. `None` when it has no pixels.
fn decode(t: &Texture) -> Result<Option<Image>, String> {
    let mut data = Vec::new();
    let mut size = None;
    if t.levels.is_empty() {
        return Ok(None);
    }
    map_rgba8_levels(
        t,
        0..1,
        TextureMetadata {
            color_space: t.color_space,
            role: t.role,
            normal: None,
        },
        |texel| {
            size = Some((texel.width, texel.height));
            data.extend_from_slice(&texel.rgba);
            Ok(texel.rgba)
        },
    )
    .map_err(|e| format!("decoding a texture: {e}"))?;
    Ok(size.map(|(width, height)| Image {
        width,
        height,
        samples: Samples::U8(data),
    }))
}

fn srgb_byte(linear: f64) -> u8 {
    (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

impl Lowering {
    /// An sRGB colour atlas made of `atlas`: its texels painted with `paint` (or as they are, when `None`), times
    /// `emissive`'s texel at the same normalised position when given. Made once per source, paint and emission map.
    /// `what` names the atlas in the texture's name and the counts (`sign`, `environment`).
    pub(super) fn bake_sign(
        &mut self,
        w: &mut World,
        atlas: Entity,
        paint: Option<&Paint>,
        emissive: Option<Entity>,
        what: &str,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        let mut key = paint.map(Paint::key).unwrap_or_default();
        key.push(u32::from(paint.is_some()));
        if let Some(&e) = self.baked.get(&(atlas, key.clone(), emissive)) {
            return Ok(Some(e));
        }
        let (Some(source), emission) = (
            w.get::<Texture>(atlas),
            emissive.map(|e| w.get::<Texture>(e).map(|t| (t.color_space, decode(t)))),
        ) else {
            log.count(&format!("{what} atlases left out: no pixels"), 1);
            return Ok(None);
        };
        let emission = match emission {
            None => None,
            Some(None) => {
                log.count(&format!("{what} emission maps left out: no pixels"), 1);
                return Ok(None);
            }
            Some(Some((space, image))) => match image? {
                Some(image) => Some((space, image)),
                None => {
                    log.count(&format!("{what} emission maps left out: no pixels"), 1);
                    return Ok(None);
                }
            },
        };
        let space = source.color_space;
        let texture = map_rgba8_levels(
            source,
            0..source.levels.len(),
            TextureMetadata {
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                let m = [r, g, b, a].map(|x| seen(space, x));
                let mut colour = match paint {
                    Some(paint) => paint.colour(m),
                    None => [m[0], m[1], m[2]],
                };
                if let Some((em_space, image)) = &emission {
                    let u = (f64::from(texel.x) + 0.5) / f64::from(texel.width);
                    let v = (f64::from(texel.y) + 0.5) / f64::from(texel.height);
                    let em = sample(image, u, v);
                    for (c, e) in colour.iter_mut().zip(em) {
                        *c *= seen(*em_space, e.round().clamp(0.0, 255.0) as u8);
                    }
                }
                Ok([
                    srgb_byte(colour[0]),
                    srgb_byte(colour[1]),
                    srgb_byte(colour[2]),
                    255,
                ])
            },
        )
        .map_err(|e| format!("painting a {what} atlas: {e}"))?;
        let name = w.get::<Name>(atlas).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(
            e,
            Name(format!(
                "{name} ({what} {})",
                if emission.is_some() { "emission" } else { "colour" }
            )),
        );
        w.insert(e, texture);
        log.count(
            &format!(
                "{what} atlases baked as {} maps",
                if emissive.is_some() { "emission" } else { "colour" }
            ),
            1,
        );
        self.baked.insert((atlas, key, emissive), e);
        Ok(Some(e))
    }

    /// The `environment_signage` family's material.
    pub(super) fn environment_signage(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if !p.on("Signage") {
            // a mural: the wall's maps are all on TEXCOORD0, the environment rule's
            let pbr = self.environment(w, e, m, log)?;
            approximate(
                log,
                e,
                "mural: the sign blended over the wall by MuralTexture left out",
                triangles,
            );
            return Ok(pbr);
        }
        let face = self.has_set(e, SIGN_FACE);
        let mut pbr = PbrMaterial {
            metallic: 0.0,
            roughness: 1.0,
            ..PbrMaterial::default()
        };

        // the cell of the atlas the sign face shows
        let cell = match (p.vec2("AtlasImages"), p.vec2("AtlasIndex")) {
            (Some(n), Some(i)) if n.iter().all(|x| *x >= 1.0) && i.iter().all(|x| x.is_finite()) => {
                let (columns, rows) = (n[0].round(), n[1].round());
                let (x, y) = ((i[0] - 1.0).floor(), (i[1] - 1.0).floor());
                let (x, y) = (x.clamp(0.0, columns - 1.0), y.clamp(0.0, rows - 1.0));
                Some(UvTransform {
                    offset: [x / columns, y / rows],
                    scale: [1.0 / columns, 1.0 / rows],
                    rotation: 0.0,
                })
            }
            _ => {
                approximate(
                    log,
                    e,
                    "no atlas cell (AtlasImages, AtlasIndex): the whole atlas",
                    triangles,
                );
                None
            }
        };
        let slot = |texture: Entity| TextureRef {
            texcoord: SIGN_FACE,
            wrap: [Wrap::Clamp; 2],
            transform: cell.filter(|t| *t != UvTransform::IDENTITY),
            ..TextureRef::new(texture)
        };

        // the paint of a colour-masked sign
        let colour_mask = p.on("ColorMask");
        let paint = if colour_mask {
            let colour = |name: &str| p.vec4(name).map(|c| [c[0], c[1], c[2]]);
            match (
                colour("Signage_BackGroundColor"),
                colour("Signage_RColor"),
                colour("Signage_GColor"),
                colour("Signage_BColor"),
                colour("Signage_AColor"),
            ) {
                (Some(bg), Some(r), Some(g), Some(b), Some(a)) => Some(Paint {
                    colours: [bg, r, g, b, a],
                    five: p.on("Signage_UseFiveColorMask"),
                    stepped: p.on("Signage_UseSteppedMask"),
                }),
                _ => None,
            }
        } else {
            None
        };

        let mut atlas_baked = None;
        match p.texture("Signage_DiffTex") {
            Binding::Texture(t) if face => {
                let srgb = w
                    .get::<Texture>(t)
                    .is_some_and(|x| x.color_space == ColorSpace::Srgb);
                if colour_mask && paint.is_none() {
                    approximate(log, e, "colour-mask paint absent: no sign texture", triangles);
                } else if !colour_mask && srgb {
                    if let Some(tex) = w.get_mut::<Texture>(t)
                        && tex.role == TexRole::Other
                    {
                        tex.role = TexRole::Color;
                    }
                    pbr.base_color_texture = Some(slot(t));
                    atlas_baked = Some(t);
                } else if let Some(baked) = self.bake_sign(w, t, paint.as_ref(), None, "sign", log)? {
                    pbr.base_color_texture = Some(slot(baked));
                    atlas_baked = Some(t);
                }
            }
            Binding::Texture(_) => {
                if let Some(bg) = p.vec4("Signage_BackGroundColor").filter(|_| colour_mask) {
                    pbr.base_color = [bg[0].max(0.0), bg[1].max(0.0), bg[2].max(0.0), 1.0];
                }
                approximate(
                    log,
                    e,
                    "a primitive without the sign face (TEXCOORD3): the background colour, no sign texture",
                    triangles,
                );
            }
            Binding::Absent => approximate(log, e, "sign atlas in no archive: white", triangles),
            Binding::None => approximate(log, e, "no sign atlas: white", triangles),
        }

        // emission: the sign's colour through EmissiveTexture, where that is on the sign face too
        if p.on("Emissive") {
            if p.on("EmissiveAltUV") {
                approximate(
                    log,
                    e,
                    "emission texture on TEXCOORD4 (EmissiveAltUV) left out: a material is sampled through one coordinate set",
                    triangles,
                );
            } else if let (Binding::Texture(em), Some(atlas), Some(tint)) =
                (p.texture("EmissiveTexture"), atlas_baked, p.vec4("EmissiveTint"))
            {
                let tint = [tint[0].max(0.0), tint[1].max(0.0), tint[2].max(0.0)];
                let strength = tint.iter().copied().fold(0.0f32, f32::max);
                if strength > 0.0
                    && let Some(baked) = self.bake_sign(w, atlas, paint.as_ref(), Some(em), "sign", log)?
                {
                    pbr.emissive_texture = Some(slot(baked));
                    pbr.emissive = tint.map(|c| c / strength.max(1.0));
                    pbr.emissive_strength = strength.max(1.0);
                    approximate(
                        log,
                        e,
                        "emission without GIEmissiveScale, the instance-variation factor and the distance encoding",
                        triangles,
                    );
                }
            } else {
                approximate(
                    log,
                    e,
                    "emission left out: no emission or sign texture",
                    triangles,
                );
            }
        }

        if matches!(p.texture("NormHeightTex_A"), Binding::Texture(_))
            || matches!(p.texture("RoughMetAoPorTex_A"), Binding::Texture(_))
        {
            approximate(
                log,
                e,
                "normal, roughness and metalness maps (TEXCOORD0) left out: dielectric, rough",
                triangles,
            );
        }
        if matches!(p.texture("AoWetBlendContextTex"), Binding::Texture(_)) {
            approximate(log, e, "occlusion map (TEXCOORD1) left out", triangles);
        }
        approximate(
            log,
            e,
            "wetness, lighting and scene occlusion (run-time) left out: dry",
            triangles,
        );
        for (switch, what) in [
            ("Modulate", "modulate texture left out"),
            ("Lightmap", "lightmap left out"),
            ("LEDScreen", "LED screen effect left out"),
            ("_AnimatedSign", "sign animation left out"),
            (
                "RandomiseIndexOnOff",
                "randomised atlas index left out: the material's cell",
            ),
            ("RetroReflective", "retro-reflection left out"),
            (
                "BiCubic",
                "bicubic filtering of the sign left out: the renderer's own filter",
            ),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(pbr)
    }
}
