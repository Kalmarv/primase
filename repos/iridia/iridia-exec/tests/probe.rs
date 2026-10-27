//! The vertex probe ([`iridia_exec::probe`]) on a vertex program that reads a texture of its own, as fxc and dxc
//! compile it (fixtures/probe_vs.hlsl): the texture's reads are answered with the stated stand-ins and counted, a check
//! that fails names them, and a program that is not a vertex program is refused.
use std::collections::BTreeMap;

use iridia::{Program, ResourceKind};
use iridia_exec::probe::{
    LOAD_STAND_IN, QUERY_STAND_IN, SAMPLE_STAND_IN, TextureUse, VertexProbe, Zeros,
};

const POSITION: [f32; 4] = [0.25, 0.5, 0.75, 1.0];
const SCALE: [f32; 4] = [2.0, 2.0, 2.0, 1.0];
const OFFSET: [f32; 4] = [0.0, 0.0, 0.5, 0.0];

fn programs() -> [(&'static str, Program); 2] {
    [
        (
            "fxc",
            primase_dxbc::lift(include_bytes!("fixtures/probe_vs.dxbc")).expect("fxc's program lifts"),
        ),
        (
            "dxc",
            primase_dxil::lift(include_bytes!("fixtures/probe_vs.dxil")).expect("dxc's program lifts"),
        ),
    ]
}

fn bits(v: [f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// One run of the probe with `lift`: the constant buffer `Probe` as the fixture lays it out, the position and a UV.
fn run(probe: &VertexProbe, lift: f32) -> iridia_exec::probe::ProbeRun<Zeros> {
    let program = probe.program();
    let buffer = program
        .interface
        .resources
        .iter()
        .find(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
        .expect("the constant buffer")
        .id;
    let constants = BTreeMap::from([(
        buffer,
        vec![bits(SCALE), bits(OFFSET), bits([lift, 0.0, 0.0, 0.0])],
    )]);
    let inputs: Vec<[u32; 4]> = program
        .interface
        .inputs
        .iter()
        .map(|i| match i.label.name.to_ascii_uppercase().as_str() {
            "POSITION" => bits(POSITION),
            _ => [0; 4],
        })
        .collect();
    probe.run(constants, &inputs, Zeros).expect("the program runs")
}

fn near(got: [f32; 4], want: [f32; 4]) -> bool {
    got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1e-5)
}

/// Each texture read is answered with its stand-in and counted. With `lift` zero the clip position is the constants'
/// alone; with `lift` one it moves by what the stand-ins answer (a size of one texel and one level in each of the three
/// numbers the program adds, a sample's x, a load's y), so a check of the position fails and the reason names them.
#[test]
fn a_programs_texture_is_answered_with_stand_ins_and_counted() {
    let base: [f32; 4] = std::array::from_fn(|k| POSITION[k] * SCALE[k] + OFFSET[k]);
    // the size query answers the width and rows, the level-count query the levels
    let sizes = (QUERY_STAND_IN[0] + QUERY_STAND_IN[1] + QUERY_STAND_IN[0]) as f32;
    let moved = [
        base[0] + sizes,
        base[1] + SAMPLE_STAND_IN[0],
        base[2] + f32::from_bits(LOAD_STAND_IN[1]),
        base[3],
    ];
    // one sample, one load, and two queries: both lifters write `GetDimensions` of a texture's size and level count
    // (fxc's `resinfo`, dxc's `getDimensions`) as a size query and a level-count query
    let counted = TextureUse {
        samples: 1,
        loads: 1,
        queries: 2,
    };
    for (compiler, program) in programs() {
        let probe = VertexProbe::new(program).expect("a vertex program with a clip position");
        let still = run(&probe, 0.0);
        assert!(near(still.clip, base), "{compiler}: {:?}", still.clip);
        assert_eq!(still.textures, counted, "{compiler}");
        let lifted = run(&probe, 1.0);
        assert!(
            near(lifted.clip, moved),
            "{compiler}: {:?} not {moved:?}",
            lifted.clip
        );
        assert_eq!(lifted.textures, counted, "{compiler}");
        let why = lifted
            .textures
            .reason("its clip position is not the position given".into());
        assert!(
            why.starts_with("its clip position is not the position given; "),
            "{why}"
        );
        for part in [
            "1 samples answered with [0.5, 0.5, 0.5, 0.5]",
            "1 loads answered with [0, 0, 0, 0]",
            "2 size and level-count queries answered with [1, 1, 1, 1]",
        ] {
            assert!(why.contains(part), "{compiler}: {why}");
        }
    }
}

/// A run that reads no texture gives a reason with nothing added.
#[test]
fn a_run_that_reads_no_texture_names_no_stand_in() {
    let none = TextureUse::default();
    assert!(none.none());
    assert_eq!(none.stand_ins(), None);
    assert_eq!(none.reason("why".into()), "why");
}

/// A program that is not a vertex program is refused before it is run.
#[test]
fn a_pixel_program_is_refused() {
    let pixel = primase_dxbc::lift(include_bytes!("fixtures/lanes.dxbc")).expect("the pixel program lifts");
    let refused = VertexProbe::new(pixel).expect_err("a pixel program");
    assert_eq!(refused, "the program is not a vertex program");
}
