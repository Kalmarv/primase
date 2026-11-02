//! Reads the Shibuya Scramble box of the Tokyo track (x -1550..-1100, z -5000..-4566.67, summer) from the install the
//! `GAME_DATA` configuration names, and loads the meshes of a sample of its prototypes. Checks that the box has
//! placements of both kinds, that every placement's prototype is alive, and how the winding the reader gives the
//! loaded triangles agrees with their stored normals: a front face of the world's frame turns counter-clockwise, so
//! its geometric normal and its vertices' normals point the same way; and that every loaded material's chain reaches
//! its parent and shader family through the track's `bin.zip`, with the families by the triangles they shade. Without
//! the install it skips with a line naming its key.
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
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry};
use primase_core::{Entity, Instance, Lods, Mesh, Semantic, Stream, Texture, World};
use primase_forzatech::components::{
    ForzaMaterial, LodProfiles, MaterialTextures, ModelFiles, Placement, PlacementOrigin,
};
use primase_forzatech::passes::SelectForzaDrawn;
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use primase_forzatech::read::textures::TextureScope;
use std::collections::{BTreeMap, BTreeSet};

fn scope() -> Scope {
    Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
        // every stored tier: the textures as the game stores them
        textures: TextureScope::All,
        terrain_textures: TextureScope::All,
    }
}

#[test]
fn shibuya_scramble() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let t0 = std::time::Instant::now();
    let track = Track::open(folder).unwrap();
    println!("opened in {:.1} s: {track:?}", t0.elapsed().as_secs_f64());
    let mut reader = ForzaReader::new(track, scope());
    let t1 = std::time::Instant::now();
    let (world, lines) = reader.read().unwrap();
    println!("read in {:.1} s", t1.elapsed().as_secs_f64());
    // what convert checks before anything uses the read world, and each loaded part
    let mut registry = PassRegistry::new();
    primase_forzatech::register(&mut registry);
    let manager = PassManager::new(registry);
    manager.verify(&world, "read").unwrap();
    for (k, v) in &lines {
        println!("{k}: {v}");
    }
    for (what, map) in [
        ("no model", &reader.placements.no_model),
        ("no template", &reader.placements.no_template),
        ("two templates", &reader.placements.ambiguous_template),
    ] {
        for (name, n) in map {
            println!("{what}: {name} x{n}");
        }
    }
    let (mut rule, mut hand) = (0, 0);
    for (_, p) in world.iter::<Placement>() {
        match p.origin {
            PlacementOrigin::Rule { .. } => rule += 1,
            PlacementOrigin::Hand { .. } => hand += 1,
        }
    }
    assert!(rule > 0 && hand > 0, "{rule} rule, {hand} hand");
    let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    assert!(prototypes.iter().all(|p| world.is_alive(*p)));
    println!(
        "{rule} rule-placed, {hand} hand-placed, {} prototypes",
        prototypes.len()
    );

    // every 20th prototype's forms, loaded
    drawn_by_tier(&world);

    let sample: BTreeSet<Entity> = prototypes.iter().copied().step_by(20).collect();
    let t2 = std::time::Instant::now();
    let (part, lines) = reader.load(&world, &sample).unwrap();
    manager.verify(&part, "load").unwrap();
    // the conversion installs the part's scope over every entity it asked for: load keeps each alive
    let asked = world.closure(sample.iter().copied());
    let lost: Vec<Entity> = asked.iter().copied().filter(|e| !part.is_alive(*e)).collect();
    assert!(
        lost.is_empty(),
        "load despawned {} entities it was asked for",
        lost.len()
    );
    println!(
        "loaded {} prototypes in {:.1} s",
        sample.len(),
        t2.elapsed().as_secs_f64()
    );
    for (k, v) in &lines {
        println!("{k}: {v}");
    }
    let (agree, disagree, flat) = winding(&part);
    let all = (agree + disagree) as f64;
    println!(
        "winding against normals: {agree} agree, {disagree} disagree ({:.3}% disagree), {flat} without a direction",
        100.0 * disagree as f64 / all
    );
    let with_files = part
        .iter::<ModelFiles>()
        .filter(|(_, f)| !f.files.is_empty())
        .count();
    let meshes = part.iter::<Mesh>().count();
    let forms: usize = part.iter::<Lods>().map(|(_, l)| l.forms.len()).sum();
    println!("{with_files} entities with files, {meshes} with a mesh, {forms} forms");
    assert!(meshes > 0);
    assert!(agree > disagree * 10, "{agree} agree, {disagree} disagree");
    materials(&part, &reader);
}

/// The LOD profiles' (pass, tier) records, and what `select-forzatech-drawn` keeps at 5 m at each tier.
fn drawn_by_tier(world: &World) {
    let mut kinds: BTreeMap<(u16, u16), usize> = BTreeMap::new();
    if let Some(p) = world.resource::<LodProfiles>() {
        for r in p.0.iter().flatten() {
            *kinds.entry((r.pass, r.tier)).or_default() += 1;
        }
    }
    println!("LOD profile records by (pass, tier): {kinds:?}");
    for tier in 0..5 {
        let mut w = world.subset(&world.entities().collect());
        let mut log = PassLog::default();
        SelectForzaDrawn::new(5.0, tier)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        println!(
            "select-forzatech-drawn at 5 m, tier {tier}: dropped {:?}",
            log.drops
        );
    }
}

/// Every loaded material's chain reaches a parent and a shader family, every resolved texture parameter's hash a
/// texture entity with a `core.texture`; the families by the loaded triangles they shade.
fn materials(part: &World, reader: &ForzaReader) {
    let r = &reader.materials;
    for (path, n) in r
        .parent_unlisted
        .iter()
        .chain(&r.parent_missing)
        .chain(&r.shader_missing)
    {
        println!("unresolved: {path} x{n}");
    }
    for (hash, (path, n)) in &r.texture_missing {
        println!("texture in no archive: {hash:#010x} {path:?} x{n}");
    }
    let mut by_family: BTreeMap<&str, u64> = BTreeMap::new();
    let (mut shaded, mut unshaded) = (0u64, 0u64);
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            let triangles = p.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
            let family = p
                .material
                .and_then(|m| part.get::<ForzaMaterial>(m))
                .and_then(|m| m.shader.as_ref())
                .map(|s| s.family.as_str());
            match family {
                Some(f) => {
                    *by_family.entry(f).or_default() += triangles;
                    shaded += triangles;
                }
                None => unshaded += triangles,
            }
        }
    }
    let mut families: Vec<_> = by_family.into_iter().collect();
    families.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    for (f, t) in &families {
        println!(
            "family {f}: {t} triangles ({:.1}%)",
            100.0 * *t as f64 / shaded as f64
        );
    }
    let (mut bound, mut unbound) = (0, 0);
    for (_, t) in part.iter::<MaterialTextures>() {
        for x in &t.0 {
            match x.texture {
                Some(e) => {
                    assert!(part.get::<Texture>(e).is_some());
                    bound += 1;
                }
                None => unbound += 1,
            }
        }
    }
    println!(
        "{} materials over {shaded} shaded triangles ({unshaded} with no instance); {bound} texture bindings to an \
         entity, {unbound} to a hash in no archive; {} texture entities",
        part.iter::<ForzaMaterial>().count(),
        part.iter::<Texture>().count()
    );
    assert!(r.instances > 0);
    assert!(r.parent_unlisted.is_empty() && r.parent_missing.is_empty() && r.shader_missing.is_empty());
}

/// Per triangle, whether `(b - a) × (c - a)` points the way its vertices' summed normal does.
fn winding(world: &World) -> (u64, u64, u64) {
    let (mut agree, mut disagree, mut flat) = (0, 0, 0);
    for (_, mesh) in world.iter::<Mesh>() {
        for p in mesh.primitives() {
            let Some(Stream::F32x3(n)) = p.streams.get(&Semantic::Normal) else {
                continue;
            };
            let pos = p.positions().unwrap();
            for t in p.indices.as_ref().unwrap().as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| pos[i as usize].map(f64::from));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let g = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let s = t.map(|i| n[i as usize].map(f64::from));
                let sum = [
                    s[0][0] + s[1][0] + s[2][0],
                    s[0][1] + s[1][1] + s[2][1],
                    s[0][2] + s[1][2] + s[2][2],
                ];
                let d = g[0] * sum[0] + g[1] * sum[1] + g[2] * sum[2];
                if d > 0.0 {
                    agree += 1;
                } else if d < 0.0 {
                    disagree += 1;
                } else {
                    flat += 1;
                }
            }
        }
    }
    (agree, disagree, flat)
}
