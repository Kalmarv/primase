//! An area's MSB as stored, against an independent reading: SoulsFormats' parts of the same area
//! (`oracles.soulsformats.sekiro/m11_01`'s parts.jsonl, every map piece and object part). Each part by name: its kind
//! and model, and its position, rotation (degrees) and scale bit for bit. The matrices derived from them are checked
//! against the same dump by sekiro_scene.rs. Not covered here: other part kinds (enemies, collisions, dummies) and map
//! offsets, which the dump doesn't hold (an open coverage gap, CONTRIBUTING's real-data table). Elden Ring's layouts
//! are checked whole, every part of every kind, against SoulsFormatsNEXT's reading of them.
mod inputs;

use primase_fromsoft::msb::PartKind;
use primase_fromsoft::{Archive, dcx::Decoder, msb::MapLayout};
use primase_testdata::gate::{GateReport, Tolerance};

use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;
#[allow(unsafe_code)]
mod native {
    pub fn load(
        path: crate::inputs::OodleLibrary,
    ) -> primase_foundations::compress::Result<primase_foundations::compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe {
            primase_foundations::compress::oodle::Oodle::open(
                path.path(),
                primase_foundations::compress::oodle::Version::V6,
            )
        }
    }
}

/// The area the dump read (its parts.jsonl is m11_01's).
const AREA: &str = "/map/mapstudio/m11_01_00_00.msb.dcx";

fn floats(value: &serde_json::Value) -> Result<Vec<f32>> {
    value
        .as_array()
        .ok_or("float array")?
        .iter()
        .map(|v| v.as_f64().map(|f| f as f32).ok_or("float".into()))
        .collect()
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[test]
fn stored_parts_match_an_independent_reading() -> Result<()> {
    let Some(root) = inputs::install(inputs::Game::Sekiro) else {
        return Ok(());
    };
    let Some(dump) = inputs::oracle(inputs::OracleSet::SoulsFormats, inputs::Game::Sekiro, "m11_01") else {
        return Ok(());
    };
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = native::load(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let archive = Archive::open(root)?;
    let map = MapLayout::read(&archive.read(AREA, &decoder)?)?;
    // the parts the dump holds, by name: map pieces and objects
    let mut ours: BTreeMap<&str, &primase_fromsoft::msb::Part> = BTreeMap::new();
    for p in &map.parts {
        if matches!(p.kind, PartKind::MapPiece | PartKind::Object) {
            assert!(
                ours.insert(p.name.as_str(), p).is_none(),
                "part name {} repeats",
                p.name
            );
        }
    }
    let mut stored = GateReport::new("MSB parts as stored", Tolerance::Exact);
    let mut seen = 0;
    for line in std::fs::read_to_string(dump.join("parts.jsonl"))?.lines() {
        let want: serde_json::Value = serde_json::from_str(line)?;
        let name = want["name"].as_str().ok_or("name")?;
        let part = ours
            .remove(name)
            .ok_or_else(|| format!("{name}: in the dump, not read"))?;
        let kind = match part.kind {
            PartKind::MapPiece => "piece",
            _ => "object",
        };
        assert_eq!(kind, want["kind"].as_str().ok_or("kind")?, "{name}");
        assert_eq!(
            part.model.map(|i| map.models[i].name.as_str()),
            want["model"].as_str(),
            "{name}"
        );
        for (label, values) in [
            ("position", part.position),
            ("rotation", part.rotation_degrees),
            ("scale", part.scale),
        ] {
            stored.compare_bytes(
                format!("{name}/{label}"),
                &bytes(&values),
                &bytes(&floats(&want[label])?),
            );
        }
        seen += 1;
    }
    assert!(
        ours.is_empty(),
        "read, not in the dump: {:?}",
        ours.keys().take(5).collect::<Vec<_>>()
    );
    println!("{seen} parts of {AREA}\n{stored}");
    assert!(seen > 0);
    stored.assert_passed();
    Ok(())
}

/// Every map layout of a game against an independent reader's reading of the same files (`msb.jsonl` of the game's
/// format oracle: SoulsFormatsNEXT ee1dd61's MSBE for Elden Ring, `oracles.soulsformats.elden/format`): the models'
/// names, and every part of every kind by name: its kind, model, position, rotation and scale (f32 bit patterns),
/// entity ID and groups, the six shadow controls and a collision part's hit filter; and every spawn point region in
/// file order (name, position and rotation; a repeated name renamed as the reference renames it, over every region).
#[test]
fn every_layout_matches_an_independent_reading() -> Result<()> {
    use primase_fromsoft::Game;
    for (game, format) in [(
        Game::EldenRing,
        inputs::oracle(inputs::OracleSet::SoulsFormats, inputs::Game::Elden, "format"),
    )] {
        let (Some(archive), Some(oracle), Some(dll)) = (
            common::archive(game)?,
            format,
            inputs::oodle(inputs::Tool::Oodle6),
        ) else {
            continue;
        };
        let dll = native::load(dll)?;
        let decoder = Decoder {
            oodle: Some(&dll),
            max_output: 1 << 30,
        };
        let (mut maps, mut parts, mut fields, mut spawn_points) = (0usize, 0usize, 0usize, 0usize);
        let mut failures = Vec::new();
        for line in std::fs::read_to_string(oracle.join("msb.jsonl"))?.lines() {
            let record: serde_json::Value = serde_json::from_str(line)?;
            let path = record["path"].as_str().ok_or("path")?;
            let layout =
                MapLayout::read(&archive.read(path, &decoder)?).map_err(|e| format!("{path}: {e}"))?;
            maps += 1;
            let mut model_names: Vec<String> = layout.models.iter().map(|m| m.name.clone()).collect();
            common::disambiguate(&mut model_names);
            let mut want_models: Vec<&str> = record["models"]
                .as_array()
                .ok_or("models")?
                .iter()
                .map(|m| m["name"].as_str().unwrap_or_default())
                .collect();
            let mut have_models: Vec<&str> = model_names.iter().map(String::as_str).collect();
            want_models.sort_unstable();
            have_models.sort_unstable();
            if want_models != have_models {
                failures.push(format!("{path}: model names differ"));
            }
            // the spawn point regions, in file order: name, position and rotation as stored. The reference renames a
            // repeated name over every region of every type (m12_04_00_00 stores one spawn point name five times)
            let points = layout.spawn_points().map_err(|e| format!("{path}: {e}"))?;
            let mut region_names = layout.region_names().map_err(|e| format!("{path}: {e}"))?;
            common::disambiguate(&mut region_names);
            let want_points = record["spawn_points"].as_array().ok_or("spawn_points")?;
            if points.len() != want_points.len() {
                failures.push(format!(
                    "{path}: {} spawn points, the reference {}",
                    points.len(),
                    want_points.len()
                ));
            } else {
                for (p, want) in points.iter().zip(want_points) {
                    spawn_points += 1;
                    if Some(region_names[p.region].as_str()) != want["name"].as_str()
                        || p.position.map(f32::to_bits) != common::bits3(&want["position"])?
                        || p.rotation_degrees.map(f32::to_bits) != common::bits3(&want["rotation"])?
                    {
                        failures.push(format!("{path} spawn point {}: differs", region_names[p.region]));
                    }
                }
            }
            let mut names: Vec<String> = layout.parts.iter().map(|p| p.name.clone()).collect();
            common::disambiguate(&mut names);
            let ours: BTreeMap<&str, usize> =
                names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
            let theirs = record["parts"].as_array().ok_or("parts")?;
            if theirs.len() != layout.parts.len() || ours.len() != layout.parts.len() {
                failures.push(format!(
                    "{path}: part count {} vs {}",
                    layout.parts.len(),
                    theirs.len()
                ));
                continue;
            }
            for want in theirs {
                let name = want["name"].as_str().ok_or("part name")?;
                let Some(&i) = ours.get(name) else {
                    failures.push(format!("{path}: no part {name}"));
                    continue;
                };
                let p = &layout.parts[i];
                parts += 1;
                let groups: Vec<i64> = p.entity_group_ids.iter().map(|&g| i64::from(g)).collect();
                let want_groups: Vec<i64> = want["entity_group_ids"]
                    .as_array()
                    .ok_or("groups")?
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .collect();
                // the reference reads the point-light byte as a signed byte: 0 or -1
                let point_light = match want["point_light"].as_i64() {
                    Some(0) => false,
                    Some(-1) => true,
                    other => return Err(format!("{path} {name}: point light {other:?}").into()),
                };
                let checks = [
                    (
                        "kind",
                        format!("{:?}", p.kind) == want["kind"].as_str().unwrap_or_default(),
                    ),
                    (
                        "model",
                        p.model.map(|m| model_names[m].as_str()) == want["model"].as_str(),
                    ),
                    (
                        "position",
                        p.position.map(f32::to_bits) == common::bits3(&want["position"])?,
                    ),
                    (
                        "rotation",
                        p.rotation_degrees.map(f32::to_bits) == common::bits3(&want["rotation"])?,
                    ),
                    (
                        "scale",
                        p.scale.map(f32::to_bits) == common::bits3(&want["scale"])?,
                    ),
                    (
                        "entity id",
                        Some(i64::from(p.entity_id)) == want["entity_id"].as_i64(),
                    ),
                    ("entity groups", groups == want_groups),
                    ("cast", Some(p.shadows.cast) == want["cast"].as_bool()),
                    ("receive", Some(p.shadows.receive) == want["receive"].as_bool()),
                    ("only", Some(p.shadows.only) == want["only"].as_bool()),
                    ("point light", p.shadows.point_light_cast == point_light),
                    (
                        "static",
                        Some(i64::from(p.shadows.static_cast)) == want["static_cast"].as_i64(),
                    ),
                    (
                        "cascade3",
                        Some(i64::from(p.shadows.cascade3_cast)) == want["cascade3_cast"].as_i64(),
                    ),
                    (
                        "hit filter",
                        p.hit_filter.map(i64::from) == want["hit_filter"].as_i64(),
                    ),
                ];
                for (field, same) in checks {
                    fields += 1;
                    if !same {
                        failures.push(format!("{path} {name}: {field}"));
                    }
                }
            }
        }
        println!(
            "{}: maps {maps}, parts {parts}, fields compared {fields}, spawn points {spawn_points}, mismatches {}",
            game.name(),
            failures.len()
        );
        for f in failures.iter().take(40) {
            println!("MISMATCH {f}");
        }
        assert!(maps > 0 && failures.is_empty());
    }
    Ok(())
}
