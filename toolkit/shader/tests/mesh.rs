#![allow(clippy::unwrap_used)]
//! A mesh program built by hand: a thread group of 32 that sets its counts (4 vertices, 2 triangles), writes the position of
//! the vertex its group index names and the indices of the first triangle. It validates, is written as WGSL with the mesh
//! stage's attributes, and the executor refuses it by name.
use naga::{Expression, Statement};
use primase_shader::build::Builder;
use primase_shader::{
    Conventions, D3dModes, Interface, Label, MeshFacts, MeshTopology, Program, Provenance, ScalarType,
    ShaderFormat, Stage, StageFacts, SystemValue, Value, Varying, VaryingProvenance, emit, entry_io,
    mesh_vertex_io,
};

fn varying(name: &str, system: Option<SystemValue>, scalar: ScalarType, width: u8) -> Varying {
    Varying {
        label: Label {
            name: name.into(),
            index: 0,
        },
        system,
        value: Value { scalar, width },
        interp: None,
        used: true,
        provenance: VaryingProvenance::default(),
    }
}

fn interface() -> Interface {
    Interface {
        stage: Stage::Mesh,
        inputs: vec![varying(
            "SV_GroupIndex",
            Some(SystemValue::GroupIndex),
            ScalarType::Uint,
            1,
        )],
        outputs: vec![
            varying("COLOR", None, ScalarType::Float, 3),
            varying(
                "SV_Position",
                Some(SystemValue::ClipPosition),
                ScalarType::Float,
                4,
            ),
        ],
        resources: Vec::new(),
        facts: StageFacts::Mesh(MeshFacts {
            topology: MeshTopology::Triangle,
            max_vertices: 4,
            max_primitives: 2,
        }),
    }
}

fn mesh() -> Program {
    let interface = interface();
    let facts = MeshFacts {
        topology: MeshTopology::Triangle,
        max_vertices: 4,
        max_primitives: 2,
    };
    let io = entry_io(&interface).unwrap();
    assert!(io.results.is_empty());
    assert_eq!(io.arguments.len(), 1);
    let vertex = mesh_vertex_io(&interface).unwrap();
    assert_eq!(vertex.len(), 2);
    let mut b = Builder::new();
    b.workgroup_size = [32, 1, 1];
    let declared = b.declare_entry(&io).unwrap();
    let output = b.declare_mesh_output(&facts, &vertex).unwrap();
    let (four, two, zero, one, two_u) = (b.lit_u(4), b.lit_u(2), b.lit_u(0), b.lit_u(1), b.lit_u(2));
    let global = b.global_ptr(output.global);
    let counts = [(output.vertex_count, four), (output.primitive_count, two)];
    for (member, value) in counts {
        let p = b.app(Expression::AccessIndex {
            base: global,
            index: member,
        });
        b.store(p, value);
    }
    // vertices[group index].SV_Position.x = 1.0
    let index = b.app(Expression::FunctionArgument(0));
    let vertices = b.app(Expression::AccessIndex {
        base: global,
        index: output.vertices,
    });
    let element = b.app(Expression::Access {
        base: vertices,
        index,
    });
    let position = b.app(Expression::AccessIndex {
        base: element,
        index: 1,
    });
    let x = b.app(Expression::AccessIndex {
        base: position,
        index: 0,
    });
    let value = b.lit_f(1.0);
    b.store(x, value);
    // primitives[0].indices = (0, 1, 2)
    let primitives = b.app(Expression::AccessIndex {
        base: global,
        index: output.primitives,
    });
    let first = b.app(Expression::AccessIndex {
        base: primitives,
        index: 0,
    });
    let indices = b.app(Expression::AccessIndex {
        base: first,
        index: output.indices_member,
    });
    let triangle = b.compose(&[zero, one, two_u]);
    b.store(indices, triangle);
    b.push(Statement::Return { value: None });
    let module = b.finish_mesh(declared.arguments, &facts, &output);
    Program {
        module,
        stage: Stage::Mesh,
        entry: "main".into(),
        interface,
        bindings: Vec::new(),
        conventions: Conventions::D3D,
        modes: D3dModes::ALL,
        provenance: Provenance {
            format: ShaderFormat::Dxil,
            target: "ms_6_5".into(),
            lifter: "test".into(),
            lifter_version: 0,
            source_hash: None,
        },
        notes: Vec::new(),
    }
}

#[test]
fn a_mesh_program_validates_and_is_written_with_the_mesh_stage() {
    let p = mesh();
    p.validate().unwrap_or_else(|e| panic!("{e:?}"));
    let wgsl = emit::wgsl(&p).unwrap();
    assert!(wgsl.contains("enable wgpu_mesh_shader;"), "{wgsl}");
    assert!(wgsl.contains("@mesh(mesh_output)"), "{wgsl}");
    assert!(wgsl.contains("@workgroup_size(32, 1, 1)"), "{wgsl}");
    assert!(
        wgsl.contains("@builtin(triangle_indices) indices: vec3<u32>"),
        "{wgsl}"
    );
    assert!(
        wgsl.contains("@builtin(vertices) vertices: array<MeshVertex, 4>"),
        "{wgsl}"
    );
    let text = p.text().unwrap();
    assert!(text.contains("// stage mesh entry main"), "{text}");
    assert!(
        text.contains("// mesh topology=Triangle max_vertices=4 max_primitives=2"),
        "{text}"
    );
}

#[test]
fn the_executor_refuses_a_mesh_program_by_name() {
    let e = primase_shader::exec::Executable::new(&mesh())
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("Mesh program") && e.contains("mesh shaders"), "{e}");
}

#[test]
fn a_mesh_program_without_its_facts_or_with_none_to_write_is_refused() {
    let mut p = mesh();
    p.interface.facts = StageFacts::None;
    assert!(
        p.interface
            .verify()
            .unwrap_err()
            .contains("mesh program without its facts")
    );
    let mut p = mesh();
    p.interface.facts = StageFacts::Mesh(MeshFacts {
        topology: MeshTopology::Line,
        max_vertices: 300,
        max_primitives: 1,
    });
    assert!(p.interface.verify().unwrap_err().contains("300 vertices"));
}
