#![allow(clippy::unwrap_used)]
//! How fast the executor runs a lifted program against the DXBC evaluator, invocation for invocation, over the pixel
//! programs of a corpus (the corpus oracle set's `sekiro/shaders`, `riders/shaders`, `sledding/shaders`; the debug key
//! `shader.diff_limit` bounds the programs, `shader.speed_runs` the invocations timed per program, default 200). It
//! prints the distribution of the ratio and the slowest programs, and asserts nothing: a measurement, to be run with
//! `--release`.
mod common;
use common::{case, refs};
use primase_shader::exec::ProgramShader;
use primase_shader::{Load, PixelShader, Sample, Sampler};
use primase_testdata::Game;
use std::time::Instant;

/// A sampler that answers at once and the same for both: the time is the program's.
struct Fixed;
impl Sampler for Fixed {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        Ok([0.25 + r.texture as f32 / 64.0, 0.5, 0.75, 1.0])
    }
    fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
        Ok([r.texture, 1, 2, 3])
    }
    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        Ok(slot ^ element ^ offset)
    }
    fn buffer_elements(&mut self, _slot: u32) -> Option<u32> {
        Some(65536)
    }
}

struct Row {
    name: String,
    evaluator_ns: f64,
    executor_ns: f64,
    steps: u64,
}

fn measure(game: Game) {
    let Some((name, paths)) = common::corpus(game) else {
        return;
    };
    let runs: u32 = primase_testdata::debug("shader.speed_runs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let mut rows = Vec::new();
    for path in paths.iter().take(common::diff_limit()) {
        let bytes = std::fs::read(path).unwrap();
        let Ok(program) = primase_dxbc::lift(&bytes) else {
            continue;
        };
        if program.stage != primase_shader::Stage::Pixel {
            continue;
        }
        let Ok(reference) =
            primase_dxbc::Shader::parse(&bytes).or_else(|_| primase_dxbc::Shader::parse_stripped(&bytes))
        else {
            continue;
        };
        let Ok(executor) = ProgramShader::new(program) else {
            continue;
        };
        let case = case(&reference, &executor, 0);
        let zero = vec![[[0u32; 4]; 2]; case.inputs.len()];
        let run = |s: &dyn PixelShader| {
            s.execute_with_derivatives(&case.inputs, &zero, &refs(&case, s), &mut Fixed)
        };
        // both must run it, and the cost of one run bounds the timing
        let (Ok(e), Ok(x)) = (run(&reference), run(&executor)) else {
            continue;
        };
        let started = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(run(&reference).ok());
        }
        let evaluator_ns = started.elapsed().as_nanos() as f64 / f64::from(runs);
        let started = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(run(&executor).ok());
        }
        let executor_ns = started.elapsed().as_nanos() as f64 / f64::from(runs);
        rows.push(Row {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            evaluator_ns,
            executor_ns,
            steps: x.instructions as u64,
        });
        let _ = e;
    }
    rows.sort_by(|a, b| (a.executor_ns / a.evaluator_ns).total_cmp(&(b.executor_ns / b.evaluator_ns)));
    let n = rows.len();
    let q = |p: f64| {
        let r = &rows[((n as f64 - 1.0) * p) as usize];
        r.executor_ns / r.evaluator_ns
    };
    let (te, tx): (f64, f64) = rows
        .iter()
        .fold((0.0, 0.0), |a, r| (a.0 + r.evaluator_ns, a.1 + r.executor_ns));
    println!(
        "{name}: {n} programs, {runs} runs each; total executor/evaluator {:.2}; per program p10 {:.2} median {:.2} p90 {:.2} p99 {:.2} max {:.2}",
        tx / te,
        q(0.1),
        q(0.5),
        q(0.9),
        q(0.99),
        q(1.0)
    );
    for r in rows.iter().rev().take(15) {
        println!(
            "  {:10} executor {:9.0} ns, evaluator {:9.0} ns, ratio {:5.2}, {} operations per run",
            r.name,
            r.executor_ns,
            r.evaluator_ns,
            r.executor_ns / r.evaluator_ns,
            r.steps
        );
    }
}

#[test]
fn sekiro_speed() {
    measure(Game::Sekiro);
}
#[test]
fn riders_republic_speed() {
    measure(Game::Riders);
}
#[test]
fn unity_speed() {
    measure(Game::Sledding);
}
