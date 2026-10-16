#![allow(clippy::unwrap_used)]
use primase_dxbc::{Program, Sample};

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
    for (op, name) in [(31, "if"), (48, "loop"), (72, "sample_l"), (73, "sample_d")] {
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

/// Optional directory of independent ps_5_0 binaries and matching disassembler
/// text (.asm appended). Counts executable instructions including final ret.
#[test]
fn independent_disassembly_counts() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_core::gate::real_data_value("PRIMASE_DXBC_SHADERS") else {
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
            .skip_while(|l| l.trim() != "ps_5_0")
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
            let mut traces = Vec::new();
            let result = p
                .execute_traced(
                    &inputs,
                    &refs,
                    &mut |_: Sample| Ok([0.25, 0.5, 0.75, 1.0]),
                    &mut |t| traces.push((t.word, t.temps.to_vec(), t.outputs.to_vec())),
                )
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let trace_path = path.with_extension("ppo.trace.json");
            if trace_path.exists() {
                let oracle: serde_json::Value = serde_json::from_slice(&std::fs::read(trace_path)?)?;
                let oracle = oracle.as_array().unwrap();
                assert_eq!(oracle.len(), traces.len());
                'first: for (n, ((word, temps, outputs), expected)) in traces.iter().zip(oracle).enumerate() {
                    for (kind, values) in [("temps", temps), ("outputs", outputs)] {
                        let expected = vectors(&expected[kind]);
                        for (r, v) in values.iter().enumerate() {
                            for (c, expected_bits) in expected[r].iter().enumerate() {
                                if v.mask & (1 << c) != 0 && v.bits[c] != *expected_bits {
                                    println!(
                                        "first trace divergence {} instruction {n} word {word} {} {kind}[{r}].{c}: {:08x} vs {:08x}; operands {:?}",
                                        path.file_name().unwrap().to_string_lossy(),
                                        p.instructions()[n].name(),
                                        v.bits[c],
                                        expected[r][c],
                                        p.instructions()[n].operands
                                    );
                                    break 'first;
                                }
                            }
                        }
                    }
                }
            }
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
