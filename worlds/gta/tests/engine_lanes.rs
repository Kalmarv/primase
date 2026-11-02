//! The lanes of two engine constants every program of the shader database reads (vertex and pixel; the lifted program's
//! emitted WGSL says which lanes it takes), which `resolve-gta-shaders` gives roles on that evidence:
//!
//! - `globalScreenSize` (`misc_globals`): lanes x, y are read by 3,298 and 3,288 programs and z, w by 2,091 and 2,093. The smallest program
//!   reading z and w, `VS_SSAO`, computes `uv * 0.5 * (x, y)` (a pixel coordinate: noise lookups) and
//!   `(z * 2 + u, v)`, `(u, w * 2 + v)`: z and w are uv-space steps of whole texels. That they are `1 / x` and `1 / y` is
//!   the natural reading of that use; no program shown multiplies them together.
//! - `gDirectionalAmbientColour` (`lighting_globals`): lanes x, y, z are read by 2,834 programs, w by 2,372.
//!   `PS_lightTile_ambient`, the smallest pixel reader, adds `ambient.xyz * saturate(dot(direction, normal))` to the same
//!   channels of the lit colour (xyz are a colour); `VS_TransformD` scales the vertex colour with w
//!   (`(w - 1) * fade + 1`).
//!
//! The lane counts are asserted; the program-level statements are the doc's. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod common;
mod inputs;

use std::collections::BTreeMap;

use inputs::{Game, install, keys};
use primase_core::World;
use primase_gta::read::source::GtaSource;
use rage_engine_formats::sgd2_table::ProgramTable;

/// What a program takes of the two constants, one `(name, lane|"whole"|"dynamic buffer")` note per lane, row or buffer
/// (as its constant table places them), from one lift; `None` where it does not lift or emit WGSL.
fn notes(p: &common::Program, table: &ProgramTable) -> Option<Vec<(String, String)>> {
    let (lanes, whole, dynamic) = iridia_dxil::lift(&p.shader.bytecode)
        .map_err(|e| e.to_string())
        .and_then(|program| common::wgsl_reads(&program))
        .ok()?;
    let mut notes = Vec::new();
    for (i, r) in table.resources.iter().enumerate() {
        if r.constants == 0 {
            continue;
        }
        for c in table.constants_of(i) {
            let Some(name) = table.name_of(c.name) else {
                continue;
            };
            if !["globalScreenSize", "gDirectionalAmbientColour"].contains(&name) {
                continue;
            }
            let row = u32::from(c.offset) / 16;
            let (sp, rg) = (u32::from(r.space), u32::from(r.register));
            for lane in 0..4 {
                if lanes.contains(&(sp, rg, row, lane)) {
                    notes.push((name.to_owned(), format!("lane {lane}")));
                }
            }
            if whole.contains(&(sp, rg, row)) {
                notes.push((name.to_owned(), "row loaded, no lane taken".into()));
            }
            if dynamic.contains(&(sp, rg)) {
                notes.push((name.to_owned(), "buffer indexed dynamically".into()));
            }
        }
    }
    Some(notes)
}

#[test]
fn lanes() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let (mut programs, mut unlifted) = (0usize, 0usize);
    // (name, lane|"whole"|"dynamic buffer") -> programs
    let mut hist: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut examples: BTreeMap<(String, String), String> = BTreeMap::new();
    // the programs with a constant table
    let all: Vec<(common::Program, &ProgramTable)> = common::programs(&world)
        .into_iter()
        .filter_map(|p| p.table.map(|table| (p, table)))
        .collect();
    for ((p, _), notes) in all.iter().zip(common::each(&all, |(p, table)| notes(p, table))) {
        let Some(notes) = notes else {
            unlifted += 1;
            continue;
        };
        programs += 1;
        for key in notes {
            *hist.entry(key.clone()).or_default() += 1;
            examples.entry(key).or_insert_with(|| p.program.name.clone());
        }
    }
    println!("{programs} programs lifted, {unlifted} not");
    for ((n, w), c) in &hist {
        println!(
            "LANE {n}: {w}: {c} programs, e.g. {}",
            examples[&(n.clone(), w.clone())]
        );
    }
    let count = |name: &str, what: &str| {
        hist.get(&(name.to_owned(), what.to_owned()))
            .copied()
            .unwrap_or(0)
    };
    for lane in 0..2 {
        assert!(count("globalScreenSize", &format!("lane {lane}")) > 3_000);
    }
    for lane in 2..4 {
        assert!(count("globalScreenSize", &format!("lane {lane}")) > 2_000);
    }
    for lane in 0..3 {
        assert!(count("gDirectionalAmbientColour", &format!("lane {lane}")) > 2_500);
    }
    assert!(count("gDirectionalAmbientColour", "lane 3") > 2_000);
}
