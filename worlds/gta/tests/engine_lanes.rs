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
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use inputs::{Game, install, keys};
use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::{GtaEffect, GtaShaderTable};
use primase_gta::read::source::GtaSource;

/// The lanes of rows a program reads, from its WGSL (the emitter writes only what the program uses; the lifted IR also
/// holds an access per lane it never uses): (space, register, row, lane) directly taken, rows used otherwise (a whole
/// vector), and buffers indexed by a value that is not a literal.
type Reads = (
    BTreeSet<(u32, u32, u32, u32)>,
    BTreeSet<(u32, u32, u32)>,
    BTreeSet<(u32, u32)>,
);

fn reads(container: &[u8]) -> Result<Reads, String> {
    let program = iridia_dxil::lift(container).map_err(|e| e.to_string())?;
    let text = iridia::emit::wgsl(&program).map_err(|e| format!("{e:?}"))?;
    // buffer variable name -> (space, register)
    let mut buffers: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if let Some(b) = l
            .trim()
            .strip_prefix("@group(0) @binding(")
            .and_then(|r| r.strip_suffix(')'))
            && let Ok(b) = b.trim().parse::<u32>()
            && let Some(v) = lines
                .get(i + 1)
                .and_then(|n| n.trim().strip_prefix("var<uniform> "))
            && let Some((name, _)) = v.split_once(':')
        {
            buffers.insert(name.to_owned(), (b >> 16, b & 0xffff));
        }
    }
    let (mut lanes, mut whole, mut dynamic) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    // let _eK = cbN_[ROW]; -> K -> (space, reg, row)
    let mut loaded: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for l in &lines {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("let ")
            && let Some((var, rhs)) = rest.split_once(" = ")
            && let Some((buf, idx)) = rhs.trim_end_matches(';').split_once('[')
            && let Some(&(sp, rg)) = buffers.get(buf)
            && let Some(idx) = idx.strip_suffix(']')
        {
            match idx.parse::<u32>() {
                Ok(row) => {
                    loaded.insert(var.to_owned(), (sp, rg, row));
                }
                Err(_) => {
                    dynamic.insert((sp, rg));
                }
            }
        }
    }
    for (var, &(sp, rg, row)) in &loaded {
        let mut any_lane = false;
        let mut other = false;
        for l in &lines {
            let mut rest: &str = l;
            let declared = l.trim().starts_with(&format!("let {var} = "));
            if declared {
                continue;
            }
            while let Some(at) = rest.find(var.as_str()) {
                let after = &rest[at + var.len()..];
                let before_ok = rest[..at]
                    .chars()
                    .last()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if before_ok {
                    match after.strip_prefix('.').and_then(|a| a.chars().next()) {
                        Some(c @ ('x' | 'y' | 'z' | 'w')) => {
                            lanes.insert((sp, rg, row, "xyzw".find(c).unwrap() as u32));
                            any_lane = true;
                        }
                        _ if after
                            .chars()
                            .next()
                            .is_none_or(|c| !(c.is_alphanumeric() || c == '_')) =>
                        {
                            other = true
                        }
                        _ => {}
                    }
                }
                rest = after;
            }
        }
        if other || !any_lane {
            whole.insert((sp, rg, row));
        }
    }
    Ok((lanes, whole, dynamic))
}

#[test]
fn lanes() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    let (mut programs, mut unlifted) = (0usize, 0usize);
    // (name, lane|"whole"|"dynamic buffer") -> programs
    let mut hist: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut examples: BTreeMap<(String, String), String> = BTreeMap::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in &effect.programs {
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
            let Ok((lanes, whole, dynamic)) = reads(&sh.bytecode) else {
                unlifted += 1;
                continue;
            };
            programs += 1;
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
                    let mut note = |what: String| {
                        *hist.entry((name.to_owned(), what.clone())).or_default() += 1;
                        examples
                            .entry((name.to_owned(), what))
                            .or_insert_with(|| p.name.clone());
                    };
                    for lane in 0..4 {
                        if lanes.contains(&(sp, rg, row, lane)) {
                            note(format!("lane {lane}"));
                        }
                    }
                    if whole.contains(&(sp, rg, row)) {
                        note("row loaded, no lane taken".into());
                    }
                    if dynamic.contains(&(sp, rg)) {
                        note("buffer indexed dynamically".into());
                    }
                }
            }
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
