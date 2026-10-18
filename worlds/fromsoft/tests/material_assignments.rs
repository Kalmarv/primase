//! Main-layer policy compared with independent assignments and stored scene parts.
//! Numeric-slot ties prefer an MTD-declared binding over a foreign shader-family
//! binding. This is an explicit preview lowering choice, not confirmed game
//! sampler-binding behavior; both bindings survive for the later shader bake.
use primase_fromsoft::{
    Archive, Binder,
    dcx::Decoder,
    flver::{MaterialGeometry, Model},
    materials,
    mtd::MaterialDefinition,
};
use primase_testdata::real_data;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
fn native(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: caller explicitly supplies a trusted version-6 native library.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}
#[test]
fn area_materials_and_scene() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_MATERIAL_ORACLE") else {
        return Ok(());
    };
    let Some(scene) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let dll = native(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let mut definitions = BTreeMap::new();
    for f in Binder::read(
        &archive.read("/mtd/allmaterialbnd.mtdbnd.dcx", &decoder)?,
        &decoder,
    )?
    .files
    {
        let name = f.name.as_deref().unwrap_or("");
        if !name.to_lowercase().ends_with(".mtd") {
            continue;
        }
        definitions.insert(
            materials::stem(name),
            MaterialDefinition::read(&decoder.decode_if_dcx(&f.bytes)?)?,
        );
    }
    let scene: serde_json::Value = serde_json::from_slice(&std::fs::read(scene.join("scene.json"))?)?;
    let mut scene_protos: BTreeMap<_, _> = scene["prototypes"]
        .as_array()
        .ok_or("prototypes")?
        .iter()
        .map(|p| Ok((p["id"].as_str().ok_or("id")?, p)))
        .collect::<Result<_>>()?;
    let mut count = 0;
    let mut layered = 0;
    let mut scene_parts = 0;
    let mut scene_materials = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for line in std::fs::read_to_string(oracle.join("models.jsonl"))?.lines() {
        let w: serde_json::Value = serde_json::from_str(line)?;
        if w["missing"] == true {
            continue;
        }
        let key = w["key"].as_str().ok_or("key")?;
        let (area, model) = key.split_once('/').ok_or("key")?;
        let (path, leaf) = if area == "obj" {
            (format!("/obj/{model}.objbnd.dcx"), format!("{model}.flver"))
        } else {
            let stem = format!("{area}_{}", &model[1..]);
            (format!("/map/{area}/{stem}.mapbnd.dcx"), format!("{stem}.flver"))
        };
        let binder = Binder::read(&archive.read(&path, &decoder)?, &decoder)?;
        let f = binder
            .files
            .iter()
            .find(|f| f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(leaf.as_str()))
            .ok_or("FLVER")?;
        let model = Model::read(&f.bytes)?;
        let expected = w["materials"].as_array().ok_or("materials")?;
        assert_eq!(model.materials.len(), expected.len());
        let resolved: Vec<_> = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve(m, definitions.get(&materials::stem(&m.mtd)), |name| {
                    w["available"][name].as_bool() == Some(true)
                })
            })
            .collect();
        for (i, (m, e)) in resolved.iter().zip(expected).enumerate() {
            let ours = serde_json::to_value(m)?;
            for (field, reference) in [
                ("mtd", "Mtd"),
                ("shader", "Shader"),
                ("class", "Class"),
                ("skip", "Skip"),
                ("albedo", "Albedo"),
                ("normal", "Normal"),
                ("metallic", "Metallic"),
                ("emissive", "Emissive"),
                ("two_sided", "TwoSided"),
                ("is_emissive", "IsEmissive"),
                ("surface", "Surface"),
                ("natural", "Natural"),
            ] {
                if ours[field] != e[reference] {
                    failures.push(format!("{key}/{i} {field}: {} != {}", ours[field], e[reference]));
                }
            }
            if m.cutoff.to_bits() != (e["Cutoff"].as_f64().ok_or("cutoff")? as f32).to_bits() {
                failures.push(format!("{key}/{i} cutoff"));
            }
            assert_eq!(
                m.uv_scale,
                e["uv_scale"]
                    .as_array()
                    .ok_or("scale")?
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect::<Vec<_>>()
                    .as_slice()
            );
            assert_eq!(m.layered.is_some(), e["Layered"].as_bool().ok_or("layered")?);
            count += 1;
            layered += usize::from(m.layered.is_some());
        }
        if let Some(p) = scene_protos.remove(key) {
            let choices: Vec<_> = resolved
                .iter()
                .map(|m| MaterialGeometry {
                    skip: m.skip.is_some(),
                    uv_scale: m.uv_scale,
                })
                .collect();
            let geometry = model.geometry(&choices)?;
            let parts = p["parts"].as_array().ok_or("parts")?;
            assert_eq!(geometry.len(), parts.len(), "{key}");
            for (part, want) in geometry.iter().zip(parts) {
                let m = &resolved[part.material];
                let name = want["material"].as_str().ok_or("material key")?;
                let expected = &scene["materials"][name];
                // The preview policy for a material without a base texture, now applied in source PBR
                // lowering; checked here against the reference exporter's bytes.
                let has_base = m
                    .albedo
                    .as_deref()
                    .is_some_and(|n| !scene["textures"][n].is_null());
                let fallback = (!has_base).then_some(if m.class == "water" {
                    [48, 98, 122, 190]
                } else {
                    [128, 128, 128, 255]
                });
                assert_eq!(
                    serde_json::to_value(fallback)?,
                    expected["base_color"],
                    "{key}/{name}: fallback colour"
                );
                assert_eq!(m.is_emissive, expected["emissive"].as_bool().unwrap_or(false));
                let base = m.albedo.as_deref().filter(|n| !scene["textures"][*n].is_null());
                let normal = m
                    .normal
                    .as_deref()
                    .map(|n| format!("{n}@normal"))
                    .filter(|n| !scene["textures"][n].is_null());
                for (field, value) in [
                    ("class", serde_json::json!(m.class)),
                    ("base", serde_json::json!(base)),
                    ("normal", serde_json::json!(normal)),
                ] {
                    if expected[field] != value {
                        failures.push(format!(
                            "scene {key} {name}/{field}: {value} != {}",
                            expected[field]
                        ));
                    }
                }
                if let Some(c) = expected["cutoff"].as_f64() {
                    assert_eq!(m.cutoff.to_bits(), (c as f32).to_bits());
                }
                assert_eq!(
                    m.two_sided,
                    expected["two_sided"]
                        .as_bool()
                        .unwrap_or(matches!(m.class.as_str(), "cutout" | "leaves"))
                );
                scene_parts += 1;
                scene_materials.insert(name.to_owned());
            }
        }
    }
    println!(
        "assignments {count}, layered {layered}, scene parts {scene_parts}, distinct scene materials {}, remaining prototypes {}, failures {}",
        scene_materials.len(),
        scene_protos.len(),
        failures.len()
    );
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    assert!(failures.is_empty());
    assert!(scene_protos.is_empty());
    Ok(())
}
