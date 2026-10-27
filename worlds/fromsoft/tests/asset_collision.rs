//! Every asset collision file of Elden Ring (each `_h` and `_l.geomhkxbnd.dcx` the census of the install lists:
//! `entries.tsv` of `oracles.soulsformats.elden/census`) read as object collision bodies, and each body tessellated:
//! compressed meshes, the convex shapes (box, polytope, capsule, cylinder, plain convex, triangle, sphere), the extern shape
//! instances compressed meshes hold, and the custom primitives of an unknown kind, which are counted. No file may be
//! refused and no convex body may tessellate to nothing. HKLib's reading of the extern instances themselves is compared
//! in `havok_census.rs`.
use fromsoft_formats::collision::{BodyGeometry, read_bodies};
use fromsoft_formats::{Binder, Game, dcx::Decoder};
use primase_testdata::{OracleSet, Query, Tool, expect_found, oodle, oracle};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

#[test]
fn every_asset_collision_file_reads_and_tessellates() -> Result<()> {
    let (Some(archive), Some(census), Some(dll)) = (
        common::archive(Game::EldenRing)?,
        oracle(OracleSet::SoulsFormats, primase_testdata::Game::Elden, "census"),
        oodle(Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let paths: Vec<String> = std::fs::read_to_string(census.join("entries.tsv"))?
        .lines()
        .filter_map(|l| l.split('\t').nth(2))
        .filter(|p| p.starts_with("/asset/") && p.ends_with(".geomhkxbnd.dcx"))
        .map(str::to_owned)
        .collect();
    let (mut files, mut externs, mut unread_custom) = (0usize, 0usize, 0usize);
    let (mut unkeyed_rows, mut placement_dependent) = (0usize, 0usize);
    let mut shapes: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for path in &paths {
        for member in Binder::read(&archive.read(path, &decoder)?, &decoder)?.files {
            let name = member.name.unwrap_or_default();
            let leaf = name.rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
            if !leaf.ends_with(".hkx") {
                continue;
            }
            files += 1;
            let bodies = match read_bodies(&decoder.decode_if_dcx(&member.bytes)?, None) {
                Ok(bodies) => bodies,
                Err(e) => {
                    failures.push(format!("{path}:{leaf}: {e}"));
                    continue;
                }
            };
            for body in bodies {
                *shapes.entry(body.shape.clone()).or_default() += 1;
                match &body.geometry {
                    BodyGeometry::Mesh(soup) => {
                        unread_custom += soup.unread_custom_primitives;
                        unkeyed_rows += soup.unkeyed_triangle_data;
                        placement_dependent += soup.placement_dependent_keys;
                    }
                    BodyGeometry::Convex { instance, .. } => externs += usize::from(instance.is_some()),
                }
                match primase_fromsoft::sekiro::objects::body_triangles(&body.geometry) {
                    Ok(t) if t.is_empty() && matches!(body.geometry, BodyGeometry::Convex { .. }) => {
                        failures.push(format!(
                            "{path}:{leaf}: {} {}: no triangles",
                            body.name, body.shape
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => failures.push(format!("{path}:{leaf}: {} {}: {e}", body.name, body.shape)),
                }
            }
        }
    }
    println!(
        "files {files}, bodies by shape {shapes:?}, extern shape instances {externs}, custom primitives of an unknown \
         kind {unread_custom}, triangle-data rows no key takes {unkeyed_rows}, keys whose material depends on their place {placement_dependent}, failures {}",
        failures.len()
    );
    for f in failures.iter().take(40) {
        println!("FAILED {f}");
    }
    expect_found(
        Query::Oracle(OracleSet::SoulsFormats, primase_testdata::Game::Elden, "census"),
        "asset collision files",
        files,
    );
    assert!(failures.is_empty());
    Ok(())
}
