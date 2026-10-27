#![allow(clippy::unwrap_used)]
//! Hand-worked SM3 tokens and arithmetic. Expected numbers come from the SDK instruction definitions, not
//! a second bytecode decoder. The common executor only observes the lifted IR's answer.
use iridia::{ResourceKind, ShaderFormat};
use iridia_d3d9::{lift, read};
use iridia_exec::{PixelShader, ProgramShader, Sample};
use primase_core::gate::{GateReport, Tolerance};

fn program(body: &[u32]) -> Vec<u8> {
    [&[0xffff_0300][..], body, &[0xffff][..]]
        .concat()
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn instruction(op: u32, args: &[u32]) -> Vec<u32> {
    let mut words = vec![op | (args.len() as u32) << 24];
    words.extend(args);
    words
}
fn def(n: u32, values: [f32; 4]) -> Vec<u32> {
    instruction(
        81,
        &[vec![0xa00f_0000 | n], values.map(f32::to_bits).to_vec()].concat(),
    )
}
fn run(op: u32, values: &[[f32; 4]]) -> [f32; 4] {
    let mut body = Vec::new();
    for (n, v) in values.iter().enumerate() {
        body.extend(def(n as u32, *v));
    }
    let args: Vec<_> = std::iter::once(0x800f_0000)
        .chain((0..values.len()).map(|n| {
            (if matches!(op, 6 | 7 | 14 | 15 | 32) {
                0xa000_0000
            } else {
                0xa0e4_0000
            }) | n as u32
        }))
        .collect();
    body.extend(instruction(op, &args));
    body.extend(instruction(1, &[0x800f_0800, 0x80e4_0000]));
    let p = lift(&program(&body)).unwrap();
    p.validate().unwrap();
    let shader = ProgramShader::new(p).unwrap();
    shader
        .execute(&[], &[], &mut |_: Sample| Err("unexpected sample".into()))
        .unwrap()
        .outputs[0]
        .map(f32::from_bits)
}

#[test]
fn arithmetic_has_hand_worked_answers() {
    let a = [1., 2., 3., 4.];
    let b = [4., 3., 2., 1.];
    let one = [1.; 4];
    let cases = [
        (1, vec![a], a),
        (2, vec![a, b], [5.; 4]),
        (3, vec![a, b], [-3., -1., 1., 3.]),
        (4, vec![a, b, one], [5., 7., 7., 5.]),
        (5, vec![a, b], [4., 6., 6., 4.]),
        (6, vec![[2.; 4]], [0.5; 4]),
        (7, vec![[-4.; 4]], [0.5; 4]),
        (8, vec![a, b], [16.; 4]),
        (9, vec![a, b], [20.; 4]),
        (10, vec![a, b], [1., 2., 2., 1.]),
        (11, vec![a, b], [4., 3., 3., 4.]),
        (14, vec![[3.; 4]], [8.; 4]),
        (15, vec![[-8.; 4]], [3.; 4]),
        (18, vec![[0.25; 4], a, b], [3.25, 2.75, 2.25, 1.75]),
        (19, vec![[-1.25, 0., 2.5, 3.75]], [0.75, 0., 0.5, 0.75]),
        (32, vec![[-2.; 4], [3.; 4]], [8.; 4]),
        (35, vec![[-1., 2., -3., 4.]], a),
        (36, vec![[0., 3., 4., 10.]], [0., 0.6, 0.8, 2.]),
        (88, vec![[-1., 0., 1., -2.], a, b], [4., 2., 3., 1.]),
        (90, vec![a, b, [2.; 4]], [12.; 4]),
    ];
    let mut gate = GateReport::new("SM3 hand-worked arithmetic", Tolerance::Abs(2e-7));
    for (op, values, wanted) in cases {
        let actual = run(op, &values);
        gate.record(
            format!("opcode {op}: {actual:?} vs {wanted:?}"),
            actual
                .iter()
                .zip(wanted)
                .map(|(&a, b)| f64::from((a - b).abs()))
                .fold(0., f64::max),
        );
    }
    gate.assert_passed();
    assert_eq!(run(36, &[[0.; 4]]), [0.; 4]);
}

#[test]
fn masks_swizzles_sparse_inputs_and_local_def_overrides_are_exact() {
    // dcl_texcoord2 v3.yw; mov r0.xy,v3.wywy; mov r0.zw,c0; mov_sat oC0,-abs(r0.zyxw)
    let mut body = instruction(31, &[0x8002_0005, 0x900a_0003]);
    body.extend(instruction(1, &[0x8003_0000, 0x9077_0003]));
    body.extend(instruction(1, &[0x800c_0000, 0xa0e4_0000]));
    body.extend(instruction(1, &[0x800f_0801, 0x80c6_0000]));
    body.extend(instruction(1, &[0x801f_0800, 0x8cc6_0000]));
    let p = lift(&program(&body)).unwrap();
    assert_eq!(p.provenance.format, ShaderFormat::D3d9);
    assert_eq!(p.interface.inputs[0].label.index, 2);
    assert_eq!(p.interface.inputs[0].provenance.d3d.unwrap().mask, 10);
    let shader = ProgramShader::new(p).unwrap();
    let inputs = vec![
        [0; 4],
        [0; 4],
        [0; 4],
        [0f32.to_bits(), 0.25f32.to_bits(), 0, 0.75f32.to_bits()],
    ];
    let constants = [[0., 0., 2., -0.5].map(f32::to_bits)];
    let got = shader
        .execute(&inputs, &[(0, &constants)], &mut |_: Sample| {
            Err("no sample".into())
        })
        .unwrap();
    assert_eq!(got.outputs[0].map(f32::from_bits), [0.; 4]);
    assert_eq!(got.outputs[1].map(f32::from_bits), [2., 0.25, 0.75, -0.5]);
    // def c0 wins over external constants; .w scalar expands to all output lanes.
    let body = [
        def(0, [1., 2., 3., 4.]),
        instruction(1, &[0x800f_0800, 0xa0ff_0000]),
    ]
    .concat();
    let p = lift(&program(&body)).unwrap();
    assert!(p.interface.resources.is_empty());
    let got = ProgramShader::new(p)
        .unwrap()
        .execute(&[], &[], &mut |_: Sample| Err("no sample".into()))
        .unwrap();
    assert_eq!(got.outputs[0].map(f32::from_bits), [4.; 4]);
}

#[test]
fn sampling_projection_bias_and_discard_keep_their_meaning() {
    for mode in 0..3 {
        let body = [
            def(0, [0.25, 0.5, 0., 2.]),
            instruction(31, &[0x9000_0000, 0xa00f_0802]),
            instruction(66 | mode << 16, &[0x800f_0800, 0xa0e4_0000, 0xa0e4_0802]),
        ]
        .concat();
        let p = lift(&program(&body)).unwrap();
        let shader = ProgramShader::new(p).unwrap();
        let mut calls = 0;
        let got = shader
            .execute(&[], &[], &mut |s: Sample| {
                calls += 1;
                assert_eq!((s.texture, s.sampler), (2, 2));
                assert_eq!(s.uv, if mode == 1 { [0.125, 0.25] } else { [0.25, 0.5] });
                assert_eq!(
                    s.lod,
                    if mode == 2 {
                        iridia_exec::Lod::Bias(2.)
                    } else {
                        iridia_exec::Lod::Implicit
                    }
                );
                Ok([0.2, 0.4, 0.6, 0.8])
            })
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(got.outputs[0].map(f32::from_bits), [0.2, 0.4, 0.6, 0.8]);
    }
    for w in [-1., 1.] {
        let body = [
            def(0, [1., 1., 1., w]),
            instruction(1, &[0x800f_0000, 0xa0e4_0000]),
            instruction(65, &[0x800f_0000]),
            instruction(1, &[0x800f_0800, 0xa0e4_0000]),
        ]
        .concat();
        let p = lift(&program(&body)).unwrap();
        let got = ProgramShader::new(p)
            .unwrap()
            .execute(&[], &[], &mut |_: Sample| Err("no sample".into()))
            .unwrap();
        assert_eq!(got.discarded, w < 0.);
    }
    // Cube and volume declarations retain their dimensionality and lift valid modules.
    for (declaration, dim) in [
        (0x9800_0000, iridia::TextureDim::Cube),
        (0xa000_0000, iridia::TextureDim::D3),
    ] {
        let body = [
            def(0, [0.5; 4]),
            instruction(31, &[declaration, 0xa00f_0800]),
            instruction(66, &[0x800f_0800, 0xa0e4_0000, 0xa0e4_0800]),
        ]
        .concat();
        let p = lift(&program(&body)).unwrap();
        assert!(matches!(p.interface.resources[0].kind,ResourceKind::Texture{dim:d,..} if d==dim));
        iridia::emit::wgsl(&p).unwrap();
    }
}

/// A hand-laid CTAB: header 0..28, constant info 28..48, float4 type 48..64,
/// defaults 64..80, creator/name/target at 80/84/89. One c0, float4 Tint = (1,2,3,4).
fn ctab() -> Vec<u32> {
    let mut b: Vec<u8> = [28u32, 80, 0xffff_0300, 1, 28, 0, 89, 84, 2, 1, 48, 64]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    b.extend([1u16, 3, 1, 4, 1, 0].into_iter().flat_map(u16::to_le_bytes));
    b.extend(0u32.to_le_bytes());
    b.extend(
        [1f32, 2., 3., 4.]
            .into_iter()
            .flat_map(|f| f.to_bits().to_le_bytes()),
    );
    b.extend(b"fxc\0Tint\0ps_3_0\0");
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let mut words = vec![u32::from_le_bytes(*b"CTAB")];
    words.extend(b.as_chunks::<4>().0.iter().map(|s| u32::from_le_bytes(*s)));
    [vec![0xfffe | (words.len() as u32) << 16], words].concat()
}

#[test]
fn constant_table_keeps_names_types_defaults_and_unknown_comments() {
    let body = [
        vec![0x0002_fffe, 0x12345678, 0xabcdef01],
        ctab(),
        instruction(1, &[0x800f_0800, 0xa0e4_0000]),
    ]
    .concat();
    let bytes = program(&body);
    let shader = read(&bytes).unwrap();
    assert_eq!(shader.comments[0].data, [0x12345678, 0xabcdef01]);
    let table = shader.constants.unwrap();
    assert_eq!(table.creator, "fxc");
    assert_eq!(table.target, "ps_3_0");
    let c = &table.constants[0];
    assert_eq!((&*c.name, c.register, c.count), ("Tint", 0, 1));
    assert_eq!(
        (c.ty.class, c.ty.kind, c.ty.rows, c.ty.columns, c.ty.elements),
        (1, 3, 1, 4, 1)
    );
    assert_eq!(
        c.default,
        Some(
            [1f32, 2., 3., 4.]
                .into_iter()
                .flat_map(|f| f.to_bits().to_le_bytes())
                .collect()
        )
    );
    let p = lift(&bytes).unwrap();
    let ResourceKind::ConstantBuffer { layout } = &p.interface.resources[0].kind else {
        panic!()
    };
    assert_eq!(layout.size, 16);
    assert_eq!(layout.constants[0].name, "Tint");
}

#[test]
fn malformed_and_unsupported_programs_are_refused_without_panics() {
    let cases = [
        (instruction(40, &[0xa0e4_0000]), "opcode 40"),
        (instruction(1, &[0x800f_0800, 0x80e4_0000]), "uninitialized"),
        (instruction(1, &[0x800f_0800, 0xa0e4_2000]), "relative"),
        (
            instruction(1 | 1 << 28, &[0x800f_0800, 0xa0e4_0000]),
            "predication",
        ),
        (instruction(1, &[0x800f_0800, 0xa2e4_0000]), "modifier"),
        (
            instruction(66, &[0x800f_0800, 0xa0e4_0000, 0xa0e4_0800]),
            "undeclared sampler",
        ),
        (vec![0x0300_0001, 0x800f_0800], "extends past"),
    ];
    for (body, wanted) in cases {
        let err = lift(&program(&body)).unwrap_err().to_string();
        assert!(err.contains(wanted), "{err}");
    }
    let bytes = program(
        &[
            ctab(),
            def(1, [2.; 4]),
            instruction(1, &[0x800f_0800, 0xa0e4_0001]),
        ]
        .concat(),
    );
    for n in 0..bytes.len() {
        let _ = lift(&bytes[..n]);
    }
    for n in 0..bytes.len() {
        for bit in [1, 8, 128] {
            let mut changed = bytes.clone();
            changed[n] ^= bit;
            let _ = lift(&changed);
        }
    }
    assert_eq!(
        read(&[0x00, 0x03, 0xfe, 0xff, 0xff, 0xff, 0, 0]).unwrap().stage,
        iridia::Stage::Vertex
    );
    let mut trailing = bytes;
    trailing.extend([0; 4]);
    assert!(read(&trailing).unwrap_err().detail.contains("after END"));
}

#[test]
fn lifted_fixture_changes_require_a_version_bump() {
    let fixtures = [
        (
            "ctab",
            [ctab(), instruction(1, &[0x800f_0800, 0xa0e4_0000])].concat(),
        ),
        (
            "normalize",
            [
                def(0, [0., 3., 4., 10.]),
                instruction(36, &[0x800f_0000, 0xa0e4_0000]),
                instruction(1, &[0x800f_0800, 0x80e4_0000]),
            ]
            .concat(),
        ),
        (
            "texture",
            [
                def(0, [0.5; 4]),
                instruction(31, &[0x9000_0000, 0xa00f_0800]),
                instruction(66, &[0x800f_0800, 0xa0e4_0000, 0xa0e4_0800]),
            ]
            .concat(),
        ),
        ("relative", instruction(1, &[0x800f_0800, 0xa0e4_2000])),
    ];
    let outcomes = fixtures
        .into_iter()
        .map(|(name, body)| {
            let outcome = lift(&program(&body))
                .map_err(|e| e.to_string())
                .and_then(|p| p.text().map_err(|e| e.to_string()));
            (name.to_owned(), outcome)
        })
        .collect::<Vec<_>>();
    let table = iridia::gate::table("primase-d3d9", iridia_d3d9::LIFTER_VERSION, &outcomes);
    iridia::gate::check(include_str!("versions.tsv"), &table).unwrap_or_else(|e| panic!("{e}\n{table}"));
}
