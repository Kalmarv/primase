//! The constants a program reads ([`iridia::WordsRead`] of the lifted program, from its value graph) held to an
//! independent reader of the same programs: the lanes each constant-buffer row is taken at in the program's emitted WGSL
//! (which writes only what the program computes with). Over every distinct program of the shader database that lifts,
//! every word the value graph reaches is a lane the code takes; the lanes the code takes that nothing the program
//! outputs, discards or writes is made of are counted, not errors, and the programs left out are printed by where and
//! why (no WGSL, no lift, the value graph refused).
//!
//! Needs `installs.gta` and `keys.gta`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod common;
mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use inputs::{Game, install, keys};
use iridia::ResourceKind;
use primase_core::World;
use primase_gta::read::source::GtaSource;

/// What one program gives: why it is left out (where, and the error), or what comparing its reads found.
enum Outcome {
    Skipped(&'static str, String),
    Compared {
        rows: usize,
        dynamic_buffers: usize,
        unused: usize,
        differ: Vec<String>,
    },
}

/// The program's value-graph reads held to the lanes its WGSL takes, from one lift.
fn compare(p: &common::Program) -> Outcome {
    let program = match iridia_dxil::lift(&p.shader.bytecode) {
        Ok(program) => program,
        Err(e) => return Outcome::Skipped("no lift", e.to_string()),
    };
    let (lanes, whole, dynamic) = match common::wgsl_reads(&program) {
        Ok(r) => r,
        Err(e) => return Outcome::Skipped("no WGSL", e),
    };
    let mine = match program.constant_reads() {
        Ok(m) => m,
        Err(e) => return Outcome::Skipped("value graph refused", e),
    };
    let (mut rows, mut unused, mut differ) = (0usize, 0usize, Vec::new());
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
                    p.program.name,
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
    Outcome::Compared {
        rows,
        dynamic_buffers: dynamic.len(),
        unused,
        differ,
    }
}

#[test]
fn the_reads_agree_with_the_emitted_code() {
    let (Some(dir), Some(keys)) = (install(Game::Gta), keys(Game::Gta)) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).unwrap();
    let mut world = World::new();
    source.add_shader_database(&mut world).unwrap();
    let (mut programs, mut rows, mut dynamic_buffers, mut unused) = (0usize, 0usize, 0usize, 0usize);
    // the programs left out of the comparison, by where and why (the message's first words)
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut differ: Vec<String> = Vec::new();
    for outcome in common::each(&common::programs(&world), compare) {
        match outcome {
            Outcome::Skipped(at, why) => {
                let why: String = why.chars().take(80).collect();
                *skipped.entry(format!("{at}: {why}")).or_default() += 1;
            }
            Outcome::Compared {
                rows: r,
                dynamic_buffers: d,
                unused: u,
                differ: mut more,
            } => {
                programs += 1;
                rows += r;
                dynamic_buffers += d;
                unused += u;
                more.truncate(8 - differ.len());
                differ.append(&mut more);
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
