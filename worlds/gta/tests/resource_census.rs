//! Census of drawables and texture dictionaries: every ydr and ytd of an install (every copy, in every archive) read
//! by `Drawable::read_resource` and `TextureDictionary::read_resource`, the drawables' embedded dictionaries
//! included. Resources in another version's layout (Legacy's ydr 165, older tools' 154, ytd 2 and 13) are counted
//! by version, not read. Failures are counted by message; resource versions, vertex formats per semantic, texture
//! formats and null dictionary entries are counted.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`; `PRIMASE_GTA_FILTER` limits the archives to paths containing that
//! text.
use std::collections::BTreeMap;
use std::fs::File;
use std::sync::Mutex;

use primase_gta::drawable::Drawable;
use primase_gta::texture::TextureDictionary;
use primase_gta::{EntryKind, Install, Keys};
use primase_testdata::real_data;

#[derive(Default)]
struct Tally {
    read: BTreeMap<String, u64>,
    versions: BTreeMap<(String, u32), u64>,
    vertex_formats: BTreeMap<(&'static str, u8), u64>,
    texture_formats: BTreeMap<u8, u64>,
    geometries: u64,
    textures: u64,
    null_textures: u64,
    /// Effect and texture parameter name hashes no name in `shader_names` gives.
    unnamed: BTreeMap<String, u64>,
    other_layout: BTreeMap<(String, u32), u64>,
    failures: BTreeMap<String, (u64, Vec<String>)>,
}

#[test]
fn every_drawable_and_texture_dictionary_reads() {
    let (Some(dir), Some(key_dir)) = (real_data("PRIMASE_GTA_DIR"), real_data("PRIMASE_GTA_KEYS")) else {
        return;
    };
    let filter = std::env::var("PRIMASE_GTA_FILTER").unwrap_or_default();
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
                    if !a.path.contains(&filter) {
                        continue;
                    }
                    let mut f: Option<File> = None;
                    for (ei, e) in a.entries.iter().enumerate() {
                        let ext = e.path.rsplit_once('.').map_or("", |x| x.1);
                        if !matches!(e.kind, EntryKind::Resource { .. }) || !["ydr", "ytd"].contains(&ext) {
                            continue;
                        }
                        let file = f.get_or_insert_with(|| File::open(&a.file).expect("open archive"));
                        let res = a.resource(file, ei, &keys);
                        let mut local = Tally::default();
                        let mut other_layout = None;
                        let result = res.and_then(|r| {
                            *local.versions.entry((ext.to_string(), r.version)).or_default() += 1;
                            // resources in another version's layout (Legacy's, older tools') are counted, not read
                            let want = if ext == "ydr" {
                                primase_gta::drawable::VERSION
                            } else {
                                primase_gta::texture::VERSION
                            };
                            if r.version != want {
                                other_layout = Some(r.version);
                                return Ok(());
                            }
                            let dict = if ext == "ydr" {
                                let d = Drawable::read_resource(&r)?;
                                for sh in &d.shaders {
                                    if primase_gta::shader_names::effect(sh.name).is_none() {
                                        *local
                                            .unnamed
                                            .entry(format!("effect {:#010x}", sh.name))
                                            .or_default() += 1;
                                    }
                                    for p in &sh.params {
                                        if p.info & 3 == 0
                                            && primase_gta::shader_names::texture_param(p.name).is_none()
                                        {
                                            *local
                                                .unnamed
                                                .entry(format!("texture parameter {:#010x}", p.name))
                                                .or_default() += 1;
                                        }
                                    }
                                }
                                for g in d.lods.iter().flatten().flat_map(|m| &m.geometries) {
                                    local.geometries += 1;
                                    for e in &g.elements {
                                        *local
                                            .vertex_formats
                                            .entry((e.semantic(), e.format.0))
                                            .or_default() += 1;
                                    }
                                }
                                d.textures
                            } else {
                                Some(TextureDictionary::read_resource(&r)?)
                            };
                            for t in dict.iter().flat_map(|d| &d.textures) {
                                match t {
                                    Some(t) => {
                                        local.textures += 1;
                                        *local.texture_formats.entry(t.format.0).or_default() += 1;
                                    }
                                    None => local.null_textures += 1,
                                }
                            }
                            Ok(())
                        });
                        let mut t = tally.lock().expect("lock");
                        *t.read.entry(ext.to_string()).or_default() += 1;
                        t.geometries += local.geometries;
                        t.textures += local.textures;
                        t.null_textures += local.null_textures;
                        for (k, v) in local.unnamed {
                            *t.unnamed.entry(k).or_default() += v;
                        }
                        if let Some(v) = other_layout {
                            *t.other_layout.entry((ext.to_string(), v)).or_default() += 1;
                        }
                        for (k, v) in local.versions {
                            *t.versions.entry(k).or_default() += v;
                        }
                        for (k, v) in local.vertex_formats {
                            *t.vertex_formats.entry(k).or_default() += v;
                        }
                        for (k, v) in local.texture_formats {
                            *t.texture_formats.entry(k).or_default() += v;
                        }
                        if let Err(err) = result {
                            let key = match err {
                                primase_gta::Error::Invalid { format, detail, .. } => {
                                    format!("{ext}: {format}: {detail}")
                                }
                                other => format!("{ext}: {other}"),
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
    println!(
        "resource census: {:?}, {} geometries, {} textures ({} null dictionary entries); versions {:?}",
        t.read, t.geometries, t.textures, t.null_textures, t.versions
    );
    println!("not read, another version's layout: {:?}", t.other_layout);
    println!("unnamed shader names: {:?}", t.unnamed);
    println!("vertex formats (semantic, code): {:?}", t.vertex_formats);
    println!("texture formats (code): {:?}", t.texture_formats);
    for (msg, (n, paths)) in &t.failures {
        println!("  {n}x {msg}\n      e.g. {}", paths.join(", "));
    }
    assert!(t.failures.is_empty(), "{} failure kinds", t.failures.len());
    assert!(t.unnamed.is_empty(), "shader names without a name");
}
