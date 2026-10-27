#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! The subobjects of a library's runtime data, read from data built by hand to the layout `primase_dxil::rdat` states: every
//! kind, the references (strings, index arrays, raw bytes), and the refusals.
use iridia::{HitGroup, HitGroupKind, Subobject};
use primase_dxil::rdat::{export_name, subobjects};

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// An RDAT of these parts, as (type, bytes) pairs, each padded to a multiple of four.
fn rdat(parts: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = words(&[0x10, parts.len() as u32]);
    let mut offset = 8 + 4 * parts.len();
    let mut body = Vec::new();
    for (kind, bytes) in parts {
        out.extend(words(&[offset as u32]));
        let mut padded = bytes.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        body.extend(words(&[*kind, padded.len() as u32]));
        body.extend(padded.clone());
        offset += 8 + padded.len();
    }
    out.extend(body);
    out
}

/// The strings `names` as a buffer (an empty string first, at 0) and the offset of each.
fn strings(names: &[&str]) -> (Vec<u8>, Vec<u32>) {
    let mut buffer = vec![0u8];
    let mut at = Vec::new();
    for n in names {
        at.push(buffer.len() as u32);
        buffer.extend(n.as_bytes());
        buffer.push(0);
    }
    (buffer, at)
}

const STRIDE: u32 = 6;

fn table(records: &[[u32; 6]]) -> Vec<u8> {
    let mut w = vec![records.len() as u32, STRIDE * 4];
    for r in records {
        w.extend(r);
    }
    words(&w)
}

fn library() -> Vec<u8> {
    let (buffer, s) = strings(&[
        "state",
        "global",
        "local",
        "assoc",
        "config",
        "pipeline",
        "triangles",
        "procedural",
        "any_hit",
        "closest_hit",
        "intersection",
        "miss",
        "pipeline1",
    ]);
    // the index array: two exports, "closest_hit" and "miss", at word 1 (word 0 is a count of nothing)
    let arrays = words(&[0, 2, s[9], s[11]]);
    let raw = vec![0xde, 0xad, 0xbe, 0xef, 1, 2, 3];
    rdat(&[
        (1, buffer),
        (2, arrays),
        (5, raw),
        (
            6,
            table(&[
                [0, s[0], 0x20, 0, 0, 0],
                [1, s[1], 0, 4, 0, 0],
                [2, s[2], 4, 3, 0, 0],
                [8, s[3], s[2], 1, 0, 0],
                [9, s[4], 32, 8, 0, 0],
                [10, s[5], 2, 0, 0, 0],
                [11, s[6], 0, 0, s[9], 0],
                [11, s[7], 1, s[8], s[9], s[10]],
                [12, s[12], 3, 5, 0, 0],
            ]),
        ),
    ])
}

#[test]
fn every_kind_of_subobject_reads() {
    let found = subobjects(&library()).unwrap();
    assert_eq!(
        found,
        [
            Subobject::StateObjectConfig {
                name: "state".into(),
                flags: 0x20
            },
            Subobject::GlobalRootSignature {
                name: "global".into(),
                signature: vec![0xde, 0xad, 0xbe, 0xef]
            },
            Subobject::LocalRootSignature {
                name: "local".into(),
                signature: vec![1, 2, 3]
            },
            Subobject::ExportsAssociation {
                name: "assoc".into(),
                subobject: "local".into(),
                exports: vec!["closest_hit".into(), "miss".into()]
            },
            Subobject::ShaderConfig {
                name: "config".into(),
                max_payload_bytes: 32,
                max_attribute_bytes: 8
            },
            Subobject::PipelineConfig {
                name: "pipeline".into(),
                max_recursion_depth: 2,
                flags: None
            },
            Subobject::HitGroup(HitGroup {
                name: "triangles".into(),
                kind: HitGroupKind::Triangles,
                any_hit: None,
                closest_hit: Some("closest_hit".into()),
                intersection: None
            }),
            Subobject::HitGroup(HitGroup {
                name: "procedural".into(),
                kind: HitGroupKind::ProceduralPrimitives,
                any_hit: Some("any_hit".into()),
                closest_hit: Some("closest_hit".into()),
                intersection: Some("intersection".into())
            }),
            Subobject::PipelineConfig {
                name: "pipeline1".into(),
                max_recursion_depth: 3,
                flags: Some(5)
            },
        ]
    );
}

#[test]
fn a_library_without_a_subobject_table_has_no_subobjects() {
    let (buffer, _) = strings(&["x"]);
    assert_eq!(subobjects(&rdat(&[(1, buffer)])).unwrap(), []);
}

#[test]
fn what_is_outside_the_layout_is_refused_by_name() {
    let mut other_version = library();
    other_version[0] = 0x11;
    assert!(
        subobjects(&other_version)
            .unwrap_err()
            .to_string()
            .contains("version")
    );
    let (buffer, s) = strings(&["x"]);
    let kind = rdat(&[(1, buffer.clone()), (6, table(&[[13, s[0], 0, 0, 0, 0]]))]);
    assert!(subobjects(&kind).unwrap_err().to_string().contains("kind 13"));
    let reserved = rdat(&[(1, buffer.clone()), (6, table(&[[3, s[0], 0, 0, 0, 0]]))]);
    assert!(
        subobjects(&reserved)
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    let outside = rdat(&[(1, buffer.clone()), (6, table(&[[0, 9999, 0, 0, 0, 0]]))]);
    assert!(subobjects(&outside).is_err());
    let signature = rdat(&[(1, buffer), (5, vec![0; 4]), (6, table(&[[1, s[0], 2, 9, 0, 0]]))]);
    assert!(
        subobjects(&signature)
            .unwrap_err()
            .to_string()
            .contains("past the raw bytes")
    );
}

#[test]
fn no_prefix_of_the_data_panics() {
    let data = library();
    for n in 0..data.len() {
        let _ = subobjects(&data[..n]);
    }
}

#[test]
fn an_export_name_is_the_function_name_undecorated() {
    assert_eq!(export_name("\u{1}?closest_hit@@YAXXZ"), "closest_hit");
    assert_eq!(export_name("?miss@@YAXXZ"), "miss");
    assert_eq!(export_name("plain"), "plain");
}

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
        let program = primase_dxil::lift(&fixture(&format!("ray_subobjects.{export}.dxil"))).unwrap();
        let StageFacts::RayTracing(facts) = &program.interface.facts else {
            panic!("{export}: {:?}", program.interface.facts)
        };
        assert_eq!(facts.export, export);
        assert_eq!(facts.subobjects, found, "{export}");
    }
}
