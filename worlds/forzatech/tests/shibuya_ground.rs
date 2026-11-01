//! The ground families' rules over the whole Shibuya Scramble box of the Tokyo track (x -1550..-1100, z -5000..-4566.67,
//! summer; every prototype, as `families.rs` runs with `STEP=1`), from the install the `GAME_DATA` configuration names:
//! how many materials and triangles `environment_plot`, `terr_mega`, `terr_distant` and `uber_road` lower, what they
//! leave out by name, and where the planar coordinates put the terrain on its textures. Without the install it skips.
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
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, LayeredMaterial, Mesh, Semantic, Stream};
use primase_forzatech::components::ForzaMaterial;
use primase_forzatech::passes::{LowerForzaMaterials, LowerForzaPlanarUv};
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use std::collections::BTreeMap;

fn scope() -> Scope {
    Scope {
        area: Area::new(-1550.0, -5000.0, -1100.0, -13_700.0 / 3.0),
        season: Season::Summer,
    }
}

const GROUND: [&str; 4] = ["environment_plot", "terr_mega", "terr_distant", "uber_road"];

#[test]
fn shibuya_ground() {
    let Some(folder) = install_file(Game::Fh6, "media/Tracks/Brio") else {
        return;
    };
    let track = Track::open(folder).unwrap();
    let mut reader = ForzaReader::new(track, scope());
    let (world, _) = reader.read().unwrap();
    let (mut part, _) = reader.load_all(world).unwrap();

    let mut triangles: BTreeMap<Entity, u64> = BTreeMap::new();
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            if let Some(m) = p.material {
                *triangles.entry(m).or_default() += p.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
            }
        }
    }
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut part, &mut log).unwrap();

    let mut lowered: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut unlowered: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut layered = (0u64, 0u64);
    for (e, m) in part.iter::<ForzaMaterial>() {
        let family = m
            .shader
            .as_ref()
            .map_or("no shader family", |s| s.family.as_str());
        if !GROUND.contains(&family) {
            continue;
        }
        let t = triangles.get(&e).copied().unwrap_or(0);
        let map = if part.has::<PbrMaterial>(e) {
            &mut lowered
        } else {
            &mut unlowered
        };
        let entry = map.entry(family.to_owned()).or_default();
        entry.0 += 1;
        entry.1 += t;
        if part.has::<LayeredMaterial>(e) {
            layered.0 += 1;
            layered.1 += t;
        }
    }
    println!("lowered (materials, triangles): {lowered:?}");
    println!("unlowered (materials, triangles): {unlowered:?}");
    println!("layered roads (materials, triangles): {layered:?}");
    for (what, n) in &log.approximations {
        println!("approximation {what}: {n}");
    }
    for (what, n) in &log.counts {
        println!("count {what}: {n}");
    }
    assert!(unlowered.is_empty(), "{unlowered:?}");
    for (e, pbr) in part.iter::<PbrMaterial>() {
        pbr.verify(e, &part).unwrap();
    }
    for (e, l) in part.iter::<LayeredMaterial>() {
        l.verify(e, &part).unwrap();
    }

    // where the planar coordinates put the terrain on its textures: the family's own scale and offset, over each vertex
    let mut slots: BTreeMap<Entity, ([f32; 2], [f32; 2])> = BTreeMap::new();
    for (e, pbr) in part.iter::<PbrMaterial>() {
        let family = part
            .get::<ForzaMaterial>(e)
            .and_then(|m| m.shader.as_ref())
            .map(|s| s.family.as_str());
        if matches!(family, Some("terr_mega" | "terr_distant"))
            && let Some(t) = pbr.base_color_texture.and_then(|b| b.transform)
        {
            slots.insert(e, (t.scale, t.offset));
        }
    }
    let mut pass_log = PassLog::default();
    LowerForzaPlanarUv.run(&mut part, &mut pass_log).unwrap();
    println!("planar pass counts: {:?}", pass_log.counts);
    let (mut inside, mut total) = (0u64, 0u64);
    for (_, mesh) in part.iter::<Mesh>() {
        for p in mesh.primitives() {
            let Some(m) = p.material else { continue };
            let Some((scale, offset)) = slots.get(&m) else {
                continue;
            };
            let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(3)) else {
                continue;
            };
            for q in uv {
                let (u, v) = (offset[0] + scale[0] * q[0], offset[1] + scale[1] * q[1]);
                total += 1;
                if (-0.01..=1.01).contains(&u) && (-0.01..=1.01).contains(&v) {
                    inside += 1;
                }
            }
        }
    }
    println!("terrain vertices on their texture: {inside} of {total}");
    assert!(total > 0, "no terrain vertex was given planar coordinates");
    assert!(inside * 100 >= total * 99, "{inside} of {total}");
}
