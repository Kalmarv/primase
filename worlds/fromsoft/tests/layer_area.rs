//! Group identities from the independent export, before any area pixels bake.
mod common;
#[path = "common/layers.rs"]
mod layer_oracle;
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_testdata::real_data;
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn area_layer_groups() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_LAYER_AREA") else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let counts: Value = serde_json::from_slice(&std::fs::read(oracle.join("counts.json"))?)?;
    let mut failures = Vec::new();
    for name in ["slice", "castle"] {
        let region = serde_json::from_value(counts[name]["region"].clone())?;
        let references: Vec<Value> =
            serde_json::from_slice(&std::fs::read(oracle.join(format!("{name}-materials.json")))?)?;
        let mut world = primase_core::World::default();
        sekiro::read_area(
            &root,
            "m11_01_00_00",
            &decoder,
            &sekiro::Options {
                region,
                ..Default::default()
            },
            &mut world,
        )?;
        common::run(&mut world, common::LEGACY_GEOMETRY)?;
        let groups = sekiro::layers::AreaBake::from_world(&world)?.groups;
        let without_gx: BTreeSet<_> = groups
            .iter()
            .map(|g| {
                let mut identity = serde_json::to_value(&g.material).expect("material JSON");
                identity.as_object_mut().expect("material object").remove("gx");
                identity.to_string()
            })
            .collect();
        println!(
            "{name}: {} bake-group representatives, {} identities excluding raw GX",
            groups.len(),
            without_gx.len()
        );
        let mut matched = BTreeSet::new();
        let mut variants = 0;
        let mut nonunit = 0;
        for group in &groups {
            let candidates: Vec<_> = references
                .iter()
                .filter(|r| layer_oracle::matches(r, &group.material))
                .collect();
            if candidates.len() != 1 {
                failures.push(format!(
                    "{name}: {} matched {} reference materials",
                    group.material.mtd,
                    candidates.len()
                ));
                continue;
            }
            let key = candidates[0]["key"].as_str().unwrap().to_lowercase();
            let expected_period: [f32; 2] = serde_json::from_value(candidates[0]["period"].clone())?;
            if group.period != expected_period || !group.period_exact {
                failures.push(format!(
                    "{name}: {key}: period {:?} vs {expected_period:?}, exact {}",
                    group.period, group.period_exact
                ));
            }
            if !matched.insert(key.clone()) {
                failures.push(format!("{name}: duplicate {key}"));
            }
            let expected: BTreeSet<[i32; 8]> = counts[name]["records"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["material"].as_str().unwrap().eq_ignore_ascii_case(&key))
                .map(|r| {
                    serde_json::from_value::<[f32; 8]>(r["weights"].clone())
                        .unwrap()
                        .map(|x| (x * 64.).round() as i32)
                })
                .collect();
            let actual: BTreeSet<_> = group
                .centers
                .iter()
                .map(|c| c.map(|x| (x * 64.).round() as i32))
                .collect();
            if actual != expected {
                failures.push(format!("{name}: {key}: centers {actual:?} vs {expected:?}"));
            }
            variants += group.centers.len();
            if group.period != [1.; 2] {
                nonunit += group.centers.len();
            }
        }
        println!(
            "{name}: {} materials, {variants} variants, {nonunit} non-unit UV variants, {} matched identities",
            groups.len(),
            matched.len()
        );
        if matched.len() != references.len() {
            failures.push(format!(
                "{name}: {} of {} material identities",
                matched.len(),
                references.len()
            ));
        }
        if variants as u64 != counts[name]["variants"].as_u64().unwrap() {
            failures.push(format!("{name}: variant count {variants}"));
        }
        if nonunit as u64 != counts[name]["nonunit_variants"].as_u64().unwrap() {
            failures.push(format!("{name}: non-unit variant count {nonunit}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
