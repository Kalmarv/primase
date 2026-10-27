//! The sampler value a material stores for a texture (`DiffuseSampler` and the like, `GtaParam::Sampler`) is not an
//! index into its effect's sampler records. The records' first entries are the same in every effect (point, then
//! pairs of clamping and wrapping filters), and a value of 0, 1 or 2 points at a clamping record; but the primitives
//! whose material stores such a value tile their texture: their texture coordinates span several units, which a
//! clamping sampler would smear into the edge texel. So `resolve-gta-shaders` leaves a material-chosen sampler
//! unstated.
//!
//! The measurement: over a region of the map (Vespucci Beach by default, the debug key `gta.region`), the primitives
//! with a `DiffuseSampler` value that is a record of their effect's table, by whether the record clamps on both axes
//! and whether the primitive's `TEXCOORD0` spans more than 1.5 units.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, debug, install, keys};
use primase_core::{Mesh, Semantic, Stream, World};
use primase_gta::components::{GtaEffectTechniques, GtaMaterialEffect, GtaParam, GtaShader};
use primase_gta::read::source::GtaSource;

#[test]
fn a_materials_sampler_value_is_not_an_index_of_the_effects_records() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let region: Vec<f32> = debug("gta.region")
        .unwrap_or_else(|| "-1500,-1650,-1150,-1330".into())
        .split(',')
        .map(|v| v.trim().parse().unwrap())
        .collect();
    let source = GtaSource::open(&dir, &keys).unwrap();
    let (read, _) = source
        .read([region[0], region[1], region[2], region[3]], 600.0)
        .unwrap();
    let (mut world, _): (World, _) = source.load_all(&read).unwrap();
    source.add_shader_database(&mut world).unwrap();
    let key = primase_foundations::hash::joaat(b"DiffuseSampler");
    // by the value: (primitives, of them tiling), over the primitives whose value's record clamps on both axes
    let mut clamping: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
    let mut beyond_table = 0usize;
    for (_, mesh) in world.iter::<Mesh>() {
        for q in mesh.primitives() {
            let Some(material) = q.material else { continue };
            let (Some(shader), Some(link)) = (
                world.get::<GtaShader>(material),
                world.get::<GtaMaterialEffect>(material),
            ) else {
                continue;
            };
            let Some(GtaParam::Sampler(v)) = shader.params.iter().find(|(k, _)| *k == key).map(|(_, p)| p)
            else {
                continue;
            };
            let Some(table) = world.get::<GtaEffectTechniques>(link.effect) else {
                continue;
            };
            let Some(record) = table.samplers.get(usize::from(*v)) else {
                beyond_table += 1;
                continue;
            };
            if record.address[0] != 3 || record.address[1] != 3 {
                continue;
            }
            let Some(Stream::F32x2(uv)) = q.streams.get(&Semantic::TexCoord(0)) else {
                continue;
            };
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for p in uv {
                for a in 0..2 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
            let e = clamping.entry(*v).or_default();
            e.0 += 1;
            e.1 += usize::from((0..2).any(|a| hi[a] - lo[a] > 1.5));
        }
    }
    println!("primitives whose DiffuseSampler value is past their effect's table: {beyond_table}");
    for (v, (n, tiling)) in &clamping {
        println!("value {v}, a clamping record: {n} primitives, {tiling} with a TEXCOORD0 span over 1.5");
    }
    let (n, tiling) = clamping.values().fold((0, 0), |(n, t), (a, b)| (n + a, t + b));
    assert!(n > 1_000, "{n}");
    // a fifth of them is far more than a clamping sampler leaves of tiled geometry (the measured share is a half)
    assert!(tiling * 5 > n, "{tiling} of {n} tile");
}
