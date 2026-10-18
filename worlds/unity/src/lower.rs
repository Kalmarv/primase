//! Unity materials lowered to `pbr.material` (pass `lower-unity-pbr`), by shader:
//!
//! - **Universal Render Pipeline/Lit**, as URP's Lit input reads its properties: base colour `_BaseMap` x
//!   `_BaseColor`; metallic `_Metallic` and roughness 1 - `_Smoothness`, or with `_METALLICSPECGLOSSMAP` the map's R
//!   and 1 - A x `_Smoothness` (re-encoded as a glTF metallic-roughness texture, B and G); occlusion from the map's G,
//!   lerped from white by `_OcclusionStrength` (moved to R); `_BumpMap` x `_BumpScale` with `_NORMALMAP`;
//!   `_EmissionColor` x `_EmissionMap` with `_EMISSION` (HDR colour above 1: normalised, the rest as
//!   `emissive_strength`); `_Cull` 0 double-sided.
//! - **Universal Render Pipeline/Unlit**: `_BaseMap` x `_BaseColor`, drawn lit (counted).
//! - **Universal Render Pipeline/Nature/SpeedTree7**: `_MainTex` x `_Color`, a rough dielectric (counted).
//! - **any other shader** (not modelled, counted by name): the base colour only, from Unity's usual property
//!   names ([`other_base`]).
//!
//! Every shader's alpha mode is what Unity draws it with: the draw pass of the subshader the player's pipeline takes
//! ([`UnityMaterial::draw_pass`]: URP's forward pass, built-in's `ForwardBase`, not a per-light or shadow pass) and
//! its blend factors, bound ones read from the material. One Zero is opaque; any other blending is alpha blending
//! (other than SrcAlpha OneMinusSrcAlpha counted). One case is drawn opaque, counted, as before this rule: a pass
//! that blends while writing depth in Unity's opaque queue range (up to 2500). Lowering it as Blend needs the fact
//! core cannot carry yet, the depth write, beside `core.draw_order`, so that targets choose their own approximation;
//! until then the blend factors stay in `unity.material` and the queue in `core.draw_order`. Cut out at `_Cutoff`
//! with the `_ALPHATEST_ON` keyword, URP's `_AlphaClip`, or the render type `TransparentCutout` (the material's tag,
//! else its subshader's: legacy cutout shaders clip in their programs).
//!
//! Colour properties are gamma-encoded as stored and become linear. The base map's tiling becomes every slot's
//! [`UvTransform`] (origin moved to top-left): URP Lit's vertex pass computes one coordinate,
//! `TRANSFORM_TEX(texcoord, _BaseMap)` (`LitForwardPass.hlsl`), and its surface input samples the base, metallic,
//! normal, occlusion and emission maps with it (`InitializeStandardLitSurfaceData`, `LitInput.hlsl`), so the other
//! properties' own tiling is never read. Detail maps are the one place a second transform applies (`_DetailAlbedoMap`'s
//! on top of the base's); they are not represented, and counted. A texture's wrap mode becomes its slot's wrap. Slots
//! whose texture has no pixels (not decoded) are left out and counted. Re-encoded maps are new texture entities, RGBA8,
//! every level: metallic-roughness and occlusion as above, and colour textures Unity samples as linear encoded as sRGB.
//!
//! Each material also gets its draw order (Unity's render queue, the layer its alpha mode is) and, where its shader
//! states it, its usage (`core.material_usage`, the named table [`SHADER_USAGE`]); the rest are counted by shader.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Name, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexRole, Texture,
    World,
};
use primase_core::{DrawLayer, DrawOrder, MaterialUsage};
use primase_image::{linear_to_srgb, srgb_to_linear};

use crate::components::{UnityMaterial, UnityRenderSettings, UnityTexEnv, UnityTexture};
use crate::material::SubShader;
use crate::player::RenderPipeline;
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

/// Whether the lowering models a shader (URP Lit, URP Unlit, SpeedTree 7) rather than taking its base colour only.
pub fn modelled(shader: Option<&str>) -> bool {
    family(shader) != Family::Other
}

/// The shaders whose family states what their surfaces are (`core.material_usage`): FlatKit's water shader draws a
/// water surface; URP's SpeedTree 7 shader draws a tree's vegetation (wind sway, hue variation, alpha test), except a
/// material whose geometry keyword is bark ([`SPEEDTREE_BARK`]). A shader's own name decides nothing else: Sledding's
/// `Shader Graphs/ocean` draws a spray-paint bottle.
const SHADER_USAGE: [(&str, MaterialUsage); 2] = [
    ("FlatKit/Water", MaterialUsage::Water),
    (
        "Universal Render Pipeline/Nature/SpeedTree7",
        MaterialUsage::Foliage,
    ),
];

/// SpeedTree 7's geometry-type keywords for branches (bark and its detail layer), which the importer sets on the
/// material it makes for each geometry type; leaf, frond and mesh geometry stay vegetation.
const SPEEDTREE_BARK: [&str; 2] = ["GEOM_TYPE_BRANCH", "GEOM_TYPE_BRANCH_DETAIL"];

/// What a material's surfaces are, as its shader states it ([`SHADER_USAGE`]); `None` for an ordinary surface.
pub fn usage(m: &UnityMaterial) -> Option<MaterialUsage> {
    let (_, u) = SHADER_USAGE
        .iter()
        .find(|(s, _)| m.shader.as_deref() == Some(*s))?;
    let bark =
        family(m.shader.as_deref()) == Family::SpeedTree && SPEEDTREE_BARK.iter().any(|k| m.keyword(k));
    (!bark).then_some(*u)
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
    /// The player's render pipeline (`unity.render_settings`), which decides the pass a material draws with.
    pipeline: Option<RenderPipeline>,
}

/// The last render queue Unity draws with the opaque objects (`RenderQueue.GeometryLast`; `RenderQueueRange.opaque`
/// is 0 to 2500, transparent 2501 to 5000), in both the built-in renderer and URP.
const GEOMETRY_LAST: i32 = 2500;

/// How a draw pass's blend factors (source, destination; `None` when not `BlendMode` values) are drawn: `None` for
/// One Zero (opaque: the surface replaces what is behind); SrcAlpha OneMinusSrcAlpha is alpha blending (`Some(None)`);
/// any other blending is drawn as alpha blending, named for the count (`Some(Some(name))`).
fn blending(factors: Option<(u8, u8)>) -> Option<Option<&'static str>> {
    use crate::material::blend_mode::{
        DST_COLOR, ONE, ONE_MINUS_SRC_ALPHA, ONE_MINUS_SRC_COLOR, SRC_ALPHA, SRC_COLOR, ZERO,
    };
    let Some((src, dst)) = factors else {
        return Some(Some(
            "blend factors that are not BlendMode values as alpha blending",
        ));
    };
    Some(match (src, dst) {
        (ONE, ZERO) => return None,
        (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) => None,
        (ONE, ONE_MINUS_SRC_ALPHA) => Some("premultiplied blending (One OneMinusSrcAlpha) as alpha blending"),
        (_, ONE) => Some("additive blending (destination One) as alpha blending"),
        (ONE, ONE_MINUS_SRC_COLOR) => Some("soft additive blending (One OneMinusSrcColor) as alpha blending"),
        (DST_COLOR, ZERO) | (ZERO, SRC_COLOR) | (DST_COLOR, SRC_COLOR) => {
            Some("multiplicative blending as alpha blending")
        }
        _ => Some("other blend factors as alpha blending"),
    })
}

/// A material's render type: its own `RenderType` tag (`stringTagMap`, which Unity's material editors set, Standard's
/// `TransparentCutout` among them), else its subshader's.
fn render_type<'a>(m: &'a UnityMaterial, sub: Option<&'a SubShader>) -> Option<&'a str> {
    let find = |tags: &'a [(String, String)]| {
        tags.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("RenderType"))
            .map(|(_, v)| v.as_str())
    };
    find(&m.tags).or_else(|| sub.and_then(|s| find(&s.tags)))
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
        let texture = primase_image::map_rgba8_levels(
            &t,
            0..t.levels.len(),
            primase_image::TextureMetadata {
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
            |texel| {
                let p = texel.rgba;
                Ok(match kind {
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
                })
            },
        )
        .map_err(|e| invalid(format!("re-encoding a map: {e}")))?;
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
        w.insert(e, texture);
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
        let sub = self.pipeline.as_ref().and_then(|pl| m.subshader(pl));
        let pass = self.pipeline.as_ref().and_then(|pl| m.draw_pass(pl));
        let clip = if m.keyword("_ALPHATEST_ON") {
            Some("the _ALPHATEST_ON keyword")
        } else if f("_AlphaClip", 0.0) == 1.0 {
            Some("_AlphaClip")
        } else if render_type(m, sub).is_some_and(|t| t.eq_ignore_ascii_case("TransparentCutout")) {
            Some("render type TransparentCutout")
        } else {
            None
        };
        let blend = match pass {
            Some(pass) => blending(m.blend_factors(pass)),
            None => {
                self.log.count(
                    match (&self.pipeline, sub) {
                        (None, _) => "alpha: render pipeline not known, blending not read",
                        (Some(RenderPipeline::Other(_)), _) => {
                            "alpha: render pipeline not modelled, blending not read"
                        }
                        (Some(_), None) => "alpha: no subshader for the pipeline, blending not read",
                        (Some(_), Some(_)) => "alpha: no draw pass in the subshader, blending not read",
                    },
                    1,
                );
                None
            }
        };
        p.alpha_mode = match (clip, blend) {
            (Some(by), b) => {
                self.log.count(&format!("cut out by {by}"), 1);
                if b.is_some() {
                    self.log.approximate("blended and cut out: drawn cut out", 1);
                }
                AlphaMode::Mask {
                    cutoff: f("_Cutoff", 0.5),
                }
            }
            // blending that writes depth among the opaque objects: kept opaque until core carries depth writes
            (None, Some(_))
                if pass.is_some_and(|p| m.state_value(&p.zwrite) != 0.0)
                    && m.render_queue(self.pipeline.as_ref()).0 <= GEOMETRY_LAST =>
            {
                self.log.approximate("blending that writes depth in the opaque queue range (glTF blending neither writes depth nor draws with opaque surfaces): drawn opaque", 1);
                AlphaMode::Opaque
            }
            (None, Some(other)) => {
                if let Some(what) = other {
                    self.log.approximate(what, 1);
                }
                AlphaMode::Blend
            }
            (None, None) => AlphaMode::Opaque,
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
        pipeline: w
            .resource::<UnityRenderSettings>()
            .and_then(|s| s.pipeline.clone()),
    };
    let materials: Vec<(Entity, UnityMaterial)> =
        w.iter::<UnityMaterial>().map(|(e, m)| (e, m.clone())).collect();
    for (e, m) in materials {
        let p = l.lower(w, &m)?;
        // where it comes in the draw sequence: Unity's render queue; the layer its lowered alpha mode is
        let (queue, from) = m.render_queue(l.pipeline.as_ref());
        l.log.count(&format!("draw order from {from}"), 1);
        let layer = match p.alpha_mode {
            AlphaMode::Opaque => DrawLayer::Opaque,
            AlphaMode::Mask { .. } => DrawLayer::Cutout,
            AlphaMode::Blend => DrawLayer::Transparent,
        };
        w.insert(e, DrawOrder { layer, queue });
        let shader = m.shader.as_deref().unwrap_or("no shader");
        match usage(&m) {
            Some(u) => {
                l.log.count(&format!("material usage {u:?} ({shader})"), 1);
                w.insert(e, u);
            }
            None => l.log.count(&format!("materials without a usage ({shader})"), 1),
        }
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
        &[
            "pbr.material",
            "core.texture",
            "core.draw_order",
            "core.material_usage",
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    /// It lowers Unity materials and keeps them.
    fn legalizes(&self) -> &[&'static str] {
        &["unity.material"]
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::components::UnityObject;
    use crate::material::fixture;

    fn material(path_id: i64, shader: Option<&str>, keywords: &[&str]) -> UnityMaterial {
        UnityMaterial {
            material: UnityObject {
                file: "sharedassets1.assets".into(),
                path_id,
            },
            shader: shader.map(Into::into),
            shader_state: Vec::new(),
            textures: Vec::new(),
            floats: Vec::new(),
            ints: Vec::new(),
            colors: Vec::new(),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            tags: Vec::new(),
            render_queue: -1,
        }
    }

    /// Water and SpeedTree leaves state their usage; SpeedTree bark, a shader named like water but not in the table,
    /// URP Lit and a material without a shader are ordinary, each counted by shader.
    #[test]
    fn water_and_tree_shaders_state_their_usage() {
        let speedtree = Some("Universal Render Pipeline/Nature/SpeedTree7");
        let cases = [
            (
                material(1, Some("FlatKit/Water"), &[]),
                Some(MaterialUsage::Water),
            ),
            (
                material(2, speedtree, &["EFFECT_HUE_VARIATION", "GEOM_TYPE_MESH"]),
                Some(MaterialUsage::Foliage),
            ),
            (
                material(3, speedtree, &["GEOM_TYPE_LEAF"]),
                Some(MaterialUsage::Foliage),
            ),
            (material(4, speedtree, &["GEOM_TYPE_BRANCH"]), None),
            (material(5, speedtree, &["GEOM_TYPE_BRANCH_DETAIL"]), None),
            (material(6, Some("Shader Graphs/ocean"), &[]), None),
            (material(7, Some("Universal Render Pipeline/Lit"), &[]), None),
            (material(8, None, &[]), None),
        ];
        let mut w = World::new();
        let entities: Vec<Entity> = cases
            .iter()
            .map(|(m, _)| {
                let e = w.spawn();
                w.insert(e, m.clone());
                e
            })
            .collect();
        let log = lower_unity_pbr(&mut w).unwrap();
        for (e, (m, want)) in entities.iter().zip(&cases) {
            assert_eq!(usage(m), *want, "{:?}", m.material);
            assert_eq!(w.get::<MaterialUsage>(*e).copied(), *want, "{:?}", m.material);
        }
        let count = |k: &str| log.counts.get(k).copied();
        assert_eq!(count("material usage Water (FlatKit/Water)"), Some(1));
        assert_eq!(
            count("material usage Foliage (Universal Render Pipeline/Nature/SpeedTree7)"),
            Some(2)
        );
        assert_eq!(
            count("materials without a usage (Universal Render Pipeline/Nature/SpeedTree7)"),
            Some(2)
        );
        assert_eq!(count("materials without a usage (Shader Graphs/ocean)"), Some(1));
        assert_eq!(count("materials without a usage (no shader)"), Some(1));
    }
    /// A material of `shader` with subshaders `subs`, saved floats `floats` and keywords `keywords`.
    fn stated(
        shader: &str,
        subs: Vec<SubShader>,
        floats: &[(&str, f32)],
        keywords: &[&str],
    ) -> UnityMaterial {
        let mut m = material(1, Some(shader), keywords);
        m.shader_state = subs;
        m.floats = floats.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        m
    }

    /// Lower `materials` under `pipeline`: each one's alpha mode, and the log.
    fn alpha(materials: &[UnityMaterial], pipeline: Option<RenderPipeline>) -> (Vec<AlphaMode>, PassLog) {
        let mut w = World::new();
        w.insert_resource(UnityRenderSettings { sun: None, pipeline });
        let es: Vec<Entity> = materials
            .iter()
            .map(|m| {
                let e = w.spawn();
                w.insert(e, m.clone());
                e
            })
            .collect();
        let log = lower_unity_pbr(&mut w).unwrap();
        (
            es.iter()
                .map(|e| w.get::<PbrMaterial>(*e).unwrap().alpha_mode)
                .collect(),
            log,
        )
    }

    /// Built-in Standard's modes as its editor stores them (`_Mode`, keywords, `_SrcBlend`/`_DstBlend` bound by the
    /// base pass, the render queue: -1 for Opaque, 2450 for Cutout, 3000 for Fade and Transparent): Opaque One Zero
    /// stays opaque although the per-light pass adds (`[_SrcBlend] One`); Cutout clips; Fade blends; Transparent blends
    /// premultiplied (counted).
    #[test]
    fn standard_takes_its_alpha_from_the_base_pass() {
        let standard = |queue: i64, floats: &[(&str, f32)], keywords: &[&str]| {
            let mut m = stated("Standard", vec![fixture::standard()], floats, keywords);
            m.render_queue = queue;
            m
        };
        let opaque = standard(
            -1,
            &[("_Mode", 0.0), ("_SrcBlend", 1.0), ("_DstBlend", 0.0)],
            &["_NORMALMAP"],
        );
        let cutout = standard(
            2450,
            &[
                ("_Mode", 1.0),
                ("_SrcBlend", 1.0),
                ("_DstBlend", 0.0),
                ("_Cutoff", 0.3),
            ],
            &["_ALPHATEST_ON"],
        );
        let fade = standard(
            3000,
            &[("_Mode", 2.0), ("_SrcBlend", 5.0), ("_DstBlend", 10.0)],
            &["_ALPHABLEND_ON"],
        );
        let transparent = standard(
            3000,
            &[("_Mode", 3.0), ("_SrcBlend", 1.0), ("_DstBlend", 10.0)],
            &["_ALPHAPREMULTIPLY_ON"],
        );
        let (modes, log) = alpha(
            &[opaque, cutout, fade, transparent],
            Some(RenderPipeline::BuiltIn),
        );
        assert_eq!(
            modes,
            [
                AlphaMode::Opaque,
                AlphaMode::Mask { cutoff: 0.3 },
                AlphaMode::Blend,
                AlphaMode::Blend
            ]
        );
        assert_eq!(
            log.approximations
                .get("premultiplied blending (One OneMinusSrcAlpha) as alpha blending")
                .copied(),
            Some(1)
        );
        assert_eq!(
            log.counts.get("cut out by the _ALPHATEST_ON keyword").copied(),
            Some(1)
        );
    }

    /// Shaders whose base pass states its own blending: a legacy transparent one blends; a legacy cutout one clips by
    /// its render type; an unlit additive one, whose only pass has no light mode, blends (counted); an opaque base
    /// after a GrabPass stays opaque. A material's own `RenderType` tag overrides its subshader's.
    #[test]
    fn built_in_shaders_state_their_blending_in_the_base_pass() {
        use crate::material::PassKind;
        let legacy = |tags: &[(&str, &str)], src, dst| {
            fixture::sub(
                tags,
                vec![
                    fixture::own(Some("FORWARDBASE"), src, dst),
                    fixture::own(Some("FORWARDADD"), src, 1.0),
                ],
            )
        };
        let transparent = stated(
            "Legacy Shaders/Transparent/Diffuse",
            vec![legacy(
                &[("QUEUE", "Transparent"), ("RenderType", "Transparent")],
                5.0,
                10.0,
            )],
            &[],
            &[],
        );
        let cutout = stated(
            "Legacy Shaders/Transparent/Cutout/Diffuse",
            vec![legacy(
                &[("QUEUE", "AlphaTest"), ("RenderType", "TransparentCutout")],
                1.0,
                0.0,
            )],
            &[("_Cutoff", 0.4)],
            &[],
        );
        let additive = stated(
            "Legacy Shaders/Particles/Additive",
            vec![fixture::sub(
                &[("QUEUE", "Transparent")],
                vec![fixture::own(None, 5.0, 1.0)],
            )],
            &[],
            &[],
        );
        let grab = fixture::pass(
            PassKind::Grab,
            None,
            fixture::value(5.0, None),
            fixture::value(10.0, None),
        );
        let glass = stated(
            "Dirty Glass",
            vec![fixture::sub(
                &[("QUEUE", "Transparent+0")],
                vec![grab, fixture::own(Some("FORWARDBASE"), 1.0, 0.0)],
            )],
            &[],
            &[],
        );
        let mut tagged = stated(
            "Standard",
            vec![fixture::standard()],
            &[("_SrcBlend", 1.0), ("_DstBlend", 0.0)],
            &[],
        );
        tagged.tags = vec![("RenderType".into(), "TransparentCutout".into())];
        let (modes, log) = alpha(
            &[transparent, cutout, additive, glass, tagged],
            Some(RenderPipeline::BuiltIn),
        );
        assert_eq!(
            modes,
            [
                AlphaMode::Blend,
                AlphaMode::Mask { cutoff: 0.4 },
                AlphaMode::Blend,
                AlphaMode::Opaque,
                AlphaMode::Mask { cutoff: 0.5 }
            ]
        );
        assert_eq!(
            log.approximations
                .get("additive blending (destination One) as alpha blending")
                .copied(),
            Some(1)
        );
        assert_eq!(
            log.counts
                .get("cut out by render type TransparentCutout")
                .copied(),
            Some(2)
        );
    }

    /// URP takes its forward pass in its own subshader; a built-in subshader's base pass is not URP's, so a Standard
    /// material under URP has no draw pass (counted) and is opaque. SrcAlpha OneMinusSrcAlpha in the Geometry queue
    /// writing depth (Sledding's snow shader) is drawn opaque, counted; the same without depth writes blends. Without a
    /// known pipeline nothing is read.
    #[test]
    fn urp_takes_its_forward_pass_and_an_unknown_pipeline_reads_none() {
        let urp = fixture::sub(
            &[("RenderPipeline", "UniversalPipeline"), ("QUEUE", "Transparent")],
            vec![
                fixture::pass(
                    crate::material::PassKind::Normal,
                    Some("UniversalForward"),
                    fixture::value(1.0, Some("_SrcBlend")),
                    fixture::value(0.0, Some("_DstBlend")),
                ),
                fixture::own(Some("ShadowCaster"), 1.0, 0.0),
            ],
        );
        let lit = stated(
            "Universal Render Pipeline/Lit",
            vec![urp, fixture::standard()],
            &[("_Surface", 1.0), ("_SrcBlend", 5.0), ("_DstBlend", 10.0)],
            &[],
        );
        let mut fade = stated(
            "Standard",
            vec![fixture::standard()],
            &[("_SrcBlend", 5.0), ("_DstBlend", 10.0)],
            &[],
        );
        fade.render_queue = 3000;
        let snow = |zwrite: f32| {
            let mut pass = fixture::own(Some("UniversalForward"), 5.0, 10.0);
            pass.zwrite = fixture::value(zwrite, None);
            stated(
                "BruteForce/URP/SnowIceURP",
                vec![fixture::sub(
                    &[("RenderPipeline", "UniversalPipeline"), ("QUEUE", "Geometry")],
                    vec![pass],
                )],
                &[],
                &[],
            )
        };
        let (modes, log) = alpha(
            &[lit.clone(), fade.clone(), snow(1.0), snow(0.0)],
            Some(RenderPipeline::Universal),
        );
        assert_eq!(
            modes,
            [
                AlphaMode::Blend,
                AlphaMode::Opaque,
                AlphaMode::Opaque,
                AlphaMode::Blend
            ]
        );
        assert_eq!(
            log.counts
                .get("alpha: no draw pass in the subshader, blending not read")
                .copied(),
            Some(1)
        );
        assert_eq!(
            log.approximations
                .get("blending that writes depth in the opaque queue range (glTF blending neither writes depth nor draws with opaque surfaces): drawn opaque")
                .copied(),
            Some(1)
        );
        let (modes, log) = alpha(&[lit, fade], None);
        assert_eq!(modes, [AlphaMode::Opaque, AlphaMode::Opaque]);
        assert_eq!(
            log.counts
                .get("alpha: render pipeline not known, blending not read")
                .copied(),
            Some(2)
        );
    }
}
