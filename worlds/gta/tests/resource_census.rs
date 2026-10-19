//! Census of drawables and texture dictionaries: every ydr, yft, ydd and ytd of an install (every copy, in every
//! archive) read by `Drawable::read_resource`, `Fragment::read_resource`, `DrawableDictionary::read_resource` and
//! `TextureDictionary::read_resource`, the drawables' embedded dictionaries included. Resources in another version's
//! layout (Legacy's ydr 165, older tools' 154, ytd 2 and 13) are counted by version, not read. Failures are counted by
//! message; resource versions, vertex formats per semantic, texture formats, null dictionary entries and shaders by
//! preset word are counted, and every effect name and texture and constant buffer parameter name must resolve
//! (`shader_names`).
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`; `PRIMASE_GTA_FILTER` limits the archives to paths containing that
//! text.
use std::collections::BTreeMap;
use std::fs::File;
use std::sync::Mutex;

use primase_gta::rage::drawable::{Drawable, DrawableDictionary, Fragment};
use primase_gta::rage::texture::TextureDictionary;
use primase_gta::read::archive::archive_set::Install;
use primase_gta::read::archive::keys::Keys;
use primase_gta::read::archive::rpf::EntryKind;
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
    /// Shaders by their preset word.
    presets: BTreeMap<u32, u64>,
    /// Light records by kind, and the checks on them (non-finite values, reserved words not zero).
    lights: BTreeMap<String, u64>,
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
                        if !matches!(e.kind, EntryKind::Resource { .. })
                            || !["ydr", "yft", "ydd", "ytd"].contains(&ext)
                        {
                            continue;
                        }
                        let file = f.get_or_insert_with(|| File::open(&a.file).expect("open archive"));
                        let res = a.resource(file, ei, &keys);
                        let mut local = Tally::default();
                        let mut other_layout = None;
                        let result = res.and_then(|r| {
                            *local.versions.entry((ext.to_string(), r.version)).or_default() += 1;
                            // resources in another version's layout (Legacy's, older tools') are counted, not read
                            let want = match ext {
                                "ydr" | "ydd" => primase_gta::rage::drawable::VERSION,
                                "yft" => primase_gta::rage::drawable::FRAGMENT_VERSION,
                                _ => primase_gta::rage::texture::VERSION,
                            };
                            if r.version != want {
                                other_layout = Some(r.version);
                                return Ok(());
                            }
                            let mut lights = Vec::new();
                            let drawables: Vec<Drawable> = match ext {
                                "ydr" => vec![Drawable::read_resource(&r)?],
                                "yft" => {
                                    let f = Fragment::read_resource(&r)?;
                                    lights.extend(f.lights);
                                    f.drawable
                                        .into_iter()
                                        .chain(f.extra.into_iter().map(|(_, d)| d))
                                        .collect()
                                }
                                "ydd" => DrawableDictionary::read_resource(&r)?.drawables,
                                _ => Vec::new(),
                            };
                            lights.extend(drawables.iter().flat_map(|d| d.lights.iter().cloned()));
                            for l in &lights {
                                let kind = match l.kind {
                                    1 => "point".to_string(),
                                    2 => "spot".to_string(),
                                    4 => "capsule".to_string(),
                                    k => format!("kind {k}"),
                                };
                                *local.lights.entry(format!("{ext}: {kind}")).or_default() += 1;
                                let floats = l
                                    .position
                                    .iter()
                                    .chain(&l.direction)
                                    .chain(&l.tangent)
                                    .chain(&l.extent);
                                if !floats.chain([&l.intensity, &l.falloff]).all(|v| v.is_finite()) {
                                    *local.lights.entry("not finite".into()).or_default() += 1;
                                }
                                if l.reserved.iter().any(|&w| w != 0) {
                                    *local
                                        .lights
                                        .entry(format!("reserved words {:x?}", l.reserved))
                                        .or_default() += 1;
                                }
                            }
                            let mut dicts: Vec<TextureDictionary> = Vec::new();
                            for d in drawables {
                                for sh in &d.shaders {
                                    *local.presets.entry(sh.preset).or_default() += 1;
                                    if primase_gta::passes::shader_names::effect(sh.name).is_none() {
                                        *local
                                            .unnamed
                                            .entry(format!("effect {:#010x}", sh.name))
                                            .or_default() += 1;
                                    }
                                    for p in &sh.params {
                                        let named = match p.info & 3 {
                                            0 => primase_gta::passes::shader_names::texture_param(p.name)
                                                .is_some(),
                                            3 => primase_gta::passes::shader_names::constant_param(p.name)
                                                .is_some(),
                                            _ => true,
                                        };
                                        if !named {
                                            *local
                                                .unnamed
                                                .entry(format!(
                                                    "parameter kind {} {:#010x}",
                                                    p.info & 3,
                                                    p.name
                                                ))
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
                                dicts.extend(d.textures);
                            }
                            if ext == "ytd" {
                                dicts.push(TextureDictionary::read_resource(&r)?);
                            }
                            for t in dicts.iter().flat_map(|d| &d.textures) {
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
                        for (k, v) in local.presets {
                            *t.presets.entry(k).or_default() += v;
                        }
                        for (k, v) in local.lights {
                            *t.lights.entry(k).or_default() += v;
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
    println!("shaders by preset word: {:x?}", t.presets);
    println!("light records: {:?}", t.lights);
    println!("vertex formats (semantic, code): {:?}", t.vertex_formats);
    println!("texture formats (code): {:?}", t.texture_formats);
    for (msg, (n, paths)) in &t.failures {
        println!("  {n}x {msg}\n      e.g. {}", paths.join(", "));
    }
    assert!(t.failures.is_empty(), "{} failure kinds", t.failures.len());
    assert!(t.unnamed.is_empty(), "shader names without a name");
    assert!(
        t.lights
            .keys()
            .all(|k| k.ends_with(": point") || k.ends_with(": spot") || k.ends_with(": capsule")),
        "light records outside the known kinds, not finite, or with reserved words set"
    );
}
