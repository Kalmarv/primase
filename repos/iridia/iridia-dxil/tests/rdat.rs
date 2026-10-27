#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The subobjects of a library's runtime data as dxc wrote them, mapped into the interface vocabulary
//! (`iridia_dxil::subobjects`), and carried by every program of the library the lift makes.
use iridia::{HitGroup, HitGroupKind, Subobject};
use iridia_dxil::subobjects;

/// `ray_subobjects.hlsl` as dxc compiled it (the whole library and one program of it each, `-exports`): the runtime data
/// dxc wrote reads to the subobjects the source declares, every program of the library carries them, and the lifted
/// program's facts name its own export.
#[test]
fn the_subobjects_dxc_wrote_are_the_ones_the_source_declares() {
    use iridia::StageFacts;
    let fixture = |name: &str| {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    };
    let bytes = fixture("ray_subobjects.dxil");
    let chunks = dxcontainer::chunks(&bytes).unwrap();
    let rdat = chunks.iter().find(|c| &c.kind == b"RDAT").unwrap();
    let found = subobjects(rdat.data).unwrap();
    let names: Vec<_> = found
        .iter()
        .map(|s| match s {
            Subobject::StateObjectConfig { name, .. }
            | Subobject::GlobalRootSignature { name, .. }
            | Subobject::LocalRootSignature { name, .. }
            | Subobject::ExportsAssociation { name, .. }
            | Subobject::ShaderConfig { name, .. }
            | Subobject::PipelineConfig { name, .. } => name.as_str(),
            Subobject::HitGroup(g) => g.name.as_str(),
        })
        .collect();
    assert_eq!(
        names,
        [
            "state_config",
            "global_signature",
            "local_signature",
            "association",
            "shader_config",
            "pipeline_config",
            "triangle_group",
            "procedural_group"
        ]
    );
    // the flag is dxc's ALLOW_LOCAL_DEPENDENCIES_ON_EXTERNAL_DEFINITIONS (1), a serialised root signature is version 2
    // (its first word) of a 48-byte description
    assert!(matches!(&found[0], Subobject::StateObjectConfig { flags: 1, .. }));
    for s in &found[1..3] {
        let (Subobject::GlobalRootSignature { signature, .. }
        | Subobject::LocalRootSignature { signature, .. }) = s
        else {
            panic!("{s:?}")
        };
        assert_eq!((signature.len(), signature[..4].to_vec()), (48, vec![2, 0, 0, 0]));
    }
    assert_eq!(
        found[3..],
        [
            Subobject::ExportsAssociation {
                name: "association".into(),
                subobject: "local_signature".into(),
                exports: vec!["closest_hit".into(), "miss".into()]
            },
            Subobject::ShaderConfig {
                name: "shader_config".into(),
                max_payload_bytes: 32,
                max_attribute_bytes: 8
            },
            Subobject::PipelineConfig {
                name: "pipeline_config".into(),
                max_recursion_depth: 2,
                flags: None
            },
            Subobject::HitGroup(HitGroup {
                name: "triangle_group".into(),
                kind: HitGroupKind::Triangles,
                any_hit: None,
                closest_hit: Some("closest_hit".into()),
                intersection: None
            }),
            Subobject::HitGroup(HitGroup {
                name: "procedural_group".into(),
                kind: HitGroupKind::ProceduralPrimitives,
                any_hit: Some("any_hit".into()),
                closest_hit: Some("closest_hit".into()),
                intersection: Some("intersection".into())
            }),
        ]
    );
    for export in ["closest_hit", "any_hit", "intersection", "miss"] {
        let program = iridia_dxil::lift(&fixture(&format!("ray_subobjects.{export}.dxil"))).unwrap();
        let StageFacts::RayTracing(facts) = &program.interface.facts else {
            panic!("{export}: {:?}", program.interface.facts)
        };
        assert_eq!(facts.export, export);
        assert_eq!(facts.subobjects, found, "{export}");
    }
}
