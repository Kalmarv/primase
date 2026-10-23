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
