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
//! # `environment`
//!
//! Evidence, over all 379 GBuffer permutations of the family in the Tokyo track's `bin.zip` (render target 0 holds the
//! albedo and, in W, the metalness; 1 the roughness in W and the lit colour in XYZ; 2 the normal):
//! - the metalness (target 0 W) is `RoughMetAoPorTex_A.g` in the 273 permutations with one texture set (199 alone, 74
//!   with the mega-skirt blend's textures, the `_MS` ones), and that texture and `RoughMetAoPorTex_B.g` in the 106 that
//!   blend two (`Blend2`, off in every material of the track's box);
//! - the roughness (target 1 W) with wetness off is `saturate(RoughMetAoPorTex_A.r)`; wetness (`WetnessMask`) lerps it
//!   with a wet model from run-time constants;
//! - occlusion is `RoughMetAoPorTex_A.b`, raised to a scene power and lerped by a scene strength into the albedo
//!   (target 0 XYZ);
//! - the albedo (target 0 XYZ) of the permutation without swatch tint, modulate, deposits or triplanar is
//!   `DiffAlphaTex_A.rgb` (read through an sRGB view, so linear) taken to gamma 1/2.2, darkened by wetness, taken
//!   back by 2.2: itself when dry, times the occlusion factor above. 378 of the 379 permutations read
//!   `DiffAlphaTex_A.r` into target 0;
//! - the normal (target 2) is `x = 2 r - 1, y = 2 g - 1, z = sqrt(saturate(1 - x^2 - y^2))` of `NormHeightTex_A`,
//!   brought to the world as `x T - y B + z N` with `B = (N x T) w` of the vertex tangent (`TANGENT.w` is decoded to
//!   its sign). The game's `B` points along +v, down the image (the engine wiki's check over 73,176 triangles of its
//!   meshes), the opposite of glTF's, so the `-y B` is glTF's `+y B`: green up, X red, Z rebuilt (`NormalConvention`
//!   says so, and no writer converts it). 378 of the 379 permutations read `NormHeightTex_A.r` and `.g` into target 2;
//! - the three maps are sampled at `tiling * (uv0 + Uv0Offset)` with each map's own tiling parameter
//!   (`BaseColorAlpha_A_UVTiling`, `NormalHeight_A_UVTiling`, `RoughMetAoPoros_A_UVTiling`), so each slot's transform
//!   is `scale = tiling, offset = tiling * Uv0Offset` on the first texture coordinate set (the shader's `TEXCOORD0`).
//!   A vertical scroll (`AnimTranslate`) is added to the V of the normal, albedo and RoughMetAo samples when a
//!   parameter is set; it is off in every material of the box.
//!
//! The `RoughMetAoPorTex_A` texture (R rough, G metal, B occlusion, A porosity) is re-encoded as glTF's packed map,
//! R occlusion, G roughness, B metallic, and bound to both the metallic-roughness and the occlusion slot, so each
//! sampler reads it once; a source stored sRGB (BC1 `_SRGB` in the track) is decoded first, as the shader's view does.
//!
//! The simplifications: the surface the permutation makes dry and unlit (wetness, the lightmap and light probes are
//! run-time state); the albedo's occlusion product is carried by the occlusion slot instead; and, named per material,
//! swatch tint, modulate, deposits, triplanar, relaxed cone-step mapping, mega-skirt, emission, retro-reflection and
//! per-instance random UV offset are left out, because each needs a vertex colour, the instance variation, the world
//! position or a run-time constant that no material holds.
//!
//! # `uber_foliage`
//!
//! Evidence, over the 27 GBuffer permutations of the family (the dependency census of each render target channel;
//! the arithmetic of the bush and tree permutations read out beside it): the albedo is `DiffuseTexture.rgb` times a
//! factor of `RTAOTexture.b` that the scene's constants (`cb2[43]`, `[44]`, `[159]`) and the vertex normal's Y shape;
//! the roughness (target 1 W) is `NormalTexture.a * RoughnessScale` in the 11 high-quality, static and punch-through tree,
//! bush and grass permutations (the very-low-quality ones write none, nor a normal), and the metalness is 0;
//! the normal (target 2) of the bush permutations is the standard decode of `NormalTexture.rg` taken to the world on
//! the vertex tangent (fading to the up vector beyond 4,500 m), and the tree's a canopy normal of all four channels
//! and the tree's bounding box (`CanopyNormal*`), which no glTF slot holds; the pixel is discarded when
//! `saturate((DiffuseTexture.a - AlphaStepMin) / (AlphaStepMax - AlphaStepMin))` is below the draw's reference
//! (`cb2[128].x`), so the alpha is tested against `AlphaStepMin + reference * (AlphaStepMax - AlphaStepMin)`. Every
//! map is read at `TEXCOORD0` untiled.
//!
//! The rule: `DiffuseTexture` the base colour, its alpha masked at the middle reference; `NormalTexture.a * scale`
//! re-encoded as the roughness of a metallic-roughness map (no metal), which glTF reads at `TEXCOORD0` as the shader
//! does; the bush's `NormalTexture` the normal map; `RTAOTexture.b` re-encoded as the occlusion map (its scene
//! strength and power are left out, named); `DoubleSided` the material's. The billboard and impostor permutations
//! (atlases of views) are left unlowered; tint, world tint, wind animation, vertex occlusion and wetness are left out,
//! named. A BC5 `NormalTexture` has no alpha: rough.
//!
//! # `uber_branch`
//!
//! Evidence, over the 7 GBuffer permutations: the albedo is `DiffuseA.rgb` read at `uv0 * DiffuseAUVTiling`, times a
//! factor of `NormalA.a` (the `NormalAlpha_AO` permutation's occlusion), the vertex colour's alpha and scene constants;
//! the roughness is `DiffuseA.a`, the metalness 0; the normal is the standard decode of `NormalA.rg` read at
//! `uv0 * NormalAUVTiling`, `z` rebuilt from the unscaled `x, y`, then `x, y` times `NormalAIntensity`, brought to the
//! world as the environment's is (green up, which is glTF's `normalScale`); the only discard compares a constant, not a
//! texture, so the surface is opaque. The `_MS` permutations blend the terrain in below a height (mega-skirt): left
//! out, named. A material that blends a second set (`Blend2`) is left unlowered.
//!
//! # `projection_lod`
//!
//! Evidence, over the 24 GBuffer permutations: every map (`DiffuseATexture`, `RoughnessMetalnessATexture`,
//! `NormalATexture`, `EmissiveTexture`, `BakedVCTexture`) is read at `UVOutMinMax.xy + (UVOutMinMax.zw -
//! UVOutMinMax.xy) * TEXCOORD0`, the atlas cell of the building's far view; the albedo is `DiffuseATexture.rgb` times
//! `(1 - a (1 - v)) (1 - s) + s`, `a` the diffuse alpha, `v = BakedVCTexture.r`, `s = SwatchTintTexture.r` (the swatch
//! row `BakedVCTexture.g` picks, from the instance variation); with `RoughnessMetal` the roughness is
//! `RoughnessMetalnessATexture.r` and the metalness `.g` (otherwise the constant `Roughness` and 0); the HiQ
//! permutations read the normal from `NormalATexture.rg` as the environment does and the others take the vertex normal;
//! the alpha-test permutations discard when `DiffuseATexture.a` is below the draw's reference. The roughness-metalness
//! maps are BC5 SNORM (9,650 triangles) or BC7 sRGB (352): the shader reads `v` of the signed one, so a roughness of 0
//! to 1 is its positive half (19 of the 36 maps' 34,701,312 channels in the box are below -0.06), and the sRGB one as decoded.
//!
//! The rule: the cell is each slot's transform; `DiffuseATexture` the base colour, masked at the middle reference
//! where the permutation tests alpha; the roughness-metalness map re-encoded as glTF's (G roughness, B metallic, the
//! signed half-range read back); `NormalATexture` the normal map. The baked occlusion factor (two textures), swatch
//! tint, emission (its distance encoding and instance variation are run-time), lightmap and wetness are left out, named.
//!
//! # Over the Shibuya Scramble box
//!
//! `tests/shibuya_materials.rs` runs the pass on every 20th prototype of the box (x -1550..-1100, z -5000..-4566.67,
//! summer: 77 prototypes, 90,430 shaded triangles). Lowered (materials, triangles): `environment` 758, 55,464 (61.3%);
//! `projection_lod` 59, 10,002; `uber_foliage` 16, 13,670; `uber_branch` 6, 8,213:
//! 87,349 of the box's 90,430 triangles (96.6%). Each lowered material passes `pbr.material`'s own verification.
//!
//! The `environment` rule packs 85 distinct RoughMetAoPor maps and makes normal maps of 83 distinct textures (7 more,
//! 838 triangles, are BC5 SNORM, which the shader's `2 r - 1` does not read: left out, counted). The packed maps'
//! metallic channel holds an extreme (below 16 or above 239) in 97.97% of its texels over the lowered materials' maps,
//! the binary mask a metalness is. Left out by name (materials, triangles; a material may carry several): swatch tint
//! 568, 15,984; modulate 594, 8,747; deposits 554, 7,159; triplanar 568, 7,642; wetness 594, 11,139; relaxed cone-step
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
//! Left unlowered, with no rule yet (materials, triangles): `environmentdeferreddecal` 70, 1,549; `pylon_cables` 2,
//! 532; `environment_signage` 88, 520; `glass` 8, 188; `uber_roaddecal` 2, 146; `window_linecastroom` 51, 78;
//! `emissiveonly` 2, 64; and the `uber_foliage` billboard and impostor, 2, 4. The normal maps are made in
//! `NormalZ::Rebuild`: `normalize-normals` converts them to glTF's before a writer.
use crate::components::{ForzaMaterial, MaterialTextures, Value};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Component, Entity, Mesh, Name, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ,
    TexFormat, TexRole, Texture, World,
};
use primase_foundations::color::srgb_to_linear;
use std::collections::{BTreeMap, HashMap};

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
    /// Roughness in R and metalness in G (BC5, signed or not, or an sRGB view): G roughness, B metallic, R 1.
    RoughnessMetalRg,
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
            Derived::RoughnessMetalRg => ("roughness-metalness-in-rg", "roughness metallic"),
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
                    Derived::AlphaOcclusion => [a, a, a, 255],
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

    /// The `environment` family's material.
    fn environment(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<PbrMaterial, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        let offset = p.vec2("Uv0Offset").unwrap_or([0.0; 2]);
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffAlphaTex_A") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, p.vec2("BaseColorAlpha_A_UVTiling"), offset));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "base colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no base colour texture: white", triangles),
        }

        match p.texture("NormHeightTex_A") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(Self::slot(t, p.vec2("NormalHeight_A_UVTiling"), offset));
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        match p.texture("RoughMetAoPorTex_A") {
            Binding::Texture(t) => match self.derive(w, t, Derived::OcclusionRoughnessMetallic, log)? {
                Some(packed) => {
                    let slot = Self::slot(packed, p.vec2("RoughMetAoPoros_A_UVTiling"), offset);
                    pbr.metallic_roughness_texture = Some(slot);
                    pbr.occlusion_texture = Some(slot);
                    pbr.metallic = 1.0;
                    pbr.roughness = 1.0;
                }
                None => approximate(
                    log,
                    e,
                    "RoughMetAoPor map not lowered: dielectric, rough",
                    triangles,
                ),
            },
            Binding::Absent | Binding::None => {
                approximate(log, e, "no RoughMetAoPor map: dielectric, rough", triangles);
            }
        }
        if pbr.metallic_roughness_texture.is_none() {
            pbr.metallic = 0.0;
            pbr.roughness = 1.0;
        }

        if p.on("AlphaTest") {
            // the reference is a per-draw constant of the engine, not a parameter of the material
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: ALPHA_REFERENCE,
            };
            approximate(log, e, "alpha-test reference taken as one half", triangles);
        } else if p.on("AlphaBlend") {
            pbr.alpha_mode = AlphaMode::Blend;
        }

        for (switch, what) in [
            ("SwatchTint", "swatch tint left out"),
            ("Modulate", "modulate texture left out"),
            ("Deposits", "deposits left out"),
            ("TriPlanarMapping", "triplanar mapping left out"),
            ("RCSM", "relaxed cone-step mapping left out"),
            ("MegaSkirt", "mega-skirt terrain blend left out"),
            ("Emissive", "emission left out"),
            ("RetroReflective", "retro-reflection left out"),
            ("RandomOffsetOnOff", "per-instance random UV offset left out"),
            ("DualUv", "second UV set of a dual-UV material left out"),
            ("WetnessMask", "wetness (run-time) left out: dry"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(pbr)
    }

    /// The `uber_foliage` family's material: `None` for the billboard and impostor permutations, whose textures are
    /// atlases of views, not the leaf and bark maps.
    fn uber_foliage(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("Billboard") || p.on("Impostor") {
            return Ok(None);
        }
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseTexture") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(TextureRef::new(t));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "base colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no base colour texture: white", triangles),
        }

        // RT1.w is NormalTexture.a * RoughnessScale in the high-quality permutations; no metal
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        let scale = p.float("RoughnessScale").unwrap_or(1.0).max(0.0);
        match p.texture("NormalTexture") {
            Binding::Texture(t) => {
                let alpha = w
                    .get::<Texture>(t)
                    .is_some_and(|x| !matches!(x.format, TexFormat::Bc5Unorm | TexFormat::Bc5Snorm));
                if alpha {
                    if let Some(rough) = self.derive(w, t, Derived::AlphaRoughness(scale.to_bits()), log)? {
                        pbr.metallic_roughness_texture = Some(TextureRef::new(rough));
                    }
                } else {
                    approximate(log, e, "roughness map has no alpha channel: rough", triangles);
                }
                // the bush's normal is the map's red and green, brought to the world on the vertex tangent; the
                // tree's is a canopy normal of all four channels and the bounding box, which has no glTF form
                if p.on("Bush") {
                    if Self::normal_map(w, t, log) {
                        pbr.normal_texture = Some(TextureRef::new(t));
                    } else {
                        approximate(log, e, "normal map not lowered", triangles);
                    }
                } else {
                    approximate(log, e, "canopy normal left out", triangles);
                }
            }
            Binding::Absent | Binding::None => {
                approximate(log, e, "no normal and roughness map: rough", triangles)
            }
        }

        // the albedo is darkened by RTAOTexture.b through scene constants: taken whole as the occlusion
        match p.texture("RTAOTexture") {
            Binding::Texture(t) => {
                if let Some(occlusion) = self.derive(w, t, Derived::BlueOcclusion, log)? {
                    pbr.occlusion_texture = Some(TextureRef::new(occlusion));
                }
                approximate(log, e, "occlusion strength of the scene left out", triangles);
            }
            Binding::Absent | Binding::None => approximate(log, e, "no occlusion map", triangles),
        }

        // discard if sat((a - AlphaStepMin) / (AlphaStepMax - AlphaStepMin)) < reference, the reference a per-draw
        // constant: alpha at the point of the range the middle reference picks
        if alpha_tested(m) {
            let min = p.float("AlphaStepMin").unwrap_or(0.0);
            let max = p.float("AlphaStepMax").unwrap_or(1.0);
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: (min + ALPHA_REFERENCE * (max - min)).clamp(0.0, 1.0),
            };
            approximate(
                log,
                e,
                "alpha-test reference taken as one half of the step range",
                triangles,
            );
        }
        pbr.double_sided = p.on("DoubleSided");

        for (switch, what) in [
            ("Tint", "tint (colour lookup) left out"),
            ("UseWorldTint", "world tint left out"),
            ("ApplyWetness", "wetness (run-time) left out: dry"),
            ("Animated", "wind animation left out"),
            ("Vertex_AO", "vertex occlusion left out"),
            ("DistantColourBlend", "distant colour blend left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(Some(pbr))
    }

    /// The `uber_branch` family's material: `None` for a permutation that blends a second set of maps.
    fn uber_branch(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        if p.on("Blend2") {
            return Ok(None);
        }
        let mut pbr = PbrMaterial::default();
        let diffuse_tiling = p.vec2("DiffuseAUVTiling");
        let normal_tiling = p.vec2("NormalAUVTiling");

        // roughness is DiffuseA.a, so DiffuseA is the colour map and the roughness map's source at once
        pbr.metallic = 0.0;
        pbr.roughness = 1.0;
        match p.texture("DiffuseA") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(Self::slot(t, diffuse_tiling, [0.0; 2]));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
                if let Some(rough) = self.derive(w, t, Derived::AlphaRoughness(1.0f32.to_bits()), log)? {
                    pbr.metallic_roughness_texture = Some(Self::slot(rough, diffuse_tiling, [0.0; 2]));
                }
            }
            Binding::Absent => approximate(
                log,
                e,
                "base colour texture in no archive: white, rough",
                triangles,
            ),
            Binding::None => approximate(log, e, "no base colour texture: white, rough", triangles),
        }

        match p.texture("NormalA") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(Self::slot(t, normal_tiling, [0.0; 2]));
                    // z is rebuilt from the unscaled x and y, then x and y are scaled: glTF's scale
                    pbr.normal_scale = p.float("NormalAIntensity").unwrap_or(1.0);
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
                // NormalAlpha_AO: the map's alpha is the ambient occlusion
                if p.on("NormalAlpha_AO")
                    && let Some(occlusion) = self.derive(w, t, Derived::AlphaOcclusion, log)?
                {
                    pbr.occlusion_texture = Some(Self::slot(occlusion, normal_tiling, [0.0; 2]));
                    approximate(log, e, "occlusion strength of the scene left out", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        for (switch, what) in [
            ("MegaSkirt", "mega-skirt terrain blend left out"),
            ("Tint", "tint left out"),
            ("Moss", "moss left out"),
            ("MicroNormal", "micro normal left out"),
            ("Animated", "wind animation left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        approximate(
            log,
            e,
            "vertex occlusion and wetness (run-time) left out",
            triangles,
        );
        Ok(Some(pbr))
    }

    /// The `projection_lod` family's material: the atlas cell of a building's far view.
    fn projection_lod(
        &mut self,
        w: &mut World,
        e: Entity,
        m: &ForzaMaterial,
        log: &mut PassLog,
    ) -> Result<Option<PbrMaterial>, String> {
        let bound = w.get::<MaterialTextures>(e).cloned();
        let p = Params::new(m, bound.as_ref());
        let triangles = self.triangles.get(&e).copied().unwrap_or(0);
        // every map is read at `UVOutMinMax.xy + (zw - xy) * uv0`
        let cell = p.vec4("UVOutMinMax").map(|c| UvTransform {
            offset: [c[0], c[1]],
            scale: [c[2] - c[0], c[3] - c[1]],
            rotation: 0.0,
        });
        let slot = |texture: Entity| TextureRef {
            transform: cell.filter(|t| *t != UvTransform::IDENTITY),
            ..TextureRef::new(texture)
        };
        if cell.is_none() {
            approximate(
                log,
                e,
                "no atlas cell (UVOutMinMax): the whole texture",
                triangles,
            );
        }
        let mut pbr = PbrMaterial::default();

        match p.texture("DiffuseATexture") {
            Binding::Texture(t) => {
                pbr.base_color_texture = Some(slot(t));
                if let Some(tex) = w.get_mut::<Texture>(t)
                    && tex.role == TexRole::Other
                    && tex.color_space == ColorSpace::Srgb
                {
                    tex.role = TexRole::Color;
                }
            }
            Binding::Absent => approximate(log, e, "base colour texture in no archive: white", triangles),
            Binding::None => approximate(log, e, "no base colour texture: white", triangles),
        }

        // RoughnessMetal: roughness is RoughnessMetalnessATexture.r and metalness .g
        pbr.metallic = 0.0;
        pbr.roughness = p.float("Roughness").unwrap_or(1.0).clamp(0.0, 1.0);
        if p.on("RoughnessMetal") {
            match p.texture("RoughnessMetalnessATexture") {
                Binding::Texture(t) => {
                    if let Some(rm) = self.derive(w, t, Derived::RoughnessMetalRg, log)? {
                        pbr.metallic_roughness_texture = Some(slot(rm));
                        pbr.metallic = 1.0;
                        pbr.roughness = 1.0;
                    }
                }
                Binding::Absent | Binding::None => {
                    approximate(
                        log,
                        e,
                        "no roughness-metalness map: dielectric, the constant roughness",
                        triangles,
                    );
                }
            }
        }

        // the HiQ permutation reads NormalATexture's red and green; the others use the vertex normal
        match p.texture("NormalATexture") {
            Binding::Texture(t) => {
                if Self::normal_map(w, t, log) {
                    pbr.normal_texture = Some(slot(t));
                } else {
                    approximate(log, e, "normal map not lowered", triangles);
                }
            }
            Binding::Absent | Binding::None => approximate(log, e, "no normal map", triangles),
        }

        // discard if DiffuseATexture.a < reference, the reference a per-draw constant
        if alpha_tested(m) {
            pbr.alpha_mode = AlphaMode::Mask {
                cutoff: ALPHA_REFERENCE,
            };
            approximate(log, e, "alpha-test reference taken as one half", triangles);
        }

        // the albedo's factor 1 - DiffuseA.a (1 - BakedVCTexture.r) is a baked occlusion from two textures
        approximate(
            log,
            e,
            "baked occlusion (BakedVCTexture masked by the diffuse alpha) left out",
            triangles,
        );
        for (switch, what) in [
            ("Tint", "swatch tint left out"),
            ("Emissive", "emission left out"),
            ("Lightmap", "lightmap left out"),
        ] {
            left_out(log, &p, e, switch, what, triangles);
        }
        Ok(Some(pbr))
    }
}

/// The alpha-test reference of a masked material, in the absence of one in the material: the middle of the range.
const ALPHA_REFERENCE: f32 = 0.5;

/// Whether the material is alpha tested: the parent material's `ATST` flag (the instance's when it has no parent).
fn alpha_tested(m: &ForzaMaterial) -> bool {
    m.material.as_ref().map_or(m.instance.atst, |x| x.atst)[0] != 0
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
        &[PbrMaterial::NAME, Texture::NAME, Name::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    fn approximates(&self) -> &'static str {
        "environment materials by their texture maps alone: the swatch tint, modulate, deposits, triplanar, \
         parallax and emission of a permutation are left out, wetness and lighting are run-time state"
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
        let materials: Vec<(Entity, ForzaMaterial)> = world
            .iter::<ForzaMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut lowering = Lowering {
            triangles,
            derived: BTreeMap::new(),
        };
        let mut left: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for (e, m) in &materials {
            let family = m.shader.as_ref().map(|s| s.family.as_str());
            let lowered = match family {
                Some("environment") => Some(lowering.environment(world, *e, m, log)?),
                Some("uber_foliage") => lowering.uber_foliage(world, *e, m, log)?,
                Some("uber_branch") => lowering.uber_branch(world, *e, m, log)?,
                Some("projection_lod") => lowering.projection_lod(world, *e, m, log)?,
                _ => None,
            };
            let Some(pbr) = lowered else {
                let name = match family {
                    Some("uber_foliage") => "uber_foliage billboard or impostor",
                    Some("uber_branch") => "uber_branch with a second set of maps",
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
