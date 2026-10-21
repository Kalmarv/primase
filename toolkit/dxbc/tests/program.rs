#![allow(clippy::unwrap_used)]
use primase_dxbc::{Dimension, Program, ResourceDeclaration, ReturnType, Sample};

fn instruction(op: u32, operands: &[u32]) -> Vec<u32> {
    let mut v = vec![op | (((operands.len() + 1) as u32) << 24)];
    v.extend_from_slice(operands);
    v
}
fn dest(kind: u32, index: u32) -> [u32; 2] {
    [2 | 15 << 4 | kind << 12 | 1 << 20, index]
}
fn lit(v: [u32; 4]) -> Vec<u32> {
    let mut r = vec![2 | 4 << 12];
    r.extend(v);
    r
}
fn program(op: u32, inputs: &[[u32; 4]]) -> Vec<u32> {
    let mut v = vec![0x50, 0];
    v.extend(instruction(101, &dest(2, 0)));
    let mut operands = dest(2, 0).to_vec();
    for &i in inputs {
        operands.extend(lit(i));
    }
    v.extend(instruction(op, &operands));
    v.extend(instruction(62, &[]));
    v[1] = v.len() as u32;
    v
}
fn run(op: u32, inputs: &[[u32; 4]]) -> [u32; 4] {
    Program::from_words(&program(op, inputs))
        .unwrap()
        .execute(&[], &[], &mut |_: Sample| -> Result<[f32; 4], String> {
            panic!("unexpected sample")
        })
        .unwrap()
        .outputs[0]
}
#[test]
fn arithmetic_and_conversion_edges() {
    let f = |x: [f32; 4]| x.map(f32::to_bits);
    assert_eq!(
        run(0, &[f([1., -2., 3., 4.]), f([2., 3., -3., 0.])]),
        f([3., 1., 0., 4.])
    );
    assert_eq!(
        run(28, &[f([-1., f32::NAN, f32::INFINITY, 3.9])]),
        [0, 0, u32::MAX, 3]
    );
    assert_eq!(
        run(86, &[[16_777_217, 16_777_219, 0, u32::MAX]]),
        f([16_777_216., 16_777_220., 0., 4294967296.])
    );
    assert_eq!(run(64, &[f([0.5, 1.5, 2.5, -1.5])]), f([0., 2., 2., -2.]));
    assert_eq!(run(35, &[[u32::MAX; 4], [2; 4], [3; 4]]), [1; 4]);
    assert_eq!(
        run(54, &[[1, 0x80000001, 0x7fc12345, 0xffffffff]]),
        [1, 0x80000001, 0x7fc12345, 0xffffffff]
    );
    assert_eq!(
        run(56, &[[1, 0x80000001, 0, 0x80000000], f([1.; 4])]),
        [0, 0x80000000, 0, 0x80000000]
    );
}
#[test]
fn reject_before_execution() {
    for (op, name) in [(76, "switch"), (109, "gather4"), (134, "countbits")] {
        let words = program(op, &[]);
        assert!(
            Program::from_words(&words)
                .unwrap_err()
                .to_string()
                .contains(name)
        );
    }
    let words = program(54, &[[1; 4]]);
    for n in 0..words.len() {
        assert!(Program::from_words(&words[..n]).is_err());
    }
}

#[test]
fn container_bounds_and_duplicate_chunks() {
    let code = program(54, &[[1; 4]]);
    let mut bytes = b"DXBC".to_vec();
    bytes.extend([0; 16]);
    let total = 44 + code.len() * 4;
    for w in [1, total as u32, 1, 36] {
        bytes.extend(w.to_le_bytes());
    }
    bytes.extend(b"SHEX");
    bytes.extend((code.len() as u32 * 4).to_le_bytes());
    for w in code {
        bytes.extend(w.to_le_bytes());
    }
    assert!(Program::parse(&bytes).is_ok());
    for n in 0..bytes.len() {
        assert!(Program::parse(&bytes[..n]).is_err());
    }
    let mut duplicate = bytes.clone();
    duplicate.splice(36..36, 40u32.to_le_bytes());
    duplicate[24..28].copy_from_slice(&((total + 4) as u32).to_le_bytes());
    duplicate[28..32].copy_from_slice(&2u32.to_le_bytes());
    duplicate[32..36].copy_from_slice(&40u32.to_le_bytes());
    assert!(
        Program::parse(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("overlapping")
    );
}

#[test]
fn relative_constant_buffer_and_aliasing() {
    let mut words = vec![0x50, 0];
    words.extend(instruction(104, &[1]));
    words.extend(instruction(89 | 0x800, &[2 | 15 << 4 | 8 << 12 | 2 << 20, 0, 2]));
    words.extend(instruction(101, &dest(2, 0)));
    let mut init = dest(0, 0).to_vec();
    init.extend(lit([1, 0, 0, 0]));
    words.extend(instruction(54, &init));
    let mut load = dest(0, 0).to_vec();
    // cb0[r0.x + 0], identity swizzle. The destination aliases the relative index.
    load.extend([
        2 | 1 << 2 | 0xe4 << 4 | 8 << 12 | 2 << 20 | 3 << 25,
        0,
        0,
        2 | 2 << 2 | 1 << 20,
        0,
    ]);
    words.extend(instruction(54, &load));
    let mut output = dest(2, 0).to_vec();
    output.extend([2 | 1 << 2 | 0xe4 << 4 | 1 << 20, 0]);
    words.extend(instruction(54, &output));
    words.extend(instruction(62, &[]));
    words[1] = words.len() as u32;
    let p = Program::from_words(&words).unwrap();
    let values = [[0; 4], [11, 22, 33, 44]];
    let out = p
        .execute(
            &[],
            &[(0, &values)],
            &mut |_: Sample| -> Result<[f32; 4], String> { panic!() },
        )
        .unwrap();
    assert_eq!(out.outputs[0], values[1]);
    assert!(p.execute(&[], &[], &mut |_: Sample| Ok([0.; 4])).is_err());
}

#[test]
fn every_numeric_opcode_has_an_analytic_control() {
    let f = |x: [f32; 4]| x.map(f32::to_bits);
    let a = f([1., 2., 3., 4.]);
    let b = f([4., 3., 2., 1.]);
    assert_eq!(run(1, &[[0xf0; 4], [0x55; 4]]), [0x50; 4]);
    assert_eq!(run(14, &[a, f([2.; 4])]), f([0.5, 1., 1.5, 2.]));
    for (op, value) in [(15, 10.), (16, 16.), (17, 20.)] {
        assert_eq!(run(op, &[a, b]), f([value; 4]));
    }
    assert_eq!(run(25, &[f([-1., 0., 1., 2.])]), f([0.5, 1., 2., 4.]));
    assert_eq!(run(47, &[f([0.5, 1., 2., 4.])]), f([-1., 0., 1., 2.]));
    assert_eq!(run(50, &[a, b, f([1.; 4])]), f([5., 7., 7., 5.]));
    assert_eq!(run(51, &[a, b]), f([1., 2., 2., 1.]));
    assert_eq!(
        run(55, &[[0, 1, 0x80000000, u32::MAX], a, b]),
        f([4., 2., 3., 4.])
    );
    assert_eq!(run(56, &[a, b]), f([4., 6., 6., 4.]));
    assert_eq!(run(57, &[a, b]), [u32::MAX; 4]);
    assert_eq!(run(68, &[f([0.25, 1., 4., 16.])]), f([2., 1., 0.5, 0.25]));
    assert_eq!(run(75, &[f([0.25, 1., 4., 16.])]), f([0.5, 1., 2., 4.]));
    assert_eq!(
        run(54 | 0x2000, &[f([-1., 0.5, 2., f32::NAN])]),
        f([0., 0.5, 1., 0.])
    );
    // Separate multiply/add rounds (1+2^-23)*(1-2^-23) to 1, then subtracts 1.
    assert_eq!(
        run(
            50,
            &[
                f([f32::from_bits(0x3f800001); 4]),
                f([f32::from_bits(0x3f7ffffe); 4]),
                f([-1.; 4])
            ]
        ),
        f([0.; 4])
    );
    let r = run(68, &[f([-1., -0., 0., f32::INFINITY])]).map(f32::from_bits);
    assert!(r[0].is_nan());
    assert_eq!(&r[1..], &[f32::NEG_INFINITY, f32::INFINITY, 0.]);
    let r = run(47, &[f([-1., -0., f32::from_bits(1), f32::INFINITY])]).map(f32::from_bits);
    assert!(r[0].is_nan());
    assert_eq!(&r[1..], &[f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY]);
}

#[test]
fn traced_precision_causes_stay_explicit() {
    let x = 0x3f91d6c7;
    assert_eq!(run(68, &[[x; 4]]), [0x3f6fd523; 4]);
    let serial = 1.0f32 / f32::from_bits(x).sqrt();
    assert_eq!(serial.to_bits(), 0x3f6fd524);
    let value = 1056964608.0f32.to_bits();
    let binary = run(56, &[[value; 4], [0x3b808081; 4]]);
    let decimal = run(56, &[[value; 4], [0.003922f32.to_bits(); 4]]);
    assert_eq!(binary, [0x4a7cfcfe; 4]);
    assert_eq!(decimal, [0x4a7d041d; 4]);
}

#[test]
fn signed_multiply_writes_high_and_low_and_null() {
    let mut words = vec![0x50, 0];
    words.extend(instruction(101, &dest(2, 0)));
    words.extend(instruction(101, &dest(2, 1)));
    let mut operands = dest(2, 0).to_vec();
    operands.extend(dest(2, 1));
    operands.extend(lit([u32::MAX, i32::MIN as u32, i32::MAX as u32, 0]));
    operands.extend(lit([2, 2, i32::MAX as u32, 9]));
    words.extend(instruction(38, &operands));
    words.extend(instruction(62, &[]));
    words[1] = words.len() as u32;
    let p = Program::from_words(&words).unwrap();
    let r = p
        .execute(&[], &[], &mut |_: Sample| -> Result<[f32; 4], String> {
            panic!()
        })
        .unwrap();
    assert_eq!(r.outputs[0], [u32::MAX, u32::MAX, 0x3fffffff, 0]);
    assert_eq!(r.outputs[1], [0xfffffffe, 0, 1, 0]);
}

#[test]
fn sample_callback_uses_coordinates_and_return_swizzle() {
    let mut words = vec![0x50, 0];
    words.extend(instruction(
        88 | 3 << 11,
        &[2 | 15 << 4 | 7 << 12 | 1 << 20, 2, 0x5555],
    ));
    words.extend(instruction(90, &[6 << 12 | 1 << 20, 3]));
    words.extend(instruction(101, &dest(2, 0)));
    let mut operands = dest(2, 0).to_vec();
    operands.extend(lit([0.25f32.to_bits(), 0.75f32.to_bits(), 0, 0]));
    // resource wzxy swizzle
    operands.extend([
        2 | 1 << 2 | 0x4b << 4 | 7 << 12 | 1 << 20,
        2,
        6 << 12 | 1 << 20,
        3,
    ]);
    words.extend(instruction(69, &operands));
    words.extend(instruction(62, &[]));
    words[1] = words.len() as u32;
    let mut calls = 0;
    let r = Program::from_words(&words)
        .unwrap()
        .execute(&[], &[], &mut |s: Sample| {
            calls += 1;
            assert_eq!((s.texture, s.sampler, s.uv), (2, 3, [0.25, 0.75]));
            Ok([1., 2., 3., 4.])
        })
        .unwrap();
    assert_eq!(r.outputs[0].map(f32::from_bits), [4., 3., 1., 2.]);
    assert_eq!((calls, r.samples, r.instructions), (1, 1, 2));
}

/// Optional directory of independent ps_4_0, ps_4_1 and ps_5_0 binaries and matching disassembler
/// text (.asm appended). Counts executable instructions including final ret.
#[test]
fn independent_disassembly_counts() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::real_data_value("PRIMASE_DXBC_SHADERS") else {
        eprintln!("SKIPPED: PRIMASE_DXBC_SHADERS not set");
        return Ok(());
    };
    let mut count = 0;
    let (mut exact, mut within, mut nan) = (0, 0, 0);
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_none_or(|x| x != "ppo") {
            continue;
        }
        let asm_path = path.with_extension("ppo.asm");
        if !asm_path.exists() {
            continue;
        }
        let asm = std::fs::read_to_string(asm_path)?;
        let p = Program::parse(&std::fs::read(&path)?).map_err(|e| format!("{}: {e}", path.display()))?;
        let instructions: Vec<_> = asm
            .lines()
            .skip_while(|l| !matches!(l.trim(), "ps_4_0" | "ps_4_1" | "ps_5_0"))
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("dcl_"))
            .collect();
        assert_eq!(p.instructions().len(), instructions.len(), "{}", path.display());
        for (a, b) in p.instructions().iter().zip(instructions) {
            let name = b.split_whitespace().next().unwrap();
            let name = if name.starts_with("sample_indexable") {
                "sample"
            } else {
                name.strip_suffix("_sat").unwrap_or(name)
            };
            // a conditional's test is its control, not its name
            let name = match name {
                "if_nz" | "if_z" => "if",
                "breakc_nz" | "breakc_z" => "breakc",
                "continuec_nz" | "continuec_z" => "continuec",
                _ => name,
            };
            assert_eq!(a.name(), name, "{} word {}", path.display(), a.word);
        }
        let state_path = path.with_extension("ppo.state.json");
        if state_path.exists() {
            let j: serde_json::Value = serde_json::from_slice(&std::fs::read(state_path)?)?;
            let vectors = |j: &serde_json::Value| -> Vec<[u32; 4]> {
                j.as_array()
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|v| std::array::from_fn(|i| v[i].as_u64().unwrap() as u32))
                    .collect()
            };
            let inputs = vectors(&j["inputs"]);
            let buffers: Vec<_> = j["buffers"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, j)| !j.is_null())
                .map(|(i, j)| (i as u32, vectors(j)))
                .collect();
            let refs: Vec<_> = buffers.iter().map(|(i, b)| (*i, b.as_slice())).collect();
            // The optional `.ppo.trace.json` oracle diverges on documented cases (README, "Traced value-oracle
            // differences"), so trace equality is not a gate; the instruction stream and final outputs are.
            let result = p
                .execute(&inputs, &refs, &mut |_: Sample| Ok([0.25, 0.5, 0.75, 1.0]))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let expected = vectors(&j["outputs"]);
            for (i, a) in result.outputs.iter().enumerate() {
                for c in 0..4 {
                    if result.output_masks[i] & (1 << c) == 0 {
                        continue;
                    }
                    let (x, y) = (f32::from_bits(a[c]), f32::from_bits(expected[i][c]));
                    if a[c] == expected[i][c] {
                        exact += 1;
                    } else if x.is_nan() && y.is_nan() {
                        nan += 1;
                    } else {
                        assert!(
                            (x - y).abs() <= 4.0 * f32::EPSILON * y.abs().max(1.0),
                            "{} o{i}.{c}: {x} != {y}",
                            path.display()
                        );
                        within += 1;
                    }
                }
            }
        }
        count += 1;
    }
    assert!(count > 0, "no paired shaders");
    println!("independent bytecode/disassembly instruction streams matched: {count}");
    println!(
        "constant-sampler value oracle: {exact} exact, {within} within 4 f32 epsilon at max(1, abs(reference)), {nan} NaN pairs"
    );
    Ok(())
}

#[test]
fn opcodes_outside_the_subset_are_refused_by_name() {
    // Switches, umul and countbits are refused when the program is read, never evaluated approximately.
    for (op, name) in [(76, "switch"), (81, "umul"), (134, "countbits")] {
        let mut words = vec![0x50, 0];
        words.extend(instruction(op, &[]));
        words.extend(instruction(62, &[]));
        words[1] = words.len() as u32;
        let error = Program::from_words(&words).unwrap_err().to_string();
        assert!(error.contains(&format!("unsupported {name} ({op})")), "{error}");
    }
}

fn temp(index: u32) -> [u32; 2] {
    dest(0, index)
}
/// A temp's component `c` as a scalar source.
fn scalar(kind: u32, index: u32, c: u32) -> [u32; 2] {
    [2 | 2 << 2 | c << 4 | kind << 12 | 1 << 20, index]
}
/// A register as an xyzw source.
fn whole(kind: u32, index: u32) -> [u32; 2] {
    [2 | 1 << 2 | 0xe4 << 4 | kind << 12 | 1 << 20, index]
}
fn op(code: u32, parts: &[&[u32]]) -> Vec<u32> {
    instruction(code, &parts.concat())
}
/// A ps_5_0 program of `temps` temporaries and `inputs` inputs writing o0 and o1, around `body`.
fn shader(temps: u32, inputs: u32, body: &[Vec<u32>]) -> Vec<u32> {
    let mut v = vec![0x50, 0];
    v.extend(instruction(104, &[temps]));
    for i in 0..inputs {
        v.extend(instruction(98 | 2 << 11, &dest(1, i)));
    }
    v.extend(instruction(101, &dest(2, 0)));
    v.extend(instruction(101, &dest(2, 1)));
    for b in body {
        v.extend(b);
    }
    v.extend(instruction(62, &[]));
    v[1] = v.len() as u32;
    v
}
fn no_sample(_: Sample) -> Result<[f32; 4], String> {
    panic!("unexpected sample")
}

#[test]
fn every_new_alu_opcode_has_an_analytic_control() {
    let f = |x: [f32; 4]| x.map(f32::to_bits);
    let i = |x: [i32; 4]| x.map(|v| v as u32);
    const T: u32 = u32::MAX;
    let (a, b) = (f([1., 2., -3., f32::NAN]), f([1., 3., -4., 1.]));
    assert_eq!(run(24, &[a, b]), [T, 0, 0, 0]);
    assert_eq!(run(29, &[a, b]), [T, 0, T, 0]);
    assert_eq!(run(49, &[a, b]), [0, T, 0, 0]);
    assert_eq!(run(26, &[f([1.25, -1.25, 3., -0.5])]), f([0.25, 0.75, 0., 0.5]));
    assert_eq!(
        run(27, &[f([-1.9, 2.9, f32::NAN, 3e10])]),
        i([-1, 2, 0, i32::MAX])
    );
    assert_eq!(run(30, &[[u32::MAX, 1, 2, 3], [1, 1, 1, 1]]), [0, 2, 3, 4]);
    let (x, y) = (i([-1, 2, 3, 4]), i([1, 2, -5, 4]));
    assert_eq!(run(32, &[x, y]), [0, T, 0, T]);
    assert_eq!(run(33, &[x, y]), [0, T, T, T]);
    assert_eq!(run(34, &[x, y]), [T, 0, 0, 0]);
    assert_eq!(run(36, &[x, y]), i([1, 2, 3, 4]));
    assert_eq!(run(37, &[x, y]), i([-1, 2, -5, 4]));
    assert_eq!(run(39, &[x, y]), [T, 0, T, 0]);
    assert_eq!(run(40, &[i([1, -2, i32::MIN, 0])]), i([-1, 2, i32::MIN, 0]));
    assert_eq!(run(41, &[[1, 1, 1, 3], [0, 4, 33, 31]]), [1, 16, 2, 0x8000_0000]);
    assert_eq!(run(42, &[i([-16, 16, -1, 8]), [2, 2, 31, 35]]), i([-4, 4, -1, 1]));
    assert_eq!(
        run(43, &[i([-3, 0, 16_777_217, i32::MIN])]),
        f([-3., 0., 16_777_216., -2147483648.])
    );
    assert_eq!(run(52, &[a, b]), f([1., 3., -3., 1.]));
    assert_eq!(run(59, &[[0, T, 0xf0f0_f0f0, 1]]), [T, 0, 0x0f0f_0f0f, !1]);
    assert_eq!(run(60, &[[0xf0, 0, 1, 2], [0x0f, 0, 2, 2]]), [0xff, 0, 3, 2]);
    let r = f([1.5, -1.5, -0.5, 2.]);
    assert_eq!(run(65, &[r]), f([1., -2., -1., 2.]));
    assert_eq!(run(66, &[r]), f([2., -1., -0., 2.]));
    assert_eq!(run(67, &[r]), f([1., -1., -0., 2.]));
    let (x, y) = ([1, T, 3, 0], [2, 1, 3, T]);
    assert_eq!(run(79, &[x, y]), [T, 0, 0, T]);
    assert_eq!(run(80, &[x, y]), [0, T, T, 0]);
    assert_eq!(run(83, &[x, y]), [2, T, 3, T]);
    assert_eq!(run(84, &[x, y]), [1, 1, 3, 0]);
    assert_eq!(run(85, &[[0x8000_0000, 16, 1, 7], [31, 2, 32, 1]]), [1, 4, 1, 3]);
    assert_eq!(run(87, &[[0xff, 0, T, 5], [0x0f, 0, T, 3]]), [0xf0, 0, 0, 6]);
}

#[test]
fn sincos_writes_sine_and_cosine_and_either_may_be_null() {
    let quarter = std::f32::consts::FRAC_PI_2;
    let source = lit([0f32.to_bits(), quarter.to_bits(), 0, 0]);
    let p = shader(0, 0, &[op(77, &[&dest(2, 0), &dest(2, 1), &source])]);
    let r = Program::from_words(&p)
        .unwrap()
        .execute(&[], &[], &mut no_sample)
        .unwrap();
    let (sin, cos) = (r.outputs[0].map(f32::from_bits), r.outputs[1].map(f32::from_bits));
    assert_eq!((sin[0], cos[0]), (0., 1.));
    assert_eq!(
        (sin[1], cos[1]),
        ((quarter as f64).sin() as f32, (quarter as f64).cos() as f32)
    );
    // the cosine alone, the sine to null
    let null = [13 << 12];
    let p = shader(0, 0, &[op(77, &[&null, &dest(2, 1), &source])]);
    let r = Program::from_words(&p)
        .unwrap()
        .execute(&[], &[], &mut no_sample)
        .unwrap();
    assert_eq!((r.output_masks[0], f32::from_bits(r.outputs[1][0])), (0, 1.));
}

/// if/else/endif on a nonzero or zero test, and a loop counting to three by breakc, as structured flow.
#[test]
fn structured_flow_takes_its_branches_and_loops() {
    let one = lit([1f32.to_bits(); 4]);
    let two = lit([2f32.to_bits(); 4]);
    let branch = |condition: u32, nonzero: bool| {
        let test = if nonzero { 1 << 18 } else { 0 };
        shader(
            1,
            0,
            &[
                op(54, &[&temp(0), &lit([condition; 4])]),
                op(31 | test, &[&scalar(0, 0, 0)]),
                op(54, &[&dest(2, 0), &one]),
                op(18, &[]),
                op(54, &[&dest(2, 0), &two]),
                op(21, &[]),
            ],
        )
    };
    let out = |words: Vec<u32>| {
        let r = Program::from_words(&words)
            .unwrap()
            .execute(&[], &[], &mut no_sample)
            .unwrap();
        (f32::from_bits(r.outputs[0][0]), r.instructions)
    };
    assert_eq!(out(branch(7, true)).0, 1.);
    assert_eq!(out(branch(0, true)).0, 2.);
    assert_eq!(out(branch(0, false)).0, 1.);
    // r0.x accumulates 1.5 while the integer counter r1.x is below 3
    let counted = shader(
        2,
        0,
        &[
            op(54, &[&temp(0), &lit([0; 4])]),
            op(54, &[&temp(1), &lit([0; 4])]),
            op(48, &[]),
            op(33, &[&[2 | 2 << 4 | 1 << 20, 1], &scalar(0, 1, 0), &lit([3; 4])]),
            op(3 | 1 << 18, &[&scalar(0, 1, 1)]),
            op(0, &[&temp(0), &whole(0, 0), &lit([1.5f32.to_bits(); 4])]),
            op(30, &[&temp(1), &whole(0, 1), &lit([1; 4])]),
            op(22, &[]),
            op(54, &[&dest(2, 0), &whole(0, 0)]),
        ],
    );
    let (value, executed) = out(counted);
    assert_eq!(value, 4.5);
    // two movs, loop once (endloop returns past it), per pass ige/breakc/add/iadd/endloop (5 x 3), the last
    // ige/breakc, mov, ret
    assert_eq!(executed, 2 + 1 + 15 + 2 + 2);
}

#[test]
fn unstructured_flow_and_unending_loops_are_refused() {
    for (body, message) in [
        (vec![op(18, &[])], "else without its if"),
        (vec![op(21, &[])], "endif without its if"),
        (vec![op(22, &[])], "endloop without its loop"),
        (vec![op(2, &[])], "break or continue outside a loop"),
        (vec![op(48, &[])], "ret inside an if or loop"),
    ] {
        let error = Program::from_words(&shader(0, 0, &body)).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    }
    let forever = shader(0, 0, &[op(48, &[]), op(22, &[])]);
    let error = Program::from_words(&forever)
        .unwrap()
        .execute(&[], &[], &mut no_sample)
        .unwrap_err()
        .to_string();
    assert!(error.contains("instruction budget"), "{error}");
}

/// deriv_rtx/rty read the caller's derivatives of an input register, swizzled; none given is an error; a derivative
/// of a computed value is refused when the program is read.
#[test]
fn derivatives_are_the_callers_for_inputs_only() {
    let words = shader(
        0,
        1,
        &[
            op(
                11,
                &[&dest(2, 0), &[2 | 1 << 2 | 0x1b << 4 | 1 << 12 | 1 << 20, 0]],
            ),
            op(125, &[&dest(2, 1), &whole(1, 0)]),
        ],
    );
    let p = Program::from_words(&words).unwrap();
    let d = [
        [1f32, 2., 3., 4.].map(f32::to_bits),
        [5f32, 6., 7., 8.].map(f32::to_bits),
    ];
    let r = p
        .execute_with_derivatives(&[[0; 4]], &[d], &[], &mut no_sample)
        .unwrap();
    // wzyx of d/dx; d/dy as it is
    assert_eq!(r.outputs[0].map(f32::from_bits), [4., 3., 2., 1.]);
    assert_eq!(r.outputs[1].map(f32::from_bits), [5., 6., 7., 8.]);
    let error = p.execute(&[[0; 4]], &[], &mut no_sample).unwrap_err().to_string();
    assert!(error.contains("no derivatives given for v0"), "{error}");
    let computed = shader(
        1,
        0,
        &[
            op(54, &[&temp(0), &lit([0; 4])]),
            op(12, &[&dest(2, 0), &whole(0, 0)]),
        ],
    );
    // a derivative of a computed value reads, and is refused where it runs
    let p = Program::from_words(&computed).unwrap();
    let error = p.execute(&[[0; 4]], &[], &mut no_sample).unwrap_err().to_string();
    assert!(error.contains("a derivative of a computed value"), "{error}");
}

/// sample_d hands its gradients to the sampler; a texture3d is declared but sampling it is refused; a ps_4_0 program
/// reads as ps_5_0 does.
#[test]
fn gradient_samples_volumes_and_shader_model_4() {
    let texture = |slot: u32, dimension: u32| {
        instruction(
            88 | dimension << 11,
            &[2 | 15 << 4 | 7 << 12 | 1 << 20, slot, 0x5555],
        )
    };
    let resource = |slot: u32| {
        [
            2 | 1 << 2 | 0xe4 << 4 | 7 << 12 | 1 << 20,
            slot,
            6 << 12 | 1 << 20,
            0,
        ]
    };
    let uv = lit([0.25f32.to_bits(), 0.5f32.to_bits(), 0, 0]);
    let program = |version: u32, slot: u32| {
        let mut v = vec![version, 0];
        v.extend(texture(1, 3));
        v.extend(texture(2, 5));
        v.extend(instruction(90, &[6 << 12 | 1 << 20, 0]));
        v.extend(instruction(101, &dest(2, 0)));
        let dx = lit([0.125f32.to_bits(), 0, 0, 0]);
        let dy = lit([0, 0.0625f32.to_bits(), 0, 0]);
        v.extend(op(73, &[&dest(2, 0), &uv, &resource(slot), &dx, &dy]));
        v.extend(instruction(62, &[]));
        v[1] = v.len() as u32;
        v
    };
    let p = Program::from_words(&program(0x40, 1)).unwrap();
    assert_eq!((p.textures(), p.volumes()), (&[1][..], &[2][..]));
    let r = p
        .execute(&[], &[], &mut |s: Sample| {
            assert_eq!((s.texture, s.uv), (1, [0.25, 0.5]));
            assert_eq!(s.lod, primase_shader::Lod::Gradients([[0.125, 0.], [0., 0.0625]]));
            Ok([1., 2., 3., 4.])
        })
        .unwrap();
    assert_eq!(r.outputs[0].map(f32::from_bits), [1., 2., 3., 4.]);
    let error = Program::from_words(&program(0x50, 2))
        .unwrap()
        .execute(&[], &[], &mut no_sample)
        .unwrap_err()
        .to_string();
    assert!(error.contains("t2 is a 3D texture"), "{error}");
    assert!(Program::from_words(&program(0x30, 1)).is_err());
}

/// An instruction's name as primase's table spells it, from Microsoft's disassembly: the resource and return-type
/// annotation and `_indexable` dropped, an immediate offset of zero (`_aoffimmi(0,0,0)`, the plain instruction) dropped,
/// saturation dropped, a conditional's test dropped.
fn disassembled_name(token: &str) -> &str {
    let name = token.split('(').next().unwrap_or(token);
    let name = name.strip_suffix("_indexable").unwrap_or(name);
    let zero_offset = token.contains("_aoffimmi_indexable(0,0,0)") || token.contains("_aoffimmi(0,0,0)");
    let name = match name.strip_suffix("_aoffimmi") {
        Some(plain) if zero_offset => plain,
        _ => name,
    };
    let name = name.strip_suffix("_sat").unwrap_or(name);
    match name {
        "if_nz" | "if_z" => "if",
        "breakc_nz" | "breakc_z" => "breakc",
        "continuec_nz" | "continuec_z" => "continuec",
        "discard_nz" | "discard_z" => "discard",
        "retc_nz" | "retc_z" => "retc",
        _ => name,
    }
}

/// A declared resource (`t`) or view (`u`) from a `dcl_resource_*` or `dcl_uav_*` line: its slot, shape and return
/// type.
fn disassembled_declaration(line: &str) -> Option<(char, ResourceDeclaration)> {
    let (kind, rest) = if let Some(r) = line.strip_prefix("dcl_resource_") {
        ('t', r)
    } else if let Some(r) = line.strip_prefix("dcl_uav_typed_") {
        ('u', r)
    } else {
        ('u', line.strip_prefix("dcl_uav_")?)
    };
    let register = |s: &str| s.trim().trim_start_matches(kind).parse::<u32>().unwrap();
    if let Some(r) = rest.strip_prefix("structured") {
        let (slot, stride) = r.trim_start_matches("_glc").trim().split_once(',').unwrap();
        return Some((
            kind,
            ResourceDeclaration {
                slot: register(slot),
                dimension: Dimension::Structured(stride.trim().parse().unwrap()),
                returns: ReturnType::Mixed,
            },
        ));
    }
    let (shape, rest) = rest.split_once(' ').unwrap();
    let dimension = match shape.trim_end_matches("_glc") {
        "buffer" => Dimension::Buffer,
        "texture2d" => Dimension::Texture2d,
        "texture2darray" => Dimension::Texture2dArray,
        "texture3d" => Dimension::Texture3d,
        "texturecube" => Dimension::TextureCube,
        "texturecubearray" => Dimension::TextureCubeArray,
        other => panic!("a shape primase doesn't declare: {other}"),
    };
    let (types, slot) = rest.rsplit_once(' ').unwrap();
    let returns = match types {
        "(float,float,float,float)" => ReturnType::Float,
        "(uint,uint,uint,uint)" => ReturnType::Uint,
        "(sint,sint,sint,sint)" => ReturnType::Sint,
        other => panic!("return types primase doesn't declare: {other}"),
    };
    Some((
        kind,
        ResourceDeclaration {
            slot: register(slot),
            dimension,
            returns,
        },
    ))
}

/// The immediate constant buffer's values as the disassembler prints them, row by row.
fn disassembled_immediates(text: &str) -> Vec<[String; 4]> {
    let Some(at) = text.find("dcl_immediateConstantBuffer") else {
        return Vec::new();
    };
    // the outer braces' contents: up to the brace closing the first one
    let block = &text[at..];
    let mut depth = 0;
    let end = block
        .char_indices()
        .find(|&(_, c)| {
            depth += match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            };
            c == '}' && depth == 0
        })
        .unwrap()
        .0;
    block[block.find('{').unwrap() + 1..end]
        .split('{')
        .filter_map(|row| {
            let row = row.split('}').next()?.trim();
            (!row.is_empty()).then(|| {
                let v: Vec<String> = row.split(',').map(|x| x.trim().to_owned()).collect();
                std::array::from_fn(|i| v[i].clone())
            })
        })
        .collect()
}

/// Every pixel container of `PRIMASE_DXBC_PACK_DISASSEMBLY` (`NNNN.dxbc`, each with Microsoft's D3D Shader
/// Disassembler text `NNNN.txt`) that primase reads: its checksum verified, then its instruction stream, its declared
/// resources and views, and its immediate constant buffer equal to the disassembly's (each value printed as the
/// disassembler prints it). The ones primase refuses are counted by reason.
#[test]
fn pack_against_microsoft_disassembly() {
    let Some(root) = primase_testdata::real_data("PRIMASE_DXBC_PACK_DISASSEMBLY") else {
        return;
    };
    let mut read = 0;
    let mut refused = std::collections::BTreeMap::<String, u32>::new();
    let mut paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "dxbc"))
        .collect();
    paths.sort();
    for path in paths {
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            primase_dxbc::checksum(&bytes).unwrap()[..],
            bytes[4..20],
            "{}",
            path.display()
        );
        let text = String::from_utf8(std::fs::read(path.with_extension("txt")).unwrap())
            .unwrap()
            .replace('\0', "");
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let Some(start) = lines.iter().position(|l| *l == "ps_5_0") else {
            continue;
        };
        let p = match Program::parse(&bytes) {
            Ok(p) => p,
            Err(e) => {
                let reason: String = e.detail.chars().filter(|c| !c.is_ascii_digit()).collect();
                *refused.entry(reason).or_default() += 1;
                continue;
            }
        };
        let body = &lines[start + 1..];
        let names: Vec<&str> = body
            .iter()
            .filter(|l| {
                !l.is_empty() && !l.starts_with("//") && !l.starts_with("dcl_") && !l.starts_with('{')
            })
            .map(|l| disassembled_name(l.split_whitespace().next().unwrap()))
            .collect();
        let ours: Vec<&str> = p.instructions().iter().map(|i| i.name()).collect();
        assert_eq!(ours, names, "{}", path.display());
        let (mut resources, mut views) = (Vec::new(), Vec::new());
        for (kind, d) in body.iter().filter_map(|l| disassembled_declaration(l)) {
            if kind == 't' {
                resources.push(d);
            } else {
                views.push(d);
            }
        }
        let by_slot = |v: &[ResourceDeclaration]| {
            let mut v = v.to_vec();
            v.sort_by_key(|d| d.slot);
            v
        };
        assert_eq!(by_slot(p.resources()), by_slot(&resources), "{}", path.display());
        assert_eq!(by_slot(p.views()), by_slot(&views), "{}", path.display());
        let printed: Vec<[String; 4]> = p
            .immediates()
            .iter()
            .map(|row| {
                row.map(|bits| {
                    let f = f32::from_bits(bits);
                    // a lane whose bits are a normal float is printed as one, the others as the integer
                    if f.is_normal() {
                        format!("{f:.6}")
                    } else {
                        (bits as i32).to_string()
                    }
                })
            })
            .collect();
        assert_eq!(printed, disassembled_immediates(&text), "{}", path.display());
        read += 1;
    }
    println!("pixel containers read, matching the disassembly: {read}; refused: {refused:?}");
    assert!(read > 0);
}
