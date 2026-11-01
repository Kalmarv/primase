//! Census of the Shibuya Scramble box's materials by shader family: for each family, the techniques and switch values
//! its materials resolve to and the triangles they shade, then the texture parameters each binds. The evidence the
//! lowering pass (`passes/materials.rs`) states its rules over. Prints; asserts only that the box reads.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "a test reading the files you point it at"
)]

mod inputs;

use forzatech_formats::world::{Area, Season};
use inputs::{Game, install_file};
use primase_core::convert::Reader;
use primase_core::{Entity, Instance, Mesh, Texture, World};
use primase_forzatech::components::{ForzaMaterial, MaterialTextures, Value};
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use std::collections::{BTreeMap, BTreeSet};

fn scope() -> Scope {
    Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
    }
}

/// How a parameter shows in a signature: switches (ints and bools) with their values, everything else by name.
fn switch(p: &primase_forzatech::components::Parameter) -> Option<String> {
    let name = p.name.clone().unwrap_or_else(|| format!("{:#010x}", p.hash));
    match &p.value {
        Value::Int(i) => Some(format!("{name}={i}")),
        Value::Bool(b) => Some(format!("{name}={}", u8::from(*b != 0))),
        _ => None,
    }
}

#[test]
fn shibuya_families() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let family = std::env::var("FAMILY").unwrap_or_else(|_| "environment".to_owned());
    let track = Track::open(folder).unwrap();
    let mut reader = ForzaReader::new(track, scope());
    let (world, _) = reader.read().unwrap();
    let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    let sample: BTreeSet<Entity> = prototypes
        .iter()
        .copied()
        .step_by(
            std::env::var("STEP")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(20),
        )
        .collect();
    let (part, _) = reader.load(&world, &sample).unwrap();
    for family in family.split(',') {
        census(&part, family);
    }
}

fn census(part: &World, family: &str) {
    let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(m) = p.material {
                *triangles.entry(m).or_default() += p.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
            }
        }
    }
    // by technique + third string + the switches that differ between materials
    let mut by_sig: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut switch_values: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut bindings: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut total = 0u64;
    for (e, m) in part.iter::<ForzaMaterial>() {
        let Some(s) = &m.shader else { continue };
        if s.family != family {
            continue;
        }
        let t = triangles.get(&e).copied().unwrap_or(0);
        total += t;
        let mat = m.material.as_ref().unwrap();
        let mut sig = format!(
            "{} / {:?} / atst={:?} blst={:?} /",
            mat.technique, mat.third, mat.atst, mat.blst
        );
        for (_, p) in m.resolved() {
            if let Some(s) = switch(p) {
                *switch_values
                    .entry(s.split('=').next().unwrap().to_owned())
                    .or_default()
                    .entry(s.clone())
                    .or_default() += t;
                sig.push(' ');
                sig.push_str(&s);
            }
        }
        let entry = by_sig.entry(sig).or_default();
        entry.0 += 1;
        entry.1 += t;
        let bound = part.get::<MaterialTextures>(e);
        for (_, p) in m.resolved() {
            if let Value::Texture { path, path_hash } = &p.value {
                let name = p.name.clone().unwrap_or_else(|| format!("{:#010x}", p.hash));
                let b = bindings.entry(name).or_default();
                if *path_hash == 0 {
                    b.0 += t; // no texture bound
                } else if bound
                    .and_then(|b| b.0.iter().find(|x| x.path_hash == *path_hash))
                    .is_some_and(|x| x.texture.is_some())
                {
                    b.1 += t;
                } else {
                    b.2 += t; // a hash in no archive
                }
                let _ = path;
            }
        }
    }
    println!("== family {family}: {total} triangles");
    if let Some((_, m)) = part
        .iter::<ForzaMaterial>()
        .find(|(_, m)| m.shader.as_ref().is_some_and(|s| s.family == family))
        && let Some(s) = &m.shader
    {
        let name = |hash: u32| {
            s.declared
                .iter()
                .find(|d| d.hash == hash)
                .map_or_else(|| format!("{hash:#010x}"), |d| d.name.clone())
        };
        println!("-- shader tables: constants {} bytes", s.constants_size);
        for (what, map) in [
            ("CBMP", &s.constants),
            ("TXMP", &s.textures),
            ("SPMP", &s.samplers),
        ] {
            for e in map {
                println!("  {what} {} = {}", name(e.hash), e.value);
            }
        }
    }
    println!("-- switches (triangles by value)");
    for (name, values) in &switch_values {
        let v: Vec<String> = values.iter().map(|(k, t)| format!("{k}: {t}")).collect();
        println!("  {name}: {}", v.join(", "));
    }
    println!("-- signatures (materials, triangles)");
    let mut sigs: Vec<_> = by_sig.into_iter().collect();
    sigs.sort_by_key(|(_, (_, t))| std::cmp::Reverse(*t));
    for (sig, (n, t)) in sigs.iter().take(60) {
        println!("  {n:>4} {t:>8}  {sig}");
    }
    println!("-- formats of the textures bound to each parameter (triangles by format, colour space)");
    let mut formats: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for (e, m) in part.iter::<ForzaMaterial>() {
        let Some(s) = &m.shader else { continue };
        if s.family != family {
            continue;
        }
        let t = triangles.get(&e).copied().unwrap_or(0);
        for (_, p) in m.resolved() {
            let Value::Texture { path_hash, .. } = &p.value else {
                continue;
            };
            if *path_hash == 0 {
                continue;
            }
            let tex = part
                .get::<MaterialTextures>(e)
                .and_then(|b| b.0.iter().find(|x| x.path_hash == *path_hash))
                .and_then(|x| x.texture)
                .and_then(|x| part.get::<Texture>(x));
            let key = tex.map_or_else(
                || "absent".to_owned(),
                |x| format!("{:?} {:?}", x.format, x.color_space),
            );
            let name = p.name.clone().unwrap_or_else(|| format!("{:#010x}", p.hash));
            *formats.entry(name).or_default().entry(key).or_default() += t;
        }
    }
    for (name, by_format) in &formats {
        println!("  {name}: {by_format:?}");
    }
    println!("-- texture parameters (triangles: unbound-hash0, bound entity, hash in no archive)");
    for (name, (z, b, m)) in &bindings {
        println!("  {name}: {z}, {b}, {m}");
    }
}
