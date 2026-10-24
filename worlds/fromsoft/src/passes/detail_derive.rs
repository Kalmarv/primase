//! A detail-blend program's equation read from runs of it ([`ProgramProbe`]) with designed texture samples, and proven on
//! random ones: the [`DetailLerp`](primase_core::detail::DetailLerp) a program computes, or what differs.
//!
//! The textures are told apart by the samplers the program reads them through (its colour textures by the diffuse
//! sampler, its normals by the bump sampler, its mask alone), then by what moves which output ([`identify`]): the base
//! normal is the one normal texture that alone tips the normal over flat (only the base's `xy` gives the normal's `z`),
//! the base colour the one colour moving the colour at both ends of the weights, each detail the colour and normal
//! moving the outputs where the weights take it alone. The weights are the family's ([`weight_channels`]); the proof
//! holds a program to them. Each constant of the equation is read from a run whose samples isolate it (a detail taken
//! alone by the base normal's alpha, or weighed most where none is, the chain then inverted; the reflectance mix at 0 or
//! 1 by the base mask; a lane set so the term in question is unsaturated); the program's tangent frame is read from the
//! base normal alone. The equation is then proven against the program on random samples ([`PROOF_SETS`]), every decoded
//! role compared within [`BOUND_UNITS`], and each constant is compared with the material word it reads
//! ([`FieldComparison`]).
use std::collections::BTreeMap;

use primase_core::detail::{
    DETAIL_LIMIT, Detail, DetailLerp, DetailSample, DetailSamples, DetailSurface, ReflectanceTerm, chain,
};
use primase_core::layered::{Channel, Source, mask_value};
use primase_core::pbr::TextureRef;
use primase_core::shader::{NormalEncoding, OutputRole, ScalarEncoding};
use primase_shader::ResourceKind;
use primase_shader_exec::ImageSample;
use primase_shader_exec::probe::{AnsweredReads, affine_uv};

use crate::detail_probe::ProgramProbe;
pub use crate::probe_accept::BOUND_UNITS;
use crate::probe_accept::{Seeded, moved, units, word_sources};

/// How many random sample sets a proof runs.
pub const PROOF_SETS: usize = 256;

/// The texture slots of a detail program, by their part in the equation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRoles {
    pub base_colour: u32,
    pub base_normal: u32,
    pub base_mask: u32,
    /// Each detail's, in the chain's order.
    pub details: Vec<DetailSlots>,
}

/// One detail's texture slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetailSlots {
    pub colour: u32,
    pub normal: u32,
}

/// One detail's constants, as read from its program.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailTerms {
    /// The gain times the tint: the program computes `gain * A * chain(t_i * D_i)` and the gain and a tint are read only
    /// as their product, so the equation takes the gain as 1 and the product as the tint. The two orders round
    /// differently; [`FoldCost`] measures it on every proof (at most 4.5 units in the last place of the colour on
    /// Sekiro's switch 0, 13 and 6 programs).
    pub tint: [f32; 3],
    pub gloss_bias: f32,
    /// Its term of the reflectance mix, the form as measured: the one detail whose term moves with the base mask reads
    /// it.
    pub reflectance: ReflectanceTerm,
}

/// The constants of a detail program's equation, as read from it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailConstants {
    /// In the chain's order.
    pub details: Vec<DetailTerms>,
    pub normal_strength: f32,
    pub dielectric_f0: f32,
    pub colour: [f32; 3],
    /// The binding's gloss encoding: the gloss is solved as stored and decoded by it.
    pub gloss_encoding: ScalarEncoding,
}

/// A proof's result: the largest difference seen per role.
#[derive(Clone, Debug, PartialEq)]
pub struct Proof {
    pub sets: usize,
    /// The largest difference per role in units of `f32::EPSILON` of the compared magnitude.
    pub largest: BTreeMap<&'static str, f32>,
}

/// What a run decodes to: the roles the program writes.
#[derive(Clone, Copy, Debug)]
struct Observed {
    /// World space, unit length.
    normal: [f32; 3],
    /// Decoded (the smoothness), and as stored.
    gloss: f32,
    gloss_stored: f32,
    albedo: [f32; 3],
    reflectance: [f32; 3],
    emission: [f32; 3],
}

/// The output registers and encodings of the roles a detail-blend program writes.
#[derive(Clone, Copy, Debug)]
struct Outputs {
    normal: (usize, f32),
    gloss: (usize, ScalarEncoding),
    albedo: usize,
    reflectance: (usize, ScalarEncoding),
    emission: Option<(usize, ScalarEncoding)>,
}

fn outputs(probe: &ProgramProbe) -> Result<Outputs, String> {
    let register = |label: &primase_shader::Label| -> Result<usize, String> {
        probe
            .interface()
            .outputs
            .iter()
            .find(|v| v.label == *label)
            .and_then(|v| v.provenance.d3d)
            .map(|d| d.register as usize)
            .ok_or_else(|| format!("no output register for {}{}", label.name, label.index))
    };
    let (mut normal, mut gloss, mut albedo, mut reflectance, mut emission) = (None, None, None, None, None);
    for o in &probe.outputs {
        let r = register(&o.label)?;
        match o.role {
            OutputRole::Normal {
                encoding: NormalEncoding::XyzBiased { bias_bits },
                ..
            } if o.lanes == 7 => normal = Some((r, f32::from_bits(bias_bits))),
            OutputRole::Gloss { encoding } if o.lanes == 1 => gloss = Some((r, encoding)),
            OutputRole::Albedo if o.lanes == 7 => albedo = Some(r),
            OutputRole::Reflectance { encoding } if o.lanes == 7 => reflectance = Some((r, encoding)),
            OutputRole::Emission { encoding } if o.lanes == 7 => emission = Some((r, encoding)),
            _ => {
                return Err(format!(
                    "output role {:?} on lanes {} is not a detail blend's",
                    o.role, o.lanes
                ));
            }
        }
    }
    Ok(Outputs {
        normal: normal.ok_or("no biased world normal")?,
        gloss: gloss.ok_or("no gloss")?,
        albedo: albedo.ok_or("no albedo")?,
        reflectance: reflectance.ok_or("no reflectance")?,
        emission,
    })
}

/// The weights of the details after the first, by the base normal's sample: the second detail's `(1 - N.a)^8`, the
/// third's `N.a^8`, as Sekiro's DetailBlend programs compute them (the mask's channels and bias of each).
const WEIGHT_FORMS: [([f32; 4], f32); DETAIL_LIMIT - 1] = [([0., 0., 0., -1.], 1.), ([0., 0., 0., 1.], 0.)];

/// The weights of a chain of `n` details, of the base normal `base_normal`: one fewer than the details
/// ([`WEIGHT_FORMS`]).
pub fn weight_channels(n: usize, base_normal: TextureRef) -> Vec<Channel> {
    WEIGHT_FORMS[..n.saturating_sub(1)]
        .iter()
        .map(|&(channels, bias)| Channel::Power {
            v: Source::Mask {
                texture: base_normal,
                channels,
                bias,
            },
            exponent: 8,
        })
        .collect()
}

/// The weights' values for the base normal sample `normal`, as the equation computes them.
fn weights(n: usize, normal: [f32; 4]) -> Vec<f32> {
    let texture = TextureRef::new(primase_core::World::default().spawn());
    weight_channels(n, texture)
        .iter()
        .zip(&WEIGHT_FORMS)
        .map(|(w, &(channels, bias))| w.evaluate(&[mask_value(channels, bias, normal)]))
        .collect()
}

/// The weights' values at the base normal alpha `alpha`.
fn weights_at(n: usize, alpha: f32) -> Vec<f32> {
    weights(n, [0., 0., 0., alpha])
}

/// The base normal's alpha at each end of the weights.
const MASK_OFF: f32 = 1.;
const MASK_ON: f32 = 0.;

/// The alpha at which the weights take detail `j` of `n` alone (each weight 0 or 1, the last 1 its own, or none 1 for
/// the first): one end of the weights, or `None` where neither end takes it alone.
fn isolating(n: usize, j: usize) -> Option<f32> {
    [MASK_OFF, MASK_ON].into_iter().find(|&alpha| {
        let w = weights_at(n, alpha);
        w.iter().all(|&x| x == 0. || x == 1.) && w.iter().rposition(|&x| x == 1.).map_or(0, |i| i + 1) == j
    })
}

/// The alpha detail `j` of `n` is read at: where it is taken alone, else the middle, where the weights of the
/// family's both ends are smallest (`2^-8` each) and the first detail weighs most.
fn observing(n: usize, j: usize) -> f32 {
    isolating(n, j).unwrap_or(0.5)
}

/// The alpha at which detail `j`'s weight (`j` at least 1) is one half: the chain mixed half and half there.
fn half_way(n: usize, j: usize) -> f32 {
    let candidates = [1. - 0.5f32.powf(1. / 8.), 0.5f32.powf(1. / 8.)];
    let distance = |alpha: f32| (weights_at(n, alpha)[j - 1] - 0.5).abs();
    if distance(candidates[0]) <= distance(candidates[1]) {
        candidates[0]
    } else {
        candidates[1]
    }
}

/// The value of term `j` of a chain that gives `q` with the other `terms` (`terms[j]` unread) and `weights`: each lerp
/// after it undone from the end (`lerp(p, t, w) = q` gives `p = (q - t * w) / (1 - w)`), then its own
/// (`t = (q - p * (1 - w)) / w`, `p` the chain before it).
fn unlerp(q: f32, terms: &[f32], weights: &[f32], j: usize) -> f32 {
    let mut q = q;
    for i in (j + 1..terms.len()).rev() {
        let w = weights[i - 1];
        q = (q - terms[i] * w) / (1. - w);
    }
    if j == 0 {
        return q;
    }
    let before = chain(terms[..j].iter().copied(), &weights[..j - 1]);
    let w = weights[j - 1];
    (q - before * (1. - w)) / w
}

/// A detail program ready to be run with samples by role, its engine textures read as the bake reads them
/// ([`ProgramProbe::engine`]).
struct Program<'p> {
    probe: &'p ProgramProbe,
    outputs: Outputs,
    roles: &'p SlotRoles,
}

/// The samples of every role of one run.
#[derive(Clone, Debug)]
struct Samples {
    a: [f32; 4],
    n: [f32; 4],
    m: [f32; 4],
    /// Each detail's colour and normal.
    d: Vec<[f32; 4]>,
    k: Vec<[f32; 4]>,
}

impl Samples {
    fn as_core(&self) -> DetailSamples {
        DetailSamples {
            base_colour: self.a,
            base_normal: self.n,
            base_mask: self.m,
            details: self
                .d
                .iter()
                .zip(&self.k)
                .map(|(&colour, &normal)| DetailSample { colour, normal })
                .collect(),
        }
    }
}

/// One run of `probe` at `uv` with each of its textures answered from `by_slot` (the engine's as the bake reads them),
/// decoded by `outputs`.
fn observe_slots(
    probe: &ProgramProbe,
    outputs: Outputs,
    by_slot: &BTreeMap<u32, [f32; 4]>,
    uv: [f32; 2],
) -> Result<Observed, String> {
    let mut reads = AnsweredReads::new(|q: &ImageSample<u32>| {
        by_slot
            .get(&q.texture)
            .or_else(|| probe.engine.get(&q.texture))
            .copied()
            .ok_or_else(|| format!("t{} is no texture of the equation", q.texture))
    });
    let (out, discarded) = probe.pixel(uv, &mut reads).map_err(|e| e.to_string())?;
    if discarded {
        return Err("the program discarded the pixel".into());
    }
    let o = outputs;
    let lanes3 = |i: usize| [out[i][0], out[i][1], out[i][2]];
    let n = lanes3(o.normal.0).map(|v| (v - o.normal.1) * 2.);
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    Ok(Observed {
        normal: n.map(|v| v / length),
        gloss: o.gloss.1.decode(out[o.gloss.0][0]),
        gloss_stored: out[o.gloss.0][0],
        albedo: lanes3(o.albedo),
        reflectance: lanes3(o.reflectance.0).map(|v| o.reflectance.1.decode(v)),
        emission: o
            .emission
            .map_or([0.; 3], |(i, e)| lanes3(i).map(|v| e.decode(v))),
    })
}

impl Program<'_> {
    fn details(&self) -> usize {
        self.roles.details.len()
    }

    fn observe(&self, s: &Samples, uv: [f32; 2]) -> Result<Observed, String> {
        let r = self.roles;
        let mut by_slot = BTreeMap::from([(r.base_colour, s.a), (r.base_normal, s.n), (r.base_mask, s.m)]);
        for (i, d) in r.details.iter().enumerate() {
            by_slot.insert(d.colour, s.d[i]);
            by_slot.insert(d.normal, s.k[i]);
        }
        observe_slots(self.probe, self.outputs, &by_slot, uv)
    }
}

/// The designed base colour at a scale of 1.
const BASE_COLOUR: [f32; 3] = [0.1, 0.15, 0.2];

/// The designed detail colours, by the detail's place in the chain.
const DETAIL_COLOURS: [[f32; 4]; DETAIL_LIMIT] =
    [[0.5, 0.6, 0.7, 1.], [0.4, 0.5, 0.6, 1.], [0.3, 0.4, 0.5, 1.]];

/// A designed run's samples for `n` details: a mid base colour times `scale`, the base normal flat with the weights set
/// by its alpha, the details mid.
fn design(n: usize, scale: f32, alpha: f32, base_mask: f32) -> Samples {
    Samples {
        a: [
            BASE_COLOUR[0] * scale,
            BASE_COLOUR[1] * scale,
            BASE_COLOUR[2] * scale,
            1.,
        ],
        n: [0.5, 0.5, 0.5, alpha],
        m: [base_mask, 0., 0., 0.],
        d: DETAIL_COLOURS[..n].to_vec(),
        k: vec![[0.5, 0.5, 0.5, 0.]; n],
    }
}

const UV: [f32; 2] = [0.3, 0.6];

/// The base mask samples a bias is searched with, nearest zero first: quarters out to 32 either way, so a bias of any
/// size a material states leaves the mix unsaturated at one of them (the window of an unsaturated mix is at least a
/// half wide).
fn base_masks() -> impl Iterator<Item = f32> {
    (0..=128).flat_map(|i| {
        let m = i as f32 * 0.25;
        if i == 0 { vec![m] } else { vec![m, -m] }
    })
}

/// How many times the designed base colour may be halved to leave the designed colours unsaturated ([`solve`]).
const HALVINGS: i32 = 12;

/// How many rounds the reflectance biases are solved in, at most, until they settle ([`solve_at`]): a bias read where
/// another weighs in needs the other's, which may need it.
const BIAS_ROUNDS: usize = 16;

/// A program's tangent frame: x, y and z of tangent space in world space.
type Frame = [[f32; 3]; 3];

/// A solve's result: the constants, the program's tangent frame, and the scale of the designed base colour it read them
/// at.
type Solved = (DetailConstants, Frame, f32);

/// The constants of the equation as `program` computes them, read run by run with designed samples. The material's
/// words are taken as they are (none clamped); where its tints and gain saturate the designed colour, the designed base
/// colour is halved until no designed colour saturates (at most [`HALVINGS`] times), the scale returned.
fn solve(program: &Program<'_>) -> Result<Solved, String> {
    let n = program.details();
    for halvings in 0..=HALVINGS {
        let scale = 0.5f32.powi(halvings);
        if let Some((constants, frame)) = solve_at(program, scale)? {
            // each detail whose colour is read from two base colours ([`solve_at`]) unsaturated at the second, twice
            let s = design(n, 2. * scale, MASK_ON, 0.);
            let unsaturated = constants.details.iter().enumerate().all(|(j, d)| {
                matches!(d.reflectance, ReflectanceTerm::MaskPlusBias(_))
                    || (0..3).all(|i| d.tint[i] * s.a[i] * s.d[j][i] < 1.)
            });
            if unsaturated {
                return Ok((constants, frame, scale));
            }
        }
    }
    Err(format!(
        "the designed colour saturates at every scale of the base colour down to 1/{}",
        1 << HALVINGS
    ))
}

/// Which detail's reflectance term reads the base mask: the one whose observed colour moves with it where it is taken
/// alone, or, where none taken alone does, the one no end takes alone.
fn masked_detail(program: &Program<'_>, scale: f32) -> Result<usize, String> {
    let n = program.details();
    let moves = |j: usize| -> Result<bool, String> {
        let alpha = observing(n, j);
        let (high, low) = (
            program.observe(&design(n, scale, alpha, 100.), UV)?,
            program.observe(&design(n, scale, alpha, -100.), UV)?,
        );
        Ok(colour_differs(&high, &low))
    };
    let mut reading = Vec::new();
    for j in (0..n).filter(|&j| isolating(n, j).is_some()) {
        if moves(j)? {
            reading.push(j);
        }
    }
    let unisolated: Vec<usize> = (0..n).filter(|&j| isolating(n, j).is_none()).collect();
    if reading.is_empty() {
        for &j in &unisolated {
            if moves(j)? {
                reading.push(j);
            }
        }
    } else if !unisolated.is_empty() {
        let list = |js: &[usize]| js.iter().map(|j| j.to_string()).collect::<Vec<_>>().join(", ");
        return Err(format!(
            "the reflectance term of detail {} reads the base mask, and no end of the weights takes detail {} alone \
             to read its own",
            list(&reading),
            list(&unisolated)
        ));
    }
    match reading.as_slice() {
        [p] => Ok(*p),
        [] => Err("no detail's reflectance term reads the base mask".into()),
        _ => Err(format!(
            "the reflectance terms of details {reading:?} read the base mask, not one"
        )),
    }
}

/// [`solve`] with the designed base colour at `scale`: `None` where a designed colour saturates.
fn solve_at(program: &Program<'_>, scale: f32) -> Result<Option<(DetailConstants, Frame)>, String> {
    let n = program.details();
    let run = |s: &Samples| program.observe(s, UV);
    let design = |alpha: f32, base_mask: f32| design(n, scale, alpha, base_mask);
    let p = masked_detail(program, scale)?;
    let at_p = observing(n, p);
    let p_alone = isolating(n, p).is_some();
    // the mix f at 1 (the base mask far above any bias) where the masked detail is read: the reflectance is C
    let full = run(&design(at_p, 100.))?;
    let s = design(at_p, 100.);
    if full.albedo.iter().any(|&a| a != 0.) {
        return Err(format!(
            "the albedo is {:?} where the reflectance mix is 1",
            full.albedo
        ));
    }
    let c_p = full.reflectance;
    if c_p.iter().any(|&c| c >= 1.) {
        return Ok(None);
    }
    if c_p.iter().any(|&c| c <= 0.) {
        return Err(format!(
            "detail {p}'s colour {c_p:?} is zero at the designed sample"
        ));
    }
    let mut tints = vec![[0f32; 3]; n];
    if p_alone {
        tints[p] = std::array::from_fn(|i| c_p[i] / (s.a[i] * s.d[p][i]));
    }
    // the mix at 0: the reflectance is the dielectric's, the albedo C times the colour
    let none = run(&design(at_p, -100.))?;
    let f0 = none.reflectance[0];
    if none.reflectance.iter().any(|&r| r != f0) {
        return Err(format!(
            "the dielectric reflectance {:?} is not grey",
            none.reflectance
        ));
    }
    let colour: [f32; 3] = std::array::from_fn(|i| none.albedo[i] / c_p[i]);
    // each other detail, its term the bias alone, where it is taken alone: two base colours, the second twice the
    // first, give its mix from the slopes, then its colour
    let mut mixes = vec![0f32; n];
    for j in (0..n).filter(|&j| j != p) {
        let alpha = isolating(n, j).ok_or_else(|| {
            format!(
                "detail {j}'s reflectance term is its bias alone and no end of the weights takes it alone"
            )
        })?;
        let on = |factor: f32| {
            let mut s = design(alpha, 0.);
            for v in &mut s.a[..3] {
                *v *= factor;
            }
            run(&s)
        };
        let (one, two) = (on(1.)?, on(2.)?);
        let s = design(alpha, 0.);
        let k = 2;
        let d_albedo = two.albedo[k] - one.albedo[k];
        let d_reflectance = two.reflectance[k] - one.reflectance[k];
        let mix = if one.albedo.iter().all(|&a| a == 0.) {
            1.
        } else {
            let ratio = d_reflectance / (d_albedo / colour[k]);
            ratio / (1. + ratio)
        };
        let c: [f32; 3] = std::array::from_fn(|i| {
            if mix < 1. {
                one.albedo[i] / ((1. - mix) * colour[i])
            } else {
                one.reflectance[i]
            }
        });
        tints[j] = std::array::from_fn(|i| c[i] / (s.a[i] * s.d[j][i]));
        mixes[j] = mix;
    }
    // the masked detail where no end takes it alone: its term of the colour's chain, the others known
    if !p_alone {
        let w = weights_at(n, at_p);
        tints[p] = std::array::from_fn(|i| {
            let terms: Vec<f32> = (0..n).map(|k| tints[k][i] * s.d[k][i]).collect();
            unlerp(c_p[i] / s.a[i], &terms, &w, p) / s.d[p][i]
        });
    }
    // the reflectance biases: each read where the mix is unsaturated, the masked detail's where it is read, every
    // other's half way through its weight; each needs the others where they weigh in, so they are read in rounds until
    // they settle (at once where the masked detail is taken alone)
    let mut bias = mixes;
    let term = |j: usize, bias: &[f32], m: f32| if j == p { m + bias[j] } else { bias[j] };
    let mut settled = false;
    for _ in 0..BIAS_ROUNDS {
        let before = bias.clone();
        bias[p] = (|| {
            let w = weights_at(n, at_p);
            for m in base_masks() {
                let o = run(&design(at_p, m))?;
                let f = (o.reflectance[2] - f0) / (c_p[2] - f0);
                if f > 0.01 && f < 0.99 {
                    if p_alone {
                        return Ok(f - m);
                    }
                    let terms: Vec<f32> = (0..n).map(|k| term(k, &bias, m)).collect();
                    return Ok(unlerp(f, &terms, &w, p) - m);
                }
            }
            Err(format!(
                "no base mask leaves the reflectance mix unsaturated where detail {p} is read"
            ))
        })()?;
        for j in (0..n).filter(|&j| j != p) {
            let alpha = half_way(n, j);
            let w = weights_at(n, alpha);
            bias[j] = (|| {
                for m in base_masks() {
                    let o = run(&design(alpha, m))?;
                    let s = design(alpha, m);
                    let colour_at = {
                        let terms = (0..n).map(|k| tints[k][2] * s.d[k][2]);
                        (s.a[2] * chain(terms, &w)).clamp(0., 1.)
                    };
                    let f = (o.reflectance[2] - f0) / (colour_at - f0);
                    if f > 0.01 && f < 0.99 {
                        let terms: Vec<f32> = (0..n).map(|k| term(k, &bias, m)).collect();
                        return Ok(unlerp(f, &terms, &w, j));
                    }
                }
                Err(format!(
                    "no base mask leaves the reflectance mix unsaturated half way through detail {j}'s weight"
                ))
            })()?;
        }
        if bias.iter().zip(&before).all(|(a, b)| a.to_bits() == b.to_bits()) {
            settled = true;
            break;
        }
    }
    if !settled {
        return Err(format!(
            "the reflectance biases do not settle in {BIAS_ROUNDS} rounds"
        ));
    }
    // the tangent frame, from the base normal alone (the details flat): x, y and z of tangent space
    let flat = |nx: f32, ny: f32| {
        let mut s = design(MASK_OFF, 0.);
        s.n[0] = nx;
        s.n[1] = ny;
        run(&s).map(|o| o.normal)
    };
    let z = flat(0.5, 0.5)?;
    let (along_x, along_y) = (flat(0.75, 0.5)?, flat(0.5, 0.75)?);
    let root = 0.75f32.sqrt();
    let frame = [
        std::array::from_fn(|i| (along_x[i] - root * z[i]) / 0.5),
        std::array::from_fn(|i| (along_y[i] - root * z[i]) / 0.5),
        z,
    ];
    let to_tangent = |world: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|axis| (0..3).map(|i| frame[axis][i] * world[i]).sum())
    };
    // the normal strength: the detail one end takes alone tilted fully along x (its term `2 * 1 - 1`), the base flat;
    // the G-buffer stores the normal biased, its precision absolute, so the largest tilt reads the strength most
    // closely
    let tilted = (0..n)
        .find(|&j| isolating(n, j) == Some(MASK_OFF))
        .ok_or("no detail is taken alone where the base normal's alpha is 1")?;
    let mut s = design(MASK_OFF, 0.);
    s.k[tilted][0] = 1.;
    let t = to_tangent(run(&s)?.normal);
    let normal_strength = t[0] / t[2];
    // the gloss biases: a base normal z and a detail z that leave the gloss unsaturated, each detail where it is
    // read, those no end takes alone after the others
    let mut gloss_bias = vec![0f32; n];
    let mut order: Vec<usize> = (0..n).filter(|&j| isolating(n, j).is_some()).collect();
    order.extend((0..n).filter(|&j| isolating(n, j).is_none()));
    for j in order {
        let alpha = observing(n, j);
        let alone = isolating(n, j).is_some();
        let w = weights_at(n, alpha);
        gloss_bias[j] = (|| {
            for kz in [0.5, 1., 0.] {
                for nz in (1..=8).map(|i| 0.5f32.powi(i)) {
                    let mut s = design(alpha, 0.);
                    s.n[2] = nz;
                    s.k[j][2] = kz;
                    let g = run(&s)?.gloss_stored;
                    if g > 0.01 && g < 0.99 {
                        if alone {
                            return Ok(g / (2. * nz) - kz);
                        }
                        let terms: Vec<f32> = (0..n).map(|k| s.k[k][2] + gloss_bias[k]).collect();
                        return Ok(unlerp(g / (2. * nz), &terms, &w, j) - kz);
                    }
                }
            }
            Err(format!(
                "no base normal z leaves the gloss unsaturated where detail {j} is read"
            ))
        })()?;
    }
    let details = (0..n)
        .map(|j| DetailTerms {
            tint: tints[j],
            gloss_bias: gloss_bias[j],
            reflectance: if j == p {
                ReflectanceTerm::MaskPlusBias(bias[j])
            } else {
                ReflectanceTerm::Bias(bias[j])
            },
        })
        .collect();
    Ok(Some((
        DetailConstants {
            details,
            normal_strength,
            dielectric_f0: f0,
            colour,
            gloss_encoding: program.outputs.gloss.1,
        },
        frame,
    )))
}

/// One random sample set of `n` details: the base's, then each detail's colour, then each one's normal.
fn random_samples(random: &mut Seeded, n: usize) -> Samples {
    let (a, normal, m) = (random.lanes(), random.lanes(), random.lanes());
    let d = (0..n).map(|_| random.lanes()).collect();
    let k = (0..n).map(|_| random.lanes()).collect();
    Samples {
        a,
        n: normal,
        m,
        d,
        k,
    }
}

/// The equation of `c` with `gain` (1 where `c`'s tints fold it, [`DetailTerms::tint`]) and each detail's texture
/// `texture`.
fn detail_lerp(c: &DetailConstants, gain: f32, texture: TextureRef) -> DetailLerp {
    DetailLerp {
        base_colour: texture,
        base_normal: texture,
        base_mask: texture,
        details: c
            .details
            .iter()
            .map(|d| Detail {
                colour: texture,
                normal: texture,
                tint: d.tint,
                gloss_bias: d.gloss_bias,
                reflectance: d.reflectance,
            })
            .collect(),
        weights: weight_channels(c.details.len(), texture),
        gain,
        normal_strength: c.normal_strength,
        dielectric_f0: c.dielectric_f0,
        colour: c.colour,
        gloss_encoding: c.gloss_encoding,
    }
}

/// The equation's surface for `s` with `c` and `gain` ([`detail_lerp`]).
fn equation(c: &DetailConstants, gain: f32, s: &Samples) -> DetailSurface {
    let texture = TextureRef::new(primase_core::World::default().spawn());
    detail_lerp(c, gain, texture).evaluate(&s.as_core(), &weights(c.details.len(), s.n))
}

/// `program` against the equation of `c` on [`PROOF_SETS`] random sample sets at random coordinates: the largest
/// difference per role, or the first set past [`BOUND_UNITS`] (or with an emission).
fn prove(program: &Program<'_>, c: &DetailConstants, frame: &Frame, seed: u32) -> Result<Proof, String> {
    let mut random = Seeded::new(seed);
    let mut largest: BTreeMap<&'static str, f32> = BTreeMap::new();
    let limit = BOUND_UNITS;
    for set in 0..PROOF_SETS {
        let s = random_samples(&mut random, program.details());
        let uv = [random.unit(), random.unit()];
        let seen = program.observe(&s, uv)?;
        let want = equation(c, 1., &s);
        // the equation's tangent normal through the program's frame, unit length
        let n = want.normal;
        let world: [f32; 3] = std::array::from_fn(|i| (0..3).map(|axis| frame[axis][i] * n[axis]).sum());
        let length = (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
        let mut check = |role: &'static str, got: f32, wanted: f32| -> Result<(), String> {
            let u = units(got, wanted);
            let e = largest.entry(role).or_default();
            *e = e.max(u);
            if u > limit {
                return Err(format!(
                    "set {set}: {role} {got} where the equation gives {wanted} ({u:.0} units, past {limit:.0})"
                ));
            }
            Ok(())
        };
        for (i, w) in world.iter().enumerate() {
            check("albedo", seen.albedo[i], want.albedo[i])?;
            check("reflectance", seen.reflectance[i], want.reflectance[i])?;
            check("normal", seen.normal[i], w / length)?;
            check("emission", seen.emission[i], 0.)?;
        }
        check("gloss", seen.gloss, want.gloss)?;
    }
    Ok(Proof {
        sets: PROOF_SETS,
        largest,
    })
}

/// The material's texture slots the program reads, grouped by the sampler each is read through, each group in the
/// order the program first reads its textures.
fn sampler_groups(probe: &ProgramProbe) -> Result<BTreeMap<u32, Vec<u32>>, String> {
    let mut reads = AnsweredReads::new(|_: &ImageSample<u32>| Ok([0.5; 4]));
    probe.pixel(UV, &mut reads).map_err(|e| e.to_string())?;
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for r in reads
        .samples
        .iter()
        .filter(|r| !probe.engine.contains_key(&r.texture))
    {
        let g = groups.entry(r.sampler).or_default();
        if !g.contains(&r.texture) {
            g.push(r.texture);
        }
    }
    Ok(groups)
}

/// The sampler slot named `name` (`SS_DiffuseMap`, `SS_BumpMap`).
fn sampler_slot(probe: &ProgramProbe, name: &str) -> Option<u32> {
    probe.interface().resources.iter().find_map(|r| {
        (matches!(r.kind, ResourceKind::Sampler { .. }) && r.provenance.name == name)
            .then(|| r.provenance.d3d.map(|d| d.register))
            .flatten()
    })
}

/// The tiling the program samples texture `slot` at: the product of its coordinate map's scales ([`affine_uv`], from
/// runs at four coordinates).
fn tiling(probe: &ProgramProbe, slot: u32) -> Result<f64, String> {
    let mut points = Vec::new();
    for uv in [[0.1, 0.2], [0.7, 0.3], [0.4, 0.9], [0.9, 0.8]] {
        let mut reads = AnsweredReads::new(|_: &ImageSample<u32>| Ok([0.5; 4]));
        probe.pixel(uv, &mut reads).map_err(|e| e.to_string())?;
        let sample = reads
            .samples
            .iter()
            .find(|r| r.texture == slot)
            .ok_or_else(|| format!("t{slot} is not sampled"))?;
        points.push((uv, [sample.coord[0], sample.coord[1]]));
    }
    let map = affine_uv(&points).map_err(|e| format!("t{slot}'s coordinate: {e}"))?;
    Ok((map.scale[0] * map.scale[1]).abs())
}

/// A solved constant's lane beside the material word the program reads it from (condition of the derivation: the solved
/// value is the one used, the word is evidence of it).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldComparison {
    /// The constant and lane, as `tint[0][2]`.
    pub constant: String,
    pub solved: f32,
    /// The material words a change of which moves the constant (past [`BOUND_UNITS`]): each field's name and its word, by
    /// the change of one word at a time. None: the program's own constant.
    pub fields: Vec<(String, u32)>,
    /// What the solved value should be from the word, where `fields` is one: the word itself, or for a tint the gain
    /// times the word ([`DetailTerms::tint`]).
    pub expected: Option<f32>,
}

impl FieldComparison {
    /// Whether the solved value is the expected one, within [`BOUND_UNITS`]; `None` where there is no one word to compare.
    pub fn agrees(&self) -> Option<bool> {
        self.expected.map(|e| units(self.solved, e) <= BOUND_UNITS)
    }
}

/// The cost of the tint fold ([`DetailTerms::tint`]) on one program: the equation with the gain and the tints read from
/// the material's words beside the folded one, on the proof's random sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldCost {
    /// The gain: the first tint's lane over its word, the same for every tint lane (each compared in [`FieldComparison`]).
    pub gain: f32,
    /// The largest difference of the albedo and reflectance, folded against unfolded, in units of `f32::EPSILON` of
    /// the larger magnitude.
    pub largest: f32,
}

/// A detail program's equation: its roles, constants and proof.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    pub roles: SlotRoles,
    pub constants: DetailConstants,
    pub proof: Proof,
    /// The program's tangent frame (x, y, z of tangent space in world space) under the probe's frozen inputs.
    pub frame: [[f32; 3]; 3],
    /// The scale of the designed base colour the constants were read at: 1, or halved until the material's tints and
    /// gain leave the designed colours unsaturated ([`solve`]). No word of the material is clamped.
    pub design_scale: f32,
    /// Each solved constant's lane beside the word it reads ([`FieldComparison`]).
    pub fields: Vec<FieldComparison>,
    /// The fold's cost, where every tint lane reads one word.
    pub fold: Option<FoldCost>,
}

impl Derived {
    /// The comparisons whose solved value is not the word's.
    pub fn disagreements(&self) -> impl Iterator<Item = &FieldComparison> {
        self.fields.iter().filter(|f| f.agrees() == Some(false))
    }

    /// The equation as a [`DetailLerp`], each texture slot's reference `texture(slot)`.
    pub fn detail_lerp(&self, texture: impl Fn(u32) -> TextureRef) -> DetailLerp {
        let base_normal = texture(self.roles.base_normal);
        let c = &self.constants;
        DetailLerp {
            base_colour: texture(self.roles.base_colour),
            base_normal,
            base_mask: texture(self.roles.base_mask),
            details: self
                .roles
                .details
                .iter()
                .zip(&c.details)
                .map(|(slots, d)| Detail {
                    colour: texture(slots.colour),
                    normal: texture(slots.normal),
                    tint: d.tint,
                    gloss_bias: d.gloss_bias,
                    reflectance: d.reflectance,
                })
                .collect(),
            weights: weight_channels(c.details.len(), base_normal),
            gain: 1.,
            normal_strength: c.normal_strength,
            dielectric_f0: c.dielectric_f0,
            colour: c.colour,
            gloss_encoding: c.gloss_encoding,
        }
    }
}

/// The constants' lanes by name, in a fixed order: each detail's tint lanes, the normal strength, each gloss bias, each
/// reflectance bias, the dielectric reflectance, the colour's lanes.
fn constant_lanes(c: &DetailConstants) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for (j, d) in c.details.iter().enumerate() {
        out.extend(
            d.tint
                .iter()
                .enumerate()
                .map(|(i, &v)| (format!("tint[{j}][{i}]"), v)),
        );
    }
    out.push(("normal_strength".into(), c.normal_strength));
    out.extend(
        c.details
            .iter()
            .enumerate()
            .map(|(j, d)| (format!("gloss_bias[{j}]"), d.gloss_bias)),
    );
    out.extend(
        c.details
            .iter()
            .enumerate()
            .map(|(j, d)| (format!("reflectance_bias[{j}]"), d.reflectance.bias())),
    );
    out.push(("dielectric_f0".into(), c.dielectric_f0));
    out.extend(
        c.colour
            .iter()
            .enumerate()
            .map(|(i, &v)| (format!("colour[{i}]"), v)),
    );
    out
}

/// Each constant of `probe`'s program, as solved, beside the material word it reads ([`word_sources`]: each word
/// changed alone and the constants solved again).
fn compare_fields(
    probe: &mut ProgramProbe,
    outputs: Outputs,
    roles: &SlotRoles,
) -> Result<Vec<FieldComparison>, String> {
    let read = |probe: &ProgramProbe| {
        solve(&Program {
            probe,
            outputs,
            roles,
        })
        .map(|(c, _, _)| constant_lanes(&c))
    };
    let lanes = word_sources(probe, read)?;
    // the gain: the first tint lane over its word, where it reads one
    let gain = match lanes[0].2.as_slice() {
        [w] if w.value != 0. => Some(lanes[0].1 / w.value),
        _ => None,
    };
    Ok(lanes
        .into_iter()
        .map(|(constant, solved, sources)| {
            let expected = match sources.as_slice() {
                [w] if constant.starts_with("tint") => gain.map(|g| g * w.value),
                [w] => Some(w.value),
                _ => None,
            };
            FieldComparison {
                constant,
                solved,
                fields: sources.into_iter().map(|w| (w.field, w.word)).collect(),
                expected,
            }
        })
        .collect())
}

/// The fold's cost: the equation of `c` against the one with `gain` and the tints `words` it was read from, on the
/// proof's random sets ([`prove`]'s sequence for `seed`); an error past [`BOUND_UNITS`].
fn fold_cost(c: &DetailConstants, gain: f32, words: &[[f32; 3]], seed: u32) -> Result<FoldCost, String> {
    let mut random = Seeded::new(seed);
    let unfolded_constants = DetailConstants {
        details: c
            .details
            .iter()
            .zip(words)
            .map(|(d, &tint)| DetailTerms { tint, ..*d })
            .collect(),
        ..c.clone()
    };
    let mut largest = 0f32;
    for _ in 0..PROOF_SETS {
        let s = random_samples(&mut random, c.details.len());
        let _uv = [random.unit(), random.unit()];
        let folded = equation(c, 1., &s);
        let unfolded = equation(&unfolded_constants, gain, &s);
        for i in 0..3 {
            largest = largest
                .max(units(folded.albedo[i], unfolded.albedo[i]))
                .max(units(folded.reflectance[i], unfolded.reflectance[i]));
        }
    }
    if largest > BOUND_UNITS {
        return Err(format!(
            "the tint fold moves the colour {largest} units, past {BOUND_UNITS}"
        ));
    }
    Ok(FoldCost { gain, largest })
}

/// The constants, proof and comparisons of `probe`'s program with its textures assigned as `roles`.
fn derive_with(
    probe: &mut ProgramProbe,
    outputs: Outputs,
    roles: SlotRoles,
    seed: u32,
) -> Result<Derived, String> {
    let program = Program {
        probe,
        outputs,
        roles: &roles,
    };
    let (constants, frame, design_scale) = solve(&program)?;
    let proof = prove(&program, &constants, &frame, seed)
        .map_err(|e| format!("{constants:?} frame {frame:?}: {e}"))?;
    let fields = compare_fields(probe, outputs, &roles)?;
    let tint_lanes = 3 * constants.details.len();
    let word_of = |f: &FieldComparison| match f.fields.as_slice() {
        [(name, k)] => probe
            .material
            .iter()
            .find(|m| m.name == *name)
            .and_then(|m| probe.words(m.buffer, m.word + k, 1))
            .map(|w| f32::from_bits(w[0])),
        _ => None,
    };
    let tint_words: Option<Vec<f32>> = fields[..tint_lanes].iter().map(word_of).collect();
    let fold = match tint_words {
        Some(w) if fields[..tint_lanes].iter().all(|f| f.agrees() == Some(true)) => {
            let gain = constants.details[0].tint[0] / w[0];
            let words: Vec<[f32; 3]> = w.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
            Some(fold_cost(&constants, gain, &words, seed)?)
        }
        _ => None,
    };
    Ok(Derived {
        roles,
        constants,
        proof,
        frame,
        design_scale,
        fields,
        fold,
    })
}

/// Texture slots as `t0, t3` (`none` for none).
fn slots(t: &[u32]) -> String {
    if t.is_empty() {
        return "none".into();
    }
    t.iter().map(|t| format!("t{t}")).collect::<Vec<_>>().join(", ")
}

/// Whether any decoded role differs between two runs past [`BOUND_UNITS`].
fn differ(a: &Observed, b: &Observed) -> bool {
    let lanes = |o: &Observed| {
        let mut v = Vec::with_capacity(13);
        v.extend(o.albedo);
        v.extend(o.reflectance);
        v.extend(o.normal);
        v.extend(o.emission);
        v.push(o.gloss_stored);
        v
    };
    lanes(a).iter().zip(lanes(b)).any(|(x, y)| moved(*x, y))
}

/// Whether the colour (albedo or reflectance) differs between two runs past [`BOUND_UNITS`].
fn colour_differs(a: &Observed, b: &Observed) -> bool {
    a.albedo
        .iter()
        .chain(&a.reflectance)
        .zip(b.albedo.iter().chain(&b.reflectance))
        .any(|(x, y)| moved(*x, *y))
}

/// Whether the normal differs between two runs past [`BOUND_UNITS`].
fn normal_differs(a: &Observed, b: &Observed) -> bool {
    a.normal.iter().zip(b.normal).any(|(x, y)| moved(*x, y))
}

/// Why a program's textures are not told apart ([`identify`]).
#[derive(Debug)]
enum Unidentified {
    /// No colour texture moves the colour: it is multiplied by something the bake freezes ([`frozen_factors`] names it).
    ColourUnmoved(String),
    /// The program's structure differs from the equation's, as stated.
    Differs(String),
}

impl From<String> for Unidentified {
    fn from(why: String) -> Self {
        Unidentified::Differs(why)
    }
}

impl From<&str> for Unidentified {
    fn from(why: &str) -> Self {
        Unidentified::Differs(why.to_owned())
    }
}

/// The textures of a detail program by their part in the equation, each read from what moves which output:
///
/// - the base normal: the one normal texture that, at `(1, 0.5)` with the others flat, turns the normal a right angle
///   from flat (the base's `xy` gives the normal's `z`, `sqrt(1 - 1)`; a detail's only adds to `xy`);
/// - the weights: for two or more details the base normal's alpha moves the outputs, for one it does not;
/// - where the weights take a detail alone (at each end), the colour textures and the normal textures that move the
///   colour and the normal: the base colour the one moving the colour at both ends, each detail the other there; a
///   detail no end takes alone the colour and normal left;
/// - for one detail, the base colour and the detail colour enter the equation only as a product: the detail is the one
///   the program samples at the larger tiling ([`tiling`]); at equal tiling the choice changes no output, and the one
///   the program samples first is taken as the detail.
///
/// An error names the structure that differs.
fn identify(probe: &ProgramProbe, outputs: Outputs) -> Result<SlotRoles, Unidentified> {
    let groups = sampler_groups(probe)?;
    let colour_sampler = sampler_slot(probe, "SS_DiffuseMap").ok_or("no SS_DiffuseMap sampler")?;
    let normal_sampler = sampler_slot(probe, "SS_BumpMap").ok_or("no SS_BumpMap sampler")?;
    let colours = groups.get(&colour_sampler).cloned().unwrap_or_default();
    let normals = groups.get(&normal_sampler).cloned().unwrap_or_default();
    let masks: Vec<u32> = groups
        .iter()
        .filter(|(s, _)| **s != colour_sampler && **s != normal_sampler)
        .flat_map(|(_, t)| t.iter().copied())
        .collect();
    let [mask] = masks.as_slice() else {
        return Err(format!("{} mask textures, not one", masks.len()).into());
    };
    let n = colours.len().saturating_sub(1);
    if colours.len() != normals.len() || !(1..=DETAIL_LIMIT).contains(&n) {
        return Err(format!(
            "{} colour and {} normal textures, not a base and 1 to {DETAIL_LIMIT} details of each",
            colours.len(),
            normals.len()
        )
        .into());
    }
    // dim colours (a gain and tint of several times one leave them unsaturated), flat normals, a mid mask
    let mut mid: BTreeMap<u32, [f32; 4]> = BTreeMap::new();
    mid.extend(colours.iter().map(|&c| (c, [0.05, 0.05, 0.05, 1.])));
    mid.extend(normals.iter().map(|&n| (n, [0.5, 0.5, 0.5, 0.5])));
    mid.insert(*mask, [0.5, 0., 0., 0.]);
    let run = |answers: &BTreeMap<u32, [f32; 4]>| observe_slots(probe, outputs, answers, UV);
    let with = |answers: &BTreeMap<u32, [f32; 4]>, slot: u32, f: &dyn Fn(&mut [f32; 4])| {
        let mut a = answers.clone();
        if let Some(v) = a.get_mut(&slot) {
            f(v);
        }
        a
    };
    // the base normal: the one that turns the normal a right angle from flat
    let flat = run(&mid)?.normal;
    let mut right_angle = Vec::new();
    for &t in &normals {
        let tipped = run(&with(&mid, t, &|v| {
            v[0] = 1.;
            v[1] = 0.5;
        }))?
        .normal;
        let cosine: f32 = flat.iter().zip(tipped).map(|(a, b)| a * b).sum();
        if !moved(cosine, 0.) {
            right_angle.push(t);
        }
    }
    let [base_normal] = right_angle.as_slice() else {
        return Err(format!(
            "normal textures {} turn the normal a right angle from flat at (1, 0.5), not one: no one base normal gives \
             its z",
            slots(&right_angle)
        )
        .into());
    };
    let alpha_moves = differ(
        &run(&with(&mid, *base_normal, &|v| v[3] = 1.))?,
        &run(&with(&mid, *base_normal, &|v| v[3] = 0.))?,
    );
    match (n, alpha_moves) {
        (1, true) => {
            return Err("the base normal's alpha moves the outputs of a program with one detail".into());
        }
        (2.., false) => {
            return Err(
                "no normal texture's alpha moves the outputs: the mask is not the base normal's (1 - N.a)^8"
                    .into(),
            );
        }
        _ => {}
    }
    // what moves the colour among `colours` with `answers`
    let moving_colour = |answers: &BTreeMap<u32, [f32; 4]>| -> Result<Vec<u32>, String> {
        let reference = run(answers)?;
        let mut moving = Vec::new();
        for &c in &colours {
            if colour_differs(&reference, &run(&with(answers, c, &|v| v[..3].fill(0.025)))?) {
                moving.push(c);
            }
        }
        Ok(moving)
    };
    if n == 1 {
        if moving_colour(&mid)?.is_empty() {
            return Err(Unidentified::ColourUnmoved(
                "no colour texture moves the colour".into(),
            ));
        }
        let detail_normal = normals
            .iter()
            .copied()
            .find(|t| t != base_normal)
            .ok_or("no detail normal")?;
        // the base colour and the detail colour enter only as a product: the detail is the one tiled more
        let (first, second) = (colours[0], colours[1]);
        let (detail, base) = if tiling(probe, second)? > tiling(probe, first)? {
            (second, first)
        } else {
            (first, second)
        };
        return Ok(SlotRoles {
            base_colour: base,
            base_normal: *base_normal,
            base_mask: *mask,
            details: vec![DetailSlots {
                colour: detail,
                normal: detail_normal,
            }],
        });
    }
    // what moves the colour and the normal with the base normal's alpha at `alpha`
    let at = |alpha: f32| -> Result<(Vec<u32>, Vec<u32>), String> {
        let end = with(&mid, *base_normal, &|v| v[3] = alpha);
        let reference = run(&end)?;
        let moving_colour = moving_colour(&end)?;
        let mut moving_normal = Vec::new();
        for &t in normals.iter().filter(|&&t| t != *base_normal) {
            if normal_differs(&reference, &run(&with(&end, t, &|v| v[0] = 0.75))?) {
                moving_normal.push(t);
            }
        }
        Ok((moving_colour, moving_normal))
    };
    // each end of the weights takes one detail alone
    let ends: Vec<(usize, f32)> = (0..n)
        .filter_map(|j| isolating(n, j).map(|alpha| (j, alpha)))
        .collect();
    let mut moving = Vec::new();
    for &(j, alpha) in &ends {
        moving.push((j, alpha, at(alpha)?));
    }
    if moving.iter().all(|(_, _, (c, _))| c.is_empty()) {
        return Err(Unidentified::ColourUnmoved(
            "no colour texture moves the colour at either end of the mask".into(),
        ));
    }
    let both: Vec<u32> = colours
        .iter()
        .copied()
        .filter(|c| moving.iter().all(|(_, _, (m, _))| m.contains(c)))
        .collect();
    let [base_colour] = both.as_slice() else {
        let at_ends: Vec<String> = moving
            .iter()
            .map(|(_, alpha, (c, _))| format!("{} where the base normal's alpha is {alpha}", slots(c)))
            .collect();
        return Err(Unidentified::Differs(if both.is_empty() {
            format!(
                "no colour texture moves the colour at both ends of the mask ({}): the base colour multiplies one \
                 detail only",
                at_ends.join(", ")
            )
        } else {
            format!(
                "colour textures {} move the colour at both ends of the mask, not one",
                slots(&both)
            )
        }));
    };
    let mut details: Vec<Option<DetailSlots>> = vec![None; n];
    for (j, alpha, (c, k)) in &moving {
        let rest: Vec<u32> = c.iter().copied().filter(|t| t != base_colour).collect();
        match (rest.as_slice(), k.as_slice()) {
            ([colour], [normal]) => {
                details[*j] = Some(DetailSlots {
                    colour: *colour,
                    normal: *normal,
                })
            }
            _ => {
                return Err(format!(
                    "where the base normal's alpha is {alpha}: colours {} and normals {} move the outputs, not one \
                     detail's each",
                    slots(&rest),
                    slots(k)
                )
                .into());
            }
        }
    }
    // a detail no end takes alone: the colour and normal left
    let taken: Vec<DetailSlots> = details.iter().flatten().copied().collect();
    let left_colours: Vec<u32> = colours
        .iter()
        .copied()
        .filter(|c| c != base_colour && !taken.iter().any(|d| d.colour == *c))
        .collect();
    let left_normals: Vec<u32> = normals
        .iter()
        .copied()
        .filter(|t| t != base_normal && !taken.iter().any(|d| d.normal == *t))
        .collect();
    let unfilled = details.iter().filter(|d| d.is_none()).count();
    if left_colours.len() != unfilled || left_normals.len() != unfilled || unfilled > 1 {
        return Err(format!(
            "{unfilled} details no end of the weights takes alone, and colours {} and normals {} left",
            slots(&left_colours),
            slots(&left_normals)
        )
        .into());
    }
    let details: Vec<DetailSlots> = details
        .into_iter()
        .map(|d| {
            d.unwrap_or_else(|| DetailSlots {
                colour: left_colours[0],
                normal: left_normals[0],
            })
        })
        .collect();
    Ok(SlotRoles {
        base_colour: *base_colour,
        base_normal: *base_normal,
        base_mask: *mask,
        details,
    })
}

/// The program's engine constants (as the bake freezes them, [`ProgramProbe::engine_fields`]) that, set to one, make
/// the colour of `answers` move: what a colour that no texture moves is multiplied by. The words are restored.
fn frozen_factors(
    probe: &mut ProgramProbe,
    outputs: Outputs,
    answers: &BTreeMap<u32, [f32; 4]>,
) -> Result<Vec<String>, String> {
    let reference = observe_slots(probe, outputs, answers, UV)?;
    let mut found = Vec::new();
    for f in probe.engine_fields.clone() {
        let Some(saved) = probe.words(f.buffer, f.word, f.words) else {
            continue;
        };
        let ones = vec![1f32.to_bits(); f.words as usize];
        probe
            .set_words(f.buffer, f.word, &ones)
            .map_err(|e| e.to_string())?;
        let changed = observe_slots(probe, outputs, answers, UV);
        probe
            .set_words(f.buffer, f.word, &saved)
            .map_err(|e| e.to_string())?;
        if changed.is_ok_and(|c| colour_differs(&reference, &c)) {
            found.push(f.name);
        }
    }
    Ok(found)
}

/// The [`DetailLerp`](primase_core::detail::DetailLerp) `probe`'s program computes: its textures' parts read from what
/// moves which output ([`identify`]), its constants solved, proven and compared with the words they read; an error
/// naming what differs from the equation.
pub fn derive(probe: &mut ProgramProbe, seed: u32) -> Result<Derived, String> {
    let outputs = outputs(probe)?;
    let roles = match identify(probe, outputs) {
        Ok(roles) => roles,
        Err(Unidentified::ColourUnmoved(e)) => {
            // name what multiplies the colour by zero: an engine constant the bake freezes
            let groups = sampler_groups(probe)?;
            let answers: BTreeMap<u32, [f32; 4]> = groups
                .values()
                .flatten()
                .map(|&t| (t, [0.05, 0.05, 0.05, 0.5]))
                .collect();
            let factors = frozen_factors(probe, outputs, &answers)?;
            return Err(format!(
                "{e}: the colour is multiplied by engine constants the bake freezes ({}), set per draw",
                factors.join(", ")
            ));
        }
        Err(Unidentified::Differs(e)) => return Err(e),
    };
    derive_with(probe, outputs, roles, seed)
}
