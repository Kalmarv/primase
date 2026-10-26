//! Census over every archive of a GTA V install: every table of contents read strictly, compared entry for entry
//! with an independent reader's listing of the same install, then every binary entry and every resource decoded
//! to its exact end. Failures are counted by kind.
//!
//! Oracle (`oracles.codewalker.gta/listing.jsonl`): an independent reader's listing, JSON lines, one archive per line:
//! `{"path": "x64a.rpf\\levels\\gta5\\...rpf", "file_size": n, "start": n, "encryption": u32, "entries": [[path, "dir"]
//! | [path, "bin", block, stored, size, encrypted] | [path, "res", block, stored, system, graphics]]}` with archive and
//! entry paths lowercase, relative to the install, joined by `\`. Needs `installs.gta` (the install) and `keys.gta`
//! (key material, see `Keys::from_dir`); the debug key `gta.archive_filter` limits the archives to paths containing that text.
mod inputs;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use inputs::{Game, OracleSet, install, keys};
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::archive::rpf::{Archive, EntryKind};

#[derive(serde::Deserialize)]
struct OracleArchive {
    path: String,
    file_size: u64,
    start: u64,
    encryption: u32,
    entries: Vec<Vec<serde_json::Value>>,
}

fn rpf_files(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, String)>) {
    let mut items: Vec<_> = std::fs::read_dir(dir)
        .expect("read install folder")
        .flatten()
        .map(|e| e.path())
        .collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            rpf_files(&p, root, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("rpf")) {
            let rel = p
                .strip_prefix(root)
                .expect("archive under the install folder")
                .to_string_lossy()
                .replace('/', "\\")
                .to_lowercase();
            out.push((p, rel));
        }
    }
}

/// An entry as the oracle lists it.
fn listed(a: &Archive, e: &rage_engine_formats::archive::rpf::Entry) -> Vec<serde_json::Value> {
    use serde_json::json;
    match e.kind {
        EntryKind::Directory { .. } => vec![json!(e.path), json!("dir")],
        EntryKind::Binary {
            block,
            stored,
            size,
            encrypted,
        } => {
            vec![
                json!(e.path),
                json!("bin"),
                json!(block),
                json!(stored),
                json!(size),
                json!(u32::from(encrypted)),
            ]
        }
        EntryKind::Resource {
            block,
            stored,
            system,
            graphics,
        } => {
            let _ = a;
            vec![
                json!(e.path),
                json!("res"),
                json!(block),
                json!(stored),
                json!(system.0),
                json!(graphics.0),
            ]
        }
    }
}

#[derive(Default)]
struct Tally {
    archives: u64,
    binaries: u64,
    resources: u64,
    bytes: u64,
    failures: BTreeMap<String, Vec<String>>,
    /// Resources whose body ends inside their last page (zero-filled): accepted, named, counted.
    short_last_page: Vec<String>,
}

impl Tally {
    fn fail(&mut self, kind: &str, item: String) {
        self.failures.entry(kind.to_string()).or_default().push(item);
    }
}

/// Every archive in `a` (itself and, depth first, the archives it holds), each decoded entry tallied.
fn walk(a: &Archive, f: &mut File, keys: &Keys, oracle: &BTreeMap<String, OracleArchive>, t: &mut Tally) {
    t.archives += 1;
    match oracle.get(&a.path) {
        None if !oracle.is_empty() => t.fail("archive not in the oracle", a.path.clone()),
        None => {}
        Some(o) => {
            let enc = match a.encryption {
                rage_engine_formats::archive::rpf::Encryption::None => 0,
                rage_engine_formats::archive::rpf::Encryption::Open => 0x4E45_504F,
                rage_engine_formats::archive::rpf::Encryption::Aes => 0x0FFF_FFF9,
                rage_engine_formats::archive::rpf::Encryption::Ng => 0x0FEF_FFFF,
            };
            if (o.file_size, o.start, o.encryption) != (a.size, a.start, enc) {
                t.fail("archive header differs", a.path.clone());
            }
            let ours: Vec<_> = a.entries.iter().map(|e| listed(a, e)).collect();
            if ours.len() != o.entries.len() {
                t.fail(
                    "entry count differs",
                    format!("{}: {} vs {}", a.path, ours.len(), o.entries.len()),
                );
            }
            for (x, y) in ours.iter().zip(&o.entries) {
                if x != y {
                    t.fail("entry differs", format!("{:?} vs {:?}", x, y));
                }
            }
        }
    }
    for (i, e) in a.entries.iter().enumerate() {
        match e.kind {
            EntryKind::Directory { .. } => {}
            EntryKind::Binary { .. } if e.name.to_lowercase().ends_with(".rpf") => match a.nested(f, i, keys)
            {
                Ok(n) => walk(&n, f, keys, oracle, t),
                Err(err) => t.fail("nested archive", format!("{}: {err}", e.path)),
            },
            EntryKind::Binary { size, .. } => {
                t.binaries += 1;
                match a.binary(f, i, keys) {
                    Ok(d) if d.len() == size as usize => t.bytes += d.len() as u64,
                    Ok(d) => t.fail("binary size", format!("{}: {} vs {size}", e.path, d.len())),
                    Err(err) => t.fail("binary", format!("{}: {err}", e.path)),
                }
            }
            EntryKind::Resource { .. } => {
                t.resources += 1;
                match a.resource(f, i, keys) {
                    Ok(r) => {
                        t.bytes += r.data.len() as u64;
                        if r.zero_filled > 0 {
                            t.short_last_page
                                .push(format!("{} ({} bytes)", e.path, r.zero_filled));
                        }
                    }
                    Err(err) => t.fail("resource", format!("{}: {err}", e.path)),
                }
            }
        }
    }
}

#[test]
fn every_archive_reads_and_every_entry_decodes() {
    let (Some(dir), Some(key_dir)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir, None).expect("key material");
    let filter = inputs::debug("gta.archive_filter")
        .unwrap_or_default()
        .to_lowercase();
    let mut oracle = BTreeMap::new();
    if let Some(listing) = inputs::oracle(OracleSet::CodeWalker, Game::Gta, "listing.jsonl") {
        for line in std::fs::read_to_string(listing).expect("listing").lines() {
            let o: OracleArchive = serde_json::from_str(line).expect("listing line");
            oracle.insert(o.path.clone(), o);
        }
    }
    let mut files = Vec::new();
    rpf_files(&dir, &dir, &mut files);
    files.retain(|(_, rel)| rel.contains(&filter));
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(files.len().max(1));
    let next = Mutex::new(0usize);
    let tallies: Vec<Tally> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut t = Tally::default();
                    loop {
                        let i = {
                            let mut n = next.lock().unwrap();
                            *n += 1;
                            *n - 1
                        };
                        let Some((path, rel)) = files.get(i) else { break };
                        match Archive::open(path, rel, &keys) {
                            Ok(a) => {
                                let mut f = File::open(path).expect("open archive");
                                walk(&a, &mut f, &keys, &oracle, &mut t);
                            }
                            Err(err) => t.fail("archive", format!("{rel}: {err}")),
                        }
                    }
                    t
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut t = Tally::default();
    for x in tallies {
        t.archives += x.archives;
        t.binaries += x.binaries;
        t.resources += x.resources;
        t.bytes += x.bytes;
        t.short_last_page.extend(x.short_last_page);
        for (k, mut v) in x.failures {
            t.failures.entry(k).or_default().append(&mut v);
        }
    }
    let listed_in_scope = oracle.keys().filter(|p| p.contains(&filter)).count() as u64;
    if !oracle.is_empty() && filter.is_empty() && listed_in_scope != t.archives {
        t.fail(
            "archive count",
            format!("{} read, {} listed", t.archives, listed_in_scope),
        );
    }
    println!(
        "census: {} archives, {} binary entries, {} resources, {:.1} GiB decoded",
        t.archives,
        t.binaries,
        t.resources,
        t.bytes as f64 / (1u64 << 30) as f64
    );
    t.short_last_page.sort();
    println!(
        "  resources ending inside their last page (zero-filled): {} {:?}",
        t.short_last_page.len(),
        t.short_last_page
    );
    for (k, v) in &mut t.failures {
        v.sort();
        println!(
            "  {k}: {} failures, e.g. {}",
            v.len(),
            v.iter().take(3).cloned().collect::<Vec<_>>().join(" | ")
        );
    }
    assert!(t.failures.is_empty(), "{} failure kinds", t.failures.len());
}
