#![allow(clippy::unwrap_used)]
//! The interface vocabulary of one HLSL fixture (tests/fixtures/resources.hlsl) read from both bytecodes, compiled by
//! two independent compilers (fxc for SM5, dxc for SM6): the resources, the constant layouts and the varyings are
//! worked out from the HLSL and fxc's own listing of the buffer layouts (the Buffer Definitions of its `/Fc`
//! output), and must be the same from either.
use iridia::{
    Access, BufferKind, ConstantType, D3dClass, Interface, InterpMode, InterpSampling, Majorness, Member,
    ResourceKind, SampleType, ScalarType, StorageTarget, SystemValue, TextureDim, Value,
};
use std::collections::BTreeMap;

/// A fixture's interface from each bytecode. DXBC's interpolation is the program's: only the lift of the program reads it
/// (the signature has none), so the varyings fixture is lifted and the resource fixture, which uses doubles (the lifter
/// refuses them), is reflected.
fn interfaces(resources: bool) -> Vec<(&'static str, Interface)> {
    let (dxbc, dxil): (&[u8], &[u8]) = if resources {
        (
            include_bytes!("fixtures/resources.dxbc"),
            include_bytes!("fixtures/resources.dxil"),
        )
    } else {
        (
            include_bytes!("fixtures/varyings.dxbc"),
            include_bytes!("fixtures/varyings.dxil"),
        )
    };
    vec![
        (
            "DXBC",
            if resources {
                iridia_dxbc::reflection(dxbc).unwrap()
            } else {
                iridia_dxbc::lift(dxbc).unwrap().interface
            },
        ),
        ("DXIL", primase_dxil::interface(dxil).unwrap()),
    ]
}

fn lanes(scalar: ScalarType, width: u8) -> Value {
    Value { scalar, width }
}
fn texture(dim: TextureDim, arrayed: bool, multisampled: bool, sample: SampleType) -> ResourceKind {
    ResourceKind::Texture {
        dim,
        arrayed,
        multisampled,
        sample,
    }
}

#[test]
fn resources_are_the_same_from_either_bytecode() {
    use ScalarType::{Float, Uint};
    let image = |sample| StorageTarget::Image {
        dim: TextureDim::D2,
        arrayed: false,
        sample,
    };
    let storage = |target| ResourceKind::Storage {
        target,
        access: Access::ReadWrite,
    };
    // (name, class, register, what it is); constant buffers are checked below
    let expected: Vec<(&str, D3dClass, u32, ResourceKind)> = vec![
        (
            "tex1d",
            D3dClass::ShaderResource,
            0,
            texture(TextureDim::D1, false, false, SampleType::Float),
        ),
        (
            "tex2d",
            D3dClass::ShaderResource,
            1,
            texture(TextureDim::D2, false, false, SampleType::Float),
        ),
        (
            "texuint",
            D3dClass::ShaderResource,
            2,
            texture(TextureDim::D2, false, false, SampleType::Uint),
        ),
        (
            "texsint",
            D3dClass::ShaderResource,
            3,
            texture(TextureDim::D2, false, false, SampleType::Sint),
        ),
        (
            "texarray",
            D3dClass::ShaderResource,
            4,
            texture(TextureDim::D2, true, false, SampleType::Float),
        ),
        (
            "texms",
            D3dClass::ShaderResource,
            5,
            texture(TextureDim::D2, false, true, SampleType::Float),
        ),
        (
            "tex3d",
            D3dClass::ShaderResource,
            6,
            texture(TextureDim::D3, false, false, SampleType::Float),
        ),
        (
            "texcube",
            D3dClass::ShaderResource,
            7,
            texture(TextureDim::Cube, false, false, SampleType::Float),
        ),
        (
            "texcubearray",
            D3dClass::ShaderResource,
            8,
            texture(TextureDim::Cube, true, false, SampleType::Float),
        ),
        (
            "typed",
            D3dClass::ShaderResource,
            9,
            ResourceKind::Buffer(BufferKind::Typed(lanes(Float, 2))),
        ),
        (
            "structured",
            D3dClass::ShaderResource,
            10,
            ResourceKind::Buffer(BufferKind::Structured { stride: 32 }),
        ),
        (
            "raw",
            D3dClass::ShaderResource,
            11,
            ResourceKind::Buffer(BufferKind::Raw),
        ),
        (
            "image",
            D3dClass::UnorderedAccess,
            2,
            storage(image(SampleType::Float)),
        ),
        (
            "ubuffer",
            D3dClass::UnorderedAccess,
            3,
            storage(StorageTarget::Buffer(BufferKind::Typed(lanes(Uint, 1)))),
        ),
        (
            "ustructured",
            D3dClass::UnorderedAccess,
            4,
            storage(StorageTarget::Buffer(BufferKind::Structured { stride: 32 })),
        ),
        (
            "uraw",
            D3dClass::UnorderedAccess,
            5,
            storage(StorageTarget::Buffer(BufferKind::Raw)),
        ),
        (
            "linear_wrap",
            D3dClass::Sampler,
            0,
            ResourceKind::Sampler { comparison: false },
        ),
        (
            "shadow",
            D3dClass::Sampler,
            3,
            ResourceKind::Sampler { comparison: true },
        ),
    ];
    for (name, interface) in interfaces(true) {
        let mut ids = std::collections::BTreeSet::new();
        for resource in &interface.resources {
            assert!(ids.insert(resource.id), "{name}: ids repeat");
            assert_eq!(resource.count, 1, "{name} {}", resource.provenance.name);
            assert_eq!(resource.provenance.d3d.unwrap().space, 0);
        }
        for (resource_name, class, register, kind) in &expected {
            let found = interface
                .resources
                .iter()
                .find(|r| r.provenance.name == *resource_name)
                .unwrap_or_else(|| panic!("{name}: no {resource_name}"));
            let d3d = found.provenance.d3d.unwrap();
            assert_eq!(
                (d3d.class, d3d.register),
                (*class, *register),
                "{name} {resource_name}"
            );
            assert_eq!(&found.kind, kind, "{name} {resource_name}");
        }
        let buffers: Vec<_> = interface
            .resources
            .iter()
            .filter(|r| matches!(r.kind, ResourceKind::ConstantBuffer { .. }))
            .map(|r| (r.provenance.name.as_str(), r.provenance.d3d.unwrap().register))
            .collect();
        assert_eq!(buffers.len(), 2, "{name}");
        assert!(
            buffers.contains(&("Frame", 2)) && buffers.contains(&("Extra", 5)),
            "{name}: {buffers:?}"
        );
        assert_eq!(interface.resources.len(), expected.len() + 2, "{name}");
    }
}

/// The constants of fxc's Buffer Definitions: each name with its offset, type and size.
#[test]
fn constant_layouts_follow_the_compilers_listing() {
    use ScalarType::{Bool, Double, Float, Sint, Uint};
    let vector = |scalar, width| ConstantType::Vector(Value { scalar, width });
    let light = ConstantType::Struct {
        members: vec![
            Member {
                name: "position".into(),
                offset: 0,
                ty: vector(Float, 3),
            },
            Member {
                name: "range".into(),
                offset: 12,
                ty: ConstantType::Scalar(Float),
            },
            Member {
                name: "colour".into(),
                offset: 16,
                ty: ConstantType::Array {
                    element: Box::new(vector(Float, 4)),
                    count: 2,
                    stride: 16,
                },
            },
        ],
    };
    // (name, offset, type, size)
    let frame = vec![
        (
            "world",
            0,
            ConstantType::Matrix {
                scalar: Float,
                rows: 4,
                columns: 4,
                major: Majorness::Column,
                stride: 16,
            },
            64,
        ),
        (
            "rotation",
            64,
            ConstantType::Matrix {
                scalar: Float,
                rows: 3,
                columns: 3,
                major: Majorness::Row,
                stride: 16,
            },
            44,
        ),
        (
            "offsets",
            112,
            ConstantType::Array {
                element: Box::new(vector(Float, 2)),
                count: 3,
                stride: 16,
            },
            40,
        ),
        (
            "lights",
            160,
            ConstantType::Array {
                element: Box::new(light),
                count: 2,
                stride: 48,
            },
            96,
        ),
        ("count", 256, ConstantType::Scalar(Sint), 4),
        ("flags", 272, vector(Uint, 4), 16),
        ("enabled", 288, ConstantType::Scalar(Bool), 4),
    ];
    let extra = vec![
        ("scale", 0, ConstantType::Scalar(Float), 4),
        ("wide", 8, ConstantType::Scalar(Double), 8),
        (
            "skew",
            16,
            ConstantType::Matrix {
                scalar: Float,
                rows: 3,
                columns: 4,
                major: Majorness::Column,
                stride: 16,
            },
            60,
        ),
        (
            "pose",
            80,
            ConstantType::Matrix {
                scalar: Float,
                rows: 2,
                columns: 4,
                major: Majorness::Row,
                stride: 16,
            },
            32,
        ),
    ];
    for (name, interface) in interfaces(true) {
        for (buffer, constants) in [("Frame", &frame), ("Extra", &extra)] {
            let resource = interface
                .resources
                .iter()
                .find(|r| r.provenance.name == buffer)
                .unwrap();
            let ResourceKind::ConstantBuffer { layout } = &resource.kind else {
                panic!("{name}: {buffer} is not a constant buffer")
            };
            let actual: Vec<_> = layout
                .constants
                .iter()
                .map(|c| (c.name.as_str(), c.offset, c.ty.clone(), c.ty.size().unwrap()))
                .collect();
            let expected: Vec<_> = constants
                .iter()
                .map(|(n, o, t, s)| (*n, *o, t.clone(), *s))
                .collect();
            assert_eq!(actual, expected, "{name} {buffer}");
        }
    }
}

#[test]
fn varyings_carry_their_interpolation_and_system_values() {
    use ScalarType::{Float, Uint};
    use SystemValue::{FragCoord, FrontFacing, Target};
    let center = |mode| Some((mode, InterpSampling::Center));
    // (label, index, system value, value, interpolation); registers are each compiler's packing
    let inputs = [
        (
            "SV_Position",
            0,
            Some(FragCoord),
            lanes(Float, 4),
            center(InterpMode::Linear),
        ),
        (
            "TEXCOORD",
            0,
            None,
            lanes(Float, 2),
            center(InterpMode::Perspective),
        ),
        ("TEXCOORD", 1, None, lanes(Uint, 1), center(InterpMode::Flat)),
        (
            "TEXCOORD",
            2,
            None,
            lanes(Float, 3),
            Some((InterpMode::Perspective, InterpSampling::Centroid)),
        ),
        ("TEXCOORD", 3, None, lanes(Float, 1), center(InterpMode::Linear)),
        (
            "TEXCOORD",
            4,
            None,
            lanes(Float, 4),
            Some((InterpMode::Perspective, InterpSampling::Sample)),
        ),
        (
            "SV_IsFrontFace",
            0,
            Some(FrontFacing),
            lanes(Uint, 1),
            center(InterpMode::Flat),
        ),
    ];
    let outputs = [
        ("SV_Target", 0, Some(Target(0)), lanes(Float, 4)),
        ("SV_Target", 1, Some(Target(1)), lanes(Float, 4)),
    ];
    for (name, interface) in interfaces(false) {
        let found: BTreeMap<_, _> = interface
            .inputs
            .iter()
            .map(|v| {
                (
                    (v.label.name.as_str(), v.label.index),
                    (v.system, v.value, v.interp.map(|i| (i.mode, i.sampling))),
                )
            })
            .collect();
        let want: BTreeMap<_, _> = inputs
            .iter()
            .map(|&(n, i, s, v, p)| ((n, i), (s, v, p)))
            .collect();
        assert_eq!(found, want, "{name} inputs");
        assert!(
            interface
                .inputs
                .iter()
                .all(|v| v.used || v.label.name == "SV_Position"),
            "{name}"
        );
        let found: BTreeMap<_, _> = interface
            .outputs
            .iter()
            .map(|v| ((v.label.name.as_str(), v.label.index), (v.system, v.value)))
            .collect();
        let want: BTreeMap<_, _> = outputs.iter().map(|&(n, i, s, v)| ((n, i), (s, v))).collect();
        assert_eq!(found, want, "{name} outputs");
        assert!(
            interface.outputs.iter().all(|v| v.interp.is_none() && v.used),
            "{name}"
        );
    }
}

/// The depth output states itself by name in fxc's output signature and by number in dxc's.
#[test]
fn the_depth_output_is_a_system_value_from_either_bytecode() {
    for (name, interface) in interfaces(true) {
        let depth = interface
            .outputs
            .iter()
            .find(|v| v.label.name.eq_ignore_ascii_case("SV_Depth"))
            .unwrap_or_else(|| panic!("{name}: no depth output"));
        assert_eq!(depth.system, Some(SystemValue::Depth), "{name}");
        assert_eq!(depth.value, lanes(ScalarType::Float, 1), "{name}");
        assert_eq!(interface.outputs.len(), 3, "{name}");
    }
}
