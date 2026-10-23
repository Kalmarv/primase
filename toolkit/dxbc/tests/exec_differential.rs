#![allow(clippy::unwrap_used)]
//! The program executor against the DXBC evaluator, bit for bit: the same program, inputs, constant buffers and
//! sampler answers through `PixelShader` on both, scalar and as a quad. The evaluator is the reference the executor
//! replaces; where it refuses (a 3D or cube sample) the executor's answer is counted, not compared.
//!
//! The fixture program always runs; the corpora need `PRIMASE_SHADER_CORPUS_SEKIRO`, `PRIMASE_SHADER_CORPUS_RR` or
//! `PRIMASE_SHADER_CORPUS_UNITY` (a folder, searched recursively, of containers as the game ships them) and
//! optionally `PRIMASE_SHADER_DIFF_LIMIT` (programs to compare, default all).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_shader::exec::ProgramShader;
use primase_shader::{
    ConstantType, Execution, Load, Majorness, PixelShader, ResourceKind, Sample, Sampler, ScalarType,
    SystemValue,
};

fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn mix(x: u64) -> u32 {
    mix64(x) as u32
}

fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Answers every request with a value that depends on every field of the request, so two implementations agree only
/// where they make the same requests.
#[derive(Default)]
struct Hashing {
    requests: usize,
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

/// The word classes of a constant buffer: 1 bool, 2 integer, 3 float.
fn classify(ty: &ConstantType, base: u32, out: &mut BTreeMap<u32, u32>) {
    let class = |s: ScalarType| match s {
        ScalarType::Bool => 1,
        ScalarType::Sint | ScalarType::Uint => 2,
        _ => 3,
    };
    match ty {
        ConstantType::Scalar(s) => {
            out.insert(base / 4, class(*s));
        }
        ConstantType::Vector(v) => {
            for i in 0..u32::from(v.width) {
                out.insert(base / 4 + i, class(v.scalar));
            }
        }
        ConstantType::Matrix {
            scalar,
            rows,
            columns,
            major,
            stride,
        } => {
            let (lines, lanes) = match major {
                Majorness::Row => (*rows, *columns),
                Majorness::Column => (*columns, *rows),
            };
            for line in 0..u32::from(lines) {
                for lane in 0..u32::from(lanes) {
                    out.insert((base + line * stride) / 4 + lane, class(*scalar));
                }
            }
        }
        ConstantType::Array {
            element,
            count,
            stride,
        } => {
            for i in 0..*count {
                classify(element, base + i * stride, out);
            }
        }
        ConstantType::Struct { members } => {
            for m in members {
                classify(&m.ty, base + m.offset, out);
            }
        }
    }
}

/// One deterministic test vector: input registers, constant buffers.
struct Case {
    inputs: Vec<[u32; 4]>,
    buffers: Vec<(u32, Vec<[u32; 4]>)>,
}

fn case(shader: &dyn PixelShader, other: &dyn PixelShader, c: u32) -> Case {
    let interface = shader.interface();
    let mut inputs = vec![[0u32; 4]; shader.input_count().max(1)];
    let front = if c >= 2 {
        !c.is_multiple_of(3)
    } else {
        c.is_multiple_of(2)
    };
    for v in &interface.inputs {
        let Some(sig) = v.provenance.d3d else { continue };
        let Some(register) = inputs.get_mut(sig.register as usize) else {
            continue;
        };
        for (j, lane) in (0..4usize).filter(|b| sig.mask >> b & 1 != 0).enumerate() {
            let h = mix(u64::from(c) << 40 ^ u64::from(sig.register) << 8 ^ lane as u64 ^ 0x5151);
            register[lane] = match (v.system, v.value.scalar) {
                (Some(SystemValue::FragCoord), _) => [0.5f32, 0.5, 0.5, 1.0][lane].to_bits(),
                (Some(SystemValue::FrontFacing), _) => {
                    if front {
                        u32::MAX
                    } else {
                        0
                    }
                }
                (_, ScalarType::Float) => (unit(h) * if j == 3 { 1.0 } else { 2.0 } - 0.25).to_bits(),
                (_, ScalarType::Uint) => h % 4,
                (_, ScalarType::Sint) => (h % 5).wrapping_sub(2),
                _ => 0,
            };
        }
    }
    let mut buffers = Vec::new();
    // the buffers of both implementations, each as long as the longer of the two says: a lifted program's array is
    // the larger of the declared rows and the reflection's
    let mut sizes: BTreeMap<u32, u32> = BTreeMap::new();
    for &(slot, rows) in shader.constant_buffers().iter().chain(other.constant_buffers()) {
        let e = sizes.entry(slot).or_default();
        *e = (*e).max(rows);
    }
    for (slot, rows) in sizes {
        let mut classes = BTreeMap::new();
        for r in &interface.resources {
            if r.provenance.d3d.is_some_and(|d| d.register == slot)
                && let ResourceKind::ConstantBuffer { layout } = &r.kind
            {
                for k in &layout.constants {
                    classify(&k.ty, k.offset, &mut classes);
                }
            }
        }
        let mut v = vec![[0u32; 4]; rows as usize];
        for (r, row) in v.iter_mut().enumerate() {
            for (k, w) in row.iter_mut().enumerate() {
                let word = (r * 4 + k) as u32;
                let h = mix(u64::from(c) << 44 ^ u64::from(slot) << 32 ^ u64::from(word) ^ 0x77);
                *w = match classes.get(&word).copied().unwrap_or(3) {
                    1 => h & 1,
                    2 => h % 3,
                    _ => (unit(h) * 2.0 - 0.25).to_bits(),
                };
            }
        }
        buffers.push((slot, v));
    }
    Case { inputs, buffers }
}

/// Lane `k` of a quad: every float input advances by (0.01, 0.02) per step in x and y.
fn quad_lane(shader: &dyn PixelShader, c: &Case, k: usize) -> Vec<[u32; 4]> {
    let off = (k & 1) as f32 * 0.01 + (k >> 1) as f32 * 0.02;
    let mut inputs = c.inputs.clone();
    for v in &shader.interface().inputs {
        let Some(sig) = v.provenance.d3d else { continue };
        if v.system.is_some() || v.value.scalar != ScalarType::Float {
            continue;
        }
        let Some(register) = inputs.get_mut(sig.register as usize) else {
            continue;
        };
        for lane in (0..4usize).filter(|b| sig.mask >> b & 1 != 0) {
            register[lane] = (f32::from_bits(register[lane]) + off).to_bits();
        }
    }
    inputs
}

/// The buffers as `shader` takes them: each as long as the shader says (the evaluator wants its declared rows exactly,
/// a lifted program's array may be longer), the contents the same word for word.
fn refs<'a>(c: &'a Case, shader: &dyn PixelShader) -> Vec<(u32, &'a [[u32; 4]])> {
    shader
        .constant_buffers()
        .iter()
        .filter_map(|&(slot, rows)| {
            let (_, v) = c.buffers.iter().find(|(s, _)| *s == slot)?;
            Some((slot, &v[..(rows as usize).min(v.len())]))
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    programs: usize,
    lifted: usize,
    runs: usize,
    lanes: usize,
    exact: usize,
    discard_differs: usize,
    sample_counts_differ: usize,
    mask_differs: usize,
    evaluator_refuses: BTreeMap<String, usize>,
    executor_refuses: BTreeMap<String, usize>,
    /// The programs the executor refused where the evaluator ran.
    executor_refusing: BTreeMap<String, String>,
    both_refuse: usize,
    /// By program: how many lanes or discards differ, and the first.
    mismatches: BTreeMap<String, (usize, String)>,
}

fn shape(e: &impl ToString) -> String {
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

fn same(a: u32, b: u32) -> bool {
    a == b || (f32::from_bits(a).is_nan() && f32::from_bits(b).is_nan())
}

fn compare(name: &str, reference: &Execution, got: &Execution, t: &mut Tally) {
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

fn run_program(name: &str, bytes: &[u8], cases: u32, t: &mut Tally) {
    t.programs += 1;
    let Ok(program) = primase_dxbc::lift(bytes) else {
        return;
    };
    if program.stage != primase_shader::Stage::Pixel {
        return;
    }
    let Ok(reference) =
        primase_dxbc::Shader::parse(bytes).or_else(|_| primase_dxbc::Shader::parse_stripped(bytes))
    else {
        return;
    };
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
        let case = case(&reference, &executor, c);
        let zero = vec![[[0u32; 4]; 2]; case.inputs.len()];
        // scalar
        let run = |s: &dyn PixelShader| {
            s.execute_with_derivatives(&case.inputs, &zero, &refs(&case, s), &mut Hashing::default())
        };
        let (e, x) = (run(&reference), run(&executor));
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
        let lanes: Vec<Vec<[u32; 4]>> = (0..4).map(|k| quad_lane(&reference, &case, k)).collect();
        let quad = |s: &dyn PixelShader| {
            s.execute_quad(
                [&lanes[0], &lanes[1], &lanes[2], &lanes[3]],
                &refs(&case, s),
                &mut Hashing::default(),
            )
        };
        match (quad(&reference), quad(&executor)) {
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

fn report(title: &str, t: &Tally) {
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
fn check(t: &Tally) {
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

fn containers(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            containers(&path, out);
        } else if std::fs::read(&path).is_ok_and(|b| b.starts_with(b"DXBC")) {
            out.push(path);
        }
    }
}

fn corpus(variable: &str) {
    let Some(root) = primase_testdata::real_data(variable) else {
        return;
    };
    let limit = std::env::var("PRIMASE_SHADER_DIFF_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let mut paths = Vec::new();
    containers(Path::new(&root), &mut paths);
    assert!(
        !paths.is_empty(),
        "{variable}: no DXBC containers under {}",
        root.display()
    );
    let mut t = Tally::default();
    for path in paths.iter().take(limit) {
        let bytes = std::fs::read(path).unwrap();
        run_program(&path.display().to_string(), &bytes, 3, &mut t);
    }
    report(variable, &t);
    check(&t);
}

#[test]
fn the_modes_fixture() {
    let bytes =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/modes.dxbc")).unwrap();
    let mut t = Tally::default();
    run_program("modes.dxbc", &bytes, 6, &mut t);
    report("fixture", &t);
    assert_eq!(t.lifted, 1, "the fixture lifts and runs on the executor");
    assert!(t.runs > 0 && t.lanes > 0);
    check(&t);
}

#[test]
fn sekiro_programs() {
    corpus("PRIMASE_SHADER_CORPUS_SEKIRO");
}
#[test]
fn riders_republic_programs() {
    corpus("PRIMASE_SHADER_CORPUS_RR");
}
#[test]
fn unity_programs() {
    corpus("PRIMASE_SHADER_CORPUS_UNITY");
}

/// Records every request a run makes, then answers as [`Hashing`] does.
#[derive(Default)]
struct Logging {
    inner: Hashing,
    log: Vec<String>,
}
impl Sampler for Logging {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        self.log.push(format!("{r:?}"));
        self.inner.sample(r)
    }
    fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
        self.log.push(format!("{r:?}"));
        self.inner.load(r)
    }
    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        self.inner.buffer_elements(slot)
    }
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        self.log.push(format!("structured t{slot} {element} {offset}"));
        self.inner.structured(slot, element, offset)
    }
}

/// `PRIMASE_SHADER_DIFF_TRACE=<container>` (and `PRIMASE_SHADER_DIFF_CASE=<n>`, default 0): both implementations'
/// requests and outputs for that case, scalar and as a quad, printed. A debugging aid for a disagreement the corpus
/// run reports.
#[test]
fn trace_one_program() {
    let Ok(path) = std::env::var("PRIMASE_SHADER_DIFF_TRACE") else {
        return;
    };
    let c: u32 = std::env::var("PRIMASE_SHADER_DIFF_CASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let bytes = std::fs::read(&path).unwrap();
    let reference = primase_dxbc::Shader::parse(&bytes)
        .or_else(|_| primase_dxbc::Shader::parse_stripped(&bytes))
        .unwrap();
    let executor = ProgramShader::new(primase_dxbc::lift(&bytes).unwrap()).unwrap();
    let case = case(&reference, &executor, c);
    println!("inputs {:x?}", case.inputs);
    let zero = vec![[[0u32; 4]; 2]; case.inputs.len()];
    let show = |r: Result<Execution, primase_shader::Error>, s: &Logging| {
        for l in &s.log {
            println!("  {l}");
        }
        match r {
            Ok(e) => println!(
                "  outputs {:x?} masks {:x?} discarded {} samples {}",
                e.outputs, e.output_masks, e.discarded, e.samples
            ),
            Err(e) => println!("  error {e}"),
        }
    };
    let mut logs: Vec<Vec<String>> = Vec::new();
    for (what, shader) in [
        ("evaluator", &reference as &dyn PixelShader),
        ("executor", &executor),
    ] {
        let mut s = Logging::default();
        let r = shader.execute_with_derivatives(&case.inputs, &zero, &refs(&case, shader), &mut s);
        println!("== {what} scalar");
        show(r, &s);
        let lanes: Vec<Vec<[u32; 4]>> = (0..4).map(|k| quad_lane(&reference, &case, k)).collect();
        let mut s = Logging::default();
        let r = shader.execute_quad(
            [&lanes[0], &lanes[1], &lanes[2], &lanes[3]],
            &refs(&case, shader),
            &mut s,
        );
        println!("== {what} quad (lane 0 shown; {} requests)", s.log.len());
        logs.push(std::mem::take(&mut s.log));
        show(r.map(|e| e.into_iter().next().unwrap()), &Logging::default());
    }
    let at = logs[0].iter().zip(&logs[1]).position(|(a, b)| a != b);
    match at {
        Some(i) => {
            println!("quad requests first differ at {i}:");
            let from = i.saturating_sub(2);
            for (k, (a, b)) in logs[0].iter().zip(&logs[1]).enumerate().skip(from).take(5) {
                println!("  {k} evaluator {a}");
                println!("  {k} executor  {b}");
            }
        }
        None => println!(
            "quad requests agree over {} entries (evaluator {}, executor {})",
            logs[0].len().min(logs[1].len()),
            logs[0].len(),
            logs[1].len()
        ),
    }
}

/// `PRIMASE_SHADER_DIFF_TEXT=<container>`: the lifted program as text, printed.
#[test]
fn print_one_program() {
    let Ok(path) = std::env::var("PRIMASE_SHADER_DIFF_TEXT") else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    println!("{}", primase_dxbc::lift(&bytes).unwrap().text().unwrap());
}
