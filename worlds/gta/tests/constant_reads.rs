//! The constants a program reads ([`iridia::WordsRead`] of the lifted program, from its value graph) held to an
//! independent reader of the same programs: the lanes each constant-buffer row is taken at in the program's emitted WGSL
//! (which writes only what the program computes with). Over every distinct program of the shader database that lifts,
//! every word the value graph reaches is a lane the code takes; the lanes the code takes that nothing the program
//! outputs, discards or writes is made of are counted, not errors, and the programs left out are printed by where and
//! why (no WGSL, no lift, the value graph refused).
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::Shader;
use primase_gta::components::GtaEffect;
use primase_gta::read::source::GtaSource;
use iridia::ResourceKind;
use primase_testdata::{Game, install, keys};

/// The lanes of rows a program reads, from its WGSL (the emitter writes only what the program uses; the lifted IR also
/// holds an access per lane it never uses): (space, register, row, lane) directly taken, rows used otherwise (a whole
/// vector), and buffers indexed by a value that is not a literal.
type Reads = (
    BTreeSet<(u32, u32, u32, u32)>,
    BTreeSet<(u32, u32, u32)>,
    BTreeSet<(u32, u32)>,
);

fn reads(container: &[u8]) -> Result<Reads, String> {
    let program = primase_dxil::lift(container).map_err(|e| e.to_string())?;
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
fn the_reads_agree_with_the_emitted_code() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let mut seen = BTreeSet::new();
    let (mut programs, mut rows, mut dynamic_buffers, mut unused) = (0usize, 0usize, 0usize, 0usize);
    // the programs left out of the comparison, by where and why (the message's first words)
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut skip = |at: &str, why: &str| {
        let why: String = why.chars().take(80).collect();
        *skipped.entry(format!("{at}: {why}")).or_default() += 1;
    };
    let mut differ: Vec<String> = Vec::new();
    for (_, effect) in world.iter::<GtaEffect>() {
        for p in &effect.programs {
            let Some(e) = p.shader else { continue };
            if !seen.insert(e) {
                continue;
            }
            let Some(sh) = world.get::<Shader>(e) else {
                continue;
            };
            let (lanes, whole, dynamic) = match reads(&sh.bytecode) {
                Ok(r) => r,
                Err(e) => {
                    skip("no WGSL", &e);
                    continue;
                }
            };
            let program = match primase_dxil::lift(&sh.bytecode) {
                Ok(p) => p,
                Err(e) => {
                    skip("no lift", &e.to_string());
                    continue;
                }
            };
            let mine = match program.constant_reads() {
                Ok(m) => m,
                Err(e) => {
                    skip("value graph refused", &e);
                    continue;
                }
            };
            programs += 1;
            dynamic_buffers += dynamic.len();
            // every row the text reader sees: the lanes it takes, or all four where the row is used whole
            let mut text: BTreeMap<(u32, u32, u32), BTreeSet<u32>> = BTreeMap::new();
            for (sp, rg, row, lane) in &lanes {
                text.entry((*sp, *rg, *row)).or_default().insert(*lane);
            }
            for (sp, rg, row) in &whole {
                text.entry((*sp, *rg, *row)).or_default().extend(0..4);
            }
            // every word the value graph reaches is a lane the code takes
            for r in &program.interface.resources {
                let (ResourceKind::ConstantBuffer { .. }, Some(d3d)) = (&r.kind, r.provenance.d3d) else {
                    continue;
                };
                let (sp, rg) = (d3d.space, d3d.register);
                if dynamic.contains(&(sp, rg)) {
                    continue;
                }
                for word in mine.words(r.id) {
                    let (row, lane) = (word / 4, word % 4);
                    if !text.get(&(sp, rg, row)).is_some_and(|l| l.contains(&lane)) && differ.len() < 8 {
                        differ.push(format!(
                            "{} row ({sp},{rg},{row}): reads lane {lane}, the code takes {:?}",
                            p.name,
                            text.get(&(sp, rg, row))
                        ));
                    }
                }
            }
            // the lanes the code takes and nothing is made of
            for ((sp, rg, row), l) in &text {
                rows += 1;
                if dynamic.contains(&(*sp, *rg)) {
                    continue;
                }
                unused += l
                    .iter()
                    .filter(|lane| !mine.reads_at(*sp, *rg, row * 16 + *lane * 4, 4))
                    .count();
            }
        }
    }
    println!(
        "{programs} programs, {rows} rows compared, {dynamic_buffers} dynamically indexed buffers, {unused} lanes taken \
         and unused"
    );
    for (why, n) in &skipped {
        println!("SKIPPED {n:6} {why}");
    }
    assert!(programs > 9_000);
    assert!(differ.is_empty(), "{differ:?}");
}
