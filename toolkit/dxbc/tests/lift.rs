#![allow(clippy::unwrap_used)]
//! Lifting the D3D behaviour fixture (tests/fixtures/modes.hlsl, our own HLSL, compiled with fxc /T ps_5_0 /E main):
//! the interface and resources as worked out from the HLSL, and the D3D behaviours present in the lifted body as
//! plain operations.
use iridia::{BufferKind, ResourceKind, ScalarType, Stage, SystemValue};
use primase_dxbc::lift;

const MODES: &[u8] = include_bytes!("fixtures/modes.dxbc");

#[test]
fn the_fixture_lifts_validates_and_writes_wgsl() {
    let program = lift(MODES).unwrap();
    assert_eq!(program.stage, Stage::Pixel);
    assert_eq!(program.provenance.target, "ps_5_0");
    assert!(program.modes.divide_by_zero && program.modes.float_to_int && program.modes.min_max_nan);
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));

    // the program takes SV_Position as a parameter but never reads it: the signature lists it, and it is unused
    let inputs: Vec<_> = program
        .interface
        .inputs
        .iter()
        .map(|v| (v.system, v.used))
        .collect();
    assert_eq!(inputs, [(Some(SystemValue::FragCoord), false)]);
    // five colour targets out: a float4, three uint4 and a float4 (HLSL: struct O)
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

    let wgsl = iridia::emit::wgsl(&program).unwrap();
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
        text.contains(&format!(
            "// source Dxbc ps_5_0 lifted by primase-dxbc {}",
            primase_dxbc::LIFTER_VERSION
        )),
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

/// A typed unordered-access texture view, stored to and loaded from: `dcl_uav_typed_texture2d (float,float,float,
/// float) u3; store_uav_typed u3.xyzw, r0.xyxx, r1.xyzw; ld_uav_typed r1.xyzw, r0.xyxx, u3.xyzw`. It lifts to a
/// storage image of four 32-bit float lanes, read and written, bound as `u3`, and the instructions to an image store
/// and an image load.
#[test]
fn a_typed_view_is_a_storage_image_stored_and_loaded() {
    let temp_mask = |register: u32| [2 | 15 << 4 | 1 << 20, register];
    let temp = |register: u32, swizzle: u32| [2 | 1 << 2 | swizzle << 4 | 1 << 20, register];
    let view = |slot: u32, mask_or_swizzle: u32, swizzled: bool| {
        [
            2 | u32::from(swizzled) << 2 | mask_or_swizzle << 4 | 30 << 12 | 1 << 20,
            slot,
        ]
    };
    let xyxx = 0b00_00_01_00;
    let xyzw = 0b11_10_01_00;
    let declaration = [30 << 12 | 1 << 20, 3, 0x5555];
    let store = [view(3, 15, false).as_slice(), &temp(0, xyxx), &temp(1, xyzw)].concat();
    let load = [temp_mask(1).as_slice(), &temp(0, xyxx), &view(3, xyzw, true)].concat();
    let container = pixel_container(&[
        instruction(156 | 3 << 11, &declaration),
        instruction(104, &[2]),
        instruction(164, &store),
        instruction(163, &load),
        instruction(62, &[]),
    ]);
    let program = lift(&container).unwrap();
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    let [resource] = program.interface.resources.as_slice() else {
        panic!("{:?}", program.interface.resources)
    };
    assert_eq!(resource.provenance.name, "u3");
    assert_eq!(
        resource.provenance.d3d.map(|b| (b.class, b.register)),
        Some((iridia::D3dClass::UnorderedAccess, 3))
    );
    assert_eq!(
        resource.kind,
        ResourceKind::Storage {
            target: iridia::StorageTarget::Image {
                dim: iridia::TextureDim::D2,
                arrayed: false,
                sample: iridia::SampleType::Float,
            },
            access: iridia::Access::ReadWrite,
        }
    );
    let wgsl = iridia::emit::wgsl(&program).unwrap();
    assert!(wgsl.contains("@group(3) @binding(3)"), "{wgsl}");
    assert!(
        wgsl.contains("texture_storage_2d<rgba32float,read_write>"),
        "{wgsl}"
    );
    assert!(wgsl.contains("textureStore(u3_"), "{wgsl}");
    assert!(wgsl.contains("textureLoad(u3_"), "{wgsl}");
    assert!(
        program
            .notes
            .iter()
            .any(|n| n.contains("storage image of four 32-bit lanes"))
    );
}

/// A store through a view with a partial write mask would keep the other lanes, which a store of four lanes cannot:
/// refused by name rather than lanes lost.
#[test]
fn a_partial_write_mask_on_a_view_is_refused() {
    let declaration = [30 << 12 | 1 << 20, 0, 0x4444];
    let store = [
        [2 | 1 << 4 | 30 << 12 | 1 << 20, 0],
        [2 | 1 << 2 | 0b00_00_01_00 << 4 | 1 << 20, 0],
        [2 | 1 << 2 | 0b11_10_01_00 << 4 | 1 << 20, 1],
    ]
    .concat();
    let container = pixel_container(&[
        instruction(156 | 3 << 11, &declaration),
        instruction(104, &[2]),
        instruction(164, &store),
        instruction(62, &[]),
    ]);
    let e = lift(&container).unwrap_err();
    assert!(e.detail.contains("partial write mask"), "{e}");
}

const COMPUTE: [&[u8]; 2] = [
    include_bytes!("fixtures/compute_basic.dxbc"),
    include_bytes!("fixtures/compute_basic.od.dxbc"),
];

/// The compute fixture (tests/fixtures/compute_basic.hlsl, fxc /T cs_5_0): a thread group of 8 by 4 by 2, the four
/// ids, shared memory with atomics, and a raw, a structured and a typed buffer.
#[test]
fn the_compute_fixture_lifts_validates_and_writes_wgsl() {
    for (name, container) in ["default", "/Od"].into_iter().zip(COMPUTE) {
        let program = lift(container).unwrap_or_else(|e| panic!("{name}: {e}"));
        program.validate().unwrap_or_else(|p| panic!("{name}: {p:?}"));
        assert_eq!(program.stage, Stage::Compute, "{name}");
        assert_eq!(program.provenance.target, "cs_5_0", "{name}");
        assert_eq!(program.module.entry_points[0].workgroup_size, [8, 4, 2], "{name}");
        let systems: Vec<_> = program.interface.inputs.iter().filter_map(|v| v.system).collect();
        for id in [
            SystemValue::GroupId,
            SystemValue::GroupThreadId,
            SystemValue::DispatchThreadId,
            SystemValue::GroupIndex,
        ] {
            assert!(systems.contains(&id), "{name}: no {id:?} in {systems:?}");
        }
        // u0 raw, u1 structured of uint, u2 typed: all read and written
        let storage: Vec<_> = program
            .interface
            .resources
            .iter()
            .filter_map(|r| match &r.kind {
                ResourceKind::Storage {
                    target: iridia::StorageTarget::Buffer(kind),
                    ..
                } => Some((r.provenance.d3d.unwrap().register, *kind)),
                _ => None,
            })
            .collect();
        assert!(
            storage.iter().any(|(r, k)| *r == 0 && *k == BufferKind::Raw),
            "{name}: {storage:?}"
        );
        assert!(
            storage
                .iter()
                .any(|(r, k)| *r == 1 && matches!(k, BufferKind::Structured { stride: 4 })),
            "{name}: {storage:?}"
        );
        assert!(
            storage
                .iter()
                .any(|(r, k)| *r == 2 && matches!(k, BufferKind::Typed(_))),
            "{name}: {storage:?}"
        );
        let wgsl = iridia::emit::wgsl(&program).unwrap();
        for needle in [
            "@compute @workgroup_size(8, 4, 2)",
            "var<workgroup>",
            "atomicAdd(",
            "workgroupBarrier()",
            "var<storage, read_write>",
        ] {
            assert!(wgsl.contains(needle), "{name}: {needle:?} is not in\n{wgsl}");
        }
    }
}

/// The hull fixture whose phases declare different numbers of temporaries (tests/fixtures/hull_phases.hlsl, fxc /T hs_5_0:
/// `dcl_temps 2` in the control-point phase, 1 in the last fork phase) lifts: each phase may use the temporaries it
/// declared, so the program has the most any of them declares.
#[test]
fn a_hull_program_whose_phases_declare_different_temporaries_lifts() {
    let program = lift(include_bytes!("fixtures/hull_phases.hs.dxbc")).unwrap_or_else(|e| panic!("{e}"));
    program.validate().unwrap_or_else(|p| panic!("{p:?}"));
    assert_eq!(program.stage, Stage::Hull);
    for name in ["main", "patch_constant"] {
        iridia_exec::Executable::function(&program, name).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}
