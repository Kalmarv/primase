#![allow(clippy::unwrap_used)]
//! Ray tracing programs built by hand: a closest-hit program that writes the hit distance into its payload, and a ray
//! generation program that traces a ray. They validate against the function the interface says (the ray's system values
//! then pointers to the payload and the attributes), the pipeline's operations are functions without a body, and the
//! WGSL writer and the executor refuse them by name.
use iridia::build::{Builder, Kind};
use iridia::{
    Conventions, D3dBinding, D3dClass, D3dModes, Interface, Label, Program, Provenance, Resource, ResourceId,
    ResourceKind, ResourceProvenance, ScalarType, ShaderFormat, Stage, StageFacts, SystemValue, Value,
    Varying, VaryingProvenance, emit, rt,
};
use naga::{AddressSpace, Expression, FunctionArgument, Handle, Statement, StructMember, Type, TypeInner};

fn system_input(name: &str, system: SystemValue, scalar: ScalarType, width: u8) -> Varying {
    Varying {
        label: Label {
            name: name.into(),
            index: 0,
        },
        system: Some(system),
        value: Value { scalar, width },
        interp: None,
        used: true,
        provenance: VaryingProvenance::default(),
    }
}

fn provenance() -> Provenance {
    Provenance {
        format: ShaderFormat::Dxil,
        target: "lib_6_6".into(),
        lifter: "test".into(),
        lifter_version: 0,
        source_hash: None,
        instruction_count: None,
    }
}

/// A structure of one float and one `u32`, as a payload.
fn payload(b: &mut Builder) -> Handle<Type> {
    let f = b.ty_of(Kind::F, 1);
    let u = b.ty_of(Kind::U, 1);
    b.module.types.insert(
        Type {
            name: Some("Payload".into()),
            inner: TypeInner::Struct {
                members: vec![
                    StructMember {
                        name: Some("m0".into()),
                        ty: f,
                        binding: None,
                        offset: 0,
                    },
                    StructMember {
                        name: Some("m1".into()),
                        ty: u,
                        binding: None,
                        offset: 4,
                    },
                ],
                span: 8,
            },
        },
        naga::Span::UNDEFINED,
    )
}

/// `main(SV_RayTCurrent0, payload, attributes)`: `payload.m0 = SV_RayTCurrent0`.
fn closest_hit() -> Program {
    let interface = Interface {
        stage: Stage::ClosestHit,
        inputs: vec![system_input(
            "SV_RayTCurrent",
            SystemValue::RayTCurrent,
            ScalarType::Float,
            1,
        )],
        outputs: Vec::new(),
        resources: Vec::new(),
        facts: StageFacts::None,
    };
    let sigs = iridia::stage_functions(&interface).unwrap().unwrap();
    let names: Vec<_> = sigs[0].arguments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["SV_RayTCurrent0", "payload", "attributes"]);
    let mut b = Builder::new();
    let (payload, attributes) = (payload(&mut b), payload(&mut b));
    let decl = b.declare_function_with(&sigs[0], &[payload, attributes]).unwrap();
    let t = b.app(Expression::FunctionArgument(0));
    let pointer = b.app(Expression::FunctionArgument(1));
    let field = b.app(Expression::AccessIndex {
        base: pointer,
        index: 0,
    });
    b.push(Statement::Store {
        pointer: field,
        value: t,
    });
    b.push(Statement::Return { value: None });
    b.end_function("main", decl);
    Program {
        module: b.finish_functions(),
        stage: Stage::ClosestHit,
        entry: "main".into(),
        interface,
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

/// `main()`: a local payload and `trace_ray(...)` with it.
fn ray_generation() -> Program {
    let interface = Interface {
        stage: Stage::RayGeneration,
        inputs: Vec::new(),
        outputs: Vec::new(),
        // the acceleration structure the program traces into, t0
        resources: vec![Resource {
            id: ResourceId(0),
            kind: ResourceKind::AccelerationStructure,
            count: 1,
            provenance: ResourceProvenance {
                name: "scene".into(),
                d3d: Some(D3dBinding {
                    class: D3dClass::ShaderResource,
                    register: 0,
                    space: 0,
                }),
                counter_of: None,
                layout: None,
            },
        }],
        facts: StageFacts::None,
    };
    let sigs = iridia::stage_functions(&interface).unwrap().unwrap();
    assert!(sigs[0].arguments.is_empty());
    let mut b = Builder::new();
    let payload = payload(&mut b);
    let accel = b.ty(TypeInner::AccelerationStructure { vertex_return: false });
    let (u, f, v3) = (b.ty_of(Kind::U, 1), b.ty_of(Kind::F, 1), b.ty_of(Kind::F, 3));
    let pointer = b.ty(TypeInner::Pointer {
        base: payload,
        space: AddressSpace::Function,
    });
    let argument = |name: &str, ty| FunctionArgument {
        name: Some(name.into()),
        ty,
        binding: None,
    };
    let trace = b.declare_operation(
        rt::TRACE_RAY,
        vec![
            argument("acceleration_structure", accel),
            argument("ray_flags", u),
            argument("instance_mask", u),
            argument("hit_group_offset", u),
            argument("hit_group_stride", u),
            argument("miss_index", u),
            argument("origin", v3),
            argument("t_min", f),
            argument("direction", v3),
            argument("t_max", f),
            argument("payload", pointer),
        ],
        false,
    );
    let decl = b.declare_function(&sigs[0]).unwrap();
    // the acceleration structure is a global of the module
    let global = b.global(naga::GlobalVariable {
        name: Some("scene".into()),
        space: AddressSpace::Handle,
        binding: Some(naga::ResourceBinding { group: 1, binding: 0 }),
        ty: accel,
        init: None,
        memory_decorations: Default::default(),
    });
    let local = b.local("payload", payload);
    let (scene, local) = (b.global_ptr(global), b.local_ptr(local));
    let (flags, mask, zero, one) = (b.lit_u(1), b.lit_u(255), b.lit_u(0), b.lit_u(1));
    let (t_min, t_max) = (b.lit_f(0.0), b.lit_f(1000.0));
    let (x, y, z) = (b.lit_f(0.0), b.lit_f(1.0), b.lit_f(0.0));
    let origin = b.compose(&[x, x, x]);
    let direction = b.compose(&[x, y, z]);
    b.push(Statement::Call {
        function: trace,
        arguments: vec![
            scene,
            flags.expr,
            mask.expr,
            zero.expr,
            one.expr,
            zero.expr,
            origin.expr,
            t_min.expr,
            direction.expr,
            t_max.expr,
            local,
        ],
        result: None,
    });
    b.push(Statement::Return { value: None });
    b.end_function("main", decl);
    Program {
        module: b.finish_functions(),
        stage: Stage::RayGeneration,
        entry: "main".into(),
        interface,
        bindings: vec![(ResourceId(0), global)],
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance(),
        notes: Vec::new(),
    }
}

fn problems(p: &Program) -> Vec<String> {
    p.validate()
        .err()
        .unwrap_or_default()
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_closest_hit_program_validates_and_is_shown_as_text() {
    let p = closest_hit();
    assert_eq!(problems(&p), Vec::<String>::new());
    let text = p.text().unwrap();
    assert!(text.contains("// stage closest_hit entry main"), "{text}");
    assert!(text.contains("system=RayTCurrent"), "{text}");
    assert!(text.contains("payload: ptr<function,"), "{text}");
}

#[test]
fn a_ray_generation_program_traces_through_an_operation_without_a_body() {
    let p = ray_generation();
    assert_eq!(problems(&p), Vec::<String>::new());
    assert_eq!(p.module.functions.len(), 2);
    let text = p.text().unwrap();
    assert!(text.contains("trace_ray("), "{text}");
}

#[test]
fn an_operation_with_a_body_is_a_problem() {
    let mut p = ray_generation();
    let (_, f) = p
        .module
        .functions
        .iter_mut()
        .find(|(_, f)| f.name.as_deref() == Some(rt::TRACE_RAY))
        .unwrap();
    f.body.push(Statement::Kill, naga::Span::UNDEFINED);
    let found = problems(&p);
    assert!(found.iter().any(|m| m.contains("has a body")), "{found:?}");
}

#[test]
fn a_payload_that_is_not_a_pointer_is_a_problem() {
    let mut p = closest_hit();
    let float = {
        let mut b = Builder::new();
        b.module = std::mem::take(&mut p.module);
        let t = b.ty_of(Kind::F, 1);
        p.module = b.module;
        t
    };
    let (_, f) = p.module.functions.iter_mut().next().unwrap();
    f.arguments[1].ty = float;
    let found = problems(&p);
    assert!(found.iter().any(|m| m.contains("payload")), "{found:?}");
}

#[test]
fn a_ray_tracing_program_is_refused_by_name_by_the_writer_and_as_an_entry_point_by_the_executor() {
    for p in [closest_hit(), ray_generation()] {
        let wgsl = emit::wgsl(&p).unwrap_err().to_string();
        assert!(wgsl.contains("no WGSL stage"), "{wgsl}");
        let functions = emit::wgsl_functions(&p).unwrap_err().to_string();
        assert!(functions.contains("ray tracing pipeline"), "{functions}");
        let exec = iridia_exec::Executable::new(&p).err().unwrap().to_string();
        assert!(exec.contains("ray tracing pipeline"), "{exec}");
        // its function compiles: the pipeline runs it (the hand-built programs here have no real body to run)
        let _ = iridia_exec::Executable::function(&p, "main");
    }
}
