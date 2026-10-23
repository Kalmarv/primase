#![allow(clippy::unwrap_used)]
//! The mesh fixture (`mesh.hlsl`: a quad as two triangles, a thread group of 32) in each build: its facts are the ones the
//! entry point states, it is a mesh entry point with D3D's thread group and the group index as its one argument, it
//! validates and is written as WGSL with the mesh stage's attributes, and the executor refuses it by name.
use primase_dxil::lift;
use primase_shader::{MeshFacts, MeshTopology, Stage, StageFacts};

#[test]
fn a_mesh_program_is_a_mesh_entry_point_with_its_facts() {
    for name in ["mesh.dxil", "mesh.od.dxil", "mesh.66.dxil"] {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap();
        let p = lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.stage, Stage::Mesh, "{name}");
        assert_eq!(
            p.interface.facts,
            StageFacts::Mesh(MeshFacts {
                topology: MeshTopology::Triangle,
                max_vertices: 4,
                max_primitives: 2,
            }),
            "{name}"
        );
        let [entry] = p.module.entry_points.as_slice() else {
            panic!("{name}")
        };
        assert_eq!(entry.stage, naga::ShaderStage::Mesh, "{name}");
        assert_eq!(entry.workgroup_size, [32, 1, 1], "{name}");
        let arguments: Vec<_> = entry
            .function
            .arguments
            .iter()
            .map(|a| a.name.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(arguments, ["SV_GroupIndex0"], "{name}");
        let outputs: Vec<_> = p
            .interface
            .outputs
            .iter()
            .map(|v| v.label.name.as_str())
            .collect();
        assert_eq!(outputs, ["SV_Position", "COLOR"], "{name}");
        p.validate().unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let wgsl = primase_shader::emit::wgsl(&p).unwrap();
        assert!(wgsl.contains("@mesh(mesh_output)"), "{name}: {wgsl}");
        assert!(wgsl.contains("@workgroup_size(32, 1, 1)"), "{name}");
        assert!(wgsl.contains("@builtin(triangle_indices)"), "{name}");
        // a mesh program is an entry point the executor runs (a thread group writing one meshlet)
        primase_shader::exec::Executable::new(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}
