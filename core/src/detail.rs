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
    TwoDetailLerp(TwoDetailLerp),
}

/// Two tinted details lerped by a per-pixel mask over a base, as Sekiro's `Character_AMSN_[DetailBlend]` programs
/// compute them. With `A`, `N`, `M` the base colour, normal and mask samples, `D0`, `D1` the detail colours, `K0`, `K1`
/// the detail normals, `w` the mask's value and `lerp(a, b, t) = a + (b - a) * t` (lane by lane):
///
/// - `C = saturate(gain * A.rgb * lerp(tint[0] * D0.rgb, tint[1] * D1.rgb, w))`
/// - `f = saturate(lerp(M.x + reflectance_bias[0], reflectance_bias[1], w))`
/// - albedo `(1 - f) * C * colour`, reflectance (F0) `lerp(dielectric_f0, C, f)`
/// - normal: `xy = 2 * N.xy - 1`, `z = sqrt(1 - min(dot(xy, xy), 1))` of the base's `xy`, then `xy += normal_strength *
///   (2 * lerp(K0.xy, K1.xy, w) - 1)`: tangent space, unnormalised
/// - gloss: `saturate(2 * N.z * lerp(K0.z + gloss_bias[0], K1.z + gloss_bias[1], w))` as the program stores it, decoded by
///   `gloss_encoding` (the smoothness the lighting reads)
/// - alpha `saturate(D0.a * A.a)`
///
/// The operations are [`TwoDetailLerp::evaluate`]'s, in its order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TwoDetailLerp {
    pub base_colour: TextureRef,
    pub base_normal: TextureRef,
    pub base_mask: TextureRef,
    pub detail_colour: [TextureRef; 2],
    pub detail_normal: [TextureRef; 2],
    /// The lerp's weight per pixel, from the base's samples (as `1 - N.a` to the eighth: a [`Channel::Power`]).
    pub mask: Channel,
    /// Linear RGB each detail colour is multiplied by.
    pub tint: [[f32; 3]; 2],
    pub gain: f32,
    pub normal_strength: f32,
    pub gloss_bias: [f32; 2],
    /// Added to the base mask's x (the first) and taken alone (the second), lerped by the mask: the reflectance mix `f`.
    pub reflectance_bias: [f32; 2],
    /// The reflectance (F0) of the dielectric part, where `f` is 0.
    pub dielectric_f0: f32,
    /// Linear RGB the albedo is multiplied by.
    pub colour: [f32; 3],
    /// How the program stores the gloss in its G-buffer (the binding's `Gloss` encoding).
    pub gloss_encoding: ScalarEncoding,
}

/// The texture samples of one pixel of a [`TwoDetailLerp`], each a texture's four lanes as sampled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoDetailSamples {
    pub base_colour: [f32; 4],
    pub base_normal: [f32; 4],
    pub base_mask: [f32; 4],
    pub detail_colour: [[f32; 4]; 2],
    pub detail_normal: [[f32; 4]; 2],
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

impl TwoDetailLerp {
    /// The textures it samples, in [`TwoDetailSamples`]' order, and those of its mask after them.
    pub fn textures(&self) -> Vec<&TextureRef> {
        let mut out = vec![&self.base_colour, &self.base_normal, &self.base_mask];
        out.extend(&self.detail_colour);
        out.extend(&self.detail_normal);
        out.extend(self.mask.sources().into_iter().filter_map(|s| match s {
            Source::Mask { texture, .. } => Some(texture),
            Source::Vertex { .. } => None,
        }));
        out
    }

    /// One pixel's surface from its samples and the values of its mask's sources (in [`Channel::sources`] order).
    pub fn evaluate(&self, s: &TwoDetailSamples, mask_sources: &[f32]) -> DetailSurface {
        let w = self.mask.evaluate(mask_sources);
        let (a, n, m) = (s.base_colour, s.base_normal, s.base_mask);
        let [d0, d1] = s.detail_colour;
        let [k0, k1] = s.detail_normal;
        let c: [f32; 3] = std::array::from_fn(|i| {
            let detail = lerp(self.tint[0][i] * d0[i], self.tint[1][i] * d1[i], w);
            (self.gain * a[i] * detail).clamp(0.0, 1.0)
        });
        let f = lerp(m[0] + self.reflectance_bias[0], self.reflectance_bias[1], w).clamp(0.0, 1.0);
        let albedo = std::array::from_fn(|i| (1.0 - f) * c[i] * self.colour[i]);
        let reflectance = c.map(|c| lerp(self.dielectric_f0, c, f));
        let base = [2.0 * n[0] - 1.0, 2.0 * n[1] - 1.0];
        let z = (1.0 - (base[0] * base[0] + base[1] * base[1]).min(1.0)).sqrt();
        let detail = |i: usize| 2.0 * lerp(k0[i], k1[i], w) - 1.0;
        let normal = [
            base[0] + self.normal_strength * detail(0),
            base[1] + self.normal_strength * detail(1),
            z,
        ];
        let stored =
            (2.0 * n[2] * lerp(k0[2] + self.gloss_bias[0], k1[2] + self.gloss_bias[1], w)).clamp(0.0, 1.0);
        let gloss = self.gloss_encoding.decode(stored);
        let alpha = (d0[3] * a[3]).clamp(0.0, 1.0);
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
        self.tint
            .iter()
            .flatten()
            .chain(&self.gloss_bias)
            .chain(&self.reflectance_bias)
            .chain(&self.colour)
            .chain([&self.gain, &self.normal_strength, &self.dielectric_f0])
            .all(|x| x.is_finite())
    }
}

impl DetailMix {
    /// The textures it samples, its mask's included.
    pub fn textures(&self) -> Vec<&TextureRef> {
        match self {
            DetailMix::TwoDetailLerp(mix) => mix.textures(),
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
    /// Its constants and texture transforms finite, its textures alive, its mask a bounded channel of sources that are.
    fn verify(&self, _e: Entity, world: &World) -> Result<(), String> {
        match &self.mix {
            DetailMix::TwoDetailLerp(mix) => {
                if !mix.constants_finite() {
                    return Err("a detail mix constant is not finite".into());
                }
                mix.mask.check().map_err(|e| format!("mask: {e}"))?;
                mix.gloss_encoding
                    .check()
                    .map_err(|e| format!("gloss encoding: {e}"))?;
                for s in mix.mask.sources() {
                    check_source(world, "mask", s)?;
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

    fn mix(world: &mut World) -> TwoDetailLerp {
        let mut t = || TextureRef::new(world.spawn());
        let (base_colour, base_normal, base_mask) = (t(), t(), t());
        let detail_colour = [t(), t()];
        let detail_normal = [t(), t()];
        TwoDetailLerp {
            mask: Channel::Power {
                v: Source::Mask {
                    texture: base_normal,
                    channels: [0., 0., 0., -1.],
                    bias: 1.,
                },
                exponent: 8,
            },
            base_colour,
            base_normal,
            base_mask,
            detail_colour,
            detail_normal,
            tint: [[1., 0.5, 0.25], [0.5, 1., 1.]],
            gain: 4.55,
            normal_strength: 0.5,
            gloss_bias: [0.25, -0.25],
            reflectance_bias: [0.125, 0.75],
            dielectric_f0: 0.03,
            colour: [1., 1., 0.5],
            gloss_encoding: ScalarEncoding::LINEAR,
        }
    }

    /// Each output of the equation as its doc states it, for one pixel's samples: the mask `(1 - N.a)^8` by squaring.
    #[test]
    fn two_details_are_lerped_by_the_mask_over_the_base() {
        let mut world = World::new();
        let mix = mix(&mut world);
        let s = TwoDetailSamples {
            base_colour: [0.1, 0.2, 0.05, 0.5],
            base_normal: [0.75, 0.25, 0.5, 0.5],
            base_mask: [0.5, 0., 0., 0.],
            detail_colour: [[0.5, 0.5, 0.5, 1.], [1., 0.25, 0.5, 0.]],
            detail_normal: [[0.5, 0.5, 0.75, 0.], [1., 0., 0.25, 0.]],
        };
        let w = power(1. - s.base_normal[3], 8);
        assert_eq!(
            w.to_bits(),
            (((0.5f32 * 0.5) * (0.5 * 0.5)) * ((0.5 * 0.5) * (0.5 * 0.5))).to_bits()
        );
        let out = mix.evaluate(&s, &[1. - s.base_normal[3]]);
        let c: [f32; 3] = std::array::from_fn(|i| {
            let d = mix.tint[0][i] * s.detail_colour[0][i];
            let e = mix.tint[1][i] * s.detail_colour[1][i];
            (mix.gain * s.base_colour[i] * (d + (e - d) * w)).clamp(0., 1.)
        });
        let f = (0.5 + 0.125 + (0.75 - 0.625) * w).clamp(0., 1.);
        for (i, c) in c.iter().enumerate() {
            assert_eq!(out.albedo[i], (1. - f) * c * mix.colour[i]);
            assert_eq!(out.reflectance[i], 0.03 + (c - 0.03) * f);
        }
        let k = |i: usize| {
            2. * (s.detail_normal[0][i] + (s.detail_normal[1][i] - s.detail_normal[0][i]) * w) - 1.
        };
        assert_eq!(
            out.normal,
            [0.5 + 0.5 * k(0), -0.5 + 0.5 * k(1), (1f32 - 0.5).sqrt()]
        );
        let gloss = 2. * 0.5 * (1. + (0. - 1.) * w);
        assert_eq!(out.gloss, gloss.clamp(0., 1.));
        assert_eq!(out.alpha, 0.5);
    }

    /// The gloss is computed as the program stores it and decoded by the stated encoding (Sekiro's: the square root of
    /// the stored value, times 0.95); an encoding of power 0 is refused.
    #[test]
    fn the_gloss_is_decoded_by_its_encoding() {
        let mut world = World::new();
        let linear = mix(&mut world);
        let encoding = ScalarEncoding {
            power: 1,
            root: 2,
            scale_bits: 0.95f32.to_bits(),
        };
        let encoded = TwoDetailLerp {
            gloss_encoding: encoding,
            ..linear.clone()
        };
        let s = TwoDetailSamples {
            base_colour: [0.1, 0.2, 0.05, 0.5],
            base_normal: [0.5, 0.5, 0.25, 0.75],
            base_mask: [0.5, 0., 0., 0.],
            detail_colour: [[0.5; 4], [0.5; 4]],
            detail_normal: [[0.5, 0.5, 0.75, 0.], [0.5, 0.5, 0.5, 0.]],
        };
        let weight = [1. - s.base_normal[3]];
        let stored = linear.evaluate(&s, &weight).gloss;
        assert!(stored > 0. && stored < 1.);
        let gloss = encoded.evaluate(&s, &weight).gloss;
        assert_eq!(gloss, encoding.decode(stored));
        assert_eq!(gloss, stored.sqrt() * 0.95);
        let e = world.spawn();
        let bad = DetailedMaterial {
            mix: DetailMix::TwoDetailLerp(TwoDetailLerp {
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

    /// A dead texture, a constant that is not finite and an unbounded mask are refused; the textures are its references.
    #[test]
    fn a_detail_material_verifies_its_textures_constants_and_mask() {
        let mut world = World::new();
        let good = DetailedMaterial {
            mix: DetailMix::TwoDetailLerp(mix(&mut world)),
            lowering: SurfaceLowering::GlossReflectance,
            frozen: vec!["vertex colour white".into()],
        };
        let e = world.spawn();
        assert_eq!(good.verify(e, &world), Ok(()));
        let mut refs = Vec::new();
        good.references(&mut refs);
        assert_eq!(refs.len(), 8, "seven textures and the mask's");
        let mut bad = good.clone();
        let DetailMix::TwoDetailLerp(m) = &mut bad.mix;
        m.gain = f32::NAN;
        assert!(bad.verify(e, &world).unwrap_err().contains("not finite"));
        let mut bad = good.clone();
        let DetailMix::TwoDetailLerp(m) = &mut bad.mix;
        m.mask = Channel::Power {
            v: Source::Vertex {
                semantic: crate::Semantic::Color(0),
                channel: 0,
            },
            exponent: 0,
        };
        assert!(bad.verify(e, &world).unwrap_err().contains("exponent"));
        let mut bad = good.clone();
        let DetailMix::TwoDetailLerp(m) = &mut bad.mix;
        let dead = world.spawn();
        world.despawn(dead);
        m.detail_colour[1] = TextureRef::new(dead);
        assert!(bad.verify(e, &world).unwrap_err().contains("not alive"));
    }
}
