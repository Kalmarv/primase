#![allow(clippy::unwrap_used)]
//! Independent hand-worked answers for lane slicing; full-program comparisons are regressions, not an oracle.
use iridia_d3d9::{lift, project_vertex_output};
use iridia::flow::{At, Node, Values};
use iridia_exec::{PixelShader, ProgramShader, Sample};

fn op(code: u32, args: &[u32]) -> Vec<u32> {
    [vec![code | (args.len() as u32) << 24], args.to_vec()].concat()
}
fn bytes(body: &[u32]) -> Vec<u8> {
    [vec![0xfffe_0300], body.to_vec(), vec![65535]]
        .concat()
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn declarations() -> Vec<u32> {
    [
        op(31, &[0x8000_0000, 0x900f_0000]), // v0 POSITION
        op(31, &[0x8000_0005, 0x900f_0001]), // v1 TEXCOORD0
        op(31, &[0x8000_0003, 0x900f_0002]), // v2 NORMAL
        op(31, &[0x8000_0000, 0xe00f_0007]), // o7 POSITION
        op(31, &[0x8007_0005, 0xe00f_0003]), // o3 TEXCOORD7
        op(31, &[0x8000_000a, 0xe00f_0005]), // o5 COLOR0
    ]
    .concat()
}
fn fixture() -> Vec<u8> {
    bytes(
        &[
            declarations(),
            op(81, &[0xa00f_0001, 1f32.to_bits(), 0, 0, 0]),
            op(1, &[0xe00f_0007, 0x90e4_0000]),              // position
            op(5, &[0xe00f_0005, 0x90e4_0002, 0xa0e4_0004]), // unrelated frame value
            op(4, &[0x8007_0000, 0x90c4_0001, 0xa050_0001, 0xa005_0001]), // r0.xyz = (u,v,1)
            op(8, &[0xe001_0003, 0xa0e4_000a, 0x80e4_0000]),
            op(8, &[0xe002_0003, 0xa0e4_000b, 0x80e4_0000]),
            op(1, &[0xe00c_0003, 0xa0e4_000c]), // unrelated z/w
        ]
        .concat(),
    )
}

#[test]
fn output_slice_keeps_original_hash_and_arithmetic_without_other_outputs() {
    let raw = fixture();
    let full = lift(&raw).unwrap();
    let sliced = project_vertex_output(&raw, 3, 3).unwrap();
    assert_eq!(sliced.stage, iridia::Stage::Pixel);
    assert_eq!(sliced.provenance.source_hash, full.provenance.source_hash);
    assert_ne!(sliced.provenance.lifter, full.provenance.lifter);
    assert!(sliced.notes.iter().any(|n| n.contains("placeholders")));
    iridia::emit::wgsl(&sliced).unwrap();
    let graph = Values::of_program(&sliced).unwrap();
    let mut input_lanes = Vec::new();
    let mut words = Vec::new();
    for n in graph.reached_from(graph.roots()) {
        match graph.node(n) {
            Node::Input { input, lane } => input_lanes.push((*input, *lane)),
            Node::Global {
                at: At::Component(word),
                ..
            } => words.push(*word),
            _ => {}
        }
    }
    input_lanes.sort();
    words.sort();
    assert_eq!(input_lanes, [(1, 0), (1, 1)]);
    assert_eq!(words, [40, 41, 42, 44, 45, 46]);
    let mut constants = [[f32::NAN.to_bits(); 4]; 13];
    constants[10] = [2f32, -3., 0.25, f32::NAN].map(f32::to_bits);
    constants[11] = [5f32, 7., -2., f32::NAN].map(f32::to_bits);
    let mut inputs = [[f32::NAN.to_bits(); 4]; 3];
    inputs[1] = [0.4f32, -0.2, f32::NAN, f32::NAN].map(f32::to_bits);
    let got = ProgramShader::new(sliced)
        .unwrap()
        .execute(&inputs, &[(0, &constants)], &mut |_: Sample| {
            Err("no texture".into())
        })
        .unwrap();
    let actual = got.outputs[0].map(f32::from_bits);
    for (a, b) in actual.into_iter().zip([1.65, -1.4, 0., 1.]) {
        assert!((a - b).abs() < 1e-6, "{actual:?}");
    }
}

#[test]
fn partial_overwrites_aliasing_and_every_mask_preserve_original_lane_positions() {
    let raw = bytes(
        &[
            declarations(),
            op(1, &[0xe00f_0007, 0x90e4_0000]),
            op(1, &[0xe00f_0005, 0x90e4_0002]),
            op(1, &[0x800f_0000, 0x90e4_0001]),
            op(5, &[0x8001_0000, 0x8055_0000, 0xa000_000a]), // r0.x = old r0.y * c10.x
            op(2, &[0x8002_0000, 0x8000_0000, 0xa055_000a]), // r0.y = new r0.x + c10.y
            op(1, &[0xe00f_0003, 0x80e4_0000]),
        ]
        .concat(),
    );
    let inputs = [
        [0f32, 0., 0., 1.].map(f32::to_bits),
        [2f32, 3., 4., 5.].map(f32::to_bits),
        [0; 4],
    ];
    let mut constants = [[0u32; 4]; 11];
    constants[10] = [7f32, 11., 0., 0.].map(f32::to_bits);
    let full = ProgramShader::new(lift(&raw).unwrap())
        .unwrap()
        .execute_vertex(&inputs, &[(0, &constants)], &mut |_: Sample| {
            Err("no texture".into())
        })
        .unwrap()
        .0;
    assert_eq!(full.outputs[3].map(f32::from_bits), [21., 32., 4., 5.]);
    for mask in 1..16 {
        let p = project_vertex_output(&raw, 3, mask).unwrap();
        let got = ProgramShader::new(p)
            .unwrap()
            .execute(&inputs, &[(0, &constants)], &mut |_: Sample| {
                Err("no texture".into())
            })
            .unwrap();
        let expected = std::array::from_fn(|lane| {
            if mask & (1 << lane) != 0 {
                [21f32, 32., 4., 5.][lane]
            } else {
                [0f32, 0., 0., 1.][lane]
            }
        });
        assert_eq!(got.outputs[0].map(f32::from_bits), expected, "mask {mask}");
    }
}

#[test]
fn projection_refuses_bad_selection_wrong_stage_and_invalid_dead_code() {
    let raw = fixture();
    for (register, mask) in [(3, 0), (3, 16), (9, 3)] {
        assert!(
            project_vertex_output(&raw, register, mask)
                .unwrap_err()
                .detail
                .contains("mask")
        );
    }
    let ps = [0xffff_0300u32, 65535]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    assert!(
        project_vertex_output(&ps, 0, 3)
            .unwrap_err()
            .detail
            .contains("requires vs_3_0")
    );
    let mut invalid = raw.clone();
    let end = invalid.len() - 4;
    invalid.splice(
        end..end,
        op(40, &[0xa0e4_0000]).into_iter().flat_map(u32::to_le_bytes),
    );
    assert!(
        project_vertex_output(&invalid, 3, 3)
            .unwrap_err()
            .detail
            .contains("opcode 40")
    );
    let missing = bytes(&[declarations(), op(1, &[0xe00f_0003, 0x90e4_0001])].concat());
    assert!(
        project_vertex_output(&missing, 3, 3)
            .unwrap_err()
            .detail
            .contains("uninitialized output")
    );
}

#[test]
fn projected_vertex_output_has_a_versioned_ir_baseline() {
    let outcome = project_vertex_output(&fixture(), 3, 3)
        .unwrap()
        .text()
        .map_err(|e| e.to_string());
    let table = iridia::gate::table(
        "primase-d3d9 vertex output",
        iridia_d3d9::LIFTER_VERSION,
        &[("uv".into(), outcome)],
    );
    iridia::gate::check(include_str!("vertex-output-versions.tsv"), &table)
        .unwrap_or_else(|e| panic!("{e}\n{table}"));
}

#[test]
fn normalize_fourth_lane_and_scalar_swizzles_keep_their_dependencies() {
    let wrap = |middle: Vec<u32>| {
        bytes(
            &[
                declarations(),
                op(1, &[0xe00f_0007, 0x90e4_0000]),
                op(1, &[0xe00f_0005, 0x90e4_0002]),
                middle,
                op(1, &[0xe00f_0003, 0x80e4_0000]),
            ]
            .concat(),
        )
    };
    let inputs = [
        [0f32, 0., 0., 1.].map(f32::to_bits),
        [0f32, 3., 4., 10.].map(f32::to_bits),
        [0; 4],
    ];
    let raw = wrap(op(36, &[0x800f_0000, 0x90e4_0001]));
    for (mask, expected) in [(8, [0f32, 0., 0., 2.]), (2, [0f32, 0.6, 0., 1.])] {
        let p = project_vertex_output(&raw, 3, mask).unwrap();
        let got = ProgramShader::new(p)
            .unwrap()
            .execute(&inputs, &[], &mut |_: Sample| Err("no texture".into()))
            .unwrap();
        for (got, want) in got.outputs[0].map(f32::from_bits).into_iter().zip(expected) {
            assert!((got - want).abs() < 1e-6);
        }
    }
    let raw = wrap(
        [
            op(1, &[0x800f_0000, 0x90e4_0001]),
            op(32, &[0x8002_0000, 0x9000_0001, 0xa000_000a]),
        ]
        .concat(),
    );
    let p = project_vertex_output(&raw, 3, 2).unwrap();
    let mut constants = [[0u32; 4]; 11];
    constants[10][0] = 3f32.to_bits();
    let inputs = [
        [0f32, 0., 0., 1.].map(f32::to_bits),
        [2f32, 0., 0., 0.].map(f32::to_bits),
        [0; 4],
    ];
    let got = ProgramShader::new(p)
        .unwrap()
        .execute(&inputs, &[(0, &constants)], &mut |_: Sample| {
            Err("no texture".into())
        })
        .unwrap();
    for (got, want) in got.outputs[0]
        .map(f32::from_bits)
        .into_iter()
        .zip([0f32, 8., 0., 1.])
    {
        assert!((got - want).abs() < 1e-5);
    }
}
