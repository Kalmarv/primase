//! Unity materials lowered to `pbr.material` (pass `lower-unity-pbr`), by shader:
//!
//! - **Universal Render Pipeline/Lit**, as URP's Lit input reads its properties: base colour `_BaseMap` x
//!   `_BaseColor`; metallic `_Metallic` and roughness 1 - `_Smoothness`, or with `_METALLICSPECGLOSSMAP` the map's R
//!   and 1 - A x `_Smoothness` (re-encoded as a glTF metallic-roughness texture, B and G); occlusion from the map's G,
//!   lerped from white by `_OcclusionStrength` (moved to R); `_BumpMap` x `_BumpScale` with `_NORMALMAP`;
//!   `_EmissionColor` x `_EmissionMap` with `_EMISSION` (HDR colour above 1: normalised, the rest as
//!   `emissive_strength`).
//! - **Standard** and **Standard (Specular setup)** (built-in), as `UnityStandardInput.cginc` reads them: base colour
//!   `_MainTex` x `_Color`; metallic `_Metallic` and roughness 1 - `_Glossiness`, or with `_METALLICGLOSSMAP` the map's
//!   R (it replaces `_Metallic`) and 1 - A x `_GlossMapScale` (re-encoded as for URP Lit); smoothness in the base
//!   map's alpha (`_SMOOTHNESS_TEXTURE_ALBEDO_CHANNEL_A`) from `_GlossMapScale` only (counted); occlusion from the
//!   map's G whenever it has one (Standard has no keyword for it), strength `_OcclusionStrength`; `_BumpMap` x
//!   `_BumpScale` with `_NORMALMAP`; emission as URP Lit; the specular setup as a dielectric (counted). Every map is
//!   sampled through `_MainTex`'s tiling (`TexCoords`: `texcoord.xy = TRANSFORM_TEX(uv0, _MainTex)`); detail maps,
//!   on `_DetailAlbedoMap`'s, are not represented (counted).
//! - **Autodesk Interactive** (built-in): Standard's core with `RoughnessSetup` (`AutodeskInteractive.shader`:50,
//!   `UnityStandardCore.cginc`:210-226): base colour, occlusion, normal, emission and tiling as Standard; metallic
//!   `_Metallic`, or with `_METALLICGLOSSMAP` `_MetallicGlossMap`'s R; roughness `_Glossiness` (`MetallicRough`,
//!   `UnityStandardInput.cginc`:176-191, takes the smoothness as 1 - `_Glossiness`, the inspector's "Roughness"), or
//!   with `_SPECGLOSSMAP` `_SpecGlossMap`'s R; the maps re-encoded as one glTF metallic-roughness texture (B and G).
//! - **The legacy surface shaders**, as their 2019.4.20f1 built-in source reads them: base colour `_MainTex` x
//!   `_Color` (alpha `_MainTex`'s x `_Color`'s), metallic 0, each map through its own tiling (a surface shader's
//!   `uv_<map>` input is `TRANSFORM_TEX(uv0, <map>)`), the normal map `_BumpMap` unscaled (`UnpackNormal`):
//!   - **Legacy Shaders/Bumped Specular**, **Transparent/Bumped Specular** and **Transparent/Cutout/Bumped Specular**
//!     (`Normal-BumpSpec.shader`:23-30, `Alpha-BumpSpec.shader`:30-37, `AlphaTest-BumpSpec.shader`:31-38), and SideFX's
//!     **Houdini/SpecularVertexColor** (its compiled forward base fragment program is the same surface with the vertex
//!     colour in the albedo, reading `_SpecColor`, `_Color`, `_Shininess`, `_MainTex` and `_BumpMap` through its own
//!     tiling), lit by `UnityBlinnPhongLight` (`Lighting.cginc`:89-103): the highlight `_SpecColor` x pow(n.h, 128 x
//!     `_Shininess`) x the gloss, `_MainTex`'s alpha. As GGX (approximated, counted): roughness
//!     [`blinn_phong_roughness`]; `KHR_materials_specular` colour `_SpecColor` / 0.04 (its reflectance at normal
//!     incidence `_SpecColor`, as Standard's specular setup reads `_SpecColor`, `UnityStandardInput.cginc`:143) and
//!     strength texture `_MainTex` (A, the gloss). The cutout variant's `alphatest:_Cutoff` is its render type's
//!     cutout at `_Cutoff`; the transparent one's `alpha:fade` its pass's SrcAlpha OneMinusSrcAlpha.
//!   - **Legacy Shaders/Bumped Diffuse** (`Normal-Bumped.shader`:26-31) and **Legacy Shaders/Diffuse**
//!     (`Normal-Diffuse.shader`:22-26, no normal map), lit by `Lambert`: diffuse only (specular factor 0).
//!   - **Legacy Shaders/Self-Illumin/Diffuse** (`Illumin-Diffuse.shader`:27-36): `Lambert` without a normal map,
//!     emitting `_MainTex` x `_Color` x `_Illum`'s alpha: emissive `_Color`, the texture `_MainTex`, or `_Illum`'s
//!     alpha, or with both the two multiplied (re-encoded as sRGB; on different tilings, the base map's only, counted).
//! - **Unlit/Color** (`Unlit-Color.shader`:49-55): `_Color` drawn as it is: emitted over a black surface without
//!   specular.
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
//! else its subshader's: legacy cutout shaders clip in their programs). Double-sided where the draw pass culls
//! nothing (`Cull Off`, its own or bound to a property such as URP Lit's `_Cull`); without a draw pass, where the
//! material's `_Cull` is 0.
//!
//! Colour properties are gamma-encoded as stored and become linear. The base map's tiling becomes every slot's
//! [`UvTransform`] (origin moved to top-left): URP Lit's vertex pass computes one coordinate,
//! `TRANSFORM_TEX(texcoord, _BaseMap)` (`LitForwardPass.hlsl`), and its surface input samples the base, metallic,
//! normal, occlusion and emission maps with it (`InitializeStandardLitSurfaceData`, `LitInput.hlsl`), so the other
//! properties' own tiling is never read (Standard's `TexCoords` likewise). Detail maps are the one place a second
//! transform applies there (`_DetailAlbedoMap`'s on top of the base's); they are not represented, and counted. The
//! legacy surface shaders differ: each map is sampled through its own tiling. A texture's wrap mode becomes its
//! slot's wrap. Slots whose texture has no pixels (not decoded) are left out and counted. Re-encoded maps are new
//! texture entities, RGBA8, every level: metallic-roughness, occlusion and emission as above, and colour textures
//! Unity samples as linear encoded as sRGB. Two maps combined into one (Self-Illumin's, Autodesk Interactive's) are
//! sampled as the first is: a second of another size is read at its nearest texel, and one with another sampler
//! (wrap, filter) is counted.
//!
//! Each material also gets its draw order (Unity's render queue, the layer its alpha mode is) and, where its shader
//! states it, its usage (`core.material_usage`, the named table [`SHADER_USAGE`]) and whether it ignores the vertex
//! colour (`core.vertex_color`, [`IGNORES_VERTEX_COLOR`]; [`TAKES_VERTEX_COLOR`] keep the glTF convention); the rest
//! are counted by shader.

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Specular, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Name, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexRole, Texture,
    World,
};
use primase_core::{DrawLayer, DrawOrder, MaterialUsage, VertexColor};
use primase_foundations::{color::linear_to_srgb, color::srgb_to_linear};
use primase_texcodec::TexDecoder;

use crate::components::{UnityMaterial, UnityRenderSettings, UnityTexEnv, UnityTexture};
use crate::invalid;
use unity_formats::Result;
use unity_formats::player::RenderPipeline;
use unity_formats::shader::SubShader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Lit,
    Unlit,
    SpeedTree,
    /// Built-in Standard; `true` for its specular setup.
    Standard(bool),
    /// Built-in Autodesk Interactive (Standard's core with `RoughnessSetup`).
    Autodesk,
    /// A legacy surface shader.
    Legacy(Legacy),
    /// Built-in Unlit/Color.
    UnlitColor,
    Other,
}

/// The legacy surface shaders modelled, by lighting model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Legacy {
    /// Lit by `BlinnPhong`, reading `_MainTex` (gloss in A), `_Color`, `_SpecColor`, `_Shininess` and `_BumpMap`: the
    /// legacy bumped specular shaders and `Houdini/SpecularVertexColor`.
    BlinnPhong,
    /// Lit by `Lambert`, reading `_MainTex`, `_Color` and, when `true`, `_BumpMap`: Legacy Bumped Diffuse (`true`) and
    /// Legacy Diffuse.
    Lambert(bool),
    /// Legacy Self-Illumin/Diffuse: `Lambert` with emission.
    SelfIllumin,
}

fn family(shader: Option<&str>) -> Family {
    match shader {
        Some("Universal Render Pipeline/Lit") => Family::Lit,
        Some("Universal Render Pipeline/Unlit") => Family::Unlit,
        Some("Universal Render Pipeline/Nature/SpeedTree7") => Family::SpeedTree,
        Some("Standard") => Family::Standard(false),
        Some("Standard (Specular setup)") => Family::Standard(true),
        Some("Autodesk Interactive") => Family::Autodesk,
        Some(
            "Legacy Shaders/Bumped Specular"
            | "Legacy Shaders/Transparent/Bumped Specular"
            | "Legacy Shaders/Transparent/Cutout/Bumped Specular"
            | "Houdini/SpecularVertexColor",
        ) => Family::Legacy(Legacy::BlinnPhong),
        Some("Legacy Shaders/Bumped Diffuse") => Family::Legacy(Legacy::Lambert(true)),
        Some("Legacy Shaders/Diffuse") => Family::Legacy(Legacy::Lambert(false)),
        Some("Legacy Shaders/Self-Illumin/Diffuse") => Family::Legacy(Legacy::SelfIllumin),
        Some("Unlit/Color") => Family::UnlitColor,
        _ => Family::Other,
    }
}

/// Whether the lowering models a shader (those of the module documentation) rather than taking its base colour only.
pub fn modelled(shader: Option<&str>) -> bool {
    family(shader) != Family::Other
}

/// The shaders whose family states what their surfaces are (`core.material_usage`): FlatKit's water shader draws a
/// water surface; URP's SpeedTree 7 shader draws a tree's vegetation (wind sway, hue variation, alpha test), except a
/// material whose geometry keyword is bark ([`SPEEDTREE_BARK`]). A shader's own name decides nothing else: one game's
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

/// Shaders whose programs multiply the mesh's vertex colour (`COLOR_0`) into the surface, as their source shows it:
/// Unity's built-in UI, sprite and particle shaders (2019.4 built-in source: `UI-Default.shader`'s `v.color * _Color`,
/// `UnitySprites.cginc`, the particle shaders' `color : COLOR` binding, `UnityStandardParticles.cginc`'s colour modes,
/// the default Multiply among them), TextMesh Pro's (face colour times the vertex colour) and SideFX's
/// `Houdini/SpecularVertexColor` (its compiled fragment program multiplies albedo by `_Color` and `COLOR0`). These
/// keep the glTF convention, which is what an unmarked material means.
const TAKES_VERTEX_COLOR: [&str; 15] = [
    "Houdini/SpecularVertexColor",
    "UI/Default",
    "Sprites/Default",
    "Legacy Shaders/Particles/Additive",
    "Legacy Shaders/Particles/Additive (Soft)",
    "Legacy Shaders/Particles/Alpha Blended",
    "Legacy Shaders/Particles/Alpha Blended Premultiply",
    "Legacy Shaders/Particles/VertexLit Blended",
    "Mobile/Particles/Additive",
    "Mobile/Particles/Alpha Blended",
    "Particles/Standard Unlit",
    "Particles/Standard Surface",
    "TextMeshPro/Distance Field",
    "TextMeshPro/Mobile/Distance Field",
    "TextMeshPro/Sprite",
];

/// Shaders whose programs take no vertex colour (no `COLOR` input), as their source shows it: built-in Standard
/// (`UnityStandardInput.cginc`'s `VertexInput`) and Autodesk Interactive on its core, the legacy surface shaders (their
/// `Input` has no colour), Unlit/Texture and Unlit/Color (`appdata_t`: a position only), and URP's Lit and Unlit (their
/// `Attributes` have none). Their materials are [`VertexColor::Ignored`].
const IGNORES_VERTEX_COLOR: [&str; 18] = [
    "Standard",
    "Standard (Specular setup)",
    "Autodesk Interactive",
    "Unlit/Color",
    "Universal Render Pipeline/Lit",
    "Universal Render Pipeline/Unlit",
    "Legacy Shaders/Diffuse",
    "Legacy Shaders/Bumped Diffuse",
    "Legacy Shaders/Bumped Specular",
    "Legacy Shaders/Specular",
    "Legacy Shaders/Self-Illumin/Diffuse",
    "Legacy Shaders/Transparent/Diffuse",
    "Legacy Shaders/Transparent/Bumped Diffuse",
    "Legacy Shaders/Transparent/Bumped Specular",
    "Legacy Shaders/Transparent/Cutout/Diffuse",
    "Legacy Shaders/Transparent/Cutout/Bumped Diffuse",
    "Legacy Shaders/Transparent/Cutout/Bumped Specular",
    "Unlit/Texture",
];

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

/// A linear emitted colour as `emissive` and `emissive_strength`: above 1, normalised by its largest channel, the rest
/// as the strength (HDR emission, counted).
fn emission(log: &mut PassLog, c: [f32; 3]) -> ([f32; 3], f32) {
    let max = c[0].max(c[1]).max(c[2]);
    if max > 1.0 {
        log.count("HDR emission kept as emissive strength", 1);
        (
            std::array::from_fn(|k| (f64::from(c[k]) / f64::from(max)) as f32),
            max,
        )
    } else {
        (c, 1.0)
    }
}

/// The glTF roughness of Unity's legacy `_Shininess` `s`: `UnityBlinnPhongLight` (`Lighting.cginc`) raises n.h to
/// n = 128 s; the microfacet alpha with the highlight of a Blinn-Phong exponent n, alpha = sqrt(2 / (n + 2)) (Walter
/// et al. 2007), is roughness squared, so roughness = (2 / (n + 2))^(1/4).
fn blinn_phong_roughness(shininess: f32) -> f32 {
    let n = 128.0 * f64::from(shininess.max(0.0));
    (2.0 / (n + 2.0)).powf(0.25) as f32
}

/// The specular of a surface without any (`Lambert`): `KHR_materials_specular` factor 0 leaves glTF's BRDF its
/// diffuse lobe alone.
const NO_SPECULAR: Specular = Specular {
    factor: 0.0,
    texture: None,
    color: [1.0; 3],
    color_texture: None,
};

/// The value a shader reads from a texel channel `x` of a texture in `space`: decoded to linear when it is sRGB (the
/// GPU decodes it before the shader reads it), as stored otherwise.
fn seen(space: ColorSpace, x: u8) -> f64 {
    let x = f64::from(x) / 255.0;
    if space == ColorSpace::Srgb {
        srgb_to_linear(x)
    } else {
        x
    }
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
    /// Autodesk Interactive's maps as `MetallicRough` reads them, from the metallic map (`metallic`) or else the
    /// roughness map: glTF B the metallic map's R, G the roughness map's R (`roughness`: the roughness map when the
    /// source is the metallic map); a channel without its map 1, its factor carrying the property.
    RoughnessSetup {
        metallic: bool,
        roughness: Option<Entity>,
    },
    /// Self-Illumin's emission from the base map: its colour times the alpha of the illumination map (the entity),
    /// encoded as sRGB.
    Illuminated(Entity),
    /// Self-Illumin's emission from the illumination map alone (white base): its alpha as an sRGB grey.
    Illumination,
}

impl Derived {
    /// The second map the texture is made from, beside the property's own.
    fn other(self) -> Option<Entity> {
        match self {
            Derived::RoughnessSetup { roughness, .. } => roughness,
            Derived::Illuminated(e) => Some(e),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Derived::MetallicRoughness(_) | Derived::RoughnessSetup { .. } => "metallic-roughness",
            Derived::Occlusion => "occlusion",
            Derived::Srgb => "sRGB",
            Derived::Illuminated(_) | Derived::Illumination => "emission",
        }
    }

    fn color(self) -> bool {
        matches!(
            self,
            Derived::Srgb | Derived::Illuminated(_) | Derived::Illumination
        )
    }
}

/// A second map's texels, read beside a texture of possibly other size: at the same mip where its size matches, else
/// the nearest texel of its largest.
struct Beside {
    levels: Vec<(u32, u32, Vec<u8>)>,
    color_space: ColorSpace,
}

impl Beside {
    fn decode(t: &Texture) -> Result<Beside> {
        let levels = (0..t.levels.len())
            .map(|i| {
                let shift = u32::try_from(i).unwrap_or(u32::MAX);
                let w = t.width.checked_shr(shift).unwrap_or(0).max(1);
                let h = t.height.checked_shr(shift).unwrap_or(0).max(1);
                primase_texcodec::Decoder::default()
                    .decode_rgba8(t.format, w, h, &t.levels[i])
                    .map(|px| (w, h, px))
                    .map_err(|e| invalid(format!("reading a second map: {e}")))
            })
            .collect::<Result<_>>()?;
        Ok(Beside {
            levels,
            color_space: t.color_space,
        })
    }

    /// Whether every mip of a `width` x `height` texture of `count` mips has its own match.
    fn matches(&self, width: u32, height: u32, count: usize) -> bool {
        self.levels.len() >= count && self.levels.first().is_some_and(|l| (l.0, l.1) == (width, height))
    }

    fn texel(&self, t: &primase_image::Rgba8Texel) -> [u8; 4] {
        let (w, h, px) = match self.levels.get(t.level) {
            Some(l) if (l.0, l.1) == (t.width, t.height) => (l.0, l.1, &l.2),
            _ => {
                let l = &self.levels[0];
                (l.0, l.1, &l.2)
            }
        };
        let x = (u64::from(t.x) * u64::from(w) / u64::from(t.width)) as usize;
        let y = (u64::from(t.y) * u64::from(h) / u64::from(t.height)) as usize;
        let i = 4 * (y * w as usize + x);
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }
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
    use unity_formats::shader::blend_mode::{
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

/// The tiling of a material's texture property `name` (read whether or not it has a texture) as a slot's transform:
/// `None` for the identity or a non-finite one.
fn tiling(m: &UnityMaterial, name: &str) -> Option<UvTransform> {
    m.textures
        .iter()
        .find(|env| env.name == name)
        .and_then(UnityTexEnv::finite_tiling)
        .filter(|(scale, offset)| *scale != [1.0; 2] || *offset != [0.0; 2])
        .map(|(scale, offset)| UvTransform::from_bottom_left(scale, offset))
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
            wrap,
            transform: tiling,
            ..TextureRef::new(e)
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
        // the second map, decoded once; one of another size is read at its nearest texel (counted)
        let other = match kind.other() {
            Some(o) => {
                let Some(ot) = w.get::<Texture>(o) else {
                    self.log.count("slots left out: texture not decoded", 1);
                    return Ok(None);
                };
                let beside = Beside::decode(ot)?;
                if !beside.matches(t.width, t.height, t.levels.len()) {
                    self.log.approximate(
                        "maps combined at another size: the second read at its nearest texel",
                        1,
                    );
                }
                // the combined map is sampled as the first is: a second sampled otherwise (wrap, filter) differs
                // outside UV 0-1 or in its filtering
                let sampler = |e: Entity| w.get::<UnityTexture>(e).map(|u| (u.wrap, u.filter));
                if sampler(source) != sampler(o) {
                    self.log.approximate(
                        "maps combined with another sampler (wrap or filter): the second sampled as the first",
                        1,
                    );
                }
                Some(beside)
            }
            None => None,
        };
        let encode = |x: f64| byte(linear_to_srgb(x));
        let texture = primase_image::map_rgba8_levels(
            &t,
            0..t.levels.len(),
            primase_image::TextureMetadata {
                color_space: if kind.color() {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                },
                role: if kind.color() {
                    TexRole::Color
                } else {
                    TexRole::Mask
                },
                normal: None,
            },
            |texel| {
                let p = texel.rgba;
                let q = other.as_ref().map(|o| (o.texel(&texel), o.color_space));
                Ok(match kind {
                    Derived::MetallicRoughness(bits) => {
                        let smoothness = f64::from(p[3]) / 255.0 * f64::from(f32::from_bits(bits));
                        [255, byte(1.0 - smoothness), byte(seen(t.color_space, p[0])), 255]
                    }
                    Derived::Occlusion => {
                        let g = byte(seen(t.color_space, p[1]));
                        [g, g, g, 255]
                    }
                    Derived::Srgb => {
                        let e = |x: u8| encode(f64::from(x) / 255.0);
                        [e(p[0]), e(p[1]), e(p[2]), p[3]]
                    }
                    Derived::RoughnessSetup { metallic: true, .. } => {
                        let rough = q.map_or(255, |(q, space)| byte(seen(space, q[0])));
                        [255, rough, byte(seen(t.color_space, p[0])), 255]
                    }
                    Derived::RoughnessSetup { metallic: false, .. } => {
                        [255, byte(seen(t.color_space, p[0])), 255, 255]
                    }
                    Derived::Illuminated(_) => {
                        // alpha is never sRGB-decoded
                        let a = q.map_or(1.0, |(q, _)| f64::from(q[3]) / 255.0);
                        let e = |x: u8| encode(seen(t.color_space, x) * a);
                        [e(p[0]), e(p[1]), e(p[2]), 255]
                    }
                    Derived::Illumination => {
                        let g = encode(f64::from(p[3]) / 255.0);
                        [g, g, g, 255]
                    }
                })
            },
        )
        .map_err(|e| invalid(format!("re-encoding a map: {e}")))?;
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} ({})", kind.name())));
        w.insert(e, texture);
        self.log.approximate(
            match kind {
                Derived::MetallicRoughness(_) => {
                    "metallic-smoothness maps re-encoded as RGBA8 metallic-roughness"
                }
                Derived::Occlusion => "occlusion maps re-encoded as RGBA8, G moved to R",
                Derived::Srgb => "linear colour textures re-encoded as sRGB RGBA8",
                Derived::RoughnessSetup { .. } => {
                    "metallic and roughness maps (R) re-encoded as RGBA8 metallic-roughness"
                }
                Derived::Illuminated(_) | Derived::Illumination => {
                    "self-illumination re-encoded as an RGBA8 sRGB emission map"
                }
            },
            1,
        );
        self.derived.insert((source, kind), e);
        r.texture = e;
        Ok(Some(r))
    }

    fn lower(&mut self, w: &mut World, m: &UnityMaterial) -> Result<PbrMaterial> {
        // a property storing a non-finite sentinel reads as unset (its default), counted
        let non_finite = m.non_finite() as u64;
        if non_finite > 0 {
            self.log.approximate(
                "material properties storing a non-finite number, read as unset",
                non_finite,
            );
        }
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        let fam = family(m.shader.as_deref());
        let mut p = PbrMaterial {
            metallic: 0.0,
            roughness: 1.0,
            ..PbrMaterial::default()
        };
        let (tex, color) = match fam {
            Family::Lit | Family::Unlit => ("_BaseMap", Some("_BaseColor")),
            Family::SpeedTree
            | Family::Standard(_)
            | Family::Autodesk
            | Family::Legacy(_)
            | Family::UnlitColor => ("_MainTex", Some("_Color")),
            Family::Other => other_base(m),
        };
        p.base_color = linear(color.and_then(|c| m.color(c)).unwrap_or([1.0; 4]));
        let tiling = tiling(m, tex);
        // Unlit/Color samples no texture: a `_MainTex` left from another shader is not its
        if fam != Family::UnlitColor
            && let Some(env) = m.texture(tex)
        {
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
        // the draw pass's culling, a bound property read from the material (URP Lit's `Cull [_Cull]`) or the shader's own
        // (a legacy leaf shader's `Cull Off`); without a draw pass, the material's `_Cull`
        let cull = match pass {
            Some(pass) => m.state_value(&pass.cull),
            None => f("_Cull", 2.0),
        };
        match cull as i64 {
            0 => p.double_sided = true,
            1 => self
                .log
                .approximate("front faces culled: drawn as back faces culled", 1),
            _ => {}
        }
        match fam {
            Family::Lit => self.lit(w, m, tiling, &mut p)?,
            Family::Standard(specular) => self.standard(w, m, tiling, specular, &mut p)?,
            Family::Autodesk => self.autodesk(w, m, tiling, &mut p)?,
            Family::Legacy(legacy) => self.legacy(w, m, legacy, &mut p)?,
            Family::UnlitColor => self.unlit_color(m, &mut p),
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
        self.maps(w, m, tiling, m.keyword("_OCCLUSIONMAP"), p)
    }

    /// Built-in Standard's metallic and smoothness (see the module documentation), then its other maps.
    fn standard(
        &mut self,
        w: &mut World,
        m: &UnityMaterial,
        tiling: Option<UvTransform>,
        specular: bool,
        p: &mut PbrMaterial,
    ) -> Result<()> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        let scale = f("_GlossMapScale", 1.0);
        let albedo_alpha = m.keyword("_SMOOTHNESS_TEXTURE_ALBEDO_CHANNEL_A");
        if albedo_alpha {
            self.log
                .approximate("smoothness in the base map's alpha: from _GlossMapScale only", 1);
        }
        let smoothness = if albedo_alpha {
            scale
        } else {
            f("_Glossiness", 0.5)
        };
        p.roughness = 1.0 - smoothness;
        if specular {
            self.log.approximate("specular setup as a dielectric", 1);
        } else {
            p.metallic = f("_Metallic", 0.0);
            if m.keyword("_METALLICGLOSSMAP")
                && let Some(env) = m.texture("_MetallicGlossMap")
            {
                if albedo_alpha {
                    p.metallic_roughness_texture = None;
                } else if let Some(r) =
                    self.derived(w, env, tiling, Derived::MetallicRoughness(scale.to_bits()))?
                {
                    p.metallic_roughness_texture = Some(r);
                    p.metallic = 1.0;
                    p.roughness = 1.0;
                }
            }
        }
        self.maps(w, m, tiling, true, p)
    }

    /// Autodesk Interactive, Standard's core with `RoughnessSetup` (see the module documentation), then Standard's
    /// other maps.
    fn autodesk(
        &mut self,
        w: &mut World,
        m: &UnityMaterial,
        tiling: Option<UvTransform>,
        p: &mut PbrMaterial,
    ) -> Result<()> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        // `MetallicRough`: smoothness 1 - `_Glossiness` (the inspector's "Roughness"), so the roughness is the property
        p.metallic = f("_Metallic", 0.0);
        p.roughness = f("_Glossiness", 0.5);
        let map = |keyword: &str, property: &str| m.keyword(keyword).then(|| m.texture(property)).flatten();
        let metallic = map("_METALLICGLOSSMAP", "_MetallicGlossMap");
        let roughness = map("_SPECGLOSSMAP", "_SpecGlossMap");
        let derived = match (metallic, roughness) {
            (Some(env), r) => self.derived(
                w,
                env,
                tiling,
                Derived::RoughnessSetup {
                    metallic: true,
                    roughness: r.and_then(|r| r.texture),
                },
            )?,
            (None, Some(env)) => self.derived(
                w,
                env,
                tiling,
                Derived::RoughnessSetup {
                    metallic: false,
                    roughness: None,
                },
            )?,
            (None, None) => None,
        };
        if let Some(r) = derived {
            p.metallic_roughness_texture = Some(r);
            if metallic.is_some() {
                p.metallic = 1.0;
            }
            if roughness.is_some() {
                p.roughness = 1.0;
            }
        }
        self.maps(w, m, tiling, true, p)
    }

    /// The legacy surface shaders (see the module documentation): the base colour is set; the normal map through its
    /// own tiling, then each family's specular and emission.
    fn legacy(
        &mut self,
        w: &mut World,
        m: &UnityMaterial,
        legacy: Legacy,
        p: &mut PbrMaterial,
    ) -> Result<()> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        // `UnpackNormal(tex2D(_BumpMap, IN.uv_BumpMap))`: unscaled, on `_BumpMap`'s own `_ST`; a shader without one
        // does not read a `_BumpMap` left from another
        if matches!(legacy, Legacy::BlinnPhong | Legacy::Lambert(true))
            && let Some(env) = m.texture("_BumpMap")
        {
            p.normal_texture = self.slot(w, env, tiling(m, "_BumpMap"), TexRole::Normal);
        }
        match legacy {
            Legacy::BlinnPhong => {
                p.roughness = blinn_phong_roughness(f("_Shininess", 0.078125));
                let c = linear(m.color("_SpecColor").unwrap_or([0.5, 0.5, 0.5, 1.0]));
                p.specular = Some(Specular {
                    factor: 1.0,
                    // the gloss, `_MainTex`'s alpha (A is the strength `KHR_materials_specular` reads)
                    texture: p.base_color_texture,
                    color: std::array::from_fn(|k| c[k] / 0.04),
                    color_texture: None,
                });
                self.log.approximate(
                    "Blinn-Phong as GGX: roughness (2 / (128 _Shininess + 2))^(1/4), _SpecColor the reflectance at \
                     normal incidence (specular colour _SpecColor / 0.04), the gloss (_MainTex alpha) its strength",
                    1,
                );
            }
            Legacy::Lambert(_) => {
                p.specular = Some(NO_SPECULAR);
                self.log.count("Lambert as diffuse only (specular factor 0)", 1);
            }
            Legacy::SelfIllumin => {
                p.specular = Some(NO_SPECULAR);
                self.log.count("Lambert as diffuse only (specular factor 0)", 1);
                // `o.Emission = c.rgb * tex2D(_Illum, IN.uv_Illum).a`, c = `_MainTex` x `_Color` (`_Emission` scales
                // the lightmapper's meta pass only)
                let c = linear(m.color("_Color").unwrap_or([1.0; 4]));
                (p.emissive, p.emissive_strength) = emission(&mut self.log, [c[0], c[1], c[2]]);
                let base = m.texture("_MainTex");
                let illum = m.texture("_Illum");
                let (main_tiling, illum_tiling) = (tiling(m, "_MainTex"), tiling(m, "_Illum"));
                p.emissive_texture = match (base, illum) {
                    (_, None) => p.base_color_texture,
                    (None, Some(env)) => self.derived(w, env, illum_tiling, Derived::Illumination)?,
                    (Some(env), Some(illum)) if main_tiling == illum_tiling => self.derived(
                        w,
                        env,
                        main_tiling,
                        Derived::Illuminated(illum.texture.expect("a texture property with a texture")),
                    )?,
                    (Some(_), Some(_)) => {
                        self.log.approximate(
                            "self-illumination map on another tiling than the base map: emission from the base map",
                            1,
                        );
                        p.base_color_texture
                    }
                };
            }
        }
        Ok(())
    }

    /// Unlit/Color: `_Color` drawn as it is, unlit: emitted over a black surface without specular.
    fn unlit_color(&mut self, m: &UnityMaterial, p: &mut PbrMaterial) {
        let c = linear(m.color("_Color").unwrap_or([1.0; 4]));
        (p.emissive, p.emissive_strength) = emission(&mut self.log, [c[0], c[1], c[2]]);
        p.base_color = [0.0, 0.0, 0.0, c[3]];
        p.specular = Some(NO_SPECULAR);
        self.log
            .count("Unlit/Color as emission over a black surface without specular", 1);
    }

    /// The maps URP Lit and Standard share, all through the base map's `tiling`: occlusion (when `occlusion` and a
    /// texture), normal, emission; detail and height maps counted.
    fn maps(
        &mut self,
        w: &mut World,
        m: &UnityMaterial,
        tiling: Option<UvTransform>,
        occlusion: bool,
        p: &mut PbrMaterial,
    ) -> Result<()> {
        let f = |k: &str, d: f32| m.float(k).unwrap_or(d);
        if occlusion && let Some(env) = m.texture("_OcclusionMap") {
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
            (p.emissive, p.emissive_strength) = emission(&mut self.log, [c[0], c[1], c[2]]);
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
        // the depth write by the layer's convention until the draw pass's ZWrite is read (known debt)
        w.insert(e, DrawOrder::conventional(layer, queue));
        let shader = m.shader.as_deref().unwrap_or("no shader");
        match m.shader.as_deref() {
            Some(s) if IGNORES_VERTEX_COLOR.contains(&s) => {
                l.log.count("vertex colour ignored by the shader", 1);
                w.insert(e, VertexColor::Ignored);
            }
            Some(s) if TAKES_VERTEX_COLOR.contains(&s) => l.log.count("vertex colour taken by the shader", 1),
            _ => l.log.count(
                &format!("vertex colour: shader in neither table, COLOR_0 multiplies (default) ({shader})"),
                1,
            ),
        }
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
#[derive(Debug, Clone, Copy)]
pub struct LowerUnityPbr;

/// `lower-unity-pbr`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by Unity's own rules.
pub const LOWER_UNITY_PBR: PassSpec = PassSpec {
    name: "lower-unity-pbr",
    options: &[],
    make: |_| Ok(Box::new(LowerUnityPbr)),
};

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
            // the textures it derives are new entities, named after their source
            "core.name",
            "core.draw_order",
            "core.material_usage",
            "core.vertex_color",
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
        "shaders not modelled by their base colour; Blinn-Phong as GGX; maps re-encoded as RGBA8 metallic-roughness, \
         occlusion and emission; blend modes, detail and parallax maps not represented"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = lower_unity_pbr(w).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::components::Stored;
    use crate::fixture;
    use unity_formats::player::ObjRef;

    fn material(path_id: i64, shader: Option<&str>, keywords: &[&str]) -> UnityMaterial {
        UnityMaterial {
            material: Stored(ObjRef {
                file: "sharedassets1.assets".into(),
                path_id,
            }),
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
        w.insert_resource(UnityRenderSettings {
            pipeline,
            ..Default::default()
        });
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
        use unity_formats::shader::PassKind;
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
    /// writing depth (a game's snow shader) is drawn opaque, counted; the same without depth writes blends. Without a
    /// known pipeline nothing is read.
    #[test]
    fn urp_takes_its_forward_pass_and_an_unknown_pipeline_reads_none() {
        let urp = fixture::sub(
            &[("RenderPipeline", "UniversalPipeline"), ("QUEUE", "Transparent")],
            vec![
                fixture::pass(
                    unity_formats::shader::PassKind::Normal,
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

    /// A 1x1 RGBA8 texture entity (linear, as Unity marks a data map) read from the player as `format`.
    fn texel(w: &mut World, rgba: [u8; 4]) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: 1,
                height: 1,
                format: primase_core::TexFormat::Rgba8Unorm,
                levels: vec![rgba.to_vec()],
                color_space: ColorSpace::Linear,
                role: TexRole::Other,
                normal: None,
            },
        );
        w.insert(
            e,
            UnityTexture {
                texture: Stored(ObjRef {
                    file: "sharedassets1.assets".into(),
                    path_id: 100,
                }),
                format: 4,
                srgb: false,
                usage: 0,
                wrap: [0, 0],
                filter: 1,
                mip_bias: 0.0,
                aniso: 1,
                not_decoded: None,
            },
        );
        e
    }

    /// Built-in Standard as `UnityStandardInput.cginc` (2019.4.20f1) samples it: every main map, the normal map
    /// included, through `_MainTex`'s tiling whatever its own (`TexCoords`: `TRANSFORM_TEX(uv0, _MainTex)`); the
    /// metallic map's R replaces `_Metallic` and its A x `_GlossMapScale` is the smoothness (`MetallicGloss`); without
    /// the map `_Metallic` and `_Glossiness`; the occlusion map without a keyword.
    #[test]
    fn standard_samples_every_map_through_the_main_texture_tiling() {
        let mut w = World::new();
        let env = |name: &str, texture: Option<Entity>, scale: f32, offset: [f32; 2]| UnityTexEnv {
            name: name.into(),
            referenced: texture.is_some(),
            texture,
            scale: [scale; 2],
            offset,
        };
        let base = texel(&mut w, [255, 255, 255, 255]);
        let bump = texel(&mut w, [128, 128, 255, 255]);
        let gloss = texel(&mut w, [64, 0, 0, 128]);
        let occlusion = texel(&mut w, [0, 200, 0, 255]);
        let mut mapped = stated(
            "Standard",
            vec![fixture::standard()],
            &[],
            &["_NORMALMAP", "_METALLICGLOSSMAP"],
        );
        mapped.floats = vec![
            ("_SrcBlend".into(), 1.0),
            ("_DstBlend".into(), 0.0),
            ("_Metallic".into(), 0.5),
            ("_GlossMapScale".into(), 0.5),
            ("_BumpScale".into(), 0.8),
            ("_OcclusionStrength".into(), 0.7),
        ];
        // the koa table: the base map at 0.1 offset 0.9, the normal map's own tiling 1, the detail map's 3
        mapped.textures = vec![
            env("_MainTex", Some(base), 0.1, [0.0, 0.9]),
            env("_BumpMap", Some(bump), 1.0, [0.0, 0.0]),
            env("_MetallicGlossMap", Some(gloss), 1.0, [0.0, 0.0]),
            env("_OcclusionMap", Some(occlusion), 1.0, [0.0, 0.0]),
            env("_DetailAlbedoMap", None, 3.0, [0.0, 0.0]),
        ];
        let mut plain = stated(
            "Standard",
            vec![fixture::standard()],
            &[
                ("_SrcBlend", 1.0),
                ("_DstBlend", 0.0),
                ("_Metallic", 0.25),
                ("_Glossiness", 0.75),
            ],
            &[],
        );
        plain.material.path_id = 2;
        w.insert_resource(UnityRenderSettings {
            pipeline: Some(RenderPipeline::BuiltIn),
            ..Default::default()
        });
        let (a, b) = (w.spawn(), w.spawn());
        w.insert(a, mapped);
        w.insert(b, plain);
        lower_unity_pbr(&mut w).unwrap();
        let p = w.get::<PbrMaterial>(a).unwrap().clone();
        let main = Some(UvTransform::from_bottom_left([0.1; 2], [0.0, 0.9]));
        for (slot, r) in [
            ("base", p.base_color_texture),
            ("normal", p.normal_texture),
            ("metallic-roughness", p.metallic_roughness_texture),
            ("occlusion", p.occlusion_texture),
        ] {
            assert_eq!(r.map(|r| r.transform), Some(main), "{slot}");
        }
        assert_eq!(p.normal_scale, 0.8);
        assert_eq!(p.occlusion_strength, 0.7);
        assert_eq!((p.metallic, p.roughness), (1.0, 1.0));
        // R 64 replaces _Metallic 0.5 (B); A 128 x 0.5 is the smoothness, roughness 1 - 0.251 (G)
        let mr = w
            .get::<Texture>(p.metallic_roughness_texture.unwrap().texture)
            .unwrap();
        assert_eq!(mr.levels[0], vec![255, 191, 64, 255]);
        let q = w.get::<PbrMaterial>(b).unwrap();
        assert_eq!((q.metallic, q.roughness), (0.25, 0.25));
    }

    /// Double-sidedness follows the draw pass's culling: a legacy leaf shader's own `Cull Off`, URP Lit's `Cull [_Cull]`
    /// read from the material, Standard's fixed `Cull Back`; without a draw pass the material's `_Cull`.
    #[test]
    fn double_sided_follows_the_draw_pass_culling() {
        use unity_formats::shader::PassKind;
        let mut leaves_pass = fixture::own(Some("FORWARDBASE"), 1.0, 0.0);
        leaves_pass.cull = fixture::value(0.0, None);
        let leaves = stated(
            "Legacy Shaders/Transparent/Cutout/Diffuse",
            vec![fixture::sub(
                &[("RenderType", "TransparentCutout")],
                vec![leaves_pass],
            )],
            &[],
            &[],
        );
        let mut bound = fixture::pass(
            PassKind::Normal,
            Some("FORWARDBASE"),
            fixture::value(1.0, None),
            fixture::value(0.0, None),
        );
        bound.cull = fixture::value(2.0, Some("_Cull"));
        let two_sided = stated(
            "Custom/Two Sided",
            vec![fixture::sub(&[], vec![bound])],
            &[("_Cull", 0.0)],
            &[],
        );
        let standard = stated(
            "Standard",
            vec![fixture::standard()],
            &[("_SrcBlend", 1.0), ("_DstBlend", 0.0), ("_Cull", 0.0)],
            &[],
        );
        let sided = |ms: &[UnityMaterial], pipeline| {
            let mut w = World::new();
            w.insert_resource(UnityRenderSettings {
                pipeline,
                ..Default::default()
            });
            let es: Vec<Entity> = ms
                .iter()
                .map(|m| {
                    let e = w.spawn();
                    w.insert(e, m.clone());
                    e
                })
                .collect();
            lower_unity_pbr(&mut w).unwrap();
            es.iter()
                .map(|e| w.get::<PbrMaterial>(*e).unwrap().double_sided)
                .collect::<Vec<_>>()
        };
        // Standard's `_Cull` 0 is a stray float its passes never read
        assert_eq!(
            sided(
                &[leaves.clone(), two_sided.clone(), standard.clone()],
                Some(RenderPipeline::BuiltIn)
            ),
            [true, true, false]
        );
        // no pipeline known, so no draw pass: the material's `_Cull`
        assert_eq!(sided(&[leaves, two_sided, standard], None), [false, true, true]);
    }

    /// Shaders that take no vertex colour mark their materials ignored; those that multiply it and those not in either
    /// table are unmarked (the glTF convention), the unknown counted by shader.
    #[test]
    fn shaders_state_whether_they_take_the_vertex_colour() {
        let mut w = World::new();
        let names = ["Standard", "Houdini/SpecularVertexColor", "Shader Graphs/ocean"];
        let es: Vec<Entity> = names
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let e = w.spawn();
                w.insert(e, material(i as i64, Some(s), &[]));
                e
            })
            .collect();
        let log = lower_unity_pbr(&mut w).unwrap();
        let marks: Vec<Option<VertexColor>> = es.iter().map(|e| w.get::<VertexColor>(*e).copied()).collect();
        assert_eq!(marks, [Some(VertexColor::Ignored), None, None]);
        assert_eq!(
            log.counts.get("vertex colour ignored by the shader").copied(),
            Some(1)
        );
        assert_eq!(
            log.counts.get("vertex colour taken by the shader").copied(),
            Some(1)
        );
        assert_eq!(
            log.counts
                .get("vertex colour: shader in neither table, COLOR_0 multiplies (default) (Shader Graphs/ocean)")
                .copied(),
            Some(1)
        );
    }

    /// A 1x1 RGBA8 texture entity in `space`.
    fn pixel(w: &mut World, rgba: [u8; 4], space: ColorSpace) -> Entity {
        let e = texel(w, rgba);
        w.get_mut::<Texture>(e).unwrap().color_space = space;
        e
    }

    fn env(name: &str, texture: Option<Entity>, scale: f32) -> UnityTexEnv {
        UnityTexEnv {
            name: name.into(),
            referenced: texture.is_some(),
            texture,
            scale: [scale; 2],
            offset: [0.0; 2],
        }
    }

    /// Lower `materials` (built-in pipeline) in `w`: each one's `pbr.material`, and the log.
    fn lowered(w: &mut World, materials: Vec<UnityMaterial>) -> (Vec<PbrMaterial>, PassLog) {
        w.insert_resource(UnityRenderSettings {
            pipeline: Some(RenderPipeline::BuiltIn),
            ..Default::default()
        });
        let es: Vec<Entity> = materials
            .into_iter()
            .map(|m| {
                let e = w.spawn();
                w.insert(e, m);
                e
            })
            .collect();
        let log = lower_unity_pbr(w).unwrap();
        (
            es.iter()
                .map(|e| w.get::<PbrMaterial>(*e).unwrap().clone())
                .collect(),
            log,
        )
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    const BLINN_PHONG: &str = "Blinn-Phong as GGX: roughness (2 / (128 _Shininess + 2))^(1/4), _SpecColor the \
                               reflectance at normal incidence (specular colour _SpecColor / 0.04), the gloss \
                               (_MainTex alpha) its strength";

    /// Legacy Bumped Specular as `Normal-BumpSpec.shader` and `UnityBlinnPhongLight` read it: base `_MainTex` x
    /// `_Color`, the normal map through its own tiling, unscaled; roughness from `_Shininess` (n = 128 x 0.078125 = 10:
    /// (2 / 12)^(1/4)); `_SpecColor` as F0 (colour / 0.04), the gloss `_MainTex`'s alpha (the base slot, as the
    /// specular strength texture). The Houdini shader without textures: the same lobe at full gloss (no texture), its
    /// vertex colour kept.
    #[test]
    fn bumped_specular_is_blinn_phong_as_ggx() {
        let mut w = World::new();
        let main = pixel(&mut w, [200, 100, 50, 128], ColorSpace::Srgb);
        let bump = pixel(&mut w, [128, 128, 255, 255], ColorSpace::Linear);
        let mut spec = material(1, Some("Legacy Shaders/Bumped Specular"), &[]);
        spec.textures = vec![env("_MainTex", Some(main), 4.0), env("_BumpMap", Some(bump), 2.0)];
        spec.colors = vec![
            ("_Color".into(), [1.0, 0.5, 1.0, 1.0]),
            ("_SpecColor".into(), [0.5, 0.5, 0.5, 1.0]),
        ];
        spec.floats = vec![("_Shininess".into(), 0.078125)];
        let mut houdini = material(2, Some("Houdini/SpecularVertexColor"), &[]);
        houdini.colors = vec![("_SpecColor".into(), [1.0, 1.0, 1.0, 1.0])];
        houdini.floats = vec![("_Shininess".into(), 1.0)];
        let (ps, log) = lowered(&mut w, vec![spec, houdini]);
        let p = &ps[0];
        let main_tiling = Some(UvTransform::from_bottom_left([4.0; 2], [0.0; 2]));
        assert_eq!(
            p.base_color_texture.map(|r| (r.texture, r.transform)),
            Some((main, main_tiling))
        );
        assert_eq!(
            p.normal_texture.map(|r| r.transform),
            Some(Some(UvTransform::from_bottom_left([2.0; 2], [0.0; 2])))
        );
        assert_eq!(p.normal_scale, 1.0);
        assert!(close(p.base_color[1], srgb_to_linear(0.5) as f32));
        assert_eq!((p.metallic, p.alpha_mode), (0.0, AlphaMode::Opaque));
        assert!(close(p.roughness, (2.0f32 / 12.0).powf(0.25)), "{}", p.roughness);
        let s = p.specular.unwrap();
        assert_eq!(
            (s.factor, s.texture, s.color_texture),
            (1.0, p.base_color_texture, None)
        );
        assert!(close(s.color[0], srgb_to_linear(0.5) as f32 / 0.04));
        let q = &ps[1];
        assert!(close(q.roughness, (2.0f32 / 130.0).powf(0.25)));
        let s = q.specular.unwrap();
        assert_eq!((s.factor, s.texture, s.color), (1.0, None, [25.0; 3]));
        assert_eq!(log.approximations.get(BLINN_PHONG).copied(), Some(2));
        assert_eq!(
            log.counts.get("vertex colour taken by the shader").copied(),
            Some(1)
        );
    }

    /// The transparent and cutout Bumped Specular: alpha `_MainTex`'s x `_Color`'s (glTF multiplies the factor's into
    /// the texture's), blended by the transparent one's `alpha:fade` pass, cut out at `_Cutoff` by the cutout one's
    /// render type.
    #[test]
    fn transparent_bumped_specular_blends_and_the_cutout_clips() {
        let mut w = World::new();
        let main = pixel(&mut w, [255, 255, 255, 100], ColorSpace::Srgb);
        let legacy = |tags: &[(&str, &str)], src, dst| {
            fixture::sub(
                tags,
                vec![
                    fixture::own(Some("FORWARDBASE"), src, dst),
                    fixture::own(Some("FORWARDADD"), src, 1.0),
                ],
            )
        };
        let mut fade = stated(
            "Legacy Shaders/Transparent/Bumped Specular",
            vec![legacy(
                &[("QUEUE", "Transparent"), ("RenderType", "Transparent")],
                5.0,
                10.0,
            )],
            &[],
            &[],
        );
        fade.textures = vec![env("_MainTex", Some(main), 1.0)];
        fade.colors = vec![("_Color".into(), [1.0, 1.0, 1.0, 0.5])];
        let mut cutout = stated(
            "Legacy Shaders/Transparent/Cutout/Bumped Specular",
            vec![legacy(
                &[("QUEUE", "AlphaTest"), ("RenderType", "TransparentCutout")],
                1.0,
                0.0,
            )],
            &[("_Cutoff", 0.3)],
            &[],
        );
        cutout.material.path_id = 2;
        cutout.textures = fade.textures.clone();
        let (ps, log) = lowered(&mut w, vec![fade, cutout]);
        assert_eq!((ps[0].alpha_mode, ps[0].base_color[3]), (AlphaMode::Blend, 0.5));
        assert_eq!(ps[0].base_color_texture.map(|r| r.texture), Some(main));
        assert_eq!(ps[1].alpha_mode, AlphaMode::Mask { cutoff: 0.3 });
        assert_eq!(log.approximations.get(BLINN_PHONG).copied(), Some(2));
    }

    /// Legacy Bumped Diffuse, `Lambert`: diffuse only (specular factor 0), rough, its normal map through its own
    /// tiling. Legacy Diffuse the same without a normal map, a `_BumpMap` left from another shader not read.
    #[test]
    fn bumped_diffuse_is_lambert() {
        let mut w = World::new();
        let main = pixel(&mut w, [255, 255, 255, 255], ColorSpace::Srgb);
        let bump = pixel(&mut w, [128, 128, 255, 255], ColorSpace::Linear);
        let mut m = material(1, Some("Legacy Shaders/Bumped Diffuse"), &[]);
        m.textures = vec![env("_MainTex", Some(main), 1.0), env("_BumpMap", Some(bump), 8.0)];
        let mut plain = m.clone();
        plain.material.path_id = 2;
        plain.shader = Some("Legacy Shaders/Diffuse".into());
        let (ps, log) = lowered(&mut w, vec![m, plain]);
        let q = &ps[1];
        assert_eq!(
            (q.metallic, q.roughness, q.specular),
            (0.0, 1.0, Some(NO_SPECULAR))
        );
        assert_eq!(q.normal_texture, None);
        let p = &ps[0];
        assert_eq!((p.metallic, p.roughness), (0.0, 1.0));
        assert_eq!(p.specular, Some(NO_SPECULAR));
        assert_eq!(p.base_color_texture.map(|r| r.transform), Some(None));
        assert_eq!(
            p.normal_texture.map(|r| r.transform),
            Some(Some(UvTransform::from_bottom_left([8.0; 2], [0.0; 2])))
        );
        assert_eq!(
            log.counts
                .get("Lambert as diffuse only (specular factor 0)")
                .copied(),
            Some(2)
        );
    }

    /// Autodesk Interactive as `RoughnessSetup` reads it: `_Glossiness` is the roughness (1 stays fully rough) and
    /// `_Metallic` the metallic; `_MetallicGlossMap`'s R (B) and `_SpecGlossMap`'s R (G) replace them, each map only
    /// under its keyword; every map through `_MainTex`'s tiling, the normal map's own ignored.
    #[test]
    fn autodesk_interactive_reads_roughness_and_metallic_maps() {
        let mut w = World::new();
        let main = pixel(&mut w, [255, 255, 255, 255], ColorSpace::Srgb);
        let bump = pixel(&mut w, [128, 128, 255, 255], ColorSpace::Linear);
        let metal = pixel(&mut w, [64, 0, 0, 255], ColorSpace::Linear);
        let rough = pixel(&mut w, [200, 0, 0, 255], ColorSpace::Linear);
        let autodesk = |id: i64, keywords: &[&str]| {
            let mut m = stated(
                "Autodesk Interactive",
                vec![fixture::standard()],
                &[
                    ("_SrcBlend", 1.0),
                    ("_DstBlend", 0.0),
                    ("_Glossiness", 1.0),
                    ("_Metallic", 0.25),
                ],
                keywords,
            );
            m.material.path_id = id;
            m.textures = vec![
                env("_MainTex", Some(main), 20.0),
                env("_BumpMap", Some(bump), 3.0),
                env("_MetallicGlossMap", Some(metal), 1.0),
                env("_SpecGlossMap", Some(rough), 1.0),
            ];
            m
        };
        let (ps, log) = lowered(
            &mut w,
            vec![
                autodesk(1, &["_NORMALMAP"]),
                autodesk(2, &["_METALLICGLOSSMAP"]),
                autodesk(3, &["_SPECGLOSSMAP"]),
                autodesk(4, &["_METALLICGLOSSMAP", "_SPECGLOSSMAP"]),
            ],
        );
        let main_tiling = Some(UvTransform::from_bottom_left([20.0; 2], [0.0; 2]));
        assert_eq!((ps[0].metallic, ps[0].roughness), (0.25, 1.0));
        assert_eq!(ps[0].metallic_roughness_texture, None);
        assert_eq!(ps[0].normal_texture.map(|r| r.transform), Some(main_tiling));
        let mr = |p: &PbrMaterial| {
            let r = p.metallic_roughness_texture.unwrap();
            assert_eq!(r.transform, main_tiling);
            w.get::<Texture>(r.texture).unwrap().levels[0].clone()
        };
        assert_eq!((ps[1].metallic, ps[1].roughness), (1.0, 1.0));
        assert_eq!(mr(&ps[1]), [255, 255, 64, 255]);
        assert_eq!((ps[2].metallic, ps[2].roughness), (0.25, 1.0));
        assert_eq!(mr(&ps[2]), [255, 200, 255, 255]);
        assert_eq!((ps[3].metallic, ps[3].roughness), (1.0, 1.0));
        assert_eq!(mr(&ps[3]), [255, 200, 64, 255]);
        assert_eq!(
            log.approximations
                .get("metallic and roughness maps (R) re-encoded as RGBA8 metallic-roughness")
                .copied(),
            Some(3)
        );
        assert_eq!(
            log.counts.get("vertex colour ignored by the shader").copied(),
            Some(4)
        );
    }

    /// Legacy Self-Illumin/Diffuse emits `_MainTex` x `_Color` x `_Illum`'s alpha: without maps `_Color`; with the
    /// base map alone that map; with the illumination map alone its alpha as an sRGB grey; with both on one tiling
    /// their product; on different tilings the base map's (counted). Diffuse only, as `Lambert`.
    #[test]
    fn self_illumin_emits_the_base_times_the_illumination_alpha() {
        let mut w = World::new();
        let main = pixel(&mut w, [255, 128, 0, 255], ColorSpace::Srgb);
        let illum = pixel(&mut w, [0, 0, 0, 51], ColorSpace::Linear);
        let lit = |id: i64, textures: Vec<UnityTexEnv>| {
            let mut m = material(id, Some("Legacy Shaders/Self-Illumin/Diffuse"), &[]);
            m.colors = vec![("_Color".into(), [1.0, 0.5, 1.0, 1.0])];
            m.textures = textures;
            m
        };
        let (ps, log) = lowered(
            &mut w,
            vec![
                lit(1, vec![]),
                lit(2, vec![env("_MainTex", Some(main), 1.0)]),
                lit(3, vec![env("_Illum", Some(illum), 1.0)]),
                lit(
                    4,
                    vec![env("_MainTex", Some(main), 1.0), env("_Illum", Some(illum), 1.0)],
                ),
                lit(
                    5,
                    vec![env("_MainTex", Some(main), 1.0), env("_Illum", Some(illum), 2.0)],
                ),
            ],
        );
        let half = srgb_to_linear(0.5) as f32;
        for p in &ps {
            assert_eq!(p.emissive, [1.0, half, 1.0]);
            assert_eq!(p.specular, Some(NO_SPECULAR));
        }
        assert_eq!(ps[0].emissive_texture, None);
        assert_eq!(ps[1].emissive_texture, ps[1].base_color_texture);
        let pixels = |p: &PbrMaterial| {
            w.get::<Texture>(p.emissive_texture.unwrap().texture)
                .unwrap()
                .levels[0]
                .clone()
        };
        let grey = byte(linear_to_srgb(0.2));
        assert_eq!(pixels(&ps[2]), [grey, grey, grey, 255]);
        let times = |x: u8| byte(linear_to_srgb(srgb_to_linear(f64::from(x) / 255.0) * 0.2));
        assert_eq!(pixels(&ps[3]), [times(255), times(128), 0, 255]);
        assert_eq!(ps[4].emissive_texture, ps[4].base_color_texture);
        assert_eq!(
            log.approximations
                .get("self-illumination map on another tiling than the base map: emission from the base map")
                .copied(),
            Some(1)
        );
    }

    /// Two maps combined into one texture are sampled as the first: a second with another wrap (here clamped beside a
    /// repeating base) is reported, for Self-Illumin's emission and Autodesk Interactive's metallic-roughness alike;
    /// maps sampled alike are not.
    #[test]
    fn maps_combined_with_another_sampler_are_reported() {
        let mut w = World::new();
        let main = pixel(&mut w, [255, 255, 255, 255], ColorSpace::Srgb);
        let clamped = pixel(&mut w, [128, 0, 0, 128], ColorSpace::Linear);
        w.get_mut::<UnityTexture>(clamped).unwrap().wrap = [1, 1];
        let repeating = pixel(&mut w, [64, 0, 0, 255], ColorSpace::Linear);
        let mut lit = material(1, Some("Legacy Shaders/Self-Illumin/Diffuse"), &[]);
        lit.textures = vec![
            env("_MainTex", Some(main), 1.0),
            env("_Illum", Some(clamped), 1.0),
        ];
        let autodesk = |id: i64, roughness: Entity| {
            let mut m = stated(
                "Autodesk Interactive",
                vec![fixture::standard()],
                &[("_SrcBlend", 1.0), ("_DstBlend", 0.0)],
                &["_METALLICGLOSSMAP", "_SPECGLOSSMAP"],
            );
            m.material.path_id = id;
            m.textures = vec![
                env("_MetallicGlossMap", Some(repeating), 1.0),
                env("_SpecGlossMap", Some(roughness), 1.0),
            ];
            m
        };
        let (_, log) = lowered(&mut w, vec![lit, autodesk(2, clamped), autodesk(3, main)]);
        assert_eq!(
            log.approximations
                .get("maps combined with another sampler (wrap or filter): the second sampled as the first")
                .copied(),
            Some(2)
        );
    }

    /// Unlit/Color draws `_Color` unlit: emitted over a black surface without specular; a `_MainTex` left from another
    /// shader is not sampled; no vertex colour.
    #[test]
    fn unlit_color_is_emission() {
        let mut w = World::new();
        let stale = pixel(&mut w, [255, 255, 255, 255], ColorSpace::Srgb);
        let mut m = material(1, Some("Unlit/Color"), &[]);
        m.colors = vec![("_Color".into(), [0.5, 1.0, 0.0, 1.0])];
        m.textures = vec![env("_MainTex", Some(stale), 1.0)];
        let (ps, log) = lowered(&mut w, vec![m]);
        let p = &ps[0];
        assert_eq!(p.emissive, [srgb_to_linear(0.5) as f32, 1.0, 0.0]);
        assert_eq!((p.base_color, p.base_color_texture), ([0.0, 0.0, 0.0, 1.0], None));
        assert_eq!(p.specular, Some(NO_SPECULAR));
        assert_eq!(
            log.counts
                .get("Unlit/Color as emission over a black surface without specular")
                .copied(),
            Some(1)
        );
        assert_eq!(
            log.counts.get("vertex colour ignored by the shader").copied(),
            Some(1)
        );
    }
}
