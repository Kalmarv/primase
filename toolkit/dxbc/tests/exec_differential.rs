#![allow(clippy::unwrap_used)]
//! The program executor against the DXBC evaluator, bit for bit: the same program, inputs, constant buffers and
//! sampler answers through `PixelShader` on both, scalar and as a quad. The evaluator is the reference the executor
//! replaces; where it refuses (a 3D or cube sample) the executor's answer is counted, not compared.
//!
//! The fixture program always runs; the corpora need `PRIMASE_SHADER_CORPUS_SEKIRO`, `PRIMASE_SHADER_CORPUS_RR` or
//! `PRIMASE_SHADER_CORPUS_UNITY` (a folder, searched recursively, of containers as the game ships them) and
//! optionally `PRIMASE_SHADER_DIFF_LIMIT` (programs to compare, default all).
use std::path::Path;

mod common;
use common::differential::{Hashing, Tally, check, report, run};
use common::{case, containers, quad_lane, refs};
use primase_shader::exec::ProgramShader;
use primase_shader::{Execution, Load, PixelShader, Sample, Sampler};

fn run_program(name: &str, bytes: &[u8], cases: u32, t: &mut Tally) {
    let Ok(program) = primase_dxbc::lift(bytes) else {
        t.programs += 1;
        return;
    };
    if program.stage != primase_shader::Stage::Pixel {
        t.programs += 1;
        return;
    }
    let Ok(reference) =
        primase_dxbc::Shader::parse(bytes).or_else(|_| primase_dxbc::Shader::parse_stripped(bytes))
    else {
        t.programs += 1;
        return;
    };
    run(name, program, &reference, cases, t);
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
