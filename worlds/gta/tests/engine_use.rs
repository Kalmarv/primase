//! The engine constants `resolve-gta-shaders` gives roles or reasons (`engine_role`, `engine_reason`), held to the vertex
//! programs that read them: over every distinct vertex program of the shader database, the lanes of each constant a
//! program loads (the lifted program's constant-buffer loads, by row and lane).
//!
//! - `globalAnimUV0` / `globalAnimUV1` (rows 0 and 1 of `common_locals`) are read at lanes x, y, z only: the rows of an
//!   affine transform applied to `(u, v, 1)` (`VS_TransformUnlit`: `o.x = dot(row0.xyz, (u, v, 1))`).
//! - `tintPaletteSelector` is read at lanes x and y; x is the v the palette texture is sampled at (u is a vertex colour).
//! - `gClothVertexStride` is read at lane z only and `gUIHDRParams` at x and y, by the same 12 programs.
//! - `gDirectionalLight` (row 0 of `lighting_globals`) and `gDirectionalColour` (row 1) are read whole, as a direction and
//!   a colour: `VS_TransformDeferred` multiplies `saturate(dot(normal, -light.xyz))` by the colour.
//! - `gDirectionalAmbientColour` is read whole by 1,000 programs, in `VS_TransformD` through its w lane
//!   (`(w - 1) * fade + 1` scales the vertex colour's x and y).
//! - `bumpiness` and `bumpSelfShadowAmount` are scalars read at one lane each.
//!
//! The counts are printed. Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use inputs::{Game, install, keys};
use naga::Expression;
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable, GtaStage};
use primase_gta::passes::resolve_shaders::split_program_name;
use primase_gta::read::source::GtaSource;

const NAMES: [&str; 14] = [
    "globalScalars",
    "globalScalars2",
    "gDirectionalAmbientColour",
    "gDirectionalLight",
    "globalScreenSize",
    "globalAnimUV0",
    "globalAnimUV1",
    "gUIHDRParams",
    "gClothVertexStride",
    "tintPaletteSelector",
    "bumpiness",
    "bumpSelfShadowAmount",
    "gViewInverse",
    "gDirectionalColour",
];

/// The lanes of constant-buffer rows a program reads: (space, register, row, lane); lane 4 for a row used whole.
fn rows_read(container: &[u8]) -> Result<BTreeSet<(u32, u32, u32, u32)>, String> {
    let program = iridia_dxil::lift(container).map_err(|e| e.to_string())?;
    let function = &program.module.entry_points.first().ok_or("no entry")?.function;
    let mut loads: BTreeMap<usize, (u32, u32, u32)> = BTreeMap::new();
    for (h, e) in function.expressions.iter() {
        let Expression::Load { pointer } = e else { continue };
        let row = match function.expressions.try_get(*pointer) {
            Ok(Expression::AccessIndex { base, index }) => Some((*base, *index)),
            Ok(Expression::Access { base, index }) => match function.expressions.try_get(*index) {
                Ok(Expression::Literal(naga::Literal::U32(v))) => Some((*base, *v)),
                _ => None,
            },
            _ => None,
        };
        let Some((base, row)) = row else { continue };
        let Ok(Expression::GlobalVariable(g)) = function.expressions.try_get(base) else {
            continue;
        };
        let Some(b) = program
            .module
            .global_variables
            .try_get(*g)
            .ok()
            .and_then(|g| g.binding)
        else {
            continue;
        };
        loads.insert(h.index(), (b.binding >> 16, b.binding & 0xffff, row));
    }
    let mut out = BTreeSet::new();
    let mut laned = BTreeSet::new();
    for (_, e) in function.expressions.iter() {
        if let Expression::AccessIndex { base, index } = e
            && let Some(&(sp, rg, row)) = loads.get(&base.index())
        {
            out.insert((sp, rg, row, *index));
            laned.insert(base.index());
        }
    }
    for (h, (sp, rg, row)) in &loads {
        if !laned.contains(h) {
            out.insert((*sp, *rg, *row, 4));
        }
    }
    Ok(out)
}

/// A constant: its buffer, byte offset, the vertex programs that read it and their stems.
type Read = (String, u16, usize, BTreeMap<String, usize>);

#[test]
fn which_programs_read_the_engine_constants() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    // name -> (buffer, offset, programs reading it, stems)
    let mut reads: BTreeMap<&str, Read> = BTreeMap::new();
    let mut lane_hist: BTreeMap<String, usize> = BTreeMap::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in effect.programs.iter().filter(|p| p.stage == GtaStage::Vertex) {
            let Some(e) = p.shader else { continue };
            if !seen.insert(e) {
                continue;
            }
            let (Some(sh), Some(table)) = (
                world.get::<Shader>(e),
                world.get::<GtaShaderTable>(e).map(|t| &t.0),
            ) else {
                continue;
            };
            let Ok(rows) = rows_read(&sh.bytecode) else {
                continue;
            };
            for (i, r) in table.resources.iter().enumerate() {
                if r.constants == 0 {
                    continue;
                }
                for c in table.constants_of(i) {
                    let Some(name) = table.name_of(c.name) else {
                        continue;
                    };
                    let Some(&key) = NAMES.iter().find(|n| **n == name) else {
                        continue;
                    };
                    let first = u32::from(c.offset) / 16;
                    let lane0 = u32::from(c.offset) % 16 / 4;
                    let lanes = primase_gta::passes::resolve_shaders::value_bytes(c.flags)
                        .map_or(1, |b| b / 4)
                        .min(4 - lane0);
                    let read: Vec<u32> = (lane0..lane0 + lanes)
                        .filter(|l| {
                            rows.iter().any(|(sp, rg, row, lane)| {
                                *sp == u32::from(r.space)
                                    && *rg == u32::from(r.register)
                                    && *row == first
                                    && (*lane == *l || *lane == 4)
                            })
                        })
                        .collect();
                    if read.is_empty() {
                        continue;
                    }
                    *lane_hist.entry(format!("{key} lanes {read:?}")).or_default() += 1;
                    let entry = reads.entry(key).or_insert_with(|| {
                        (
                            table.name_of(r.name).unwrap_or("?").to_owned(),
                            c.offset,
                            0,
                            BTreeMap::new(),
                        )
                    });
                    entry.2 += 1;
                    let stem = split_program_name(&p.name).map_or(p.name.clone(), |(s, _)| s.to_owned());
                    *entry.3.entry(stem).or_default() += 1;
                }
            }
        }
    }
    for (name, (buffer, offset, n, stems)) in &reads {
        let mut top: Vec<_> = stems.iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        println!(
            "READ {buffer}.{name} @{offset}: {n} vertex programs; top stems {:?}",
            &top[..top.len().min(6)]
        );
    }
    for (k, n) in &lane_hist {
        println!("LANES {k}: {n}");
    }
    let lanes = |name: &str| -> Vec<String> {
        lane_hist
            .keys()
            .filter(|k| k.starts_with(&format!("{name} lanes")))
            .cloned()
            .collect()
    };
    for (name, expected) in [
        ("globalAnimUV0", "[0, 1, 2]"),
        ("globalAnimUV1", "[0, 1, 2]"),
        ("tintPaletteSelector", "[0, 1]"),
        ("gClothVertexStride", "[2]"),
        ("gUIHDRParams", "[0, 1]"),
        ("gDirectionalLight", "[0, 1, 2, 3]"),
        ("gDirectionalColour", "[0, 1, 2, 3]"),
    ] {
        assert_eq!(lanes(name), [format!("{name} lanes {expected}")], "{name}");
    }
    assert_eq!(reads["gClothVertexStride"].2, reads["gUIHDRParams"].2);
    assert!(reads["gDirectionalAmbientColour"].2 > 900);
    assert!(reads["globalAnimUV0"].2 == reads["globalAnimUV1"].2);
}
