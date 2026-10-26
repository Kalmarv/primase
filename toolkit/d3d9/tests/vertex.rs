#![allow(clippy::unwrap_used)]
//! Hand-assembled SM3 tokens: bank edges, packed output semantics and independent arithmetic answers.
use primase_d3d9::{lift, read};
use primase_shader::{Stage, SystemValue};
use primase_shader_exec::{ProgramShader, Sample};

fn instruction(op: u32, operands: &[u32]) -> Vec<u32> {
    [vec![op | (operands.len() as u32) << 24], operands.to_vec()].concat()
}
fn program(body: &[u32]) -> Vec<u8> {
    [vec![0xfffe_0300], body.to_vec(), vec![0x0000_ffff]]
        .concat()
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn dcl(usage: u32, index: u32, dst: u32) -> Vec<u32> {
    instruction(31, &[0x8000_0000 | index << 16 | usage, dst])
}
fn position() -> Vec<u32> {
    [dcl(0, 0, 0x900f_000f), dcl(0, 0, 0xe00f_0007)].concat()
}
fn fixture() -> Vec<u8> {
    program(
        &[
            position(),
            dcl(10, 4, 0xe001_0003), // COLOR4 -> o3.x
            dcl(5, 3, 0xe006_0003),  // TEXCOORD3 -> o3.yz
            dcl(11, 0, 0xe008_0003), // FOG -> o3.w
            instruction(
                81,
                &[
                    0xa00f_00fe,
                    2f32.to_bits(),
                    3f32.to_bits(),
                    4f32.to_bits(),
                    5f32.to_bits(),
                ],
            ),
            instruction(1, &[0xe00f_0007, 0x90e4_000f]), // position = v15
            instruction(4, &[0x800f_0000, 0x90e4_000f, 0xa0e4_00fe, 0xa0e4_00ff]),
            instruction(1, &[0xe00f_0003, 0x80e4_0000]), // o3 = v15 * def c254 + external c255
        ]
        .concat(),
    )
}

#[test]
fn vertex_banks_clip_position_and_packed_semantics_keep_their_values() {
    let bytes = fixture();
    let source = read(&bytes).unwrap();
    assert_eq!(source.stage, Stage::Vertex);
    assert_eq!(source.outputs.len(), 4);
    let p = lift(&bytes).unwrap();
    assert_eq!(p.stage, Stage::Vertex);
    assert_eq!(p.provenance.target, "vs_3_0");
    assert_eq!(
        p.provenance.source_hash,
        Some(primase_shader::source_hash(&bytes))
    );
    assert!(p.interface.inputs.iter().all(|v| v.interp.is_none()));
    let outs = &p.interface.outputs;
    assert_eq!(outs[0].system, Some(SystemValue::ClipPosition));
    assert_eq!(
        outs.iter()
            .skip(1)
            .map(|v| (&*v.label.name, v.label.index, v.provenance.d3d.unwrap().mask))
            .collect::<Vec<_>>(),
        [("COLOR", 4, 1), ("TEXCOORD", 3, 6), ("FOG", 0, 8)]
    );
    primase_shader::emit::wgsl(&p).unwrap();
    let mut input = [[0u32; 4]; 16];
    input[15] = [1f32, 2., 3., 1.].map(f32::to_bits);
    let mut constants = [[0u32; 4]; 256];
    constants[255] = [10f32, 20., 30., 40.].map(f32::to_bits);
    let (got, clip) = ProgramShader::new(p)
        .unwrap()
        .execute_vertex(&input, &[(0, &constants)], &mut |_: Sample| {
            Err("no texture".into())
        })
        .unwrap();
    assert_eq!(clip, [1f32, 2., 3., 1.].map(f32::to_bits));
    assert_eq!(got.outputs[3].map(f32::from_bits), [12., 26., 42., 45.]);
}

#[test]
fn vertex_refuses_undeclared_unwritten_and_unsupported_features() {
    let good_write = instruction(1, &[0xe00f_0007, 0x90e4_000f]);
    let cases = [
        (
            [
                position(),
                instruction(88, &[0x800f_0000, 0x90e4_000f, 0x90e4_000f, 0x90e4_000f]),
            ]
            .concat(),
            "pixel-only SM3 opcode",
        ),
        (
            [
                position(),
                instruction(90, &[0x800f_0000, 0x90e4_000f, 0x90e4_000f, 0x90e4_000f]),
            ]
            .concat(),
            "pixel-only SM3 opcode",
        ),
        (vec![], "no declared POSITION0"),
        (position(), "uninitialized output"),
        (
            [position(), instruction(1, &[0xe003_0007, 0x90e4_000f])].concat(),
            "uninitialized output lanes",
        ),
        (
            [position(), instruction(1, &[0xe00f_0006, 0x90e4_000f])].concat(),
            "undeclared vertex output",
        ),
        (
            [position(), instruction(1, &[0xe00f_0007, 0xa0e4_0100])].concat(),
            "outside the supported",
        ),
        (
            [position(), instruction(1, &[0xe00f_0007, 0x90e4_0010])].concat(),
            "outside the supported",
        ),
        (
            [position(), instruction(1, &[0xe00f_000c, 0x90e4_000f])].concat(),
            "outside the supported",
        ),
        (
            [position(), instruction(1, &[0x800f_0800, 0x90e4_000f])].concat(),
            "outside the supported",
        ),
        (
            [position(), dcl(5, 0, 0xe003_0007)].concat(),
            "overlapping output",
        ),
        (
            [position(), dcl(0, 0, 0xe00f_0006)].concat(),
            "duplicate semantic",
        ),
        (dcl(0, 0, 0xe003_0007), "all four lanes"),
        ([position(), dcl(4, 0, 0xe00f_0006)].concat(), "PSIZE"),
        (
            [position(), instruction(65, &[0x800f_0000])].concat(),
            "pixel sampling/discard",
        ),
        (
            [
                position(),
                instruction(66, &[0x800f_0000, 0x90e4_000f, 0xa0e4_0800]),
            ]
            .concat(),
            "pixel sampling/discard",
        ),
        (
            [position(), instruction(46, &[0xb00f_0000, 0x90e4_000f])].concat(),
            "MOVA",
        ),
        (
            [position(), instruction(1, &[0xe00f_0007, 0xa0e4_2000])].concat(),
            "relative addressing",
        ),
        (
            [position(), instruction(1, &[0xe02f_0007, 0x90e4_000f])].concat(),
            "partial precision",
        ),
        (
            [position(), good_write.clone(), dcl(5, 0, 0xe00f_0006)].concat(),
            "DCL after executable",
        ),
        (
            [position(), instruction(1, &[0xe00f_0007, 0xe0e4_0007])].concat(),
            "not a readable float register",
        ),
    ];
    for (body, expected) in cases {
        let e = lift(&program(&body)).unwrap_err().to_string();
        assert!(e.contains(expected), "expected {expected}, got {e}");
    }
    let bytes = fixture();
    for length in 0..bytes.len() {
        let _ = lift(&bytes[..length]);
    }
    for at in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[at] ^= 0x80;
        let _ = lift(&changed);
    }
    let cut = primase_d3d9::Projection {
        after_word: 0,
        temporary: 0,
        lanes: [0, 1, 2],
    };
    assert!(
        primase_d3d9::project(&bytes, cut)
            .unwrap_err()
            .detail
            .contains("pixel-only")
    );
    assert!(
        primase_d3d9::reencode(&bytes, cut)
            .unwrap_err()
            .detail
            .contains("pixel-only")
    );
}

#[test]
fn vertex_fixture_has_a_versioned_ir_baseline() {
    let outcome = lift(&fixture()).unwrap().text().map_err(|e| e.to_string());
    let table = primase_shader::gate::table(
        "primase-d3d9 vertex",
        primase_d3d9::LIFTER_VERSION,
        &[("packed".into(), outcome)],
    );
    primase_shader::gate::check(include_str!("vertex-versions.tsv"), &table)
        .unwrap_or_else(|e| panic!("{e}\n{table}"));
}

/// A hand-laid CTAB with one float4 at c255; the stage controls its legal register range.
fn ctab() -> Vec<u32> {
    let mut bytes: Vec<u8> = [28u32, 80, 0xfffe_0300, 1, 28, 0, 89, 84, 0x00ff_0002, 1, 48, 64]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    bytes.extend([1u16, 3, 1, 4, 1, 0].into_iter().flat_map(u16::to_le_bytes));
    bytes.extend(0u32.to_le_bytes());
    bytes.extend([1f32, 2., 3., 4.].into_iter().flat_map(f32::to_le_bytes));
    bytes.extend(b"fxc\0Tint\0vs_3_0\0");
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let words: Vec<_> = [
        vec![u32::from_le_bytes(*b"CTAB")],
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect(),
    ]
    .concat();
    [vec![65534 | (words.len() as u32) << 16], words].concat()
}

#[test]
fn constant_table_checks_stage_and_vertex_allocation_limits() {
    let bytes = program(
        &[
            ctab(),
            dcl(0, 0, 0xe00f_0007),
            instruction(1, &[0xe00f_0007, 0xa0e4_00ff]),
        ]
        .concat(),
    );
    let p = lift(&bytes).unwrap();
    let primase_shader::ResourceKind::ConstantBuffer { layout } = &p.interface.resources[0].kind else {
        panic!()
    };
    assert_eq!(layout.size, 4096);
    assert_eq!(layout.constants[0].offset, 4080);
    assert_eq!(
        layout.constants[0].default.as_deref(),
        Some(
            [1f32, 2., 3., 4.]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>()
                .as_slice()
        )
    );
    let mut crossed = bytes.clone();
    crossed[20..24].copy_from_slice(&0xffff_0300u32.to_le_bytes());
    assert!(
        read(&crossed)
            .unwrap_err()
            .detail
            .contains("CTAB version disagrees")
    );
    crossed[..4].copy_from_slice(&0xffff_0300u32.to_le_bytes());
    assert!(read(&crossed).unwrap_err().detail.contains("out-of-bank"));
}
