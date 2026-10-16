//! Census of META resources: every ymap and ytyp of an install read as a META container and its root structure
//! decoded through every array and pointer, reaching every byte of every data block (a structure covers its extent,
//! a string its NUL). Failures are counted by message, unreached bytes by block type.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::collections::BTreeMap;
use std::fs::File;
use std::sync::Mutex;

use primase_core::gate::real_data;
use primase_gta::meta::Meta;
use primase_gta::{EntryKind, Install, Keys};

#[derive(Default)]
struct Tally {
    read: BTreeMap<String, u64>,
    /// Per extension and block type: blocks with bytes no decoding reached, and those bytes.
    unreached: BTreeMap<(String, u32), (u64, u64)>,
    unreached_examples: Vec<String>,
    failures: BTreeMap<String, Vec<String>>,
}

#[test]
fn every_ymap_and_ytyp_decodes() {
    let (Some(dir), Some(key_dir)) = (real_data("PRIMASE_GTA_DIR"), real_data("PRIMASE_GTA_KEYS")) else {
        return;
    };
    let keys = Keys::from_dir(&key_dir).expect("key material");
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
                    let mut f: Option<File> = None;
                    for (ei, e) in a.entries.iter().enumerate() {
                        let ext = e.path.rsplit_once('.').map_or("", |x| x.1);
                        if !matches!(e.kind, EntryKind::Resource { .. }) || !["ymap", "ytyp"].contains(&ext) {
                            continue;
                        }
                        let file = f.get_or_insert_with(|| File::open(&a.file).expect("open archive"));
                        let result = a
                            .resource(file, ei, &keys)
                            .and_then(|r| Meta::read(&r))
                            .and_then(|m| m.root_covered().map(|(_, u)| (m, u)));
                        let mut t = tally.lock().expect("lock");
                        if let Ok((m, unreached)) = &result {
                            for (b, &n) in m.blocks.iter().zip(unreached) {
                                if n > 0 {
                                    let e2 = t.unreached.entry((ext.to_string(), b.name)).or_default();
                                    e2.0 += 1;
                                    e2.1 += n as u64;
                                    if t.unreached_examples.len() < 10 {
                                        t.unreached_examples.push(format!(
                                            "{} block {:#x}: {n} of {}",
                                            e.path,
                                            b.name,
                                            b.data.len()
                                        ));
                                    }
                                }
                            }
                        }
                        *t.read.entry(ext.to_string()).or_default() += 1;
                        if let Err(err) = result {
                            let key = match err {
                                primase_gta::Error::Invalid { format, detail, .. } => {
                                    format!("{ext}: {format}: {detail}")
                                }
                                other => format!("{ext}: {other}"),
                            };
                            let v = t.failures.entry(key).or_default();
                            if v.len() < 3 {
                                v.push(e.path.clone());
                            }
                        }
                    }
                }
            });
        }
    });
    let t = tally.into_inner().expect("lock");
    println!("META census: {:?}", t.read);
    for ((ext, name), (blocks, bytes)) in &t.unreached {
        println!("  unreached: {ext} blocks of {name:#010x}: {blocks} blocks, {bytes} bytes");
    }
    for x in &t.unreached_examples {
        println!("    e.g. {x}");
    }
    for (msg, paths) in &t.failures {
        println!("  {msg}\n      e.g. {}", paths.join(", "));
    }
    assert!(t.failures.is_empty(), "{} failure kinds", t.failures.len());
    assert!(
        t.unreached.is_empty(),
        "data blocks with bytes no decoding reached"
    );
}
