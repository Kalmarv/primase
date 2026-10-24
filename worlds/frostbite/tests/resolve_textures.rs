//! The texture step on the game's data (the skate. install and its Oodle 9 library; unset skips): on every
//! twentieth bundle of `Win32/items.toc` that holds a MeshSet, the materials of its keys resolved as in
//! `resolve_shaders.rs`, then `Source::resolve_textures` against an index of one scope, then the pass. Two tests, one
//! for each scope the index can have: the home TOC (`items.toc`) alone, and every TOC of the install (the home TOC
//! first). Each prints the index (its build time and how much it holds; run one test at a time to measure the
//! process's peak memory), the texture references of the materials by where they led (linked, or the reason they are
//! `Missing`), the texture slots of the bindings by source, and for the textures bound whether the texture decoder the
//! lab samples through reads their base level. Every component of every world must verify.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use primase_compress::oodle::{Oodle, Version};
use primase_core::pass::{Pass, PassLog};
use primase_core::shader::{ShaderBinding, TextureSource};
use primase_core::{Entity, Texture, TextureLayers, TextureShape, World};
use primase_frostbite::MESHSET;
use primase_frostbite::bundle::Kind;
use primase_frostbite::components::{FrostbiteMaterial, FrostbiteTexture};
use primase_frostbite::folder::{Folder, Source};
use primase_frostbite::passes::ResolveFrostbiteShaders;
use primase_frostbite::program::Scope;
use primase_frostbite::textures::TextureIndex;
use primase_testdata::{Game, Tool, install_file, tool};
use primase_texcodec::{Decoder, TexDecoder};

/// The slot kind a texture of this shape is counted under, and its short name.
fn shape_name(shape: TextureShape) -> (&'static str, &'static str) {
    match shape {
        TextureShape::Array { .. } => ("texture, array", "array"),
        TextureShape::Cube { .. } => ("texture, cube", "cube"),
        TextureShape::Volume { .. } => ("texture, volume", "volume"),
    }
}

fn census(every_toc: bool) {
    let (Some(data), Some(dll)) = (install_file(Game::Skate, "Data"), tool(Tool::Oodle9)) else {
        return;
    };
    let folder = Folder::open(data).unwrap();
    // SAFETY: the configured Oodle 9 library is the game's own.
    #[allow(unsafe_code)]
    let oodle = unsafe { Oodle::load(dll, Version::V9) }.unwrap();
    let src = Source {
        folder: &folder,
        base: Some(&folder),
        oodle: Some(&oodle),
    };
    let items = src.listing("Win32/items.toc").unwrap();
    let globals = src.listing("Win32/globals.toc").unwrap();
    let root = src
        .listing("Win32/levels/game/dingolevel_root/dingolevel_root.toc")
        .unwrap();
    let tocs: Vec<&str> = if every_toc {
        std::iter::once("Win32/items.toc")
            .chain(
                folder
                    .tocs
                    .iter()
                    .map(String::as_str)
                    .filter(|t| *t != "Win32/items.toc"),
            )
            .collect()
    } else {
        vec!["Win32/items.toc"]
    };
    let started = Instant::now();
    let index = TextureIndex::build(&src, &tocs).unwrap();
    println!(
        "INDEX of {} TOC(s): {} bundles, {} texture assets by GUID, {} Texture resources by id, built in {:.1} s",
        tocs.len(),
        index.bundles(),
        index.assets(),
        index.textures(),
        started.elapsed().as_secs_f64()
    );
    let decoder = Decoder::default();
    let mut log = PassLog::default();
    let (mut references, mut linked, mut distinct) = (0usize, 0usize, 0usize);
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut slots: BTreeMap<String, usize> = BTreeMap::new();
    let mut decoded: BTreeMap<String, usize> = BTreeMap::new();
    let mut formats: BTreeMap<u32, usize> = BTreeMap::new();
    let (mut pixel_bytes, mut violations) = (0u64, Vec::new());
    let mut read_time = 0f64;
    for bundle in items
        .bundles
        .iter()
        .filter(|b| b.manifest.res.iter().any(|a| a.rtype == MESHSET))
        .step_by(20)
    {
        let mut scope_bundles = vec![bundle.clone()];
        scope_bundles.extend(globals.bundles.iter().cloned());
        scope_bundles.extend(root.bundles.iter().cloned());
        let scope = Scope::new(&scope_bundles);
        let all = src.materials(&scope).unwrap();
        let mut w = World::new();
        for entry in bundle
            .entries()
            .filter(|e| e.kind == Kind::Res && e.resource_type == Some(MESHSET))
        {
            let made = src.read_entry(bundle, &entry, &mut w).unwrap();
            src.resolve_materials(&scope, &all, made[0], &mut w).unwrap();
        }
        let t = Instant::now();
        let report = src.resolve_textures(&index, &mut w);
        read_time += t.elapsed().as_secs_f64();
        references += report.references;
        linked += report.linked;
        distinct += report.textures;
        for (why, n) in report.missing {
            *missing.entry(why).or_default() += n;
        }
        ResolveFrostbiteShaders.run(&mut w, &mut log).unwrap();
        for (component, _, why) in w.verify_components() {
            violations.push(format!("{}: {component}: {why}", bundle.name));
        }
        // the textures bound, each once per world: the decoder the lab samples through reads the base level or not
        let mut bound: BTreeSet<Entity> = BTreeSet::new();
        for (_, b) in w.iter::<ShaderBinding>() {
            for s in &b.stages {
                for t in &s.textures {
                    match &t.source {
                        TextureSource::Texture { texture, .. } => {
                            bound.insert(*texture);
                            let kind = w
                                .get::<TextureLayers>(*texture)
                                .map_or("texture, 2D map", |l| shape_name(l.shape).0);
                            *slots.entry(kind.to_owned()).or_default() += 1;
                        }
                        TextureSource::Missing => *slots.entry("missing".to_owned()).or_default() += 1,
                        _ => *slots.entry("another source".to_owned()).or_default() += 1,
                    }
                }
            }
        }
        for (_, m) in w.iter::<FrostbiteMaterial>() {
            assert!(m.textures.len() <= m.depot.as_ref().map_or(0, |d| d.parameters.len()));
        }
        for e in bound {
            let header = &w.get::<FrostbiteTexture>(e).unwrap().header;
            *formats.entry(header.format).or_default() += 1;
            if let Some(layers) = w.get::<TextureLayers>(e) {
                // the texture decoder reads the base level of every image
                pixel_bytes += layers
                    .levels
                    .iter()
                    .flatten()
                    .map(|l| l.len() as u64)
                    .sum::<u64>();
                let (w0, h0) = (layers.width, layers.height);
                let ok = layers.levels.first().is_some_and(|images| {
                    images.iter().all(|l| {
                        decoder
                            .decode_rgba8(layers.format, w0, h0, l)
                            .is_ok_and(|rgba| rgba.len() == w0 as usize * h0 as usize * 4)
                    })
                });
                *decoded
                    .entry(format!(
                        "{} {:?} base level of every image {}",
                        shape_name(layers.shape).1,
                        layers.format,
                        if ok { "decodes" } else { "FAILS" }
                    ))
                    .or_default() += 1;
                continue;
            }
            let tex = w.get::<Texture>(e).unwrap();
            pixel_bytes += tex.levels.iter().map(|l| l.len() as u64).sum::<u64>();
            let ok = tex.levels.first().is_some_and(|l| {
                decoder
                    .decode_rgba8(tex.format, tex.width, tex.height, l)
                    .is_ok_and(|rgba| rgba.len() == tex.width as usize * tex.height as usize * 4)
            });
            *decoded
                .entry(format!(
                    "{:?} base level {}",
                    tex.format,
                    if ok { "decodes" } else { "FAILS" }
                ))
                .or_default() += 1;
        }
    }
    println!(
        "references {references}, linked {linked}, distinct textures read {distinct}, step took {read_time:.1} s"
    );
    for (why, n) in &missing {
        println!("  {n:>6}  Missing: {why}");
    }
    println!("texture slots of the bindings by source: {slots:?}");
    println!("textures bound (once per world) by lab decoding: {decoded:?}");
    println!("their Frostbite formats: {formats:?}; {pixel_bytes} bytes of levels");
    for (k, n) in log.counts.iter().filter(|(k, _)| k.starts_with("texture slots")) {
        println!("  {n:>6}  {k}");
    }
    assert!(violations.is_empty(), "{violations:?}");
    assert_eq!(references, linked + missing.values().sum::<usize>());
    assert!(references > 0);
}

#[test]
fn textures_resolved_against_the_home_toc() {
    census(false);
}

#[test]
fn textures_resolved_against_every_toc() {
    census(true);
}
