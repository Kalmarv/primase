//! Census of collision bounds: every ybn of an install (every copy, in every archive) read by `bounds::read_ybn`.
//! Failures are counted by message; resource versions, bound kinds (nested composites' children included), polygon
//! kinds, material counts and the composite transforms' fourth words are counted. Every ybn must read.
//!
//! Needs `installs.gta` and `keys.gta`; the debug key `gta.archive_filter` limits the archives to paths containing that
//! text.
mod inputs;

use std::collections::BTreeMap;
use std::fs::File;
use std::sync::Mutex;

use inputs::{Game, install, keys};
use rage_engine_formats::archive::archive_set::Install;
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::archive::rpf::EntryKind;
use rage_engine_formats::bounds::{Bound, Contents, Polygon, read_ybn};

#[derive(Default)]
struct Tally {
    files: u64,
    versions: BTreeMap<u32, u64>,
    kinds: BTreeMap<String, u64>,
    failures: BTreeMap<String, (u64, Vec<String>)>,
}

fn count(b: &Bound, t: &mut BTreeMap<String, u64>) {
    *t.entry(format!("bound kind {}", b.header.kind)).or_default() += 1;
    match &b.contents {
        Contents::Geometry(g) => {
            *t.entry("geometry vertices".into()).or_default() += g.vertices.len() as u64;
            *t.entry(format!("geometry materials {}", g.materials.len().min(9)))
                .or_default() += 1;
            for p in &g.polygons {
                let k = match p {
                    Polygon::Triangle { .. } => "polygon triangle",
                    Polygon::Sphere { .. } => "polygon sphere",
                    Polygon::Capsule { .. } => "polygon capsule",
                    Polygon::Box { .. } => "polygon box",
                    Polygon::Cylinder { .. } => "polygon cylinder",
                };
                *t.entry(k.into()).or_default() += 1;
            }
            if g.vertices.iter().flatten().any(|v| !v.is_finite()) {
                *t.entry("geometry with vertices that are not finite".into())
                    .or_default() += 1;
            }
        }
        Contents::Composite(children) => {
            *t.entry("composite children".into()).or_default() += children.len() as u64;
            for c in children {
                count(&c.bound, t);
            }
        }
        _ => {}
    }
}

#[test]
fn every_ybn_reads() {
    let (Some(dir), Some(key_dir)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let filter = inputs::debug("gta.archive_filter").unwrap_or_default();
    let keys = Keys::from_dir(&key_dir, None).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let tally = Mutex::new(Tally::default());
    let next = Mutex::new(0usize);
    std::thread::scope(|s| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            s.spawn(|| {
                loop {
                    let ai = {
                        let mut n = next.lock().expect("lock");
                        *n += 1;
                        *n - 1
                    };
                    let Some(a) = install.archives.get(ai) else { break };
                    if !a.path.contains(&filter) {
                        continue;
                    }
                    let mut f: Option<File> = None;
                    for (ei, e) in a.entries.iter().enumerate() {
                        if !e.path.ends_with(".ybn") || !matches!(e.kind, EntryKind::Resource { .. }) {
                            continue;
                        }
                        let file = f.get_or_insert_with(|| File::open(&a.file).expect("open archive"));
                        let mut kinds = BTreeMap::new();
                        let mut version = None;
                        let result = a.resource(file, ei, &keys).and_then(|r| {
                            version = Some(r.version);
                            let b = read_ybn(&r)?;
                            count(&b, &mut kinds);
                            Ok(())
                        });
                        let mut t = tally.lock().expect("lock");
                        t.files += 1;
                        if let Some(v) = version {
                            *t.versions.entry(v).or_default() += 1;
                        }
                        for (k, v) in kinds {
                            *t.kinds.entry(k).or_default() += v;
                        }
                        if let Err(err) = result {
                            let key = match err {
                                rage_engine_formats::Error::Invalid { format, detail, .. } => {
                                    format!("{format}: {detail}")
                                }
                                other => other.to_string(),
                            };
                            let v = t.failures.entry(key).or_default();
                            v.0 += 1;
                            if v.1.len() < 3 {
                                v.1.push(e.path.clone());
                            }
                        }
                    }
                }
            });
        }
    });
    let t = tally.into_inner().expect("lock");
    println!("ybn census: {} files; versions {:?}", t.files, t.versions);
    println!("kinds: {:?}", t.kinds);
    for (msg, (n, paths)) in &t.failures {
        println!("  {n}x {msg}\n      e.g. {}", paths.join(", "));
    }
    assert!(t.failures.is_empty(), "{} failure kinds", t.failures.len());
}
