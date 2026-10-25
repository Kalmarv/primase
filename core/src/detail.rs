//! A material's surface as its base and tiled detail textures combine (component `pbr.detail`), for a target that
//! composites detail at draw time instead of taking it baked.
//!
//! It sits beside the material's `pbr.material`, which stays the complete bake (the detail baked in at a capped size):
//! the host and the fallback for every target that does not draw this. The textures are the game's own, each sampled
//! with its own UV set, wrap, transform (its tiling) and filters ([`TextureRef`]), their lanes as the game's program
//! reads them (a base normal's alpha may be a mask). Nothing here is baked.
//!
//! The combination is a closed set of equations ([`DetailMix`]), each the one a family of programs computes, added only
//! where every program said to compute it is shown to on the CPU executor (the equation against the program's decoded
//! outputs, over random texture samples). Its outputs are the surface the equation yields ([`DetailSurface`]), the mix
//! happening before any lighting model; a closed set of lowerings ([`SurfaceLowering`]) says how a G-buffer holds that
//! surface ([`SurfaceLowering::written`]: the roles a program writes, what the proof compares) and makes it the PBR
//! model's ([`SurfaceLowering::evaluate`]), the same lowering the source's bake applies, so every consumer converts
//! through core.
use serde::Serialize;
use serde_json::Value;

use crate::layered::{Source, check_source, mask_value};
use crate::pbr::TextureRef;
use crate::shader::ScalarEncoding;
use crate::{Component, Entity, JsonError, World};

/// A material's detail form (component `pbr.detail`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DetailedMaterial {
    pub mix: DetailMix,
    /// How the mix's outputs become the PBR model.
    pub lowering: SurfaceLowering,
    /// What the equation holds constant that the source varies per draw, each at the value the source's bake freezes
    /// it to (the surface front-facing, decals and wear off): what a consumer shows beside it, and what holds for the
    /// equation to be the program's.
    pub frozen: Vec<Frozen>,
}

/// One thing a [`DetailedMaterial`]'s equation holds constant ([`DetailedMaterial::frozen`]): a closed set, each with
/// its value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Frozen {
    /// The surface drawn front-facing: the program's facing input true.
    FrontFacing,
    /// A vertex input the program reads, held at `value` (lanes past the input's own 0), by its semantic name and
    /// index.
    VertexInput {
        semantic: String,
        index: u32,
        value: [f32; 4],
    },
    /// A constant the source's engine sets per draw, held at `value` (its words as floats), by its name in the
    /// program.
    EngineConstant { name: String, value: Vec<f32> },
    /// A texture the source's engine binds per draw, read as the constant `value`, by its name in the program.
    EngineTexture { name: String, value: [f32; 4] },
}

impl Frozen {
    /// Its values all finite.
    fn finite(&self) -> bool {
        match self {
            Frozen::FrontFacing => true,
            Frozen::VertexInput { value, .. } | Frozen::EngineTexture { value, .. } => {
                value.iter().all(|v| v.is_finite())
            }
            Frozen::EngineConstant { value, .. } => value.iter().all(|v| v.is_finite()),
        }
    }
}

/// The equations a detail material is drawn by.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailMix {
    DetailLerp(DetailLerp),
}

/// The model's revision: the version of the component and the revision the glTF extension states
/// (`PRIMASE_materials_detail_blend`). 4 adds the base colour's desaturation, the `max`, `affine` and `saturate` weight
/// steps and the `metallic_gloss` lowering, and moves the dielectric reflectance into the `gloss_reflectance` lowering;
/// 3, the earlier shape, is read as a subset.
pub const REVISION: u32 = 4;

/// The most details a [`DetailLerp`] chains: this model's own bound, the largest chain an evaluator is built for (the
/// texture bindings of a detail material grow with it: three for the base and two for each detail). The programs it
/// was derived from read one to three.
pub const DETAIL_LIMIT: usize = 3;

/// The weights of the luma a program desaturates the base colour toward ([`DetailLerp::base_saturation`]): the values the
/// programs write, which sum to 1.
pub const LUMA: [f32; 3] = [0.29891, 0.58661, 0.11448];

/// Tinted details lerped in turn over a base, as the detail-blend programs this model was derived from compute them. With
/// `A`, `N`, `M` the base colour, normal and mask samples, `D_i`, `K_i` the detail colour and normal samples,
/// `lerp(a, b, t) = a + (b - a) * t` (lane by lane) and `chain(x)` the details' values `x_0, x_1, ...` lerped in turn by
/// the weights (`x_0`, then `lerp(.., x_1, w_1)`, then `lerp(.., x_2, w_2)`):
///
/// - `A' = lerp(A.rgb, luma(A.rgb), base_saturation)` with `luma(x) = dot(x, `[`LUMA`]`)`; `A.rgb` where it is 0
/// - `C = saturate(gain * A'.rgb * chain(tint_i * D_i.rgb))`
/// - `f = saturate(chain(r_i))`, each detail's reflectance term `r_i` as it states ([`ReflectanceTerm`])
/// - the surface is `C`, `colour` and `f`; the lowering says what a G-buffer holds of it: albedo `(1 - f) * C * colour`
///   and reflectance (F0) `lerp(dielectric_f0, C, f)` ([`SurfaceLowering::GlossReflectance`]), or albedo `C * colour` and
///   metallic `f` ([`SurfaceLowering::MetallicGloss`])
/// - normal: `xy = 2 * N.xy - 1`, `z = sqrt(1 - min(dot(xy, xy), 1))` of the base's `xy`, then `xy += normal_strength *
///   (2 * chain(K_i.xy) - 1)`: tangent space, unnormalised
/// - gloss: `saturate(2 * chain(s_i) * chain(K_i.z + gloss_bias_i))` as the program stores it, each detail's scale `s_i`
///   as it states ([`GlossScale`]), decoded by `gloss_encoding` (the smoothness the lighting reads)
/// - alpha `saturate(D_alpha.a * A.a)` of the detail `alpha` states, where the program's alpha has a role; 1 where it
///   has none
///
/// One detail is no lerp at all. The operations are [`DetailLerp::evaluate`]'s, in its order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DetailLerp {
    pub base_colour: TextureRef,
    pub base_normal: TextureRef,
    pub base_mask: TextureRef,
    /// How far the base colour is desaturated toward its luma before the details are multiplied over it (0: not at all,
    /// 1: grey).
    pub base_saturation: f32,
    /// One to [`DETAIL_LIMIT`], in the chain's order.
    pub details: Vec<Detail>,
    /// The weight each detail after the first is lerped in by, per pixel ([`Weight`]): one fewer than the details.
    pub weights: Vec<Weight>,
    pub gain: f32,
    pub normal_strength: f32,
    /// Linear RGB the albedo is multiplied by.
    pub colour: [f32; 3],
    /// How the program stores the gloss in its G-buffer (the binding's `Gloss` encoding).
    pub gloss_encoding: ScalarEncoding,
    /// The detail whose colour's alpha, times the base colour's, is the surface's alpha: where the program writes its
    /// alpha to a lane with a role (the binding's `Coverage` or `Opacity`). `None` where that lane has no role (no blend
    /// weighs by it, no discard tests it): the surface is opaque.
    pub alpha: Option<usize>,
}

/// The largest exponent of a [`WeightStep::Power`].
pub const WEIGHT_POWER_LIMIT: u16 = 512;

/// A detail's weight per pixel: its source's value, then each step in turn.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Weight {
    pub source: Source,
    pub steps: Vec<WeightStep>,
}

/// One step of a [`Weight`]: a closed set, each a form a program is shown to compute.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightStep {
    /// `x^exponent`, 1 to [`WEIGHT_POWER_LIMIT`], as [`weight_power`] takes it.
    Power(u16),
    /// `min(x, bound)`, the bound where `x` is NaN (D3D's `min`).
    Min(f32),
    /// `max(x, bound)`, the bound where `x` is NaN (D3D's `max`): `max(x, 0)` before a power keeps a program's `pow` of a
    /// negative value (NaN, which its `saturate` makes 0) the same 0 here.
    Max(f32),
    /// `x * scale + bias`, the product rounded before the sum.
    Affine { scale: f32, bias: f32 },
    /// `clamp(x, 0, 1)`, 0 where `x` is NaN (D3D's `saturate`).
    Saturate,
}

/// `x^exponent` by squaring, as a program writes a power: `x`, `x^2`, `x^4`, ... each the square of the one before, and
/// the powers of the exponent's set bits multiplied in from the lowest (`x^260 = x^4 * x^256`; a power of two is its
/// squarings alone).
pub fn weight_power(x: f32, exponent: u16) -> f32 {
    let mut square = x;
    let mut out: Option<f32> = None;
    for bit in 0..16 {
        if exponent >> bit == 0 {
            break;
        }
        if exponent & (1 << bit) != 0 {
            out = Some(out.map_or(square, |acc| acc * square));
        }
        square *= square;
    }
    out.unwrap_or(1.0)
}

/// `steps` of `v`, in turn.
pub fn weight_steps(v: f32, steps: &[WeightStep]) -> f32 {
    steps.iter().fold(v, |x, step| match *step {
        WeightStep::Power(exponent) => weight_power(x, exponent),
        WeightStep::Min(bound) => x.min(bound),
        WeightStep::Max(bound) => x.max(bound),
        WeightStep::Affine { scale, bias } => x * scale + bias,
        // as a GPU's saturate: NaN is 0
        WeightStep::Saturate => if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) },
    })
}

impl Weight {
    /// Its value for its source's value `v`.
    pub fn evaluate(&self, v: f32) -> f32 {
        weight_steps(v, &self.steps)
    }

    /// Its value for the samples of the base normal, where its source is a mask of it.
    pub fn of_sample(&self, sample: [f32; 4]) -> Option<f32> {
        match self.source {
            Source::Mask { channels, bias, .. } => Some(self.evaluate(mask_value(channels, bias, sample))),
            Source::Vertex { .. } => None,
        }
    }

    /// Its own checks (its source's apart): each power's exponent 1 to [`WEIGHT_POWER_LIMIT`], each bound finite.
    pub fn check(&self) -> Result<(), String> {
        for step in &self.steps {
            match *step {
                WeightStep::Power(e) if !(1..=WEIGHT_POWER_LIMIT).contains(&e) => {
                    return Err(format!("power exponent {e} is not 1 to {WEIGHT_POWER_LIMIT}"));
                }
                WeightStep::Min(bound) if !bound.is_finite() => {
                    return Err(format!("min bound {bound} is not finite"));
                }
                WeightStep::Max(bound) if !bound.is_finite() => {
                    return Err(format!("max bound {bound} is not finite"));
                }
                WeightStep::Affine { scale, bias } if !(scale.is_finite() && bias.is_finite()) => {
                    return Err(format!("affine step {scale} * x + {bias} is not finite"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// What a detail's gloss term is scaled by before the doubling: a closed set, each a form a program is shown to compute.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossScale {
    /// The base normal's z sample.
    BaseNormalZ,
    /// A constant.
    Constant(f32),
}

impl GlossScale {
    /// Its value for the base normal sample `normal`.
    pub fn value(self, normal: [f32; 4]) -> f32 {
        match self {
            GlossScale::BaseNormalZ => normal[2],
            GlossScale::Constant(c) => c,
        }
    }
}

/// One detail of a [`DetailLerp`]: its textures and constants.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Detail {
    pub colour: TextureRef,
    pub normal: TextureRef,
    /// Linear RGB its colour is multiplied by.
    pub tint: [f32; 3],
    /// Added to its normal's z in the gloss.
    pub gloss_bias: f32,
    /// What its gloss term is scaled by.
    pub gloss_scale: GlossScale,
    /// Its term of the reflectance mix `f`.
    pub reflectance: ReflectanceTerm,
}

/// A detail's term of the reflectance mix, as the program computes it: a closed set, each a form a program is shown to
/// compute.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectanceTerm {
    /// The base mask's x plus the bias.
    MaskPlusBias(f32),
    /// The bias alone.
    Bias(f32),
}

impl ReflectanceTerm {
    /// Its value for the base mask sample `mask`.
    pub fn value(self, mask: [f32; 4]) -> f32 {
        match self {
            ReflectanceTerm::MaskPlusBias(bias) => mask[0] + bias,
            ReflectanceTerm::Bias(bias) => bias,
        }
    }

    /// Its bias.
    pub fn bias(self) -> f32 {
        match self {
            ReflectanceTerm::MaskPlusBias(bias) | ReflectanceTerm::Bias(bias) => bias,
        }
    }
}

/// The texture samples of one pixel of a [`DetailLerp`], each a texture's four lanes as sampled.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailSamples {
    pub base_colour: [f32; 4],
    pub base_normal: [f32; 4],
    pub base_mask: [f32; 4],
    /// Each detail's colour and normal, in the chain's order.
    pub details: Vec<DetailSample>,
}

/// One detail's samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailSample {
    pub colour: [f32; 4],
    pub normal: [f32; 4],
}

/// One pixel's surface, before any G-buffer or PBR lowering: what the equation yields. A lowering says what a G-buffer
/// holds of it ([`SurfaceLowering::written`]) and what the PBR model is ([`SurfaceLowering::evaluate`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailSurface {
    /// Linear RGB: the details' colour `C`, before the material's colour.
    pub c: [f32; 3],
    /// Linear RGB the material multiplies the albedo by.
    pub colour: [f32; 3],
    /// The mix `f` between the dielectric and the colour, 0 to 1 (the metal).
    pub mix: f32,
    pub alpha: f32,
    /// Tangent space, unnormalised (the program normalises after its tangent frame).
    pub normal: [f32; 3],
    /// Smoothness, 1 - perceptual roughness.
    pub gloss: f32,
}

/// The roles a program's G-buffer holds of a [`DetailSurface`] under a [`SurfaceLowering`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Written {
    /// Linear RGB: the diffuse colour.
    pub albedo: [f32; 3],
    /// Linear RGB reflectance at normal incidence (F0), where the G-buffer stores one.
    pub reflectance: Option<[f32; 3]>,
    /// The metal factor, where the G-buffer stores one.
    pub metallic: Option<f32>,
}

/// How a [`DetailSurface`] is held by a G-buffer and becomes the PBR model: a closed set, each the lowering a source's
/// bake applies to the same surface. Both are the same statement (a mix between a dielectric and the colour); they
/// differ in what the source's G-buffer stores of it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceLowering {
    /// A G-buffer that stores an F0 reflectance: albedo `(1 - f) * C * colour` and F0 `lerp(dielectric_f0, C, f)`. The PBR
    /// model: the base colour the albedo, roughness `1 - gloss`, metallic 0 (the reflectance carries the metal), and the
    /// F0 as the specular colour glTF's `KHR_materials_specular` states with a colour factor of 25 (its F0 is
    /// `0.04 * factor * colour`).
    GlossReflectance {
        /// The reflectance (F0) of the dielectric part, where `f` is 0.
        dielectric_f0: f32,
    },
    /// A G-buffer that stores a metallic factor: albedo `C * colour` and metallic `f`. The PBR model: the base colour the
    /// albedo, metallic `f`, roughness `1 - gloss`, and no specular colour (the dielectric reflectance is the consumer's:
    /// glTF's 0.04, which is a choice the source's lighting may not make, so a writer reports it).
    MetallicGloss,
}

/// One pixel of the PBR model ([`SurfaceLowering::evaluate`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrPixel {
    /// Linear RGB and alpha.
    pub base_color: [f32; 4],
    /// Tangent space, unit length (NaN where the surface's normal is zero, as a program's normalise gives).
    pub normal: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// The specular colour, `KHR_materials_specular`'s `specularColorTexture` value, with its colour factor
    /// ([`SPECULAR_COLOR_FACTOR`]); `None` where the lowering states none.
    pub specular_color: Option<[f32; 3]>,
}

/// The `specularColorFactor` the F0 of [`SurfaceLowering::GlossReflectance`] is written with: glTF's F0 is
/// `0.04 * factor * colour`, so 25 makes the colour the F0.
pub const SPECULAR_COLOR_FACTOR: f32 = 25.0;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `x` lerped in turn by `weights`: the first value, then each next one lerped over the result by its weight.
pub fn chain(x: impl IntoIterator<Item = f32>, weights: &[f32]) -> f32 {
    let mut x = x.into_iter();
    let first = x.next().unwrap_or(0.0);
    x.zip(weights).fold(first, |acc, (next, &w)| lerp(acc, next, w))
}

impl DetailLerp {
    /// The textures it samples: the base's, each detail's colour and normal, and those of its weights after them.
    pub fn textures(&self) -> Vec<&TextureRef> {
        let mut out = vec![&self.base_colour, &self.base_normal, &self.base_mask];
        for d in &self.details {
            out.extend([&d.colour, &d.normal]);
        }
        out.extend(self.weights.iter().filter_map(|w| match &w.source {
            Source::Mask { texture, .. } => Some(texture),
            Source::Vertex { .. } => None,
        }));
        out
    }

    /// The weights' values for the values of their sources.
    pub fn weight_values(&self, sources: &[f32]) -> Vec<f32> {
        self.weights
            .iter()
            .zip(sources)
            .map(|(w, &v)| w.evaluate(v))
            .collect()
    }

    /// One pixel's surface from its samples and the values of its weights ([`DetailLerp::weight_values`]).
    pub fn evaluate(&self, s: &DetailSamples, weights: &[f32]) -> DetailSurface {
        let (a, n, m) = (s.base_colour, s.base_normal, s.base_mask);
        let pairs = || self.details.iter().zip(&s.details);
        let base: [f32; 3] = if self.base_saturation == 0.0 {
            [a[0], a[1], a[2]]
        } else {
            let luma = a[0] * LUMA[0] + a[1] * LUMA[1] + a[2] * LUMA[2];
            std::array::from_fn(|i| lerp(a[i], luma, self.base_saturation))
        };
        let c: [f32; 3] = std::array::from_fn(|i| {
            let detail = chain(pairs().map(|(d, x)| d.tint[i] * x.colour[i]), weights);
            (self.gain * base[i] * detail).clamp(0.0, 1.0)
        });
        let f = chain(self.details.iter().map(|d| d.reflectance.value(m)), weights).clamp(0.0, 1.0);
        let base = [2.0 * n[0] - 1.0, 2.0 * n[1] - 1.0];
        let z = (1.0 - (base[0] * base[0] + base[1] * base[1]).min(1.0)).sqrt();
        let detail = |i: usize| 2.0 * chain(s.details.iter().map(|x| x.normal[i]), weights) - 1.0;
        let normal = [
            base[0] + self.normal_strength * detail(0),
            base[1] + self.normal_strength * detail(1),
            z,
        ];
        let scale = chain(self.details.iter().map(|d| d.gloss_scale.value(n)), weights);
        let stored =
            (2.0 * scale * chain(pairs().map(|(d, x)| x.normal[2] + d.gloss_bias), weights)).clamp(0.0, 1.0);
        let gloss = self.gloss_encoding.decode(stored);
        let alpha = self
            .alpha
            .and_then(|j| s.details.get(j))
            .map_or(1.0, |x| (x.colour[3] * a[3]).clamp(0.0, 1.0));
        DetailSurface {
            c,
            colour: self.colour,
            mix: f,
            alpha,
            normal,
            gloss,
        }
    }

    /// Its constants all finite.
    fn constants_finite(&self) -> bool {
        let scale = |d: &Detail| match d.gloss_scale {
            GlossScale::Constant(c) => c,
            GlossScale::BaseNormalZ => 0.0,
        };
        self.details
            .iter()
            .flat_map(|d| {
                d.tint
                    .into_iter()
                    .chain([d.gloss_bias, d.reflectance.bias(), scale(d)])
            })
            .chain(self.colour)
            .chain([self.gain, self.normal_strength, self.base_saturation])
            .all(f32::is_finite)
    }

    /// One to [`DETAIL_LIMIT`] details, one weight fewer, each weight bounded, the constants finite, the alpha's detail
    /// one of them and the gloss encoding sound.
    fn check(&self) -> Result<(), String> {
        if !(1..=DETAIL_LIMIT).contains(&self.details.len()) {
            return Err(format!(
                "{} details: a detail lerp chains 1 to {DETAIL_LIMIT}",
                self.details.len()
            ));
        }
        if self.weights.len() + 1 != self.details.len() {
            return Err(format!(
                "{} weights for {} details: one fewer than the details",
                self.weights.len(),
                self.details.len()
            ));
        }
        if !self.constants_finite() {
            return Err("a detail mix constant is not finite".into());
        }
        for w in &self.weights {
            w.check().map_err(|e| format!("weight: {e}"))?;
        }
        if let Some(j) = self.alpha
            && j >= self.details.len()
        {
            return Err(format!(
                "the alpha is detail {j}'s, of {} details",
                self.details.len()
            ));
        }
        self.gloss_encoding
            .check()
            .map_err(|e| format!("gloss encoding: {e}"))
    }
}

impl DetailMix {
    /// The textures it samples, its weights' included.
    pub fn textures(&self) -> Vec<&TextureRef> {
        match self {
            DetailMix::DetailLerp(mix) => mix.textures(),
        }
    }
}

impl SurfaceLowering {
    /// The roles a G-buffer under this lowering holds of `surface`: what a program that writes them writes, and what the
    /// proof compares with the program's decoded outputs.
    pub fn written(self, surface: &DetailSurface) -> Written {
        let (c, f) = (surface.c, surface.mix);
        match self {
            SurfaceLowering::GlossReflectance { dielectric_f0 } => Written {
                albedo: std::array::from_fn(|i| (1.0 - f) * c[i] * surface.colour[i]),
                reflectance: Some(c.map(|c| lerp(dielectric_f0, c, f))),
                metallic: None,
            },
            SurfaceLowering::MetallicGloss => Written {
                albedo: std::array::from_fn(|i| c[i] * surface.colour[i]),
                reflectance: None,
                metallic: Some(f),
            },
        }
    }

    /// The PBR model's pixel for `surface`.
    pub fn evaluate(self, surface: &DetailSurface) -> PbrPixel {
        let written = self.written(surface);
        let [r, g, b] = written.albedo;
        let n = surface.normal;
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        PbrPixel {
            base_color: [r, g, b, surface.alpha],
            normal: n.map(|x| x / length),
            roughness: 1.0 - surface.gloss,
            metallic: written.metallic.unwrap_or(0.0),
            specular_color: written.reflectance,
        }
    }

    /// Its constants finite.
    fn check(self) -> Result<(), String> {
        match self {
            SurfaceLowering::GlossReflectance { dielectric_f0 } if !dielectric_f0.is_finite() => {
                Err(format!("dielectric reflectance {dielectric_f0} is not finite"))
            }
            _ => Ok(()),
        }
    }
}

impl Component for DetailedMaterial {
    const NAME: &'static str = "pbr.detail";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    const VERSION: u32 = REVISION;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Its constants and texture transforms finite, its textures alive, its details 1 to [`DETAIL_LIMIT`] and its weights
    /// bounded channels of sources that are, one fewer.
    fn verify(&self, _e: Entity, world: &World) -> Result<(), String> {
        if let Some(f) = self.frozen.iter().find(|f| !f.finite()) {
            return Err(format!("frozen {f:?} is not finite"));
        }
        self.lowering.check()?;
        match &self.mix {
            DetailMix::DetailLerp(mix) => {
                mix.check()?;
                for w in &mix.weights {
                    check_source(world, "weight", &w.source)?;
                }
            }
        }
        for t in self.mix.textures() {
            if !world.is_alive(t.texture) {
                return Err(format!("texture {:?} is not alive", t.texture));
            }
            if let Some(x) = t.transform
                && !x
                    .offset
                    .iter()
                    .chain(&x.scale)
                    .chain([&x.rotation])
                    .all(|v| v.is_finite())
            {
                return Err(format!("texture transform {x:?} is not finite"));
            }
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.mix.textures().into_iter().map(|t| t.texture));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layered::{Channel, power};

    /// `(1 - N.a)^8` (`flip`: `N.a^8`) of the base normal `texture`.
    fn weight(texture: TextureRef, flip: bool) -> Weight {
        Weight {
            source: Source::Mask {
                texture,
                channels: [0., 0., 0., if flip { 1. } else { -1. }],
                bias: if flip { 0. } else { 1. },
            },
            steps: vec![WeightStep::Power(8)],
        }
    }

    /// A detail lerp of `n` details with distinct constants; the second detail's weight `(1 - N.a)^8`, the third's
    /// `N.a^8`, as a source program that squares three times.
    fn mix(world: &mut World, n: usize) -> DetailLerp {
        let mut t = || TextureRef::new(world.spawn());
        let (base_colour, base_normal, base_mask) = (t(), t(), t());
        let tints = [[1., 0.5, 0.25], [0.5, 1., 1.], [0.25, 0.75, 2.]];
        let reflectance = [
            ReflectanceTerm::MaskPlusBias(0.125),
            ReflectanceTerm::Bias(0.75),
            ReflectanceTerm::Bias(0.375),
        ];
        let details = (0..n)
            .map(|i| Detail {
                colour: t(),
                normal: t(),
                tint: tints[i],
                gloss_bias: [0.25, -0.25, 0.5][i],
                gloss_scale: GlossScale::BaseNormalZ,
                reflectance: reflectance[i],
            })
            .collect();
        DetailLerp {
            weights: [weight(base_normal, false), weight(base_normal, true)][..n - 1].to_vec(),
            base_colour,
            base_normal,
            base_mask,
            base_saturation: 0.,
            details,
            gain: 4.55,
            normal_strength: 0.5,
            colour: [1., 1., 0.5],
            gloss_encoding: ScalarEncoding::LINEAR,
            alpha: Some(0),
        }
    }

    fn samples(n: usize) -> DetailSamples {
        let details = [
            ([0.5, 0.5, 0.5, 1.], [0.5, 0.5, 0.75, 0.]),
            ([1., 0.25, 0.5, 0.], [1., 0., 0.25, 0.]),
            ([0.25, 0.75, 0.125, 0.5], [0.25, 0.75, 0.5, 0.]),
        ];
        DetailSamples {
            base_colour: [0.1, 0.2, 0.05, 0.5],
            base_normal: [0.75, 0.25, 0.5, 0.75],
            base_mask: [0.5, 0., 0., 0.],
            details: details[..n]
                .iter()
                .map(|&(colour, normal)| DetailSample { colour, normal })
                .collect(),
        }
    }

    /// Each output of the equation as its doc states it, for one pixel's samples of one, two and three details: one
    /// detail no lerp, two lerped by `(1 - N.a)^8` (by squaring), three lerped on by `N.a^8`.
    #[test]
    fn details_are_lerped_in_turn_over_the_base() {
        let mut world = World::new();
        for n in 1..=3 {
            let mix = mix(&mut world, n);
            let s = samples(n);
            let a = s.base_normal[3];
            let sources = [1. - a, a];
            let weights = mix.weight_values(&sources[..n - 1]);
            if n >= 2 {
                assert_eq!(weights[0].to_bits(), power(1. - a, 8).to_bits());
                assert_eq!(
                    weights[0].to_bits(),
                    (((0.25f32 * 0.25) * (0.25 * 0.25)) * ((0.25 * 0.25) * (0.25 * 0.25))).to_bits()
                );
            }
            if n == 3 {
                assert_eq!(weights[1].to_bits(), power(a, 8).to_bits());
            }
            // the chain written out: each next value lerped over the result so far
            let by_hand = |x: &[f32]| -> f32 {
                let mut acc = x[0];
                for (next, w) in x[1..].iter().zip(&weights) {
                    acc = acc + (next - acc) * w;
                }
                acc
            };
            let out = mix.evaluate(&s, &weights);
            let written = SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 }.written(&out);
            let c: [f32; 3] = std::array::from_fn(|i| {
                let x: Vec<f32> = (0..n)
                    .map(|j| mix.details[j].tint[i] * s.details[j].colour[i])
                    .collect();
                (mix.gain * s.base_colour[i] * by_hand(&x)).clamp(0., 1.)
            });
            let terms: Vec<f32> = [0.5 + 0.125, 0.75, 0.375][..n].to_vec();
            let f = by_hand(&terms).clamp(0., 1.);
            assert_eq!((out.c, out.mix, out.colour), (c, f, mix.colour), "{n} details");
            for (i, c) in c.iter().enumerate() {
                assert_eq!(written.albedo[i], (1. - f) * c * mix.colour[i], "{n} details");
                assert_eq!(written.reflectance.map(|r| r[i]), Some(0.03 + (c - 0.03) * f), "{n} details");
                assert_eq!(written.metallic, None);
            }
            let k = |i: usize| {
                let x: Vec<f32> = s.details.iter().map(|d| d.normal[i]).collect();
                2. * by_hand(&x) - 1.
            };
            assert_eq!(
                out.normal,
                [0.5 + 0.5 * k(0), -0.5 + 0.5 * k(1), (1f32 - 0.5).sqrt()],
                "{n} details"
            );
            let g: Vec<f32> = (0..n)
                .map(|j| s.details[j].normal[2] + mix.details[j].gloss_bias)
                .collect();
            assert_eq!(out.gloss, (2. * 0.5 * by_hand(&g)).clamp(0., 1.), "{n} details");
            assert_eq!(out.alpha, 0.5);
        }
        // one detail is no lerp: its own values exactly
        let one = mix(&mut world, 1);
        let s = samples(1);
        let out = one.evaluate(&s, &[]);
        let f = 0.5 + 0.125;
        let written = SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 }.written(&out);
        assert_eq!(
            written.reflectance.map(|r| r[0]),
            Some(0.03 + ((4.55 * 0.1 * (1. * 0.5)) - 0.03) * f)
        );
    }

    /// A base saturation of 0 leaves the base colour as it is (the same bits as before the term existed); otherwise the
    /// base colour is lerped toward its luma before the details are multiplied over it.
    #[test]
    fn the_base_colour_is_desaturated_toward_its_luma() {
        let mut world = World::new();
        let mut one = mix(&mut world, 1);
        let s = samples(1);
        let plain = one.evaluate(&s, &[]);
        let a = s.base_colour;
        let (gain, tint) = (one.gain, one.details[0].tint);
        let by_hand = |saturation: f32| -> [f32; 3] {
            let luma = a[0] * LUMA[0] + a[1] * LUMA[1] + a[2] * LUMA[2];
            std::array::from_fn(|i| {
                let base = a[i] + (luma - a[i]) * saturation;
                (gain * base * (tint[i] * s.details[0].colour[i])).clamp(0., 1.)
            })
        };
        let none: [f32; 3] =
            std::array::from_fn(|i| (gain * a[i] * (tint[i] * s.details[0].colour[i])).clamp(0., 1.));
        assert_eq!(plain.c, none);
        for saturation in [0.25, 1.] {
            one.base_saturation = saturation;
            assert_eq!(one.evaluate(&s, &[]).c, by_hand(saturation), "{saturation}");
        }
        assert!((LUMA.iter().sum::<f32>() - 1.).abs() < 1e-6);
        // a grey base colour is its own luma
        let mut grey = samples(1);
        grey.base_colour = [0.25, 0.25, 0.25, 1.];
        let desaturated = one.evaluate(&grey, &[]);
        one.base_saturation = 0.;
        let kept = one.evaluate(&grey, &[]);
        for i in 0..3 {
            assert!((kept.c[i] - desaturated.c[i]).abs() <= 1e-6, "lane {i}");
        }
    }

    /// The `max`, `affine` and `saturate` steps are taken as they read, in turn, and a NaN is the bound (`max`) or 0
    /// (`saturate`) as a D3D program's is.
    #[test]
    fn the_ramp_steps_are_taken_in_turn() {
        let ramp = [
            WeightStep::Affine { scale: 2., bias: -0.5 },
            WeightStep::Saturate,
        ];
        assert_eq!(weight_steps(0.375, &ramp), 0.25);
        assert_eq!(weight_steps(0.125, &ramp), 0.);
        assert_eq!(weight_steps(1., &ramp), 1.);
        assert_eq!(weight_steps(f32::NAN, &ramp), 0.);
        assert_eq!(weight_steps(-1., &[WeightStep::Max(0.), WeightStep::Power(8)]), 0.);
        assert_eq!(weight_steps(f32::NAN, &[WeightStep::Max(0.)]), 0.);
        assert_eq!(weight_steps(0.5, &[WeightStep::Max(0.75)]), 0.75);
        // a power after the ramp window: the order matters
        assert_eq!(
            weight_steps(0.5, &[WeightStep::Affine { scale: 1., bias: 0.25 }, WeightStep::Power(2)]),
            0.5625
        );
        let mut world = World::new();
        let mut weight = |steps| Weight {
            source: Source::Mask {
                texture: TextureRef::new(world.spawn()),
                channels: [0., 0., 0., 1.],
                bias: 0.,
            },
            steps,
        };
        assert_eq!(weight(ramp.to_vec()).check(), Ok(()));
        assert!(
            weight(vec![WeightStep::Affine { scale: f32::NAN, bias: 0. }])
                .check()
                .unwrap_err()
                .contains("affine")
        );
        assert!(
            weight(vec![WeightStep::Max(f32::INFINITY)])
                .check()
                .unwrap_err()
                .contains("max bound")
        );
        assert_eq!(weight(vec![WeightStep::Saturate]).check(), Ok(()));
    }

    /// The gloss is computed as the program stores it and decoded by the stated encoding (for example the square root of
    /// the stored value, times 0.95); an encoding of power 0 is refused.
    #[test]
    fn the_gloss_is_decoded_by_its_encoding() {
        let mut world = World::new();
        let linear = mix(&mut world, 2);
        let encoding = ScalarEncoding {
            power: 1,
            root: 2,
            scale_bits: 0.95f32.to_bits(),
        };
        let encoded = DetailLerp {
            gloss_encoding: encoding,
            ..linear.clone()
        };
        let mut s = samples(2);
        s.base_normal = [0.5, 0.5, 0.25, 0.75];
        s.details[1].normal = [0.5, 0.5, 0.5, 0.];
        let weights = linear.weight_values(&[1. - s.base_normal[3]]);
        let stored = linear.evaluate(&s, &weights).gloss;
        assert!(stored > 0. && stored < 1.);
        let gloss = encoded.evaluate(&s, &weights).gloss;
        assert_eq!(gloss, encoding.decode(stored));
        assert_eq!(gloss, stored.sqrt() * 0.95);
        let e = world.spawn();
        let bad = DetailedMaterial {
            mix: DetailMix::DetailLerp(DetailLerp {
                gloss_encoding: ScalarEncoding { power: 0, ..encoding },
                ..linear
            }),
            lowering: SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 },
            frozen: Vec::new(),
        };
        assert!(bad.verify(e, &world).unwrap_err().contains("gloss encoding"));
    }

    /// A power of two is taken by squaring and any other exponent by multiplying in from the left; an exponent of 0 or
    /// past the limit is refused.
    #[test]
    fn a_power_is_taken_in_its_stated_order_and_bounded() {
        let x = 0.737_f32;
        assert_eq!(power(x, 1), x);
        assert_eq!(power(x, 8).to_bits(), {
            let a = x * x;
            let b = a * a;
            (b * b).to_bits()
        });
        assert_eq!(power(x, 3).to_bits(), ((x * x) * x).to_bits());
        let channel = |exponent| Channel::Power {
            v: Source::Vertex {
                semantic: crate::Semantic::Color(0),
                channel: 0,
            },
            exponent,
        };
        assert!(channel(0).check().is_err());
        assert!(channel(crate::layered::POWER_LIMIT + 1).check().is_err());
        assert!(channel(16).check().is_ok());
        assert_eq!(channel(2).evaluate(&[0.5]), 0.25);
    }

    /// A weight's power is taken by squaring, the powers of the exponent's set bits multiplied in from the lowest, as
    /// one source program writes `(22 N.z)^260` (`x^4 * x^256`) and others `(1 - N.a)^8` (three squarings, the
    /// layered [`power`]'s bits); a min takes its bound where the value is NaN.
    #[test]
    fn a_weights_steps_are_taken_in_the_programs_order() {
        let x = 0.987_654_3_f32;
        let squares: Vec<f32> = std::iter::successors(Some(x), |s| Some(s * s)).take(9).collect();
        assert_eq!(
            weight_power(x, 260).to_bits(),
            (squares[2] * squares[8]).to_bits()
        );
        assert_eq!(weight_power(x, 8).to_bits(), power(x, 8).to_bits());
        assert_eq!(weight_power(x, 1), x);
        assert_eq!(
            weight_power(x, 7).to_bits(),
            ((squares[0] * squares[1]) * squares[2]).to_bits()
        );
        let mut world = World::new();
        let switch_one = Weight {
            source: Source::Mask {
                texture: TextureRef::new(world.spawn()),
                channels: [0., 0., 22., 0.],
                bias: 0.,
            },
            steps: vec![WeightStep::Power(260), WeightStep::Min(1.)],
        };
        assert_eq!(switch_one.check(), Ok(()));
        // a base normal z of 0 takes the first detail alone, one of 1/11 (22 z = 2, past 1 before the power) the second
        assert_eq!(switch_one.of_sample([0.5, 0.5, 0., 0.5]), Some(0.));
        assert_eq!(switch_one.of_sample([0.5, 0.5, 1. / 11., 0.5]), Some(1.));
        let z = 0.045_f32;
        let v = 22. * z;
        let squares: Vec<f32> = std::iter::successors(Some(v), |s| Some(s * s)).take(9).collect();
        assert_eq!(
            switch_one.of_sample([0.5, 0.5, z, 0.5]).unwrap().to_bits(),
            (squares[2] * squares[8]).min(1.).to_bits()
        );
        assert_eq!(switch_one.evaluate(f32::NAN), 1.);
        // the old channel's eighth power and the weight's are the same bits
        let old = Channel::Power {
            v: Source::Vertex {
                semantic: crate::Semantic::Color(0),
                channel: 0,
            },
            exponent: 8,
        };
        let new = weight(TextureRef::new(world.spawn()), true);
        for a in [0.1_f32, 0.37, 0.5, 0.93] {
            assert_eq!(old.evaluate(&[a]).to_bits(), new.evaluate(a).to_bits());
        }
    }

    /// A detail's gloss term is scaled as it states: switch 1's first detail by 0.5, its second by the base normal's z,
    /// lerped by the weight like any other term; a scale every detail takes from the base normal's z is that z.
    #[test]
    fn each_detail_scales_its_gloss_as_it_states() {
        let mut world = World::new();
        let mut two = mix(&mut world, 2);
        let s = samples(2);
        let n = s.base_normal;
        let w = 0.25_f32;
        let base = two.evaluate(&s, &[w]).gloss;
        let terms: Vec<f32> = (0..2)
            .map(|j| s.details[j].normal[2] + two.details[j].gloss_bias)
            .collect();
        let g = terms[0] + (terms[1] - terms[0]) * w;
        assert_eq!(base, (2. * n[2] * g).clamp(0., 1.));
        two.details[0].gloss_scale = GlossScale::Constant(0.5);
        let scale = 0.5 + (n[2] - 0.5) * w;
        assert_eq!(two.evaluate(&s, &[w]).gloss, (2. * scale * g).clamp(0., 1.));
    }

    /// The alpha is the stated detail's colour alpha times the base colour's, and 1 where none is stated.
    #[test]
    fn the_alpha_is_the_stated_details() {
        let mut world = World::new();
        let mut two = mix(&mut world, 2);
        let s = samples(2);
        two.alpha = Some(1);
        assert_eq!(two.evaluate(&s, &[0.5]).alpha, 0.);
        two.alpha = Some(0);
        assert_eq!(two.evaluate(&s, &[0.5]).alpha, 0.5);
        two.alpha = None;
        assert_eq!(two.evaluate(&s, &[0.5]).alpha, 1.);
    }

    /// Under each lowering the surface is held as a G-buffer holds it: an F0 reflectance (the albedo `(1 - f) C colour`),
    /// or a metallic factor (the albedo `C colour`); either way roughness `1 - gloss` and the normal made unit length.
    #[test]
    fn a_surface_lowers_to_the_pbr_model_as_its_g_buffer_holds_it() {
        let surface = DetailSurface {
            c: [0.25, 0.5, 0.75],
            colour: [1., 1., 0.5],
            mix: 0.5,
            alpha: 0.5,
            normal: [0., 3., 4.],
            gloss: 0.75,
        };
        let f0 = SurfaceLowering::GlossReflectance { dielectric_f0: 0. };
        let written = f0.written(&surface);
        assert_eq!(written.albedo, [0.125, 0.25, 0.1875]);
        assert_eq!(written.reflectance, Some([0.125, 0.25, 0.375]));
        assert_eq!(written.metallic, None);
        let pixel = f0.evaluate(&surface);
        assert_eq!(pixel.base_color, [0.125, 0.25, 0.1875, 0.5]);
        assert_eq!(pixel.normal, [0., 0.6, 0.8]);
        assert_eq!((pixel.roughness, pixel.metallic), (0.25, 0.));
        assert_eq!(pixel.specular_color, Some([0.125, 0.25, 0.375]));
        assert_eq!(0.04 * SPECULAR_COLOR_FACTOR, 1.);
        let metal = SurfaceLowering::MetallicGloss;
        let written = metal.written(&surface);
        assert_eq!(written.albedo, [0.25, 0.5, 0.375]);
        assert_eq!((written.reflectance, written.metallic), (None, Some(0.5)));
        let pixel = metal.evaluate(&surface);
        assert_eq!(pixel.base_color, [0.25, 0.5, 0.375, 0.5]);
        assert_eq!(pixel.normal, [0., 0.6, 0.8]);
        assert_eq!((pixel.roughness, pixel.metallic, pixel.specular_color), (0.25, 0.5, None));
        // a dielectric reflectance that is not finite is refused
        let mut world = World::new();
        let e = world.spawn();
        let bad = DetailedMaterial {
            mix: DetailMix::DetailLerp(mix(&mut world, 1)),
            lowering: SurfaceLowering::GlossReflectance { dielectric_f0: f32::NAN },
            frozen: Vec::new(),
        };
        assert!(bad.verify(e, &world).unwrap_err().contains("dielectric"));
    }

    /// A dead texture, a constant that is not finite, an unbounded weight, no or too many details and a weight count other
    /// than one fewer are refused; the textures are its references.
    #[test]
    fn a_detail_material_verifies_its_textures_constants_and_weights() {
        let mut world = World::new();
        let good = DetailedMaterial {
            mix: DetailMix::DetailLerp(mix(&mut world, 2)),
            lowering: SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 },
            frozen: vec![
                Frozen::FrontFacing,
                Frozen::EngineConstant {
                    name: "SomeEngineConstant".into(),
                    value: vec![0.],
                },
            ],
        };
        let e = world.spawn();
        assert_eq!(good.verify(e, &world), Ok(()));
        let mut refs = Vec::new();
        good.references(&mut refs);
        assert_eq!(refs.len(), 8, "seven textures and the weight's");
        let edit = |f: &dyn Fn(&mut DetailLerp)| {
            let mut bad = good.clone();
            let DetailMix::DetailLerp(m) = &mut bad.mix;
            f(m);
            bad
        };
        let bad = edit(&|m| m.gain = f32::NAN);
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let bad = edit(&|m| m.base_saturation = f32::INFINITY);
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let mut bad = good.clone();
        bad.frozen.push(Frozen::EngineTexture {
            name: "g_DecalTexture".into(),
            value: [f32::INFINITY; 4],
        });
        assert!(bad.verify(e, &world).unwrap_err().contains("frozen"));
        let bad = edit(&|m| m.details[1].reflectance = ReflectanceTerm::Bias(f32::INFINITY));
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let bad = edit(&|m| m.weights[0].steps = vec![WeightStep::Power(0)]);
        assert!(bad.verify(e, &world).unwrap_err().contains("exponent"));
        let bad = edit(&|m| m.weights[0].steps = vec![WeightStep::Power(WEIGHT_POWER_LIMIT + 1)]);
        assert!(bad.verify(e, &world).unwrap_err().contains("exponent"));
        let bad = edit(&|m| m.weights[0].steps.push(WeightStep::Min(f32::NAN)));
        assert!(bad.verify(e, &world).unwrap_err().contains("min bound"));
        let bad = edit(&|m| m.alpha = Some(2));
        assert!(bad.verify(e, &world).unwrap_err().contains("detail 2's, of 2"));
        let bad = edit(&|m| m.details[0].gloss_scale = GlossScale::Constant(f32::INFINITY));
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let bad = edit(&|m| m.weights.clear());
        assert!(bad.verify(e, &world).unwrap_err().contains("one fewer"));
        let bad = edit(&|m| {
            m.details.clear();
            m.weights.clear();
        });
        assert!(bad.verify(e, &world).unwrap_err().contains("1 to 3"));
        let mut four = mix(&mut world, 3);
        four.details.push(four.details[0].clone());
        four.weights.push(four.weights[0].clone());
        let bad = DetailedMaterial {
            mix: DetailMix::DetailLerp(four),
            ..good.clone()
        };
        assert!(bad.verify(e, &world).unwrap_err().contains("1 to 3"));
        let dead = world.spawn();
        world.despawn(dead);
        let bad = edit(&|m| m.details[1].colour = TextureRef::new(dead));
        assert!(bad.verify(e, &world).unwrap_err().contains("not alive"));
    }
}
