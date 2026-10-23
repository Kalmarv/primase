#![allow(clippy::unwrap_used)]
//! Programs of the stages naga has no entry point for, built by hand: a hull program (a control-point function and a
//! patch-constant function), the domain program that reads its patches, and a vertex program that feeds them. They
//! validate against the functions the interface says, are shown as text, and are refused by the WGSL writer by name.
use naga::{Expression, Statement};
use primase_shader::build::{Builder, Kind};
use primase_shader::emit;
use primase_shader::{
    Conventions, D3dModes, DomainFacts, HullFacts, Interface, Label, Partitioning, Program, Provenance,
    ScalarType, ShaderFormat, Stage, StageFacts, SystemValue, TessDomain, TessTopology, Value, Varying,
    VaryingProvenance,
};

fn varying(name: &str, index: u32, width: u8, system: Option<SystemValue>) -> Varying {
    Varying {
        label: Label {
            name: name.into(),
            index,
        },
        system,
        value: Value {
            scalar: ScalarType::Float,
            width,
        },
        interp: None,
        used: true,
        provenance: VaryingProvenance::default(),
    }
}

fn provenance(target: &str) -> Provenance {
    Provenance {
        format: ShaderFormat::Dxbc,
        target: target.into(),
        lifter: "test".into(),
        lifter_version: 0,
        source_hash: None,
    }
}

fn patch_constants() -> Vec<Varying> {
    vec![
        varying("SV_TessFactor", 0, 1, None),
        varying("SV_InsideTessFactor", 0, 1, None),
    ]
}

fn hull_interface() -> Interface {
    Interface {
        stage: Stage::Hull,
        inputs: vec![varying("POSITION", 0, 3, None)],
        outputs: vec![varying("POSITION", 0, 3, None)],
        resources: Vec::new(),
        facts: StageFacts::Hull(HullFacts {
            domain: TessDomain::Triangle,
            partitioning: Partitioning::Integer,
            topology: TessTopology::TriangleCw,
            input_control_points: 3,
            output_control_points: 3,
            max_tess_factor_bits: 16.0f32.to_bits(),
            patch_constants: patch_constants(),
        }),
    }
}

/// `main`: output control point `id` is input control point `id`; `patch_constant`: a tessellation factor of 4 and an
/// inside factor of 2.
fn hull() -> Program {
    let interface = hull_interface();
    let sigs = primase_shader::stage_functions(&interface).unwrap().unwrap();
    let mut b = Builder::new();
    // main(POSITION0: array<vec3<f32>, 3>, SV_OutputControlPointID: u32) -> Out { POSITION0 }
    let decl = b.declare_function(&sigs[0]).unwrap();
    let out = decl.result.unwrap();
    let (points, id) = (
        b.app(Expression::FunctionArgument(0)),
        b.app(Expression::FunctionArgument(1)),
    );
    let point = b.app(Expression::Access {
        base: points,
        index: id,
    });
    let result = b.app(Expression::Compose {
        ty: out,
        components: vec![point],
    });
    b.push(Statement::Return { value: Some(result) });
    b.end_function("main", decl);
    // patch_constant(POSITION0: array, out_POSITION0: array, SV_PrimitiveID) -> Out { SV_TessFactor0, SV_InsideTessFactor0 }
    let decl = b.declare_function(&sigs[1]).unwrap();
    let out = decl.result.unwrap();
    let (four, two) = (b.lit_f(4.0), b.lit_f(2.0));
    let result = b.app(Expression::Compose {
        ty: out,
        components: vec![four.expr, two.expr],
    });
    b.push(Statement::Return { value: Some(result) });
    b.end_function("patch_constant", decl);
    Program {
        module: b.finish_functions(),
        stage: Stage::Hull,
        entry: "main".into(),
        interface,
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance("hs_5_0"),
        notes: Vec::new(),
    }
}

fn domain_interface() -> Interface {
    Interface {
        stage: Stage::Domain,
        inputs: vec![varying("POSITION", 0, 3, None)],
        outputs: vec![varying("SV_Position", 0, 4, Some(SystemValue::ClipPosition))],
        resources: Vec::new(),
        facts: StageFacts::Domain(DomainFacts {
            domain: TessDomain::Triangle,
            input_control_points: 3,
            patch_constants: patch_constants(),
        }),
    }
}

/// `main`: the clip position is the domain location with a w of 1.
fn domain() -> Program {
    let interface = domain_interface();
    let sigs = primase_shader::stage_functions(&interface).unwrap().unwrap();
    let mut b = Builder::new();
    let decl = b.declare_function(&sigs[0]).unwrap();
    let out = decl.result.unwrap();
    // SV_TessFactor0, SV_InsideTessFactor0, POSITION0, SV_DomainLocation, SV_PrimitiveID
    let location = b.app(Expression::FunctionArgument(3));
    let one = b.lit_f(1.0);
    let v4 = b.ty_of(Kind::F, 4);
    let position = b.app(Expression::Compose {
        ty: v4,
        components: vec![location, one.expr],
    });
    let result = b.app(Expression::Compose {
        ty: out,
        components: vec![position],
    });
    b.push(Statement::Return { value: Some(result) });
    b.end_function("main", decl);
    Program {
        module: b.finish_functions(),
        stage: Stage::Domain,
        entry: "main".into(),
        interface,
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: provenance("ds_5_0"),
        notes: Vec::new(),
    }
}

#[test]
fn the_hull_and_domain_programs_validate_against_their_functions() {
    for p in [hull(), domain()] {
        p.validate().unwrap_or_else(|e| panic!("{:?}: {e:?}", p.stage));
    }
}

#[test]
fn the_functions_are_shown_and_no_wgsl_stage_is_written() {
    let text = hull().text().unwrap();
    assert!(text.contains("// stage hull entry main"), "{text}");
    assert!(
        text.contains("// hull domain=Triangle partitioning=Integer topology=TriangleCw control_points=3->3 max_tess_factor=16"),
        "{text}"
    );
    assert!(text.contains("fn patch_constant("), "{text}");
    assert!(text.contains("// patch_constant SV_TessFactor0 float1"), "{text}");
    for p in [hull(), domain()] {
        let error = emit::wgsl(&p).unwrap_err().to_string();
        assert!(error.contains("has no WGSL stage"), "{error}");
    }
}

#[test]
fn a_module_that_is_not_the_stages_functions_is_refused() {
    // an entry point where there should be functions, and a function with the wrong argument
    let mut p = hull();
    p.entry = "other".into();
    let problems = p
        .validate()
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        problems.iter().any(|m| m.contains("not one of its functions")),
        "{problems:?}"
    );
    let mut p = domain();
    if let StageFacts::Domain(d) = &mut p.interface.facts {
        d.input_control_points = 4;
    }
    let problems = p
        .validate()
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(
        problems.iter().any(|m| m.contains("POSITION0's type")),
        "{problems:?}"
    );
    let mut p = hull();
    p.interface.facts = StageFacts::None;
    assert!(
        p.validate()
            .unwrap_err()
            .iter()
            .any(|m| m.to_string().contains("without its patch facts"))
    );
}
