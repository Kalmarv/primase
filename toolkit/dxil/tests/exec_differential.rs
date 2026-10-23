#![allow(clippy::unwrap_used)]
//! The program executor on lifted DXIL against the DXIL evaluator, bit for bit, on the fixtures the evaluator runs
//! (it has no loops, switches or memory, and few `dx.op`s): the same inputs, constant buffers and sampler answers
//! through `PixelShader` on both, scalar and as a quad.
use std::path::Path;

#[path = "../../dxbc/tests/common/mod.rs"]
mod common;
use common::differential::{Tally, check, report, run};

fn compare(fixture: &str, cases: u32) -> Tally {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture),
    )
    .unwrap();
    let program = primase_dxil::lift(&bytes).unwrap();
    let reference = primase_dxil::Shader::parse(&bytes).unwrap();
    let mut t = Tally::default();
    run(fixture, program, &reference, cases, &mut t);
    report(fixture, &t);
    t
}

#[test]
fn the_fixtures_the_evaluator_runs() {
    for fixture in ["blend.dxil", "ops.dxil"] {
        let t = compare(fixture, 40);
        assert_eq!(t.lifted, 1, "{fixture} lifts and runs on the executor");
        assert!(
            t.runs > 0 && t.lanes > 0,
            "{fixture}: {} runs, {} lanes",
            t.runs,
            t.lanes
        );
        check(&t);
    }
}
