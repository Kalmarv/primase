#![allow(clippy::unwrap_used)]
//! Hand-worked controls of the lifted programs' values: each case is a program written as tokens, its expected outputs worked out
//! by hand from Direct3D's definition of the instruction (the value, not what a lift or an executor does), lifted and run by
//! the executor (`primase_shader::exec::ProgramShader`). The oracle is the hand-worked value; these cases stood for the
//! legacy evaluator's and now stand for the executor's.
use primase_dxbc::decode::Declared;
use primase_dxbc::{decode, lift};
use primase_shader::exec::ProgramShader;
use primase_shader::{Execution, PixelShader, Sample, Sampler};

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
/// A program writing o0 from `op` applied to literal `inputs`.
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

/// A signature chunk of these elements: (semantic name, index, system value, register, mask).
fn signature(elements: &[(&str, u32, u32, u32, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend((elements.len() as u32).to_le_bytes());
    out.extend(8u32.to_le_bytes());
    let mut names = Vec::new();
    let strings_at = 8 + 24 * elements.len();
    for &(name, index, system, register, mask) in elements {
        out.extend(((strings_at + names.len()) as u32).to_le_bytes());
        names.extend(name.bytes());
        names.push(0);
        out.extend(index.to_le_bytes());
        out.extend(system.to_le_bytes());
        // D3D_REGISTER_COMPONENT_FLOAT32
        out.extend(3u32.to_le_bytes());
        out.extend(register.to_le_bytes());
        out.extend([mask, mask, 0, 0]);
    }
    out.extend(names);
    out
}

/// The container of a program: its SHEX, and the signatures its input and output declarations state (inputs are texture
/// coordinates, outputs render targets).
fn container(words: &[u32]) -> Vec<u8> {
    let decoded = decode(words).unwrap();
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for d in &decoded.declarations {
        match &d.kind {
            Declared::Input { operand, .. } if operand.register == primase_dxbc::Register::Input => {
                let register = operand.indices[0].base;
                let primase_dxbc::Selection::Mask(mask) = operand.selection else {
                    panic!("an input without a mask")
                };
                inputs.push(("TEXCOORD", register, 0, register, mask));
            }
            Declared::Output { operand, .. } if operand.register == primase_dxbc::Register::Output => {
                let register = operand.indices[0].base;
                let primase_dxbc::Selection::Mask(mask) = operand.selection else {
                    panic!("an output without a mask")
                };
                outputs.push(("SV_Target", register, 64, register, mask));
            }
            _ => {}
        }
    }
    let shex: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let chunks: [(&[u8; 4], Vec<u8>); 3] = [
        (b"ISGN", signature(&inputs)),
        (b"OSGN", signature(&outputs)),
        (b"SHEX", shex),
    ];
    let directory = 32 + 4 * chunks.len();
    let mut offsets = Vec::new();
    let mut at = directory;
    for (_, data) in &chunks {
        offsets.push(at as u32);
        at += 8 + data.len();
    }
    let mut out = b"DXBC".to_vec();
    out.extend([0u8; 16]);
    for w in [1, at as u32, chunks.len() as u32] {
        out.extend(w.to_le_bytes());
    }
    for o in offsets {
        out.extend(o.to_le_bytes());
    }
    for (tag, data) in chunks {
        out.extend(tag);
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
    }
    out
}

/// The program lifted and ready to run.
fn executor(words: &[u32]) -> ProgramShader {
    let program = lift(&container(words)).unwrap_or_else(|e| panic!("{e}"));
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    ProgramShader::new(program).unwrap_or_else(|e| panic!("{e}"))
}

fn no_sample(_: Sample) -> Result<[f32; 4], String> {
    panic!("unexpected sample")
}

fn run_words(words: &[u32]) -> Execution {
    executor(words).execute(&[], &[], &mut no_sample).unwrap()
}

fn run(op: u32, inputs: &[[u32; 4]]) -> [u32; 4] {
    run_words(&program(op, inputs)).outputs[0]
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
    let p = executor(&words);
    let values = [[0; 4], [11, 22, 33, 44]];
    let out = p.execute(&[], &[(0, &values)], &mut no_sample).unwrap();
    assert_eq!(out.outputs[0], values[1]);
    // the buffer is the declared rows exactly, and a buffer the program reads must be given
    assert!(p.execute(&[], &[], &mut no_sample).is_err());
    let error = p
        .execute(&[], &[(0, &values[..1])], &mut no_sample)
        .unwrap_err()
        .to_string();
    assert!(error.contains("1 rows given, 2 needed"), "{error}");
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
    let r = run_words(&words);
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
    let r = executor(&words)
        .execute(&[], &[], &mut |s: Sample| {
            calls += 1;
            assert_eq!((s.texture, s.sampler, s.uv), (2, 3, [0.25, 0.75]));
            Ok([1., 2., 3., 4.])
        })
        .unwrap();
    assert_eq!(r.outputs[0].map(f32::from_bits), [4., 3., 1., 2.]);
    assert_eq!((calls, r.samples), (1, 1));
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
    let r = run_words(&p);
    let (sin, cos) = (r.outputs[0].map(f32::from_bits), r.outputs[1].map(f32::from_bits));
    assert_eq!((sin[0], cos[0]), (0., 1.));
    assert_eq!(
        (sin[1], cos[1]),
        ((quarter as f64).sin() as f32, (quarter as f64).cos() as f32)
    );
    // the cosine alone, the sine to null
    let null = [13 << 12];
    let p = shader(0, 0, &[op(77, &[&null, &dest(2, 1), &source])]);
    let r = run_words(&p);
    assert_eq!(f32::from_bits(r.outputs[1][0]), 1.);
}

/// udiv: the unsigned quotient and remainder (worked by hand), Direct3D's 0xFFFFFFFF for both on a zero divisor, and
/// either destination null.
#[test]
fn udiv_writes_quotient_and_remainder_and_either_may_be_null() {
    let x = lit([17, 0xFFFF_FFFF, 5, 12]);
    let y = lit([5, 2, 0, 12]);
    let p = shader(0, 0, &[op(78, &[&dest(2, 0), &dest(2, 1), &x, &y])]);
    let r = run_words(&p);
    assert_eq!(r.outputs[0], [3, 0x7FFF_FFFF, u32::MAX, 1]);
    assert_eq!(r.outputs[1], [2, 1, u32::MAX, 0]);
    // the remainder alone, the quotient to null
    let null = [13 << 12];
    let p = shader(0, 0, &[op(78, &[&null, &dest(2, 1), &x, &y])]);
    let r = run_words(&p);
    assert_eq!(r.outputs[1], [2, 1, u32::MAX, 0]);
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
    let out = |words: Vec<u32>| f32::from_bits(run_words(&words).outputs[0][0]);
    assert_eq!(out(branch(7, true)), 1.);
    assert_eq!(out(branch(0, true)), 2.);
    assert_eq!(out(branch(0, false)), 1.);
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
    assert_eq!(out(counted), 4.5);
}

/// Unbalanced flow is refused when the program is lifted; a loop that never ends runs to the executor's step budget and is
/// an error, not a hang.
#[test]
fn unstructured_flow_and_unending_loops_are_refused() {
    for (body, message) in [
        (vec![op(18, &[])], "else without its if"),
        (vec![op(21, &[])], "endif without its if"),
        (vec![op(22, &[])], "endloop without its loop"),
    ] {
        let error = lift(&container(&shader(0, 0, &body))).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
    }
    let forever = shader(0, 0, &[op(48, &[]), op(22, &[])]);
    let error = executor(&forever)
        .execute(&[], &[], &mut no_sample)
        .unwrap_err()
        .to_string();
    assert!(error.contains("budget"), "{error}");
}

/// deriv_rtx/rty read the caller's derivatives of an input register, swizzled.
#[test]
fn derivatives_are_the_callers_for_inputs() {
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
    let p = executor(&words);
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
}

/// sample_d hands its gradients to the sampler; a ps_4_0 program reads as ps_5_0 does.
#[test]
fn gradient_samples_and_shader_model_4() {
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
    for version in [0x40, 0x50] {
        let p = executor(&program(version, 1));
        assert_eq!(p.textures(), &[1][..], "version {version:#x}");
        let r = p
            .execute(&[], &[], &mut |s: Sample| {
                assert_eq!((s.texture, s.uv), (1, [0.25, 0.5]));
                assert_eq!(s.lod, primase_shader::Lod::Gradients([[0.125, 0.], [0., 0.0625]]));
                Ok([1., 2., 3., 4.])
            })
            .unwrap();
        assert_eq!(
            r.outputs[0].map(f32::from_bits),
            [1., 2., 3., 4.],
            "version {version:#x}"
        );
    }
}

/// A quad's lanes' v0.x: 1, 2 (the top row), 3, 5 (the bottom).
fn quad_inputs() -> [Vec<[u32; 4]>; 4] {
    [1f32, 2., 3., 5.].map(|x| vec![[x.to_bits(), 0, 0, 0]])
}

fn quad(words: &[u32], sampler: &mut dyn Sampler) -> [Execution; 4] {
    let inputs = quad_inputs();
    executor(words)
        .execute_quad([&inputs[0], &inputs[1], &inputs[2], &inputs[3]], &[], sampler)
        .unwrap()
}

/// Derivatives of a computed value r0 = v0.x squared (1, 4 / 9, 25) inside the quad: coarse d/dx 4 - 1 = 3 and
/// d/dy 9 - 1 = 8 for every lane; fine d/dx the lane's row (3 above, 25 - 9 = 16 below), fine d/dy its column
/// (9 - 1 = 8 left, 25 - 4 = 21 right).
#[test]
fn a_quad_differences_its_lanes() {
    let program = |x: u32, y: u32| {
        shader(
            1,
            1,
            &[
                op(56, &[&temp(0), &scalar(1, 0, 0), &scalar(1, 0, 0)]),
                op(x, &[&dest(2, 0), &whole(0, 0)]),
                op(y, &[&dest(2, 1), &whole(0, 0)]),
            ],
        )
    };
    let lanes = |words: Vec<u32>| {
        quad(&words, &mut no_sample).map(|e| [0, 1].map(|k| f32::from_bits(e.outputs[k][0])))
    };
    // deriv_rtx_coarse, deriv_rty_fine
    let r = lanes(program(122, 125));
    assert_eq!(r.map(|o| o[0]), [3.; 4]);
    assert_eq!(r.map(|o| o[1]), [8., 21., 8., 21.]);
    // deriv_rtx_fine, deriv_rty_coarse; deriv_rtx and deriv_rty are the coarse ones
    let r = lanes(program(123, 124));
    assert_eq!(r.map(|o| o[0]), [3., 3., 16., 16.]);
    assert_eq!(r.map(|o| o[1]), [8.; 4]);
    let r = lanes(program(11, 12));
    assert_eq!((r.map(|o| o[0]), r.map(|o| o[1])), ([3.; 4], [8.; 4]));
}

/// An implicit-LOD sample in a quad: its footprint the coordinates' coarse gradients, (0.25, 0) and (0, 0.5) here.
#[test]
fn a_quads_implicit_sample_has_its_gradients() {
    let mut words = vec![0x50, 0];
    words.extend(instruction(
        88 | 3 << 11,
        &[2 | 15 << 4 | 7 << 12 | 1 << 20, 2, 0x5555],
    ));
    words.extend(instruction(90, &[6 << 12 | 1 << 20, 3]));
    words.extend(instruction(98 | 2 << 11, &dest(1, 0)));
    words.extend(instruction(101, &dest(2, 0)));
    let resource = [
        2 | 1 << 2 | 0xe4 << 4 | 7 << 12 | 1 << 20,
        2,
        6 << 12 | 1 << 20,
        3,
    ];
    words.extend(op(69, &[&dest(2, 0), &whole(1, 0), &resource]));
    words.extend(instruction(62, &[]));
    words[1] = words.len() as u32;
    let uvs = [[0f32, 0.], [0.25, 0.], [0., 0.5], [0.25, 0.5]];
    let inputs = uvs.map(|[u, v]| vec![[u.to_bits(), v.to_bits(), 0, 0]]);
    let mut seen = Vec::new();
    let r = executor(&words)
        .execute_quad(
            [&inputs[0], &inputs[1], &inputs[2], &inputs[3]],
            &[],
            &mut |s: Sample| {
                seen.push((s.uv, s.lod));
                Ok([1., 2., 3., 4.])
            },
        )
        .unwrap();
    let footprint = primase_shader::Lod::Gradients([[0.25, 0.], [0., 0.5]]);
    assert_eq!(seen, uvs.map(|uv| (uv, footprint)));
    assert!(
        r.iter()
            .all(|e| e.samples == 1 && e.outputs[0] == [1f32, 2., 3., 4.].map(f32::to_bits))
    );
}

/// A lane that discards goes on as a helper, so its neighbours still have their derivatives, and is marked discarded.
#[test]
fn a_quad_keeps_its_helpers() {
    // the top-left lane discards (v0.x < 2), then every lane takes d/dx of v0.x: 2 - 1 = 1
    let helper = shader(
        1,
        1,
        &[
            op(49, &[&temp(0), &scalar(1, 0, 0), &lit([2f32.to_bits(); 4])]),
            op(13 | 1 << 18, &[&scalar(0, 0, 0)]),
            op(122, &[&dest(2, 0), &whole(1, 0)]),
        ],
    );
    let r = quad(&helper, &mut no_sample);
    assert_eq!(r.each_ref().map(|e| e.discarded), [true, false, false, false]);
    assert_eq!(r.each_ref().map(|e| f32::from_bits(e.outputs[0][0])), [1.; 4]);
}

/// Lanes that reach a derivative apart (one takes a branch the others don't) are refused, never given a derivative of
/// a partial quad.
#[test]
fn a_quad_refuses_divergent_derivatives() {
    // v0.x < 2: only the top-left lane enters the branch holding the derivative
    let divergent = shader(
        1,
        1,
        &[
            op(49, &[&temp(0), &scalar(1, 0, 0), &lit([2f32.to_bits(); 4])]),
            op(31 | 1 << 18, &[&scalar(0, 0, 0)]),
            op(122, &[&dest(2, 0), &whole(1, 0)]),
            op(21, &[]),
        ],
    );
    let inputs = quad_inputs();
    let error = executor(&divergent)
        .execute_quad(
            [&inputs[0], &inputs[1], &inputs[2], &inputs[3]],
            &[],
            &mut no_sample,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("diverge"), "{error}");
}

/// A program cut anywhere is refused, whole words or not.
#[test]
fn a_program_cut_short_is_refused() {
    let bytes = container(&program(54, &[[1; 4]]));
    assert!(lift(&bytes).is_ok());
    for n in 0..bytes.len() {
        assert!(lift(&bytes[..n]).is_err(), "{n} of {} bytes", bytes.len());
    }
}

/// An instruction with fewer operands than its opcode takes is refused naming the opcode (these opcodes are all in the
/// lift: it is the operand count that is wrong), never lifted from what the words happen to hold.
#[test]
fn an_instruction_with_the_wrong_operand_count_is_refused_by_name() {
    for (op, name) in [(76, "switch"), (81, "umul"), (109, "gather4"), (134, "countbits")] {
        let error = lift(&container(&program(op, &[]))).unwrap_err().to_string();
        assert!(error.contains(&format!("{name}: ")), "{error}");
    }
}
