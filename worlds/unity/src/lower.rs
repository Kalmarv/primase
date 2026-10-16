//! Unity materials lowered to `pbr.material` (pass `lower-unity-pbr`), by shader:
//!
//! - **Universal Render Pipeline/Lit**, as URP's Lit input reads its properties: base colour `_BaseMap` x
//!   `_BaseColor`; metallic `_Metallic` and roughness 1 - `_Smoothness`, or with `_METALLICSPECGLOSSMAP` the map's R
//!   and 1 - A x `_Smoothness` (re-encoded as a glTF metallic-roughness texture, B and G); occlusion from the map's G,
//!   lerped from white by `_OcclusionStrength` (moved to R); `_BumpMap` x `_BumpScale` with `_NORMALMAP`;
//!   `_EmissionColor` x `_EmissionMap` with `_EMISSION` (HDR colour above 1: normalised, the rest as
//!   `emissive_strength`); `_Surface` 1 blended, `_AlphaClip` cut out at `_Cutoff`;
//!   `_Cull` 0 double-sided.
//! - **Universal Render Pipeline/Unlit**: `_BaseMap` x `_BaseColor`, drawn lit (counted).
//! - **Universal Render Pipeline/Nature/SpeedTree7**: `_MainTex` x `_Color`, cut out at `_Cutoff` with
//!   `_ALPHATEST_ON`, a rough dielectric (counted).
//! - **any other shader** (not modelled, counted by name): the base colour only, from Unity's usual property
//!   names ([`other_base`]).
//!
//! Colour properties are gamma-encoded as stored and become linear. The base map's tiling becomes every slot's
//! [`UvTransform`] (origin moved to top-left): URP Lit's vertex pass computes one coordinate,
//! `TRANSFORM_TEX(texcoord, _BaseMap)` (`LitForwardPass.hlsl`), and its surface input samples the base, metallic,
//! normal, occlusion and emission maps with it (`InitializeStandardLitSurfaceData`, `LitInput.hlsl`), so the other
//! properties' own tiling is never read. Detail maps are the one place a second transform applies (`_DetailAlbedoMap`'s
//! on top of the base's); they are not represented, and counted. A texture's wrap mode becomes its slot's wrap. Slots
//! whose texture has no pixels (not decoded) are left out and counted. Re-encoded maps are new texture entities, RGBA8,
//! every level: metallic-roughness and occlusion as above, and colour textures Unity samples as linear encoded as sRGB.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Name, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat,
    TexRole, Texture, World,
};
use primase_image::{linear_to_srgb, srgb_to_linear};
use primase_texcodec::{Decoder, TexDecoder};

use crate::components::{UnityMaterial, UnityTexEnv, UnityTexture};
use crate::{Result, invalid};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Lit,
    Unlit,
    SpeedTree,
    Other,
}

fn family(shader: Option<&str>) -> Family {
    match shader {
        Some("Universal Render Pipeline/Lit") => Family::Lit,
        Some("Universal Render Pipeline/Unlit") => Family::Unlit,
        Some("Universal Render Pipeline/Nature/SpeedTree7") => Family::SpeedTree,
        _ => Family::Other,
    }
}

/// The base colour properties of a shader not modelled, by Unity's usual names: the first texture property with a
/// texture of `_BaseMap`, `_MainTex`, `_Main_Tex`, `_Texture2D`, and the colour named with it by convention (`_BaseMap`
/// and `_BaseColor`, `_MainTex` and `_Color`), else the first colour of `_BaseColor`, `_Color`, `_Default_Color`,
/// `_Base_Color`.
fn other_base(m: &UnityMaterial) -> (&'static str, Option<&'static str>) {
    const TEXTURES: [&str; 4] = ["_BaseMap", "_MainTex", "_Main_Tex", "_Texture2D"];
    const COLORS: [&str; 4] = ["_BaseColor", "_Color", "_Default_Color", "_Base_Color"];
    let tex = TEXTURES
        .into_iter()
        .find(|t| m.texture(t).is_some())
        .unwrap_or("_BaseMap");
    let paired = match tex {
        "_BaseMap" => Some("_BaseColor"),
        "_MainTex" => Some("_Color"),
        _ => None,
    }
    .filter(|c| m.color(c).is_some());
    (
        tex,
        paired.or_else(|| COLORS.into_iter().find(|c| m.color(c).is_some())),
    )
}

/// A gamma-encoded colour as linear RGB and its alpha.
fn linear(c: [f32; 4]) -> [f32; 4] {
    let l = |x: f32| srgb_to_linear(f64::from(x)) as f32;
    [l(c[0]), l(c[1]), l(c[2]), c[3]]
}

fn byte(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A map re-encoded for a glTF slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Derived {
    /// From a metallic (R) smoothness (A) map with `_Smoothness` (its bits): glTF G roughness, B metallic.
    MetallicRoughness(u32),
    /// From an occlusion map's G: glTF R.
    Occlusion,
    /// From a colour texture Unity samples as linear: its colour channels encoded as sRGB (glTF's colour slots are).
    Srgb,
}

struct Lowering {
    log: PassLog,
    derived: BTreeMap<(Entity, Derived), Entity>,
}

impl Lowering {
    /// The slot for a material's texture property, sampled through `tiling` (the material's, see the module
    /// documentation): `None` (counted) when its texture has no pixels.
    fn slot(
        &mut self,
        w: &mut World,
        env: &UnityTexEnv,
        tiling: Option<UvTransform>,
        role: TexRole,
    ) -> Option<TextureRef> {
        let e = env.texture?;
        if !w.has::<Texture>(e) {
            self.log.count("slots left out: texture not decoded", 1);
            return None;
        }
        let ut = w.get::<UnityTexture>(e)?.clone();
        let wrap = ut.wrap.map(|m| match m {
            1 => Wrap::Clamp,
            2 => Wrap::Mirror,
            3 => {
                self.log.approximate("mirror-once wrap as mirror", 1);
                Wrap::Mirror
            }
            _ => Wrap::Repeat,
        });
        let t = w.get_mut::<Texture>(e)?;
        match role {
            TexRole::Normal if t.role != TexRole::Normal => {
                // URP unpacks X as red times alpha: red for an RGB map
                t.role = TexRole::Normal;
                t.color_space = ColorSpace::Linear;
                t.normal = Some(NormalConvention {
                    green: NormalGreen::Up,
                    z: NormalZ::Rebuild,
                    x: NormalX::Red,
                    basis: NormalBasis::TangentBitangent,
                });
                self.log.count("textures made normal maps by their slot", 1);
            }
            TexRole::Color if t.role == TexRole::Other && t.color_space == ColorSpace::Srgb => {
                t.role = TexRole::Color;
            }
            _ => {}
        }
        Some(TextureRef {
            texture: e,
            texcoord: 0,
            wrap,
            transform: tiling,
        })
    }

    /// A colour slot: the property's texture, or its sRGB re-encoding when Unity samples it as linear.
    fn color(
        &mut self,
        w: &mut World,
        env: &UnityTexEnv,
        tiling: Option<UvTransform>,
    ) -> Result<Option<TextureRef>> {
        let linear = env
            .texture
            .and_then(|e| w.get::<Texture>(e))
            .is_some_and(|t| t.color_space == ColorSpace::Linear);
        if linear {
            self.derived(w, env, tiling, Derived::Srgb)
        } else {
            Ok(self.slot(w, env, tiling, TexRole::Color))
        }
    }

    /// A slot whose texture is `kind` re-encoded from the property's texture.
    fn derived(
        &mut self,
        w: &mut World,
        env: &UnityTexEnv,
        tiling: Option<UvTransform>,
        kind: Derived,
    ) -> Result<Option<TextureRef>> {
        let Some(mut r) = self.slot(w, env, tiling, TexRole::Other) else {
            return Ok(None);
        };
        let source = r.texture;
        if let Some(&e) = self.derived.get(&(source, kind)) {
            r.texture = e;
            return Ok(Some(r));
        }
        let t = w.get::<Texture>(source).expect("slot checked").clone();
        // an sRGB-flagged map is decoded by the GPU before the shader reads it
        let decode = |x: u8| {
            let x = f64::from(x) / 255.0;
            if t.color_space == ColorSpace::Srgb {
                srgb_to_linear(x)
            } else {
                x
            }
        };
        let mut levels = Vec::with_capacity(t.levels.len());
        for (i, l) in t.levels.iter().enumerate() {
            let (lw, lh) = ((t.width >> i).max(1), (t.height >> i).max(1));
            let mut px = Decoder::default()
                .decode_rgba8(t.format, lw, lh, l)
                .map_err(|e| invalid(format!("re-encoding a map: {e}")))?;
            for p in px.as_chunks_mut::<4>().0 {
                *p = match kind {
                    Derived::MetallicRoughness(bits) => {
                        let smoothness = f64::from(p[3]) / 255.0 * f64::from(f32::from_bits(bits));
                        [255, byte(1.0 - smoothness), byte(decode(p[0])), 255]
                    }
                    Derived::Occlusion => {
                        let g = byte(decode(p[1]));
                        [g, g, g, 255]
                    }
                    Derived::Srgb => {
                        let e = |x: u8| byte(linear_to_srgb(f64::from(x) / 255.0));
                        [e(p[0]), e(p[1]), e(p[2]), p[3]]
                    }
                };
            }
            levels.push(px);
        }
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(
            e,
            Name(format!(
                "{name} ({})",
                match kind {
                    Derived::MetallicRoughness(_) => "metallic-roughness",
                    Derived::Occlusion => "occlusion",
                    Derived::Srgb => "sRGB",
                }
            )),
        );
        w.insert(
            e,
            Texture {
                width: t.width,
                height: t.height,
                format: TexFormat::Rgba8Unorm,
                levels,
                color_space: if kind == Derived::Srgb {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                },
                role: if kind == Derived::Srgb {
                    TexRole::Color
                } else {
                    TexRole::Mask
                },
                normal: None,
            },
        );
        self.log.approximate(
            match kind {
                Derived::MetallicRoughness(_) => {
                    "metallic-smoothness maps re-encoded as RGBA8 metallic-roughness"
                }
                Derived::Occlusion => "occlusion maps re-encoded as RGBA8, G moved to R",
                Derived::Srgb => "linear colour textures re-encoded as sRGB RGBA8",
            },
            1,
        );
        self.derived.insert((source, kind), e);
        r.texture = e;
        Ok(Some(r))
    }

    fn lower(&mut self, w: &mut World, m: &UnityMaterial) -> Result<PbrMaterial> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        let fam = family(m.shader.as_deref());
        let mut p = PbrMaterial {
            metallic: 0.0,
            roughness: 1.0,
            ..PbrMaterial::default()
        };
        let (tex, color) = match fam {
            Family::Lit | Family::Unlit => ("_BaseMap", Some("_BaseColor")),
            Family::SpeedTree => ("_MainTex", Some("_Color")),
            Family::Other => other_base(m),
        };
        p.base_color = linear(color.and_then(|c| m.color(c)).unwrap_or([1.0; 4]));
        // the base property's tiling, read whether or not it has a texture
        let tiling = m
            .textures
            .iter()
            .find(|env| env.name == tex)
            .filter(|env| env.scale != [1.0; 2] || env.offset != [0.0; 2])
            .map(|env| UvTransform::from_bottom_left(env.scale, env.offset));
        if let Some(env) = m.texture(tex) {
            p.base_color_texture = self.color(w, env, tiling)?;
        }
        let clip = f("_AlphaClip", 0.0) == 1.0 || m.keyword("_ALPHATEST_ON");
        let blend = f("_Surface", 0.0) == 1.0;
        p.alpha_mode = match (clip, blend) {
            (true, b) => {
                if b {
                    self.log.approximate("transparent and cut out: drawn cut out", 1);
                }
                AlphaMode::Mask {
                    cutoff: f("_Cutoff", 0.5),
                }
            }
            (false, true) => {
                if f("_Blend", 0.0) != 0.0 {
                    self.log.approximate(
                        "premultiplied, additive or multiply blending as alpha blending",
                        1,
                    );
                }
                AlphaMode::Blend
            }
            (false, false) => AlphaMode::Opaque,
        };
        match f("_Cull", 2.0) as i64 {
            0 => p.double_sided = true,
            1 => self
                .log
                .approximate("front faces culled: drawn as back faces culled", 1),
            _ => {}
        }
        match fam {
            Family::Lit => self.lit(w, m, tiling, &mut p)?,
            Family::Unlit => self.log.approximate("unlit materials drawn lit", 1),
            Family::SpeedTree => self
                .log
                .approximate("SpeedTree materials as rough dielectrics", 1),
            Family::Other => self.log.approximate(
                &format!(
                    "shader not modelled, base colour only: {}",
                    m.shader.as_deref().unwrap_or("(none)")
                ),
                1,
            ),
        }
        Ok(p)
    }

    fn lit(
        &mut self,
        w: &mut World,
        m: &UnityMaterial,
        tiling: Option<UvTransform>,
        p: &mut PbrMaterial,
    ) -> Result<()> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        let smoothness = f("_Smoothness", 0.5);
        if f("_WorkflowMode", 1.0) == 0.0 {
            self.log.approximate("specular workflow as a dielectric", 1);
            p.roughness = 1.0 - smoothness;
        } else {
            p.metallic = f("_Metallic", 0.0);
            p.roughness = 1.0 - smoothness;
            if m.keyword("_METALLICSPECGLOSSMAP")
                && let Some(env) = m.texture("_MetallicGlossMap")
            {
                if f("_SmoothnessTextureChannel", 0.0) == 1.0 {
                    self.log
                        .approximate("smoothness in the base map's alpha: from _Smoothness only", 1);
                    p.metallic_roughness_texture = None;
                } else if let Some(r) =
                    self.derived(w, env, tiling, Derived::MetallicRoughness(smoothness.to_bits()))?
                {
                    p.metallic_roughness_texture = Some(r);
                    p.metallic = 1.0;
                    p.roughness = 1.0;
                }
            }
        }
        if m.keyword("_OCCLUSIONMAP")
            && let Some(env) = m.texture("_OcclusionMap")
        {
            p.occlusion_texture = self.derived(w, env, tiling, Derived::Occlusion)?;
            p.occlusion_strength = f("_OcclusionStrength", 1.0);
        }
        if m.keyword("_NORMALMAP")
            && let Some(env) = m.texture("_BumpMap")
        {
            p.normal_texture = self.slot(w, env, tiling, TexRole::Normal);
            p.normal_scale = f("_BumpScale", 1.0);
        }
        if m.keyword("_EMISSION") {
            let c = linear(m.color("_EmissionColor").unwrap_or([0.0, 0.0, 0.0, 1.0]));
            // HDR emission: the factor normalised by its largest channel, the rest as the strength
            let max = c[0].max(c[1]).max(c[2]);
            if max > 1.0 {
                p.emissive = std::array::from_fn(|k| (f64::from(c[k]) / f64::from(max)) as f32);
                p.emissive_strength = max;
                self.log.count("HDR emission kept as emissive strength", 1);
            } else {
                p.emissive = [c[0], c[1], c[2]];
            }
            if let Some(env) = m.texture("_EmissionMap") {
                p.emissive_texture = self.color(w, env, tiling)?;
            }
        }
        for (k, what) in [
            ("_DETAIL_MULX2", "detail maps not represented"),
            ("_DETAIL_SCALED", "detail maps not represented"),
            ("_PARALLAXMAP", "height (parallax) maps not represented"),
        ] {
            if m.keyword(k) {
                self.log.approximate(what, 1);
            }
        }
        Ok(())
    }
}

/// Lower every `unity.material` to a `pbr.material` on the same entity (see the module documentation).
pub fn lower_unity_pbr(w: &mut World) -> Result<PassLog> {
    let mut l = Lowering {
        log: PassLog::default(),
        derived: BTreeMap::new(),
    };
    let materials: Vec<(Entity, UnityMaterial)> =
        w.iter::<UnityMaterial>().map(|(e, m)| (e, m.clone())).collect();
    for (e, m) in materials {
        let p = l.lower(w, &m)?;
        w.insert(e, p);
        l.log.count("materials lowered", 1);
    }
    Ok(l.log)
}

/// [`lower_unity_pbr`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct LowerUnityPbr;

impl Pass for LowerUnityPbr {
    fn name(&self) -> &'static str {
        "lower-unity-pbr"
    }
    fn requires(&self) -> &[&'static str] {
        &["unity.material"]
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.material", "core.texture"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn approximates(&self) -> &'static str {
        "shaders other than URP Lit by their base colour; maps re-encoded as RGBA8 metallic-roughness and occlusion; \
         blend modes, detail and parallax maps not represented"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_pbr(w).map_err(|e| e.to_string())?;
        for (k, n) in r.counts {
            log.count(&k, n);
        }
        for (k, n) in r.approximations {
            log.approximate(&k, n);
        }
        Ok(())
    }
}
