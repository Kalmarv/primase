#![allow(clippy::unwrap_used)]
//! Lifting the D3D behaviour fixture (tests/fixtures/modes.hlsl, our own HLSL, compiled with fxc /T ps_5_0 /E main):
//! the interface and resources as worked out from the HLSL, and the D3D behaviours present in the lifted body as
//! plain operations.
use primase_dxbc::lift;
use primase_shader::{BufferKind, ResourceKind, ScalarType, Stage, SystemValue};

const MODES: &[u8] = include_bytes!("fixtures/modes.dxbc");

#[test]
fn the_fixture_lifts_validates_and_writes_wgsl() {
    let program = lift(MODES).unwrap();
    assert_eq!(program.stage, Stage::Pixel);
    assert_eq!(program.provenance.target, "ps_5_0");
    assert!(program.modes.divide_by_zero && program.modes.float_to_int && program.modes.min_max_nan);
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));

    // five colour targets out: a float4, three uint4 and a float4 (HLSL: struct O). The program takes SV_Position as
    // a parameter but never reads it, so it declares no input and exposes none.
    assert!(program.interface.inputs.is_empty());
    let targets: Vec<_> = program
        .interface
        .outputs
        .iter()
        .map(|v| (v.system, v.value.scalar, v.value.width))
        .collect();
    assert_eq!(
        targets,
        [
            (Some(SystemValue::Target(0)), ScalarType::Float, 4),
            (Some(SystemValue::Target(1)), ScalarType::Uint, 4),
            (Some(SystemValue::Target(2)), ScalarType::Uint, 4),
            (Some(SystemValue::Target(3)), ScalarType::Uint, 4),
            (Some(SystemValue::Target(4)), ScalarType::Float, 4),
        ]
    );

    // cbuffer C : register(b0) { float4 f; uint4 u; int4 i; }: 48 bytes, named by the container's reflection
    let [cb] = program.interface.resources.as_slice() else {
        panic!("{:?}", program.interface.resources)
    };
    assert_eq!(cb.provenance.name, "C");
    let ResourceKind::ConstantBuffer { layout } = &cb.kind else {
        panic!("{cb:?}")
    };
    assert_eq!(layout.size, 48);
    let names: Vec<_> = layout
        .constants
        .iter()
        .map(|c| (c.name.as_str(), c.offset))
        .collect();
    assert_eq!(names, [("f", 0), ("u", 16), ("i", 32)]);
    let _ = BufferKind::Raw;

    let wgsl = primase_shader::emit::wgsl(&program).unwrap();
    // NaN is tested on its bits (never `x != x`, which a driver may fold away): |x| bits above the infinity's
    assert!(
        wgsl.contains("2147483647u") && wgsl.contains("2139095040u"),
        "{wgsl}"
    );
    // D3D's unsigned division by zero: a division by a divisor made safe, all ones where the divisor was zero
    assert!(wgsl.contains("4294967295u"), "{wgsl}");
}

#[test]
fn the_text_form_names_what_the_lift_decided() {
    let text = lift(MODES).unwrap().text().unwrap();
    assert!(
        text.contains("// d3d divide_by_zero=true float_to_int=true min_max_nan=true saturate_nan=true"),
        "{text}"
    );
    assert!(
        text.contains("// source Dxbc ps_5_0 lifted by primase-dxbc 1"),
        "{text}"
    );
}

/// A ps_5_0 container around `body` (declarations and instructions), with empty signatures.
fn pixel_container(body: &[Vec<u32>]) -> Vec<u8> {
    let mut code = vec![0x50u32, 0];
    for b in body {
        code.extend(b);
    }
    code[1] = code.len() as u32;
    let empty_signature: Vec<u8> = [0u32, 8].iter().flat_map(|w| w.to_le_bytes()).collect();
    let shex: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let chunks: [(&[u8; 4], &[u8]); 3] = [
        (b"ISGN", &empty_signature),
        (b"OSGN", &empty_signature),
        (b"SHEX", &shex),
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

fn instruction(token: u32, operands: &[u32]) -> Vec<u32> {
    let mut v = vec![token | (((operands.len() + 1) as u32) << 24)];
    v.extend_from_slice(operands);
    v
}

/// `sincos r0.x, r1.x, r0.x` writes the sine to r0.x and the cosine to r1.x, both of the original r0.x: the
/// instruction reads its source once. (A lift that re-read r0.x for the cosine gave cos(sin(x)): found by the
/// executor-against-evaluator comparison on a Unity program.)
#[test]
fn a_destination_that_aliases_the_source_does_not_change_the_second_result() {
    let temp = |register: u32, mask: u32| [2 | mask << 4 | 1 << 20, register];
    let scalar = |register: u32, lane: u32| [2 | 2 << 2 | lane << 4 | 1 << 20, register];
    let operands = [temp(0, 1), temp(1, 1), scalar(0, 0)].concat();
    let container = pixel_container(&[
        instruction(104, &[2]),
        instruction(77, &operands),
        instruction(62, &[]),
    ]);
    let program = lift(&container).unwrap();
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    let function = &program.module.entry_points[0].function;
    let argument_of = |wanted: naga::MathFunction| {
        function.expressions.iter().find_map(|(_, e)| match e {
            naga::Expression::Math { fun, arg, .. } if *fun == wanted => Some(*arg),
            _ => None,
        })
    };
    let base_of = |h| match &function.expressions[h] {
        naga::Expression::AccessIndex { base, .. } => *base,
        other => panic!("{other:?}"),
    };
    let (sin, cos) = (
        argument_of(naga::MathFunction::Sin).unwrap(),
        argument_of(naga::MathFunction::Cos).unwrap(),
    );
    assert_eq!(base_of(sin), base_of(cos), "both read the one loaded source");
}
