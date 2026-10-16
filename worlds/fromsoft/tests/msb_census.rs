//! Stored MSB values exact; derived rotation bases within 1e-6 per unit scale.
use primase_core::gate::{GateReport, Tolerance};
use primase_fromsoft::{Archive, dcx::Decoder, msb::MapLayout};
use primase_testdata::real_data;
use sha2::{Digest, Sha256};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
mod native {
    pub fn load(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
        // SAFETY: caller explicitly supplies a trusted version-6 native library.
        unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
    }
}
fn floats(value: &serde_json::Value) -> Result<Vec<f32>> {
    value
        .as_array()
        .ok_or("float array")?
        .iter()
        .map(|v| v.as_f64().map(|f| f as f32).ok_or("float".into()))
        .collect()
}
fn stored(g: &mut GateReport, label: &str, a: &[f32], b: &[f32]) {
    g.compare_bytes(
        label,
        &a.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<_>>(),
        &b.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<_>>(),
    );
}
fn transform(g: &mut GateReport, label: &str, a: &[f32], b: &[f32], scale: f32) -> Result<()> {
    if a.len() != 12 || b.len() != 12 {
        return Err("matrix size".into());
    }
    let delta = a[..9]
        .iter()
        .zip(&b[..9])
        .map(|(a, b)| f64::from((a - b).abs()) / f64::from(scale.max(1.)))
        .fold(0., f64::max);
    g.record(label, delta);
    // Translation is stored, with only the exact handedness sign conversion.
    if a[9..] != b[9..] {
        return Err(format!("{label}: translation mismatch").into());
    }
    Ok(())
}
#[test]
fn map_values_and_transforms() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_FROMSOFT_MSB_ORACLE") else {
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
    let mut source = GateReport::new("MSB stored vectors", Tolerance::Exact);
    let mut matrices = GateReport::new("MSB basis per unit scale", Tolerance::Abs(1e-6));
    let mut maps = 0;
    let mut parts = 0;
    let mut offsets = 0;
    for line in std::fs::read_to_string(oracle.join("maps.jsonl"))?.lines() {
        let expected: serde_json::Value = serde_json::from_str(line)?;
        let path = expected["path"].as_str().ok_or("path")?;
        let data = archive.read(path, &decoder)?;
        let hash: String = Sha256::digest(&data).iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hash, expected["sha256"].as_str().ok_or("hash")?);
        let map = MapLayout::read(&data)?;
        let models = expected["models"].as_array().ok_or("models")?;
        assert_eq!(map.models.len(), models.len());
        for (model, name) in map.models.iter().zip(models) {
            assert_eq!(model.name, name.as_str().ok_or("model name")?);
        }
        let ps = expected["parts"].as_array().ok_or("parts")?;
        assert_eq!(map.parts.len(), ps.len(), "{path}");
        for (part, want) in map.parts.iter().zip(ps) {
            assert_eq!(part.name, want["name"].as_str().ok_or("name")?, "{path}");
            assert_eq!(format!("{:?}", part.kind), want["kind"].as_str().ok_or("kind")?);
            assert_eq!(
                part.model.map(|i| map.models[i].name.as_str()),
                want["model"].as_str(),
                "{path}"
            );
            for (label, values) in [
                ("position", part.position),
                ("rotation", part.rotation_degrees),
                ("scale", part.scale),
            ] {
                stored(
                    &mut source,
                    &format!("{path}/{}/{label}", part.index),
                    &values,
                    &floats(&want[label])?,
                );
            }
            let matrix: Vec<_> = part.transform()?.cols.into_iter().flatten().collect();
            transform(
                &mut matrices,
                &format!("{path}/{}", part.index),
                &matrix,
                &floats(&want["transform"])?,
                part.scale.iter().map(|x| x.abs()).fold(1., f32::max),
            )?;
            parts += 1;
        }
        let os = expected["offsets"].as_array().ok_or("offsets")?;
        assert_eq!(map.map_offsets.len(), os.len());
        for (o, w) in map.map_offsets.iter().zip(os) {
            assert_eq!(o.name, w["name"].as_str().ok_or("offset name")?);
            stored(
                &mut source,
                "offset position",
                &o.position,
                &floats(&w["position"])?,
            );
            stored(
                &mut source,
                "offset rotation",
                &[o.degrees],
                &[w["degrees"].as_f64().ok_or("offset degrees")? as f32],
            );
            let matrix: Vec<_> = o.transform()?.cols.into_iter().flatten().collect();
            transform(
                &mut matrices,
                &format!("{path}/offset/{}", o.name),
                &matrix,
                &floats(&w["transform"])?,
                1.,
            )?;
            offsets += 1;
        }
        maps += 1;
    }
    println!("maps {maps}, parts {parts}, map offsets {offsets}\n{source}\n{matrices}");
    source.assert_passed();
    matrices.assert_passed();
    Ok(())
}
#[test]
fn stored_scene_placements() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(scene) = real_data("PRIMASE_FROMSOFT_SCENE") else {
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
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(scene.join("scene.json"))?)?;
    let sources = manifest["placement_sources"].as_array().ok_or("sources")?;
    let prototypes = manifest["prototypes"].as_array().ok_or("prototypes")?;
    let mut layouts = std::collections::BTreeMap::new();
    let mut gate = GateReport::new(
        "stored scene placement basis per unit scale",
        Tolerance::Abs(1e-6),
    );
    for p in
        primase_skate_scene::tables::read_placements(&std::fs::read(scene.join("tables/placements.bin"))?)?
    {
        let label = sources[p.source as usize].as_str().ok_or("source")?;
        let (area, name) = label.split_once(':').ok_or("source key")?;
        // Object source labels omit the map; use the map-piece prototype set.
        let map = if area == "obj" {
            prototypes
                .iter()
                .filter_map(|p| p["id"].as_str())
                .find_map(|id| id.strip_prefix('m').and_then(|_| id.split_once('/').map(|x| x.0)))
                .ok_or("scene map")?
        } else {
            area
        };
        if !layouts.contains_key(map) {
            let data = archive.read(&format!("/map/mapstudio/{map}.msb.dcx"), &decoder)?;
            layouts.insert(map.to_owned(), MapLayout::read(&data)?);
        }
        let layout = &layouts[map];
        let part = layout
            .parts
            .iter()
            .find(|p| p.name == name)
            .ok_or("part source")?;
        let matrix: Vec<_> = part.transform()?.cols.into_iter().flatten().collect();
        transform(
            &mut gate,
            label,
            &matrix,
            &p.m,
            part.scale.iter().map(|x| x.abs()).fold(1., f32::max),
        )?;
        let model = &layout.models[part.model.ok_or("placement model")?].name;
        assert_eq!(
            prototypes[p.proto as usize]["id"]
                .as_str()
                .ok_or("prototype id")?,
            format!("{area}/{model}")
        );
    }
    println!("{gate}");
    gate.assert_passed();
    Ok(())
}
