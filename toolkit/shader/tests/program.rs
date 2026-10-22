#![allow(clippy::unwrap_used)]
//! The program representation on a hand-built pixel program (`uv * scale`, with a front-facing flag): the validator
//! accepts it, finds each way the module and the interface can disagree, and the text and WGSL come out as worked out
//! from the construction.
use naga::{
    AddressSpace, BinaryOperator, Binding, FunctionArgument, GlobalVariable, ResourceBinding, StructMember,
    TypeInner,
};
use primase_shader::build::{Builder, Kind};
use primase_shader::{
    BufferLayout, Conventions, D3dModes, Interface, Label, Problem, Program, Provenance, Resource,
    ResourceId, ResourceKind, ResourceProvenance, ScalarType, ShaderFormat, Stage, SystemValue, Value,
    Varying, VaryingProvenance,
};

fn varying(name: &str, index: u32, system: Option<SystemValue>, scalar: ScalarType, width: u8) -> Varying {
    Varying {
        label: Label {
            name: name.into(),
            index,
        },
        system,
        value: Value { scalar, width },
        interp: None,
        used: true,
        provenance: VaryingProvenance::default(),
    }
}

/// `SV_Target0 = float4(TEXCOORD0 * cb0[0].xy, face ? 1 : 0, 1)`
fn program() -> Program {
    let mut b = Builder::new();
    let u4 = b.ty_of(Kind::U, 4);
    let cb_ty = b.ty(TypeInner::Array {
        base: u4,
        size: naga::ArraySize::Constant(std::num::NonZeroU32::new(1).unwrap()),
        stride: 16,
    });
    let cb = b.global(GlobalVariable {
        name: Some("cb0".into()),
        space: AddressSpace::Uniform,
        binding: Some(ResourceBinding { group: 0, binding: 0 }),
        ty: cb_ty,
        init: None,
        memory_decorations: Default::default(),
    });
    let f2 = b.ty_of(Kind::F, 2);
    let boolean = b.ty_of(Kind::B, 1);
    let f4 = b.ty_of(Kind::F, 4);
    let uv = b.app(naga::Expression::FunctionArgument(0));
    let uv = primase_shader::build::Val {
        expr: uv,
        kind: Kind::F,
        width: 2,
    };
    let face = b.app(naga::Expression::FunctionArgument(1));
    let face = primase_shader::build::Val {
        expr: face,
        kind: Kind::B,
        width: 1,
    };
    let ptr = b.global_ptr(cb);
    let zero = b.lit_u(0);
    let row = b.app(naga::Expression::Access {
        base: ptr,
        index: zero.expr,
    });
    let row = b.load(row, Kind::U, 4);
    let scale = b.bitcast(row, Kind::F);
    let scale = b.swizzle(scale, &[0, 1]);
    let product = b.binary(BinaryOperator::Multiply, uv, scale);
    let one = b.lit_f(1.0);
    let nothing = b.lit_f(0.0);
    let facing = b.select(face, one, nothing);
    let x = b.lane(product, 0);
    let y = b.lane(product, 1);
    let colour = b.compose(&[x, y, facing, one]);
    let out_ty = b.ty(TypeInner::Struct {
        members: vec![StructMember {
            name: Some("SV_Target0".into()),
            ty: f4,
            binding: Some(Binding::Location {
                location: 0,
                interpolation: None,
                sampling: None,
                blend_src: None,
                per_primitive: false,
            }),
            offset: 0,
        }],
        span: 16,
    });
    let out = b.app(naga::Expression::Compose {
        ty: out_ty,
        components: vec![colour.expr],
    });
    b.push(naga::Statement::Return { value: Some(out) });
    let module = b.finish(
        "main",
        naga::ShaderStage::Fragment,
        vec![
            FunctionArgument {
                name: Some("TEXCOORD0".into()),
                ty: f2,
                binding: Some(Binding::Location {
                    location: 0,
                    interpolation: Some(naga::Interpolation::Perspective),
                    sampling: None,
                    blend_src: None,
                    per_primitive: false,
                }),
            },
            FunctionArgument {
                name: Some("SV_IsFrontFace0".into()),
                ty: boolean,
                binding: Some(Binding::BuiltIn(naga::BuiltIn::FrontFacing)),
            },
        ],
        Some(out_ty),
        false,
    );
    let mut texcoord = varying("TEXCOORD", 0, None, ScalarType::Float, 2);
    texcoord.interp = Some(primase_shader::Interpolation {
        mode: primase_shader::InterpMode::Perspective,
        sampling: primase_shader::InterpSampling::Center,
    });
    let cb_global = module.global_variables.iter().next().unwrap().0;
    Program {
        module,
        stage: Stage::Pixel,
        entry: "main".into(),
        interface: Interface {
            stage: Stage::Pixel,
            inputs: vec![
                texcoord,
                varying(
                    "SV_IsFrontFace",
                    0,
                    Some(SystemValue::FrontFacing),
                    ScalarType::Uint,
                    1,
                ),
            ],
            outputs: vec![varying(
                "SV_Target",
                0,
                Some(SystemValue::Target(0)),
                ScalarType::Float,
                3,
            )],
            resources: vec![Resource {
                id: ResourceId(0),
                kind: ResourceKind::ConstantBuffer {
                    layout: BufferLayout {
                        size: 16,
                        constants: vec![],
                    },
                },
                count: 1,
                provenance: ResourceProvenance {
                    name: "cb0".into(),
                    d3d: None,
                },
            }],
        },
        bindings: vec![(ResourceId(0), cb_global)],
        conventions: Conventions::D3D,
        modes: D3dModes::default(),
        provenance: Provenance {
            format: ShaderFormat::Dxbc,
            target: "ps_5_0".into(),
            lifter: "test".into(),
            lifter_version: 0,
            source_hash: None,
        },
    }
}

fn problems(p: &Program) -> Vec<String> {
    p.validate()
        .err()
        .unwrap_or_default()
        .iter()
        .map(Problem::to_string)
        .collect()
}

#[test]
fn a_consistent_program_validates_and_writes() {
    let p = program();
    assert_eq!(problems(&p), Vec::<String>::new());
    // SV_Target0 is a float3 element in the interface but a four-lane colour in the module
    let wgsl = primase_shader::emit::wgsl(&p).unwrap();
    assert!(wgsl.contains("@location(0) SV_Target0_: vec4<f32>"), "{wgsl}");
    assert!(
        wgsl.contains("@builtin(front_facing) SV_IsFrontFace0_: bool"),
        "{wgsl}"
    );
    assert!(wgsl.contains("@group(0) @binding(0)"), "{wgsl}");
}

#[test]
fn the_text_form_is_stable_and_states_what_the_module_cannot() {
    let p = program();
    let text = p.text().unwrap();
    assert_eq!(text, p.text().unwrap());
    for line in [
        "// primase shader program v1",
        "// stage pixel entry main",
        "// source Dxbc ps_5_0 lifted by test 0",
        "// conventions clip_depth=0..1 fragment_origin=top_left texture_origin=top_left",
        "// input TEXCOORD0 float2 perspective/center",
        "// input SV_IsFrontFace0 uint1 system=FrontFacing",
        "// output SV_Target0 float3 system=target0",
        "// resource 0 \"cb0\" constant_buffer size=16",
    ] {
        assert!(text.lines().any(|l| l == line), "missing {line:?} in\n{text}");
    }
}

#[test]
fn each_disagreement_between_module_and_interface_is_found() {
    // an argument renamed, an interface input the module lacks, a label repeated
    let mut p = program();
    p.module.entry_points[0].function.arguments[0].name = Some("UV".into());
    assert!(
        problems(&p)
            .iter()
            .any(|m| m.contains("input TEXCOORD0: the argument is named"))
    );

    let mut p = program();
    p.interface.inputs.pop();
    assert!(
        problems(&p)
            .iter()
            .any(|m| m.contains("2 entry arguments for 1 input"))
    );

    let mut p = program();
    let mut again = p.interface.inputs[0].clone();
    again.label.index = 0;
    p.interface.inputs.push(again);
    assert!(
        problems(&p)
            .iter()
            .any(|m| m.contains("input label TEXCOORD0 repeats"))
    );

    // a varying whose width differs from the module's
    let mut p = program();
    p.interface.inputs[0].value.width = 3;
    assert!(
        problems(&p)
            .iter()
            .any(|m| m.contains("the argument's type is not"))
    );

    // a resource of the wrong kind for its global, and a global that is no resource
    let mut p = program();
    p.interface.resources[0].kind = ResourceKind::Sampler { comparison: false };
    assert!(problems(&p).iter().any(|m| m.contains("resource 0 (cb0)")));

    let mut p = program();
    p.interface.resources.clear();
    p.bindings.clear();
    assert!(
        problems(&p)
            .iter()
            .any(|m| m.contains("is bound but is no resource"))
    );

    // a constant buffer smaller than its layout
    let mut p = program();
    if let ResourceKind::ConstantBuffer { layout } = &mut p.interface.resources[0].kind {
        layout.size = 32;
    }
    assert!(problems(&p).iter().any(|m| m.contains("resource 0 (cb0)")));

    // a system value no stage binding carries
    let mut p = program();
    p.interface.inputs[1].system = Some(SystemValue::ViewportArrayIndex);
    assert!(problems(&p).iter().any(|m| m.contains("has no built-in binding")));
}

#[test]
fn a_module_naga_refuses_is_reported_as_the_modules() {
    let mut p = program();
    // a second entry point of the same name
    let again = p.module.entry_points[0].clone();
    p.module.entry_points.push(again);
    let found = problems(&p);
    assert!(found.iter().any(|m| m.contains("2 entry points")), "{found:?}");
}
