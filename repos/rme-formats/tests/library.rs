//! Hand-worked bank identities: which bank holds a material id, and the namespace rule for id zero.
use rme_formats::{
    library::{BankEvidence, MaterialLibrary},
    material::{MaterialKey, MaterialSampler, ShredMaterial},
};

fn material(bank: &str, id: u32, paths: &[&str]) -> ShredMaterial {
    ShredMaterial {
        key: MaterialKey {
            bank: bank.into(),
            id,
        },
        unknown_header: [0; 4],
        vertex_shader: "v".into(),
        fragment_shader: "p".into(),
        parameters: vec![],
        samplers: paths
            .iter()
            .enumerate()
            .map(|(i, path)| MaterialSampler {
                slot: i as u32,
                hash: 999,
                name: None,
                name_source: None,
                texture: (*path).into(),
                unknown_state: [0; 5],
            })
            .collect(),
    }
}

#[test]
fn unique_ids_and_namespace_inference_keep_all_candidates() {
    let library = MaterialLibrary::from_materials(
        vec![
            material("environments", 7, &[]),
            material("environments", 0, &[]),
            material("characters", 0, &[]),
            material("actors", 0, &[]),
        ],
        &[],
    )
    .unwrap();
    let r = library.resolve(7, None);
    assert_eq!(r.evidence, Some(BankEvidence::UniqueId));
    assert_eq!(r.selected.unwrap().bank, "environments");
    for (prefix, bank) in [
        ("levels", "environments"),
        ("actors", "actors"),
        ("characters", "characters"),
    ] {
        let r = library.resolve(0, Some(prefix.as_bytes()));
        assert_eq!(r.candidates.len(), 3);
        assert_eq!(r.selected.unwrap().bank, bank);
        assert_eq!(r.evidence, Some(BankEvidence::InferredQuartetNamespace));
    }
    let r = library.resolve(0, Some(b"unknown"));
    assert_eq!(r.candidates.len(), 3);
    assert!(r.selected.is_none());
    assert!(r.evidence.is_none());
    assert!(library.resolve(99, None).candidates.is_empty());
    assert_eq!(
        library
            .material(&MaterialKey {
                bank: "actors".into(),
                id: 0
            })
            .map(|m| m.key.id),
        Some(0)
    );
    assert!(
        MaterialLibrary::from_materials(vec![material("actors", 1, &[]), material("actors", 1, &[])], &[])
            .is_err()
    );
}
