//! Which of the engine's matrices (`g_rage_matrices`: gWorld, gWorldView, gWorldViewProj, gViewInverse, the
//! `gRelative*` and `gPrev*` forms) the outputs of the vertex programs are computed from, over every distinct vertex
//! program of the shader database that has the buffer: for each output (its label), the set of matrices whose rows its
//! stores read, and the programs it is so for. This is the evidence for the roles `resolve-gta-shaders` gives the
//! matrices: the clip position is computed from exactly one of the world-view-projection forms, the previous-frame
//! interpolant from a previous-frame one, and the camera-relative forms are what the position and the previous
//! position are computed from instead of the plain ones.
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use iridia_interface::SystemValue;
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable, GtaStage};
use primase_gta::passes::gbuffer;
use primase_gta::read::source::GtaSource;
use primase_testdata::{Game, install, keys};

#[test]
fn the_matrices_the_vertex_outputs_are_computed_from() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    // (output, matrices read) -> programs
    let mut census: BTreeMap<(String, Vec<String>), usize> = BTreeMap::new();
    let mut programs = 0usize;
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in effect.programs.iter().filter(|p| p.stage == GtaStage::Vertex) {
            let Some(entity) = p.shader else { continue };
            if !seen.insert(entity) {
                continue;
            }
            let (Some(sh), Some(table)) = (
                world.get::<Shader>(entity),
                world.get::<GtaShaderTable>(entity).map(|t| &t.0),
            ) else {
                continue;
            };
            let Some(buffer) = table
                .resources
                .iter()
                .position(|r| table.name_of(r.name) == Some("g_rage_matrices") && r.constants > 0)
            else {
                continue;
            };
            let record = &table.resources[buffer];
            let binding = u32::from(record.space) << 16 | u32::from(record.register);
            let matrices: Vec<(String, BTreeSet<u32>)> = table
                .constants_of(buffer)
                .iter()
                .filter_map(|c| {
                    let first = u32::from(c.offset) / 16;
                    Some((table.name_of(c.name)?.to_owned(), (first..first + 4).collect()))
                })
                .collect();
            programs += 1;
            for v in &sh.interface.outputs {
                let Ok(rows) = gbuffer::constant_rows(&sh.bytecode, &v.label) else {
                    continue;
                };
                let read = rows.get(&binding).cloned().unwrap_or_default();
                if read.is_empty() {
                    continue;
                }
                let mut names: Vec<String> = matrices
                    .iter()
                    .filter(|(_, rows)| rows.iter().any(|r| read.contains(r)))
                    .map(|(n, _)| n.clone())
                    .collect();
                names.sort();
                let label = if v.system == Some(SystemValue::ClipPosition) {
                    "clip position".to_owned()
                } else {
                    format!("{}{}", v.label.name, v.label.index)
                };
                *census.entry((label, names)).or_default() += 1;
            }
        }
    }
    println!("{programs} vertex programs with g_rage_matrices");
    for ((label, names), n) in &census {
        println!("MATRICES {label} <- {names:?}: {n}");
    }
    let count = |label: &str, names: &[&str]| -> usize {
        census
            .iter()
            .filter(|((l, n), _)| l == label && n.iter().map(String::as_str).eq(names.iter().copied()))
            .map(|(_, n)| n)
            .sum()
    };
    // the clip position is the world-view-projection or its camera-relative form
    assert!(count("clip position", &["gWorldViewProj"]) > 1_000);
    assert!(count("clip position", &["gRelativeWorldViewProj"]) > 500);
    // the previous-frame interpolant is the previous-frame form of the same product (plain or camera-relative)
    assert!(count("TEXCOORD10", &["gPrevWorldViewProj"]) > 30);
    assert!(count("TEXCOORD10", &["gRelativePrevWorldViewProj"]) > 200);
    // no vertex program computes its clip position or the previous-frame interpolant from the other family's form
    assert_eq!(count("clip position", &["gPrevWorldViewProj"]), 0);
    assert_eq!(count("clip position", &["gRelativePrevWorldViewProj"]), 0);
    assert_eq!(count("TEXCOORD10", &["gWorldViewProj"]), 0);
    assert_eq!(count("TEXCOORD10", &["gRelativeWorldViewProj"]), 0);
}
