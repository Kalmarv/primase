//! Gates of the post-compile Frostbite readers and writers (`primase_reskate::postcompile`).
//!
//! - Synthetic round trips: containers built by our writers read back to the same values and write again to the same
//!   bytes; the TOC's perfect hashes resolve every key.
//! - `patch_round_trip` (`PRIMASE_RESKATE_PATCH`, a Patch folder written by ReSkate Studio's compile, plus
//!   `PRIMASE_SKATE_DATA`, the game's `Data` folder, and `PRIMASE_OODLE9`, the game's Oodle DLL, for the bundles a
//!   Patch places in the base game): every TOC, bundle region, bundle manifest, shader block depot, shader lookup
//!   table and `layout.toc` read and written again is byte-identical; every MeshSet reads.
//! - `patch_oracle` (`PRIMASE_RESKATE_FB_ORACLE`): the same Patch summarised by an independent reader (oracle: the
//!   Python fbpatch / sbdfmt readers; JSON schema in [`summary`]) equals our summary, compared as parsed JSON.
//! - `meshset_census` (`PRIMASE_SKATE_DATA`, `PRIMASE_OODLE9`): every MeshSet of every base-game TOC reads, failures
//!   counted by message.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use primase_compress::oodle::{Oodle, Version};
use primase_core::gate::{GateReport, Tolerance, compare_json, real_data};
use primase_reskate::postcompile::fb::bundle::{self, Asset, Kind, Manifest};
use primase_reskate::postcompile::fb::folder::{Folder, Source};
use primase_reskate::postcompile::fb::toc::{self, Loc, TocChunk};
use primase_reskate::postcompile::fb::{MESHSET, db, depot, lookup};
use primase_reskate::postcompile::meshset;
use serde_json::{Value, json};

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn loc(chunk: u32, archive: u16) -> Loc {
    Loc {
        patch: true,
        chunk,
        archive,
    }
}

#[test]
fn synthetic_toc_round_trip() -> R {
    let names = [
        "win32/levels/game/a/a",
        "win32/levels/game/a/B_sublevel",
        "win32/levels/game/a/c",
        "win32/x",
    ];
    let bundles: Vec<(String, Vec<u8>)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                n.to_string(),
                bundle::write_region(
                    &[
                        (loc(1, 2), 16 * i as u32, 100),
                        (loc(1, 2), 200, 7),
                        (loc(3, 1), 0, 9),
                    ],
                    &[],
                ),
            )
        })
        .collect();
    let chunks: Vec<TocChunk> = (0u8..9)
        .map(|i| TocChunk {
            guid: [i.wrapping_mul(37); 16],
            file: (i != 4).then_some((loc(1, 3), u32::from(i) * 64, 64)),
        })
        .collect();
    let bytes = toc::write(&bundles, &chunks, 3)?;
    let t = toc::read(&bytes)?;
    toc::verify(&t)?;
    let mut got: Vec<_> = t.bundles.clone();
    got.sort();
    let mut want = bundles.clone();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(t.chunks.len(), chunks.len());
    for c in &chunks {
        assert!(t.chunks.contains(c));
    }
    assert_eq!(toc::write(&t.bundles, &t.chunks, t.flags)?, bytes);
    for (_, region) in &t.bundles {
        let (files, inline) = bundle::read_region(region)?;
        assert_eq!(&bundle::write_region(&files, &inline), region);
    }
    Ok(())
}

#[test]
fn synthetic_manifest_round_trip() -> R {
    let mut ebx = Asset {
        kind: Kind::Ebx,
        name: "a/b".into(),
        sha1: [1; 20],
        size: 10,
        rtype: 0,
        rmeta: [0; 16],
        rid: 0,
        guid: [0; 16],
        loff: 0,
        lsize: 0,
    };
    let mut res = ebx.clone();
    res.kind = Kind::Res;
    res.name = "a/b_mesh".into();
    res.rtype = MESHSET;
    res.rmeta = [7; 16];
    res.rid = 99;
    let mut chunk = ebx.clone();
    chunk.kind = Kind::Chunk;
    chunk.name.clear();
    chunk.guid = [5; 16];
    chunk.lsize = 1234;
    chunk.size = 1234;
    ebx.size = 11;
    let m = Manifest {
        ebx: vec![ebx],
        res: vec![res],
        chunks: vec![chunk],
        meta: vec![1, 2, 3, 0],
        big_endian: false,
    };
    let bytes = bundle::write_manifest(&m)?;
    assert!(bundle::is_manifest(&bytes));
    let back = bundle::read_manifest(&bytes)?;
    assert_eq!(back, m);
    assert_eq!(bundle::write_manifest(&back)?, bytes);
    Ok(())
}

#[test]
fn synthetic_depot_round_trip() -> R {
    let block = |n: u32| {
        let mut b = n.to_le_bytes().to_vec();
        for i in 0..n {
            b.extend((0x1000u64 * u64::from(i + 1)).to_le_bytes());
            b.extend(0x2bb6_5c2du32.to_le_bytes());
            b.extend([1, 0, 0, 0, 1, 0, 0, 0]);
            b.extend(1.5f32.to_le_bytes());
        }
        b
    };
    let dep = depot::Depot {
        head: 0x10,
        version: 0x0119_0003,
        entries: vec![(3, 0), (1, 1), (2, 0)],
        blocks: vec![
            depot::Block {
                hash: 0xAA,
                data: block(2),
            },
            depot::Block {
                hash: 0xBB,
                data: block(1),
            },
        ],
    };
    let (bytes, meta) = depot::write(&dep);
    let back = depot::read(&bytes, &meta)?;
    assert_eq!(back, dep);
    assert_eq!(depot::write(&back), (bytes, meta));
    assert_eq!(depot::params(&dep.blocks[0].data)?.len(), 2);
    Ok(())
}

#[test]
fn synthetic_db_round_trip() -> R {
    let leaf = |name: &str, ty: u8, payload: Vec<u8>| db::Node {
        ty,
        named: true,
        name: name.into(),
        payload,
        ..db::Node::default()
    };
    let mut obj = db::Node {
        ty: 2,
        terminated: true,
        ..db::Node::default()
    };
    obj.children = vec![
        leaf("name", 7, b"win32/x\0".to_vec()),
        leaf("persistentIndex", 8, 3u32.to_le_bytes().to_vec()),
    ];
    let list = db::Node {
        ty: 1,
        named: true,
        name: "installChunks".into(),
        children: vec![obj],
        terminated: true,
        ..db::Node::default()
    };
    let root = db::Node {
        ty: 2,
        named: false,
        terminated: true,
        children: vec![list],
        ..db::Node::default()
    };
    let mut env = vec![0u8; toc::ENVELOPE];
    env[..4].copy_from_slice(&toc::MAGIC);
    let bytes = db::write_file(&env, &root, &[])?;
    let (e, back, trailer) = db::read_file(&bytes)?;
    assert_eq!((e, &back, trailer.len()), (env.clone(), &root, 0));
    assert_eq!(db::write_file(&env, &back, &[])?, bytes);
    Ok(())
}

#[test]
fn synthetic_program_lookup_round_trip() -> R {
    let mut d = vec![9u8; 24];
    d.extend(2u32.to_le_bytes());
    for (k, row) in [(1u64, vec![5u32, 6]), (7, vec![8])] {
        d.extend(k.to_le_bytes());
        d.extend((row.len() as u32).to_le_bytes());
        for p in row {
            d.extend(p.to_le_bytes());
        }
    }
    let mut meta = [0u8; 16];
    meta[0..4].copy_from_slice(&20u32.to_le_bytes());
    meta[4..8].copy_from_slice(&4u32.to_le_bytes());
    meta[8..12].copy_from_slice(&((d.len() - 24) as u32).to_le_bytes());
    let t = lookup::read_programs(&d, &meta)?;
    assert_eq!(t.rows[&1], vec![5, 6]);
    assert_eq!(lookup::write_programs(&t, &meta), (d, meta));
    Ok(())
}

fn oodle() -> Option<Oodle> {
    let p = real_data("PRIMASE_OODLE9")?;
    // SAFETY: the caller names the game's own Oodle DLL in PRIMASE_OODLE9.
    #[allow(unsafe_code)]
    let o = unsafe { Oodle::load(p, Version::V9) };
    Some(o.expect("PRIMASE_OODLE9 loads"))
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
    let (mut programs, mut texture_sets) = (0usize, 0usize);
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
        json!({"tocs": tocs, "depots": depots, "meshsets": meshsets, "programs": programs, "texture_sets": texture_sets}),
    )
}

fn base_and_oodle() -> R<(Option<Folder>, Option<Oodle>)> {
    let base = match real_data("PRIMASE_SKATE_DATA") {
        Some(p) => Some(Folder::open(p)?),
        None => None,
    };
    Ok((base, oodle()))
}

#[test]
fn patch_round_trip() -> R {
    let Some(patch) = real_data("PRIMASE_RESKATE_PATCH") else {
        return Ok(());
    };
    let (base, oodle) = base_and_oodle()?;
    let mut report = GateReport::new("Studio Patch read + write", Tolerance::Exact);
    let s = summary(&patch, base.as_ref(), oodle.as_ref(), &mut report)?;
    println!("{report}");
    println!(
        "meshsets {} depots {} program rows {} texture-set rows {}",
        s["meshsets"].as_array().map_or(0, Vec::len),
        s["depots"].as_array().map_or(0, Vec::len),
        s["programs"],
        s["texture_sets"]
    );
    report.assert_passed();
    Ok(())
}

#[test]
fn patch_oracle() -> R {
    let Some(patch) = real_data("PRIMASE_RESKATE_PATCH") else {
        return Ok(());
    };
    let Some(oracle) = real_data("PRIMASE_RESKATE_FB_ORACLE") else {
        return Ok(());
    };
    let (base, oodle) = base_and_oodle()?;
    let mut report = GateReport::new("round trip (not this gate)", Tolerance::Exact);
    let ours = summary(&patch, base.as_ref(), oodle.as_ref(), &mut report)?;
    let theirs: Value = serde_json::from_slice(&std::fs::read(oracle)?)?;
    let c = compare_json(&ours, &theirs, 0);
    for d in c.differences.iter().take(20) {
        println!("differs: {d}");
    }
    assert!(
        c.equal(),
        "{} differences against the independent reader",
        c.differences.len()
    );
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
        let t = toc::read(&std::fs::read(base.toc_path(rel))?)?;
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
