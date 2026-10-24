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
//! outputs, over random texture samples). Its outputs are the program's own G-buffer roles ([`DetailSurface`]), the
//! mix happening before any lighting model; a closed set of lowerings ([`SurfaceLowering`]) makes them the PBR model's,
//! the same lowering the source's bake applies, so every consumer converts through core.
use serde::Serialize;
use serde_json::Value;

use crate::layered::{Channel, Source, check_source};
use crate::pbr::TextureRef;
use crate::shader::ScalarEncoding;
use crate::{Component, Entity, JsonError, World};

/// A material's detail form (component `pbr.detail`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DetailedMaterial {
    pub mix: DetailMix,
    /// How the mix's outputs become the PBR model.
    pub lowering: SurfaceLowering,
    /// Each input the equation reads as a constant rather than per draw, as the source's bake freezes it (the vertex
    /// colour white, the instance colour, decals and wear off): what a consumer shows beside it.
    pub frozen: Vec<String>,
}

/// The equations a detail material is drawn by.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailMix {
    DetailLerp(DetailLerp),
}

/// The most details a [`DetailLerp`] chains (Sekiro's DetailBlend programs read one to three).
pub const DETAIL_LIMIT: usize = 3;

/// Tinted details lerped in turn over a base, as Sekiro's `Character_AMSN_[DetailBlend]` programs compute them. With
/// `A`, `N`, `M` the base colour, normal and mask samples, `D_i`, `K_i` the detail colour and normal samples,
/// `lerp(a, b, t) = a + (b - a) * t` (lane by lane) and `chain(x)` the details' values `x_0, x_1, ...` lerped in turn by
/// the weights (`x_0`, then `lerp(.., x_1, w_1)`, then `lerp(.., x_2, w_2)`):
///
/// - `C = saturate(gain * A.rgb * chain(tint_i * D_i.rgb))`
/// - `f = saturate(chain(r_i))`, each detail's reflectance term `r_i` as it states ([`ReflectanceTerm`])
/// - albedo `(1 - f) * C * colour`, reflectance (F0) `lerp(dielectric_f0, C, f)`
/// - normal: `xy = 2 * N.xy - 1`, `z = sqrt(1 - min(dot(xy, xy), 1))` of the base's `xy`, then `xy += normal_strength *
///   (2 * chain(K_i.xy) - 1)`: tangent space, unnormalised
/// - gloss: `saturate(2 * N.z * chain(K_i.z + gloss_bias_i))` as the program stores it, decoded by `gloss_encoding` (the
///   smoothness the lighting reads)
/// - alpha `saturate(D_0.a * A.a)`
///
/// One detail is no lerp at all. The operations are [`DetailLerp::evaluate`]'s, in its order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DetailLerp {
    pub base_colour: TextureRef,
    pub base_normal: TextureRef,
    pub base_mask: TextureRef,
    /// One to [`DETAIL_LIMIT`], in the chain's order.
    pub details: Vec<Detail>,
    /// The weight each detail after the first is lerped in by, per pixel, from the base's samples (as `1 - N.a` or
    /// `N.a` to the eighth: a [`Channel::Power`]): one fewer than the details.
    pub weights: Vec<Channel>,
    pub gain: f32,
    pub normal_strength: f32,
    /// The reflectance (F0) of the dielectric part, where `f` is 0.
    pub dielectric_f0: f32,
    /// Linear RGB the albedo is multiplied by.
    pub colour: [f32; 3],
    /// How the program stores the gloss in its G-buffer (the binding's `Gloss` encoding).
    pub gloss_encoding: ScalarEncoding,
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

/// One pixel's surface in the program's G-buffer roles, decoded, before the PBR lowering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailSurface {
    /// Linear RGB: the diffuse colour the program writes.
    pub albedo: [f32; 3],
    pub alpha: f32,
    /// Tangent space, unnormalised (the program normalises after its tangent frame).
    pub normal: [f32; 3],
    /// Smoothness, 1 - perceptual roughness.
    pub gloss: f32,
    /// Linear RGB reflectance at normal incidence (F0).
    pub reflectance: [f32; 3],
}

/// How a [`DetailSurface`] becomes the PBR model: a closed set, each the lowering a source's bake applies to the same
/// roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceLowering {
    /// Gloss and an F0 reflectance, as Sekiro's G-buffer holds them and its bake lowers them: the base colour the
    /// albedo, roughness `1 - gloss`, metallic 0 (the reflectance carries the metal), and the F0 as the specular colour
    /// glTF's `KHR_materials_specular` states with a colour factor of 25 (its F0 is `0.04 * factor * colour`).
    GlossReflectance,
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
        out.extend(
            self.weights
                .iter()
                .flat_map(Channel::sources)
                .filter_map(|s| match s {
                    Source::Mask { texture, .. } => Some(texture),
                    Source::Vertex { .. } => None,
                }),
        );
        out
    }

    /// The weights' values for the values of their sources (each weight's in [`Channel::sources`] order).
    pub fn weight_values(&self, sources: &[&[f32]]) -> Vec<f32> {
        self.weights
            .iter()
            .zip(sources)
            .map(|(w, values)| w.evaluate(values))
            .collect()
    }

    /// One pixel's surface from its samples and the values of its weights ([`DetailLerp::weight_values`]).
    pub fn evaluate(&self, s: &DetailSamples, weights: &[f32]) -> DetailSurface {
        let (a, n, m) = (s.base_colour, s.base_normal, s.base_mask);
        let pairs = || self.details.iter().zip(&s.details);
        let c: [f32; 3] = std::array::from_fn(|i| {
            let detail = chain(pairs().map(|(d, x)| d.tint[i] * x.colour[i]), weights);
            (self.gain * a[i] * detail).clamp(0.0, 1.0)
        });
        let f = chain(self.details.iter().map(|d| d.reflectance.value(m)), weights).clamp(0.0, 1.0);
        let albedo = std::array::from_fn(|i| (1.0 - f) * c[i] * self.colour[i]);
        let reflectance = c.map(|c| lerp(self.dielectric_f0, c, f));
        let base = [2.0 * n[0] - 1.0, 2.0 * n[1] - 1.0];
        let z = (1.0 - (base[0] * base[0] + base[1] * base[1]).min(1.0)).sqrt();
        let detail = |i: usize| 2.0 * chain(s.details.iter().map(|x| x.normal[i]), weights) - 1.0;
        let normal = [
            base[0] + self.normal_strength * detail(0),
            base[1] + self.normal_strength * detail(1),
            z,
        ];
        let stored =
            (2.0 * n[2] * chain(pairs().map(|(d, x)| x.normal[2] + d.gloss_bias), weights)).clamp(0.0, 1.0);
        let gloss = self.gloss_encoding.decode(stored);
        let first = s.details.first().map_or(1.0, |x| x.colour[3]);
        let alpha = (first * a[3]).clamp(0.0, 1.0);
        DetailSurface {
            albedo,
            alpha,
            normal,
            gloss,
            reflectance,
        }
    }

    /// Its constants all finite.
    fn constants_finite(&self) -> bool {
        self.details
            .iter()
            .flat_map(|d| d.tint.into_iter().chain([d.gloss_bias, d.reflectance.bias()]))
            .chain(self.colour)
            .chain([self.gain, self.normal_strength, self.dielectric_f0])
            .all(f32::is_finite)
    }

    /// One to [`DETAIL_LIMIT`] details, one weight fewer, each weight bounded, the constants finite and the gloss encoding
    /// sound.
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
    /// The PBR model's pixel for `surface`.
    pub fn evaluate(self, surface: &DetailSurface) -> PbrPixel {
        match self {
            SurfaceLowering::GlossReflectance => {
                let [r, g, b] = surface.albedo;
                let n = surface.normal;
                let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                PbrPixel {
                    base_color: [r, g, b, surface.alpha],
                    normal: n.map(|x| x / length),
                    roughness: 1.0 - surface.gloss,
                    metallic: 0.0,
                    specular_color: Some(surface.reflectance),
                }
            }
        }
    }
}

impl Component for DetailedMaterial {
    const NAME: &'static str = "pbr.detail";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    const VERSION: u32 = 1;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Its constants and texture transforms finite, its textures alive, its details 1 to [`DETAIL_LIMIT`] and its weights
    /// bounded channels of sources that are, one fewer.
    fn verify(&self, _e: Entity, world: &World) -> Result<(), String> {
        match &self.mix {
            DetailMix::DetailLerp(mix) => {
                mix.check()?;
                for s in mix.weights.iter().flat_map(Channel::sources) {
                    check_source(world, "weight", s)?;
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
    use crate::layered::power;

    /// `(1 - N.a)^8` (`flip`: `N.a^8`) of the base normal `texture`.
    fn weight(texture: TextureRef, flip: bool) -> Channel {
        Channel::Power {
            v: Source::Mask {
                texture,
                channels: [0., 0., 0., if flip { 1. } else { -1. }],
                bias: if flip { 0. } else { 1. },
            },
            exponent: 8,
        }
    }

    /// A detail lerp of `n` details with distinct constants; the second detail's weight `(1 - N.a)^8`, the third's
    /// `N.a^8`, as Sekiro's switch 13 and 6 programs.
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
                reflectance: reflectance[i],
            })
            .collect();
        DetailLerp {
            weights: [weight(base_normal, false), weight(base_normal, true)][..n - 1].to_vec(),
            base_colour,
            base_normal,
            base_mask,
            details,
            gain: 4.55,
            normal_strength: 0.5,
            dielectric_f0: 0.03,
            colour: [1., 1., 0.5],
            gloss_encoding: ScalarEncoding::LINEAR,
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
            let sources: [&[f32]; 2] = [&[1. - a], &[a]];
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
            let c: [f32; 3] = std::array::from_fn(|i| {
                let x: Vec<f32> = (0..n)
                    .map(|j| mix.details[j].tint[i] * s.details[j].colour[i])
                    .collect();
                (mix.gain * s.base_colour[i] * by_hand(&x)).clamp(0., 1.)
            });
            let terms: Vec<f32> = [0.5 + 0.125, 0.75, 0.375][..n].to_vec();
            let f = by_hand(&terms).clamp(0., 1.);
            for (i, c) in c.iter().enumerate() {
                assert_eq!(out.albedo[i], (1. - f) * c * mix.colour[i], "{n} details");
                assert_eq!(out.reflectance[i], 0.03 + (c - 0.03) * f, "{n} details");
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
        assert_eq!(out.reflectance[0], 0.03 + ((4.55 * 0.1 * (1. * 0.5)) - 0.03) * f);
    }

    /// The gloss is computed as the program stores it and decoded by the stated encoding (Sekiro's: the square root of
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
        let weights = linear.weight_values(&[&[1. - s.base_normal[3]]]);
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
            lowering: SurfaceLowering::GlossReflectance,
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

    /// Gloss and reflectance lower as Sekiro's bake lowers them: roughness 1 - gloss, metallic 0, the F0 as the
    /// specular colour; the normal made unit length.
    #[test]
    fn gloss_and_reflectance_lower_to_roughness_and_specular_colour() {
        let surface = DetailSurface {
            albedo: [0.25, 0.5, 0.75],
            alpha: 0.5,
            normal: [0., 3., 4.],
            gloss: 0.75,
            reflectance: [0.03, 0.04, 0.5],
        };
        let pixel = SurfaceLowering::GlossReflectance.evaluate(&surface);
        assert_eq!(pixel.base_color, [0.25, 0.5, 0.75, 0.5]);
        assert_eq!(pixel.normal, [0., 0.6, 0.8]);
        assert_eq!((pixel.roughness, pixel.metallic), (0.25, 0.));
        assert_eq!(pixel.specular_color, Some([0.03, 0.04, 0.5]));
        assert_eq!(0.04 * SPECULAR_COLOR_FACTOR, 1.);
    }

    /// A dead texture, a constant that is not finite, an unbounded weight, no or too many details and a weight count other
    /// than one fewer are refused; the textures are its references.
    #[test]
    fn a_detail_material_verifies_its_textures_constants_and_weights() {
        let mut world = World::new();
        let good = DetailedMaterial {
            mix: DetailMix::DetailLerp(mix(&mut world, 2)),
            lowering: SurfaceLowering::GlossReflectance,
            frozen: vec!["vertex colour white".into()],
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
        let bad = edit(&|m| m.details[1].reflectance = ReflectanceTerm::Bias(f32::INFINITY));
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let bad = edit(&|m| {
            m.weights[0] = Channel::Power {
                v: Source::Vertex {
                    semantic: crate::Semantic::Color(0),
                    channel: 0,
                },
                exponent: 0,
            }
        });
        assert!(bad.verify(e, &world).unwrap_err().contains("exponent"));
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
