#![allow(dead_code)]
//! The executor against a bytecode's own evaluator, bit for bit, shared by the DXBC and DXIL gates: the same program,
//! inputs, constant buffers and sampler answers through `PixelShader` on both, scalar and as a quad. The evaluator is
//! the reference the executor replaces; where it refuses (a 3D or cube sample) the executor's answer is counted, not
//! compared.
use std::collections::BTreeMap;

use super::{case, mix, mix64, quad_lane, refs, unit};
use primase_shader::exec::ProgramShader;
use primase_shader::{Execution, Load, PixelShader, Program, Sample, Sampler};

/// Answers every request with a value that depends on every field of the request, so two implementations agree only
/// where they make the same requests.
#[derive(Default)]
pub struct Hashing {
    pub requests: usize,
}
impl Hashing {
    fn lanes(seed: u64) -> [u32; 4] {
        std::array::from_fn(|i| {
            let h = mix(seed ^ (i as u64) << 56);
            // small integers and unit floats alternate, so a program that reads them either way stays in range
            if h & 1 == 0 {
                (h >> 1) % 4
            } else {
                (0.125 + 0.875 * unit(h >> 1)).to_bits()
            }
        })
    }
}
impl Sampler for Hashing {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        self.requests += 1;
        // a NaN has one bit pattern here: which payload an operation leaves is not defined
        let bits = |x: f32| u64::from(if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() });
        let fold = |seed: u64, g: &[[f32; 2]; 2]| g.iter().flatten().fold(seed, |a, x| mix64(a ^ bits(*x)));
        let lod = match r.lod {
            primase_shader::Lod::Implicit => 1,
            primase_shader::Lod::Bias(b) => mix64(2 ^ bits(b) << 8),
            primase_shader::Lod::Level(l) => mix64(3 ^ bits(l) << 8),
            primase_shader::Lod::Gradients(g) => fold(4, &g),
            primase_shader::Lod::GradientsBias(g, b) => fold(5 ^ bits(b) << 8, &g),
        };
        // a sampler rounds the layer the program gives; the lifted program already rounds and clamps it
        let layer = r.layer.map_or(7, |l| bits(l.round_ties_even().max(0.0)));
        let seed = mix64(u64::from(r.texture) << 32 | u64::from(r.sampler))
            ^ mix64(bits(r.uv[0]) << 32 | bits(r.uv[1]))
            ^ mix64(layer)
            ^ lod;
        Ok(Self::lanes(seed).map(|w| {
            // a sample is a float: integers become small floats
            let f = f32::from_bits(w);
            if f.is_subnormal() || f == 0.0 {
                0.25 + w as f32 / 8.0
            } else {
                f
            }
        }))
    }
    fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
        self.requests += 1;
        // a buffer or texture of 65,536 texels: a read outside it is zeros, as D3D defines
        if r.texel.iter().any(|&t| !(0..65536).contains(&t)) || !(0..16).contains(&r.mip) {
            return Ok([0; 4]);
        }
        let seed = u64::from(r.texture) << 40
            ^ (r.texel[0] as u64) << 24
            ^ (r.texel[1] as u64) << 12
            ^ r.texel[2] as u64
            ^ (r.mip as u64) << 50;
        Ok(Self::lanes(mix64(seed)))
    }
    fn buffer_elements(&mut self, _slot: u32) -> Option<u32> {
        Some(65536)
    }
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        self.requests += 1;
        if element >= 65536 {
            return Ok(0);
        }
        Ok(Self::lanes(u64::from(slot) << 40 ^ u64::from(element) << 12 ^ u64::from(offset))[0])
    }
}

#[derive(Default)]
pub struct Tally {
    pub programs: usize,
    pub lifted: usize,
    pub runs: usize,
    pub lanes: usize,
    pub exact: usize,
    pub discard_differs: usize,
    pub sample_counts_differ: usize,
    pub mask_differs: usize,
    pub evaluator_refuses: BTreeMap<String, usize>,
    pub executor_refuses: BTreeMap<String, usize>,
    /// The programs the executor refused where the evaluator ran.
    pub executor_refusing: BTreeMap<String, String>,
    pub both_refuse: usize,
    /// By program: how many lanes or discards differ, and the first.
    pub mismatches: BTreeMap<String, (usize, String)>,
}

pub fn shape(e: &impl ToString) -> String {
    e.to_string()
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .take(90)
        .collect()
}

impl Tally {
    fn mismatch(&mut self, name: &str, what: String) {
        let program = name.split(" case").next().unwrap_or(name).to_owned();
        let e = self
            .mismatches
            .entry(program)
            .or_insert_with(|| (0, format!("{name}: {what}")));
        e.0 += 1;
    }
}

pub fn same(a: u32, b: u32) -> bool {
    a == b || (f32::from_bits(a).is_nan() && f32::from_bits(b).is_nan())
}

pub fn compare(name: &str, reference: &Execution, got: &Execution, t: &mut Tally) {
    t.runs += 1;
    if reference.discarded != got.discarded {
        t.discard_differs += 1;
        t.mismatch(
            name,
            format!("discarded {} vs {}", reference.discarded, got.discarded),
        );
    }
    // a discarded invocation writes nothing in D3D: the evaluator stops at the discard with the outputs so far (the
    // quad's helper lanes run on), the executor stops with none, so only an invocation neither discards is compared
    if reference.discarded || got.discarded {
        return;
    }
    if reference.samples != got.samples {
        t.sample_counts_differ += 1;
    }
    if reference.output_masks != got.output_masks {
        t.mask_differs += 1;
    }
    for (register, mask) in reference.output_masks.iter().enumerate() {
        for lane in (0..4usize).filter(|l| mask >> l & 1 != 0) {
            t.lanes += 1;
            let (x, y) = (
                reference.outputs[register][lane],
                got.outputs.get(register).map_or(0, |r| r[lane]),
            );
            if same(x, y) {
                t.exact += 1;
            } else {
                t.mismatch(
                    name,
                    format!("o{register}.{lane} evaluator {x:#010x} executor {y:#010x}"),
                );
            }
        }
    }
}

/// Compare the executor on `program` with `reference`, `cases` test vectors each, scalar and as a quad.
pub fn run(name: &str, program: Program, reference: &dyn PixelShader, cases: u32, t: &mut Tally) {
    t.programs += 1;
    let executor = match ProgramShader::new(program) {
        Ok(e) => e,
        Err(e) => {
            *t.executor_refuses
                .entry(format!("new: {}", shape(&e)))
                .or_default() += 1;
            return;
        }
    };
    t.lifted += 1;
    for c in 0..cases {
        let case = case(reference, &executor, c);
        let zero = vec![[[0u32; 4]; 2]; case.inputs.len()];
        // scalar
        let run = |s: &dyn PixelShader| {
            s.execute_with_derivatives(&case.inputs, &zero, &refs(&case, s), &mut Hashing::default())
        };
        let (e, x) = (run(reference), run(&executor));
        match (e, x) {
            (Ok(e), Ok(x)) => compare(&format!("{name} case {c}"), &e, &x, t),
            (Err(e), Ok(_)) => *t.evaluator_refuses.entry(shape(&e)).or_default() += 1,
            (Ok(_), Err(x)) => {
                *t.executor_refuses.entry(shape(&x)).or_default() += 1;
                t.executor_refusing
                    .entry(name.to_owned())
                    .or_insert_with(|| x.to_string());
            }
            (Err(_), Err(_)) => t.both_refuse += 1,
        }
        // quad
        let lanes: Vec<Vec<[u32; 4]>> = (0..4).map(|k| quad_lane(reference, &case, k)).collect();
        let quad = |s: &dyn PixelShader| {
            s.execute_quad(
                [&lanes[0], &lanes[1], &lanes[2], &lanes[3]],
                &refs(&case, s),
                &mut Hashing::default(),
            )
        };
        match (quad(reference), quad(&executor)) {
            (Ok(e), Ok(x)) => {
                for k in 0..4 {
                    compare(&format!("{name} case {c} quad lane {k}"), &e[k], &x[k], t);
                }
            }
            (Err(e), Ok(_)) => {
                *t.evaluator_refuses
                    .entry(format!("quad: {}", shape(&e)))
                    .or_default() += 1
            }
            (Ok(_), Err(x)) => {
                *t.executor_refuses
                    .entry(format!("quad: {}", shape(&x)))
                    .or_default() += 1
            }
            (Err(_), Err(_)) => t.both_refuse += 1,
        }
    }
}

pub fn report(title: &str, t: &Tally) {
    println!(
        "{title}: {} programs, {} lifted and compared, {} runs, {} lanes, {} bit-exact; discard differs {}, sample counts differ {}, written masks differ {}, both refuse {}",
        t.programs,
        t.lifted,
        t.runs,
        t.lanes,
        t.exact,
        t.discard_differs,
        t.sample_counts_differ,
        t.mask_differs,
        t.both_refuse
    );
    for (what, list) in [
        ("evaluator refuses", &t.evaluator_refuses),
        ("EXECUTOR REFUSES", &t.executor_refuses),
    ] {
        for (k, n) in list {
            println!("  {what} {n:5}  {k}");
        }
    }
    for (program, why) in t.executor_refusing.iter().take(20) {
        println!("  EXECUTOR REFUSED {program}: {why}");
    }
    for (program, (n, first)) in &t.mismatches {
        println!("  MISMATCH {n:4} in {program}: {first}");
    }
}

/// What may differ: nothing in a value or a discard; the executor refusing what the evaluator ran is a failure unless
/// the reason is a run that never ends (the two have different step limits).
pub fn check(t: &Tally) {
    assert!(
        t.mismatches.is_empty(),
        "lanes or discards differ in {} programs",
        t.mismatches.len()
    );
    assert_eq!(t.exact, t.lanes);
    let unexpected: Vec<_> = t
        .executor_refuses
        .iter()
        .filter(|(k, _)| {
            !k.contains("budget") && !k.contains("step limit") && !k.contains("execution budget")
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "the executor refuses what the evaluator ran: {unexpected:?}"
    );
}
