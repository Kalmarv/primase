//! Blocker-card identity gate. The oracle directory contains cases.json: an array
//! of {name, area, region, placements, prototypes, meshes, triangles}; placements
//! are source labels and prototypes are the corresponding prototype identities.
use primase_core::{World, gate::real_data};
use primase_fromsoft::{dcx::Decoder, sekiro};
use std::collections::BTreeSet;
#[test]
#[allow(unsafe_code)]
fn installed_blocker_cards() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_BLOCKER_ORACLE") else {
        return Ok(());
    };
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(oracle.join("cases.json"))?)?;
    // SAFETY: explicitly supplied trusted native version-6 decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    for case in cases.as_array().ok_or("cases array")? {
        let name = case["name"].as_str().ok_or("name")?;
        let options = sekiro::Options {
            region: serde_json::from_value(case["region"].clone())?,
            ..Default::default()
        };
        let area = case["area"].as_str().ok_or("area")?;
        let cards = sekiro::read_blocker_cards(&root, area, &decoder, &options)?;
        let mut world = World::default();
        if case["assemble"].as_bool().unwrap_or(false) {
            sekiro::read_area(&root, area, &decoder, &options, &mut world)?;
            let retained = world
                .resource::<sekiro::blockers::BlockerCards>()
                .ok_or("retained")?;
            assert_eq!(
                primase_core::json::to_value(retained)?,
                primase_core::json::to_value(&cards)?
            );
            let scene = world
                .resource::<primase_skate_scene::SkateScene>()
                .ok_or("scene")?;
            assert_eq!(
                scene.other["extensions"]["sekiro"]["blocker_cards"]["drawn"],
                false
            );
        }
        let got: BTreeSet<_> = cards.placements.iter().map(|p| p.source.clone()).collect();
        let want: BTreeSet<String> = serde_json::from_value(case["placements"].clone())?;
        assert_eq!(got, want, "{name}: placement identities");
        let got: BTreeSet<_> = cards.prototypes.keys().cloned().collect();
        let want: BTreeSet<String> = serde_json::from_value(case["prototypes"].clone())?;
        assert_eq!(got, want, "{name}: prototype identities");
        assert_eq!(
            cards.meshes() as u64,
            case["meshes"].as_u64().ok_or("meshes")?,
            "{name}"
        );
        assert_eq!(
            cards.triangles() as u64,
            case["triangles"].as_u64().ok_or("triangles")?,
            "{name}"
        );
        let got: BTreeSet<(String, usize)> = cards
            .prototypes
            .iter()
            .flat_map(|(key, parts)| parts.iter().map(move |p| (key.clone(), p.source_mesh)))
            .collect();
        let want: BTreeSet<(String, usize)> = serde_json::from_value(case["mesh_identities"].clone())?;
        assert_eq!(got, want, "{name}: FLVER mesh identities");
        for (_, material) in world.iter::<primase_fromsoft::materials::Material>() {
            assert!(
                !sekiro::blockers::is_card(material),
                "{name}: card still has render material"
            );
        }
        println!(
            "{name}: {} meshes, {} prototypes, {} placements, {} triangles retained outside render meshes",
            cards.meshes(),
            cards.prototypes.len(),
            cards.placements.len(),
            cards.triangles()
        );
    }
    Ok(())
}
