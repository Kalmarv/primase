//! One reading of a pixel program's outputs as a surface: which output and lanes hold each term a lighting pass takes
//! (the albedo, the normal, the roughness, the metallic, the reflectance, the specular strength, the emission and the
//! occlusion) and how a lane's stored value decodes to the term, from the roles the program's binding states
//! ([`OutputRole`]). Every consumer that lights a program's outputs takes them through this one reading: a material bake
//! that evaluates the program on the CPU decodes with [`Surface`]'s terms, and a viewer's composite that decodes a
//! G-buffer on the GPU uses the same reading written as WGSL (`primase_shader::surface`), so the two cannot disagree on
//! what a lane means.
//!
//! The reading states only what the roles state. A term no output has a role for is `None`, and the consumer says the
//! default it takes (a matte dielectric: roughness 1, metallic 0, the metallic workflow's reflectance); a lane is never
//! taken for a term by its position. The first output with a role is the one read where several have it, as a lighting
//! pass reads one target per term.
use serde::Serialize;

use super::binding::{CUBE_FACES, NormalEncoding, NormalSpace, Occluded, OutputRole, ScalarEncoding};

/// The lanes of one output a term is stored in: the output's position among the outputs read ([`Surface::read`]) and
/// its lane mask (bit k lane k).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Place {
    pub output: usize,
    pub lanes: u8,
}

impl Place {
    /// The first lane of the mask, 0 to 3 (0 for an empty mask, which no term has).
    pub fn first(&self) -> usize {
        (self.lanes.trailing_zeros() as usize).min(3)
    }

    /// How many lanes the mask holds.
    pub fn width(&self) -> u32 {
        self.lanes.count_ones()
    }

    /// Lane `k` of the term (0 its first lane) out of the four values of its output, or 0 past the output's four.
    pub fn lane(&self, values: [f32; 4], k: usize) -> f32 {
        values.get(self.first() + k).copied().unwrap_or(0.0)
    }
}

/// A term stored in the lanes of one output, each lane's value decoded by `encoding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Term {
    pub place: Place,
    pub encoding: ScalarEncoding,
}

impl Term {
    /// The value lane `k` of the term stores, out of the four values of its output.
    pub fn decode(&self, values: [f32; 4], k: usize) -> f32 {
        self.encoding.decode(self.place.lane(values, k))
    }

    /// The three lanes of a colour term, each decoded.
    pub fn colour(&self, values: [f32; 4]) -> [f32; 3] {
        [0, 1, 2].map(|k| self.decode(values, k))
    }
}

/// Where the surface's colour is: the albedo a deferred program writes, or the finished colour of a forward program
/// that writes no albedo (the colour it lit, taken as the surface's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    Albedo(Place),
    Colour(Place),
}

impl Base {
    pub fn place(&self) -> Place {
        match *self {
            Base::Albedo(p) | Base::Colour(p) => p,
        }
    }

    /// The colour's three lanes as stored (linear).
    pub fn colour(&self, values: [f32; 4]) -> [f32; 3] {
        let place = self.place();
        [0, 1, 2].map(|k| place.lane(values, k))
    }
}

/// The surface's normal: its place, how its lanes store it and the space it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Normal {
    pub place: Place,
    pub encoding: NormalEncoding,
    pub space: NormalSpace,
}

impl Normal {
    /// The lanes its encoding stores the normal in.
    pub fn width(encoding: NormalEncoding) -> u32 {
        match encoding {
            NormalEncoding::Xy | NormalEncoding::XyUnorm | NormalEncoding::Octahedral { .. } => 2,
            NormalEncoding::Xyz
            | NormalEncoding::XyzUnorm
            | NormalEncoding::XyzBiased { .. }
            | NormalEncoding::OctahedralDiamond => 3,
            NormalEncoding::CubeFace { .. } => 4,
        }
    }

    /// The vector the lanes store, out of the four values of its output, as the encoding gives it back: the three
    /// stored components unbiased (`Xyz`, `XyzUnorm`, `XyzBiased`), which a program need not have written at unit length;
    /// the unit vector of the encodings that make one (the two-lane, octahedral and cube-face ones).
    ///
    /// A cube face is taken as the nearest axis to lane 3 and the side of one half lane 2 is on, without
    /// [`NormalEncoding::decode_cube_face`]'s refusal of lanes between them, which needs the target's quantisation step;
    /// the payloads beside a normal (the diamond layout's and the cube face's) are not part of it.
    pub fn vector(&self, values: [f32; 4]) -> [f32; 3] {
        let lane = |k: usize| self.place.lane(values, k);
        match self.encoding {
            NormalEncoding::Xyz => [lane(0), lane(1), lane(2)],
            NormalEncoding::XyzUnorm => [0, 1, 2].map(|k| (lane(k) - 0.5) * 2.0),
            NormalEncoding::XyzBiased { bias_bits } => {
                let bias = f32::from_bits(bias_bits);
                [0, 1, 2].map(|k| (lane(k) - bias) * 2.0)
            }
            NormalEncoding::Xy => with_z(lane(0), lane(1)),
            NormalEncoding::XyUnorm => with_z(lane(0) * 2.0 - 1.0, lane(1) * 2.0 - 1.0),
            NormalEncoding::Octahedral { read, .. } => unfold(read.apply(lane(0)), read.apply(lane(1))),
            NormalEncoding::OctahedralDiamond => {
                let (n, _) = NormalEncoding::decode_octahedral_diamond([lane(0), lane(1), lane(2)]);
                n.map(|c| c as f32)
            }
            NormalEncoding::CubeFace { minor, .. } => {
                let axis = (lane(3) * 3.0).round().clamp(0.0, 2.0) as usize;
                let face = axis * 2 + usize::from(lane(2) > 0.5);
                let (major, components) = CUBE_FACES[face];
                let (sc, tc) = (
                    (lane(0) - minor.bias()) / minor.scale(),
                    (lane(1) - minor.bias()) / minor.scale(),
                );
                let mut n = [0.0f32; 3];
                n[components[0].0] = sc * components[0].1 as f32;
                n[components[1].0] = tc * components[1].1 as f32;
                n[major] = (1.0 - sc * sc - tc * tc).max(0.0).sqrt() * if face % 2 == 1 { -1.0 } else { 1.0 };
                normalised(n).unwrap_or(n)
            }
        }
    }

    /// The unit normal the lanes store, or `None` where they hold no direction (a vector of length 1e-8 or less).
    pub fn decode(&self, values: [f32; 4]) -> Option<[f32; 3]> {
        normalised(self.vector(values))
    }
}

/// `n` at unit length, or `None` for a length of 1e-8 or less.
fn normalised(n: [f32; 3]) -> Option<[f32; 3]> {
    let length = n.iter().map(|x| x * x).sum::<f32>().sqrt();
    (length > 1e-8).then(|| n.map(|x| x / length))
}

/// x and y with z from the unit length (0 where they are longer).
fn with_z(x: f32, y: f32) -> [f32; 3] {
    [x, y, (1.0 - x * x - y * y).max(0.0).sqrt()]
}

/// The octahedral unfold: `z = 1 - |x| - |y|`, x and y folded back where z < 0 (`(1 - |y|) sign x`, `(1 - |x|) sign y`,
/// a sign of 0 taken as +), normalised.
fn unfold(x: f32, y: f32) -> [f32; 3] {
    let z = 1.0 - x.abs() - y.abs();
    let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
    let (x, y) = if z < 0.0 {
        ((1.0 - y.abs()) * sign(x), (1.0 - x.abs()) * sign(y))
    } else {
        (x, y)
    };
    let n = [x, y, z];
    normalised(n).unwrap_or(n)
}

/// Where the roughness comes from: the role that states the highlight's spread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Roughness {
    /// The perceptual roughness, stored.
    Roughness(Term),
    /// The gloss (smoothness): the roughness is one minus it.
    Gloss(Term),
    /// A specular exponent `n` (a Blinn-Phong falloff): the roughness is `(2 / (n + 2))^(1/4)`, the perceptual roughness
    /// whose square, the microfacet distribution's alpha, matches the Blinn-Phong lobe of exponent `n`
    /// (`alpha = sqrt(2 / (n + 2))`). A convention for showing such a source with a roughness-based light, not a measure
    /// of the source's own lighting, which keeps its exponent.
    Exponent(Term),
}

impl Roughness {
    pub fn term(&self) -> Term {
        match *self {
            Roughness::Roughness(t) | Roughness::Gloss(t) | Roughness::Exponent(t) => t,
        }
    }

    /// The perceptual roughness of the output's four values; a consumer clamps it to 0 to 1.
    pub fn decode(&self, values: [f32; 4]) -> f32 {
        let value = self.term().decode(values, 0);
        match self {
            Roughness::Roughness(_) => value,
            Roughness::Gloss(_) => 1.0 - value,
            Roughness::Exponent(_) => (2.0 / (value.max(0.0) + 2.0)).powf(0.25),
        }
    }
}

/// The light the surface gives off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Emission {
    /// A linear colour in three lanes.
    Colour(Term),
    /// One lane that scales the albedo: the light given off is the albedo times it.
    OfAlbedo(Term),
}

impl Emission {
    pub fn term(&self) -> Term {
        match *self {
            Emission::Colour(t) | Emission::OfAlbedo(t) => t,
        }
    }

    /// The light given off, of the output's four values and the surface's albedo.
    pub fn decode(&self, values: [f32; 4], albedo: [f32; 3]) -> [f32; 3] {
        match self {
            Emission::Colour(t) => t.colour(values),
            Emission::OfAlbedo(t) => {
                let e = t.decode(values, 0);
                albedo.map(|a| a * e)
            }
        }
    }
}

/// A program's outputs as a surface: each term's place and decoding, `None` where no output has a role for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Surface {
    pub base: Option<Base>,
    pub normal: Option<Normal>,
    pub roughness: Option<Roughness>,
    /// The metalness, 0 to 1: the reflectance is the metallic workflow's `mix(0.04, albedo, metallic)` and the diffuse
    /// colour `albedo * (1 - metallic)`.
    pub metallic: Option<Term>,
    /// The reflectance at normal incidence (F0), a colour in three lanes: the specular workflow, where the albedo is the
    /// diffuse colour and this the reflectance, in place of the metallic workflow's.
    pub reflectance: Option<Term>,
    /// The specular strength, 0 to 1: a consumer scales the light the surface reflects as a highlight by it.
    pub specular: Option<Term>,
    pub emission: Option<Emission>,
    /// The occlusion of every light that reaches the surface from outside it (1 unoccluded): a consumer scales the
    /// light the surface reflects by it, its emission aside.
    pub occlusion: Option<Term>,
    /// The occlusion of the environment's own ambient light alone (1 unoccluded), where the outputs state that and not
    /// [`Surface::occlusion`]: a consumer scales its ambient light by it, not its other lights.
    pub ambient_occlusion: Option<Term>,
    /// Outputs with a role of a term whose lanes do not fit it (an albedo in two lanes, a scalar in three), with why:
    /// they are not read, and the term is `None` unless another output has its role.
    pub refused: Vec<String>,
}

impl Surface {
    /// The surface of a program's outputs, each its lanes and role in the order the outputs are listed; a term's
    /// [`Place::output`] is the output's position in that order.
    pub fn read<'a>(outputs: impl IntoIterator<Item = (u8, &'a OutputRole)>) -> Surface {
        let mut s = Surface::default();
        let mut colour = None;
        let mut refused = Vec::new();
        for (output, (lanes, role)) in outputs.into_iter().enumerate() {
            let place = Place { output, lanes };
            let width = place.width();
            let mut fits = |what: &str, wanted: &[u32]| -> bool {
                if wanted.contains(&width) {
                    return true;
                }
                refused.push(format!(
                    "output {output}: {what} in {width} lanes ({lanes:#06b}), not {}",
                    wanted.iter().map(u32::to_string).collect::<Vec<_>>().join(" or ")
                ));
                false
            };
            let term = |encoding: ScalarEncoding| Term { place, encoding };
            let linear = term(ScalarEncoding::LINEAR);
            match *role {
                OutputRole::Albedo if s.base.is_none() && fits("an albedo", &[3, 4]) => {
                    s.base = Some(Base::Albedo(place));
                }
                OutputRole::Colour if colour.is_none() && fits("a finished colour", &[3, 4]) => {
                    colour = Some(Base::Colour(place));
                }
                OutputRole::Normal { encoding, space }
                    if s.normal.is_none() && fits("a normal", &[Normal::width(encoding)]) =>
                {
                    s.normal = Some(Normal {
                        place,
                        encoding,
                        space,
                    });
                }
                OutputRole::Roughness if s.roughness.is_none() && fits("a roughness", &[1]) => {
                    s.roughness = Some(Roughness::Roughness(linear));
                }
                OutputRole::Gloss { encoding } if s.roughness.is_none() && fits("a gloss", &[1]) => {
                    s.roughness = Some(Roughness::Gloss(term(encoding)));
                }
                OutputRole::SpecularExponent { encoding }
                    if s.roughness.is_none() && fits("a specular exponent", &[1]) =>
                {
                    s.roughness = Some(Roughness::Exponent(term(encoding)));
                }
                OutputRole::Metallic if s.metallic.is_none() && fits("a metallic", &[1]) => {
                    s.metallic = Some(linear);
                }
                OutputRole::Reflectance { encoding }
                    if s.reflectance.is_none() && fits("a reflectance", &[3]) =>
                {
                    s.reflectance = Some(term(encoding));
                }
                OutputRole::SpecularIntensity { encoding }
                    if s.specular.is_none() && fits("a specular intensity", &[1]) =>
                {
                    s.specular = Some(term(encoding));
                }
                OutputRole::Emission { encoding } if s.emission.is_none() && fits("an emission", &[1, 3]) => {
                    s.emission = Some(if width == 1 {
                        Emission::OfAlbedo(term(encoding))
                    } else {
                        Emission::Colour(term(encoding))
                    });
                }
                OutputRole::Occlusion {
                    of: Occluded::AllLight,
                    encoding,
                } if s.occlusion.is_none() && fits("an occlusion", &[1]) => {
                    s.occlusion = Some(term(encoding));
                }
                OutputRole::Occlusion {
                    of: Occluded::NaturalAmbient,
                    encoding,
                } if s.ambient_occlusion.is_none() && fits("an occlusion", &[1]) => {
                    s.ambient_occlusion = Some(term(encoding));
                }
                _ => {}
            }
        }
        s.base = s.base.or(colour);
        if s.occlusion.is_some() {
            s.ambient_occlusion = None;
        }
        s.refused = refused;
        s
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked values")]
mod tests {
    use super::*;
    use crate::shader::LaneAffine;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        within(a, b, 1e-4)
    }

    fn within(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < tolerance)
    }

    fn normal(encoding: NormalEncoding, lanes: u8) -> Normal {
        Normal {
            place: Place { output: 0, lanes },
            encoding,
            space: NormalSpace::World,
        }
    }

    /// Each normal encoding gives back the normal it stores: the three-lane ones as stored (unbiased, not normalised), the
    /// others at unit length, read from the term's first lane on.
    #[test]
    fn each_normal_encoding_decodes_what_it_stores() {
        let n = [0.6f32, 0.0, 0.8];
        let unorm = n.map(|c| c * 0.5 + 0.5);
        assert!(close(
            normal(NormalEncoding::Xyz, 0b0111).vector([n[0], n[1], n[2], 9.0]),
            n
        ));
        assert!(close(
            normal(NormalEncoding::XyzUnorm, 0b0111).vector([unorm[0], unorm[1], unorm[2], 0.0]),
            n
        ));
        let bias = 0.500_488_76f32;
        let biased = n.map(|c| c * 0.5 + bias);
        assert!(close(
            normal(
                NormalEncoding::XyzBiased {
                    bias_bits: bias.to_bits()
                },
                0b0111
            )
            .vector([biased[0], biased[1], biased[2], 0.0]),
            n
        ));
        // not normalised by `vector`, normalised by `decode`
        let long = normal(NormalEncoding::Xyz, 0b0111);
        assert_eq!(long.vector([0.0, 2.0, 0.0, 0.0]), [0.0, 2.0, 0.0]);
        assert_eq!(long.decode([0.0, 2.0, 0.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(long.decode([0.0; 4]), None);
        // two lanes, in lanes 2 and 3 of the output
        assert!(close(
            normal(NormalEncoding::Xy, 0b1100).vector([9.0, 9.0, 0.6, 0.0]),
            n
        ));
        assert!(close(
            normal(NormalEncoding::XyUnorm, 0b1100).vector([9.0, 9.0, 0.8, 0.5]),
            n
        ));
        // octahedral: (0.6, 0, 0.8) folds to x = 0.6 / 1.4, y = 0, read back from the 0-1 layout
        let e = 0.6f32 / 1.4;
        let octahedral = normal(
            NormalEncoding::Octahedral {
                stored: LaneAffine::UNORM,
                read: LaneAffine::SIGNED,
            },
            0b0011,
        );
        assert!(close(octahedral.vector([e * 0.5 + 0.5, 0.5, 0.0, 0.0]), n));
        // the lower hemisphere folds back: (0.6, 0, -0.8) is stored as (1 - 0) sign x, (1 - |x|) sign 0 = (1, 0.571)
        let below = octahedral.vector([1.0, (1.0 - e) * 0.5 + 0.5, 0.0, 0.0]);
        assert!(close(below, [0.6, 0.0, -0.8]), "{below:?}");
    }

    /// The diamond layout (to its 9-bit step) and the cube face give back what their encoders store.
    #[test]
    fn the_packed_normal_encodings_decode_what_their_encoders_store() {
        let n = [0.36f64, -0.48, 0.8];
        let bytes = NormalEncoding::encode_octahedral_diamond(n, 5);
        let lanes = bytes.map(|b| f32::from(b) / 255.0);
        let diamond =
            normal(NormalEncoding::OctahedralDiamond, 0b0111).decode([lanes[0], lanes[1], lanes[2], 0.0]);
        assert!(within(diamond.unwrap(), n.map(|c| c as f32), 0.01), "{diamond:?}");
        let (minor, payload) = (LaneAffine::UNORM, LaneAffine::new(0.25, 0.1));
        for n in [[0.36f32, -0.48, 0.8], [-0.9, 0.3, 0.3162], [0.2, -0.95, -0.24]] {
            let lanes = NormalEncoding::encode_cube_face(minor, payload, n, 0.5);
            let back = normal(NormalEncoding::CubeFace { minor, payload }, 0b1111).decode(lanes);
            assert!(close(back.unwrap(), normalised(n).unwrap()), "{n:?}: {back:?}");
        }
    }

    /// A gloss is one minus its decoded value, a stored roughness is itself, a specular exponent the roughness whose
    /// alpha matches its lobe: 0 for none, 1 / 4 for 510 (`(2 / 512)^(1/4)`).
    #[test]
    fn the_roughness_is_read_from_a_gloss_a_roughness_or_an_exponent() {
        let place = Place {
            output: 0,
            lanes: 0b0010,
        };
        let term = |encoding| Term { place, encoding };
        // a gloss stored as the square of the smoothness over 0.95: 0.64 is the smoothness sqrt(0.64) * 0.95 = 0.76,
        // roughness 0.24
        let gloss = Roughness::Gloss(term(ScalarEncoding::square_of(0.95)));
        assert!((gloss.decode([0.0, 0.64, 0.0, 0.0]) - 0.24).abs() < 1e-6);
        let stored = Roughness::Roughness(term(ScalarEncoding::LINEAR));
        assert_eq!(stored.decode([0.0, 0.3, 0.0, 0.0]), 0.3);
        // an exponent stored as the square root of n / 512
        let exponent = Roughness::Exponent(term(ScalarEncoding::square_root_of(512.0)));
        assert_eq!(exponent.decode([0.0; 4]), 1.0);
        let stored_510 = (510.0f32 / 512.0).sqrt();
        assert!((exponent.decode([0.0, stored_510, 0.0, 0.0]) - 0.25).abs() < 1e-4);
    }

    fn roles(outputs: &[(u8, OutputRole)]) -> Surface {
        Surface::read(outputs.iter().map(|(lanes, role)| (*lanes, role)))
    }

    /// The first output with a role is read, an albedo wins over a finished colour wherever it is, the occlusion of every
    /// light over the environment's alone (which is kept where it is the only one), and an output whose lanes do not
    /// fit its term is refused with why.
    #[test]
    fn the_reading_takes_the_first_output_of_each_role_and_refuses_lanes_that_do_not_fit() {
        let linear = ScalarEncoding::LINEAR;
        let s = roles(&[
            (0b1111, OutputRole::Colour),
            (0b0111, OutputRole::Albedo),
            (0b0111, OutputRole::Albedo),
            (
                0b0001,
                OutputRole::Occlusion {
                    of: Occluded::NaturalAmbient,
                    encoding: linear,
                },
            ),
            (
                0b0010,
                OutputRole::Occlusion {
                    of: Occluded::AllLight,
                    encoding: linear,
                },
            ),
            (0b0011, OutputRole::Metallic),
            (0b0001, OutputRole::Metallic),
            (0b1111, OutputRole::unknown("read by nothing")),
        ]);
        assert_eq!(
            s.base,
            Some(Base::Albedo(Place {
                output: 1,
                lanes: 0b0111
            }))
        );
        assert_eq!(s.occlusion.map(|t| t.place.output), Some(4));
        assert_eq!(s.ambient_occlusion, None);
        assert_eq!(s.metallic.map(|t| t.place.output), Some(6));
        let ambient = roles(&[(
            0b0001,
            OutputRole::Occlusion {
                of: Occluded::NaturalAmbient,
                encoding: linear,
            },
        )]);
        assert_eq!(
            (
                ambient.occlusion,
                ambient.ambient_occlusion.map(|t| t.place.output)
            ),
            (None, Some(0))
        );
        assert_eq!(s.refused, ["output 5: a metallic in 2 lanes (0b0011), not 1"]);
        assert_eq!(
            (s.normal, s.roughness, s.reflectance, s.emission),
            (None, None, None, None)
        );
        // a forward program's colour is the base where nothing writes an albedo
        let forward = roles(&[(0b1111, OutputRole::Colour)]);
        assert_eq!(
            forward.base,
            Some(Base::Colour(Place {
                output: 0,
                lanes: 0b1111
            }))
        );
        // one lane of emission scales the albedo, three are its colour
        let scalar = roles(&[(0b0100, OutputRole::Emission { encoding: linear })]);
        assert!(matches!(scalar.emission, Some(Emission::OfAlbedo(_))));
        assert_eq!(
            scalar
                .emission
                .unwrap()
                .decode([0.0, 0.0, 2.0, 0.0], [0.5, 0.25, 1.0]),
            [1.0, 0.5, 2.0]
        );
        let colour = roles(&[(0b0111, OutputRole::Emission { encoding: linear })]);
        assert_eq!(
            colour.emission.unwrap().decode([0.1, 0.2, 0.3, 0.0], [1.0; 3]),
            [0.1, 0.2, 0.3]
        );
    }
}
