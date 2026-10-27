//! `bake-anvil-material{size=N,normal-green=up|down,shader-uv=texcoord|one-minus-v,mip-bias=X,terrain-ao=X,
//! sun-elevation=DEG,view-cosine=X,camera-distance=M,unset=white,
//! unstated-address=refuse|wrap|clamp|mirror|mirror-once}`: Anvil Materials
//! whose templates the lowering can't express as texture lookups ([`AnvilMaterial`]; MAD_Tex1_Rocks and
//! MAD_Tex1_UniqueNormalGradient and MAD_Tex1_UniqueNormal_DiffuseBlend, which combine maps at several UV sets per
//! pixel, MAD_Hero_Ice and MAD_IceBrick, whose colour depends on the sun and the eye, MAD_Road, whose paint its
//! shader computes per pixel, MAD_Tex1_Graffiti_Monster, which sums three normal maps at two tilings, and
//! MAD_CHR_Hair_Anisotropic, whose colour mixes a root and a tip colour by the strand map) become
//! metal-rough PBR by running the game's own G-buffer pixel shader
//! over the Material's texture space and decoding what it writes (Riders Republic docs/shaders.md §6,
//! docs/material-spec.md §14.6, §15.4).
//!
//! - **Program** (established): the template's pixel permutation ([`AnvilShaders`], read with
//!   `Placer::with_shaders`) that the engine's key composer gives the G-buffer pass of a plain draw
//!   ([`gbuffer_pixel_key`]). A Material that sets a flag the composer gates by the template's feature word (not
//!   read) is left to `lower-anvil-material`, reported.
//! - **Inputs**: the program's constants from the Material's bound properties at the rows its template binds them
//!   ([`material_rows`]; static UV rows shared with the program binding), a texture per
//!   bound slot as the reader resolves it ([`crate::components::BoundTexture`]; sRGB where its TextureMap says so; one
//!   resolved to none sampled white, as `unset=white` states, the same statement as `lower-anvil-material`'s), sampled at the bake texel's own footprint (2 x 2 quads, the
//!   implicit level from the quad's UV gradients plus `mip-bias`, the engine's `cb1[41].y`: 0 in every frame the notes
//!   read, §17.1); the interpolants of a flat surface, tangent (1, 0, 0), normal (0, 0, 1), bitangent sign +1, so the
//!   normal it writes is the tangent-space normal. The engine's own textures (the terrain's, sampled at the
//!   placement's world position, so no property of the Material) are stated: the terrain's ambient occlusion
//!   (`t19`), which Rocks' `Use Terrain Albedo Mul` mixes into the albedo as `albedo (ao + 0.5)`, is `terrain-ao`
//!   everywhere (0.5 leaves the albedo as it is; every Rocks Material sets the switch); the sediment layer index
//!   (`t16`) is 0, no sediment, which takes the `Use Terrain Sediments` term out exactly; the rest are 0. A Material
//!   whose switch is on is counted as so approximated.
//! - **Output** (§15.4, by the shading class the program writes): base colour `o0.rgb` (sRGB), roughness `o0.w`, the
//!   normal decoded from `o1` as the lighting pass decodes it (its octahedral x, y and the low bits of `o1.z`, the
//!   target an 8-bit UNORM one), in the frame the shader builds (x along the tangent, y along cross(N, T) w, z along
//!   the normal: as a stored normal map is read, so `normal-green` states its green as `lower-anvil-material`'s
//!   does); layout B (classes 3 to 5: Rocks) has no metalness (the lighting pass uses F0 0.04 and the albedo as
//!   diffuse) and its occlusion in `o1.w`, its specular occlusion and translucency in `o2.zw`; layout A (class 1)
//!   its metalness in `o1.w`, emission in `o2.z`, occlusion, specular occlusion and translucency in `o3.xzw`; class 2
//!   (hair) has no metalness (the lighting pass fixes F0), its occlusion in `o3.y`, its specular occlusion and
//!   translucency as layout B's, in `o2.zw`.
//!   Occlusion is a texture where it isn't 1 everywhere; emission is the shared surface reading's linear colour,
//!   stored in an sRGB texture with an emissive strength retaining values above 1. Specular occlusion and
//!   translucency have no PBR place: where they aren't (1, 0) everywhere, reported. Opaque, single-sided, vertex colour ignored (the
//!   programs read none).
//! - **The sun and the eye**: a program whose colour mixes by the shaded normal's angle to the sun (`cb0[4]`) and to
//!   the eye (`cb1[2]`: the ice templates, material-spec §27.2) is run with the sun at `sun-elevation` degrees above
//!   the surface toward its tangent and the eye at `view-cosine` to its normal (the level's sun and a viewing angle:
//!   stated, the game's change with every frame); its parallax march sees the surface straight on (a tangent-space
//!   view with no xy: the march offsets nothing, §27.2).
//! - **The camera**: a program that mixes by the distance from the camera (UniqueNormalGradient's medium and micro
//!   albedo) is run with the camera `camera-distance` metres from the surface along its normal (stated: the game's
//!   mix changes with the distance every frame; far, past 20 m times the Material's `Detail Blend Range`, the mix is
//!   constant at the medium albedo, the notes' reading, §27.3), counted per Material; its micro detail, tens to
//!   hundreds of tiles across the UV square, is sampled at the bake texel's footprint like every texture.
//!
//! - **The character zones** (`MAD_CHR_5Zones_*`, 17 templates; docs/material-spec.md §18.2, §19): no albedo texture;
//!   a pixel is one of five zones chosen by `Mask1`'s thresholds, each a Material colour mixed with its variance colour
//!   by `Mask1.a`, over the decal; roughness, normal and metalness from `Mask2` and the Material's zone constants. The
//!   recipe takes the rows the template binds and the textures the program samples (`t75` and up: the permutations
//!   differ in their order), and states what the engine supplies and a static bake cannot: the mud layer (`FX-MUD`, a
//!   runtime dirt state whose mask is sampled as 0: `HideDirt` taken as 1, the notes' choice for a static export), the
//!   detail array (`Texture`, engine-bound: sampled as a flat detail normal, which leaves the normal as it was) and,
//!   where the Material binds none, the decal pair as no decal. The exposure `t23` is 1. A Material that isn't covered
//!   (`MAD_CHR_Skin` reads the character's customisation from a per-instance buffer) is left to `lower-anvil-material`.
//!
//! - **The character skin** (`MAD_CHR_Skin`; §18.3), with `skin-tone=R,G,B`: its plain G-buffer program writes black, the
//!   skin colour coming from a per-instance record in a typed buffer (`t70`) that the engine builds from the character's
//!   customisation, which the forge data does not hold. The pass runs the permutation with the per-instance data (key bit
//!   `0x8000`) and gives it a record built from the program's own unpack (read from its disassembly): four words at
//!   element 3 of `t70` that element 0 points to; `w0`: the tint where the normal's blue (the skin mask: high on flat
//!   skin, low in the cavities) is high in its top three bytes, a metalness byte in its low one; `w1`: the tint where it
//!   is low; `w2`: the translucency byte; `w3`: the facial-hair colour and the mask's exponent byte (`albedo = Diffuse *
//!   lerp(w1, 1, W) * lerp(1, w0, W)`, `W = |Normal.b|^(exponent / 255)`, a facial-hair colour mixed in by the Diffuse's
//!   alpha times the Material's `Use_Facial_Hair`). **What is stated, not read**: `skin-tone` is the one colour the
//!   caller chooses, used as `w0` (the engine's own conversion from a tone to the record is not read); `w1` white (no
//!   tint in the cavities), the facial-hair colour the tone itself (no separate beard colour), the exponent byte 255
//!   (W the mask itself), the metalness and translucency bytes 0, no vertex occlusion (`COLOR0.y` 1), no fade (the
//!   record's fade pair that discards pixels is 0 and 1) and instance 0. Without `skin-tone` a skin Material is left to
//!   `lower-anvil-material`.
//!
//! - **The character hair** (`MAD_CHR_Hair_Anisotropic`; §18.5, shading class 2): the zones' colours (three bands of the
//!   strand, each a root and a tip colour, contrast and offset, tinted by the ID and occlusion maps), roughness, the
//!   normal and the occlusion (`Diffuse`'s blue), from `Mask1`, `Diffuse` and `Normal` (t75 to t77) and rows 5 to 47. The
//!   program picks the band by the mesh's vertex colour (red) and takes its occlusion from it (green): the bake states
//!   a vertex colour of red 0, green 1, the first zone and no vertex occlusion, which is exact where the Material's
//!   second and third zone are off (rows 24 and 25 zero) and otherwise counted. Its specular occlusion (`o2.z`: the
//!   camera's distance and the eye's angle), translucency (`o2.w`) and the strand direction its anisotropic highlight
//!   reads (`o1.w`, `o3.x`, `o3.z`) have no place in a Material: left out, counted. The plain permutation has no
//!   coverage: its strands' cut-out is in the permutations with key bit `0x4000` (the depth pass's and the G-buffer's),
//!   which discard where `Mask1`'s alpha (t75.w) is below the Material's alpha reference (`AlphaTestValue / 255`, `cb2[18].y`) and,
//!   by a screen-position dither (`cb2[16]`), while the LOD fades. The bake states that coverage: the base colour's alpha
//!   is `Mask1`'s alpha at the texel's footprint and the Material a mask at `AlphaTestValue / 255`, where the Material
//!   tests alpha (`AlphaTestEnabled`, a value above 0, blend mode 0). That the engine draws these Materials with the
//!   `0x4000` permutations is inferred (no other permutation of the template cuts), not read from the engine's key
//!   composer; the dither is left out.
//!
//! A recipe whose paint is a pattern its shader computes (Road's lane lines) is baked over one period of the pattern in
//! the shader's own UV, its Material's UV transform set to the identity, and that transform scaled by the period put
//! on the texture references instead (`lower-anvil-material`'s reading of a UV transform, so `shader-uv` there too):
//! exact for a period the Material states as a whole number of units.
//!
//! The texture covers the shader's UV square (`shader-uv` relating it to the mesh's `TexCoord(0)` as for
//! `lower-anvil-material`), repeating: exact on a mesh whose UV leaves the square only where the Material's
//! function repeats with period 1, which the recipe requires of its UV transforms (integer coefficients). What isn't
//! the game's: the sampler (trilinear by the quad's footprint, [`iridia_exec::Filtering::Trilinear`]; its
//! addressing as `unstated-address` states, reported, since resolve-anvil-shaders does not read the engine's sampler
//! states: reading them is the open item that retires the option), the draw (no per-instance data, no LOD fade, not
//! reduced), the 8-bit encodings of the outputs.

use std::collections::BTreeMap;
use std::sync::Arc;

use iridia_exec::{
    Filtering, Grid, Input, Invocation, Layout, Lod, PixelShader, SampledTexture, SamplerUse, Unstated,
    evaluate::Constants, evaluate_raw_with,
};
use iridia_interface::SamplerState;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::shader::NormalEncoding;
use primase_core::{
    ColorSpace, Component, Entity, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat,
    TexRole, Texture, VertexColor, World,
};
use primase_foundations::{sampling::Address, sampling::Transfer};

use super::material::{Green, NORMAL_GREEN, SHADER_UV, ShaderUv, UNSET, Unset, uv_transform};
use crate::components::{AnvilMaterial, AnvilShaders, MaterialBinding, MaterialValue};
use anvil_formats::material::MaterialFlags;

/// The G-buffer pass's number, in the key's bits 22 to 29.
const GBUFFER: u32 = 1;

/// The pixel ShaderKey the engine composes for the G-buffer pass of a plain draw of a Material with these flags
/// (docs/shaders.md §6.2, read from the engine): the pass in bits 22 to 29; of the bits the Material sets, each is
/// gated by the template's feature word, which isn't read, so a Material that sets one is refused, naming it; a
/// Material that sets none has them all clear whatever the template. The draw's bits (vertex pulling, reduced
/// material levels, the mesh's per-instance features, the eight variant subsets) are those of a plain draw: none.
pub fn gbuffer_pixel_key(f: &MaterialFlags) -> Result<u32, &'static str> {
    if f.blend_mode > 0xF || f.alpha_display_mode > 3 {
        return Err("a blend or alpha display mode wider than the engine's field");
    }
    let mut set = [false; 35];
    for (s, &b) in set.iter_mut().zip(&f.booleans) {
        *s = match b {
            0 => false,
            1 => true,
            _ => return Err("a boolean stored as neither 0 nor 1"),
        };
    }
    if set[MaterialFlags::TWO_SIDED] && set[MaterialFlags::FLIP_NORMAL_ON_BACK_FACE] {
        return Err("TwoSided with FlipNormalOnBackFace (key bit 0x40, gated by the template)");
    }
    if f.alpha_display_mode == 2 {
        return Err("AlphaDisplayMode 2 (key bit 0x2, gated by the template)");
    }
    if set[MaterialFlags::LAYER_MATERIAL] {
        return Err("LayerMaterial (key bits 0x4 and 0x20, gated by the template)");
    }
    if set[MaterialFlags::BOOLEAN_42452A12] {
        return Err("boolean 42452A12 (key bit 0x10, gated by the template)");
    }
    Ok(GBUFFER << 22)
}

/// A template the pass bakes: the constant-buffer rows its program reads of the Material's properties, its texture
/// slots, and the properties that must be 0 for the engine's textures not to reach the output.
/// Where a recipe's program reads the terrain: its ambient occlusion texture, the row of the switch mixing it in, and
/// the row of the sediment switch (whose term is out where the sediment layer index is 0).
#[derive(Clone, Copy)]
struct Terrain {
    ao_texture: u32,
    ao_switch: u32,
    sediment_switch: u32,
}

/// What a program's interpolant after `TEXCOORD0` (the UV) is, as the recipe's program reads it; each given the flat
/// surface's value: the world position at the origin, tangent +x, normal +z (with the bitangent sign +1), the eye
/// straight on in tangent space, the clip position's w 1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    WorldPosition,
    Tangent,
    Normal,
    NormalSign,
    TangentView,
    Clip,
    /// The tangent with its bitangent sign +1.
    TangentSign,
    /// The instance index, 0, as the constant interpolant a per-instance-data permutation reads.
    Instance,
}

impl Role {
    fn value(self) -> [f32; 4] {
        match self {
            Role::WorldPosition => [0.0; 4],
            Role::Tangent => [1.0, 0.0, 0.0, 0.0],
            Role::TangentSign => [1.0, 0.0, 0.0, 1.0],
            Role::Instance => [0.0; 4],
            Role::Normal | Role::TangentView => [0.0, 0.0, 1.0, 0.0],
            Role::NormalSign => [0.0, 0.0, 1.0, 1.0],
            Role::Clip => [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// `TEXCOORD1` the world position, 2 the tangent, 3 the normal and its bitangent sign, 4 the clip position.
const WORLD_TANGENT_NORMAL: &[(u32, Role)] = &[
    (1, Role::WorldPosition),
    (2, Role::Tangent),
    (3, Role::NormalSign),
    (4, Role::Clip),
];
/// `TEXCOORD1` the tangent, 2 the normal and its bitangent sign, 3 the clip position.
const TANGENT_NORMAL: &[(u32, Role)] = &[(1, Role::Tangent), (2, Role::NormalSign), (3, Role::Clip)];

/// A paint the program computes, periodic in its UV: the Material's UV transform row, and per axis the row whose
/// value's reciprocal is the period (a whole number of units, else the Material is refused).
#[derive(Clone, Copy)]
struct Pattern {
    uv: u32,
    frequencies: [u32; 2],
}

/// What a recipe's program reads of the light: nothing, the eye's direction (`cb1[2]`) alone, or the sun's
/// (`cb0[4]`) and the eye's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Light {
    None,
    Eye,
    SunAndEye,
}

struct Recipe {
    template: &'static str,
    /// Its interpolants after the UV, by `TEXCOORD` index.
    interpolants: &'static [(u32, Role)],
    /// What its program reads of the sun's direction (`cb0[4]`) and the eye's (`cb1[2]`).
    light: Light,
    pattern: Option<Pattern>,
    /// `cb3` rows read (from the program's disassembly), each of which the template must bind.
    rows: &'static [u32],
    /// Texture slots (`t(75 + slot)`).
    slots: &'static [u32],
    /// Slots whose value reaches the kept outputs only times a row's scalar (`Some(row)`), or not at all (`None`:
    /// only the emission): bound to 0 where the Material sets none and that scalar is 0 (or always, for `None`).
    unkept: &'static [(u32, Option<u32>)],
    /// The engine's textures the program reads, each bound to a value (the terrain's ambient occlusion to
    /// `terrain-ao`).
    engine: &'static [(u32, f32)],
    terrain: Option<Terrain>,
    /// Whether its program reads the camera's position (`cb1[0]`) against the world position (`TEXCOORD1`): run with
    /// the camera `camera-distance` from the surface.
    camera: bool,
    /// Whether its program divides by the object's screen origin's w (`cb2[11].w`, its team stripes): run with it 1.
    object_origin: bool,
    /// The character zones' family (`template` a prefix): its slots are the textures the program samples, its rows the
    /// ones the template binds, its mud, detail array and absent decals as the module documentation states.
    zones: bool,
    /// MAD_CHR_Skin: the permutation with per-instance data, its record built from `skin-tone` (see the module
    /// documentation).
    skin: bool,
    /// The vertex colour (`COLOR0`) the program reads, which a flat surface has one value of.
    vertex_colour: Option<VertexColour>,
    /// The texture slot whose alpha the template's alpha-tested permutations discard by, where the Material tests
    /// alpha.
    coverage: Option<u32>,
}

/// What a recipe states of the vertex colour its program reads: the one value every texel has, and the `cb3` rows whose
/// switches make the program pick by the vertex colour something that one value hides (a Material with a switch on is
/// baked as the value gives and counted, `picked` saying what).
#[derive(Clone, Copy)]
struct VertexColour {
    value: [f32; 4],
    switches: &'static [u32],
    picked: &'static str,
}

/// MAD_Tex1_Rocks (docs/material-spec.md §14.6; its G-buffer program 0x25A2D read): dirt colours at rows 1 to 3,
/// value, hue and saturation at 4 to 6, the terrain switches at 7 and 8, the detail tiling at 9 and 10, the dirt
/// tiling at 12 and 13; the detail normal, detail albedo, object normal, object mask and dirt mask at t75 to t79; the
/// terrain's page table, sediment layer index, ambient occlusion, sediment colours and heights at t13, t14, t16, t19,
/// t20 and t22.
const ROCKS: Recipe = Recipe {
    template: "MAD_Tex1_Rocks",
    unkept: &[],
    interpolants: WORLD_TANGENT_NORMAL,
    light: Light::None,
    pattern: None,
    rows: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13],
    slots: &[0, 1, 2, 3, 4],
    engine: &[(13, 0.0), (14, 0.0), (16, 0.0), (19, 0.0), (20, 0.0), (22, 0.0)],
    terrain: Some(Terrain {
        ao_texture: 19,
        ao_switch: 7,
        sediment_switch: 8,
    }),
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_Tex1_UniqueNormalGradient (§27.3; its G-buffer program read): `Dirt tiling` 1, `Base tiling` 2, the zones'
/// falloff, width and threshold (3, 4, 5; 6, 14, 13; 11, 10, 9) and colours 15, 17, 16, `Micro Tiling` 7, `Detail
/// Blend Range` 8, `Dirt Color Erode` 12, value, hue and saturation 18 to 20; the medium normal and albedo, the OBJ
/// normal and mask, the dirt mask and the micro normal and albedo at t75 to t81; no engine texture; the camera's
/// distance mixing the micro albedo toward the medium.
const UNIQUE_NORMAL_GRADIENT: Recipe = Recipe {
    template: "MAD_Tex1_UniqueNormalGradient",
    unkept: &[],
    interpolants: WORLD_TANGENT_NORMAL,
    light: Light::None,
    pattern: None,
    rows: &[
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ],
    slots: &[0, 1, 2, 3, 4, 5, 6],
    engine: &[],
    terrain: None,
    camera: true,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_Road (§27.4; its G-buffer program read): `2.Metalness` 1, `4.Emissive Scale` 2, `Hue` 3, `Saturate` 4,
/// `Luminosity` 5, `Line Offset` 7, `Line Width` 8, `Line Gap` 9, `Line Frequency` 10, `Lane Width` 11, `Lane Offset`
/// 12, `3.Base tiling` 13 and 14 (`Alpha`, 6, not read by its key); `1.Normal Map` t75, `4.Emissive Mask` t76; the
/// exposure t23, which its emission is divided by (stated as 1); the emissive mask reaches only the
/// emission (its one Material sets none: bound to 0). Its paint's period: one over its
/// line frequency along u, one over its lane width along v.
const ROAD: Recipe = Recipe {
    template: "MAD_Road",
    unkept: &[(1, None)],
    interpolants: TANGENT_NORMAL,
    light: Light::None,
    pattern: Some(Pattern {
        uv: 13,
        frequencies: [10, 11],
    }),
    rows: &[1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14],
    slots: &[0, 1],
    engine: &[(23, 1.0)],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_Tex1_UniqueNormal_DiffuseBlend (§27.8; its G-buffer program read): `Use AO` 1, `3 - DETAILS -Tiling` 2 and
/// 3; `Albedo And Opacity` t75 and `Normal` t76 at the mesh's UV, `Details` (a normal map) t77 and `AO Details` t79 at
/// the tiling, the generic `Texture` t78 at the mesh's UV; layout A. The two masks reach its occlusion only, times
/// `Use AO`.
const UNIQUE_NORMAL_DIFFUSE_BLEND: Recipe = Recipe {
    template: "MAD_Tex1_UniqueNormal_DiffuseBlend",
    unkept: &[(3, Some(1)), (4, Some(1))],
    interpolants: TANGENT_NORMAL,
    light: Light::None,
    pattern: None,
    rows: &[1, 2, 3],
    slots: &[0, 1, 2, 3, 4],
    engine: &[],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_Tex1_Graffiti_Monster (§27.14.2; its G-buffer program read): `Use AO` 1, `EmissivePower` 2, `FresnelOffset` 3,
/// `Fresnelfalloff` 4, `Color - Team` 5, `Color - Custom` 6, `TeamBlendRatio` 7, `3 - DETAILS -Tiling` 8 and 9, `3 -
/// DETAILS - SecondTiling` 11 and 12; `Albedo And Opacity` t75 and `Normal` t76 at the mesh's UV, `Details` (a normal
/// map) t77 at both tilings, `Mask 1` t78 at the mesh's UV and `Details Mask 1` t79 at the first tiling; the exposure
/// t23; the stated eye for its Fresnel emission, the object's screen origin for its team stripes; layout A.
const GRAFFITI_MONSTER: Recipe = Recipe {
    template: "MAD_Tex1_Graffiti_Monster",
    unkept: &[],
    interpolants: TANGENT_NORMAL,
    light: Light::Eye,
    pattern: None,
    rows: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12],
    slots: &[0, 1, 2, 3, 4],
    engine: &[(23, 1.0)],
    terrain: None,
    camera: false,
    object_origin: true,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_Hero_Ice (§27.2; its G-buffer program read): `TilingGlobal` 1, `FresnelFalloff` 2, `LightDirectionFalloff` 3,
/// `ParallaxScale` 4, `TilingFrost` 5, `Frost power` 6, the deep colours 7 and 8, the surface colours 10 and 9;
/// t75 to t81 (`ICE - Diffuse 1`, `Object - Normal`, `ICE - Roughness`, `ICE - Heighmap`, `ICE - Normal`, `ICE -
/// Diffuse`, `ICE - MicroBump`); its eye in tangent space at `TEXCOORD3` for the parallax march; the sun and the eye.
const HERO_ICE: Recipe = Recipe {
    template: "MAD_Hero_Ice",
    unkept: &[],
    interpolants: &[
        (1, Role::Tangent),
        (2, Role::NormalSign),
        (3, Role::TangentView),
        (4, Role::Clip),
    ],
    light: Light::SunAndEye,
    pattern: None,
    rows: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    slots: &[0, 1, 2, 3, 4, 5, 6],
    engine: &[],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_IceBrick (§27.6; its G-buffer program read): Hero_Ice's albedo without its object normal or parallax, rows 1
/// to 11, t75 to t79; its normal at `TEXCOORD1` (no tangent), its eye in tangent space at `TEXCOORD2`; the sun and the
/// eye.
const ICE_BRICK: Recipe = Recipe {
    template: "MAD_IceBrick",
    unkept: &[],
    interpolants: &[(1, Role::Normal), (2, Role::TangentView), (3, Role::Clip)],
    light: Light::SunAndEye,
    pattern: None,
    rows: &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    slots: &[0, 1, 2, 3, 4],
    engine: &[],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_CHR_5Zones_* (§18.2, §19; the G-buffer programs of the 17 templates read): its interpolants after the UV are the
/// tangent, the normal with its bitangent sign and the clip position; the rest is derived from the program and the
/// template ([`Recipe::zones`]).
const FIVE_ZONES: Recipe = Recipe {
    template: "MAD_CHR_5Zones_",
    interpolants: TANGENT_NORMAL,
    light: Light::None,
    pattern: None,
    rows: &[],
    slots: &[],
    unkept: &[],
    engine: &[(23, 1.0)],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: true,
    skin: false,
    vertex_colour: None,
    coverage: None,
};

/// MAD_CHR_Skin (§18.3; its per-instance-data G-buffer program read): `TEXCOORD1` the normal, 2 the tangent with its
/// bitangent sign, 3 the instance index (a constant), 4 the clip position; `Diffuse` t75, `Normal` t76 (its blue the
/// tone mask), `Texture` t77 (the mud normal), the record's buffer t70.
const SKIN: Recipe = Recipe {
    template: "MAD_CHR_Skin",
    interpolants: &[
        (1, Role::Normal),
        (2, Role::TangentSign),
        (3, Role::Instance),
        (4, Role::Clip),
    ],
    light: Light::None,
    pattern: None,
    rows: &[],
    slots: &[0, 1, 2],
    unkept: &[],
    engine: &[],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: true,
    // the vertex occlusion: 1, none
    vertex_colour: Some(VertexColour {
        value: [1.0; 4],
        switches: &[],
        picked: "",
    }),
    coverage: None,
};

/// MAD_CHR_Hair_Anisotropic (§18.5; its G-buffer program read): `TEXCOORD1` the world position, 2 the tangent, 3 the
/// normal and its bitangent sign, 4 the clip position; `COLOR0` red picks the hair zone by band (rows 18, 24 and 25: the
/// smoothness of the bands and the switches of the second and third zone), green is the vertex occlusion; rows 5 to
/// 47 (the distance fade, the zones' root and tip colours, contrasts and offsets, the roughness, the occlusion and
/// translucency scalars); `Mask1` t75 (its blue the zones' ID tint), `Diffuse` t76 (its green the position along the
/// strand the colours blend by, its blue the occlusion) and `Normal` t77 (shading class 2). The specular
/// occlusion (`o2.z`, the camera's distance and the eye's angle), the translucency (`o2.w`) and the strand direction
/// the anisotropic highlight reads (`o1.w`, `o3.x`, `o3.z`) have no place in a Material and are left out. Its cut-out is
/// `Mask1`'s alpha (slot 0), tested in the `0x4000` permutations.
const HAIR_ANISOTROPIC: Recipe = Recipe {
    template: "MAD_CHR_Hair_Anisotropic",
    interpolants: WORLD_TANGENT_NORMAL,
    light: Light::None,
    pattern: None,
    rows: &[
        5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
        31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    ],
    slots: &[0, 1, 2],
    unkept: &[],
    engine: &[],
    terrain: None,
    camera: false,
    object_origin: false,
    zones: false,
    skin: false,
    // red 0: the first zone; green 1: no vertex occlusion
    vertex_colour: Some(VertexColour {
        value: [0.0, 1.0, 1.0, 1.0],
        switches: &[24, 25],
        picked: "the hair zones the mesh's vertex colour (its red) picks: their second or third zone is on, baked as the                  first (red 0)",
    }),
    coverage: Some(0),
};

/// The key bit of the permutations with per-instance data (`t70`).
const INSTANCE_DATA: u32 = 0x8000;

/// The record `t70` holds for a skin of `tone` (see the module documentation): element 0 points at the record, element
/// 3.
fn skin_record(tone: [u8; 3]) -> Arc<[[u32; 4]]> {
    let colour = |c: [u8; 3], low: u8| {
        (u32::from(c[0]) << 24) | (u32::from(c[1]) << 16) | (u32::from(c[2]) << 8) | u32::from(low)
    };
    // w2 is read as a float (the program's visibility test is `0 < w2`) and by its low byte (the translucency): 1.0
    Arc::from([
        [0, 0, 0, 0],
        [0; 4],
        [0; 4],
        [
            colour(tone, 0),
            colour([255; 3], 0),
            1.0f32.to_bits(),
            colour(tone, 255),
        ],
    ])
}

/// The properties of the zones' templates the bake states instead of reading a texture: the mud layer's mask
/// (`FX-MUD_NormalMask`), the generic `Texture` (the engine-bound detail array), the decal and its normal.
const HIDE_DIRT: u32 = 0xB3F7_3583;
const MUD_MASK: u32 = 0xA017_A952;
const DETAIL_ARRAY: u32 = 0x4DDB_34EE;
const DECAL: u32 = 0x7309_AE2D;
const DECAL_NORMAL: u32 = 0x5D0B_0E92;

/// The recipe of a template: an exact name, or a zones family's prefix.
fn recipe_of(template: &str) -> Option<&'static Recipe> {
    RECIPES
        .iter()
        .find(|r| r.template == template || (r.zones && template.starts_with(r.template)))
}

const RECIPES: &[Recipe] = &[
    ROCKS,
    // material-spec §27.4 / verify_m17.py: the same property and texture roles as Rocks;
    // the program itself writes layout A and takes roughness from the detail normal's alpha.
    Recipe {
        template: "MAD_Tex1_UniqueNormal",
        ..ROCKS
    },
    UNIQUE_NORMAL_GRADIENT,
    ROAD,
    UNIQUE_NORMAL_DIFFUSE_BLEND,
    HERO_ICE,
    ICE_BRICK,
    GRAFFITI_MONSTER,
    FIVE_ZONES,
    SKIN,
    HAIR_ANISOTROPIC,
];

/// The pass (see the module documentation). Each shader evaluation runs on the world's stated threads (`core.threads`,
/// a row of quads at a time): the same pixels on any count.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeAnvilMaterial {
    pub size: u32,
    pub normal_green: NormalGreen,
    pub shader_uv: ShaderUv,
    pub mip_bias: f32,
    /// The terrain's ambient occlusion where a Material mixes it in.
    pub terrain_ao: f32,
    /// The sun's elevation above the surface (degrees), and the cosine of the eye's angle to its normal.
    pub sun_elevation: f32,
    pub view_cosine: f32,
    /// How far the camera is from the surface (metres), for a program that mixes by the distance.
    pub camera_distance: f32,
    /// The skin colour (`skin-tone=R,G,B`), where the caller chooses one: without it `MAD_CHR_Skin` Materials are not
    /// baked.
    pub skin_tone: Option<[u8; 3]>,
    /// What a texture's addressing is taken to be: the engine's sampler states aren't read (`unstated-address`).
    pub unstated_address: UnstatedAddress,
}

/// `unstated-address`: what `bake-anvil-material` takes a texture's addressing to be. resolve-anvil-shaders does not
/// read the engine's sampler states, so every binding leaves its addressing unstated; the program states the
/// assumption (reported), or refuses to bake without it. Reading the sampler states is the open item that retires
/// this option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnstatedAddress {
    Refuse,
    Wrap,
    Clamp,
    Mirror,
    MirrorOnce,
}

impl Choice for UnstatedAddress {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("refuse", UnstatedAddress::Refuse),
        ("wrap", UnstatedAddress::Wrap),
        ("clamp", UnstatedAddress::Clamp),
        ("mirror", UnstatedAddress::Mirror),
        ("mirror-once", UnstatedAddress::MirrorOnce),
    ];
}

impl UnstatedAddress {
    pub(super) fn unstated(self) -> Unstated {
        match self {
            UnstatedAddress::Refuse => Unstated::Refuse,
            UnstatedAddress::Wrap => Unstated::Assume(Address::Wrap),
            UnstatedAddress::Clamp => Unstated::Assume(Address::Clamp),
            UnstatedAddress::Mirror => Unstated::Assume(Address::Mirror),
            UnstatedAddress::MirrorOnce => Unstated::Assume(Address::MirrorOnce),
        }
    }
}

impl BakeAnvilMaterial {
    /// From its declared options.
    fn from_given(g: &Given) -> Result<BakeAnvilMaterial, String> {
        let size: u32 = g
            .required("size")
            .parse()
            .map_err(|_| "size is a whole number".to_owned())?;
        if !(2..=4096).contains(&size) || !size.is_multiple_of(2) {
            return Err(format!("size {size}: even, 2 to 4096"));
        }
        let mip_bias: f32 = g
            .required("mip-bias")
            .parse()
            .map_err(|_| "mip-bias is a number".to_owned())?;
        if !mip_bias.is_finite() {
            return Err("mip-bias is finite".to_owned());
        }
        let terrain_ao: f32 = g
            .required("terrain-ao")
            .parse()
            .map_err(|_| "terrain-ao is a number".to_owned())?;
        if !(0.0..=1.0).contains(&terrain_ao) {
            return Err(format!("terrain-ao {terrain_ao}: within 0 and 1"));
        }
        let sun_elevation: f32 = g
            .required("sun-elevation")
            .parse()
            .map_err(|_| "sun-elevation is a number".to_owned())?;
        if !(-90.0..=90.0).contains(&sun_elevation) {
            return Err(format!(
                "sun-elevation {sun_elevation}: within -90 and 90 degrees"
            ));
        }
        let view_cosine: f32 = g
            .required("view-cosine")
            .parse()
            .map_err(|_| "view-cosine is a number".to_owned())?;
        if !(view_cosine > 0.0 && view_cosine <= 1.0) {
            return Err(format!("view-cosine {view_cosine}: above 0, at most 1"));
        }
        let camera_distance: f32 = g
            .required("camera-distance")
            .parse()
            .map_err(|_| "camera-distance is a number".to_owned())?;
        if !(camera_distance.is_finite() && camera_distance > 0.0) {
            return Err(format!("camera-distance {camera_distance}: finite and above 0"));
        }
        let skin_tone = match g.optional("skin-tone") {
            None => None,
            Some(v) => {
                let bytes: Vec<u8> = v
                    .split(',')
                    .map(|c| c.trim().parse::<u8>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("skin-tone={v}: three bytes R,G,B (0 to 255)"))?;
                Some(
                    <[u8; 3]>::try_from(bytes)
                        .map_err(|_| format!("skin-tone={v}: three bytes R,G,B (0 to 255)"))?,
                )
            }
        };
        Ok(BakeAnvilMaterial {
            skin_tone,
            unstated_address: g.choice("unstated-address"),
            size,
            normal_green: g.choice::<Green>("normal-green").normal_green(),
            shader_uv: g.choice("shader-uv"),
            mip_bias,
            terrain_ao,
            sun_elevation,
            view_cosine,
            camera_distance,
        })
    }
}

/// `bake-anvil-material`'s declaration ([`primase_core::pass::PassRegistry::add`]): the bake's size and the view it
/// stands in for are the program's, every option required but `skin-tone`.
pub const BAKE_ANVIL_MATERIAL: PassSpec = PassSpec {
    name: "bake-anvil-material",
    options: &[
        OptionSpec::required(
            "size",
            "512",
            "pixels a side, even, 2 to 4096: the baked textures' size",
        ),
        NORMAL_GREEN,
        SHADER_UV,
        OptionSpec::required("mip-bias", "0", "finite: the bake's sampling mip bias"),
        OptionSpec::required(
            "terrain-ao",
            "0.5",
            "within 0 and 1: the terrain occlusion the bake stands in for",
        ),
        OptionSpec::required(
            "sun-elevation",
            "45",
            "degrees, within -90 and 90: the sun's elevation the bake stands in for",
        ),
        OptionSpec::required(
            "view-cosine",
            "0.7",
            "above 0, at most 1: the view angle's cosine the bake stands in for",
        ),
        OptionSpec::required(
            "camera-distance",
            "1000000",
            "finite and above 0: the camera distance the bake stands in for",
        ),
        UNSET,
        OptionSpec::choice::<UnstatedAddress>(
            "unstated-address",
            "what a texture's addressing is taken to be, the engine's sampler states not being read: refused, or \
             wrap, clamp, mirror or mirror-once (reported)",
        ),
        OptionSpec::optional(
            "skin-tone",
            "200,160,130",
            "R,G,B bytes: the skin colour",
            "MAD_CHR_Skin Materials are not baked",
        ),
    ],
    make: |g| Ok(Box::new(BakeAnvilMaterial::from_given(g)?)),
};

/// A Material's property values at the `cb3` rows its template binds them (a row the template binds nothing at
/// absent): a scalar in `.x`, a vector as stored, a UV transform's 8 floats `su, sv, tu, tv`, speeds, `t`, rotation
/// speed as the rows `(su cos t, -su sin t, tu, 0)`, `(sv sin t, sv cos t, tv, 0)`, `(0, 0, 1, 0)` (docs/material-spec.md
/// §12.2), with zero speeds (the time term isn't known). A bound property neither the Material nor its template (its
/// Operator's default) sets, or a UV transform of another form, is an error naming it.
pub(super) fn material_rows(m: &AnvilMaterial) -> Result<BTreeMap<u32, [f32; 4]>, String> {
    // the Material's property of the binding's key, else the template's default for the binding
    let value = |key: u32, binding: MaterialBinding| {
        m.values
            .iter()
            .find(|(k, v)| *k == key && !matches!(v, MaterialValue::Texture { .. }))
            .map(|(_, v)| v)
            .or_else(|| m.defaults.iter().find(|(b, _)| *b == binding).map(|(_, v)| v))
    };
    let mut rows = BTreeMap::new();
    for &(key, binding) in &m.bindings {
        match binding {
            MaterialBinding::Scalar { vec4 } => match value(key, binding) {
                Some(MaterialValue::Float(x)) => {
                    rows.insert(vec4, [*x, 0.0, 0.0, 0.0]);
                }
                _ => {
                    return Err(format!(
                        "property {key:08X} (a scalar at row {vec4}) not set as a float"
                    ));
                }
            },
            MaterialBinding::Vector { vec4 } => match value(key, binding) {
                Some(MaterialValue::Float4(v)) => {
                    rows.insert(vec4, *v);
                }
                _ => {
                    return Err(format!(
                        "property {key:08X} (a vector at row {vec4}) not set as four floats"
                    ));
                }
            },
            MaterialBinding::UvTransform { vec4 } => match value(key, binding) {
                Some(MaterialValue::Floats(f)) => {
                    let [u, v] = super::material::static_uv_rows(f).map_err(|why| {
                        format!(
                            "UV transform {key:08X}: {}",
                            super::material::uv_motion_notice(f).unwrap_or_else(|| why.to_owned())
                        )
                    })?;
                    rows.insert(vec4, u);
                    rows.insert(vec4.saturating_add(1), v);
                    rows.insert(vec4.saturating_add(2), [0.0, 0.0, 1.0, 0.0]);
                }
                _ => return Err(format!("UV transform {key:08X} (at row {vec4}) not set")),
            },
            MaterialBinding::Texture { .. } | MaterialBinding::Other => {}
        }
    }
    Ok(rows)
}

/// What a bake gives, per texel in the texture's row order.
struct Baked {
    base: Vec<u8>,
    roughness: Vec<u8>,
    normal: Vec<u8>,
    occlusion: Option<Vec<u8>>,
    emission: Option<Vec<u8>>,
    emissive_strength: f32,
    clipped_base: bool,
    /// Whether specular occlusion and translucency are (1, 0) everywhere.
    plain: bool,
    /// Whether some texel is metallic (layout A).
    metal: bool,
    /// Whether the Material mixes in the terrain's ambient occlusion, and its sediments.
    terrain_ao: bool,
    sediment: bool,
    /// Whether the Material has a switch on that its mesh's vertex colour would pick from ([`VertexColour`]).
    vertex_picked: bool,
    /// The alpha the base colour's alpha channel is cut at (the coverage texture's, tested by the Material).
    cutoff: Option<f32>,
    /// A pattern's: the transform from the mesh's UV to its texture.
    transform: Option<UvTransform>,
    /// The textures sampled with the assumed addressing (`unstated-address`).
    assumed_addressing: u64,
}

/// An 8-bit UNORM store of `x` (what the G-buffer target holds).
fn unorm8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round_ties_even() as u8
}

/// Store the shared surface reading's linear emission without clipping its HDR range. Negative light is not PBR.
fn encode_emission(pixels: &[[f32; 3]]) -> Result<(Option<Vec<u8>>, f32), String> {
    if pixels.iter().flatten().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(
            "G-buffer emission is negative or non-finite; no nonnegative PBR emission represents it".into(),
        );
    }
    let peak = pixels.iter().flatten().copied().fold(0.0f32, f32::max);
    if peak == 0.0 {
        return Ok((None, 1.0));
    }
    let strength = peak.max(1.0);
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for rgb in pixels {
        bytes.extend(rgb.map(|v| primase_foundations::color::linear_to_srgb8(f64::from(v / strength))));
        bytes.push(255);
    }
    Ok((Some(bytes), strength))
}

/// The normal the lighting pass decodes from the stored `o1.xyz` (docs/material-spec.md §15.4) and its shading class
/// (`b >> 3`): [`NormalEncoding::OctahedralDiamond`].
fn decode_normal(o1: [f32; 3]) -> ([f64; 3], u8) {
    NormalEncoding::decode_octahedral_diamond(o1)
}

impl BakeAnvilMaterial {
    /// Run `program` for `m` (whose recipe is `recipe`) over the texture square.
    fn bake(&self, w: &World, m: &AnvilMaterial, recipe: &Recipe, program: &[u8]) -> Result<Baked, String> {
        let shader = iridia_exec::ProgramShader::new(iridia_dxbc::lift(program).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut rows = material_rows(m)?;
        if recipe.zones {
            // the mud's switch: HideDirt taken as 1 (a static export has no dirt state)
            for (key, binding) in &m.bindings {
                if *key == HIDE_DIRT
                    && let MaterialBinding::Scalar { vec4 } = binding
                {
                    rows.insert(*vec4, [1.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        if let Some(r) = recipe.rows.iter().find(|r| !rows.contains_key(r)) {
            return Err(format!(
                "its template binds nothing at row {r}, which the program reads"
            ));
        }
        let switched = |row: u32| rows.get(&row).is_some_and(|v| v[0] != 0.0);
        let (terrain_ao, sediment) = recipe.terrain.map_or((false, false), |t| {
            (switched(t.ao_switch), switched(t.sediment_switch))
        });
        // a pattern's period per axis, and its Material's UV transform as stored
        let period = match recipe.pattern {
            None => None,
            Some(p) => {
                let period = p.frequencies.map(|r| rows.get(&r).map_or(0.0, |v| 1.0 / v[0]));
                if !period
                    .iter()
                    .all(|&q| (1.0..=64.0).contains(&q) && q.fract() == 0.0)
                {
                    return Err(format!(
                        "a pattern period {period:?} that isn't a whole number of units (1 to 64)"
                    ));
                }
                let floats = m
                    .bindings
                    .iter()
                    .find(|(_, b)| *b == MaterialBinding::UvTransform { vec4: p.uv })
                    .and_then(|(k, _)| {
                        m.values.iter().find_map(|(key, v)| match v {
                            MaterialValue::Floats(f) if key == k => Some(f.clone()),
                            _ => None,
                        })
                    })
                    .ok_or("its pattern's UV transform isn't set")?;
                Some((p, period, floats))
            }
        };
        for (key, binding) in &m.bindings {
            if period.is_some() {
                break;
            }
            if let MaterialBinding::UvTransform { vec4 } = binding
                && let Some(r) = rows.get(vec4).zip(rows.get(&vec4.saturating_add(1)))
                && ![r.0[0], r.0[1], r.1[0], r.1[1]].iter().all(|s| s.fract() == 0.0)
            {
                return Err(format!(
                    "UV transform {key:08X}: a matrix coefficient that isn't whole (not periodic in the square)"
                ));
            }
        }
        let buffers: Vec<(u32, u32)> = PixelShader::constant_buffers(&shader)
            .iter()
            .map(|&(slot, n)| (slot, n.saturating_mul(4)))
            .collect();
        let mut constants = Constants::new(&Layout {
            buffers: buffers.clone(),
            fields: BTreeMap::new(),
        });
        let cb3_rows = buffers.iter().find(|b| b.0 == 3).map_or(0, |b| b.1 / 4);
        for (&row, v) in rows.range(..cb3_rows) {
            constants
                .set_words(3, row.saturating_mul(4), &v.map(f32::to_bits))
                .map_err(|e| e.to_string())?;
        }
        if recipe.terrain.is_some() {
            // The virtual page table/sediment views are the explicit uniform stand-ins below. Keep their affine
            // coordinates finite even for the UniqueNormal sibling, whose program reads the same engine rows.
            for (row, value) in [(47, [1.0f32, 0.0, 0.0, 0.0]), (48, [1.0, 1.0, 0.0, 0.0])] {
                if buffers
                    .iter()
                    .any(|&(slot, words)| slot == 0 && words >= (row + 1) * 4)
                {
                    constants
                        .set_words(0, row * 4, &value.map(f32::to_bits))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        // a pattern is baked in the shader's own UV: its Material's transform the identity
        if let Some((p, _, _)) = &period {
            for (row, v) in [(p.uv, [1.0f32, 0.0, 0.0, 0.0]), (p.uv + 1, [0.0, 1.0, 0.0, 0.0])] {
                constants
                    .set_words(3, row.saturating_mul(4), &v.map(f32::to_bits))
                    .map_err(|e| e.to_string())?;
            }
        }
        // the object's screen origin (cb2[11]): its w 1, the origin finite
        if recipe.object_origin {
            constants
                .set_words(2, 11 * 4, &[0.0f32, 0.0, 0.0, 1.0].map(f32::to_bits))
                .map_err(|e| e.to_string())?;
        }
        // the engine's mip bias (cb1[41].y)
        constants
            .set_words(1, 41 * 4 + 1, &[self.mip_bias.to_bits()])
            .map_err(|e| e.to_string())?;
        // the sun's direction (cb0[4], toward it) and the eye's (cb1[2]) against the surface's normal +z
        if recipe.light == Light::SunAndEye {
            let (sin, cos) = f64::from(self.sun_elevation).to_radians().sin_cos();
            constants
                .set_words(0, 4 * 4, &[cos as f32, 0.0, sin as f32].map(f32::to_bits))
                .map_err(|e| e.to_string())?;
        }
        if recipe.light != Light::None {
            let c = f64::from(self.view_cosine);
            constants
                .set_words(
                    1,
                    2 * 4,
                    &[(1.0 - c * c).sqrt() as f32, 0.0, c as f32].map(f32::to_bits),
                )
                .map_err(|e| e.to_string())?;
        }
        // the camera's position (cb1[0]), `camera-distance` along the normal from the surface (at the origin)
        if recipe.camera {
            constants
                .set_words(1, 0, &[0.0, 0.0, self.camera_distance].map(f32::to_bits))
                .map_err(|e| e.to_string())?;
        }
        let mut textures = BTreeMap::new();
        let mut coverage_image: Option<Arc<SampledTexture>> = None;
        let mut assumed_addressing = 0u64;
        for &(slot, value) in recipe.engine {
            let ao = recipe.terrain.is_some_and(|t| t.ao_texture == slot);
            let value = if ao { self.terrain_ao } else { value };
            textures.insert(slot, iridia_exec::View::Constant([value; 4]));
        }
        let slots: Vec<u32> = if recipe.zones {
            PixelShader::textures(&shader)
                .iter()
                .filter_map(|t| t.checked_sub(75))
                .collect()
        } else {
            recipe.slots.to_vec()
        };
        for slot in slots {
            // the texture the binding resolves to (the Material's record, its TextureSet's slot, or the template's
            // default)
            let bound = m
                .textures
                .iter()
                .find(|t| t.shader_slot == slot)
                .ok_or_else(|| format!("its template binds no texture at t{}", 75 + slot))?;
            let set = bound.texture;
            if recipe.zones {
                // what the engine supplies and a static bake can't: the mud's mask 0 (no dirt), the detail array a
                // flat detail normal, an absent decal none (alpha 0) and an absent decal normal the flat normal
                let fixed = match (bound.key, set) {
                    (MUD_MASK, _) => Some([0.0; 4]),
                    (DETAIL_ARRAY, None) => Some([0.5, 0.5, 1.0, 1.0]),
                    (DECAL, None) => Some([0.0; 4]),
                    (DECAL_NORMAL, None) => Some([0.5, 0.5, 1.0, 1.0]),
                    _ => None,
                };
                if let Some(v) = fixed {
                    textures.insert(75 + slot, iridia_exec::View::Constant(v));
                    continue;
                }
            }
            let e = match set {
                Some(e) => e,
                None if recipe.unkept.iter().any(|&(s, row)| {
                    s == slot && row.is_none_or(|r| rows.get(&r).is_some_and(|v| v[0] == 0.0))
                }) =>
                {
                    textures.insert(75 + slot, iridia_exec::View::Constant([0.0; 4]));
                    continue;
                }
                // resolved to none: white, as `unset=white` states
                None => {
                    textures.insert(75 + slot, iridia_exec::View::Constant([1.0; 4]));
                    continue;
                }
            };
            let t = w
                .get::<Texture>(e)
                .ok_or("a bound texture holds no core texture")?;
            let srgb = super::material::srgb(w, e);
            // the binding states no addressing: the program's assumption, or a refusal
            let unread = SamplerState::default();
            let image = SampledTexture::new(
                primase_image::levels(t).map_err(|e| e.to_string())?,
                if srgb {
                    Transfer::SrgbColour
                } else {
                    Transfer::Stored
                },
                SamplerUse::of(&unread, self.unstated_address.unstated())?,
                Filtering::Trilinear,
            )?;
            assumed_addressing += u64::from(SamplerUse::assumed(&unread));
            let image = Arc::new(image);
            if recipe.coverage == Some(slot) {
                coverage_image = Some(image.clone());
            }
            textures.insert(75 + slot, iridia_exec::View::Image(image));
        }
        // the cut-out: the coverage texture's alpha against the Material's `AlphaTestValue`, where it tests alpha
        // (the discard of the alpha-tested permutations: `alpha - reference < 0`)
        let cutoff = recipe
            .coverage
            .and(coverage_image.as_ref())
            .filter(|_| {
                m.flags.blend_mode == 0
                    && m.flags.alpha_test_value > 0
                    && m.flags.booleans.get(MaterialFlags::ALPHA_TEST_ENABLED) == Some(&1)
            })
            .map(|_| f32::from(m.flags.alpha_test_value) / 255.0);
        if recipe.skin {
            let tone = self.skin_tone.ok_or("no skin-tone")?;
            textures.insert(70, iridia_exec::View::Typed(skin_record(tone)));
        }
        let constant = |v: [f32; 4]| Input::Constant(v);
        let mut inputs = BTreeMap::from([
            (("SV_Position".to_owned(), 0), constant([0.0, 0.0, 0.0, 1.0])),
            (("TEXCOORD".to_owned(), 0), Input::GridUv),
        ]);
        for &(index, role) in recipe.interpolants {
            inputs.insert(("TEXCOORD".to_owned(), index), constant(role.value()));
        }
        if let Some(colour) = recipe.vertex_colour {
            inputs.insert(("COLOR".to_owned(), 0), constant(colour.value));
        }
        let textures = iridia_exec::Views::refusing(textures);
        let raw = evaluate_raw_with(
            primase_core::parallel::threads(w),
            &shader,
            &constants,
            &textures,
            &inputs,
            Grid {
                size: self.size,
                period: period.as_ref().map_or([1.0; 2], |(_, q, _)| *q),
            },
            &[(0, 15), (1, 15), (2, 15), (3, 15)],
            Invocation::Quads,
        )
        .map_err(|e| e.to_string())?;
        let n = self.size as usize;
        let mut baked = Baked {
            base: Vec::with_capacity(n * n * 4),
            roughness: Vec::with_capacity(n * n * 4),
            normal: Vec::with_capacity(n * n * 4),
            occlusion: None,
            emission: None,
            emissive_strength: 1.0,
            clipped_base: false,
            plain: true,
            metal: false,
            terrain_ao,
            sediment,
            cutoff,
            vertex_picked: recipe
                .vertex_colour
                .is_some_and(|c| c.switches.iter().any(|r| switched(*r))),
            transform: None,
            assumed_addressing,
        };
        if let Some((_, q, f)) = &period {
            // the Material's scales and translations over the period: the texture is one period
            let mut f = f.clone();
            for (k, q) in [(0, q[0]), (1, q[1]), (2, q[0]), (3, q[1])] {
                if let Some(v) = f.get_mut(k) {
                    *v = (f64::from(*v) / f64::from(q)) as f32;
                }
            }
            baked.transform = Some(
                uv_transform(&f, self.shader_uv)
                    .map_err(|why| format!("its pattern's UV transform: {why}"))?
                    .unwrap_or(UvTransform::IDENTITY),
            );
        }
        let mut occlusion = Vec::with_capacity(n * n * 4);
        // Most map programs emit nothing: allocate the temporary HDR image only on the first nonzero texel.
        let mut emission = Vec::new();
        let mut readings = BTreeMap::new();
        for y in 0..n {
            // the texture's row y is the shader's v = (y + 0.5) / n, or 1 minus it
            // (a pattern's rows are in its own UV, which its transform maps from the mesh's)
            let row = match (self.shader_uv, &period) {
                (ShaderUv::OneMinusV, None) => n - 1 - y,
                _ => y,
            };
            for x in 0..n {
                let p = row * n + x;
                let [o0, o1, o2, o3] = [0, 1, 2, 3].map(|t| raw.pixels[t][p]);
                if [o0.1, o1.1, o2.1] != [15; 3] {
                    return Err("the program leaves a G-buffer target unwritten".into());
                }
                let o3_written = o3.1 == 15;
                let [o0, o1, o2, o3] = [o0.0, o1.0, o2.0, o3.0].map(|v| v.map(f32::from_bits));
                if !o0.iter().chain(&o1).all(|v| v.is_finite()) {
                    return Err("the program writes a value that isn't finite".into());
                }
                let (normal, class) = decode_normal([o1[0], o1[1], o1[2]]);
                if let Some(expected) = super::resolve_shaders::shading_class(recipe.template)
                    .filter(|_| !recipe.template.starts_with("MAD_CHR_"))
                    && class != expected
                {
                    return Err(format!(
                        "{} program writes shading class {class}, output contract states {expected}",
                        recipe.template
                    ));
                }
                let texel = match class {
                    1 | 3..=5 => {
                        if let std::collections::btree_map::Entry::Vacant(entry) = readings.entry(class) {
                            entry.insert(super::gbuffer::Reading::new(shader.interface(), class)?);
                        }
                        readings
                            .get(&class)
                            .ok_or("missing surface reading")?
                            .read(|target| {
                                raw.pixels
                                    .get(target)
                                    .and_then(|t| t.get(p))
                                    .map(|(v, mask)| (v.map(f32::from_bits), *mask))
                            })?
                    }
                    // the lighting pass's class 2 (hair): the albedo, roughness and normal as the others, F0 fixed (no
                    // metalness), the occlusion where layout A reads it from `o3.x` read from `o3.y`, the specular
                    // occlusion and translucency of layout B (`o2.z`, `o2.w`)
                    2 if o3_written => super::gbuffer::Texel {
                        base: [o0[0], o0[1], o0[2]],
                        normal: normal.map(|v| v as f32),
                        roughness: o0[3],
                        metallic: 0.0,
                        occlusion: o3[1],
                        specular_occlusion: o2[2],
                        translucency: o2[3],
                        emission: [0.0; 3],
                    },
                    _ => {
                        return Err(format!(
                            "shading class {class}, not layout A's (1, four targets), the hair's (2) or B's (3 to 5)"
                        ));
                    }
                };
                baked.clipped_base |= texel.base.iter().any(|v| !(0.0..=1.0).contains(v));
                for c in &texel.base {
                    baked.base.push(
                        (primase_foundations::color::linear_to_srgb(f64::from(c.clamp(0.0, 1.0))) * 255.0)
                            .round() as u8,
                    );
                }
                baked.base.push(match (&coverage_image, cutoff) {
                    (Some(image), Some(_)) => {
                        let n = self.size as f32;
                        let step = 1.0 / n;
                        let at = [(x as f32 + 0.5) / n, (row as f32 + 0.5) / n];
                        let lod = Lod::GradientsBias([[step, 0.0], [0.0, step]], self.mip_bias);
                        unorm8(iridia_exec::Image::sample(&**image, at, lod)?[3])
                    }
                    _ => 255,
                });
                let metal = unorm8(texel.metallic);
                baked.metal |= metal != 0;
                baked.roughness.extend([255, unorm8(texel.roughness), metal, 255]);
                baked
                    .normal
                    .extend(texel.normal.map(|c| ((c * 0.5 + 0.5) * 255.0).round() as u8));
                baked.normal.push(255);
                let ao = unorm8(texel.occlusion);
                occlusion.extend([ao, ao, ao, 255]);
                if texel.emission != [0.0; 3] || !emission.is_empty() {
                    emission.resize(y * n + x, [0.0; 3]);
                    emission.push(texel.emission);
                }
                baked.plain &= texel.specular_occlusion == 1.0 && texel.translucency == 0.0;
            }
        }
        if occlusion.as_chunks::<4>().0.iter().any(|t| t[0] != 255) {
            baked.occlusion = Some(occlusion);
        }
        (baked.emission, baked.emissive_strength) = encode_emission(&emission)?;
        Ok(baked)
    }
}

/// A linear or sRGB RGBA8 texture of one level.
fn texture(
    size: u32,
    pixels: Vec<u8>,
    space: ColorSpace,
    role: TexRole,
    normal: Option<NormalConvention>,
) -> Texture {
    Texture {
        width: size,
        height: size,
        format: TexFormat::Rgba8Unorm,
        levels: vec![pixels],
        color_space: space,
        role,
        normal,
    }
}

impl Pass for BakeAnvilMaterial {
    fn name(&self) -> &'static str {
        "bake-anvil-material"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("size".to_owned(), self.size.to_string()),
            (
                "normal-green".to_owned(),
                Green::of(self.normal_green).text().to_owned(),
            ),
            ("shader-uv".to_owned(), self.shader_uv.text().to_owned()),
            ("mip-bias".to_owned(), self.mip_bias.to_string()),
            ("terrain-ao".to_owned(), self.terrain_ao.to_string()),
            ("sun-elevation".to_owned(), self.sun_elevation.to_string()),
            ("view-cosine".to_owned(), self.view_cosine.to_string()),
            ("camera-distance".to_owned(), self.camera_distance.to_string()),
            ("unset".to_owned(), Unset::White.text().to_owned()),
            (
                "unstated-address".to_owned(),
                self.unstated_address.text().to_owned(),
            ),
        ]);
        if let Some([r, g, b]) = self.skin_tone {
            o.insert("skin-tone".to_owned(), format!("{r},{g},{b}"));
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[
            PbrMaterial::NAME,
            Texture::NAME,
            VertexColor::NAME,
            crate::components::AnvilRenderStatus::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "Materials baked from their G-buffer shader at a stated size: the bake's own trilinear sampling at the stated \
         addressing, a plain draw, 8-bit outputs"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<Entity> = w
            .iter::<AnvilMaterial>()
            .filter(|(_, m)| {
                m.template_name
                    .as_deref()
                    .and_then(recipe_of)
                    .is_some_and(|r| !r.skin || self.skin_tone.is_some())
            })
            .map(|(e, _)| e)
            .collect();
        let normal = NormalConvention {
            green: self.normal_green,
            z: NormalZ::Stored,
            x: NormalX::Red,
            basis: NormalBasis::TangentBitangent,
        };
        for e in materials {
            let m = w.get::<AnvilMaterial>(e).ok_or("listed above")?;
            let recipe = m
                .template_name
                .as_deref()
                .and_then(recipe_of)
                .ok_or("listed above")?;
            let template = m.template_name.clone().unwrap_or_default();
            let program = match (
                gbuffer_pixel_key(&m.flags).map(|k| if recipe.skin { k | INSTANCE_DATA } else { k }),
                m.shaders,
            ) {
                (Err(why), _) => Err(why.to_owned()),
                (Ok(_), None) => Err("its template's shaders weren't read (Placer::with_shaders)".to_owned()),
                (Ok(key), Some(s)) => w
                    .get::<AnvilShaders>(s)
                    .and_then(|s| s.pixel(key))
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| format!("its template has no pixel permutation {key:#x}")),
            };
            let baked = program.and_then(|p| self.bake(w, m, recipe, &p));
            let baked = match baked {
                Ok(b) => b,
                Err(why) => {
                    let source = format!("Material {:#x}", m.index_id);
                    w.insert(
                        e,
                        crate::components::AnvilRenderStatus {
                            source,
                            template: Some(template.clone()),
                            unsupported: true,
                            reason: format!("{template}: own-program bake refused: {why}"),
                        },
                    );
                    log.omit(
                        &format!(
                            "{} Materials not baked (left to lower-anvil-material): {why}",
                            template
                        ),
                        1,
                    );
                    continue;
                }
            };
            w.remove::<AnvilMaterial>(e);
            let spawn = |w: &mut World, t: Texture| {
                let t_e = w.spawn();
                w.insert(t_e, t);
                t_e
            };
            let s = self.size;
            let base = spawn(w, texture(s, baked.base, ColorSpace::Srgb, TexRole::Color, None));
            let rough = spawn(
                w,
                texture(s, baked.roughness, ColorSpace::Linear, TexRole::Mask, None),
            );
            let normal_map = spawn(
                w,
                texture(s, baked.normal, ColorSpace::Linear, TexRole::Normal, Some(normal)),
            );
            let occlusion = baked
                .occlusion
                .map(|o| spawn(w, texture(s, o, ColorSpace::Linear, TexRole::Mask, None)));
            if occlusion.is_some() {
                log.approximate_on(
                    e,
                    &format!("{template}: {}", super::gbuffer::OCCLUSION_APPROXIMATION),
                    1,
                );
            }
            if baked.clipped_base {
                log.approximate_on(e, &format!("{template}: program albedo outside 0 to 1 clipped by RGBA8 PBR base colour; HDR emission is retained separately"), 1);
            }
            let emissive = baked
                .emission
                .map(|o| spawn(w, texture(s, o, ColorSpace::Srgb, TexRole::Color, None)));
            if baked.assumed_addressing > 0 {
                log.approximate_on(e,
                    &format!(
                        "{} Materials' textures addressed as unstated-address={} states (the engine's sampler \
                         states are not read)",
                        template,
                        self.unstated_address.text()
                    ),
                    baked.assumed_addressing,
                );
            }
            if baked.terrain_ao {
                log.approximate_on(
                    e,
                    &format!(
                        "{} Materials mixing in the terrain's ambient occlusion: the stated terrain-ao",
                        template
                    ),
                    1,
                );
            }
            if baked.sediment {
                log.approximate_on(
                    e,
                    &format!(
                        "{} Materials mixing in the terrain's sediments: none (sediment layer 0)",
                        template
                    ),
                    1,
                );
            }
            if !baked.plain {
                log.omit(
                    &format!(
                        "{template} Materials' specular occlusion or translucency (not (1, 0) everywhere; core.pbr has \
                         no place for them): left out"
                    ),
                    1,
                );
            }
            let sampled = |t: Entity| TextureRef {
                transform: baked.transform,
                ..TextureRef::new(t)
            };
            w.insert(
                e,
                PbrMaterial {
                    base_color: [1.0; 4],
                    base_color_texture: Some(sampled(base)),
                    metallic: if baked.metal { 1.0 } else { 0.0 },
                    roughness: 1.0,
                    metallic_roughness_texture: Some(sampled(rough)),
                    occlusion_texture: occlusion.map(sampled),
                    occlusion_strength: 1.0,
                    normal_texture: Some(sampled(normal_map)),
                    emissive: if emissive.is_some() { [1.0; 3] } else { [0.0; 3] },
                    emissive_texture: emissive.map(sampled),
                    emissive_strength: baked.emissive_strength,
                    alpha_mode: baked
                        .cutoff
                        .map_or(AlphaMode::Opaque, |cutoff| AlphaMode::Mask { cutoff }),
                    double_sided: false,
                    ..PbrMaterial::default()
                },
            );
            w.insert(e, VertexColor::Ignored);
            w.insert(e, crate::components::AnvilRenderStatus {
                source: "bake-anvil-material".into(), template: Some(template.clone()), unsupported: false,
                reason: format!("{template}: own G-buffer program baked at {} x {}; engine inputs and sampling choices are recorded in the pass options and approximation log", self.size, self.size),
            });
            // The bake is itself the approximation a viewer must show for this material, its text the pass's own
            // declaration made specific to this run's size (reported on the material).
            log.approximate_on(
                e,
                &format!(
                    "{template}: program baked from its G-buffer shader at {} px: trilinear sampling at the stated \
                     addressing, a plain draw, 8-bit outputs",
                    self.size
                ),
                1,
            );
            log.count(&format!("{} Materials baked", template), 1);
            if baked.cutoff.is_some() {
                log.count(&format!("{template} Materials baked with their alpha cut-out"), 1);
            }
            if recipe.zones {
                log.approximate_on(e,
                    "MAD_CHR_5Zones Materials baked with the mud layer left out (HideDirt 1, a runtime dirt state), the engine-bound detail array flat and no decal where the Material binds none",
                    1,
                );
            }
            if recipe.skin {
                log.approximate_on(e,
                    "MAD_CHR_Skin Materials baked with the chosen skin-tone as the tint where the tone mask is high and the engine's record otherwise stated (white where it is low, the facial hair the tone, exponent 1, no metalness or translucency byte, no vertex occlusion): the engine's own conversion from a tone to the record is not read",
                    1,
                );
            }
            if baked.vertex_picked
                && let Some(colour) = recipe.vertex_colour
            {
                log.approximate_on(e, &format!("{template} Materials with {}", colour.picked), 1);
            }
            if recipe.camera {
                log.approximate_on(e,
                    &format!(
                        "Materials baked with the camera at the stated camera-distance {} m (their mix by distance)",
                        self.camera_distance
                    ),
                    1,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Stored;
    use anvil_formats::template::ShaderTables;

    fn flags() -> MaterialFlags {
        MaterialFlags {
            blend_mode: 0,
            alpha_display_mode: 0,
            alpha_test_value: 0,
            booleans: [0; 35],
        }
    }

    /// A Material setting none of the gated flags draws the G-buffer's plain key; TwoSided alone doesn't enter the
    /// key, with FlipNormalOnBackFace it does (refused: the template's gate isn't read); so do LayerMaterial,
    /// AlphaDisplayMode 2 and boolean 34; a boolean byte other than 0 or 1 is refused.
    #[test]
    fn the_gbuffer_key_of_a_plain_draw() {
        assert_eq!(gbuffer_pixel_key(&flags()), Ok(0x40_0000));
        let mut f = flags();
        f.booleans[MaterialFlags::TWO_SIDED] = 1;
        f.booleans[77 - 50] = 1;
        f.alpha_display_mode = 1;
        assert_eq!(gbuffer_pixel_key(&f), Ok(0x40_0000));
        f.booleans[MaterialFlags::FLIP_NORMAL_ON_BACK_FACE] = 1;
        assert!(gbuffer_pixel_key(&f).is_err());
        for set in [
            |f: &mut MaterialFlags| f.booleans[MaterialFlags::LAYER_MATERIAL] = 1,
            |f: &mut MaterialFlags| f.booleans[MaterialFlags::BOOLEAN_42452A12] = 1,
            |f: &mut MaterialFlags| f.alpha_display_mode = 2,
            |f: &mut MaterialFlags| f.booleans[0] = 2,
            |f: &mut MaterialFlags| f.blend_mode = 16,
        ] {
            let mut f = flags();
            set(&mut f);
            assert!(gbuffer_pixel_key(&f).is_err(), "{f:?}");
        }
    }

    /// The decode is core's (`core::shader::binding`'s unit test holds it to the encoding the shader's last 20
    /// instructions make, worked by hand): +z with class 5 stores `[255, 255, 5 << 3 | 1]`.
    #[test]
    fn the_normal_decode_is_the_octahedral_diamond_decode() {
        let (n, class) = decode_normal([255, 255, 5 << 3 | 1].map(|v| v as f32 / 255.0));
        assert_eq!((n, class), ([0.0, 0.0, 1.0], 5));
    }

    /// The existing Road program writes HDR emission when its material supplies an emissive mask. Compare the
    /// material actually installed by the pass with the common surface reading of that same program and inputs.
    #[test]
    fn road_lowering_retains_the_surface_readings_hdr_emission() {
        use super::super::gbuffer::{Reading, fixture};
        use crate::components::{BoundTexture, TextureFrom};
        let Some(program) = fixture::program("MAD_Road") else {
            return;
        };
        let shader = iridia_exec::ProgramShader::new(iridia_dxbc::lift(&program).unwrap()).unwrap();
        let mut world = World::new();
        let mut textures = Vec::new();
        let bytes = [[128, 128, 153, 100], [128, 128, 128, 128]];
        for (slot, rgba) in bytes.into_iter().enumerate() {
            let e = world.spawn();
            world.insert(
                e,
                texture(1, rgba.to_vec(), ColorSpace::Linear, TexRole::Mask, None),
            );
            textures.push(BoundTexture {
                key: slot as u32,
                shader_slot: slot as u32,
                parameter: None,
                texture: Some(e),
                from: TextureFrom::Record,
            });
        }
        let mut material = AnvilMaterial {
            index_id: 1,
            template: None,
            template_name: Some("MAD_Road".into()),
            bindings: Vec::new(),
            values: Vec::new(),
            texture_set: Vec::new(),
            textures,
            defaults: Vec::new(),
            frame: primase_core::frame::SourceFrame::WORLD,
            flags: flags(),
            shaders: None,
        };
        for &row in ROAD.rows.iter().filter(|&&r| r < 13) {
            let value = match row {
                1 => 0.6,
                2 => 8.0,
                5 | 10 | 11 => 1.0,
                8 => 0.2,
                9 => 0.5,
                _ => 0.0,
            };
            material
                .bindings
                .push((row, MaterialBinding::Scalar { vec4: row }));
            material.values.push((row, MaterialValue::Float(value)));
        }
        material
            .bindings
            .push((13, MaterialBinding::UvTransform { vec4: 13 }));
        material.values.push((
            13,
            MaterialValue::Floats(vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ));
        let mut rows = vec![[0.0; 4]; 16];
        for (row, value) in material_rows(&material).unwrap() {
            rows[row as usize] = value;
        }
        let sampled = bytes.map(|b| b.map(|v| f32::from(v) / 255.0));
        let output = fixture::execute(
            &shader,
            &rows,
            &[(75, sampled[0]), (76, sampled[1]), (23, [1.0; 4])],
        );
        let expected = Reading::new(shader.interface(), 1)
            .unwrap()
            .read(|target| {
                output
                    .outputs
                    .get(target)
                    .zip(output.output_masks.get(target))
                    .map(|(v, m)| (v.map(f32::from_bits), *m))
            })
            .unwrap();
        assert!(
            expected.emission.iter().any(|v| *v > 1.0),
            "fixture must discriminate dropped/clipped emission"
        );
        let shaders = world.spawn();
        world.insert(
            shaders,
            Stored(ShaderTables {
                pixel: vec![(0x40_0000, 0)],
                vertex: Vec::new(),
                programs: vec![program],
                feature_node: Vec::new(),
            }),
        );
        material.shaders = Some(shaders);
        let e = world.spawn();
        world.insert(e, material);
        let pass = BakeAnvilMaterial {
            size: 2,
            normal_green: NormalGreen::Up,
            shader_uv: ShaderUv::TexCoord,
            mip_bias: 0.0,
            terrain_ao: 0.5,
            sun_elevation: 45.0,
            view_cosine: 0.7,
            camera_distance: 1_000_000.0,
            skin_tone: None,
            unstated_address: UnstatedAddress::Wrap,
        };
        let mut log = PassLog::default();
        pass.run(&mut world, &mut log).unwrap();
        let pbr = world.get::<PbrMaterial>(e).unwrap_or_else(|| panic!("{log:?}"));
        let emissive = world
            .get::<Texture>(pbr.emissive_texture.as_ref().expect("emission map").texture)
            .unwrap();
        assert_eq!(emissive.color_space, ColorSpace::Srgb);
        for pixel in emissive.levels[0].as_chunks::<4>().0 {
            for (k, ((&channel, &factor), &expected)) in pixel[..3]
                .iter()
                .zip(pbr.emissive.iter())
                .zip(expected.emission.iter())
                .enumerate()
            {
                let got = primase_foundations::color::srgb8_to_linear(channel) as f32
                    * factor
                    * pbr.emissive_strength;
                assert!(
                    (got - expected).abs() <= 0.009 * pbr.emissive_strength,
                    "emission lane {k}: {got}, surface {expected}"
                );
            }
        }
        let orm = world
            .get::<Texture>(pbr.metallic_roughness_texture.as_ref().unwrap().texture)
            .unwrap();
        assert_eq!(orm.levels[0][1], unorm8(expected.roughness));
        assert_eq!(orm.levels[0][2], unorm8(expected.metallic));
    }
}
