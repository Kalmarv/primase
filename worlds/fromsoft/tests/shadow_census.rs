//! Independent source shadow records, checked against the original MSB bytes,
//! then joined by part name to the placements an installed-area read selects.
use primase_core::{ShadowCast, Shadows, Source, World};
use primase_fromsoft::{dcx::Decoder, msb::MapLayout, msb::ShadowFlags, sekiro};
use primase_testdata::real_data;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
struct Oracle {
    sha256: String,
    parts: Vec<Part>,
}
#[derive(serde::Deserialize)]
struct Part {
    name: String,
    cast: bool,
    receive: bool,
    only: bool,
    point_light_cast: bool,
    static_cast: u8,
    cascade3_cast: u8,
}

#[test]
fn source_shadow_flags() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = primase_testdata::real_data("PRIMASE_FROMSOFT_SHADOW_ORACLE") else {
        return Ok(());
    };
    let data = std::fs::read(root.join("map.msb"))?;
    let oracle: Oracle = serde_json::from_slice(&std::fs::read(root.join("shadows.json"))?)?;
    assert_eq!(
        Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        oracle.sha256.to_lowercase()
    );
    let map = MapLayout::read(&data)?;
    assert!(!oracle.parts.is_empty(), "empty shadow coverage");
    assert_eq!(map.parts.len(), oracle.parts.len());
    let mut contradictory = 0;
    for (actual, expected) in map.parts.iter().zip(&oracle.parts) {
        assert_eq!(actual.name, expected.name, "part {}", actual.index);
        let s = actual.shadows;
        assert_eq!(
            (
                s.cast,
                s.receive,
                s.only,
                s.point_light_cast,
                s.static_cast,
                s.cascade3_cast
            ),
            (
                expected.cast,
                expected.receive,
                expected.only,
                expected.point_light_cast,
                expected.static_cast,
                expected.cascade3_cast
            ),
            "part {} {}",
            actual.index,
            actual.name
        );
        contradictory += usize::from(s.only && !s.cast);
    }
    println!(
        "{} source parts, six exact shadow fields each, {contradictory} contradictory cast/only combinations",
        map.parts.len()
    );
    Ok(())
}

#[allow(unsafe_code)]
fn native(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: the caller explicitly supplies a trusted native version-6 library.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}

/// The oracle map is m11_01. Every selected placement must name an oracle part;
/// its retained flags and core shadows are compared with the oracle's bytes and
/// a mapping written here, and the report counts are recounted from the join.
#[test]
fn selected_placement_shadows() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_SHADOW_ORACLE") else {
        return Ok(());
    };
    let Some(install) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(scene) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let oracle: Oracle = serde_json::from_slice(&std::fs::read(root.join("shadows.json"))?)?;
    let parts: BTreeMap<_, _> = oracle.parts.iter().map(|p| (p.name.as_str(), p)).collect();
    assert_eq!(parts.len(), oracle.parts.len(), "oracle part names are unique");
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(scene.join("scene.json"))?)?;
    let region: [f32; 4] = serde_json::from_value(manifest["region"].clone())?;
    let dll = native(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_area(
        install,
        "m11_01",
        &decoder,
        &sekiro::Options {
            region: Some(sekiro::Region {
                rect: region,
                margin: 30.0,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    let mut contradictory = 0u64;
    let mut placed = 0u64;
    for (e, source) in world.iter::<Source>() {
        if world.get::<primase_core::Instance>(e).is_none() {
            assert!(world.get::<ShadowFlags>(e).is_none() && world.get::<Shadows>(e).is_none());
            continue;
        }
        let name = source
            .0
            .strip_prefix("m11_01_00_00:")
            .ok_or("placement source area")?;
        let expected = parts.get(name).ok_or_else(|| format!("{name} not in oracle"))?;
        let raw = world.get::<ShadowFlags>(e).ok_or("raw shadow flags")?;
        assert_eq!(
            (
                raw.cast,
                raw.receive,
                raw.only,
                raw.point_light_cast,
                raw.static_cast,
                raw.cascade3_cast
            ),
            (
                expected.cast,
                expected.receive,
                expected.only,
                expected.point_light_cast,
                expected.static_cast,
                expected.cascade3_cast
            ),
            "{name}"
        );
        let cast = if expected.only {
            expected.cast.then_some(ShadowCast::Only)
        } else if expected.cast {
            Some(ShadowCast::On)
        } else {
            Some(ShadowCast::Off)
        };
        contradictory += u64::from(expected.only && !expected.cast);
        placed += 1;
        assert_eq!(
            world.get::<Shadows>(e),
            Some(&Shadows {
                cast,
                receive: Some(expected.receive)
            }),
            "{name}"
        );
    }
    assert!(placed > 0, "empty selected coverage");
    assert_eq!(placed, report.placements as u64);
    assert_eq!(
        report.shadows.counts["placements with source shadow controls"],
        placed
    );
    assert_eq!(
        report
            .shadows
            .counts
            .get("contradictory source cast/only controls")
            .copied()
            .unwrap_or(0),
        contradictory
    );
    println!(
        "{placed} selected placements joined to oracle parts, {contradictory} contradictory; {:?}",
        report.shadows
    );
    Ok(())
}
