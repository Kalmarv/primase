//! Semantic coverage of embedded Entity components, not another resource-index inventory.
//! Reads Entity/EntityGroup/GridCellDataBlock bodies one at a time. Patch copies are counted separately;
//! grid membership is the union of stored lists and group memberships, not an active world-state selection.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use primase_anvil::read::{
    archive::{Archive, At},
    component_types::component_name,
    ddecal::{HOUDINI_DECAL_INSTANCER, STATIC_DDECAL},
    entity::{ComponentBody, ENTITY, ENTITY_GROUP, StoredEntity, StoredEntityGroup},
    grid::{GRID_CELL_DATA_BLOCK, StoredGridCellDataBlock},
    water::WATER_COMPONENT,
};
use primase_testdata::{Game, install, scratch};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SAMPLES_PER_ROW: usize = 3;
const MAX_SAMPLE_BYTES: usize = 256 * 1024;
const TOTAL_SAMPLE_BYTES: usize = 32 * 1024 * 1024;

fn forges(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            forges(&entry.path(), out)?;
        } else if kind.is_file()
            && entry.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("forge"))
        {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// Only actual GridCellDataBlock references and EntityGroup members participate. In particular, Entity.links
/// can be UI targets and opaque payload words are not references. Cycles and repeated patch edges are harmless.
fn listed_closure(roots: &BTreeSet<u64>, groups: &BTreeMap<u64, BTreeSet<u64>>) -> BTreeSet<u64> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<_> = roots.iter().copied().collect();
    while let Some(id) = pending.pop() {
        if reached.insert(id)
            && let Some(members) = groups.get(&id)
        {
            pending.extend(members.iter().copied());
        }
    }
    reached
}

/// Raw is a storage representation, not a semantic verdict: these three families are read by placements.rs.
fn classification(kind: u32, body: &ComponentBody) -> (&'static str, Option<&[u8]>) {
    match body {
        ComponentBody::Raw(bytes)
            if matches!(kind, WATER_COMPONENT | HOUDINI_DECAL_INSTANCER | STATIC_DDECAL) =>
        {
            ("downstream-reader", Some(bytes))
        }
        ComponentBody::Raw(bytes) => ("length-rule-only", Some(bytes)),
        ComponentBody::Unread(bytes) => ("header-delimited-unread", Some(bytes)),
        ComponentBody::Refused { bytes, .. } => ("component-refused", Some(bytes)),
        _ => ("typed-subset", None),
    }
}

#[derive(Default)]
struct Tally {
    components: u64,
    owners: u64,
    retained_body_bytes: u64,
    body_lengths: BTreeMap<usize, u64>,
    examples: Vec<Value>,
}

#[derive(Default)]
struct Outcome {
    entries: u64,
    stored_bytes: u64,
    examples: Vec<Value>,
}

fn outcome(
    rows: &mut BTreeMap<(u32, String), Outcome>,
    kind: u32,
    status: &str,
    bytes: u64,
    example: Value,
) {
    let tally = rows.entry((kind, status.to_owned())).or_default();
    tally.entries += 1;
    tally.stored_bytes += bytes;
    if tally.examples.len() < SAMPLES_PER_ROW {
        tally.examples.push(example);
    }
}

fn outcome_rows(rows: &BTreeMap<(u32, String), Outcome>) -> Vec<Value> {
    rows.iter().map(|((kind, status), tally)| json!({
        "type_id": format!("{kind:#010x}"), "status": status, "entries": tally.entries,
        "stored_payload_bytes": tally.stored_bytes, "examples": tally.examples,
    })).collect()
}

#[test]
fn embedded_components_report_semantic_gaps_and_grid_listed_owners() {
    let Some(root) = install(Game::Riders) else { return; };
    let mut paths = Vec::new();
    forges(&root, &mut paths).expect("enumerate Riders forge files");
    paths.sort();
    assert!(!paths.is_empty(), "configured Riders install has no forge files");
    let output = scratch().join("anvil-entity-content");
    std::fs::create_dir_all(&output).expect("create census output folder");
    let mut roots = BTreeSet::new();
    let mut groups: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let mut graph_outcomes = BTreeMap::new();

    // Establish a reproducible map-candidate cohort without guessing from owner names. A missing/refused graph
    // record is counted explicitly: the cohort is then a lower bound even before state/patch selection.
    for path in &paths {
        let relative = path.strip_prefix(&root).expect("forge below install").to_string_lossy();
        let mut archive = Archive::open(&[path]).expect("open census forge");
        let entries: Vec<_> = archive.forge(0).expect("one forge").listing()
            .filter(|e| matches!(e.type_id, GRID_CELL_DATA_BLOCK | ENTITY_GROUP))
            .map(|e| (e.entry, e.type_id, e.id, e.name.to_owned(), e.size)).collect();
        for (entry, kind, id, name, bytes) in entries {
            let mut example = json!({"forge": relative, "entry": entry, "id": format!("{id:#018x}"), "name": name});
            let result = archive.resource(At { forge: 0, entry }).map_err(|e| e.to_string())
                .and_then(|r| {
                    if r.type_id != kind { return Err("index/resource type disagreement".to_owned()); }
                    if kind == GRID_CELL_DATA_BLOCK {
                        let block = StoredGridCellDataBlock::parse(&r.body).map_err(|e| e.to_string())?;
                        roots.extend(block.objects.iter().map(|r| r.id));
                    } else {
                        let group = StoredEntityGroup::parse(&r.body).map_err(|e| e.to_string())?;
                        groups.entry(id).or_default().extend(group.members.iter().map(|r| r.id));
                    }
                    Ok(())
                });
            let status = match result {
                Ok(()) => "parsed",
                Err(why) => { example["why"] = json!(why); "refused" }
            };
            outcome(&mut graph_outcomes, kind, status, u64::try_from(bytes).expect("nonnegative entry size"), example);
        }
    }
    let listed = listed_closure(&roots, &groups);
    let mut tallies: BTreeMap<(bool, u32, &'static str), Tally> = BTreeMap::new();
    let mut owner_outcomes = BTreeMap::new();
    let mut indexed_owners = 0u64;
    let mut indexed_owner_bytes = 0u64;
    let mut sample_bytes = 0usize;
    let mut sample_number = 0u64;
    for path in &paths {
        let relative = path.strip_prefix(&root).expect("forge below install").to_string_lossy();
        let mut archive = Archive::open(&[path]).expect("open census forge");
        let entries: Vec<_> = archive.forge(0).expect("one forge").listing()
            .filter(|e| matches!(e.type_id, ENTITY | ENTITY_GROUP))
            .map(|e| (e.entry, e.type_id, e.id, e.name.to_owned(), e.size)).collect();
        let count = entries.len();
        for (entry, kind, id, name, bytes) in entries {
            let bytes = u64::try_from(bytes).expect("nonnegative entry size");
            indexed_owners += 1;
            indexed_owner_bytes += bytes;
            let grid_listed = listed.contains(&id);
            let mut owner = json!({"forge": relative, "entry": entry, "owner_id": format!("{id:#018x}"),
                "owner_type": format!("{kind:#010x}"), "name": name, "grid_listed": grid_listed});
            let parsed = archive.resource(At { forge: 0, entry }).map_err(|e| e.to_string())
                .and_then(|r| {
                    if r.type_id != kind { return Err("index/resource type disagreement".to_owned()); }
                    if kind == ENTITY {
                        StoredEntity::parse(&r.body).map_err(|e| e.to_string())
                    } else {
                        StoredEntityGroup::parse(&r.body).map(|g| g.entity).map_err(|e| e.to_string())
                    }
                });
            let entity = match parsed {
                Ok(entity) => entity,
                Err(why) => {
                    owner["why"] = json!(why);
                    outcome(&mut owner_outcomes, kind, "refused", bytes, owner);
                    continue;
                }
            };
            outcome(&mut owner_outcomes, kind, "parsed", bytes, owner.clone());
            owner["matrix"] = json!(entity.matrix);
            owner["scale"] = json!(entity.scale);
            owner["data_layers"] = json!(entity.data_layers);
            owner["hidden"] = json!(entity.is_hidden());
            owner["phantom"] = json!(entity.is_phantom());
            owner["has_visual"] = json!(entity.visuals().next().is_some());
            // These are facts about co-occurrence, not a heuristic exclusion of animated map props.
            owner["has_skeleton_or_animation"] = json!(entity.components.iter().any(|c|
                matches!(&c.body, ComponentBody::Skeleton(_) | ComponentBody::Anim(_))));
            let mut seen = BTreeSet::new();
            for component in &entity.components {
                let (status, body) = classification(component.kind, &component.body);
                let key = (grid_listed, component.kind, status);
                let tally = tallies.entry(key).or_default();
                tally.components += 1;
                if seen.insert(key) { tally.owners += 1; }
                if let Some(body) = body {
                    tally.retained_body_bytes += u64::try_from(body.len()).expect("body length fits u64");
                    *tally.body_lengths.entry(body.len()).or_default() += 1;
                }
                if tally.examples.len() < SAMPLES_PER_ROW {
                    let mut example = owner.clone();
                    example["component_id"] = json!(format!("{:#018x}", component.id));
                    if let ComponentBody::Refused { why, .. } = &component.body {
                        example["why"] = json!(why.reason());
                    }
                    if let Some(body) = body {
                        example["body_bytes"] = json!(body.len());
                        example["sha256"] = json!(Sha256::digest(body).iter().map(|b| format!("{b:02x}")).collect::<String>());
                        // Samples are complete bodies or explicitly absent, never truncated evidence for a schema.
                        if status != "downstream-reader" && body.len() <= MAX_SAMPLE_BYTES
                            && body.len() <= TOTAL_SAMPLE_BYTES.saturating_sub(sample_bytes)
                        {
                            let file = format!("component-{sample_number:06}.bin");
                            std::fs::write(output.join(&file), body).expect("write full component sample");
                            example["body_file"] = json!(file);
                            sample_number += 1;
                            sample_bytes += body.len();
                        } else {
                            example["body_not_saved"] = json!("downstream reader, per-body cap or total sample budget");
                        }
                    }
                    tally.examples.push(example);
                }
            }
        }
        println!("ANVIL_ENTITY_CONTENT_FORGE={}", json!({"forge": relative, "owners": count}));
    }
    assert_eq!(owner_outcomes.values().map(|r| r.entries).sum::<u64>(), indexed_owners);
    assert_eq!(owner_outcomes.values().map(|r| r.stored_bytes).sum::<u64>(), indexed_owner_bytes);
    let rows: Vec<_> = tallies.iter().map(|((listed, kind, status), tally)| json!({
        "grid_listed": listed, "component_type": format!("{kind:#010x}"), "component": component_name(*kind),
        "status": status, "components": tally.components, "owners": tally.owners,
        "retained_body_bytes": tally.retained_body_bytes,
        "body_bytes_measured": *status != "typed-subset",
        "body_lengths": tally.body_lengths, "examples": tally.examples,
    })).collect();
    let graph = outcome_rows(&graph_outcomes);
    let outcomes = outcome_rows(&owner_outcomes);
    let report = json!({"schema": 1,
        "scope": "all stored Entity/EntityGroup entries, patch duplicates included; component bodies exclude headers",
        "grid_cohort": "union of stored GridCellDataBlock references, transitively expanded through stored EntityGroup members; no World/state/patch activation claim",
        "cohort_is_lower_bound": graph_outcomes.keys().any(|(_, status)| status == "refused"),
        "typed_subset_note": "typed-subset has no body-length observation; zero retained_body_bytes is not zero stored bytes or proof of complete semantics",
        "downstream_note": "water/deferred-decal Raw bodies have separate production readers; this census does not assert their successful lowering",
        "owners": indexed_owners, "stored_owner_payload_bytes": indexed_owner_bytes,
        "graph": graph, "owner_outcomes": outcomes, "components": rows,
        "sample_bytes": sample_bytes, "sample_body_count": sample_number,
        "sample_limits": {"per_row": SAMPLES_PER_ROW, "per_body_bytes": MAX_SAMPLE_BYTES, "total_bytes": TOTAL_SAMPLE_BYTES}});
    std::fs::write(output.join("report.json"), serde_json::to_vec_pretty(&report).expect("serialize census"))
        .expect("write census report");
    for row in &graph { println!("ANVIL_ENTITY_GRAPH={row}"); }
    for row in &outcomes { println!("ANVIL_ENTITY_OWNER={row}"); }
    for row in &rows { println!("ANVIL_ENTITY_COMPONENT={row}"); }
    println!("ANVIL_ENTITY_CONTENT_CENSUS={}", output.join("report.json").display());
}

#[test]
fn listed_membership_uses_group_edges_and_survives_cycles_and_patch_unions() {
    let roots = BTreeSet::from([1]);
    let mut groups = BTreeMap::from([(1, BTreeSet::from([2])), (2, BTreeSet::from([1, 3]))]);
    groups.entry(1).or_default().insert(4); // a second stored copy's membership
    groups.insert(9, BTreeSet::from([10])); // unlisted group, not a map membership claim
    assert_eq!(listed_closure(&roots, &groups), BTreeSet::from([1, 2, 3, 4]));
}

#[test]
fn raw_water_and_decals_are_not_ranked_as_missing_semantic_readers() {
    let bytes = ComponentBody::Raw(vec![1, 2, 3]);
    for kind in [WATER_COMPONENT, HOUDINI_DECAL_INSTANCER, STATIC_DDECAL] {
        assert_eq!(classification(kind, &bytes), ("downstream-reader", Some(&[1, 2, 3][..])));
    }
    assert_eq!(classification(0x713F_0C7D, &bytes), ("length-rule-only", Some(&[1, 2, 3][..])));
    let unread = ComponentBody::Unread(vec![4]);
    assert_eq!(classification(WATER_COMPONENT, &unread), ("header-delimited-unread", Some(&[4][..])));
}
