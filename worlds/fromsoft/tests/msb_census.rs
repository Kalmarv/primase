//! An area's MSB as stored, against an independent reading: SoulsFormats' parts of the same area
//! (`PRIMASE_FROMSOFT_RIGID_ORACLE`'s parts.jsonl, every map piece and object part). Each part by name: its kind and
//! model, and its position, rotation (degrees) and scale bit for bit. The matrices derived from them are checked
//! against the same dump by sekiro_scene.rs. Not covered here: other part kinds (enemies, collisions, dummies) and
//! map offsets, which the dump doesn't hold (an open coverage gap, CONTRIBUTING's real-data table).
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::msb::PartKind;
use primase_fromsoft::{Archive, dcx::Decoder, msb::MapLayout};
use primase_testdata::real_data;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
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
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dump) = real_data("PRIMASE_FROMSOFT_RIGID_ORACLE") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
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
