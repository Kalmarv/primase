//! Elden Ring maps read through the shared area read ([`sekiro::read_game_area`]) against SoulsFormatsNEXT's reading of
//! the same layouts (`msb.jsonl` of `oracles.soulsformats.elden/format`): the Academy of Raya Lucaria (m14_00_00_00, a
//! legacy dungeon) by itself, and the Shaded Castle tile (m60_39_54_00) with its parent tiles' parts for it. Every map
//! piece and asset part the reference lists is accounted for (a placement, or a model the archive lacks or whose face
//! sets use no vertex; SpeedTree models are placed like any other), and each placement stands at the part's stored position, moved by
//! its parent tile's offset where a parent places it: translation exact, with the core frame's z reversed.
use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use primase_core::{Instance, Source, Transform, World};
use primase_fromsoft::sekiro;

use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The reference's map piece and asset parts of the layouts named, by `<map>:<part name>`: their stored position.
fn reference_parts(oracle: &std::path::Path, maps: &[&str]) -> Result<BTreeMap<String, [f32; 3]>> {
    let mut parts = BTreeMap::new();
    for line in std::fs::read_to_string(oracle.join("msb.jsonl"))?.lines() {
        let record: serde_json::Value = serde_json::from_str(line)?;
        let path = record["path"].as_str().ok_or("path")?;
        let Some(map) = maps.iter().find(|m| path.ends_with(&format!("/{m}.msb.dcx"))) else {
            continue;
        };
        for part in record["parts"].as_array().ok_or("parts")? {
            if matches!(part["kind"].as_str(), Some("MapPiece" | "Asset")) {
                let name = part["name"].as_str().ok_or("name")?;
                let p = common::bits3(&part["position"])?.map(f32::from_bits);
                parts.insert(format!("{map}:{name}"), p);
            }
        }
    }
    Ok(parts)
}

#[test]
fn every_placed_part_is_accounted_for() -> Result<()> {
    let (Some(root), Some(keys), Some(oracle), Some(dll)) = (
        primase_testdata::install(primase_testdata::Game::Elden),
        primase_testdata::keys(primase_testdata::Game::Elden),
        primase_testdata::oracle(
            primase_testdata::OracleSet::SoulsFormats,
            primase_testdata::Game::Elden,
            "format",
        ),
        primase_testdata::oodle(primase_testdata::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = Install::new(root, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (map, parents) in [
        ("m14_00_00_00", sekiro::Parents::None),
        ("m60_39_54_00", sekiro::Parents::Include),
    ] {
        let options = sekiro::Options {
            parents,
            ..Default::default()
        };
        let mut world = World::default();
        let report = sekiro::read_game_area(&install, map, &decoder, &options, &mut world)?;
        // every material its material binary defines is linked to its shader's bundle, unless the install holds none
        // (the report names those by the folded, ASCII-lowercase shader name)
        for (e, m) in world.iter::<primase_fromsoft::materials::Material>() {
            if m.definition.is_some()
                && !report
                    .missing_shader_bundles
                    .contains(&m.shader.to_ascii_lowercase())
            {
                assert!(
                    world.has::<primase_fromsoft::sekiro::source::MaterialShader>(e),
                    "{map}: material {} (shader {}) has no fromsoft.material_shader",
                    m.mtd,
                    m.shader
                );
            }
        }
        let offsets: BTreeMap<String, [f64; 2]> = Game::EldenRing.parent_maps(map).into_iter().collect();
        let mut names = vec![map];
        names.extend(report.parent_maps.iter().map(String::as_str));
        let mut reference = reference_parts(&oracle, &names)?;
        // a parent's parts for this map only
        reference.retain(|key, _| key.starts_with(&format!("{map}:")) || key.contains(&format!(":{map}-")));
        let mut placed = 0;
        let mut failures = Vec::new();
        for (e, _) in world.iter::<Instance>() {
            let label = &world.get::<Source>(e).ok_or("placement without a source")?.0;
            let transform = world.get::<Transform>(e).ok_or("placement without a transform")?;
            let Some(stored) = reference.remove(label) else {
                failures.push(format!("{label}: not a map piece or asset part of the reference"));
                continue;
            };
            let (source_map, _) = label.split_once(':').ok_or("label")?;
            let [ox, oz] = offsets.get(source_map).copied().unwrap_or([0.; 2]);
            let want = [
                (f64::from(stored[0]) - ox) as f32,
                stored[1],
                -((f64::from(stored[2]) - oz) as f32),
            ];
            if transform.cols[3].map(f32::to_bits) != want.map(f32::to_bits) {
                failures.push(format!("{label}: at {:?}, stored {want:?}", transform.cols[3]));
            }
            placed += 1;
        }
        let unplaced = reference.len();
        println!(
            "{map}: {placed} placements ({} from {:?}), {unplaced} parts not placed (models missing {}, empty {}), \
             {} prototypes, {} materials ({} undefined), collision {} files {} triangles",
            report.parent_placements,
            report.parent_maps,
            report.skipped_missing,
            report.skipped_empty,
            report.prototypes,
            report.materials,
            report.undefined_materials,
            report.collision.files,
            report.collision.triangles,
        );
        for f in failures.iter().take(20) {
            println!("MISMATCH {f}");
        }
        assert!(failures.is_empty(), "{map}: {} placements differ", failures.len());
        assert_eq!(placed, report.placements);
        assert_eq!(
            unplaced,
            report.skipped_missing + report.skipped_empty,
            "{map}: every part not placed is a model missing or empty"
        );
    }
    Ok(())
}
