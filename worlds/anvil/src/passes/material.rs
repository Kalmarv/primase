//! `lower-anvil-material{normal-green=up|down,alpha=opaque|mask[,alpha-cutoff=X]|prelude,unset=white,shader-uv=texcoord|one-minus-v,emissive-exposure=X,underground-view=X,fake-river-s=S}`:
//! Anvil Materials ([`AnvilMaterial`]) become metal-rough PBR, for the templates whose pixel shaders the Riders
//! Republic notes evaluated (docs/material-spec.md §4: the game's own shaders run on WARP against these formulas,
//! within 5.5e-7):
//!
//! - **MAD_Fake** (and MAD_CHR_Basic, MAD_Fake_Sediment): base colour the texture at t75; the normal map t76's red
//!   and green, z rebuilt; roughness t76's alpha, metalness its blue (a metallic-roughness texture made of them);
//!   two-sided. Sediment's two switches mixing in the engine's terrain textures (§14.6) are left out, reported.
//! - **MAD_Bark, MAD_Tex1_Leaves** (layout B, §14.4-5: no metalness target, so metalness 0; their textures
//!   ([`BARK`], [`LEAVES`]) share one UV transform): Bark's base colour its diffuse, roughness its normal map's alpha;
//!   Leaves' its diffuse tinted in HSV everywhere (its per-instance mix of tinted and plain is the tint where its value
//!   is 0, else drawn tinted, reported), its alpha the TextureSet's slot 18 map's red (the opacity), roughness its
//!   mask's green. Occlusion `1 + OnOffAO (mask.b - 1)`; the shaders' further factor COLOR0.a (and Leaves'
//!   `sat(COLOR0.a + Vertex AO)` on the base colour, 1 where Vertex AO is at least 1) isn't a COLOR_0 product: left
//!   out, reported, as are the effects without a PBR place (translucency, specular occlusion, near fade, dead-tree
//!   thinning, wind debug). MAD_Tex1_Rocks isn't lowered: it combines maps sampled at three UV sets per pixel.
//! - **MAD_Tex2** (§4.3: [`TEX2`]): a height blend of two layers ([`LayeredMaterial`] with
//!   [`Composition::Height`]). Each layer an ordinary material at its own UV transform (`Base tiling A`, `B`): its
//!   diffuse tinted in HSV by its mask's red, roughness `S(sat((a + Roughness Offset - 0.1) 1.25))` and metalness
//!   `b` of its normal map, occlusion `1 + OnOffAO (mask.b - 1)`; B weighted by
//!   `w = sat((COLOR0.a - 0.5) Alpha Contrast + 0.5 + Alpha Contrast Alpha Offset)`, A by `1 - w`, each by its
//!   diffuse's alpha as its height, at the Height Contrast. Its colour factor `1 + Vertex Color Intensity
//!   (COLOR0.rgb - 1)` isn't lowered, reported.
//! - **MAD_Clutters_Grass** (layout B, §15.2: [`GRASS`]): base colour its diffuse, alpha its `1 - Alpha` map's red
//!   (TextureSet slot 18), roughness its normal map's alpha, metalness 0, occlusion `1 + OnOffAO (mask.b - 1)`,
//!   two-sided. Its noise tint (weighted by a noise tiled in world space), translucency, specular occlusion and
//!   per-instance dither fade are left out, reported.
//! - **MAD_Tex1_SkinningSupported** is MAD_Tex1 exactly (§27.14: its rows Tex1's), **MAD_Decals1_Graffiti** MAD_Decals1
//!   where its `EmissivePower` (row 6) is 0, as on both placed (its team colour and Fresnel emission off; otherwise the
//!   placeholder, reported), and **MAD_Tex2_Graffiti_Mode** MAD_Tex2 ([`TEX2_GRAFFITI_ROWS`]) with each layer's
//!   roughness its normal map's alpha raw, no vertex-colour factor, its own rows, where its `State Value` is 0 (the
//!   graffiti palette off, as placed; otherwise the placeholder, reported).
//! - **MAD_Tex1_Screen** (§27.14.4): base colour `0.6 sat(t75)^2.2` (its shader's decode), roughness 0.1, metalness 0,
//!   emission `0.666667` over `emissive-exposure`, coloured by the albedo; t75 the generic `Texture`, the TextureSet's
//!   slot 0.
//! - **MAD_Screens** (§27.14.1): where `Use Videos` (row 5) is 0 and `Use Logos` (row 4) 0 or the `Neutral` colour
//!   (row 7) black, as on the 56 LED and 15 team screens placed, its colour is 0 whatever the frame: black, no
//!   emission, roughness 1. A video screen (it reads a second UV set the meshes' lowering doesn't decode) is the
//!   placeholder, reported.
//! - **MAD_SHD_UNLIT_UndergroundBox** (§27.14.7): its albedo is a gradient by the view, `lerp(ColorB, ColorA, x)`,
//!   `x = 0.5 + 0.5 d.z / sqrt(|d|^2 + 1)`: `underground-view` states `x`; roughness 1, opaque (its G-buffer key has no
//!   blend, whatever its prelude says).
//! - **MAD_Tex1_FakeRiver** (§27.14.9): its albedo `base + t76.a (1.5 - s) (target - base)`,
//!   `base = (1 - s) (0.07451, 0.10196, 0.06275) + (0.04706, 0.05882, 0.07059)`, `target = (0.12549, 0.16078,
//!   0.23922)`, `s` its program's COLOR0.x (the vertex colour's red, which the albedo isn't a product of):
//!   `fake-river-s` states it for the whole Material; the normal map t75 at 0.1 of
//!   its strength (z rebuilt from the unscaled map, as its shader does); roughness and metalness its `3 - Metalness`.
//! - **SHD_Nat_Veg_Grass_AO** (§27.14.8; layout B): base colour its diffuse t75 times `1 + AO Mult (COLOR0.a - 1)`,
//!   that factor written into `COLOR_0` (`(f, f, f, 1)`) of every primitive drawn with it alone, as MAD_FakeForest's
//!   (an `AO Mult` outside 0 to 1 is the placeholder, reported); its normal map the generic `Texture` t76, the
//!   TextureSet's slot 1; roughness 1, metalness 0; two-sided where its flags say. Its specular occlusion (0.2) has no
//!   PBR place, reported.
//! - **MAD_Tex1, MAD_Tex1_Wind, MAD_Decals1**: base colour t76, tinted by `Hue`, `Saturation` and `Luminosity` in
//!   HSV where its mask (t77, the Material's TextureSet slot 7) has red, everywhere on Decals1 (no mask); roughness
//!   and metalness of the normal map t75's alpha and blue, `S(sat(a + Roughness Offset))` and `b * Metalness` on Tex1
//!   and Decals1 (`S(x) = x x (3 - 2 x)`), raw on Wind; occlusion `1 + OnOffAO (mask.b - 1)` (none on Decals1). Each
//!   template's constants are read where its evaluated pixel shaders read them ([`TEX1`], [`TEX1_WIND`] §13.1,
//!   [`DECALS1`] §13.2, [`TEX1_TERRAIN_BLEND`] §15.1: Tex1 with its UV at 15 and metalness 0 (layout B), its blend
//!   toward the terrain's colour near the ground left out, reported), and only if its template binds those
//!   properties there; otherwise the placeholder, reported.
//! - **MAD_Tex1_Graffiti_Mode** (§17.2, [`TEX1_GRAFFITI`]): as MAD_Tex1 (its mask and normal map from the TextureSet's
//!   slots 7 and 1, metalness and roughness raw, no vertex colour) where its logo is 0 and its team and custom
//!   colours' alphas 0, as on all four of its Materials; otherwise the placeholder, reported (runtime state).
//! - **MAD_Tex1_TeamColoured** (§27.9): MAD_Tex1 with the Neutral hue, saturation and luminosity as its tint (the
//!   only ones its placed permutation reads; the team looks are runtime state); its hue wraps where Tex1's doesn't, so
//!   a negative Neutral hue (none placed) is the placeholder, reported.
//! - **MAD_Tex2_TintBFromShaderConstants** (§27.5): MAD_Tex2 with layer B tinted by the Neutral constants.
//! - **MAD_Grounds** (§27.7, corrected): its masks are real (t79 the TextureSet's slot 7, t80 the template's
//!   parquet default), but with the neutral vertex the meshes' lowering states (no tire wear: its program's scalar
//!   `c` 1; the vertex colour white: reported) and `Use Blend` 0, a `Tire Albedo` at most 1 and a `Tire Vertex Color
//!   Impact` at least 0, its arithmetic is layer A alone whatever the masks: the tire factors 1, the vertex-colour term
//!   the identity, a Tex1 without mask or occlusion, its diffuse tinted by `Hue`, `Saturate` and `Luminosity`
//!   everywhere, roughness and metalness the normal map's alpha and blue raw, at `Base tiling`. Otherwise (the Diorama:
//!   `Use Blend` 1 blends the layers by the parquet mask at another UV) the placeholder, reported.
//! - **MAD_Tex1_Graffiti_Mode_DecalFX** (§27.10, its alpha-tested permutation `0x404000` read): an atlas logo cut out
//!   with a flat colour. Where its `TeamBlendRatio` is at least 1 its screen-space stripes are the team colour
//!   everywhere, so with an opaque team colour (its alpha 1) the base colour is the team colour to the 2.2 (its
//!   shader's decode), the base colour texture white with the atlas's alpha, sampled at the logo's cell (`Color -
//!   Logo` 1: the cell `DecalStateID` of a `DecalDimXY` atlas, as its shader computes it; 0: `Base tiling`), cut out at
//!   its `AlphaTestValue` over 255 (the engine sets its shader's reference so where `AlphaTestEnabled` is on), whatever
//!   `alpha` states; roughness 1, no metalness, the mesh's normal. Its emission is `EmissiveMult g^2` over
//!   `emissive-exposure` (its shader's `colour.a EmissiveMult (0.5 (2 g - 1) + 0.5)^2 / t23`, the team colour's alpha
//!   1), `g` the green of its emissive mask t76 at the mask's own tiling (`cb3[11]`; the template's default
//!   GFX_Jammer_NormalMap where the Material sets none), coloured by the albedo. Otherwise the placeholder, reported.
//! - **MAD_FakeForest** (§26.6, executed by the notes: the far forest's canopy cells and tree proxies): no texture,
//!   no property; its albedo is `pow(saturate(COLOR0.rgb), 2.2)` per vertex (the engine's 2.2, not the sRGB curve),
//!   so the pass writes that into `COLOR_0` (the colour core multiplies) of every primitive drawn with it and with
//!   nothing else (one also drawn with another Material keeps its stored colour, reported; one without `COLOR_0` is
//!   drawn white, its vertex layout's default not read, reported), base colour 1, roughness 1, metalness 0,
//!   single-sided and opaque: its alpha test is the LOD fade's dither only (§26.6), whatever `alpha` states. Its
//!   translucency (0.5, written to the G-buffer) has no PBR place, reported.
//! - **MAD_Tex1_Emissive, MAD_Tex1_UGE** (§14.3, [`TEX1_EMISSIVE`], [`TEX1_UGE`]): Tex1's shape without a mask:
//!   the diffuse t76 tinted everywhere, its alpha times `Alpha`; roughness the normal map t75's alpha raw, metalness
//!   `b * Metalness`; no occlusion. Emission the emissive mask t77's red (UGE: its alpha) times `Emissive Scale`
//!   over the stated `emissive-exposure` (the shader divides by the engine's exposure texture t23, whose value isn't
//!   known: §17.1), coloured by the albedo (the ambient pass emits albedo times that intensity: §23 (4)): a texture of
//!   the tinted diffuse times that lane, and a factor (above 1 carried in the emissive strength). UGE's `Overriden
//!   Emissive Scale` (read only by the instanced permutations) is reported.
//!   Tex1's vertex-colour factor `1 + Vertex Color Intensity (COLOR0.rgb - 1)` is the mesh's COLOR_0 product where the
//!   intensity is 1 (a consumer multiplies COLOR0's alpha in too, which the shader doesn't: reported), none where 0,
//!   and otherwise not lowered, reported. Every other Material is marked [`VertexColor::Ignored`] (its shader takes no
//!   COLOR0 as a colour; Tex2's blend weight is its own). Wind's `6 - Translucency` (written to the G-buffer as is) has
//!   no PBR place, left out, reported.
//!
//! A texture's colour space follows its TextureMap's u32 at 25 (1: sampled as sRGB, the notes' rule from the game's
//! texture-creation records; not a role's); values are made linear as the shader samples them before any arithmetic,
//! and a made texture is linear except a tinted base colour, sRGB.
//!
//! **UV transform** (Tex1 and Wind: the one transform `Base tiling`, at `cb3[8]` and `cb3[19]`, samples all three
//! textures; Decals1 has none, sampling `TEXCOORD0` as is). Of
//! the property's 8 f32 `su, sv, tu, tv`, speeds u and v, `t`, rotation speed, the shader dots `(u, v, 1)` with the
//! rows `(su cos t, -su sin t, tu)` and `(sv sin t, sv cos t, tv)` (docs/material-spec.md §12: eleven resident
//! constant-buffer matrices, each equal to one stored combination). That is `uv' = S R(t) uv + (tu, tv)` on the
//! shader's own coordinates, `R` core's rotation sense. Lowered only where the notes saw that form: no rotation (any
//! scales and translation), or a rotation with `su = sv` and no translation, every speed 0. Otherwise the textures are
//! sampled untransformed, each case reported: rotated with unequal scales or with a translation (`S R` against `R S`,
//! and how a translation joins a rotation, are not shown), a moving UV (the time term is a lead), a binding the
//! Material leaves unset (its default isn't known), or not 8 floats.
//!
//! `shader-uv` states what the shader's `(u, v)` is in terms of the mesh's `TexCoord(0)` (the mesh's UV decode is
//! itself a stated choice, `lower-anvil-mesh`'s). `texcoord`: the same, giving offset `(tu, tv)`, scale `(su, sv)`,
//! rotation `t`. `one-minus-v`: `(u, 1 - v)` (the shader's v read as stored where the mesh was lowered with
//! `uv-v=one-minus`); conjugating by that flip `D` gives rotation `-t` and offset `(tu - su sin t, 1 - sv cos t - tv)`
//! (`D R(t) D = R(-t)`, `D` commutes with `S`, and where lowered `S` commutes with `R`).
//!
//! `alpha=prelude` draws each Material as its prelude (the 102 bytes after its references) reads in the notes
//! (docs/material-spec.md §13.3; AnvilToolkit's BlendMode and alpha-test fields at those places, not checked in the
//! game): its u32 at 20 4, blended with the base colour's alpha; 0, alpha-tested at its byte at 28 over 255 where
//! that byte and its byte at 53 are other than 0 (the byte at 53 the likely enable, a correlation: the addendum's 68
//! cut-out and 40 solid Tex1 and Wind Materials), else opaque; 1 and 6 (additive-looking) aren't lowered, drawn
//! opaque, reported.
//!
//! Every option is the program's: `underground-view` and `fake-river-s` state what the static export takes for a view
//! and a vertex value the shaders read; the normal map's green sign against glTF's isn't established, nor the alpha test's
//! reference (alpha=mask states one; alpha=prelude the prelude's reading), nor what the game binds for a texture a Material leaves unset (unset=white: the
//! notes find the masks' settings make sense only so), nor the shader's coordinates against the mesh's (shader-uv).
//! Other templates are drawn with a neutral placeholder, reported by name. The Materials are consumed.

use std::collections::BTreeMap;

use primase_core::layered::{Channel, Composition, Layer, LayerWeights, Source, WeightMapping};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Component, Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Name, NormalBasis,
    NormalConvention, NormalGreen, NormalX, NormalZ, Semantic, Stream, TexFormat, TexRole, Texture,
    VertexColor, World,
};
use primase_image::{Rgba8Texel, TextureMetadata, map_rgba8_levels, srgb_to_linear};

use super::formulas::{Rough, emission, metalness, occlusion, roughness, sat, tinted};
use crate::components::{AnvilMaterial, AnvilTextureMap, MaterialBinding, MaterialFlags, MaterialValue};

/// How the alpha channel is drawn: the program's statement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaChoice {
    Opaque,
    Mask(f32),
    /// Each Material's as its prelude states it ([`prelude_alpha`]).
    Prelude,
}

/// What the shader's texture coordinates are in terms of the mesh's `TexCoord(0)`: the program's statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderUv {
    /// The same.
    TexCoord,
    /// `(u, 1 - v)`.
    OneMinusV,
}

/// The pass (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerAnvilMaterial {
    pub normal_green: NormalGreen,
    pub alpha: AlphaChoice,
    pub shader_uv: ShaderUv,
    /// The exposure the emissive templates' emission is divided by (the engine's t23, not known: a stated value).
    pub emissive_exposure: f64,
    /// MAD_SHD_UNLIT_UndergroundBox's gradient position `x` (its shader's, by the view: a stated value).
    pub underground_view: f64,
    /// MAD_Tex1_FakeRiver's `s`, its COLOR0.x (its albedo quadratic in it, not a COLOR_0 product: a stated value).
    pub fake_river_s: f64,
}

impl LowerAnvilMaterial {
    /// From its options: `normal-green`, `alpha` (and `alpha-cutoff` with `mask`), `unset=white`, `shader-uv`,
    /// `emissive-exposure`, all required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilMaterial, String> {
        let known = [
            "normal-green",
            "alpha",
            "alpha-cutoff",
            "unset",
            "shader-uv",
            "emissive-exposure",
            "underground-view",
            "fake-river-s",
        ];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let usage = "normal-green=up|down, alpha=opaque|mask (with alpha-cutoff=X)|prelude, unset=white, \
                     shader-uv=texcoord|one-minus-v, emissive-exposure=X, underground-view=X (0 to 1), fake-river-s=S";
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
            "prelude" => AlphaChoice::Prelude,
            v => return Err(format!("alpha={v} is not opaque, mask or prelude")),
        };
        match get("unset")? {
            "white" => {}
            v => return Err(format!("unset={v}: only white is read")),
        }
        let shader_uv = match get("shader-uv")? {
            "texcoord" => ShaderUv::TexCoord,
            "one-minus-v" => ShaderUv::OneMinusV,
            v => return Err(format!("shader-uv={v} is not texcoord or one-minus-v")),
        };
        let emissive_exposure: f64 = get("emissive-exposure")?
            .parse()
            .map_err(|_| "emissive-exposure is a number".to_owned())?;
        if !(emissive_exposure.is_finite() && emissive_exposure > 0.0) {
            return Err(format!(
                "emissive-exposure {emissive_exposure}: finite and above 0"
            ));
        }
        let number = |k: &str| -> Result<f64, String> {
            let v: f64 = get(k)?.parse().map_err(|_| format!("{k} is a number"))?;
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("{k} {v}: finite"))
            }
        };
        let underground_view = number("underground-view")?;
        if !(0.0..=1.0).contains(&underground_view) {
            return Err(format!("underground-view {underground_view}: within 0 and 1"));
        }
        let fake_river_s = number("fake-river-s")?;
        Ok(LowerAnvilMaterial {
            normal_green,
            alpha,
            shader_uv,
            emissive_exposure,
            underground_view,
            fake_river_s,
        })
    }
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

/// A Tex1-family template's metalness.
#[derive(Clone, Copy, Debug)]
pub enum Metalness {
    /// The normal map's blue as is.
    Blue,
    /// The normal map's blue times the constant at this vec4.
    Scaled(u32),
    /// None written: its G-buffer layout has no metalness target (docs/material-spec.md §15.4: F0 0.04), so 0.
    Absent,
}

/// Where a Tex1-family template's evaluated pixel shader reads its constants (each a `cb3` vec4), and which
/// properties the template must bind there: `reads`, by vec4 and key. A constant it has none of is `None`: then
/// roughness is the normal map's alpha raw, and there is no tint mask, AO, vertex-colour factor, UV transform or
/// translucency.
#[derive(Clone, Copy, Debug)]
pub struct Tex1Layout {
    pub reads: &'static [(u32, u32)],
    pub metalness: Metalness,
    /// Hue, saturation, luminosity.
    pub tint: [u32; 3],
    pub roughness_offset: Option<u32>,
    pub ao: Option<u32>,
    pub vertex_colour: Option<u32>,
    pub uv: Option<u32>,
    pub translucency: Option<u32>,
    /// Its switch blending toward the terrain's colour near the ground (none: no such blend).
    pub terrain_blend: Option<u32>,
    /// Its normal map's and diffuse's textures.
    pub normal: TextureSource,
    pub diffuse: TextureSource,
    /// Its mask (tint by red, occlusion by blue), if it samples one.
    pub mask: Option<TextureSource>,
    /// Its emission (none: it emits nothing).
    pub emission: Option<Emission>,
    /// Its `Alpha`, multiplying the diffuse's alpha (none: the alpha as sampled).
    pub alpha: Option<u32>,
}

/// An emissive template's emission: its emissive mask's lane `lane` times the scalar at vec4 `scale`, over the
/// exposure.
#[derive(Clone, Copy, Debug)]
pub struct Emission {
    pub mask: TextureSource,
    pub scale: u32,
    pub lane: usize,
}

/// Where a template's texture comes from.
#[derive(Clone, Copy, Debug)]
pub enum TextureSource {
    /// The Material's texture its template binds at shader slot `t(75 + n)`.
    Bound(u32),
    /// The Material's TextureSet's slot (a generic `Texture` binding, docs/material-spec.md §4.2, §17.2).
    Set(usize),
}

/// MAD_Tex1 (docs/material-spec.md §4.2): `3 - Metalness` 1, `Hue` 2, `Saturation` 3, `Luminosity` 4,
/// `4 - Roughness Offset` 5, `5 - On Off - AO` 6, `6 - Vertex Color Intensity` 7, the UV transform `Base tiling` 8.
pub const TEX1: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0xC7C3_A6D7),
        (2, 0xEE58_1D0E),
        (3, 0x6577_AE58),
        (4, 0xFD62_B19F),
        (5, 0x89D1_84DE),
        (6, 0x6338_B6C4),
        (7, 0x95DD_8185),
        (8, 0x7091_389B),
    ],
    metalness: Metalness::Scaled(1),
    tint: [2, 3, 4],
    roughness_offset: Some(5),
    ao: Some(6),
    vertex_colour: Some(7),
    uv: Some(8),
    translucency: None,
    terrain_blend: None,
    normal: TextureSource::Bound(0),
    diffuse: TextureSource::Bound(1),
    mask: Some(TextureSource::Set(7)),
    emission: None,
    alpha: None,
};

/// MAD_Tex1_TeamColoured (§27.9; its binding table read): `3 - Metalness` 1, `Roughness Offset` 2, `On Off - AO` 3,
/// `Vertex Color Intensity` 4, the Neutral hue, saturation and luminosity 5 to 7 (Teammate 8 to 10 and Opponent 11 to
/// 13 not read by its placed key), `Base tiling` 14; its textures MAD_Tex1's.
pub const TEX1_TEAM_COLOURED: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0xC7C3_A6D7),
        (2, 0x89D1_84DE),
        (3, 0x6338_B6C4),
        (4, 0x95DD_8185),
        (5, 0x1FEF_B952),
        (6, 0x9FB2_DBA5),
        (7, 0x07A7_C462),
        (14, 0x7091_389B),
    ],
    tint: [5, 6, 7],
    roughness_offset: Some(2),
    ao: Some(3),
    vertex_colour: Some(4),
    uv: Some(14),
    ..TEX1
};

/// MAD_Grounds' layer A (§27.7; its keys the CRC-32 of its labels, its rows read): `Hue`, `Saturate`, `Luminosity` 1
/// to 3, `Tire Albedo` 8, `Tire Vertex Color Impact` 10, `9 - Use Blend` 11, `5 - Base tiling` 12; `3 - Normal Map A`
/// t75, `1 - Diffuse Alpha A` t76. Metalness and roughness the normal map's blue and alpha raw; no mask, occlusion or
/// vertex colour (see the module documentation).
pub const GROUNDS_A: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0xEE58_1D0E),
        (2, 0xCC63_AC50),
        (3, 0xFD62_B19F),
        (8, 0xFEEB_CEB6),
        (10, 0xD1BC_3B7E),
        (11, 0xFDBA_E6B2),
        (12, 0x3DA2_6308),
    ],
    metalness: Metalness::Blue,
    tint: [1, 2, 3],
    roughness_offset: None,
    ao: None,
    vertex_colour: None,
    uv: Some(12),
    translucency: None,
    terrain_blend: None,
    normal: TextureSource::Bound(0),
    diffuse: TextureSource::Bound(1),
    mask: None,
    emission: None,
    alpha: None,
};

/// MAD_Tex1_TerrainBlend (§14.7, §15.1): MAD_Tex1's cb3[2..7], `EnableTerrainBlend` 14, `Base tiling` 15. Its
/// G-buffer permutations write shading class 3 (layout B, §15.4) on every path and read no `3 - Metalness` (cb3[1]):
/// metalness 0.
pub const TEX1_TERRAIN_BLEND: Tex1Layout = Tex1Layout {
    reads: &[
        (2, 0xEE58_1D0E),
        (3, 0x6577_AE58),
        (4, 0xFD62_B19F),
        (5, 0x89D1_84DE),
        (6, 0x6338_B6C4),
        (7, 0x95DD_8185),
        (14, 0x6D30_6588),
        (15, 0x7091_389B),
    ],
    metalness: Metalness::Absent,
    uv: Some(15),
    terrain_blend: Some(14),
    ..TEX1
};

/// MAD_Tex1_Wind (§13.1: its pixel shaders read cb3[14..20] only, cb3[1..13] the vertex shader's wind): Hue 14,
/// Saturation 15, Luminosity 16, `5 - On Off - AO` 17, `6 - Translucency` 18, `Base tiling` 19 (rows 19 and 20).
pub const TEX1_WIND: Tex1Layout = Tex1Layout {
    reads: &[
        (14, 0xEE58_1D0E),
        (15, 0x6577_AE58),
        (16, 0xFD62_B19F),
        (17, 0x6338_B6C4),
        (18, 0xFEAC_9469),
        (19, 0x7091_389B),
    ],
    metalness: Metalness::Blue,
    tint: [14, 15, 16],
    roughness_offset: None,
    ao: Some(17),
    vertex_colour: None,
    uv: Some(19),
    translucency: Some(18),
    terrain_blend: None,
    normal: TextureSource::Bound(0),
    diffuse: TextureSource::Bound(1),
    mask: Some(TextureSource::Set(7)),
    emission: None,
    alpha: None,
};

/// MAD_Decals1 (§13.2: its CB3 has 6 vec4s; textures sampled at TEXCOORD0 directly): `3 - Metalness` 1, Hue 2,
/// Saturation 3, Luminosity 4, `4 - Roughness Offset` 5; no mask.
pub const DECALS1: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0xC7C3_A6D7),
        (2, 0xEE58_1D0E),
        (3, 0x6577_AE58),
        (4, 0xFD62_B19F),
        (5, 0x89D1_84DE),
    ],
    metalness: Metalness::Scaled(1),
    tint: [2, 3, 4],
    roughness_offset: Some(5),
    ao: None,
    vertex_colour: None,
    uv: None,
    translucency: None,
    terrain_blend: None,
    normal: TextureSource::Bound(0),
    diffuse: TextureSource::Bound(1),
    mask: None,
    emission: None,
    alpha: None,
};

/// MAD_Tex1_Emissive (§14.3: textures t75 normal map, t76 albedo and opacity, t77 emissive mask, each bound by the
/// Material): `2.Metalness` 1, `4.Emissive Scale` 2, Hue 3, Saturate 4, Luminosity 5, Alpha 6, Vertex Color 7,
/// `3.Base tiling` 8; no tint mask, no AO; emission the mask's red.
pub const TEX1_EMISSIVE: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0x2275_0B1E),
        (2, 0x40DC_2F97),
        (3, 0xEE58_1D0E),
        (4, 0xCC63_AC50),
        (5, 0xFD62_B19F),
        (6, 0x1121_166E),
        (7, 0xF776_61D0),
        (8, 0x7091_389B),
    ],
    metalness: Metalness::Scaled(1),
    tint: [3, 4, 5],
    roughness_offset: None,
    ao: None,
    vertex_colour: Some(7),
    uv: Some(8),
    translucency: None,
    terrain_blend: None,
    normal: TextureSource::Bound(0),
    diffuse: TextureSource::Bound(1),
    mask: None,
    emission: Some(Emission {
        mask: TextureSource::Bound(2),
        scale: 2,
        lane: 0,
    }),
    alpha: Some(6),
};

/// MAD_Tex1_UGE (§14.3): MAD_Tex1_Emissive's, with `5.Overriden Emissive Scale` 8 (read only by the instanced
/// permutations) and `3.Base tiling` 9; emission the mask's alpha.
pub const TEX1_UGE: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0x2275_0B1E),
        (2, 0x40DC_2F97),
        (3, 0xEE58_1D0E),
        (4, 0xCC63_AC50),
        (5, 0xFD62_B19F),
        (6, 0x1121_166E),
        (7, 0xF776_61D0),
        (8, 0xFD06_799F),
        (9, 0x7091_389B),
    ],
    uv: Some(9),
    emission: Some(Emission {
        mask: TextureSource::Bound(2),
        scale: 2,
        lane: 3,
    }),
    ..TEX1_EMISSIVE
};

/// MAD_Tex1_Graffiti_Mode (§17.2: t75 mask, the TextureSet's slot 7; t76 normal map, its slot 1; t77 `1 - Diffuse
/// Alpha`, bound): `3 - On Off - AO` 1, State Value 2, `Color - Logo` 3, Hue 4, Saturation 5, Luminosity 6, `Color -
/// Team` 7 and `Color - Custom` 8 (vec4s), TeamBlendRatio 9, ShutterTiling 10, `Base tiling` 11. Metalness and
/// roughness the normal map's blue and alpha raw. Tex1's formulas where its team and custom colours' alphas are 0
/// and its logo 0 (all four Materials): the team colour, its screen-space stripes and the logo atlas are runtime
/// state.
pub const TEX1_GRAFFITI: Tex1Layout = Tex1Layout {
    reads: &[
        (1, 0x41C2_6236),
        (2, 0xD2CD_C3FF),
        (3, 0x89C6_17B6),
        (4, 0xEE58_1D0E),
        (5, 0x6577_AE58),
        (6, 0xFD62_B19F),
        (7, 0xA9A8_2BBA),
        (8, 0xB5FD_CCE2),
        (9, 0xDF0B_BE34),
        (10, 0xB66D_7487),
        (11, 0x5FD1_46F2),
    ],
    metalness: Metalness::Blue,
    tint: [4, 5, 6],
    roughness_offset: None,
    ao: Some(1),
    vertex_colour: None,
    uv: Some(11),
    translucency: None,
    terrain_blend: None,
    normal: TextureSource::Set(1),
    diffuse: TextureSource::Bound(2),
    mask: Some(TextureSource::Set(7)),
    emission: None,
    alpha: None,
};

/// The textures (by shader slot `t(75 + slot)` and key) and constants (by vec4 and key) a template's evaluated pixel
/// shaders read, as the template must bind them.
pub struct Reads {
    pub textures: &'static [(u32, u32)],
    pub constants: &'static [(u32, u32)],
}

/// MAD_Bark's (§14.5): normal t76, diffuse t77 (its label with a trailing space); `5 - On Off - AO` 16, `Wind Debug`
/// 17, `4 - Base tiling` 18.
pub const BARK: Reads = Reads {
    textures: &[(1, 0x443B_F980), (2, 0x918F_B02B)],
    constants: &[(16, 0x6338_B6C4), (17, 0x9EF0_824F), (18, 0x8459_B8E0)],
};

/// MAD_Tex1_Leaves's (§14.4): normal t76, diffuse t77, `3 - Mask 1` t78; Specular Occlusion 21, Saturate 22, Hue 23,
/// Near Fade 24, Vertex AO 25, Color Tint Debug 26, On Off AO 27, Luminosity 28, Dead Trees Value 29 and On Off 30,
/// Translucency 31, Wind Debug 32, `Base tiling` 33. Its opacity is the TextureSet's slot 18 map.
pub const LEAVES: Reads = Reads {
    textures: &[(1, 0x443B_F980), (2, 0xE4B6_CC48), (3, 0xA1B7_D591)],
    constants: &[
        (21, 0x10CB_A0A5),
        (22, 0xCC63_AC50),
        (23, 0xEE58_1D0E),
        (24, 0x0E8B_9837),
        (25, 0xB554_0E5B),
        (26, 0x856A_89B3),
        (27, 0xDAC3_6D2C),
        (28, 0xFD62_B19F),
        (29, 0xD478_D1B5),
        (30, 0x47AE_4887),
        (31, 0x8A6E_48BC),
        (32, 0x02FD_4532),
        (33, 0x7091_389B),
    ],
};

/// MAD_Clutters_Grass's (§15.2; its cb3[1..9] the vertex shader's wind): Translucency 10, Specular Occlusion 11,
/// Metalness 12 (read by no pixel shader), the noise tint's Value 13, Saturation 14, On-Off 15 and Hue 17, On Off AO
/// 16. Its textures t75..t79: mask (slot 7), alpha (slot 18), diffuse (slot 0), normal (slot 1), the world-space tint
/// noise.
pub const GRASS: Reads = Reads {
    textures: &[],
    constants: &[
        (10, 0x50C4_05F8),
        (11, 0x9181_5960),
        (12, 0x6726_790A),
        (13, 0xDFF0_3A61),
        (14, 0x338D_C0B2),
        (15, 0x13DC_5DE5),
        (16, 0x9F36_C4C8),
        (17, 0x2D46_EEA4),
    ],
};

/// MAD_Tex2's (§4.3; the layers A = (t78 diffuse, t75 normal, t80 mask) and B = (t76, t77, t79)): normal A t75,
/// diffuse B t76, normal B t77, diffuse A t78, mask B t79, mask A t80; Vertex Color Intensity 1, Height Contrast 2,
/// Alpha Offset 3, Alpha Contrast 4, Hue / Saturation / Luminosity A 5-7 and B 8-10, Roughness Offset 1 and 2 at 11
/// and 12, On Off AO 13, `Base tiling A` 14, `Base tiling B` 17.
pub const TEX2: Reads = Reads {
    textures: &[
        (0, 0x840C_4002),
        (1, 0xB72C_97CC),
        (2, 0xE0FC_E9CD),
        (3, 0xE4B6_CC48),
        (4, 0x8E92_3E53),
        (5, 0xFCAC_D4EA),
    ],
    constants: &[
        (1, 0x422F_B075),
        (2, 0xA72E_5763),
        (3, 0xA7B7_68D7),
        (4, 0xD4D4_90B6),
        (5, 0x2803_E854),
        (6, 0x968A_7B9F),
        (7, 0x9F39_AFF6),
        (8, 0xB10A_B9EE),
        (9, 0x0F83_2A25),
        (10, 0x0630_FE4C),
        (11, 0xDFF7_205B),
        (12, 0x46FE_71E1),
        (13, 0xCBBE_0755),
        (14, 0x6CB3_7F10),
        (17, 0xF5BA_2EAA),
    ],
};

/// Where a Tex2 template's shader reads its rows (§4.3, §27.5, §27.14.6): each layer's tint, the roughness offsets
/// (none: each normal map's alpha raw), the occlusion switch, the two UV transforms, the vertex-colour intensity (none:
/// no colour factor), the height contrast, alpha offset and alpha contrast, and its graffiti state (none: no palette).
pub struct Tex2Rows {
    pub reads: Reads,
    pub tint_a: [u32; 3],
    pub tint_b: [u32; 3],
    pub roughness_offsets: Option<[u32; 2]>,
    pub ao: u32,
    pub uv: [u32; 2],
    pub vertex_colour: Option<u32>,
    pub height_contrast: u32,
    pub alpha_offset: u32,
    pub alpha_contrast: u32,
    pub state: Option<u32>,
}

/// MAD_Tex2's rows.
pub const TEX2_ROWS: Tex2Rows = Tex2Rows {
    reads: TEX2,
    tint_a: [5, 6, 7],
    tint_b: [8, 9, 10],
    roughness_offsets: Some([11, 12]),
    ao: 13,
    uv: [14, 17],
    vertex_colour: Some(1),
    height_contrast: 2,
    alpha_offset: 3,
    alpha_contrast: 4,
    state: None,
};

/// MAD_Tex2_Graffiti_Mode's rows (§27.14.6; its binding table read, its keys the CRC-32 of its labels): `Alpha
/// Contrast` 1, `State Value` 2, Hue / Saturation / Luminosity A 3-5, `Height Contrast` 6, `Alpha Offset` 7, Hue /
/// Saturation / Luminosity B 8-10, `On Off - AO` 11, `Base tiling A` 12 and `B` 15; its textures Tex2's slots (two of
/// them under other labels). No roughness offsets, no vertex-colour factor.
pub const TEX2_GRAFFITI_ROWS: Tex2Rows = Tex2Rows {
    reads: Reads {
        textures: &[
            (0, 0x840C_4002),
            (1, 0x7A5F_B139),
            (2, 0xE0FC_E9CD),
            (3, 0x94C8_3273),
            (4, 0x8E92_3E53),
            (5, 0xFCAC_D4EA),
        ],
        constants: &[
            (1, 0xD4D4_90B6),
            (2, 0xD2CD_C3FF),
            (3, 0x2803_E854),
            (4, 0x968A_7B9F),
            (5, 0x9F39_AFF6),
            (6, 0xA72E_5763),
            (7, 0xA7B7_68D7),
            (8, 0xB10A_B9EE),
            (9, 0x0F83_2A25),
            (10, 0x0630_FE4C),
            (11, 0xCBBE_0755),
            (12, 0x6CB3_7F10),
            (15, 0xF5BA_2EAA),
        ],
    },
    tint_a: [3, 4, 5],
    tint_b: [8, 9, 10],
    roughness_offsets: None,
    ao: 11,
    uv: [12, 15],
    vertex_colour: None,
    height_contrast: 6,
    alpha_offset: 7,
    alpha_contrast: 1,
    state: Some(2),
};

/// MAD_Tex2_TintBFromShaderConstants' rows (its binding table read): MAD_Tex2's 1 to 7, roughness offsets 8 and 9,
/// `On Off - AO` 10, layer B tinted by the Neutral hue, saturation and luminosity 11 to 13 (Teammate 14 to 16 and
/// Opponent 17 to 19 not read by its placed key), `Base tiling A` 20 and `B` 23; its textures MAD_Tex2's.
pub const TEX2_TINT_B_ROWS: Tex2Rows = Tex2Rows {
    reads: Reads {
        textures: TEX2.textures,
        constants: &[
            (1, 0x422F_B075),
            (2, 0xA72E_5763),
            (3, 0xA7B7_68D7),
            (4, 0xD4D4_90B6),
            (5, 0x2803_E854),
            (6, 0x968A_7B9F),
            (7, 0x9F39_AFF6),
            (8, 0xDFF7_205B),
            (9, 0x46FE_71E1),
            (10, 0xCBBE_0755),
            (11, 0x1FEF_B952),
            (12, 0x9FB2_DBA5),
            (13, 0x07A7_C462),
            (20, 0x6CB3_7F10),
            (23, 0xF5BA_2EAA),
        ],
    },
    tint_a: [5, 6, 7],
    tint_b: [11, 12, 13],
    roughness_offsets: Some([8, 9]),
    ao: 10,
    uv: [20, 23],
    vertex_colour: Some(1),
    height_contrast: 2,
    alpha_offset: 3,
    alpha_contrast: 4,
    state: None,
};

/// MAD_Decals1_Graffiti's `EmissivePower` (§27.14.5): row 6, its key the CRC-32 of its label.
const DECALS1_GRAFFITI_EMISSIVE_POWER: (u32, u32) = (6, 0x52AE_793B);

/// MAD_Screens' (§27.14.1; its binding table read): `Use Logos` 4, `Use Videos` 5, `Neutral` 7.
pub const SCREENS: Reads = Reads {
    textures: &[],
    constants: &[(4, 0x2EBA_2876), (5, 0xEB11_E364), (7, 0x6B99_DC6C)],
};

/// MAD_SHD_UNLIT_UndergroundBox's (§27.14.7): `ColorA` 1, `ColorB` 2.
pub const UNDERGROUND_BOX: Reads = Reads {
    textures: &[],
    constants: &[(1, 0x0DC5_9729), (2, 0x94CC_C693)],
};

/// SHD_Nat_Veg_Grass_AO's (§27.14.8): `0: Layer1 Diffuse + Alpha Map` t75, the generic `Texture` t76 (its normal
/// map, the TextureSet's slot 1); `AO Mult` 1.
pub const GRASS_AO: Reads = Reads {
    textures: &[(0, 0x74B9_1491), (1, 0x4DDB_34EE)],
    constants: &[(1, 0x5E05_FBCF)],
};

/// MAD_Tex1_FakeRiver's (§27.14.9): `2 - Normal Map` t75, `1 - Diffuse Alpha` t76; `3 - Metalness` 1, `3.Base
/// tiling` 2.
pub const FAKE_RIVER: Reads = Reads {
    textures: &[(0, 0x443B_F980), (1, 0xE4B6_CC48)],
    constants: &[(1, 0xC7C3_A6D7), (2, 0x7091_389B)],
};

/// MAD_Tex1_Graffiti_Mode_DecalFX (§27.10; its binding table read): `1 - Diffuse Alpha` t75, the generic `Texture`
/// t76 (the emissive mask); `DecalStateID` 1, `Color - Logo` 2, `Color - Custom` 3, `Color - Team` 4, `DecalDimXY` 5,
/// `TeamBlendRatio` 6, `EmissiveMult` 7, `Base tiling` 8, the emissive mask's tiling 11.
pub const DECAL_FX: Reads = Reads {
    textures: &[(0, 0xE4B6_CC48), (1, 0x4DDB_34EE)],
    constants: &[
        (1, 0x4328_423F),
        (2, 0x89C6_17B6),
        (3, 0xB5FD_CCE2),
        (4, 0xA9A8_2BBA),
        (5, 0xA53B_0410),
        (6, 0xDF0B_BE34),
        (7, 0xB9B5_3F6B),
        (8, 0x5FD1_46F2),
        (11, 0x49CE_4B2E),
    ],
};

/// The transform of a `DecalDimXY` `(dx, dy)` atlas's cell `state` (MAD_Tex1_Graffiti_Mode_DecalFX's shader:
/// `u / dx + state / dx`, `v / dy + (dy - (floor(state / dx + 0.01) + 1)) / dy`) as a UV property's 8 floats.
pub fn atlas_cell(state: f64, dx: f64, dy: f64) -> [f32; 8] {
    let row = (state / dx + 0.01).floor() + 1.0;
    [
        (1.0 / dx) as f32,
        (1.0 / dy) as f32,
        (state / dx) as f32,
        ((dy - row) / dy) as f32,
        0.0,
        0.0,
        0.0,
        0.0,
    ]
}

/// Whether `m`'s template binds every texture and constant `reads` names where it names it.
fn binds_all(m: &AnvilMaterial, reads: &Reads) -> bool {
    reads
        .textures
        .iter()
        .all(|&(slot, key)| m.bindings.contains(&(key, MaterialBinding::Texture { slot })))
        && reads.constants.iter().all(|&(at, key)| {
            m.bindings.iter().any(|(k, b)| {
                *k == key
                    && matches!(b, MaterialBinding::Scalar { vec4 } | MaterialBinding::Vector { vec4 }
                        | MaterialBinding::UvTransform { vec4 } if *vec4 == at)
            })
        })
}

/// Whether `m`'s template binds every property `layout` reads at its vec4.
fn binds(m: &AnvilMaterial, layout: &Tex1Layout) -> bool {
    layout.reads.iter().all(|&(at, key)| {
        m.bindings.iter().any(|(k, b)| {
            *k == key
                && matches!(b, MaterialBinding::Scalar { vec4 } | MaterialBinding::Vector { vec4 }
                    | MaterialBinding::UvTransform { vec4 } if *vec4 == at)
        })
    })
}

/// The blend mode a Material's flags state ([`MaterialFlags`]: `BlendMode`, `AlphaTestValue`, `AlphaTestEnabled`):
/// the notes' reading (docs/material-spec.md §13.3 and its addendum), not checked in the game. 4: blended; 0:
/// alpha-tested at `AlphaTestValue` over 255 where it and `AlphaTestEnabled` are other than 0 (the engine sets the
/// shader's alpha reference so where `AlphaTestEnabled` is on: docs/shaders.md §6), else opaque; otherwise (1, 6:
/// additive-looking) not lowered, `None`.
fn prelude_alpha(m: &AnvilMaterial) -> Option<AlphaMode> {
    let f = &m.flags;
    let enabled = f
        .booleans
        .get(MaterialFlags::ALPHA_TEST_ENABLED)
        .copied()
        .unwrap_or(0);
    match (f.blend_mode, f.alpha_test_value, enabled) {
        (4, _, _) => Some(AlphaMode::Blend),
        (0, 0, _) | (0, _, 0) => Some(AlphaMode::Opaque),
        (0, t, _) => Some(AlphaMode::Mask {
            cutoff: f32::from(t) / 255.0,
        }),
        _ => None,
    }
}

/// What lowering one Material reads of it.
struct Inputs<'m> {
    m: &'m AnvilMaterial,
}

impl Inputs<'_> {
    /// The texture its template binds at shader slot `t(75 + slot)`, resolved as the engine resolves it (the Material's
    /// record, else its TextureSet's slot or the template's default: [`crate::components::BoundTexture`]).
    fn texture(&self, slot: u32) -> Option<Entity> {
        self.m
            .textures
            .iter()
            .find(|t| t.shader_slot == slot)
            .and_then(|t| t.texture)
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

    /// The vec4 its template binds at the constant buffer's vec4 `vec4`: `None` where it binds none there or the
    /// Material sets none.
    fn vector(&self, vec4: u32) -> Option<[f64; 4]> {
        let &(key, _) = self
            .m
            .bindings
            .iter()
            .find(|(_, b)| *b == MaterialBinding::Vector { vec4 })?;
        self.m.values.iter().find_map(|(k, v)| match v {
            MaterialValue::Float4(x) if *k == key => Some(x.map(f64::from)),
            _ => None,
        })
    }

    /// The texture `source` names.
    fn source(&self, source: TextureSource) -> Option<Entity> {
        match source {
            TextureSource::Bound(n) => self.texture(n),
            // the binding whose TextureParameter names the set's slot `k` at channel 0, resolved; else the slot itself
            TextureSource::Set(k) => match self
                .m
                .textures
                .iter()
                .find(|t| t.parameter == u32::try_from(k).ok().map(|k| (k, 0)))
            {
                Some(t) => t.texture,
                None => self.m.texture_set.get(k).copied().flatten(),
            },
        }
    }

    /// The floats of the UV transform its template binds at the constant buffer's vec4 `vec4`: `None` where the
    /// template binds none there, `Some(None)` where the Material sets none.
    fn uv(&self, vec4: u32) -> Option<Option<&[f32]>> {
        let &(key, _) = self
            .m
            .bindings
            .iter()
            .find(|(_, b)| *b == MaterialBinding::UvTransform { vec4 })?;
        Some(self.m.values.iter().find_map(|(k, v)| match v {
            MaterialValue::Floats(f) if *k == key => Some(f.as_slice()),
            _ => None,
        }))
    }
}

/// A UV property's 8 floats as a transform of the mesh's `TexCoord(0)` (see the module documentation): `None` for
/// the identity, or why it isn't lowered.
pub(super) fn uv_transform(f: &[f32], shader_uv: ShaderUv) -> Result<Option<UvTransform>, &'static str> {
    let &[su, sv, tu, tv, speed_u, speed_v, t, speed_t] = f else {
        return Err("not 8 floats");
    };
    if (speed_u, speed_v, speed_t) != (0.0, 0.0, 0.0) {
        return Err("moving (the time term isn't established)");
    }
    if t != 0.0 && su != sv {
        return Err("rotated with unequal scales (S R against R S not shown)");
    }
    if t != 0.0 && (tu, tv) != (0.0, 0.0) {
        return Err("rotated and translated (not shown)");
    }
    if (su, sv, tu, tv, t) == (1.0, 1.0, 0.0, 0.0, 0.0) {
        return Ok(None);
    }
    Ok(Some(match shader_uv {
        ShaderUv::TexCoord => UvTransform {
            offset: [tu, tv],
            scale: [su, sv],
            rotation: t,
        },
        ShaderUv::OneMinusV => {
            let (sin, cos) = if t == 0.0 { (0.0, 1.0) } else { t.sin_cos() };
            UvTransform {
                offset: [tu - su * sin, 1.0 - sv * cos - tv],
                scale: [su, sv],
                rotation: if t == 0.0 { 0.0 } else { -t },
            }
        }
    }))
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
            (
                "shader-uv".to_owned(),
                match self.shader_uv {
                    ShaderUv::TexCoord => "texcoord",
                    ShaderUv::OneMinusV => "one-minus-v",
                }
                .to_owned(),
            ),
            ("emissive-exposure".to_owned(), self.emissive_exposure.to_string()),
            ("underground-view".to_owned(), self.underground_view.to_string()),
            ("fake-river-s".to_owned(), self.fake_river_s.to_string()),
        ]);
        match self.alpha {
            AlphaChoice::Opaque => {
                o.insert("alpha".to_owned(), "opaque".to_owned());
            }
            AlphaChoice::Mask(c) => {
                o.insert("alpha".to_owned(), "mask".to_owned());
                o.insert("alpha-cutoff".to_owned(), c.to_string());
            }
            AlphaChoice::Prelude => {
                o.insert("alpha".to_owned(), "prelude".to_owned());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            PbrMaterial::NAME,
            LayeredMaterial::NAME,
            Texture::NAME,
            VertexColor::NAME,
            Name::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME, Mesh::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "Riders Republic materials as metal-rough PBR: no vertex-colour factor or translucency, UV transforms only of the forms seen, other templates a placeholder"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<Entity> = w.iter::<AnvilMaterial>().map(|(e, _)| e).collect();
        let mut white: Option<Entity> = None;
        let normal = NormalConvention {
            green: self.normal_green,
            z: NormalZ::Rebuild,
            x: NormalX::Red,
            basis: NormalBasis::TangentBitangent,
        };
        // made textures, per (source, what, parameters' bits)
        let mut made: BTreeMap<MadeKey, Entity> = BTreeMap::new();
        // the Materials whose shaders take COLOR0 otherwise than as a colour factor, and how the pass rewrites it
        let mut colour_rules: BTreeMap<Entity, ColourRule> = BTreeMap::new();
        for e in materials {
            let Some(m) = w.remove::<AnvilMaterial>(e) else {
                continue;
            };
            let name = m.template_name.clone().unwrap_or_default();
            let i = Inputs { m: &m };
            let alpha_mode = match self.alpha {
                AlphaChoice::Opaque => AlphaMode::Opaque,
                AlphaChoice::Mask(cutoff) => AlphaMode::Mask { cutoff },
                AlphaChoice::Prelude => prelude_alpha(&m).unwrap_or_else(|| {
                    log.omit(
                        "Materials' prelude blend modes other than 0 and 4 (additive-looking, not lowered): drawn \
                         opaque",
                        1,
                    );
                    AlphaMode::Opaque
                }),
            };
            let layout = match name.as_str() {
                "MAD_Tex1" | "MAD_Tex1_SkinningSupported" => Some(TEX1),
                "MAD_Tex1_TerrainBlend" => Some(TEX1_TERRAIN_BLEND),
                "MAD_Tex1_Wind" => Some(TEX1_WIND),
                "MAD_Decals1" | "MAD_Decals1_Graffiti" => Some(DECALS1),
                "MAD_Tex1_Emissive" => Some(TEX1_EMISSIVE),
                "MAD_Tex1_UGE" => Some(TEX1_UGE),
                "MAD_Tex1_Graffiti_Mode" => Some(TEX1_GRAFFITI),
                "MAD_Tex1_TeamColoured" => Some(TEX1_TEAM_COLOURED),
                "MAD_Grounds" => Some(GROUNDS_A),
                _ => None,
            };
            // whether the Material's shader takes COLOR0 as a factor of the base colour, as COLOR_0 is
            let mut takes_colour = false;
            // whether it became a layered material (no PBR material of its own then)
            let mut layered = false;
            let tex2 = match name.as_str() {
                "MAD_Tex2" => Some(&TEX2_ROWS),
                "MAD_Tex2_TintBFromShaderConstants" => Some(&TEX2_TINT_B_ROWS),
                "MAD_Tex2_Graffiti_Mode" => Some(&TEX2_GRAFFITI_ROWS),
                _ => None,
            };
            if name == "MAD_Grounds" && grounds_reduce(&i) {
                log.approximate(
                    "Grounds Materials' tire wear and vertex colour (per-vertex values the meshes' lowering doesn't \
                     carry): none and white (stated)",
                    1,
                );
            }
            let pbr = match (name.as_str(), layout) {
                ("MAD_Tex1_TeamColoured", _) if i.scalar(5) < 0.0 => {
                    log.omit(
                        "TeamColoured Materials with a negative Neutral hue (it wraps where Tex1's tint doesn't): a \
                         placeholder",
                        1,
                    );
                    placeholder()
                }
                ("MAD_Grounds", _) if !grounds_reduce(&i) => {
                    log.omit(
                        "Grounds Materials whose arithmetic isn't one layer (a Use Blend other than 0, a Tire Albedo \
                         above 1 or a negative Tire Vertex Color Impact): a placeholder",
                        1,
                    );
                    placeholder()
                }
                ("MAD_Tex1_Graffiti_Mode_DecalFX", _) => {
                    match decal_fx(w, &mut made, &i, &m, self.shader_uv, self.emissive_exposure, log)? {
                        Some(pbr) => pbr,
                        None => placeholder(),
                    }
                }
                ("MAD_FakeForest", _) => {
                    takes_colour = true;
                    colour_rules.insert(e, ColourRule::Pow22);
                    log.omit("FakeForest Materials' translucency (0.5, no PBR place)", 1);
                    PbrMaterial {
                        base_color: [1.0; 4],
                        metallic: 0.0,
                        roughness: 1.0,
                        alpha_mode: AlphaMode::Opaque,
                        double_sided: false,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_Fake" | "MAD_CHR_Basic" | "MAD_Fake_Sediment", _) => {
                    if name == "MAD_Fake_Sediment" && [1, 2].iter().any(|&v| i.scalar(v) != 0.0) {
                        log.omit(
                            "Fake_Sediment Materials' terrain albedo and sediment mixing (the engine's terrain \
                             textures): left out",
                            1,
                        );
                    }
                    let albedo = i.texture(0);
                    let normals = i.texture(1);
                    if let Some(a) = albedo {
                        set_colour(w, a, TexRole::Color);
                    }
                    let mr = match normals {
                        Some(n) => Some(metal_rough(w, &mut made, n, None, Rough::Raw)?),
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
                ("MAD_Bark" | "MAD_Tex1_Leaves", _)
                    if !binds_all(&m, if name == "MAD_Bark" { &BARK } else { &LEAVES }) =>
                {
                    log.omit(
                        &format!(
                            "Materials of template {name:?} (it doesn't bind the properties its evaluated shader \
                             reads: a neutral placeholder drawn)"
                        ),
                        1,
                    );
                    placeholder()
                }
                ("MAD_Bark" | "MAD_Tex1_Leaves", _) => {
                    // layout B (docs/material-spec.md §14 (a), §15.4): no metalness, AO its own target
                    let leaves = name == "MAD_Tex1_Leaves";
                    let normals = i.texture(1);
                    let diffuse = i.texture(2);
                    let mask = match if leaves { i.texture(3) } else { None }
                        .or_else(|| m.texture_set.get(7).copied().flatten())
                    {
                        Some(t) => t,
                        None => {
                            log.count(&format!("{name} masks unset: white (stated)"), 1);
                            *white.get_or_insert_with(|| white_texture(w))
                        }
                    };
                    let scalar = |v: u32| i.scalar(v);
                    let (ao_on, uv_at) = if leaves {
                        (scalar(27), 33)
                    } else {
                        (scalar(16), 18)
                    };
                    let uv = lowered_uv(&i, Some(uv_at), self.shader_uv, &name, log);
                    let sampled_by = |texture: Entity| TextureRef {
                        transform: uv,
                        ..TextureRef::new(texture)
                    };
                    log.omit(
                        &format!(
                            "{name} Materials' AO factor COLOR0.a (the vertex alpha; not a COLOR_0 product)"
                        ),
                        1,
                    );
                    if scalar(if leaves { 32 } else { 17 }) != 0.0 {
                        log.omit(&format!("{name} Materials' wind debug colour: left out"), 1);
                    }
                    let base = match diffuse {
                        Some(d) if leaves => {
                            // opacity is the TextureSet's slot 18 map's red; the tint is unmasked, and where the
                            // per-instance mix's value (26) is 0 it applies everywhere
                            let opacity = m.texture_set.get(18).copied().flatten();
                            if opacity.is_none() {
                                log.omit(
                                    "Leaves opacity maps unset (TextureSet slot 18): the diffuse alpha",
                                    1,
                                );
                            }
                            if scalar(26) != 0.0 {
                                log.omit(
                                    "Leaves Materials' per-instance tint mix (its hash of the instance position): \
                                     drawn tinted throughout",
                                    1,
                                );
                            }
                            if scalar(25) < 1.0 {
                                log.omit(
                                    "Leaves Materials' vertex AO sat(COLOR0.a + v) with v < 1 (not a COLOR_0 product)",
                                    1,
                                );
                            }
                            for (v, what) in [
                                (30, "dead-tree thinning"),
                                (24, "near-camera fade"),
                                (31, "translucency"),
                                (21, "specular occlusion"),
                            ] {
                                if scalar(v) != 0.0 {
                                    log.omit(
                                        &format!("Leaves Materials' {what} (no PBR place): left out"),
                                        1,
                                    );
                                }
                            }
                            Some(base_colour(
                                w,
                                &mut made,
                                d,
                                [scalar(23), scalar(22), scalar(28)],
                                None,
                                opacity,
                            )?)
                        }
                        Some(d) => Some(base_colour(w, &mut made, d, [0.0; 3], None, None)?),
                        None => None,
                    };
                    // roughness: Leaves the mask's green, Bark the normal map's alpha
                    let mr = if leaves {
                        Some(made_texture(w, &mut made, (mask, "leaves rough", [0; 5]), |w| {
                            derived(w, mask, TexRole::Mask, ColorSpace::Linear, |_, c| {
                                [255, byte(c[1]), 0, 255]
                            })
                        })?)
                    } else {
                        match normals {
                            Some(n) => Some(metal_rough(w, &mut made, n, Some(0.0), Rough::Raw)?),
                            None => None,
                        }
                    };
                    let occlusion = if ao_on != 0.0 {
                        Some(occlusion_of(w, &mut made, mask, ao_on)?)
                    } else {
                        None
                    };
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    log.count(&format!("{name} Materials lowered"), 1);
                    PbrMaterial {
                        base_color_texture: base.map(sampled_by),
                        metallic: 1.0,
                        roughness: 1.0,
                        metallic_roughness_texture: mr.map(sampled_by),
                        occlusion_texture: occlusion.map(sampled_by),
                        normal_texture: normals.map(sampled_by),
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_Decals1_Graffiti", _)
                    if !(binds_all(
                        &m,
                        &Reads {
                            textures: &[],
                            constants: &[DECALS1_GRAFFITI_EMISSIVE_POWER],
                        },
                    ) && i.scalar(DECALS1_GRAFFITI_EMISSIVE_POWER.0) == 0.0) =>
                {
                    log.omit(
                        "Decals1_Graffiti Materials with an EmissivePower (their team colour and Fresnel emission): a \
                         placeholder",
                        1,
                    );
                    placeholder()
                }
                ("MAD_Tex1_Screen", _) => {
                    let diffuse = match i.source(TextureSource::Set(0)) {
                        Some(t) => t,
                        None => {
                            log.count("Tex1_Screen diffuses unset: white (stated)", 1);
                            *white.get_or_insert_with(|| white_texture(w))
                        }
                    };
                    let base = made_texture(w, &mut made, (diffuse, "screen", [0; 5]), |w| {
                        derived(w, diffuse, TexRole::Color, ColorSpace::Srgb, |_, c| {
                            let v = |x: f64| primase_image::linear_to_srgb8(0.6 * sat(x).powf(2.2));
                            [v(c[0]), v(c[1]), v(c[2]), 255]
                        })
                    })?;
                    let k = SCREEN_EMISSION / self.emissive_exposure;
                    let (emissive, emissive_strength) = if k <= 1.0 {
                        ([k as f32; 3], 1.0)
                    } else {
                        ([1.0; 3], k as f32)
                    };
                    log.count("MAD_Tex1_Screen Materials lowered", 1);
                    PbrMaterial {
                        base_color_texture: Some(TextureRef::new(base)),
                        metallic: 0.0,
                        roughness: 0.1,
                        emissive,
                        emissive_texture: Some(TextureRef::new(base)),
                        emissive_strength,
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_Screens", _)
                    if binds_all(&m, &SCREENS)
                        && i.scalar(5) == 0.0
                        && (i.scalar(4) == 0.0 || i.vector(7).is_some_and(|c| c[..3] == [0.0; 3])) =>
                {
                    log.count(
                        "MAD_Screens Materials lowered (black: no video, and no logo or a black Neutral colour)",
                        1,
                    );
                    PbrMaterial {
                        base_color: [0.0, 0.0, 0.0, 1.0],
                        metallic: 0.0,
                        roughness: 1.0,
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_SHD_UNLIT_UndergroundBox", _) if binds_all(&m, &UNDERGROUND_BOX) => {
                    let (a, b) = (i.vector(1).unwrap_or([0.0; 4]), i.vector(2).unwrap_or([0.0; 4]));
                    let x = self.underground_view;
                    let c: [f32; 3] = std::array::from_fn(|k| (b[k] + (a[k] - b[k]) * x) as f32);
                    log.approximate(
                        "UndergroundBox Materials' view gradient at the stated underground-view",
                        1,
                    );
                    log.count("MAD_SHD_UNLIT_UndergroundBox Materials lowered", 1);
                    PbrMaterial {
                        base_color: [c[0], c[1], c[2], 1.0],
                        metallic: 0.0,
                        roughness: 1.0,
                        alpha_mode: AlphaMode::Opaque,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_Tex1_FakeRiver", _) if binds_all(&m, &FAKE_RIVER) => {
                    let s = self.fake_river_s;
                    let foam = match i.texture(1) {
                        Some(t) => t,
                        None => {
                            log.count("FakeRiver foam maps unset: white (stated)", 1);
                            *white.get_or_insert_with(|| white_texture(w))
                        }
                    };
                    let base = made_texture(
                        w,
                        &mut made,
                        (foam, "fake river", [s.to_bits(), 0, 0, 0, 0]),
                        |w| {
                            derived(w, foam, TexRole::Color, ColorSpace::Srgb, |_, c| {
                                let albedo = fake_river_albedo(c[3], s);
                                let v = |x: f64| primase_image::linear_to_srgb8(x);
                                [v(albedo[0]), v(albedo[1]), v(albedo[2]), 255]
                            })
                        },
                    )?;
                    let normals = i.texture(0);
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    let uv = lowered_uv(&i, Some(2), self.shader_uv, "FakeRiver", log);
                    let sampled_by = |texture: Entity| TextureRef {
                        transform: uv,
                        ..TextureRef::new(texture)
                    };
                    let rm = sat(i.scalar(1)) as f32;
                    log.approximate(
                        "FakeRiver Materials' COLOR0.x (s) at the stated fake-river-s, not per vertex",
                        1,
                    );
                    log.count("MAD_Tex1_FakeRiver Materials lowered", 1);
                    PbrMaterial {
                        base_color_texture: Some(sampled_by(base)),
                        metallic: rm,
                        roughness: rm,
                        normal_texture: normals.map(sampled_by),
                        normal_scale: 0.1,
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                ("SHD_Nat_Veg_Grass_AO", _)
                    if binds_all(&m, &GRASS_AO) && (0.0..=1.0).contains(&i.scalar(1)) =>
                {
                    let diffuse = i.texture(0);
                    if let Some(d) = diffuse {
                        set_colour(w, d, TexRole::Color);
                    }
                    let normals = i.source(TextureSource::Set(1));
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    let ao_mult = i.scalar(1);
                    if ao_mult != 0.0 {
                        takes_colour = true;
                        colour_rules.insert(e, ColourRule::AlphaAo(ao_mult));
                    }
                    log.omit("Grass_AO Materials' specular occlusion (0.2, no PBR place)", 1);
                    log.count("SHD_Nat_Veg_Grass_AO Materials lowered", 1);
                    PbrMaterial {
                        base_color_texture: diffuse.map(TextureRef::new),
                        metallic: 0.0,
                        roughness: 1.0,
                        normal_texture: normals.map(TextureRef::new),
                        alpha_mode,
                        double_sided: m.flags.booleans.get(MaterialFlags::TWO_SIDED) == Some(&1),
                        ..PbrMaterial::default()
                    }
                }
                ("SHD_Nat_Veg_Grass_AO", _) => {
                    log.omit(
                        "Grass_AO Materials not binding their shader's rows, or with an AO Mult outside 0 to 1: a \
                         placeholder",
                        1,
                    );
                    placeholder()
                }
                (_, _) if tex2.is_some_and(|rows| rows.state.is_some_and(|s| i.scalar(s) != 0.0)) => {
                    log.omit(
                        "Tex2 graffiti Materials with a State Value (the graffiti palette, runtime state): a placeholder",
                        1,
                    );
                    placeholder()
                }
                (_, _) if tex2.is_some_and(|rows| !binds_all(&m, &rows.reads)) => {
                    log.omit(
                        &format!(
                            "Materials of template {name:?} (it doesn't bind the properties its evaluated shader reads: \
                             a neutral placeholder drawn)"
                        ),
                        1,
                    );
                    placeholder()
                }
                (_, _) if let Some(rows) = tex2 => {
                    let scalar = |v: u32| i.scalar(v);
                    if rows.vertex_colour.is_some_and(|v| scalar(v) != 0.0) {
                        log.omit(
                            "Tex2 Materials' vertex-colour factor (COLOR0 weights the layers; its colour factor isn't \
                             lowered)",
                            1,
                        );
                    }
                    let uv_a = lowered_uv(&i, Some(rows.uv[0]), self.shader_uv, "Tex2 A", log);
                    let uv_b = lowered_uv(&i, Some(rows.uv[1]), self.shader_uv, "Tex2 B", log);
                    let ao_on = scalar(rows.ao);
                    // each layer an ordinary material: (diffuse, normal map, mask, its HSL, its roughness offset)
                    let parts = [
                        (
                            i.texture(3),
                            i.texture(0),
                            i.texture(5),
                            rows.tint_a,
                            rows.roughness_offsets.map(|o| o[0]),
                        ),
                        (
                            i.texture(1),
                            i.texture(2),
                            i.texture(4),
                            rows.tint_b,
                            rows.roughness_offsets.map(|o| o[1]),
                        ),
                    ];
                    let mut layers = Vec::with_capacity(2);
                    let mut heights = Vec::with_capacity(2);
                    for (((diffuse, normals, mask, hsl, offset), uv), which) in
                        parts.into_iter().zip([uv_a, uv_b]).zip(["A", "B"])
                    {
                        let (Some(d), Some(n), Some(k)) = (diffuse, normals, mask) else {
                            layers.push(None);
                            continue;
                        };
                        let base = base_colour(w, &mut made, d, hsl.map(scalar), Some(k), None)?;
                        let rough = offset.map_or(Rough::Raw, |o| Rough::Tex2(scalar(o)));
                        let mr = metal_rough(w, &mut made, n, None, rough)?;
                        let occlusion = if ao_on != 0.0 {
                            Some(occlusion_of(w, &mut made, k, ao_on)?)
                        } else {
                            None
                        };
                        set_normal(w, n, normal);
                        let layer = w.spawn();
                        if let Some(Name(material)) = w.get::<Name>(e).cloned() {
                            w.insert(layer, Name(format!("{material} layer {which}")));
                        }
                        w.insert(
                            layer,
                            PbrMaterial {
                                base_color_texture: Some(TextureRef::new(base)),
                                metallic: 1.0,
                                roughness: 1.0,
                                metallic_roughness_texture: Some(TextureRef::new(mr)),
                                occlusion_texture: occlusion.map(TextureRef::new),
                                normal_texture: Some(TextureRef::new(n)),
                                ..PbrMaterial::default()
                            },
                        );
                        w.insert(layer, VertexColor::Ignored);
                        // the layer's height: its diffuse's alpha, at its own uv
                        heights.push(Source::Mask {
                            texture: TextureRef {
                                transform: uv,
                                ..TextureRef::new(d)
                            },
                            channels: [0.0, 0.0, 0.0, 1.0],
                            bias: 0.0,
                        });
                        layers.push(Some(Layer {
                            material: layer,
                            texcoord: None,
                            transform: uv,
                        }));
                    }
                    if heights.len() == 2 {
                        // w = sat((COLOR0.a - 0.5) AlphaContrast + 0.5 + AlphaContrast AlphaOffset): layer B's weight,
                        // A's 1 - w (= sat(1 - that))
                        let (contrast, offset) = (scalar(rows.alpha_contrast), scalar(rows.alpha_offset));
                        let k0 = (0.5 + contrast * offset - 0.5 * contrast) as f32;
                        let k1 = contrast as f32;
                        let alpha = Source::Vertex {
                            semantic: Semantic::Color(0),
                            channel: 3,
                        };
                        let threshold = |k: [f32; 4]| Channel::Threshold {
                            v: alpha.clone(),
                            m: alpha.clone(),
                            k,
                        };
                        let mut weights = LayerWeights::new(Semantic::Color(0), WeightMapping::Identity);
                        weights.channels = Some(vec![
                            threshold([1.0 - k0, -k1, 0.0, 0.0]),
                            threshold([k0, k1, 0.0, 0.0]),
                        ]);
                        weights.composition = Composition::Height {
                            contrast: scalar(rows.height_contrast) as f32,
                        };
                        weights.heights = heights;
                        w.insert(e, LayeredMaterial { layers, weights });
                        log.count(
                            &format!("{name} Materials lowered (a height blend of two layers)"),
                            1,
                        );
                        layered = true;
                        placeholder()
                    } else {
                        log.omit(
                            "Tex2 Materials missing a layer's diffuse, normal map or mask: a placeholder",
                            1,
                        );
                        placeholder()
                    }
                }
                ("MAD_Clutters_Grass", _) if !binds_all(&m, &GRASS) => {
                    log.omit(
                        "Materials of template \"MAD_Clutters_Grass\" (it doesn't bind the properties its evaluated \
                         shader reads: a neutral placeholder drawn)",
                        1,
                    );
                    placeholder()
                }
                ("MAD_Clutters_Grass", _) => {
                    // layout B, shading class 4 (foliage): non-metal; each texture its binding's, else its TextureSet slot
                    let of = |shader: u32, set: usize| {
                        i.texture(shader)
                            .or_else(|| m.texture_set.get(set).copied().flatten())
                    };
                    let (mask, alpha, diffuse, normals) = (of(0, 7), of(1, 18), of(2, 0), of(3, 1));
                    let scalar = |v: u32| i.scalar(v);
                    if scalar(15) != 0.0 && [13, 14, 17].iter().any(|&v| scalar(v) != 0.0) {
                        log.omit(
                            "Clutters_Grass Materials' noise tint (weighted by a noise tiled in world space, not the \
                             mesh's uv): left out",
                            1,
                        );
                    }
                    for (v, what) in [(10, "translucency"), (11, "specular occlusion")] {
                        if scalar(v) != 0.0 {
                            log.omit(
                                &format!("Clutters_Grass Materials' {what} (no PBR place): left out"),
                                1,
                            );
                        }
                    }
                    log.omit(
                        "Clutters_Grass Materials' per-instance dither fade by distance: left out",
                        1,
                    );
                    if alpha.is_none() {
                        log.omit(
                            "Clutters_Grass alpha maps unset (TextureSet slot 18): the diffuse alpha",
                            1,
                        );
                    }
                    let base = match diffuse {
                        Some(d) => Some(base_colour(w, &mut made, d, [0.0; 3], None, alpha)?),
                        None => None,
                    };
                    let mr = match normals {
                        Some(n) => Some(metal_rough(w, &mut made, n, Some(0.0), Rough::Raw)?),
                        None => None,
                    };
                    let ao_on = scalar(16);
                    let occlusion = match mask {
                        Some(k) if ao_on != 0.0 => Some(occlusion_of(w, &mut made, k, ao_on)?),
                        _ => None,
                    };
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    log.count("MAD_Clutters_Grass Materials lowered", 1);
                    PbrMaterial {
                        base_color_texture: base.map(TextureRef::new),
                        metallic: 1.0,
                        roughness: 1.0,
                        metallic_roughness_texture: mr.map(TextureRef::new),
                        occlusion_texture: occlusion.map(TextureRef::new),
                        normal_texture: normals.map(TextureRef::new),
                        alpha_mode,
                        double_sided: true,
                        ..PbrMaterial::default()
                    }
                }
                ("MAD_Tex1_Graffiti_Mode", Some(layout))
                    if binds(&m, &layout)
                        && !(i.scalar(3) == 0.0
                            && [7, 8].iter().all(|&v| i.vector(v).is_some_and(|c| c[3] <= 0.0))) =>
                {
                    log.omit(
                        "Graffiti Materials with a logo or a team or custom colour (alpha above 0, or unset): runtime \
                         state, a neutral placeholder drawn",
                        1,
                    );
                    placeholder()
                }
                (_, Some(layout)) if !binds(&m, &layout) => {
                    log.omit(
                        &format!(
                            "Materials of template {name:?} (it doesn't bind the properties its evaluated shader \
                             reads: a neutral placeholder drawn)"
                        ),
                        1,
                    );
                    placeholder()
                }
                (_, Some(layout)) => {
                    let normals = i.source(layout.normal);
                    let diffuse = i.source(layout.diffuse);
                    let mask = layout.mask.map(|source| match i.source(source) {
                        Some(t) => t,
                        None => {
                            log.count("Tex1 masks unset: white (stated)", 1);
                            *white.get_or_insert_with(|| white_texture(w))
                        }
                    });
                    let [hue, saturation, luminosity] = layout.tint.map(|v| i.scalar(v));
                    let metal = match layout.metalness {
                        Metalness::Blue => None,
                        Metalness::Scaled(v) => Some(i.scalar(v)),
                        Metalness::Absent => Some(0.0),
                    };
                    let rough_offset = layout.roughness_offset.map(|v| i.scalar(v));
                    let ao_on = layout.ao.map_or(0.0, |v| i.scalar(v));
                    match layout.vertex_colour.map(|v| i.scalar(v)) {
                        None | Some(0.0) => {}
                        Some(1.0) => {
                            takes_colour = true;
                            log.approximate(
                                "Tex1 Materials' vertex-colour factor 1 as the mesh's COLOR_0 (a consumer multiplies \
                                 its alpha in too; the shader doesn't)",
                                1,
                            );
                        }
                        Some(_) => log.omit(
                            "Tex1 Materials' vertex-colour factor other than 0 and 1 (1 + f (COLOR0 - 1) isn't a \
                             COLOR_0 product): drawn without it",
                            1,
                        ),
                    }
                    if layout.terrain_blend.is_some_and(|v| i.scalar(v) != 0.0) {
                        log.omit(
                            "TerrainBlend Materials' blend toward the terrain's colour near the ground (the engine's \
                             terrain textures): left out",
                            1,
                        );
                    }
                    if layout.translucency.is_some_and(|v| i.scalar(v) != 0.0) {
                        log.omit(
                            "Tex1_Wind Materials' translucency (written to the G-buffer as is, not PBR): left out",
                            1,
                        );
                    }
                    if name == "MAD_Tex1_UGE" && i.scalar(8) != 0.0 {
                        log.omit(
                            "UGE Materials' Overriden Emissive Scale (read only by the instanced permutations, its \
                             per-instance blend not decoded): left out",
                            1,
                        );
                    }
                    let uv = lowered_uv(&i, layout.uv, self.shader_uv, "Tex1", log);
                    let sampled_by = |texture: Entity| TextureRef {
                        transform: uv,
                        ..TextureRef::new(texture)
                    };
                    let (emissive, emissive_texture, emissive_strength) = match layout.emission {
                        Some(em) => emission_of(
                            w,
                            &mut made,
                            &i,
                            em,
                            diffuse,
                            [hue, saturation, luminosity],
                            takes_colour,
                            self.emissive_exposure,
                            log,
                        )?,
                        None => ([0.0; 3], None, 1.0),
                    };
                    let alpha = layout.alpha.map_or(1.0, |v| i.scalar(v) as f32);
                    let base = match diffuse {
                        Some(d) => Some(base_colour(
                            w,
                            &mut made,
                            d,
                            [hue, saturation, luminosity],
                            mask,
                            None,
                        )?),
                        None => None,
                    };
                    let mr = match normals {
                        Some(n) => Some(metal_rough(
                            w,
                            &mut made,
                            n,
                            metal,
                            rough_offset.map_or(Rough::Raw, Rough::Offset),
                        )?),
                        None => None,
                    };
                    let occlusion = match mask {
                        Some(k) if ao_on != 0.0 => Some(occlusion_of(w, &mut made, k, ao_on)?),
                        _ => None,
                    };
                    if let Some(n) = normals {
                        set_normal(w, n, normal);
                    }
                    log.count(&format!("{name} Materials lowered"), 1);
                    PbrMaterial {
                        base_color: [1.0, 1.0, 1.0, alpha],
                        base_color_texture: base.map(sampled_by),
                        metallic: 1.0,
                        roughness: 1.0,
                        metallic_roughness_texture: mr.map(sampled_by),
                        occlusion_texture: occlusion.map(sampled_by),
                        normal_texture: normals.map(sampled_by),
                        emissive,
                        emissive_texture: emissive_texture.map(sampled_by),
                        emissive_strength,
                        alpha_mode,
                        ..PbrMaterial::default()
                    }
                }
                (other, None) => {
                    log.omit(
                        &format!(
                            "Materials of template {other:?} (not lowered: a neutral placeholder drawn)"
                        ),
                        1,
                    );
                    placeholder()
                }
            };
            if !layered {
                w.insert(e, pbr);
            }
            if !takes_colour {
                w.insert(e, VertexColor::Ignored);
            }
        }
        vertex_colours(w, &colour_rules, log)?;
        Ok(())
    }
}

/// How a Material's shader takes COLOR0, as the pass rewrites `COLOR_0` for it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ColourRule {
    /// MAD_FakeForest's albedo: `pow(saturate(c), 2.2)` per colour channel, alpha as stored.
    Pow22,
    /// Grass_AO's factor `1 + m (a - 1)` of the vertex alpha `a`, on each colour channel; alpha 1.
    AlphaAo(f64),
}

impl ColourRule {
    fn name(self) -> &'static str {
        match self {
            ColourRule::Pow22 => "FakeForest",
            ColourRule::AlphaAo(_) => "Grass_AO",
        }
    }
    fn apply(self, [r, g, b, a]: [f32; 4]) -> [f32; 4] {
        match self {
            ColourRule::Pow22 => {
                let d = |c: f32| f64::from(c).clamp(0.0, 1.0).powf(2.2) as f32;
                [d(r), d(g), d(b), a]
            }
            ColourRule::AlphaAo(m) => {
                let f = (1.0 + m * (f64::from(a) - 1.0)) as f32;
                [f, f, f, 1.0]
            }
        }
    }
}

/// `COLOR_0` of every primitive drawn with Materials of one rule of `rules` and nothing else rewritten by that rule, in
/// f32 (see the module documentation).
fn vertex_colours(
    w: &mut World,
    rules: &BTreeMap<Entity, ColourRule>,
    log: &mut PassLog,
) -> Result<(), String> {
    if rules.is_empty() {
        return Ok(());
    }
    for (_, f) in w.iter::<MeshFile>() {
        if let Some(rule) = f.materials.iter().flatten().find_map(|m| rules.get(m)) {
            return Err(format!(
                "a {} Material draws a mesh still in its file: load it before lower-anvil-material",
                rule.name()
            ));
        }
    }
    let prototypes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
    for e in prototypes {
        let mesh = w.get::<Mesh>(e).ok_or("listed")?;
        let variants = w.get::<MaterialVariants>(e);
        let mut decode = Vec::new();
        for (i, p) in mesh.primitives().iter().enumerate() {
            let drawn: Vec<Entity> = p
                .material
                .into_iter()
                .chain(
                    variants
                        .and_then(|v| v.0.get(i))
                        .into_iter()
                        .flat_map(|v| v.values().copied()),
                )
                .collect();
            let Some(&rule) = drawn.iter().find_map(|m| rules.get(m)) else {
                continue;
            };
            if !drawn.iter().all(|m| rules.get(m) == Some(&rule)) {
                log.omit(
                    &format!(
                        "primitives drawn with a {} Material and another: COLOR_0 as stored",
                        rule.name()
                    ),
                    1,
                );
                continue;
            }
            match p.streams.get(&Semantic::Color(0)) {
                None => log.omit(
                    &format!(
                        "{} primitives without COLOR_0: drawn white (the shader's default isn't read)",
                        rule.name()
                    ),
                    1,
                ),
                Some(stream) => decode.push((i, rule, stream.clone())),
            }
        }
        if decode.is_empty() {
            continue;
        }
        let mesh = w.get_mut::<Mesh>(e).ok_or("listed")?;
        for (i, rule, stream) in decode {
            let colours: Vec<[f32; 4]> = match stream {
                Stream::U8x4(v) => v.iter().map(|c| c.map(|x| f32::from(x) / 255.0)).collect(),
                Stream::U16x4(v) => v.iter().map(|c| c.map(|x| f32::from(x) / 65535.0)).collect(),
                Stream::F32x4(v) => v,
                Stream::F32x3(v) => v.iter().map(|&[r, g, b]| [r, g, b, 1.0]).collect(),
                _ => {
                    return Err(format!(
                        "a {} primitive's COLOR_0 isn't a colour stream",
                        rule.name()
                    ));
                }
            };
            let rewritten = colours.into_iter().map(|c| rule.apply(c)).collect();
            let mut p = mesh.primitive_mut(i).ok_or("listed")?;
            p.streams_mut()
                .insert(Semantic::Color(0), Stream::F32x4(rewritten));
            log.count(
                match rule {
                    ColourRule::Pow22 => "FakeForest primitives' COLOR_0 decoded (pow 2.2)",
                    ColourRule::AlphaAo(_) => {
                        "Grass_AO primitives' COLOR_0 made its AO factor of the vertex alpha"
                    }
                },
                1,
            );
        }
    }
    Ok(())
}

/// Whether a MAD_Grounds Material's arithmetic is layer A alone with the neutral vertex (§27.7, `c` 1 and the vertex
/// colour white): `Use Blend` (11) 0, so the blend is 0 whatever its mask; `Tire Albedo` (8) at most 1, so its factor
/// `sat(c - TA + 1)` is 1 (the other tire factor is 1 for any mask at `c` 1); `Tire Vertex Color Impact` (10) at least
/// 0, so `w1 = 1 - sat(c + TVI)` is 0 and the vertex-colour term is the identity for any mask.
fn grounds_reduce(i: &Inputs) -> bool {
    i.scalar(11) == 0.0 && i.scalar(8) <= 1.0 && i.scalar(10) >= 0.0
}

/// MAD_Tex1_Graffiti_Mode_DecalFX lowered (see the module documentation); `None`, reported, where its arithmetic isn't
/// a flat-coloured cut-out.
fn decal_fx(
    w: &mut World,
    made: &mut BTreeMap<MadeKey, Entity>,
    i: &Inputs<'_>,
    m: &AnvilMaterial,
    shader_uv: ShaderUv,
    exposure: f64,
    log: &mut PassLog,
) -> Result<Option<PbrMaterial>, String> {
    let refuse = |log: &mut PassLog, why: &str| {
        log.omit(&format!("DecalFX Materials {why}: a placeholder"), 1);
        None
    };
    if !binds_all(m, &DECAL_FX) {
        return Ok(refuse(
            log,
            "whose template doesn't bind the properties its shader reads",
        ));
    }
    if i.scalar(6) < 1.0 {
        return Ok(refuse(
            log,
            "with a TeamBlendRatio under 1 (their colour stripes in screen space)",
        ));
    }
    let Some(team) = i.vector(4) else {
        return Ok(refuse(log, "without a team colour"));
    };
    if sat(team[3]) != 1.0 {
        return Ok(refuse(
            log,
            "whose team colour isn't opaque (the atlas's colour mixes in)",
        ));
    }
    let enabled = m
        .flags
        .booleans
        .get(MaterialFlags::ALPHA_TEST_ENABLED)
        .copied()
        .unwrap_or(0);
    if enabled != 1 {
        return Ok(refuse(
            log,
            "not alpha-tested (another permutation than the one read)",
        ));
    }
    let Some(diffuse) = i.texture(0) else {
        return Ok(refuse(log, "without their atlas"));
    };
    let transform = match i.scalar(2) {
        0.0 => lowered_uv(i, Some(8), shader_uv, "DecalFX", log),
        1.0 => {
            let Some(dim) = i.vector(5).filter(|d| d[0] > 0.0 && d[1] > 0.0) else {
                return Ok(refuse(log, "without an atlas size"));
            };
            match uv_transform(&atlas_cell(i.scalar(1), dim[0], dim[1]), shader_uv) {
                Ok(t) => t,
                Err(why) => return Ok(refuse(log, &format!("whose atlas cell isn't lowered ({why})"))),
            }
        }
        _ => return Ok(refuse(log, "with a Color - Logo between 0 and 1")),
    };
    // emission: EmissiveMult g^2 over the exposure, coloured by the albedo (the team colour to the 2.2)
    let colour = [0, 1, 2].map(|c| sat(team[c]).powf(2.2) as f32);
    let k = i.scalar(7) / exposure;
    let (emissive, emissive_texture, emissive_strength) = match i.texture(1) {
        _ if k == 0.0 => ([0.0; 3], None, 1.0),
        _ if k < 0.0 => {
            log.omit(
                "DecalFX Materials' negative EmissiveMult (no glTF emission): no emission drawn",
                1,
            );
            ([0.0; 3], None, 1.0)
        }
        None => {
            log.omit(
                "DecalFX Materials whose emissive mask resolves to none: no emission drawn",
                1,
            );
            ([0.0; 3], None, 1.0)
        }
        Some(mask) => {
            let lane = made_texture(w, made, (mask, "decal emission", [0; 5]), |w| {
                derived(w, mask, TexRole::Color, ColorSpace::Srgb, |_, c| {
                    let v = primase_image::linear_to_srgb8(c[1] * c[1]);
                    [v, v, v, 255]
                })
            })?;
            let uv = lowered_uv(i, Some(11), shader_uv, "DecalFX emissive", log);
            let e = colour.map(|c| f64::from(c) * k);
            let top = e.iter().copied().fold(0.0, f64::max);
            let (factor, strength) = if top <= 1.0 {
                (e, 1.0)
            } else {
                (e.map(|x| x / top), top)
            };
            (
                factor.map(|x| x as f32),
                Some(TextureRef {
                    transform: uv,
                    ..TextureRef::new(lane)
                }),
                strength as f32,
            )
        }
    };
    let shape = made_texture(w, made, (diffuse, "decal alpha", [0; 5]), |w| {
        derived(w, diffuse, TexRole::Color, ColorSpace::Srgb, |t, _| {
            [255, 255, 255, t.rgba[3]]
        })
    })?;
    log.count("DecalFX Materials lowered (a flat team colour, cut out)", 1);
    Ok(Some(PbrMaterial {
        base_color: [colour[0], colour[1], colour[2], 1.0],
        base_color_texture: Some(TextureRef {
            transform,
            ..TextureRef::new(shape)
        }),
        metallic: 0.0,
        roughness: 1.0,
        emissive,
        emissive_texture,
        emissive_strength,
        alpha_mode: AlphaMode::Mask {
            cutoff: f32::from(m.flags.alpha_test_value) / 255.0,
        },
        double_sided: false,
        ..PbrMaterial::default()
    }))
}

/// A made texture's key: its source, what it is, and the parameters' bits (and other sources' indices) it is made of.
type MadeKey = (Entity, &'static str, [u64; 5]);

/// A Material's UV transform bound at `vec4` (see the module documentation), each case reported under `what`.
fn lowered_uv(
    i: &Inputs,
    vec4: Option<u32>,
    shader_uv: ShaderUv,
    what: &str,
    log: &mut PassLog,
) -> Option<UvTransform> {
    match vec4.map(|v| i.uv(v)) {
        None => None,
        Some(None) | Some(Some(None)) => {
            log.omit(
                &format!("{what} UV transforms unset (the default isn't known): untransformed"),
                1,
            );
            None
        }
        Some(Some(Some(f))) => match uv_transform(f, shader_uv) {
            Ok(t) => {
                log.count(&format!("{what} UV transforms lowered"), 1);
                t
            }
            Err(why) => {
                log.omit(&format!("{what} UV transforms {why}: untransformed"), 1);
                None
            }
        },
    }
}

/// An RGBA8 texture's level 0 and whether it is sampled as sRGB.
fn level_0(w: &World, e: Entity) -> Result<Option<(Texture, bool)>, String> {
    let meta = TextureMetadata {
        color_space: ColorSpace::Linear,
        role: TexRole::Mask,
        normal: None,
    };
    w.get::<Texture>(e)
        .map(|t| map_rgba8_levels(t, 0..1, meta, |texel| Ok(texel.rgba)))
        .transpose()
        .map_err(|e| e.to_string())
        .map(|img| img.map(|img| (img, srgb(w, e))))
}

/// The base colour of `diffuse`: tinted by `hsl` (hue, saturation, luminosity offsets) in HSV where one is other than
/// 0, by `tint_mask`'s red where given, else everywhere; its alpha `opacity`'s red where given, else its own. Each
/// other texture is read at the texel nearest the diffuse texel's place. `diffuse` itself where nothing changes.
fn base_colour(
    w: &mut World,
    made: &mut BTreeMap<MadeKey, Entity>,
    diffuse: Entity,
    hsl: [f64; 3],
    tint_mask: Option<Entity>,
    opacity: Option<Entity>,
) -> Result<Entity, String> {
    let tint_on = hsl != [0.0; 3];
    if !tint_on && opacity.is_none() {
        set_colour(w, diffuse, TexRole::Color);
        return Ok(diffuse);
    }
    let index = |e: Option<Entity>| e.map_or(u64::MAX, |e| u64::from(e.index()));
    let [h, s, l] = hsl.map(f64::to_bits);
    let key = (
        diffuse,
        "base colour",
        [h, s, l, index(tint_mask), index(opacity)],
    );
    if let Some(&e) = made.get(&key) {
        return Ok(e);
    }
    let mask_image = tint_mask.map(|k| level_0(w, k)).transpose()?.flatten();
    let opacity_image = opacity.map(|k| level_0(w, k)).transpose()?.flatten();
    made_texture(w, made, key, |w| {
        derived(w, diffuse, TexRole::Color, ColorSpace::Srgb, |t, c| {
            let rgb = if tint_on {
                let m = mask_image
                    .as_ref()
                    .map_or(1.0, |(img, srgb)| sampled(nearest(img, t), *srgb)[0]);
                tinted([c[0], c[1], c[2]], m, hsl)
            } else {
                [c[0], c[1], c[2]]
            };
            let alpha = opacity_image
                .as_ref()
                .map_or(c[3], |(img, srgb)| sampled(nearest(img, t), *srgb)[0]);
            let enc = |x: f64| primase_image::linear_to_srgb8(x);
            [enc(rgb[0]), enc(rgb[1]), enc(rgb[2]), byte(alpha)]
        })
    })
}

/// An emissive template's emission: the factor, texture and strength of `albedo * mask.lane * Scale / exposure`
/// (docs/material-spec.md §23 (4): the ambient pass emits the G-buffer's albedo times the stored intensity), the
/// texture the diffuse tinted as the base colour is, times the mask's lane (sampled at the diffuse's texel, as the
/// tint mask is). A factor above 1 is carried in the strength (the factor within 0-1); none where the Material binds
/// no diffuse or mask or its emission is 0, reported where it binds none or its scale is negative, and the vertex
/// colour factor (in the G-buffer's albedo, not in a glTF emission) reported where it is on.
#[allow(
    clippy::too_many_arguments,
    reason = "the template's inputs the emission is made of"
)]
fn emission_of(
    w: &mut World,
    made: &mut BTreeMap<MadeKey, Entity>,
    i: &Inputs<'_>,
    em: Emission,
    diffuse: Option<Entity>,
    hsl: [f64; 3],
    vertex_colour: bool,
    exposure: f64,
    log: &mut PassLog,
) -> Result<([f32; 3], Option<Entity>, f32), String> {
    let none = ([0.0; 3], None, 1.0);
    // the mask's lane is the texture; the rest a factor
    let e = emission(1.0, i.scalar(em.scale), exposure);
    if e == 0.0 {
        return Ok(none);
    }
    if e < 0.0 {
        log.omit(
            "Emissive Materials' negative Emissive Scale (no glTF emission): no emission drawn",
            1,
        );
        return Ok(none);
    }
    let (Some(mask), Some(diffuse)) = (i.source(em.mask), diffuse) else {
        log.omit(
            "Emissive Materials' emissive masks or diffuses unset (the engine's defaults not known): no emission \
             drawn",
            1,
        );
        return Ok(none);
    };
    if vertex_colour {
        log.omit(
            "Emissive Materials' vertex-colour factor in their emission (a glTF COLOR_0 multiplies the base colour \
             only): left out of the emission",
            1,
        );
    }
    let lane = em.lane;
    let [h, s, l] = hsl.map(f64::to_bits);
    let key = (
        diffuse,
        "emissive",
        [h, s, l, u64::from(mask.index()), lane as u64],
    );
    let Some((mask_image, mask_srgb)) = level_0(w, mask)? else {
        log.omit(
            "Emissive Materials' emissive masks without an image: no emission drawn",
            1,
        );
        return Ok(none);
    };
    let texture = made_texture(w, made, key, |w| {
        derived(w, diffuse, TexRole::Color, ColorSpace::Srgb, |t, c| {
            let rgb = tinted([c[0], c[1], c[2]], 1.0, hsl);
            let m = sampled(nearest(&mask_image, t), mask_srgb)[lane];
            let enc = |x: f64| primase_image::linear_to_srgb8(x * m);
            [enc(rgb[0]), enc(rgb[1]), enc(rgb[2]), 255]
        })
    })?;
    let (factor, strength) = if e <= 1.0 { (e, 1.0) } else { (1.0, e) };
    Ok(([factor as f32; 3], Some(texture), strength as f32))
}

/// The occlusion texture made of `mask`'s blue: `1 + on (b - 1)`.
fn occlusion_of(
    w: &mut World,
    made: &mut BTreeMap<MadeKey, Entity>,
    mask: Entity,
    on: f64,
) -> Result<Entity, String> {
    made_texture(w, made, (mask, "ao", [on.to_bits(), 0, 0, 0, 0]), |w| {
        derived(w, mask, TexRole::Mask, ColorSpace::Linear, |_, c| {
            let ao = byte(occlusion(c[2], on));
            [ao, ao, ao, 255]
        })
    })
}

/// The metallic-roughness texture made of the normal map `n`'s blue and alpha: metalness `b * metal` (`b` where
/// `None`), roughness by `rough`.
fn metal_rough(
    w: &mut World,
    made: &mut BTreeMap<MadeKey, Entity>,
    n: Entity,
    metal: Option<f64>,
    rough: Rough,
) -> Result<Entity, String> {
    let bits = |x: Option<f64>| x.map_or(u64::MAX, f64::to_bits);
    let (curve, offset) = match rough {
        Rough::Raw => (0, None),
        Rough::Offset(o) => (1, Some(o)),
        Rough::Tex2(o) => (2, Some(o)),
    };
    made_texture(
        w,
        made,
        (n, "mr", [bits(metal), bits(offset), curve, 0, 0]),
        |w| {
            derived(w, n, TexRole::Mask, ColorSpace::Linear, |_, c| {
                let rough = roughness(c[3], rough);
                let metal = metalness(c[2], metal);
                [255, byte(rough), byte(metal), 255]
            })
        },
    )
}

/// The neutral grey a Material not lowered is drawn with.
/// MAD_Tex1_Screen's emission before the exposure: its shader's constant (§27.14.4).
const SCREEN_EMISSION: f64 = 0.666_667;

/// MAD_Tex1_FakeRiver's albedo at a foam map alpha `a` and its `s` (§27.14.9); the constants its program's f32
/// literals, by their bits.
pub fn fake_river_albedo(a: f64, s: f64) -> [f64; 3] {
    let f = |bits: [u32; 3]| bits.map(|b| f64::from(f32::from_bits(b)));
    let low = f([1_033_410_766, 1_037_095_084, 1_031_831_640]);
    let floor = f([1_027_653_818, 1_030_811_881, 1_032_884_364]);
    let target = f([1_040_220_275, 1_042_588_816, 1_047_852_298]);
    let foam = a * (1.5 - s);
    std::array::from_fn(|k| {
        let base = (1.0 - s) * low[k] + floor[k];
        base + foam * (target[k] - base)
    })
}

fn placeholder() -> PbrMaterial {
    PbrMaterial {
        base_color: [0.5, 0.5, 0.5, 1.0],
        metallic: 0.0,
        roughness: 1.0,
        ..PbrMaterial::default()
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
    made: &mut BTreeMap<MadeKey, Entity>,
    key: MadeKey,
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

    /// Against shader rows the notes read in the world snapshot (docs/material-spec.md §12.2): the translated one as
    /// stored, `(3.25, 0, -1.875 | 0, 1.7, -0.75)`, and a rotated combination (`su = sv = 30`, `t = 2.57`) by the
    /// rows' form. Each lowered transform takes a mesh coordinate to the one whose shader coordinates the rows give.
    #[test]
    fn lowered_transforms_follow_the_shaders_rows() {
        let (s, c) = 2.57f32.sin_cos();
        let cases = [
            (
                [3.25, 1.7, -1.875, -0.75, 0.0, 0.0, 0.0, 0.0],
                [[3.25, 0.0, -1.875], [0.0, 1.7, -0.75]],
            ),
            (
                [30.0, 30.0, 0.0, 0.0, 0.0, 0.0, 2.57, 0.0],
                [[30.0 * c, -30.0 * s, 0.0], [30.0 * s, 30.0 * c, 0.0]],
            ),
        ];
        for (floats, rows) in cases {
            for (shader_uv, flip) in [(ShaderUv::TexCoord, false), (ShaderUv::OneMinusV, true)] {
                let t = uv_transform(&floats, shader_uv)
                    .expect("lowered")
                    .expect("not the identity");
                let to_shader = |[u, v]: [f32; 2]| if flip { [u, 1.0 - v] } else { [u, v] };
                for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.3, 0.7], [-2.0, 5.5]] {
                    let [u, v] = to_shader(uv);
                    let shaded = rows.map(|r| r[0] * u + r[1] * v + r[2]);
                    let want = to_shader(shaded);
                    let got = t.apply(uv);
                    assert!(
                        (0..2).all(|k| (got[k] - want[k]).abs() <= 1e-4 * want[k].abs().max(1.0)),
                        "{floats:?} {shader_uv:?} at {uv:?}: {got:?}, the rows {want:?}"
                    );
                }
            }
        }
        // the forms not seen are refused
        assert!(uv_transform(&[2.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.8727, 0.0], ShaderUv::TexCoord).is_err());
        assert!(uv_transform(&[1.0, 1.0, 0.5, 0.0, 0.0, 0.0, 0.8727, 0.0], ShaderUv::TexCoord).is_err());
        assert!(uv_transform(&[1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0], ShaderUv::TexCoord).is_err());
    }
}
