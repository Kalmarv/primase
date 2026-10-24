//! A detail-blend program's equation read from runs of it ([`ProgramProbe`]) with designed texture samples, and proven on
//! random ones: the [`DetailLerp`](primase_core::detail::DetailLerp) a program computes, or what differs.
//!
//! The textures are told apart by the samplers the program reads them through (its colour textures by the diffuse
//! sampler, its normals by the bump sampler, its mask alone), then by what moves which output ([`identify`]): the base
//! normal is the one normal texture that alone tips the normal over flat (only the base's `xy` gives the normal's `z`),
//! the base colour the one colour moving the colour at both ends of the weights, each detail the colour and normal
//! moving the outputs where the weights take it alone. The weights are the family's forms measured so far
//! ([`WEIGHT_SETS`]): a program's are the set whose lane of the base normal moves its outputs. Each constant of the
//! equation is read from a run whose samples isolate it (a detail taken alone at an end of the weights, or weighed most
//! where none is, the chain then inverted; the reflectance mix at 0 or 1 by the base mask; a lane set so the term in
//! question is unsaturated, the mix read on the colour lane where it is read most closely, [`widest`]); each detail's
//! gloss scale is the base normal's z where its gloss moves in proportion to it, else the constant read; the alpha is
//! the one detail's whose colour alpha moves the program's alpha, where the binding gives that lane a role (`Coverage`
//! or `Opacity`), and none where it gives none; the program's tangent frame is read from the base normal alone. The equation is then proven against the program on random samples ([`PROOF_SETS`]) and
//! on samples where the weights mix ([`TRANSITION_SETS`]), every decoded role compared within [`BOUND_UNITS`], and each
//! constant is compared with the material word it reads ([`FieldComparison`]).
use std::collections::BTreeMap;

use primase_core::detail::{
    DETAIL_LIMIT, Detail, DetailLerp, DetailSample, DetailSamples, DetailSurface, GlossScale,
    ReflectanceTerm, Weight, WeightStep, chain, weight_steps,
};
use primase_core::layered::{Source, mask_value};
use primase_core::pbr::TextureRef;
use primase_core::shader::{NormalEncoding, OutputBinding, OutputRole, ScalarEncoding};
use primase_shader::ResourceKind;
use primase_shader_exec::ImageSample;
use primase_shader_exec::probe::{AnsweredReads, affine_uv};

use crate::detail_probe::ProgramProbe;
pub use crate::probe_accept::BOUND_UNITS;
use crate::probe_accept::{NonFinite, Seeded, moved, units, word_sources};

/// How many random sample sets a proof runs.
pub const PROOF_SETS: usize = 256;

/// The texture slots of a detail program, by their part in the equation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRoles {
    /// The program's weights: the index of its set in [`WEIGHT_SETS`].
    pub weight_set: usize,
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
    /// What its gloss term is scaled by, the form as measured: the base normal's z where the gloss moves in proportion
    /// to it, else the constant read.
    pub gloss_scale: GlossScale,
    /// Its term of the reflectance mix, the form as measured: the one detail whose term moves with the base mask reads
    /// it.
    pub reflectance: ReflectanceTerm,
}

/// The constants of a detail program's equation, as read from it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailConstants {
    /// The weights the details are chained by: the index of the program's set in [`WEIGHT_SETS`].
    pub weight_set: usize,
    /// In the chain's order.
    pub details: Vec<DetailTerms>,
    pub normal_strength: f32,
    pub dielectric_f0: f32,
    pub colour: [f32; 3],
    /// The binding's gloss encoding: the gloss is solved as stored and decoded by it.
    pub gloss_encoding: ScalarEncoding,
    /// The detail whose colour's alpha the alpha is, where the program's alpha lane has a role; `None` where it has none.
    pub alpha: Option<usize>,
}

/// A proof's result: the largest difference seen per role.
#[derive(Clone, Debug, PartialEq)]
pub struct Proof {
    pub sets: usize,
    /// The largest difference per role in units of `f32::EPSILON` of the compared magnitude.
    pub largest: BTreeMap<&'static str, f32>,
    /// The sets drawn where the weights mix (the weights' lane between where a weight is 1/256 and 255/256, each weight
    /// in turn) and the largest difference per role on them: the main sets draw the lane uniformly, and a weight steep
    /// in it (switch 1's) mixes on few of them. `None` for one detail.
    pub transition: Option<(usize, BTreeMap<&'static str, f32>)>,
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
    /// Where its lane has a role.
    alpha: Option<f32>,
}

/// The output registers and encodings of the roles a detail-blend program writes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Outputs {
    normal: (usize, f32),
    gloss: (usize, ScalarEncoding),
    albedo: usize,
    reflectance: (usize, ScalarEncoding),
    emission: Option<(usize, ScalarEncoding)>,
    /// The alpha's register and lane, where its lane has a role (`Coverage` or `Opacity`).
    alpha: Option<(usize, usize)>,
}

fn outputs(probe: &ProgramProbe) -> Result<Outputs, String> {
    outputs_of(&probe.interface().outputs, &probe.outputs)
}

/// [`outputs`] from the program's output `varyings` and the binding's `bindings` of them: each role the equation reads
/// on the lanes it reads them, the alpha's lane where it has a role (`Coverage` or `Opacity`, one lane); a lane with no
/// role (`Unknown`) is left out, any other role refused.
fn outputs_of(varyings: &[primase_shader::Varying], bindings: &[OutputBinding]) -> Result<Outputs, String> {
    let register = |label: &primase_shader::Label| -> Result<usize, String> {
        varyings
            .iter()
            .find(|v| v.label == *label)
            .and_then(|v| v.provenance.d3d)
            .map(|d| d.register as usize)
            .ok_or_else(|| format!("no output register for {}{}", label.name, label.index))
    };
    let (mut normal, mut gloss, mut albedo, mut reflectance, mut emission) = (None, None, None, None, None);
    let mut alpha = None;
    for o in bindings {
        let r = register(&o.label)?;
        match o.role {
            OutputRole::Unknown { .. } => {}
            OutputRole::Coverage | OutputRole::Opacity if o.lanes.count_ones() == 1 => {
                alpha = Some((r, o.lanes.trailing_zeros() as usize));
            }
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
        alpha,
    })
}

/// A weight of the family, of the base normal's sample: `steps` of `dot(N, channels) + bias`, on one lane of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeightForm {
    pub channels: [f32; 4],
    pub bias: f32,
    pub steps: &'static [WeightStep],
}

/// `(1 - N.a)^8`: the second detail's weight of switches 13 and 6.
const ONE_MINUS_ALPHA_8: WeightForm = WeightForm {
    channels: [0., 0., 0., -1.],
    bias: 1.,
    steps: &[WeightStep::Power(8)],
};

/// `N.a^8`: the third detail's weight of switch 6.
const ALPHA_8: WeightForm = WeightForm {
    channels: [0., 0., 0., 1.],
    bias: 0.,
    steps: &[WeightStep::Power(8)],
};

/// `min((22 N.z)^260, 1)`: the second detail's weight of switch 1.
const Z_22_260: WeightForm = WeightForm {
    channels: [0., 0., 22., 0.],
    bias: 0.,
    steps: &[WeightStep::Power(260), WeightStep::Min(1.)],
};

/// The weights of the details after the first: the family's forms measured so far, a stated list (switch 13's, switch
/// 1's, switch 6's), not read from a program. A program's set is the one of its detail count whose lane moves its outputs
/// ([`identify`]), and the proof holds the program to it, on random sets and on sets where its weights mix. Each set's
/// weights read one lane of the base normal; a form a program computes outside the list is refused, not fitted.
pub const WEIGHT_SETS: [&[WeightForm]; 4] = [
    &[],
    &[ONE_MINUS_ALPHA_8],
    &[Z_22_260],
    &[ONE_MINUS_ALPHA_8, ALPHA_8],
];

/// The base normal's lanes by name, as the refusals name them.
const LANE_NAMES: [&str; 4] = ["x", "y", "z", "alpha"];

/// The lane value at which the first detail weighs most where no end takes it alone: the middle, where the weights of
/// switch 6's both ends are smallest (`2^-8` each).
const MIDDLE: f32 = 0.5;

/// A program's weights: one of [`WEIGHT_SETS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights(pub &'static [WeightForm]);

impl Weights {
    /// The set at `index` of [`WEIGHT_SETS`].
    fn of(index: usize) -> Weights {
        Weights(WEIGHT_SETS[index])
    }

    /// The details it weighs: one more than its weights.
    fn details(self) -> usize {
        self.0.len() + 1
    }

    /// The base normal's lane its weights read; the alpha for a single detail's, which reads none.
    fn lane(self) -> usize {
        self.0
            .first()
            .and_then(|f| f.channels.iter().position(|&c| c != 0.))
            .unwrap_or(3)
    }

    /// The weights for the base normal sample `normal`, as the equation computes them.
    fn values(self, normal: [f32; 4]) -> Vec<f32> {
        self.0
            .iter()
            .map(|f| weight_steps(mask_value(f.channels, f.bias, normal), f.steps))
            .collect()
    }

    /// The weights where the lane is `x`, the others 0.
    fn at(self, x: f32) -> Vec<f32> {
        let mut normal = [0.; 4];
        normal[self.lane()] = x;
        self.values(normal)
    }

    /// The lane value at which `form` gives `w`, from its steps undone; exactly `w` where `exact` (stepped along the lane
    /// a value at a time until it is).
    fn reaching(self, form: &WeightForm, w: f32, exact: bool) -> f32 {
        let lane = self.lane();
        let v = form.steps.iter().rev().fold(w, |x, step| match *step {
            WeightStep::Power(p) => x.powf(1. / f32::from(p)),
            WeightStep::Min(_) => x,
        });
        // + 0 makes a zero positive
        let mut x = (v - form.bias) / form.channels[lane] + 0.;
        let value = |x: f32| {
            let mut normal = [0.; 4];
            normal[lane] = x;
            weight_steps(mask_value(form.channels, form.bias, normal), form.steps)
        };
        if exact {
            let rising = form.channels[lane] > 0.;
            for _ in 0..64 {
                let at = value(x);
                if at == w {
                    break;
                }
                x = if (at < w) == rising {
                    x.next_up()
                } else {
                    x.next_down()
                };
            }
        }
        x
    }

    /// The lane values at which every weight is 0 or 1, largest first: where each weight is 0 and 1 (a single detail's,
    /// read nowhere, the alpha's 1 and 0).
    fn ends(self) -> Vec<f32> {
        if self.0.is_empty() {
            return vec![1., 0.];
        }
        let mut out: Vec<f32> = Vec::new();
        for f in self.0 {
            for w in [0., 1.] {
                let x = self.reaching(f, w, true);
                if !out.contains(&x) {
                    out.push(x);
                }
            }
        }
        out.sort_by(|a, b| b.total_cmp(a));
        out
    }

    /// The lane value at which the weights take detail `j` alone (each weight 0 or 1, the last 1 its own, or none 1 for
    /// the first): one end of the weights, or `None` where no end takes it alone.
    fn isolating(self, j: usize) -> Option<f32> {
        self.ends().into_iter().find(|&x| {
            let w = self.at(x);
            w.iter().all(|&x| x == 0. || x == 1.)
                && w.iter().rposition(|&x| x == 1.).map_or(0, |i| i + 1) == j
        })
    }

    /// The lane value detail `j` is read at: where it is taken alone, else the [`MIDDLE`].
    fn observing(self, j: usize) -> f32 {
        self.isolating(j).unwrap_or(MIDDLE)
    }

    /// The lane value at which detail `j`'s weight is one half (the second detail's for the first, which has none): the
    /// chain mixed half and half there.
    fn half_way(self, j: usize) -> f32 {
        self.reaching(&self.0[j.max(1) - 1], 0.5, false)
    }

    /// The weights as the equation states them, of the base normal `base_normal`.
    fn equation(self, base_normal: TextureRef) -> Vec<Weight> {
        self.0
            .iter()
            .map(|f| Weight {
                source: Source::Mask {
                    texture: base_normal,
                    channels: f.channels,
                    bias: f.bias,
                },
                steps: f.steps.to_vec(),
            })
            .collect()
    }

    /// The lane's name, as the refusals name it.
    fn lane_name(self) -> &'static str {
        LANE_NAMES[self.lane()]
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
        alpha: o.alpha.map(|(r, lane)| out[r][lane]),
    })
}

impl Program<'_> {
    fn details(&self) -> usize {
        self.roles.details.len()
    }

    fn weights(&self) -> Weights {
        Weights::of(self.roles.weight_set)
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

/// A designed run's samples for the details of `weights`: a mid base colour times `scale`, the base normal flat with the
/// weights set by its lane at `x`, the details mid.
fn design(weights: Weights, scale: f32, x: f32, base_mask: f32) -> Samples {
    let n = weights.details();
    let mut normal = [0.5; 4];
    normal[weights.lane()] = x;
    Samples {
        a: [
            BASE_COLOUR[0] * scale,
            BASE_COLOUR[1] * scale,
            BASE_COLOUR[2] * scale,
            1.,
        ],
        n: normal,
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
    let weights = program.weights();
    let last_end = *weights.ends().last().unwrap_or(&0.);
    for halvings in 0..=HALVINGS {
        let scale = 0.5f32.powi(halvings);
        if let Some((constants, frame)) = solve_at(program, scale)? {
            // each detail whose colour is read from two base colours ([`solve_at`]) unsaturated at the second, twice
            let s = design(weights, 2. * scale, last_end, 0.);
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
    let weights = program.weights();
    let moves = |j: usize| -> Result<bool, String> {
        let x = weights.observing(j);
        let (high, low) = (
            program.observe(&design(weights, scale, x, 100.), UV)?,
            program.observe(&design(weights, scale, x, -100.), UV)?,
        );
        Ok(colour_differs(&high, &low)?)
    };
    let mut reading = Vec::new();
    for j in (0..n).filter(|&j| weights.isolating(j).is_some()) {
        if moves(j)? {
            reading.push(j);
        }
    }
    let unisolated: Vec<usize> = (0..n).filter(|&j| weights.isolating(j).is_none()).collect();
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
    let weights = program.weights();
    let run = |s: &Samples| program.observe(s, UV);
    let design = |x: f32, base_mask: f32| design(weights, scale, x, base_mask);
    let p = masked_detail(program, scale)?;
    let at_p = weights.observing(p);
    let p_alone = weights.isolating(p).is_some();
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
        let x = weights.isolating(j).ok_or_else(|| {
            format!(
                "detail {j}'s reflectance term is its bias alone and no end of the weights takes it alone"
            )
        })?;
        let on = |factor: f32| {
            let mut s = design(x, 0.);
            for v in &mut s.a[..3] {
                *v *= factor;
            }
            run(&s)
        };
        let (one, two) = (on(1.)?, on(2.)?);
        let s = design(x, 0.);
        // on the lane where the colour changes most between the two base colours ([`widest`])
        let change: [f32; 3] = std::array::from_fn(|i| {
            (two.albedo[i] - one.albedo[i]) / colour[i] + (two.reflectance[i] - one.reflectance[i])
        });
        let lane = widest(change);
        let d_albedo = two.albedo[lane] - one.albedo[lane];
        let d_reflectance = two.reflectance[lane] - one.reflectance[lane];
        let mix = if one.albedo.iter().all(|&a| a == 0.) {
            1.
        } else {
            let ratio = d_reflectance / (d_albedo / colour[lane]);
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
        let w = weights.at(at_p);
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
            let w = weights.at(at_p);
            let lane = widest(c_p.map(|c| c - f0));
            for m in base_masks() {
                let o = run(&design(at_p, m))?;
                let f = (o.reflectance[lane] - f0) / (c_p[lane] - f0);
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
            let x = weights.half_way(j);
            let w = weights.at(x);
            bias[j] = (|| {
                for m in base_masks() {
                    let o = run(&design(x, m))?;
                    let s = design(x, m);
                    let colour_at: [f32; 3] = std::array::from_fn(|i| {
                        let terms = (0..n).map(|k| tints[k][i] * s.d[k][i]);
                        (s.a[i] * chain(terms, &w)).clamp(0., 1.)
                    });
                    let lane = widest(colour_at.map(|c| c - f0));
                    let f = (o.reflectance[lane] - f0) / (colour_at[lane] - f0);
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
    let first_end = weights.ends()[0];
    let flat = |nx: f32, ny: f32| {
        let mut s = design(first_end, 0.);
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
        .find(|&j| weights.isolating(j) == Some(first_end))
        .ok_or_else(|| {
            format!(
                "no detail is taken alone where the base normal's {} is {first_end}",
                weights.lane_name()
            )
        })?;
    let mut s = design(first_end, 0.);
    s.k[tilted][0] = 1.;
    let t = to_tangent(run(&s)?.normal);
    let normal_strength = t[0] / t[2];
    // the gloss: each detail's scale and bias where it is read, those no end takes alone after the others, from a base
    // normal z and a detail z that leave the gloss unsaturated; the base normal z only at values that leave the
    // weights as they are there (any, where the weights read another lane)
    let mut gloss_bias = vec![0f32; n];
    let mut gloss_scale = vec![GlossScale::BaseNormalZ; n];
    let mut order: Vec<usize> = (0..n).filter(|&j| weights.isolating(j).is_some()).collect();
    order.extend((0..n).filter(|&j| weights.isolating(j).is_none()));
    let unsaturated = |g: f32| g > 0.01 && g < 0.99;
    for j in order {
        let x = weights.observing(j);
        let alone = weights.isolating(j).is_some();
        let w = weights.at(x);
        let base = design(x, 0.);
        let kept = weights.values(base.n);
        let zs: Vec<f32> = (1..=8)
            .map(|i| 0.5f32.powi(i))
            .chain((weights.lane() == 2).then_some(x))
            .filter(|&nz| {
                let mut normal = base.n;
                normal[2] = nz;
                weights.values(normal) == kept
            })
            .collect();
        let gloss_at = |kz: f32, nz: f32| -> Result<f32, String> {
            let mut s = design(x, 0.);
            s.n[2] = nz;
            s.k[j][2] = kz;
            Ok(run(&s)?.gloss_stored)
        };
        let mut first = None;
        'reading: for kz in [0.5, 1., 0.] {
            for &nz in &zs {
                let g = gloss_at(kz, nz)?;
                if unsaturated(g) {
                    first = Some((kz, nz, g));
                    break 'reading;
                }
            }
        }
        let (kz, nz, g) = first.ok_or_else(|| {
            format!("no base normal z leaves the gloss unsaturated where detail {j} is read")
        })?;
        // its scale: the base normal z where the gloss moves in proportion to it, a constant where it does not move
        let mut second = None;
        for &z2 in zs.iter().filter(|&&z2| z2 != nz) {
            let g2 = gloss_at(kz, z2)?;
            if unsaturated(g2) {
                second = Some((z2, g2));
                break;
            }
        }
        if let Some((_, g2)) = second
            && !moved(g, g2)?
        {
            // the gloss `2 c (kz + b)` at a second detail z gives c, then b
            if !alone {
                return Err(format!(
                    "detail {j}'s gloss scale is a constant and no end of the weights takes it alone"
                ));
            }
            let (c, b) = constant_gloss((kz, g), |k| gloss_at(k, nz))
                .map_err(|e| format!("detail {j}'s gloss: {e}"))?;
            gloss_scale[j] = GlossScale::Constant(c);
            gloss_bias[j] = b;
            continue;
        } else if let Some((z2, g2)) = second
            && units(g / nz, g2 / z2)? > BOUND_UNITS
        {
            return Err(format!(
                "detail {j}'s gloss moves with the base normal's z, not in proportion ({g} at {nz}, {g2} at {z2})"
            ));
        }
        let mut sample = base.n;
        sample[2] = nz;
        let scale = chain(gloss_scale.iter().map(|s| s.value(sample)), &w);
        gloss_bias[j] = if alone {
            g / (2. * scale) - kz
        } else {
            let mut s = design(x, 0.);
            s.k[j][2] = kz;
            let terms: Vec<f32> = (0..n).map(|k| s.k[k][2] + gloss_bias[k]).collect();
            unlerp(g / (2. * scale), &terms, &w, j) - kz
        };
    }
    // the alpha, where its lane has a role: the one detail whose colour's alpha moves it, the base colour's moving it too
    let alpha = match program.outputs.alpha {
        None => None,
        Some(_) => {
            let s = design(first_end, 0.);
            let reference = run(&s)?.alpha;
            let moves = |f: &dyn Fn(&mut Samples)| -> Result<bool, String> {
                let mut t = s.clone();
                f(&mut t);
                match (reference, run(&t)?.alpha) {
                    (Some(a), Some(b)) => Ok(moved(a, b)?),
                    _ => Ok(false),
                }
            };
            let mut moving = Vec::new();
            for j in 0..n {
                if moves(&|t| t.d[j][3] = 0.5)? {
                    moving.push(j);
                }
            }
            let [j] = moving.as_slice() else {
                return Err(format!(
                    "the alpha moves with the colour alpha of details {moving:?}, not one detail's"
                ));
            };
            if !moves(&|t| t.a[3] = 0.5)? {
                return Err("the alpha does not move with the base colour's alpha".into());
            }
            Some(*j)
        }
    };
    let details = (0..n)
        .map(|j| DetailTerms {
            tint: tints[j],
            gloss_bias: gloss_bias[j],
            gloss_scale: gloss_scale[j],
            reflectance: if j == p {
                ReflectanceTerm::MaskPlusBias(bias[j])
            } else {
                ReflectanceTerm::Bias(bias[j])
            },
        })
        .collect();
    Ok(Some((
        DetailConstants {
            weight_set: program.roles.weight_set,
            details,
            normal_strength,
            dielectric_f0: f0,
            colour,
            gloss_encoding: program.outputs.gloss.1,
            alpha,
        },
        frame,
    )))
}

/// The lane of `values` that is largest, the lowest on a tie: each reading of the reflectance mix is taken on the colour
/// lane where what it divides by is largest (`C - f0` for the mix, the change of `C` for a mix read from two base
/// colours), as the stored reflectance's rounding is divided by it.
fn widest(values: [f32; 3]) -> usize {
    (1..3).fold(0, |best, k| if values[k] > values[best] { k } else { best })
}

/// The scale `c` and bias `b` of a detail's gloss `2 c (k + b)` scaled by a constant, from its gloss `g` at the detail z
/// `kz` (unsaturated) and `gloss_at` a detail z: a second reading the widest step from `kz` (1 down to 1/16, either way)
/// that leaves the gloss unsaturated, for the most digits of `c`.
fn constant_gloss(
    (kz, g): (f32, f32),
    gloss_at: impl Fn(f32) -> Result<f32, String>,
) -> Result<(f32, f32), String> {
    for step in [1f32, 0.5, 0.25, 0.125, 0.0625] {
        for k2 in [kz + step, kz - step] {
            let g2 = gloss_at(k2)?;
            if g2 > 0.01 && g2 < 0.99 {
                let c = (g2 - g) / (2. * (k2 - kz));
                return Ok((c, g / (2. * c) - kz));
            }
        }
    }
    Err("no second detail z leaves it unsaturated".into())
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
                gloss_scale: d.gloss_scale,
                reflectance: d.reflectance,
            })
            .collect(),
        weights: Weights::of(c.weight_set).equation(texture),
        gain,
        normal_strength: c.normal_strength,
        dielectric_f0: c.dielectric_f0,
        colour: c.colour,
        gloss_encoding: c.gloss_encoding,
        alpha: c.alpha,
    }
}

/// The equation's surface for `s` with `c` and `gain` ([`detail_lerp`]).
fn equation(c: &DetailConstants, gain: f32, s: &Samples) -> DetailSurface {
    let texture = TextureRef::new(primase_core::World::default().spawn());
    detail_lerp(c, gain, texture).evaluate(&s.as_core(), &Weights::of(c.weight_set).values(s.n))
}

/// How many proof sets are drawn where the weights mix ([`prove`]).
pub const TRANSITION_SETS: usize = 64;

/// One proof set: `program` against the equation of `c` on samples `s` at `uv`, each role's difference kept in
/// `largest` where it is the largest; an error past [`BOUND_UNITS`] (or with an emission), naming the set `set`.
fn compare_set(
    program: &Program<'_>,
    c: &DetailConstants,
    frame: &Frame,
    (s, uv): (&Samples, [f32; 2]),
    set: &str,
    largest: &mut BTreeMap<&'static str, f32>,
) -> Result<(), String> {
    let limit = BOUND_UNITS;
    let seen = program.observe(s, uv)?;
    let want = equation(c, 1., s);
    // the equation's tangent normal through the program's frame, unit length
    let n = want.normal;
    let world: [f32; 3] = std::array::from_fn(|i| (0..3).map(|axis| frame[axis][i] * n[axis]).sum());
    let length = (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
    let mut check = |role: &'static str, got: f32, wanted: f32| -> Result<(), String> {
        let u = units(got, wanted).map_err(|e| format!("set {set}: {role}: {e}"))?;
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
    match (c.alpha, seen.alpha) {
        (Some(_), Some(a)) => check("alpha", a, want.alpha),
        _ => Ok(()),
    }
}

/// `program` against the equation of `c` on [`PROOF_SETS`] random sample sets at random coordinates, then on
/// [`TRANSITION_SETS`] where the weights mix: the largest difference per role of each, or the first set past
/// [`BOUND_UNITS`] (or with an emission).
fn prove(program: &Program<'_>, c: &DetailConstants, frame: &Frame, seed: u32) -> Result<Proof, String> {
    let mut random = Seeded::new(seed);
    let mut largest: BTreeMap<&'static str, f32> = BTreeMap::new();
    for set in 0..PROOF_SETS {
        let s = random_samples(&mut random, program.details());
        let uv = [random.unit(), random.unit()];
        compare_set(program, c, frame, (&s, uv), &set.to_string(), &mut largest)?;
    }
    // the weights' lane between where a weight is 1/256 and 255/256, each weight in turn, from a sequence of its own
    let weights = program.weights();
    let transition = if weights.0.is_empty() {
        None
    } else {
        let mut random = Seeded::new(seed.wrapping_add(1));
        let mut largest: BTreeMap<&'static str, f32> = BTreeMap::new();
        for set in 0..TRANSITION_SETS {
            let form = &weights.0[set % weights.0.len()];
            let (a, b) = (
                weights.reaching(form, 1. / 256., false),
                weights.reaching(form, 255. / 256., false),
            );
            let mut s = random_samples(&mut random, program.details());
            s.n[weights.lane()] = a + (b - a) * random.unit();
            let uv = [random.unit(), random.unit()];
            compare_set(
                program,
                c,
                frame,
                (&s, uv),
                &format!("transition {set}"),
                &mut largest,
            )?;
        }
        Some((TRANSITION_SETS, largest))
    };
    Ok(Proof {
        sets: PROOF_SETS,
        largest,
        transition,
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
        self.expected
            .map(|e| units(self.solved, e).is_ok_and(|u| u <= BOUND_UNITS))
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
                    gloss_scale: d.gloss_scale,
                    reflectance: d.reflectance,
                })
                .collect(),
            weights: Weights::of(c.weight_set).equation(base_normal),
            gain: 1.,
            normal_strength: c.normal_strength,
            dielectric_f0: c.dielectric_f0,
            colour: c.colour,
            gloss_encoding: c.gloss_encoding,
            alpha: c.alpha,
        }
    }
}

/// The constants' lanes by name, in a fixed order: each detail's tint lanes, the normal strength, each gloss bias, each
/// reflectance bias, the dielectric reflectance, the colour's lanes, each constant gloss scale.
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
    out.extend(
        c.details
            .iter()
            .enumerate()
            .filter_map(|(j, d)| match d.gloss_scale {
                GlossScale::Constant(v) => Some((format!("gloss_scale[{j}]"), v)),
                GlossScale::BaseNormalZ => None,
            }),
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
                .max(units(folded.albedo[i], unfolded.albedo[i])?)
                .max(units(folded.reflectance[i], unfolded.reflectance[i])?);
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

/// Whether any pair of lanes differs past [`BOUND_UNITS`]; refused at the first lane that is not finite.
fn any_moved(pairs: impl IntoIterator<Item = (f32, f32)>) -> Result<bool, NonFinite> {
    for (x, y) in pairs {
        if moved(x, y)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether any decoded role differs between two runs past [`BOUND_UNITS`]; refused where a lane is not finite.
fn differ(a: &Observed, b: &Observed) -> Result<bool, NonFinite> {
    let lanes = |o: &Observed| {
        let mut v = Vec::with_capacity(13);
        v.extend(o.albedo);
        v.extend(o.reflectance);
        v.extend(o.normal);
        v.extend(o.emission);
        v.push(o.gloss_stored);
        v
    };
    any_moved(lanes(a).into_iter().zip(lanes(b)))
}

/// Whether the colour (albedo or reflectance) differs between two runs past [`BOUND_UNITS`]; refused where a lane is
/// not finite.
fn colour_differs(a: &Observed, b: &Observed) -> Result<bool, NonFinite> {
    any_moved(
        a.albedo
            .into_iter()
            .chain(a.reflectance)
            .zip(b.albedo.into_iter().chain(b.reflectance)),
    )
}

/// Whether the normal differs between two runs past [`BOUND_UNITS`]; refused where a lane is not finite.
fn normal_differs(a: &Observed, b: &Observed) -> Result<bool, NonFinite> {
    any_moved(a.normal.into_iter().zip(b.normal))
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

impl From<NonFinite> for Unidentified {
    fn from(e: NonFinite) -> Self {
        Unidentified::Differs(e.to_string())
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
        if !moved(cosine, 0.)? {
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
    // the weights: the family's set for `n` details whose lane, from one end of its weights to the other, moves the
    // outputs; a single detail's none, its base normal's alpha moving nothing
    let lane_moves = |weights: Weights| -> Result<bool, Unidentified> {
        let (lane, ends) = (weights.lane(), weights.ends());
        Ok(differ(
            &run(&with(&mid, *base_normal, &|v| v[lane] = ends[0]))?,
            &run(&with(&mid, *base_normal, &|v| v[lane] = ends[1]))?,
        )?)
    };
    let mut weight_set = None;
    for (i, set) in WEIGHT_SETS
        .iter()
        .enumerate()
        .filter(|(_, set)| set.len() + 1 == n)
    {
        if n == 1 || lane_moves(Weights(set))? {
            weight_set = Some(i);
            break;
        }
    }
    let Some(weight_set) = weight_set else {
        return Err("no normal texture's alpha moves the outputs: the mask is not the base normal's (1 - N.a)^8, nor \
                    any other weight of the family"
            .into());
    };
    let weights = Weights::of(weight_set);
    if n == 1 && lane_moves(weights)? {
        return Err("the base normal's alpha moves the outputs of a program with one detail".into());
    }
    // what moves the colour among `colours` with `answers`
    let moving_colour = |answers: &BTreeMap<u32, [f32; 4]>| -> Result<Vec<u32>, String> {
        let reference = run(answers)?;
        let mut moving = Vec::new();
        for &c in &colours {
            if colour_differs(&reference, &run(&with(answers, c, &|v| v[..3].fill(0.025)))?)? {
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
            weight_set,
            base_colour: base,
            base_normal: *base_normal,
            base_mask: *mask,
            details: vec![DetailSlots {
                colour: detail,
                normal: detail_normal,
            }],
        });
    }
    // what moves the colour and the normal with the base normal's lane at `x`
    let lane = weights.lane();
    let at = |x: f32| -> Result<(Vec<u32>, Vec<u32>), String> {
        let end = with(&mid, *base_normal, &|v| v[lane] = x);
        let reference = run(&end)?;
        let moving_colour = moving_colour(&end)?;
        let mut moving_normal = Vec::new();
        for &t in normals.iter().filter(|&&t| t != *base_normal) {
            if normal_differs(&reference, &run(&with(&end, t, &|v| v[0] = 0.75))?)? {
                moving_normal.push(t);
            }
        }
        Ok((moving_colour, moving_normal))
    };
    // each end of the weights takes one detail alone
    let ends: Vec<(usize, f32)> = (0..n)
        .filter_map(|j| weights.isolating(j).map(|x| (j, x)))
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
            .map(|(_, x, (c, _))| {
                format!(
                    "{} where the base normal's {} is {x}",
                    slots(c),
                    weights.lane_name()
                )
            })
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
    for (j, x, (c, k)) in &moving {
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
                    "where the base normal's {} is {x}: colours {} and normals {} move the outputs, not one \
                     detail's each",
                    weights.lane_name(),
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
        weight_set,
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
        if let Ok(c) = changed
            && colour_differs(&reference, &c)?
        {
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

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::shader::{D3dSignature, ScalarType, SystemValue, Value, Varying, VaryingProvenance};
    use primase_shader::Label;

    fn target(n: u32) -> Label {
        Label {
            name: "SV_Target".into(),
            index: n,
        }
    }

    /// Sekiro's five G-buffer targets as a lifted program declares them, target `n` in register `n`.
    fn varyings() -> Vec<Varying> {
        (0..5)
            .map(|n| Varying {
                label: target(n),
                system: Some(SystemValue::Target(n)),
                value: Value {
                    scalar: ScalarType::Float,
                    width: 4,
                },
                interp: None,
                used: true,
                provenance: VaryingProvenance {
                    d3d: Some(D3dSignature {
                        register: n,
                        mask: 0b1111,
                        read_write_mask: 0,
                    }),
                },
            })
            .collect()
    }

    /// The binding of Sekiro's G-buffer layout, every lane of the five targets (`Unknown` where it has no role).
    fn bindings() -> Vec<OutputBinding> {
        crate::Game::Sekiro
            .gbuffer()
            .into_iter()
            .map(|(n, lanes, role)| OutputBinding {
                label: target(n),
                lanes,
                role,
            })
            .collect()
    }

    /// The widest lane is the largest, the lowest of equal ones.
    #[test]
    fn the_widest_lane_is_the_largest_and_the_lowest_on_a_tie() {
        assert_eq!(widest([0.1, 0.5, 0.2]), 1);
        assert_eq!(widest([0.1, 0.2, 0.5]), 2);
        assert_eq!(widest([0.5, 0.5, 0.5]), 0);
        assert_eq!(widest([0.2, 0.5, 0.5]), 1);
    }

    /// A constant gloss scale is read from a second detail z wherever the gloss is unsaturated: a designed gloss
    /// `saturate(2 * 0.5 * (k + 0.6))` is unsaturated at a detail z of 0 alone of 1, 0 and 0.5, and the second reading
    /// steps from it until it is (to -0.5) and gives the scale and the bias back.
    #[test]
    fn a_constant_gloss_scale_is_read_where_one_detail_z_alone_of_the_first_three_leaves_it_unsaturated() {
        let (c, b) = (0.5f32, 0.6f32);
        let gloss = |k: f32| -> Result<f32, String> { Ok((2. * c * (k + b)).clamp(0., 1.)) };
        let unsaturated: Vec<f32> = [1f32, 0., 0.5]
            .into_iter()
            .filter(|&k| gloss(k).is_ok_and(|g| g > 0.01 && g < 0.99))
            .collect();
        assert_eq!(unsaturated, [0.]);
        let (got_c, got_b) = constant_gloss((0., gloss(0.).unwrap()), gloss).unwrap();
        assert!(units(got_c, c).unwrap() <= BOUND_UNITS, "{got_c}");
        assert!(units(got_b, b).unwrap() <= BOUND_UNITS, "{got_b}");
        // a gloss saturated at every step is refused
        let flat = |_: f32| -> Result<f32, String> { Ok(1.) };
        assert!(constant_gloss((0., 0.5), flat).is_err());
    }

    /// The roles the equation reads are found beside a binding's lanes with no role, which are left out, and its alpha's
    /// lane where it has a role; a role the equation does not read is refused.
    #[test]
    fn the_equations_roles_are_read_beside_unroled_and_alpha_lanes() {
        let all = bindings();
        assert!(all.iter().any(|o| matches!(o.role, OutputRole::Unknown { .. })));
        let expected = Outputs {
            normal: (0, f32::from_bits(0x3f00_2008)),
            gloss: (1, ScalarEncoding::square_of(0.95)),
            albedo: 2,
            reflectance: (3, ScalarEncoding::square_root_of(1.0)),
            emission: Some((4, ScalarEncoding::LINEAR)),
            alpha: None,
        };
        assert_eq!(outputs_of(&varyings(), &all), Ok(expected));
        // the roles alone, as before the layout stated every lane
        let roled: Vec<OutputBinding> = all
            .iter()
            .filter(|o| !matches!(o.role, OutputRole::Unknown { .. }))
            .cloned()
            .collect();
        assert_eq!(outputs_of(&varyings(), &roled), Ok(expected));
        // target 2's w bound as the alpha a discard tests, or a blend weighs by
        let mut alpha = all.clone();
        let w = alpha
            .iter()
            .position(|o| o.label == target(2) && o.lanes == 0b1000)
            .unwrap();
        for role in [OutputRole::Coverage, OutputRole::Opacity] {
            alpha[w].role = role;
            let with_alpha = Outputs {
                alpha: Some((2, 3)),
                ..expected
            };
            assert_eq!(outputs_of(&varyings(), &alpha), Ok(with_alpha));
        }
        // a role the equation does not read
        let mut metallic = all;
        metallic[1].role = OutputRole::Metallic;
        let err = outputs_of(&varyings(), &metallic).unwrap_err();
        assert!(err.contains("is not a detail blend's"), "{err}");
    }
}
