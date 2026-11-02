//! `lower-forzatech-materials`: a Forza Horizon 6 material lowered to `pbr.material` on its own entity, one rule per
//! shader family, each stated from what the family's compiled pixel shader does with the material's textures and
//! constants. A family whose rule is not established is left as it is (no `pbr.material`), counted by family with the
//! triangles it shades; a rule's own simplifications are named and counted per material and per triangle, never silent.
//!
//! The shaders are bindless: a material's parameters are rows of one constant buffer, the textures are read by the
//! indices a parameter row holds, and the families' `CBMP` and `TXMP` maps say which row, and so which parameter name,
//! each load is. Reading the GBuffer pixel shaders of every permutation through those maps gives what each render
//! target channel depends on; the arithmetic of the `_Standard` permutation was read out by hand beside it.
//!
//! # Layout
//!
//! This file holds the pass, its dispatch by shader family and what every rule shares: `Params` (a material's resolved
//! parameters by name), `Binding`, `Lowering` (`slot`, `derive`, `normal_map`) and the counted-approximation helpers
//! (`approximate`, `left_out`). Each family's rule is one file, `impl Lowering { pub(super) fn <family> }`, whose module
//! docs are the evidence for the rule: what each render target channel of the family's pixel shader depends on, and
//! what the rule does with it. A new family adds a file, a `mod` line and a match arm in `run`, and its counts to the
//! section below.
//!
//! # Over the Shibuya Scramble box
//!
//! `tests/shibuya_materials.rs` runs the pass on every 20th prototype of the box (x -1550..-1100, z -5000..-4566.67,
//! summer: 77 prototypes, 90,430 shaded triangles). Lowered (materials, triangles): `environment` 758, 55,464 (61.3%);
//! `projection_lod` 59, 10,002; `uber_foliage` 18, 13,674; `uber_branch` 6, 8,213; `environmentdeferreddecal` 70, 1,549;
//! `uber_roaddecal` 2, 146; `pylon_cables` 2, 532; `environment_signage` 88, 520; `glass` 8, 188;
//! `window_linecastroom` 51, 78; `emissiveonly` 2, 64: every one of the box's 90,430 triangles. Each lowered material passes `pbr.material`'s own verification, and the decal families alone
//! have a draw order (the decal layer, no depth write).
//!
//! The `environment` rule packs 85 distinct RoughMetAoPor maps and makes normal maps of 83 distinct textures. The packed maps'
//! metallic channel holds an extreme (below 16 or above 239) in 97.97% of its texels over the lowered materials' maps,
//! the binary mask a metalness is. Left out by name (materials, triangles; a material may carry several): modulate 594, 8,747; deposits 554, 7,159; triplanar 568, 7,642; wetness 594, 11,139; relaxed cone-step
//! mapping 556, 5,596; mega-skirt 20, 13,188; emission 15, 1,636; retro-reflection 8, 1,032; per-instance random UV
//! offset 77, 1,086; the second UV set of a dual-UV material 95, 166; and the alpha-test reference (a per-draw
//! constant, not in the material: the middle of the range) 29, 1,357.
//!
//! The other rules make 6 roughness maps and 4 occlusion maps (`uber_foliage` and `uber_branch`: 2 from an alpha
//! channel) and 37 roughness-metalness maps (`projection_lod`). Left out by name: `uber_foliage` canopy normal 14,
//! 11,808, tint 3, 4,946, world tint 3, 5,498, wind animation 16, 13,670; `uber_branch` mega-skirt 6, 8,213, wind
//! animation 6, 8,213; `projection_lod` baked occlusion 59, 10,002, swatch tint 59, 10,002, emission 44, 9,820, the
//! alpha-test reference 4, 1,636, and one material (72 triangles) with no normal map.
//!
//! The three decal rules (`uber_roaddecal`, `uber_roaddecalretro`, `environmentdeferreddecal`) over the whole box
//! (`STEP=1`): `uber_roaddecal` 291 materials, 20,743 triangles; `uber_roaddecalretro` 130, 46,996;
//! `environmentdeferreddecal` 1,438, 38,263; none of the three is left unlowered (the projected permutation, which no
//! box material has, is). They make 64 colour maps (22 resample the mask to the diffuse's size) and 9 roughness-metalness
//! maps (5 resample), each shared by the materials that name the same textures. Left out by name (materials, triangles; a
//! material may carry several): the dithered alpha test taken as a plain test 418, 67,509; far fade and wetness 421,
//! 67,739; the vertex colour's blue tint 267, 52,196, red modulate 292, 52,407 and green fade 152, 50,968; the
//! world-space alpha mask 149, 50,344; parallax occlusion mapping 80, 2,393; retro-reflection and the retro distance
//! fade 130, 46,996; a road decal with `Enabled` off, drawn 1,161 triangles; and for the deferred decals the blend and
//! roughness reading 1,438, 38,263 (the decal buffer's resolve is in no file) and relaxed cone-step mapping 100
//! triangles.
//!
//! The surface families (`environment_signage`, `glass`, `window_linecastroom`, `pylon_cables`, `emissiveonly`, and in
//! the whole box `anim_flag`, `anim_ledflipbookpanel` and `diff1`) each document their evidence and what they leave out in
//! their own module. Every family the box holds has a rule (the `uber_foliage` impostor, 1 material and 2
//! triangles, is drawn as one fixed horizon frame of its atlas of views, named). The normal maps are made in `NormalZ::Rebuild`:
//! `normalize-normals` converts them to glTF's before a writer.
//!
//! The windows' room is drawn as one colour: the mean of the back wall of the atlas cells a material's placements use,
//! composited into its frame texture where the frame's mask leaves the pane empty (one baked texture per frame and room
//! colour), and the room's emission, where it is not black, as an emission map with the same mask; each is counted per
//! material and triangle (`window_linecastroom` holds the rest).
//!
//! A normal map in a signed format (BC5 SNORM) holds the shader's `v`, whose normal is `2 v - 1`. Over the whole box
//! (`STEP=1`) four such textures are read as normal maps, and none has a negative texel: a 32 x 32 default whose every
//! texel is the neutral normal (bytes 191 and 192, `|2 v - 1|` at most 0.012), a 16 x 16 default with bytes 184 to 198
//! (up to 0.11) and a 128 x 128 one with bytes 151 to 231 (up to 0.63), and a terrain map no lowered slot reads. A map whose
//! every texel has `|2 v - 1|` within 0.02 (about 1.1 degrees) is flat, so the material takes no normal map: 778 slots,
//! counted as `flat default normal map: no normal map` and, per material and triangle, as an approximation that names the
//! map's largest tilt (0.012 for the 32 x 32 default: the dropped map is not exactly flat). The bound is a little over two
//! steps of the 8-bit byte the map would be re-encoded to (a step is 2 / 255 = 0.0078 of a component), and the next map up the
//! observed tilts is 0.11, nine times the default's. Any other is re-encoded as the
//! unsigned map glTF reads (the byte is `v`, so glTF's `2 b - 1` is the shader's `2 v - 1`; the rebuilt `z` in blue; a
//! negative `v` is clamped and counted by texel): 1 texture. `normal map not lowered` now means a texture with no
//! pixels, and the box has none.
//!
//! A base colour factor above 1 (the flipbook panel's brightness of 10 in the whole box: 8 materials, 16 triangles) is
//! stored as 1 and counted: no shader of the surface families saturates the albedo it writes, and the albedo target's
//! format is pipeline state in no shipped file, so that the target holds 0 to 1 is inferred. What was brighter is the
//! emission, which the rules put in `emissive` and `emissive_strength`.
//!
//! The ground families are in the whole box (`tests/shibuya_ground.rs`, every prototype) and all are lowered (materials,
//! triangles): `environment_plot` 286, 91,811; `terr_mega` 12, 14,462; `terr_distant` 13, 10,481;
//! `uber_road` 140, 66,426, of which 52, 9,157 are a `core.layered` of two or three layers. The terrain families and
//! the roads' planar layers are mapped by position: `lower-forzatech-planar-uv` writes `TexCoord(3)` for 11 placements
//! (144 primitives), and all 35,858 vertices of the terrain land on their textures through the rule's own scale and
//! offset (the frame, the map scale and the offset agree). The 25 terrain materials all read one pair of maps (the box lies
//! in one submap): its colour map is near white (mean 254, 243, 233) and the squared alpha of its colour map (mean 0.105,
//! under 0.5 on every texel) is the shader's albedo factor, so one albedo is baked of the two (mean 86, 82, 78) and
//! the terrain carries no occlusion slot. Left out by name (materials, triangles): `environment_plot`
//! terrain snow and wetness 286, 91,811, macro occlusion 286, 91,811, lightmap 12, 126; `terr_*` the world-space normal
//! taken as a tangent-space one over the planar coordinates 25, 24,943; `uber_road` the tint overlay 140, 66,426, frost
//! and snow 140, 66,426, the modulate maps 94, 18,340, the blend's smoothstep taken as a linear ramp 52, 9,157.
//!
//! The `environment` swatch tint is not a material's but a placement's: the lowering records the palette
//! ([`crate::components::SwatchTint`]) and `bake-forzatech-swatch-tint` applies it
//! (`tests/shibuya_swatch.rs`, the whole box after `select-lod` at level 1, `select-forzatech-main-view`,
//! `lower-forzatech-materials` and a texture cap of 256): 568 prototypes of 474,356 triangles, their 3,036 primitives
//! split into 3,351 by the vertex colour, 4,149 (model variant, palette row) material variants, 7,359 material copies
//! and 625 baked base colour textures. The row is the instance variation: a rule placement's byte over 255, a
//! hand-placed instance's transform tint (26 instances, inferred) as it is. No prototype holding levels of detail
//! reaches the bake after `select-lod`. Left out, by name: the palette read at one texel, not filtered (179,810
//! triangles; the game's sampler is a run-time one, so its filter is not in the shader: 1,267 palette texels are read,
//! of which a bilinear read would change 477 with clamp addressing and 592 with wrap by more than two stored values);
//! 183 triangles whose corners disagree on the tint mask and 291 tinted ones on the palette column; 2 primitives
//! with no vertex colour; and one material with no base colour texture, tinted as its colour factor.
//! `specialize-material-variants` then makes 2,065 more prototype copies than without the tint, 384,002 triangles
//! more: the placed prototypes go from 2,685 to 4,750 and their triangles from 1,391,523 to 1,775,525.
//!
//! The lowering reports a material of no shader family as unlowered only when a primitive or a model variant uses it:
//! after `select-forzatech-main-view` the `Default.materialbin` materials of the undrawn `Bush_Flatten` and occluder
//! primitives are used by none and are counted apart.
//!
//! Emission (`projection_lod`, `environment`, `uberlod`; their module docs have the shader evidence): over the whole box
//! after `select-lod` at level 1 and `select-forzatech-main-view` (`tests/shibuya_swatch.rs`), 880 materials of 210,791
//! triangles carry an emission map and a factor (the three families together; the run did not split them), 334
//! materials of those families bind a texture with no light and emit nothing (counted), 3 name an emission
//! texture no archive holds, and the 30 materials drawn that read it on `TEXCOORD4` (`EmissiveAltUV`: 12 `environment`,
//! 1,572 triangles, and 18 `environment_signage`, 580) keep it there for `rebase-texcoord-set` and
//! `bake-forzatech-emission-product` (`tests/shibuya_altuv.rs` counts what they make of it). The emission's unit and the sky preset's scale are not in the files (see
//! [`crate::passes::ApplyForzaEmissionVariation`] for the instance variation, which is applied per placement).
mod anim_flag;
mod anim_ledflipbookpanel;
mod branch;
mod diff1;
mod emissiveonly;
mod environment;
mod environment_plot;
mod environment_signage;
pub(crate) use environment_signage::{decode, painted};
mod environmentdeferreddecal;
mod foliage;
mod glass;
mod projection_lod;
mod pylon_cables;
mod terrain;
mod uber_road;
mod uber_roaddecal;
mod uberlod;
mod window_linecastroom;

use crate::components::{
    EmissionByBaseColour, EmissionVariation, ForzaMaterial, MaterialTextures, PlanarUv, SwatchTint, Value,
};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Component, DrawOrder, Entity, LayeredMaterial, MaterialVariants, Mesh, Name, NormalBasis,
    NormalConvention, NormalGreen, NormalX, NormalZ, Semantic, Stream, TexFormat, TexRole, Texture, World,
};
use primase_foundations::color::srgb_to_linear;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// What a texture parameter of a material names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    /// The parameter is absent, or its path is empty (hash 0): no texture.
    None,
    /// The parameter names a texture no archive of the track or the library holds.
    Absent,
    Texture(Entity),
}

/// A material's resolved parameters by name.
struct Params<'a> {
    by_name: HashMap<&'a str, &'a Value>,
    bound: Option<&'a MaterialTextures>,
}

impl<'a> Params<'a> {
    fn new(m: &'a ForzaMaterial, bound: Option<&'a MaterialTextures>) -> Self {
        let mut by_name = HashMap::new();
        for (_, p) in m.resolved() {
            if let Some(n) = p.name.as_deref() {
                // resolved() lists the nearest definition first: a name two hashes share keeps that one
                by_name.entry(n).or_insert(&p.value);
            }
        }
        Params { by_name, bound }
    }

    /// Whether a switch is on (an `Int` or `Bool` parameter other than 0).
    fn on(&self, name: &str) -> bool {
        match self.by_name.get(name) {
            Some(Value::Int(i)) => *i != 0,
            Some(Value::Bool(b)) => *b != 0,
            _ => false,
        }
    }

    /// An integer parameter.
    fn int(&self, name: &str) -> Option<i32> {
        match self.by_name.get(name)? {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Whether a switch the material holds is off (an `Int` or `Bool` parameter of 0).
    fn off(&self, name: &str) -> bool {
        match self.by_name.get(name) {
            Some(Value::Int(i)) => *i == 0,
            Some(Value::Bool(b)) => *b == 0,
            _ => false,
        }
    }

    /// A scalar parameter.
    fn float(&self, name: &str) -> Option<f32> {
        match self.by_name.get(name)? {
            Value::Float(f) if f.is_finite() => Some(*f),
            _ => None,
        }
    }

    /// A four-component parameter.
    fn vec4(&self, name: &str) -> Option<[f32; 4]> {
        match self.by_name.get(name)? {
            Value::Vector(v) | Value::Colour(v) if v.iter().all(|x| x.is_finite()) => Some(*v),
            _ => None,
        }
    }

    /// A four-component swizzle or vector parameter.
    fn swizzle(&self, name: &str) -> Option<[f32; 4]> {
        match self.by_name.get(name)? {
            Value::Swizzle(v) | Value::Vector(v) if v.iter().all(|x| x.is_finite()) => Some(*v),
            _ => None,
        }
    }

    /// A two-component parameter (the first two components of a four-component one).
    fn vec2(&self, name: &str) -> Option<[f32; 2]> {
        match self.by_name.get(name)? {
            Value::Vector2(v) => Some(*v),
            Value::Vector(v) | Value::Colour(v) => Some([v[0], v[1]]),
            _ => None,
        }
    }

    fn texture(&self, name: &str) -> Binding {
        let Some(Value::Texture { path_hash, .. }) = self.by_name.get(name) else {
            return Binding::None;
        };
        if *path_hash == 0 {
            return Binding::None;
        }
        match self
            .bound
            .and_then(|b| b.0.iter().find(|t| t.path_hash == *path_hash))
            .and_then(|t| t.texture)
        {
            Some(e) => Binding::Texture(e),
            None => Binding::Absent,
        }
    }
}

/// The back wall's mean colour of each cell of a room atlas, `None` for a texture with no pixels.
type RoomMeans = Option<Vec<[f64; 3]>>;

/// How the materials of a world were lowered, by reason.
struct Lowering {
    triangles: BTreeMap<Entity, u64>,
    /// Source texture and the re-encoding made of it to the texture that holds it.
    derived: BTreeMap<(Entity, Derived), Entity>,
    /// The road decals' maps made of two textures, made once per pair and recipe.
    decal_maps: BTreeMap<uber_roaddecal::DecalMap, Entity>,
    /// Textures baked from a source and the constants of one material (a key of the baking rule's own), made once.
    baked: BTreeMap<(Entity, Vec<u32>, Option<Entity>), Entity>,
    /// The texture coordinate sets (bit n: `TexCoord(n)`) every primitive drawn with a material carries; a material no
    /// primitive draws is not listed.
    coord_sets: BTreeMap<Entity, u8>,
    /// The triangles drawn with a material by the red of their vertex colour (`ShaderInput(0)`): [all three vertices
    /// 0, all three 1 or more, the rest]; a primitive without the colour counts as all 1 (no scale to apply).
    colour_red: BTreeMap<Entity, [u64; 3]>,
    /// The glass opacity maps in which the reflection floor raised a texel above the transmission.
    reflected: BTreeSet<Entity>,
    /// The mean linear colour of the back wall of each cell of a room atlas (cell by cell, row by row), by texture and
    /// the atlas's cell grid; `None` where the texture has no pixels.
    rooms: BTreeMap<(Entity, [u32; 2]), RoomMeans>,
    /// Whether an emission texture holds any light (a texel with a colour other than black), by texture.
    lit: BTreeMap<Entity, bool>,
    /// The largest tilt of a signed normal map that is flat (see [`FLAT_TILT`]), by texture; `None` for one that is not.
    flat: BTreeMap<Entity, Option<f64>>,
}

impl Lowering {
    /// Whether every primitive drawn with material `e` carries `TexCoord(set)` (true of a material nothing draws: no
    /// primitive lacks it).
    fn has_set(&self, e: Entity, set: u8) -> bool {
        self.coord_sets.get(&e).is_none_or(|bits| bits >> set & 1 == 1)
    }
}

/// A glTF-layout map made from one channel set of a source texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Derived {
    /// `RoughMetAoPor`: R occlusion, G roughness, B metallic from R rough, G metal, B occlusion.
    OcclusionRoughnessMetallic,
    /// A roughness in the alpha channel times a scale (the `f32`'s bits): G roughness, B 0 (no metal), R 1.
    AlphaRoughness(u32),
    /// An occlusion in the blue channel: R occlusion.
    BlueOcclusion,
    /// An occlusion in the alpha channel: R occlusion.
    AlphaOcclusion,
    /// A height in the alpha channel, read as a mask: R height.
    AlphaHeight,
    /// Roughness in R and metalness in G (BC5, signed or not, or an sRGB view): G roughness, B metallic, R 1.
    RoughnessMetalRg,
    /// A roughness in the red channel times a scale (the `f32`'s bits): G roughness, B 0 (no metal), R 1.
    RedRoughness(u32),
    /// A world-space normal's `x` and `z` (the game's frame) in red and green, as a tangent-space normal map over
    /// coordinates `(x, z)` of the game's ground plane: red `x`, green `-z` (the world's frame negates `z`, and
    /// the coordinates' `v` runs down the image), blue the rebuilt `y`.
    WorldNormalXz,
    /// Occlusion in blue and roughness in red: R occlusion, G roughness.
    OcclusionRoughnessRed,
    /// Roughness in R alone and a metalness that is a constant of the material: G roughness, B 1 (the constant is the
    /// metallic factor), R 1.
    RedRoughnessMetal,
    /// A normal map stored in a signed format, whose shader reads `2 v - 1` of the signed value `v`: the positive half
    /// `v` in red and green (a negative `v` is 0), which glTF's `2 b - 1` reads as the same normal, and the rebuilt `z`
    /// in blue.
    SignedNormal,
}

/// The tilt below which a signed normal map is flat: every texel of both channels has `|2 v - 1|` at most this, about
/// 1.1 degrees. The bound is a little over two steps of the 8-bit byte the map would be re-encoded to (a step is 2 / 255
/// = 0.0078 of a component), so a map inside it differs from the flat one by about the quantisation of the lowered map.
/// Dropping such a map is still an approximation, counted with the largest tilt the dropped map has.
const FLAT_TILT: f64 = 0.02;

/// What a normal-map slot makes of its texture.
enum NormalMap {
    /// The slot's normal map: the texture itself, or the map re-encoded from it.
    Map(Entity),
    /// A flat default map (a signed texture holding the neutral normal everywhere, within [`FLAT_TILT`]) with its
    /// largest tilt: it perturbs next to nothing, so the material takes no normal map.
    Flat(f64),
    /// The texture has no pixels.
    Missing,
}

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

fn byte(x: f64) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Record a simplification of a material: counted by material and by the triangles it shades.
fn approximate(log: &mut PassLog, e: Entity, what: &str, triangles: u64) {
    log.approximate_on(e, what, 1);
    log.approximate(&format!("{what}: triangles"), triangles);
}

/// Record a simplification that is a switch of the material being on.
fn left_out(log: &mut PassLog, p: &Params, e: Entity, switch: &str, what: &str, triangles: u64) {
    if p.on(switch) {
        approximate(log, e, what, triangles);
    }
}

impl Lowering {
    /// A texture reference to `texture` through the transform `tiling * (uv + offset)`.
    fn slot(texture: Entity, tiling: Option<[f32; 2]>, offset: [f32; 2]) -> TextureRef {
        let transform = tiling
            .filter(|t| t.iter().all(|x| x.is_finite()) && offset.iter().all(|x| x.is_finite()))
            .map(|t| UvTransform {
                offset: [t[0] * offset[0], t[1] * offset[1]],
                scale: t,
                rotation: 0.0,
            })
            .filter(|t| *t != UvTransform::IDENTITY);
        TextureRef {
            transform,
            ..TextureRef::new(texture)
        }
    }

    /// Whether the emission texture `texture` holds any light: a texel of level 0 whose colour is not black. Read once per
    /// texture; a texture with no pixels counts as dark.
    fn emits_light(&mut self, w: &World, texture: Entity) -> bool {
        if let Some(&lit) = self.lit.get(&texture) {
            return lit;
        }
        let mut lit = false;
        if let Some(t) = w.get::<Texture>(texture) {
            let found = primase_image::map_rgba8_levels(
                t,
                0..1,
                primase_image::TextureMetadata {
                    color_space: t.color_space,
                    role: TexRole::Mask,
                    normal: None,
                },
                |texel| {
                    lit |= texel.rgba[..3].iter().any(|c| *c != 0);
                    Ok(texel.rgba)
                },
            );
            if found.is_err() {
                lit = false;
            }
        }
        self.lit.insert(texture, lit);
        lit
    }

    /// A material's emission read from the texture `bound` times `scale`, when the texture holds any light: the texture
    /// as the emissive map (through the identity; the caller sets the transform), the scale as the emissive factor (what
    /// is above 1 in `emissive_strength`, so the product is the authored radiance). Counts what emits nothing.
    fn emission_map(
        &mut self,
        w: &mut World,
        e: Entity,
        pbr: &mut PbrMaterial,
        bound: Binding,
        scale: f32,
        log: &mut PassLog,
    ) {
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let family = w
            .get::<ForzaMaterial>(e)
            .and_then(|m| m.shader.as_ref())
            .map_or("no shader family", |s| s.family.as_str())
            .to_owned();
        match bound {
            Binding::Texture(t) if scale > 0.0 => {
                if self.emits_light(w, t) {
                    if let Some(tex) = w.get_mut::<Texture>(t)
                        && tex.role == TexRole::Other
                        && tex.color_space == ColorSpace::Srgb
                    {
                        tex.role = TexRole::Color;
                    }
                    let strength = scale.max(1.0);
                    pbr.emissive_texture = Some(TextureRef::new(t));
                    pbr.emissive = [scale / strength; 3];
                    pbr.emissive_strength = strength;
                    approximate(
                        log,
                        e,
                        &format!(
                            "{family} emission without the distance encoding and the sky preset's scale (run-time); the instance variation is applied per placement"
                        ),
                        triangles,
                    );
                } else {
                    log.count(
                        &format!("{family} emissive materials whose emission texture is black: no emission"),
                        1,
                    );
                }
            }
            Binding::Texture(_) => log.count(
                &format!("{family} emissive materials with an emission scale of 0: no emission"),
                1,
            ),
            Binding::Absent => approximate(
                log,
                e,
                &format!("{family} emission texture in no archive: no emission"),
                triangles,
            ),
            Binding::None => approximate(
                log,
                e,
                &format!("{family} no emission texture: no emission"),
                triangles,
            ),
        }
    }

    /// Marks the emission of the lowered material `e` as scaled by the instance variation, from the ramp `min` to `max`
    /// of its parameters, when it emits.
    fn vary_emission(w: &mut World, e: Entity, p: &Params, pbr: &PbrMaterial) {
        if pbr.emissive_texture.is_some()
            && let (Some(min), Some(max)) = (p.float("EmissiveVariationMin"), p.float("EmissiveVariationMax"))
        {
            w.insert(e, EmissionVariation { min, max });
        }
    }

    /// The map of `kind` made of `source`, made once per source and kind.
    fn derive(
        &mut self,
        w: &mut World,
        source: Entity,
        kind: Derived,
        log: &mut PassLog,
    ) -> Result<Option<Entity>, String> {
        if let Some(&e) = self.derived.get(&(source, kind)) {
            return Ok(Some(e));
        }
        let (what, suffix) = match kind {
            Derived::OcclusionRoughnessMetallic => ("RoughMetAoPor", "occlusion roughness metallic"),
            Derived::AlphaRoughness(_) => ("roughness-in-alpha", "roughness"),
            Derived::BlueOcclusion => ("occlusion-in-blue", "occlusion"),
            Derived::AlphaOcclusion => ("occlusion-in-alpha", "occlusion"),
            Derived::AlphaHeight => ("height-in-alpha", "height"),
            Derived::RoughnessMetalRg => ("roughness-metalness-in-rg", "roughness metallic"),
            Derived::RedRoughness(_) => ("roughness-in-red", "roughness"),
            Derived::WorldNormalXz => ("world-space normal", "normal"),
            Derived::OcclusionRoughnessRed => ("occlusion-in-blue roughness-in-red", "occlusion roughness"),
            Derived::RedRoughnessMetal => ("roughness-in-red", "roughness metallic"),
            Derived::SignedNormal => ("signed normal", "normal"),
        };
        let Some(t) = w.get::<Texture>(source) else {
            log.count(&format!("{what} textures left out: no pixels"), 1);
            return Ok(None);
        };
        let space = t.color_space;
        // a signed texture decodes to `v * 0.5 + 0.5`; its shader read `v`, a roughness or metalness of 0 to 1
        let signed = matches!(
            t.format,
            TexFormat::R8Snorm
                | TexFormat::Rg8Snorm
                | TexFormat::Rgba8Snorm
                | TexFormat::Rg16Snorm
                | TexFormat::Bc4Snorm
                | TexFormat::Bc5Snorm
        );
        let mut negative = 0u64;
        let texture = primase_image::map_rgba8_levels(
            t,
            0..t.levels.len(),
            primase_image::TextureMetadata {
                color_space: ColorSpace::Linear,
                role: TexRole::Mask,
                normal: None,
            },
            |texel| {
                let [r, g, b, a] = texel.rgba;
                Ok(match kind {
                    Derived::SignedNormal => {
                        // the signed value is `byte / 127.5 - 1`; the shader's normal is `2 v - 1` of it
                        let (vx, vy) = (f64::from(r) / 127.5 - 1.0, f64::from(g) / 127.5 - 1.0);
                        negative += u64::from(vx < 0.0) + u64::from(vy < 0.0);
                        let (x, y) = (2.0 * vx.max(0.0) - 1.0, 2.0 * vy.max(0.0) - 1.0);
                        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
                        [byte(vx), byte(vy), byte(z * 0.5 + 0.5), 255]
                    }
                    Derived::OcclusionRoughnessMetallic => [
                        byte(seen(space, b)),
                        byte(seen(space, r)),
                        byte(seen(space, g)),
                        255,
                    ],
                    // the alpha channel is never sRGB-decoded
                    Derived::AlphaRoughness(scale) => [
                        255,
                        byte(f64::from(a) / 255.0 * f64::from(f32::from_bits(scale))),
                        0,
                        255,
                    ],
                    Derived::BlueOcclusion => {
                        let o = byte(seen(space, b));
                        [o, o, o, 255]
                    }
                    Derived::AlphaOcclusion | Derived::AlphaHeight => [a, a, a, 255],
                    Derived::RedRoughness(scale) => [
                        255,
                        byte(seen(space, r) * f64::from(f32::from_bits(scale))),
                        0,
                        255,
                    ],
                    Derived::WorldNormalXz => {
                        let (x, z) = (seen(space, r) * 2.0 - 1.0, seen(space, g) * 2.0 - 1.0);
                        let y = (1.0 - x * x - z * z).max(0.0).sqrt();
                        [byte(x * 0.5 + 0.5), byte(0.5 - z * 0.5), byte(y * 0.5 + 0.5), 255]
                    }
                    Derived::RedRoughnessMetal => [255, byte(seen(space, r)), 255, 255],
                    Derived::RoughnessMetalRg => {
                        let read = |x: u8| {
                            if signed {
                                (f64::from(x) / 127.5 - 1.0).max(0.0)
                            } else {
                                seen(space, x)
                            }
                        };
                        [255, byte(read(r)), byte(read(g)), 255]
                    }
                    Derived::OcclusionRoughnessRed => [byte(seen(space, b)), byte(seen(space, r)), 0, 255],
                })
            },
        )
        .map_err(|e| format!("making a {what} map: {e}"))?;
        if negative > 0 {
            log.approximate(
                &format!(
                    "{what} map texels below zero clamped to zero (glTF's unsigned map cannot hold them)"
                ),
                negative,
            );
        }
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} ({suffix})")));
        w.insert(e, texture);
        log.count(&format!("{what} textures re-encoded as glTF {suffix} maps"), 1);
        self.derived.insert((source, kind), e);
        Ok(Some(e))
    }

    /// The largest tilt `|2 v - 1|` of the signed normal map `texture` when it is flat, `None` otherwise: a texel of level
    /// 0 with a channel above [`FLAT_TILT`] (or whose `v` is negative, which the shader's `2 v - 1` cannot read as a
    /// unit-length normal) makes it not. Read once per texture; a texture that cannot be read is not flat.
    fn signed_flat(&mut self, w: &World, texture: Entity) -> Option<f64> {
        if let Some(&flat) = self.flat.get(&texture) {
            return flat;
        }
        let mut tilt = 0.0f64;
        let mut read = false;
        if let Some(t) = w.get::<Texture>(texture) {
            read = primase_image::map_rgba8_levels(
                t,
                0..1,
                primase_image::TextureMetadata {
                    color_space: ColorSpace::Linear,
                    role: TexRole::Mask,
                    normal: None,
                },
                |texel| {
                    for c in &texel.rgba[..2] {
                        tilt = tilt.max((2.0 * (f64::from(*c) / 127.5 - 1.0) - 1.0).abs());
                    }
                    Ok(texel.rgba)
                },
            )
            .is_ok();
        }
        let flat = (read && tilt <= FLAT_TILT).then_some(tilt);
        self.flat.insert(texture, flat);
        flat
    }

    /// The normal-map slot of material `e` for `texture` (`slot` makes it of the map), or none: a flat default map takes
    /// no slot (an approximation, named with its largest tilt), a texture with no pixels is counted as a normal map not
    /// lowered.
    fn normal_slot(
        &mut self,
        w: &mut World,
        e: Entity,
        texture: Entity,
        triangles: u64,
        log: &mut PassLog,
        slot: impl FnOnce(Entity) -> TextureRef,
    ) -> Result<Option<TextureRef>, String> {
        Ok(match self.normal_map(w, texture, log)? {
            NormalMap::Map(map) => Some(slot(map)),
            NormalMap::Flat(tilt) => {
                approximate(
                    log,
                    e,
                    &format!(
                        "flat default normal map left out: no normal map (largest tilt {tilt:.3}, within the {FLAT_TILT} bound)"
                    ),
                    triangles,
                );
                None
            }
            NormalMap::Missing => {
                approximate(
                    log,
                    e,
                    "normal map not lowered: the texture has no pixels",
                    triangles,
                );
                None
            }
        })
    }

    /// Make `texture` a normal map in the frame the environment shader reads: X in red, green up, Z rebuilt. A texture
    /// in a signed format holds the shader's `v` (its normal is `2 v - 1`): a flat one is [`NormalMap::Flat`], any other
    /// is re-encoded ([`Derived::SignedNormal`]) as the unsigned map glTF reads.
    fn normal_map(&mut self, w: &mut World, texture: Entity, log: &mut PassLog) -> Result<NormalMap, String> {
        let Some(t) = w.get::<Texture>(texture) else {
            log.count("normal maps left out: no pixels", 1);
            return Ok(NormalMap::Missing);
        };
        let texture = if matches!(
            t.format,
            TexFormat::Rg8Snorm | TexFormat::Rgba8Snorm | TexFormat::Rg16Snorm | TexFormat::Bc5Snorm
        ) {
            if t.levels.is_empty() {
                log.count("normal maps left out: no pixels", 1);
                return Ok(NormalMap::Missing);
            }
            if let Some(tilt) = self.signed_flat(w, texture) {
                log.count("flat default normal map: no normal map", 1);
                return Ok(NormalMap::Flat(tilt));
            }
            match self.derive(w, texture, Derived::SignedNormal, log)? {
                Some(made) => made,
                None => {
                    log.count("normal maps left out: no pixels", 1);
                    return Ok(NormalMap::Missing);
                }
            }
        } else {
            texture
        };
        let Some(t) = w.get_mut::<Texture>(texture) else {
            return Ok(NormalMap::Missing);
        };
        if t.role != TexRole::Normal {
            t.role = TexRole::Normal;
            t.color_space = ColorSpace::Linear;
            t.normal = Some(NormalConvention {
                green: NormalGreen::Up,
                z: NormalZ::Rebuild,
                x: NormalX::Red,
                basis: NormalBasis::TangentBitangent,
            });
            log.count("textures made normal maps by their slot", 1);
        }
        Ok(NormalMap::Map(texture))
    }
}

/// The texture coordinate set `lower-forzatech-planar-uv` fills with a point's position in the ground plane: the sets
/// the meshes carry are 0 to 4, and a lowered slot samples one of the first three (the second and third are the
/// lightmap and blend coordinates a layered road reads).
const PLANAR_TEXCOORD: u8 = 3;

/// The queue of the decal layer: after the alpha-tested surfaces, before the blended ones (core's convention for the
/// source's decal bucket; the draw sequence of the game's own passes is not in any file).
const DECAL_QUEUE: i32 = 2500;

/// The alpha-test reference of a masked material, in the absence of one in the material: the middle of the range.
const ALPHA_REFERENCE: f32 = 0.5;

/// Whether the material is alpha tested: the parent material's `ATST` flag (the instance's when it has no parent).
fn alpha_tested(m: &ForzaMaterial) -> bool {
    m.material.as_ref().map_or(m.instance.atst, |x| x.atst)[0] != 0
}

/// What a rule made of a material: a plain one, which the pass inserts, or a blend of layers (`core.layered`), which
/// the rule inserted itself and which is drawn with no plain material beside it.
enum Lowered {
    Plain(Box<PbrMaterial>),
    Blend,
}

#[derive(Clone, Copy, Debug)]
pub struct LowerForzaMaterials;

pub const LOWER_FORZATECH_MATERIALS: PassSpec = PassSpec {
    name: "lower-forzatech-materials",
    options: &[],
    make: |_| Ok(Box::new(LowerForzaMaterials)),
};

impl Pass for LowerForzaMaterials {
    fn name(&self) -> &'static str {
        "lower-forzatech-materials"
    }
    fn requires(&self) -> &[&'static str] {
        &[ForzaMaterial::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            PbrMaterial::NAME,
            LayeredMaterial::NAME,
            PlanarUv::NAME,
            SwatchTint::NAME,
            EmissionVariation::NAME,
            EmissionByBaseColour::NAME,
            Texture::NAME,
            Name::NAME,
            DrawOrder::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    fn approximates(&self) -> &'static str {
        "environment materials by their texture maps alone: the modulate, deposits, triplanar, \
         parallax and emission of a permutation are left out, wetness and lighting are run-time state; decals as \
         alpha-tested or blended materials in the decal layer: the dither is a plain test at one half and the \
         vertex colour, the far fade and the decal buffer's blend are left out"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
        for (_, mesh) in world.iter::<Mesh>() {
            for primitive in mesh.primitives() {
                if let Some(m) = primitive.material {
                    *triangles.entry(m).or_default() +=
                        primitive.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
                }
            }
        }
        let mut coord_sets: BTreeMap<Entity, u8> = BTreeMap::new();
        for (_, mesh) in world.iter::<Mesh>() {
            for primitive in mesh.primitives() {
                if let Some(m) = primitive.material {
                    let present = (0..8u8)
                        .filter(|n| primitive.streams.contains_key(&Semantic::TexCoord(*n)))
                        .fold(0u8, |bits, n| bits | 1 << n);
                    *coord_sets.entry(m).or_insert(u8::MAX) &= present;
                }
            }
        }
        let mut colour_red: BTreeMap<Entity, [u64; 3]> = BTreeMap::new();
        for (_, mesh) in world.iter::<Mesh>() {
            for primitive in mesh.primitives() {
                let Some(m) = primitive.material else {
                    continue;
                };
                let tally = colour_red.entry(m).or_default();
                let colour = match primitive.streams.get(&Semantic::ShaderInput(0)) {
                    Some(Stream::F32x4(c)) => Some(c),
                    _ => None,
                };
                let count = primitive.indices.as_ref().map_or(0, Vec::len) / 3;
                let (Some(c), Some(indices)) = (colour, primitive.indices.as_ref()) else {
                    tally[1] += count as u64;
                    continue;
                };
                for t in indices.as_chunks::<3>().0 {
                    let red = t.iter().map(|i| c.get(*i as usize).map_or(1.0, |v| v[0]));
                    let (low, high) =
                        red.fold((true, true), |(low, high), x| (low && x == 0.0, high && x >= 1.0));
                    tally[if low {
                        0
                    } else if high {
                        1
                    } else {
                        2
                    }] += 1;
                }
            }
        }
        // the materials a primitive draws or a model variant swaps in: a material with no shader family that none does
        // (the `Default.materialbin` ones of primitives taken out earlier, the occluders) is not an omission
        let mut used: BTreeSet<Entity> = triangles.keys().copied().collect();
        for (_, variants) in world.iter::<MaterialVariants>() {
            used.extend(variants.0.iter().flat_map(|map| map.values().copied()));
        }
        let materials: Vec<(Entity, ForzaMaterial)> = world
            .iter::<ForzaMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut lowering = Lowering {
            triangles,
            derived: BTreeMap::new(),
            decal_maps: BTreeMap::new(),
            baked: BTreeMap::new(),
            coord_sets,
            colour_red,
            reflected: BTreeSet::new(),
            rooms: BTreeMap::new(),
            lit: BTreeMap::new(),
            flat: BTreeMap::new(),
        };
        let mut left: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for (e, m) in &materials {
            let family = m.shader.as_ref().map(|s| s.family.as_str());
            let lowered = match family {
                Some("environment") => Some(lowering.environment(world, *e, m, log)?),
                Some("uber_road") => match lowering.uber_road(world, *e, m, log)? {
                    Lowered::Plain(pbr) => Some(*pbr),
                    Lowered::Blend => {
                        log.count("uber_road materials lowered", 1);
                        continue;
                    }
                },
                Some("terr_mega" | "terr_distant") => Some(lowering.terrain(world, *e, m, log)?),
                Some("environment_plot") => Some(lowering.environment_plot(world, *e, m, log)?),
                Some("anim_flag") => Some(lowering.anim_flag(world, *e, m, log)?),
                Some("anim_ledflipbookpanel") => Some(lowering.anim_ledflipbookpanel(world, *e, m, log)?),
                Some("diff1") => Some(lowering.diff1(world, *e, m, log)?),
                Some("emissiveonly") => Some(lowering.emissiveonly(world, *e, m, log)?),
                Some("glass") => Some(lowering.glass(world, *e, m, log)?),
                Some("window_linecastroom") => Some(lowering.window_linecastroom(world, *e, m, log)?),
                Some("pylon_cables") => Some(lowering.pylon_cables(world, *e, m, log)?),
                Some("environment_signage") => Some(lowering.environment_signage(world, *e, m, log)?),
                Some("uber_foliage") => Some(lowering.uber_foliage(world, *e, m, log)?),
                Some("uber_branch") => lowering.uber_branch(world, *e, m, log)?,
                Some("projection_lod") => lowering.projection_lod(world, *e, m, log)?,
                Some("uberlod") => lowering.uberlod(world, *e, m, log)?,
                Some("uber_roaddecal") => lowering.uber_roaddecal(world, *e, m, log)?,
                Some("uber_roaddecalretro") => lowering.uber_roaddecalretro(world, *e, m, log)?,
                Some("environmentdeferreddecal") => lowering.environmentdeferreddecal(world, *e, m, log)?,
                _ => None,
            };
            let Some(mut pbr) = lowered else {
                if family.is_none() && !used.contains(e) {
                    log.count("materials of no shader family that nothing draws", 1);
                    continue;
                }
                let name = match family {
                    Some("uber_branch") => "uber_branch with a second set of maps",
                    Some("uberlod") => "uberlod with a normal map",
                    Some("environmentdeferreddecal") => "environmentdeferreddecal projected",
                    other => other.unwrap_or("no shader family"),
                };
                let entry = left.entry(name.to_owned()).or_default();
                entry.0 += 1;
                entry.1 += lowering.triangles.get(e).copied().unwrap_or(0);
                continue;
            };
            // the G-buffer stores the albedo in a target of 0..1 (no shader of the lowered families saturates it, so
            // the target's format, run-time state, is what clamps it): a factor above 1 is stored as 1, and what
            // was brighter is emission, which the rules put in `emissive` and `emissive_strength`
            if pbr.base_color.iter().any(|x| *x > 1.0) {
                pbr.base_color = pbr.base_color.map(|x| x.min(1.0));
                approximate(
                    log,
                    *e,
                    "base colour factor above 1 stored as 1 (the albedo target holds 0 to 1)",
                    lowering.triangles.get(e).copied().unwrap_or(0),
                );
            }
            world.insert(*e, pbr);
            log.count(&format!("{} materials lowered", family.unwrap_or_default()), 1);
        }
        for (family, (materials, triangles)) in left {
            log.omit(
                &format!("materials of shader family {family} left unlowered"),
                materials,
            );
            log.omit(
                &format!("triangles of shader family {family} left unlowered"),
                triangles,
            );
        }
        Ok(())
    }
}
