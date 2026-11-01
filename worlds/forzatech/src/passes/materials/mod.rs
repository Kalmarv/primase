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
//! `projection_lod` 59, 10,002; `uber_foliage` 17, 13,672; `uber_branch` 6, 8,213; `environmentdeferreddecal` 70, 1,549;
//! `uber_roaddecal` 2, 146; `pylon_cables` 2, 532; `environment_signage` 88, 520; `glass` 8, 188;
//! `window_linecastroom` 51, 78; `emissiveonly` 2, 64: 90,428 of the box's 90,430 triangles (99.998%; the two left are
//! a foliage impostor). Each lowered material passes `pbr.material`'s own verification, and the decal families alone
//! have a draw order (the decal layer, no depth write).
//!
//! The `environment` rule packs 85 distinct RoughMetAoPor maps and makes normal maps of 83 distinct textures (7 more,
//! 838 triangles, are BC5 SNORM, which the shader's `2 r - 1` does not read: left out, counted). The packed maps'
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
//! their own module. Left unlowered, with no rule yet: the `uber_foliage` impostor, 1 material and 2 triangles (a
//! view-selected atlas of directions has no static material form). The normal maps are made in `NormalZ::Rebuild`:
//! `normalize-normals` converts them to glTF's before a writer.
//!
//! The ground families are in the whole box (`tests/shibuya_ground.rs`, every prototype) and all are lowered (materials,
//! triangles): `environment_plot` 286, 91,811; `terr_mega` 12, 14,462; `terr_distant` 13, 10,481;
//! `uber_road` 140, 66,426, of which 52, 9,157 are a `core.layered` of two or three layers. The terrain families and
//! the roads' planar layers are mapped by position: `lower-forzatech-planar-uv` writes `TexCoord(3)` for 11 placements
//! (144 primitives), and all 35,858 vertices of the terrain land on their textures through the rule's own scale and
//! offset (the frame, the map scale and the offset agree). Left out by name (materials, triangles): `environment_plot`
//! terrain snow and wetness 286, 91,811, macro occlusion 286, 91,811, lightmap 12, 126; `terr_*` the world-space normal
//! taken as a tangent-space one over the planar coordinates 25, 24,943; `uber_road` the tint overlay 140, 66,426, frost
//! and snow 140, 66,426, the modulate maps 94, 18,340, the blend's smoothstep taken as a linear ramp 52, 9,157.
//!
//! The `environment` swatch tint is not a material's but a placement's: the lowering records the palette
//! ([`crate::components::SwatchTint`]) and `bake-forzatech-swatch-tint` applies it
//! (`tests/shibuya_swatch.rs`, the whole box after `select-lod` at level 1, `lower-forzatech-materials` and a texture
//! cap of 256): 568 prototypes of 474,356 triangles, their 3,036 primitives split into 3,351 by the vertex colour, 4,144
//! (model variant, tint byte) material variants, 7,251 material copies and 549 baked base colour textures. Left out, by
//! name: the palette read at one texel, not filtered (179,810 triangles); 26 hand-placed instances, untinted (their tint
//! byte is not traced); 183 triangles whose corners disagree on the tint mask and 291 tinted ones on the palette column;
//! 2 primitives with no vertex colour; and one material with no base colour texture, tinted as its colour factor.
//! `specialize-material-variants` then makes 2,060 more prototype copies than without the tint, 383,854 triangles more:
//! the placed prototypes go from 2,685 to 4,745 and their triangles from 1,419,192 to 1,803,046.
mod anim_flag;
mod anim_ledflipbookpanel;
mod branch;
mod diff1;
mod emissiveonly;
mod environment;
mod environment_plot;
mod environment_signage;
mod environmentdeferreddecal;
mod foliage;
mod glass;
mod projection_lod;
mod pylon_cables;
mod terrain;
mod uber_road;
mod uber_roaddecal;
mod window_linecastroom;

use crate::components::{ForzaMaterial, MaterialTextures, PlanarUv, SwatchTint, Value};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Component, DrawOrder, Entity, LayeredMaterial, Mesh, Name, NormalBasis, NormalConvention,
    NormalGreen, NormalX, NormalZ, Semantic, TexFormat, TexRole, Texture, World,
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
    /// The glass opacity maps in which the reflection floor raised a texel above the transmission.
    reflected: BTreeSet<Entity>,
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
    /// An occlusion in the alpha channel, squared: R occlusion.
    SquaredAlphaOcclusion,
    /// A world-space normal's `x` and `z` (the game's frame) in red and green, as a tangent-space normal map over
    /// coordinates `(x, z)` of the game's ground plane: red `x`, green `-z` (the world's frame negates `z`, and
    /// the coordinates' `v` runs down the image), blue the rebuilt `y`.
    WorldNormalXz,
    /// Occlusion in blue and roughness in red: R occlusion, G roughness.
    OcclusionRoughnessRed,
    /// Roughness in R alone and a metalness that is a constant of the material: G roughness, B 1 (the constant is the
    /// metallic factor), R 1.
    RedRoughnessMetal,
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
            Derived::SquaredAlphaOcclusion => ("occlusion-in-alpha, squared,", "occlusion"),
            Derived::WorldNormalXz => ("world-space normal", "normal"),
            Derived::OcclusionRoughnessRed => ("occlusion-in-blue roughness-in-red", "occlusion roughness"),
            Derived::RedRoughnessMetal => ("roughness-in-red", "roughness metallic"),
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
                    Derived::SquaredAlphaOcclusion => {
                        let o = byte((f64::from(a) / 255.0).powi(2));
                        [o, o, o, 255]
                    }
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
        let name = w.get::<Name>(source).map(|n| n.0.clone()).unwrap_or_default();
        let e = w.spawn();
        w.insert(e, Name(format!("{name} ({suffix})")));
        w.insert(e, texture);
        log.count(&format!("{what} textures re-encoded as glTF {suffix} maps"), 1);
        self.derived.insert((source, kind), e);
        Ok(Some(e))
    }

    /// Make `texture` a normal map in the frame the environment shader reads: X in red, green up, Z rebuilt.
    fn normal_map(w: &mut World, texture: Entity, log: &mut PassLog) -> bool {
        let Some(t) = w.get_mut::<Texture>(texture) else {
            log.count("normal maps left out: no pixels", 1);
            return false;
        };
        if matches!(t.format, TexFormat::Bc5Snorm) {
            // the shader reads 2 r - 1 of an unsigned value; a signed format cannot be that map
            log.count(
                "normal maps left out: stored in a signed format the shader's 2 r - 1 does not read",
                1,
            );
            return false;
        }
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
        true
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
            reflected: BTreeSet::new(),
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
                Some("uber_foliage") => lowering.uber_foliage(world, *e, m, log)?,
                Some("uber_branch") => lowering.uber_branch(world, *e, m, log)?,
                Some("projection_lod") => lowering.projection_lod(world, *e, m, log)?,
                Some("uber_roaddecal") => lowering.uber_roaddecal(world, *e, m, log)?,
                Some("uber_roaddecalretro") => lowering.uber_roaddecalretro(world, *e, m, log)?,
                Some("environmentdeferreddecal") => lowering.environmentdeferreddecal(world, *e, m, log)?,
                _ => None,
            };
            let Some(pbr) = lowered else {
                let name = match family {
                    Some("uber_foliage") => "uber_foliage billboard or impostor",
                    Some("uber_branch") => "uber_branch with a second set of maps",
                    Some("environmentdeferreddecal") => "environmentdeferreddecal projected",
                    other => other.unwrap_or("no shader family"),
                };
                let entry = left.entry(name.to_owned()).or_default();
                entry.0 += 1;
                entry.1 += lowering.triangles.get(e).copied().unwrap_or(0);
                continue;
            };
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
