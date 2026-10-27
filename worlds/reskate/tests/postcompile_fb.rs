//! Gates of the post-compile Frostbite readers and writers (`primase_reskate::postcompile`).
//!
//! The containers' synthetic gates are in `repos/frostbite-formats/tests/containers.rs`.
//! - `patch_round_trip` (`oracles.studio.gta/legion/patch`, a Patch folder written by ReSkate Studio's compile, plus
//!   `installs.skate/Data`, the game's `Data` folder, and `tools.oodle9`, the game's Oodle DLL, for the bundles a
//!   Patch places in the base game): every TOC, bundle region, bundle manifest, shader block depot, shader lookup
//!   table and `layout.toc` read and written again is byte-identical; every MeshSet reads with its metadata, and
//!   written in the game's layout reads back with the same records, names and lists.
//! - `meshset_census` (`installs.skate/Data`, `tools.oodle9`): every MeshSet of every base-game TOC reads, failures
//!   counted by message.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::gate::{GateReport, Tolerance};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_reskate::postcompile::fb::bundle;
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::fb::toc;
use primase_reskate::postcompile::fb::{MESHSET, db, depot, lookup};
use primase_reskate::postcompile::meshset;
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use serde_json::{Value, json};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A MeshSet declaring more sections than its bytes can hold is refused before anything is reserved.
#[test]
fn corrupt_section_count_is_an_error() {
    // MeshSet: the plain header (0xd0 bytes, LOD records of 0xb0, sections of 0x180; null names read as the empty
    // string at byte 16), one LOD at 0xd0 declaring 0x7fffffff sections
    let mut ms = vec![0u8; 0x200];
    for (at, v) in [(0, 0xc0u32), (4, 0xb0), (8, 0x180)] {
        ms[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    ms[0x30..0x38].copy_from_slice(&0xc0u64.to_le_bytes());
    ms[0xb4..0xb6].copy_from_slice(&1u16.to_le_bytes());
    ms[0xd0 + 8..0xd0 + 12].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    ms[0xd0 + 0x54..0xd0 + 0x58].copy_from_slice(&0x21u32.to_le_bytes());
    let e = meshset::read(&ms)
        .expect_err("a corrupt count must be an error")
        .to_string();
    assert!(
        e.contains("entries of at least"),
        "refused for another reason: {e}"
    );
}

/// A MeshSet without where its records lay.
fn placeless(mut m: meshset::MeshSet) -> meshset::MeshSet {
    for l in &mut m.lods {
        l.at = 0;
        l.sections.iter_mut().for_each(|s| s.at = 0);
    }
    m
}

fn oodle() -> Option<Oodle> {
    let p = tool(Tool::Oodle9)?;
    // SAFETY: the configuration names the game's own Oodle DLL as tools.oodle9.
    #[allow(unsafe_code)]
    let o = unsafe { Oodle::load(p, Version::V9) };
    Some(o.expect("tools.oodle9 loads"))
}

/// Reads a Patch through our readers, checking each container's round trip into `report`, and returns its summary:
/// `{"tocs": {rel: {"flags", "bundles": [{"name", "files", "first", "ebx": [name], "res": [[name, type, size,
/// meta hex]], "chunks": [[guid hex, loff, lsize]]}], "chunks": count}}, "depots": [{"bundle", "name", "entries":
/// [[key hex, block hash hex, block length]]}], "meshsets": [{"bundle", "name", "lods": [{"chunk", "ib", "vb",
/// "sections": [[key hex, decl hex, primitives, start index, vertex offset, vertices]]}]}], "programs": rows,
/// "texture_sets": rows}`.
fn summary(patch: &Path, base: Option<&Folder>, oodle: Option<&Oodle>, report: &mut GateReport) -> R<Value> {
    let folder = Folder::open(patch)?;
    let layout = std::fs::read(patch.join("layout.toc"))?;
    let (env, root, trailer) = db::read_file(&layout)?;
    report.compare_bytes("layout.toc", &db::write_file(&env, &root, &trailer)?, &layout);
    let src = Source {
        folder: &folder,
        base,
        oodle,
    };
    let mut tocs = serde_json::Map::new();
    let (mut depots, mut meshsets) = (Vec::new(), Vec::new());
    let (mut programs, mut texture_sets, mut rewritten) = (0usize, 0usize, 0usize);
    for rel in &folder.tocs {
        let bytes = std::fs::read(folder.toc_path(rel))?;
        let t = toc::read(&bytes)?;
        toc::verify(&t)?;
        report.compare_bytes(rel.clone(), &toc::write(&t.bundles, &t.chunks, t.flags)?, &bytes);
        let mut bundles = Vec::new();
        for (name, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            report.compare_bytes(
                format!("{name} region"),
                &bundle::write_region(&files, &inline),
                region,
            );
            let Some((m, first, raw)) = src.manifest(&files, &inline)? else {
                bundles.push(json!({"name": name, "files": files.len(), "first": null}));
                continue;
            };
            if first == 1 {
                report.compare_bytes(format!("{name} manifest"), &bundle::write_manifest(&m)?, &raw);
            }
            for (i, a) in m.res.iter().enumerate() {
                let f = files[first + m.ebx.len() + i];
                if a.rtype == depot::RTYPE {
                    let d = src.payload(f, a.size as usize)?;
                    let dep = depot::read(&d, &a.rmeta)?;
                    let (w, wm) = depot::write(&dep);
                    report.compare_bytes(format!("{} depot", a.name), &w, &d);
                    report.compare_bytes(format!("{} depot meta", a.name), &wm, &a.rmeta);
                    let entries: Vec<Value> = dep
                        .entries
                        .iter()
                        .map(|&(k, b)| {
                            json!([
                                format!("{k:016x}"),
                                format!("{:016x}", dep.blocks[b].hash),
                                dep.blocks[b].data.len()
                            ])
                        })
                        .collect();
                    depots.push(json!({"bundle": name, "name": a.name, "entries": entries}));
                } else if a.rtype == lookup::PROGRAM_LOOKUP {
                    let d = src.payload(f, a.size as usize)?;
                    let t = lookup::read_programs(&d, &a.rmeta)?;
                    let (w, wm) = lookup::write_programs(&t, &a.rmeta);
                    report.compare_bytes(format!("{} programs", a.name), &w, &d);
                    report.compare_bytes(format!("{} programs meta", a.name), &wm, &a.rmeta);
                    programs += t.rows.len();
                } else if a.rtype == lookup::TEXTURE_LOOKUP {
                    let d = src.payload(f, a.size as usize)?;
                    let rows = lookup::read_texture_sets(&d)?;
                    report.compare_bytes(
                        format!("{} texture sets", a.name),
                        &lookup::write_texture_sets(&rows),
                        &d,
                    );
                    texture_sets += rows.len();
                } else if a.rtype == MESHSET {
                    let d = src.payload(f, a.size as usize)?;
                    let ms = meshset::read(&d)?;
                    // written in the game's layout (Studio orders its pool otherwise) and read again: the same
                    // records, names, lists and trailing data
                    let stored = placeless(meshset::read_resource(&d, &a.rmeta)?);
                    let (w, wm) = meshset::write(&stored)?;
                    let again = placeless(meshset::read_resource(&w, &wm)?);
                    if again == stored {
                        rewritten += 1;
                    } else {
                        let (x, y) = (format!("{again:?}"), format!("{stored:?}"));
                        let at = x.bytes().zip(y.bytes()).position(|(p, q)| p != q).unwrap_or(0);
                        return Err(format!(
                            "{}: written MeshSet reads back otherwise: ...{} / ...{}",
                            a.name,
                            &x[at.saturating_sub(80)..(at + 80).min(x.len())],
                            &y[at.saturating_sub(80)..(at + 80).min(y.len())]
                        )
                        .into());
                    }
                    let lods: Vec<Value> = ms
                        .lods
                        .iter()
                        .map(|l| {
                            let secs: Vec<Value> = l.sections.iter().map(|s| json!([format!("{:016x}", s.key), hex(&s.decl.bytes), s.primitives, s.start_index, s.vertex_offset, s.vertices])).collect();
                            json!({"chunk": hex(&l.chunk), "ib": l.index_bytes, "vb": l.vertex_bytes, "sections": secs})
                        })
                        .collect();
                    meshsets.push(json!({"bundle": name, "name": a.name, "lods": lods}));
                }
            }
            bundles.push(json!({
                "name": name,
                "files": files.len(),
                "first": first,
                "ebx": m.ebx.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                "res": m.res.iter().map(|a| json!([a.name, a.rtype, a.size, hex(&a.rmeta)])).collect::<Vec<_>>(),
                "chunks": m.chunks.iter().map(|a| json!([hex(&a.guid), a.loff, a.lsize])).collect::<Vec<_>>(),
            }));
        }
        tocs.insert(
            rel.clone(),
            json!({"flags": t.flags, "bundles": bundles, "chunks": t.chunks.len()}),
        );
    }
    Ok(
        json!({"tocs": tocs, "depots": depots, "meshsets": meshsets, "rewritten": rewritten, "programs": programs, "texture_sets": texture_sets}),
    )
}

fn base_and_oodle() -> R<(Option<Folder>, Option<Oodle>)> {
    let base = match install_file(Game::Skate, "Data") {
        Some(p) => Some(Folder::open(p)?),
        None => None,
    };
    Ok((base, oodle()))
}

#[test]
fn patch_round_trip() -> R {
    let Some(patch) = oracle(OracleSet::Studio, Game::Gta, "legion/patch") else {
        return Ok(());
    };
    // the Patch's chunks lie in the base game's files, Oodle-compressed: both are needed to read it
    let (Some(base), Some(oodle)) = base_and_oodle()? else {
        return Ok(());
    };
    let mut report = GateReport::new("Studio Patch read + write", Tolerance::Exact);
    let s = summary(&patch, Some(&base), Some(&oodle), &mut report)?;
    println!("{report}");
    println!(
        "meshsets {} ({} written and read back alike) depots {} program rows {} texture-set rows {}",
        s["meshsets"].as_array().map_or(0, Vec::len),
        s["rewritten"],
        s["depots"].as_array().map_or(0, Vec::len),
        s["programs"],
        s["texture_sets"]
    );
    report.assert_passed();
    Ok(())
}

#[test]
fn meshset_census() -> R {
    let (Some(base), Some(oodle)) = base_and_oodle()? else {
        return Ok(());
    };
    let src = Source {
        folder: &base,
        base: Some(&base),
        oodle: Some(&oodle),
    };
    let (mut read, mut sections, mut extended) = (0usize, 0usize, 0usize);
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for rel in &base.tocs {
        let t = base.toc(rel)?;
        for (name, region) in &t.bundles {
            let (files, inline) = bundle::read_region(region)?;
            let Some((m, first, _)) = src.manifest(&files, &inline)? else {
                continue;
            };
            for (i, a) in m.res.iter().enumerate() {
                if a.rtype != MESHSET || !seen.insert((a.name.clone(), a.sha1)) {
                    continue;
                }
                let f = files[first + m.ebx.len() + i];
                match src
                    .payload(f, a.size as usize)
                    .map_err(|e| e.to_string())
                    .and_then(|d| meshset::read(&d).map_err(|e| e.to_string()))
                {
                    Ok(ms) => {
                        read += 1;
                        sections += ms.lods.iter().map(|l| l.sections.len()).sum::<usize>();
                        extended += ms
                            .lods
                            .iter()
                            .flat_map(|l| &l.sections)
                            .filter(|x| x.draw_decl != x.decl)
                            .count();
                    }
                    Err(e) => {
                        let key = e.split(" at ").next().unwrap_or(&e).to_string()
                            + ": "
                            + e.rsplit(": ").next().unwrap_or("");
                        if failures.values().sum::<usize>() < 5 {
                            println!("{rel} {name} {}: {e}", a.name);
                        }
                        *failures.entry(key).or_default() += 1;
                    }
                }
            }
        }
    }
    println!(
        "MeshSet census: {read} read ({sections} sections, {extended} with run-time draw streams), {} failed",
        failures.values().sum::<usize>()
    );
    for (k, n) in &failures {
        println!("  {n:6} {k}");
    }
    assert!(failures.is_empty());
    Ok(())
}
