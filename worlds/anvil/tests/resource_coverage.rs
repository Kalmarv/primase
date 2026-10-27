//! Index-only, every forge and every type, including unknowns. Runs in the ordinary test suite.
//! Counts stored entries (patch duplicates included), never decoded bytes or active placements.
mod inputs;

use anvil_formats::{forge::Forge, listing::KINDS};
use inputs::{Game, install, scratch};
use primase_anvil::read::coverage;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Tally {
    entries: u64,
    bytes: u64,
    examples: Vec<Value>,
}

fn forges(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        // Do not follow symlinks/junctions out of the configured install or through loops.
        if kind.is_dir() {
            forges(&entry.path(), out)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("forge"))
        {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn rows(tallies: &BTreeMap<u32, Tally>) -> Vec<Value> {
    tallies
        .iter()
        .map(|(&id, tally)| {
            let contract = coverage::of(id);
            json!({"type_id": format!("{id:#010x}"), "type": contract.name,
            "status": contract.status.name(), "stored_entries": tally.entries,
            "stored_payload_bytes": tally.bytes, "destination": contract.destination,
            "next_observation": contract.question, "examples": tally.examples})
        })
        .collect()
}

#[test]
fn every_forge_resource_type_has_an_explicit_coverage_line() {
    let Some(root) = install(Game::Riders) else {
        return;
    };
    let mut paths = Vec::new();
    forges(&root, &mut paths).expect("enumerating every forge under installs.riders");
    paths.sort();
    assert!(
        !paths.is_empty(),
        "configured Riders install contains no forge files: {}",
        root.display()
    );
    let mut all: BTreeMap<u32, Tally> = KINDS
        .into_iter()
        .map(|k| (k.type_id(), Tally::default()))
        .chain(coverage::EXTRA.iter().map(|k| (k.type_id, Tally::default())))
        .collect();
    let mut reports = Vec::new();
    let mut entry_total = 0u64;
    let mut byte_total = 0u64;
    for path in paths {
        let relative = path
            .strip_prefix(&root)
            .expect("forge below install")
            .to_string_lossy()
            .replace('\\', "/");
        let forge = Forge::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut tallies: BTreeMap<u32, Tally> = BTreeMap::new();
        for entry in forge.listing() {
            let bytes = u64::try_from(entry.size).expect("validated forge entry size must be nonnegative");
            let example = json!({"forge": relative, "entry": entry.entry,
                "id": format!("{:#018x}", entry.id), "name": entry.name});
            for tally in [
                tallies.entry(entry.type_id).or_default(),
                all.entry(entry.type_id).or_default(),
            ] {
                tally.entries += 1;
                tally.bytes += bytes;
                if tally.examples.len() < 3 {
                    tally.examples.push(example.clone());
                }
            }
            entry_total += 1;
            byte_total += bytes;
        }
        reports.push(json!({"forge": relative, "archive_bytes": forge.file_size, "types": rows(&tallies)}));
    }
    assert_eq!(all.values().map(|t| t.entries).sum::<u64>(), entry_total);
    assert_eq!(all.values().map(|t| t.bytes).sum::<u64>(), byte_total);
    let report = json!({"schema": 1, "scope": "all stored forge entries, including patch copies and out-of-scope character/animation resources",
        "measurement": "index only; status is audited code capability, not parser success or rendering coverage",
        "byte_unit": "stored entry payload; excludes archive tables and padding; not decompressed body size",
        "stored_entries": entry_total, "stored_payload_bytes": byte_total,
        "forges": reports, "types": rows(&all)});
    let output = scratch().join("anvil-resource-coverage.json");
    std::fs::create_dir_all(output.parent().expect("scratch parent")).expect("create census output folder");
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(&report).expect("serialize census"),
    )
    .expect("write census");
    println!("ANVIL_RESOURCE_CENSUS={}", output.display());
    // Keep every decision in the test log as well as the artifact: remote test logs remain useful without downloads.
    for row in rows(&all) {
        println!("ANVIL_RESOURCE_TYPE={row}");
    }
}

#[test]
fn coverage_contracts_are_unique_and_unknown_types_stay_unread() {
    let mut ids = BTreeSet::new();
    for row in coverage::EXTRA {
        assert!(ids.insert(row.type_id), "duplicate extra type {:#x}", row.type_id);
    }
    for kind in KINDS {
        assert!(
            !ids.contains(&kind.type_id()),
            "classify listed kinds in coverage::of rather than duplicate them"
        );
        assert_eq!(coverage::of(kind.type_id()).name, kind.name());
    }
    assert_eq!(coverage::of(0xDEAD_BEEF).status, coverage::Status::Unread);
}

/// Bounded body observations for the named open map rows, distinct from the complete index denominator above.
/// Prefix bytes are observations, never scanned for coincidental reference ids or called a decoded schema.
#[test]
fn map_resource_profile_samples_name_the_next_format_observation() {
    use anvil_formats::{
        archive::{Archive, At},
        gidyn::StoredGiDyn,
        listing::Kind,
        look::StoredLook,
    };
    let Some(root) = install(Game::Riders) else {
        return;
    };
    let mut paths = Vec::new();
    forges(&root, &mut paths).expect("enumerate install");
    paths.sort();
    let mut rows = Vec::new();
    for path in paths {
        let mut archive = Archive::open(&[&path]).expect("open forge for profile samples");
        let mut samples = BTreeMap::<u32, usize>::new();
        let selected: Vec<_> = archive
            .forge(0)
            .expect("one forge")
            .listing()
            .filter_map(|entry| {
                let row = coverage::of(entry.type_id);
                let known_open =
                    row.status != coverage::Status::Read && row.name != "unknown or unclassified resource";
                let new_profile = matches!(
                    entry.kind,
                    Some(Kind::GiDynResource | Kind::AmbiancePreset | Kind::ForecastPreset)
                );
                let count = samples.entry(entry.type_id).or_default();
                if (!known_open && !new_profile) || *count >= 3 {
                    return None;
                }
                *count += 1;
                Some((entry.entry, entry.type_id, entry.id, entry.name.to_owned()))
            })
            .collect();
        for (entry, type_id, id, name) in selected {
            let contract = coverage::of(type_id);
            let mut row = json!({"forge": path.strip_prefix(&root).expect("below install").to_string_lossy(),
                "entry": entry, "id": format!("{id:#018x}"), "type_id": format!("{type_id:#010x}"),
                "type": contract.name, "name": name, "next_observation": contract.question});
            match archive.resource(At { forge: 0, entry }) {
                Err(error) => {
                    row["resource_error"] = json!(error.to_string());
                }
                Ok(resource) => {
                    assert_eq!(resource.type_id, type_id, "index/resource type disagreement");
                    row["body_bytes"] = json!(resource.body.len());
                    row["prefix_hex"] = json!(
                        resource
                            .body
                            .iter()
                            .take(96)
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    );
                    let parsed =
                        if type_id == anvil_formats::gidyn::GI_DYN_RESOURCE {
                            Some(StoredGiDyn::parse(&resource.body).map(
                                |s| json!({"cell": s.cell_id, "samples": s.samples, "probes": s.probes}),
                            ))
                        } else if matches!(
                            type_id,
                            anvil_formats::look::AMBIANCE_PRESET | anvil_formats::look::FORECAST_PRESET
                        ) {
                            Some(
                                StoredLook::parse(type_id, &resource.body)
                                    .map(|_| json!("stored profile closed")),
                            )
                        } else {
                            None
                        };
                    if let Some(result) = parsed {
                        match result {
                            Ok(value) => row["parsed"] = value,
                            Err(why) => row["profile_refusal"] = json!(why),
                        }
                    }
                }
            }
            println!("ANVIL_MAP_PROFILE={row}");
            rows.push(row);
        }
    }
    let output = scratch().join("anvil-map-resource-profiles.json");
    std::fs::create_dir_all(output.parent().expect("scratch parent")).expect("create profile folder");
    std::fs::write(
        &output,
        serde_json::to_vec_pretty(&json!({"sample_limit_per_type_per_forge": 3,
        "scope": "bounded samples, not total parser coverage", "samples": rows}))
        .expect("serialize profiles"),
    )
    .expect("write profiles");
    println!("ANVIL_MAP_PROFILE_CENSUS={}", output.display());
}
