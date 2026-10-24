//! A detail-blend program's equation read from runs of it ([`ProgramProbe`]) with designed texture samples, and proven on
//! random ones: the [`TwoDetailLerp`](primase_core::detail::TwoDetailLerp) a program computes, or what differs.
//!
//! The textures are told apart by the samplers the program reads them through (its colour textures by the diffuse
//! sampler, its normals by the bump sampler, its mask alone), then by what moves which output ([`identify`]): the base
//! normal is the one whose alpha moves the mask, the base colour the one colour moving the colour at both ends of the
//! mask, each detail the other at its end. Each constant of the equation is read from a run whose samples isolate it
//! (the mask at 0 or 1 by the base normal's alpha, the reflectance mix at 0 or 1 by the base mask, a lane set so the term
//! in question is unsaturated); the program's tangent frame is read from the base normal alone. The equation is then
//! proven against the program on random samples ([`PROOF_SETS`]), every decoded role compared within [`BOUND_UNITS`],
//! and each constant is compared with the material word it reads ([`FieldComparison`]).
use std::collections::BTreeMap;

use primase_core::detail::{DetailSurface, TwoDetailSamples};
use primase_core::layered::{Channel, Source, mask_value, power};
use primase_core::pbr::TextureRef;
use primase_core::shader::{NormalEncoding, OutputRole};
use primase_shader::ResourceKind;
use primase_shader_exec::ImageSample;
use primase_shader_exec::probe::AnsweredReads;

use crate::detail_probe::ProgramProbe;
pub use crate::probe_accept::BOUND_UNITS;
use crate::probe_accept::{Seeded, moved, units, word_sources};

/// How many random sample sets a proof runs.
pub const PROOF_SETS: usize = 256;

/// The texture slots of a two-detail program, by their part in the equation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotRoles {
    pub base_colour: u32,
    pub base_normal: u32,
    pub base_mask: u32,
    pub detail_colour: [u32; 2],
    pub detail_normal: [u32; 2],
}

/// The constants of a two-detail program's equation, as read from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoDetailConstants {
    /// The gain times each tint: the program computes `gain * A * lerp(t0 * D0, t1 * D1, w)` and the gain and a tint
    /// are read only as their product, so the equation takes the gain as 1 and the product as the tint. The two orders
    /// round differently; [`FoldCost`] measures it on every proof (at most 3 units in the last place of the colour on
    /// Sekiro's switch-13 programs).
    pub tint: [[f32; 3]; 2],
    pub normal_strength: f32,
    pub gloss_bias: [f32; 2],
    pub reflectance_bias: [f32; 2],
    pub dielectric_f0: f32,
    pub colour: [f32; 3],
    /// The binding's gloss encoding: the gloss is solved as stored and decoded by it.
    pub gloss_encoding: primase_core::shader::ScalarEncoding,
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
    gloss: (usize, primase_core::shader::ScalarEncoding),
    albedo: usize,
    reflectance: (usize, primase_core::shader::ScalarEncoding),
    emission: Option<(usize, primase_core::shader::ScalarEncoding)>,
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

/// A two-detail program ready to be run with samples by role, its engine textures read as the bake reads them
/// ([`ProgramProbe::engine`]).
struct Program<'p> {
    probe: &'p ProgramProbe,
    outputs: Outputs,
    roles: SlotRoles,
}

/// The samples of every role of one run.
#[derive(Clone, Copy, Debug)]
struct Samples {
    a: [f32; 4],
    n: [f32; 4],
    m: [f32; 4],
    d: [[f32; 4]; 2],
    k: [[f32; 4]; 2],
}

impl Samples {
    fn as_core(&self) -> TwoDetailSamples {
        TwoDetailSamples {
            base_colour: self.a,
            base_normal: self.n,
            base_mask: self.m,
            detail_colour: self.d,
            detail_normal: self.k,
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
    fn observe(&self, s: &Samples, uv: [f32; 2]) -> Result<Observed, String> {
        let r = self.roles;
        let by_slot = BTreeMap::from([
            (r.base_colour, s.a),
            (r.base_normal, s.n),
            (r.base_mask, s.m),
            (r.detail_colour[0], s.d[0]),
            (r.detail_colour[1], s.d[1]),
            (r.detail_normal[0], s.k[0]),
            (r.detail_normal[1], s.k[1]),
        ]);
        observe_slots(self.probe, self.outputs, &by_slot, uv)
    }
}

/// The mask of a two-detail program: `(1 - N.a)^8` of the base normal.
fn mask_channel(base_normal: TextureRef) -> Channel {
    Channel::Power {
        v: Source::Mask {
            texture: base_normal,
            channels: [0., 0., 0., -1.],
            bias: 1.,
        },
        exponent: 8,
    }
}

/// The mask's value for a base normal sample, as the equation computes it.
fn mask_of(n: [f32; 4]) -> f32 {
    power(mask_value([0., 0., 0., -1.], 1., n), 8)
}

/// The base normal's alpha that gives the mask 0, 1 or one half.
const MASK_OFF: f32 = 1.;
const MASK_ON: f32 = 0.;

/// The designed base colour at a scale of 1.
const BASE_COLOUR: [f32; 3] = [0.1, 0.15, 0.2];

/// A designed run's samples: a mid base colour times `scale`, the base normal flat with the mask set by its alpha, the
/// details mid.
fn design(scale: f32, mask_alpha: f32, base_mask: f32) -> Samples {
    Samples {
        a: [
            BASE_COLOUR[0] * scale,
            BASE_COLOUR[1] * scale,
            BASE_COLOUR[2] * scale,
            1.,
        ],
        n: [0.5, 0.5, 0.5, mask_alpha],
        m: [base_mask, 0., 0., 0.],
        d: [[0.5, 0.6, 0.7, 1.], [0.4, 0.5, 0.6, 1.]],
        k: [[0.5, 0.5, 0.5, 0.], [0.5, 0.5, 0.5, 0.]],
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

/// A program's tangent frame: x, y and z of tangent space in world space.
type Frame = [[f32; 3]; 3];

/// A solve's result: the constants, the program's tangent frame, and the scale of the designed base colour it read them
/// at.
type Solved = (TwoDetailConstants, Frame, f32);

/// The constants of the equation as `program` computes them, read run by run with designed samples. The material's
/// words are taken as they are (none clamped); where its tints and gain saturate the designed colour, the designed base
/// colour is halved until no designed colour saturates (at most [`HALVINGS`] times), the scale returned.
fn solve(program: &Program<'_>) -> Result<Solved, String> {
    for halvings in 0..=HALVINGS {
        let scale = 0.5f32.powi(halvings);
        if let Some((constants, frame)) = solve_at(program, scale)? {
            // the second detail's colour read at twice the scale ([`solve_at`]) unsaturated too
            let s = design(2. * scale, MASK_ON, 0.);
            if (0..3).all(|i| constants.tint[1][i] * s.a[i] * s.d[1][i] < 1.) {
                return Ok((constants, frame, scale));
            }
        }
    }
    Err(format!(
        "the designed colour saturates at every scale of the base colour down to 1/{}",
        1 << HALVINGS
    ))
}

/// [`solve`] with the designed base colour at `scale`: `None` where a designed colour saturates.
fn solve_at(program: &Program<'_>, scale: f32) -> Result<Option<(TwoDetailConstants, Frame)>, String> {
    let run = |s: &Samples| program.observe(s, UV);
    let design = |mask_alpha: f32, base_mask: f32| design(scale, mask_alpha, base_mask);
    // the mix f at 1 (the base mask far above any bias) and the mask off: the reflectance is C = gain t0 A D0
    let full = run(&design(MASK_OFF, 100.))?;
    let s = design(MASK_OFF, 100.);
    if full.albedo.iter().any(|&a| a != 0.) {
        return Err(format!(
            "the albedo is {:?} where the reflectance mix is 1",
            full.albedo
        ));
    }
    let c0 = full.reflectance;
    if c0.iter().any(|&c| c >= 1.) {
        return Ok(None);
    }
    if c0.iter().any(|&c| c <= 0.) {
        return Err(format!(
            "the first detail's colour {c0:?} is zero at the designed sample"
        ));
    }
    let tint0: [f32; 3] = std::array::from_fn(|i| c0[i] / (s.a[i] * s.d[0][i]));
    // the mix at 0: the reflectance is the dielectric's, the albedo C times the colour
    let none = run(&design(MASK_OFF, -100.))?;
    let f0 = none.reflectance[0];
    if none.reflectance.iter().any(|&r| r != f0) {
        return Err(format!(
            "the dielectric reflectance {:?} is not grey",
            none.reflectance
        ));
    }
    let colour: [f32; 3] = std::array::from_fn(|i| none.albedo[i] / c0[i]);
    // the mix's first bias: a base mask that leaves the mix unsaturated
    let bias0 = (|| {
        for m in base_masks() {
            let o = run(&design(MASK_OFF, m))?;
            let f = (o.reflectance[2] - f0) / (c0[2] - f0);
            if f > 0.01 && f < 0.99 {
                return Ok(f - m);
            }
        }
        Err("no base mask leaves the reflectance mix unsaturated".to_owned())
    })()?;
    // the mask on: the second detail. Two base colours, the second twice the first, give the mix from the slopes
    let on = |factor: f32| {
        let mut s = design(MASK_ON, 0.);
        for v in &mut s.a[..3] {
            *v *= factor;
        }
        run(&s)
    };
    let (one, two) = (on(1.)?, on(2.)?);
    let s = design(MASK_ON, 0.);
    let k = 2;
    let d_albedo = two.albedo[k] - one.albedo[k];
    let d_reflectance = two.reflectance[k] - one.reflectance[k];
    let mix_on = if one.albedo.iter().all(|&a| a == 0.) {
        1.
    } else {
        let ratio = d_reflectance / (d_albedo / colour[k]);
        ratio / (1. + ratio)
    };
    let c1: [f32; 3] = std::array::from_fn(|i| {
        if mix_on < 1. {
            one.albedo[i] / ((1. - mix_on) * colour[i])
        } else {
            one.reflectance[i]
        }
    });
    let tint1: [f32; 3] = std::array::from_fn(|i| c1[i] / (s.a[i] * s.d[1][i]));
    // the second bias: halfway through the mask, a base mask that leaves the mix unsaturated
    let half = 1. - 0.5f32.powf(1. / 8.);
    let w = mask_of([0., 0., 0., half]);
    let bias1 = (|| {
        for m in base_masks() {
            let o = run(&design(half, m))?;
            let s = design(half, m);
            let colour_at = {
                let a = tint0[2] * s.d[0][2];
                let b = tint1[2] * s.d[1][2];
                (s.a[2] * (a + (b - a) * w)).clamp(0., 1.)
            };
            let f = (o.reflectance[2] - f0) / (colour_at - f0);
            if f > 0.01 && f < 0.99 {
                return Ok((f - (m + bias0) * (1. - w)) / w);
            }
        }
        Err("no base mask leaves the reflectance mix unsaturated halfway".to_owned())
    })()?;
    // the tangent frame, from the base normal alone (the details flat, the mask off): x, y and z of tangent space
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
    // the normal strength: the first detail tilted fully along x (its term `2 * 1 - 1`), the base flat; the G-buffer
    // stores the normal biased, its precision absolute, so the largest tilt reads the strength most closely
    let mut s = design(MASK_OFF, 0.);
    s.k[0][0] = 1.;
    let t = to_tangent(run(&s)?.normal);
    let normal_strength = t[0] / t[2];
    // the gloss biases: a base normal z and a detail z that leave the gloss unsaturated
    let gloss_bias = |mask_alpha: f32, which: usize| -> Result<f32, String> {
        for kz in [0.5, 1., 0.] {
            for nz in (1..=8).map(|i| 0.5f32.powi(i)) {
                let mut s = design(mask_alpha, 0.);
                s.n[2] = nz;
                s.k[which][2] = kz;
                let g = run(&s)?.gloss_stored;
                if g > 0.01 && g < 0.99 {
                    return Ok(g / (2. * nz) - kz);
                }
            }
        }
        Err("no base normal z leaves the gloss unsaturated".into())
    };
    let gloss_bias = [gloss_bias(MASK_OFF, 0)?, gloss_bias(MASK_ON, 1)?];
    Ok(Some((
        TwoDetailConstants {
            tint: [tint0, tint1],
            normal_strength,
            gloss_bias,
            reflectance_bias: [bias0, bias1],
            dielectric_f0: f0,
            colour,
            gloss_encoding: program.outputs.gloss.1,
        },
        frame,
    )))
}

/// One random sample set.
fn random_samples(random: &mut Seeded) -> Samples {
    Samples {
        a: random.lanes(),
        n: random.lanes(),
        m: random.lanes(),
        d: [random.lanes(), random.lanes()],
        k: [random.lanes(), random.lanes()],
    }
}

/// The equation's surface for `s` with `c` and `gain` (1 where `c`'s tints fold it, [`TwoDetailConstants::tint`]).
fn equation(c: &TwoDetailConstants, gain: f32, s: &Samples) -> DetailSurface {
    let texture = TextureRef::new(primase_core::World::default().spawn());
    let mix = primase_core::detail::TwoDetailLerp {
        base_colour: texture,
        base_normal: texture,
        base_mask: texture,
        detail_colour: [texture; 2],
        detail_normal: [texture; 2],
        mask: mask_channel(texture),
        tint: c.tint,
        gain,
        normal_strength: c.normal_strength,
        gloss_bias: c.gloss_bias,
        reflectance_bias: c.reflectance_bias,
        dielectric_f0: c.dielectric_f0,
        colour: c.colour,
        gloss_encoding: c.gloss_encoding,
    };
    mix.evaluate(&s.as_core(), &[mask_value([0., 0., 0., -1.], 1., s.n)])
}

/// `program` against the equation of `c` on [`PROOF_SETS`] random sample sets at random coordinates: the largest
/// difference per role, or the first set past [`BOUND_UNITS`] (or with an emission).
fn prove(program: &Program<'_>, c: &TwoDetailConstants, frame: &Frame, seed: u32) -> Result<Proof, String> {
    let mut random = Seeded::new(seed);
    let mut largest: BTreeMap<&'static str, f32> = BTreeMap::new();
    let limit = BOUND_UNITS;
    for set in 0..PROOF_SETS {
        let s = random_samples(&mut random);
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

/// The material's texture slots the program reads, grouped by the sampler each is read through.
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
    /// times the word ([`TwoDetailConstants::tint`]).
    pub expected: Option<f32>,
}

impl FieldComparison {
    /// Whether the solved value is the expected one, within [`BOUND_UNITS`]; `None` where there is no one word to compare.
    pub fn agrees(&self) -> Option<bool> {
        self.expected.map(|e| units(self.solved, e) <= BOUND_UNITS)
    }
}

/// The cost of the tint fold ([`TwoDetailConstants::tint`]) on one program: the equation with the gain and the tints read from the material's words
/// beside the folded one, on the proof's random sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldCost {
    /// The gain: the first tint's lane over its word, the same for every tint lane (each compared in [`FieldComparison`]).
    pub gain: f32,
    /// The largest difference of the albedo and reflectance, folded against unfolded, in units of `f32::EPSILON` of
    /// the larger magnitude.
    pub largest: f32,
}

/// A two-detail program's equation: its roles, constants and proof.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    pub roles: SlotRoles,
    pub constants: TwoDetailConstants,
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
}

/// The constants' lanes by name, in a fixed order.
fn constant_lanes(c: &TwoDetailConstants) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for (i, tint) in c.tint.iter().enumerate() {
        out.extend(
            tint.iter()
                .enumerate()
                .map(|(j, &v)| (format!("tint[{i}][{j}]"), v)),
        );
    }
    out.push(("normal_strength".into(), c.normal_strength));
    out.extend(
        c.gloss_bias
            .iter()
            .enumerate()
            .map(|(i, &v)| (format!("gloss_bias[{i}]"), v)),
    );
    out.extend(
        c.reflectance_bias
            .iter()
            .enumerate()
            .map(|(i, &v)| (format!("reflectance_bias[{i}]"), v)),
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
    roles: SlotRoles,
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

/// The fold's cost: the equation of `c` against the one with `gain` and the tints `gain` divides out of them, on the
/// proof's random sets ([`prove`]'s sequence for `seed`); an error past [`BOUND_UNITS`].
fn fold_cost(c: &TwoDetailConstants, gain: f32, words: [[f32; 3]; 2], seed: u32) -> Result<FoldCost, String> {
    let mut random = Seeded::new(seed);
    let limit = BOUND_UNITS;
    let mut largest = 0f32;
    for _ in 0..PROOF_SETS {
        let s = random_samples(&mut random);
        let _uv = [random.unit(), random.unit()];
        let folded = equation(c, 1., &s);
        let unfolded = equation(&TwoDetailConstants { tint: words, ..*c }, gain, &s);
        for i in 0..3 {
            largest = largest
                .max(units(folded.albedo[i], unfolded.albedo[i]))
                .max(units(folded.reflectance[i], unfolded.reflectance[i]));
        }
    }
    if largest > limit {
        return Err(format!(
            "the tint fold moves the colour {largest} units, past {limit}"
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
        roles,
    };
    let (constants, frame, design_scale) = solve(&program)?;
    let proof = prove(&program, &constants, &frame, seed)
        .map_err(|e| format!("{constants:?} frame {frame:?}: {e}"))?;
    let fields = compare_fields(probe, outputs, roles)?;
    let tint_words: Option<Vec<f32>> = fields[..6]
        .iter()
        .map(|f| match f.fields.as_slice() {
            [(name, k)] => probe
                .material
                .iter()
                .find(|m| m.name == *name)
                .and_then(|m| probe.words(m.buffer, m.word + k, 1))
                .map(|w| f32::from_bits(w[0])),
            _ => None,
        })
        .collect();
    let fold = match (tint_words, fields[0].expected) {
        (Some(w), Some(_)) if fields[..6].iter().all(|f| f.agrees() == Some(true)) => {
            let gain = constants.tint[0][0] / w[0];
            Some(fold_cost(
                &constants,
                gain,
                [[w[0], w[1], w[2]], [w[3], w[4], w[5]]],
                seed,
            )?)
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

/// The textures of a two-detail program by their part in the equation, each read from what moves which output: the
/// base normal the one normal texture whose alpha moves the outputs (the mask); at each end of the mask (that alpha 1
/// and 0), the colour textures that move the colour and the normal textures that move the normal; the base colour the
/// one moving the colour at both ends, each detail the other at its end. An error names the structure that differs.
fn identify(probe: &ProgramProbe, outputs: Outputs) -> Result<SlotRoles, String> {
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
        return Err(format!("{} mask textures, not one", masks.len()));
    };
    if colours.len() != 3 || normals.len() != 3 {
        return Err(format!(
            "{} colour and {} normal textures, not three of each",
            colours.len(),
            normals.len()
        ));
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
    let mut alpha_moves = Vec::new();
    for &n in &normals {
        let (off, on) = (with(&mid, n, &|v| v[3] = 1.), with(&mid, n, &|v| v[3] = 0.));
        if differ(&run(&off)?, &run(&on)?) {
            alpha_moves.push(n);
        }
    }
    let [base_normal] = alpha_moves.as_slice() else {
        return Err(if alpha_moves.is_empty() {
            "no normal texture's alpha moves the outputs: the mask is not the base normal's (1 - N.a)^8"
                .to_owned()
        } else {
            format!(
                "the alphas of normal textures {} move the outputs, not one",
                slots(&alpha_moves)
            )
        });
    };
    // what moves the colour and the normal with the mask at `alpha`'s end
    let at = |alpha: f32| -> Result<(Vec<u32>, Vec<u32>), String> {
        let end = with(&mid, *base_normal, &|v| v[3] = alpha);
        let reference = run(&end)?;
        let mut moving_colour = Vec::new();
        for &c in &colours {
            if colour_differs(&reference, &run(&with(&end, c, &|v| v[..3].fill(0.025)))?) {
                moving_colour.push(c);
            }
        }
        let mut moving_normal = Vec::new();
        for &n in normals.iter().filter(|&&n| n != *base_normal) {
            let changed = run(&with(&end, n, &|v| v[0] = 0.75))?;
            if reference
                .normal
                .iter()
                .zip(changed.normal)
                .any(|(x, y)| moved(*x, y))
            {
                moving_normal.push(n);
            }
        }
        Ok((moving_colour, moving_normal))
    };
    let ((c0, k0), (c1, k1)) = (at(MASK_OFF)?, at(MASK_ON)?);
    if c0.is_empty() && c1.is_empty() {
        return Err("no colour texture moves the colour at either end of the mask".into());
    }
    let both: Vec<u32> = c0.iter().copied().filter(|c| c1.contains(c)).collect();
    let [base_colour] = both.as_slice() else {
        return Err(if both.is_empty() {
            format!(
                "no colour texture moves the colour at both ends of the mask ({} where it is 0, {} where it is 1): the \
                 base colour multiplies one detail only",
                slots(&c0),
                slots(&c1)
            )
        } else {
            format!(
                "colour textures {} move the colour at both ends of the mask, not one",
                slots(&both)
            )
        });
    };
    let one = |moving: &[u32], what: &str| -> Result<u32, String> {
        let rest: Vec<u32> = moving.iter().copied().filter(|c| c != base_colour).collect();
        match rest.as_slice() {
            [t] => Ok(*t),
            _ => Err(format!("{what}: {}, not one texture", slots(&rest))),
        }
    };
    let detail_colour = [
        one(&c0, "the detail colour where the mask is 0")?,
        one(&c1, "the detail colour where the mask is 1")?,
    ];
    let detail_normal = match (k0.as_slice(), k1.as_slice()) {
        ([k0], [k1]) if k0 != k1 => [*k0, *k1],
        _ => {
            return Err(format!(
                "the detail normals: {} move the normal where the mask is 0 and {} where it is 1, not one each",
                slots(&k0),
                slots(&k1)
            ));
        }
    };
    Ok(SlotRoles {
        base_colour: *base_colour,
        base_normal: *base_normal,
        base_mask: *mask,
        detail_colour,
        detail_normal,
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

/// The [`TwoDetailLerp`](primase_core::detail::TwoDetailLerp) `probe`'s program computes: its textures' parts read from
/// what moves which output ([`identify`]), its constants solved, proven and compared with the words they read; an
/// error naming what differs from the equation.
pub fn derive(probe: &mut ProgramProbe, seed: u32) -> Result<Derived, String> {
    let outputs = outputs(probe)?;
    let roles = match identify(probe, outputs) {
        Ok(roles) => roles,
        Err(e) if e.starts_with("no colour texture moves the colour at either end") => {
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
        Err(e) => return Err(e),
    };
    derive_with(probe, outputs, roles, seed)
}
