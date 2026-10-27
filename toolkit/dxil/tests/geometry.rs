#![allow(clippy::unwrap_used)]
//! The geometry fixture (`geometry.hlsl`: a triangle in, two strips out, two instances) in each build: its facts are the
//! ones the entry point states, its function is the one `stage_functions` describes (the primitive's vertices as arrays,
//! the primitive and instance numbers; the emitted vertices as arrays, their count and where strips start), and it
//! validates and is written as WGSL functions.
use primase_dxil::lift;
use iridia::{GsInput, GsOutput, Stage, StageFacts, stage_functions};

#[test]
fn a_geometry_program_is_the_function_its_facts_describe() {
    for name in ["geometry.dxil", "geometry.od.dxil", "geometry.66.dxil"] {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap();
        let p = lift(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.stage, Stage::Geometry, "{name}");
        let StageFacts::Geometry(g) = &p.interface.facts else {
            panic!("{name}: {:?}", p.interface.facts)
        };
        assert_eq!(
            (g.input, g.output, g.max_vertices, g.instances),
            (GsInput::Triangle, GsOutput::TriangleStrip, 6, 2),
            "{name}"
        );
        let sigs = stage_functions(&p.interface).unwrap().unwrap();
        let [main] = sigs.as_slice() else { panic!("{name}") };
        let arguments: Vec<_> = main.arguments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            arguments,
            ["SV_Position0", "TEXCOORD0", "SV_PrimitiveID", "SV_GSInstanceID"],
            "{name}"
        );
        let results: Vec<_> = main.result.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            results,
            ["SV_Position0", "TEXCOORD0", "SV_VertexCount", "SV_StripStart"],
            "{name}"
        );
        p.validate().unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let wgsl = iridia::emit::wgsl_functions(&p).unwrap();
        assert!(wgsl.contains("fn main("), "{name}: {wgsl}");
        assert!(wgsl.contains("SV_VertexCount"), "{name}");
    }
}
