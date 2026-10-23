#![allow(clippy::unwrap_used)]
//! Lifting real programs: every DXIL container in a folder of extracted programs (the user's own install) is lifted,
//! the lifted program validated and written as WGSL (as its functions where its stage has no WGSL stage). A program
//! the lifter refuses is counted by reason, with the first program of each (the refusals are by name, never silent);
//! a program it lifts must validate and write, or the lifter is wrong. Needs the corpus oracle set's `skate/shaders`
//! or `elden/shaders` (each a folder, searched recursively, of containers as the game ships them).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_testdata::{Game, OracleSet, Query, expect_found, resolve};

fn containers(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            containers(&path, out);
        } else if std::fs::read(&path).is_ok_and(|b| b.starts_with(b"DXBC")) {
            out.push(path);
        }
    }
}

#[derive(Default)]
struct Census {
    programs: usize,
    lifted: usize,
    validated: usize,
    written: usize,
    /// Refusals by reason: how many, and the first program.
    refused: BTreeMap<String, (usize, String)>,
    /// Lifted programs by stage.
    stages: BTreeMap<String, usize>,
    invalid: Vec<String>,
    /// Validated programs a writer cannot write, by reason, with the first program of each.
    unwritten: BTreeMap<String, (usize, String)>,
    /// Lifted programs the executor refuses, by reason, with the first program of each.
    unrunnable: BTreeMap<String, (usize, String)>,
}

fn census(game: Game) {
    let query = Query::Oracle(OracleSet::Corpus, game, "shaders");
    let Some(root) = resolve(query) else {
        return;
    };
    let mut paths = Vec::new();
    containers(&root, &mut paths);
    expect_found(query, "DXBC containers", paths.len());
    let mut c = Census::default();
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        c.programs += 1;
        let program = match primase_dxil::lift(&bytes) {
            Ok(p) => p,
            Err(e) => {
                let entry = c
                    .refused
                    .entry(primase_dxil::refusal_reason(&e))
                    .or_insert((0, path.display().to_string()));
                entry.0 += 1;
                continue;
            }
        };
        c.lifted += 1;
        *c.stages
            .entry(format!("{:?}", program.interface.stage))
            .or_default() += 1;
        match program.validate() {
            Ok(_) => c.validated += 1,
            Err(problems) => {
                c.invalid.push(format!(
                    "{}: {}",
                    path.display(),
                    problems
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                continue;
            }
        }
        // a program of a stage with no WGSL stage is written as its functions
        let written = if primase_shader::stage_functions(&program.interface).is_ok_and(|f| f.is_some()) {
            primase_shader::emit::wgsl_functions(&program)
        } else {
            primase_shader::emit::wgsl(&program)
        };
        match written {
            Ok(_) => c.written += 1,
            // a program the validator accepts and a writer says it cannot write is counted, not wrong
            Err(primase_shader::ProgramError::Unsupported { detail, .. }) => {
                let entry = c
                    .unwritten
                    .entry(detail)
                    .or_insert((0, path.display().to_string()));
                entry.0 += 1;
            }
            Err(e) => c.invalid.push(format!("{}: {e}", path.display())),
        }
        // the executor compiles it too, or refuses it by name
        if let Err(e) = primase_shader::exec::Executable::new(&program) {
            let entry = c
                .unrunnable
                .entry(e.to_string())
                .or_insert((0, path.display().to_string()));
            entry.0 += 1;
        }
    }
    println!(
        "{}: {} programs, {} lifted {:?}, {} validated, {} written as WGSL, {} refused",
        query.key(),
        c.programs,
        c.lifted,
        c.stages,
        c.validated,
        c.written,
        c.refused.values().map(|(n, _)| n).sum::<usize>()
    );
    for (reason, (n, example)) in &c.refused {
        println!("  refused {n:5}  {reason}  (e.g. {example})");
    }
    for (reason, (n, example)) in &c.unwritten {
        println!("  not written {n:5}  {reason}  (e.g. {example})");
    }
    for (reason, (n, example)) in &c.unrunnable {
        println!("  not executable {n:5}  {reason}  (e.g. {example})");
    }
    for line in c.invalid.iter().take(40) {
        println!("  INVALID {line}");
    }
    assert!(
        c.invalid.is_empty(),
        "{} lifted programs failed validation or writing",
        c.invalid.len()
    );
}

#[test]
fn skate_programs() {
    census(Game::Skate);
}
#[test]
fn elden_ring_programs() {
    census(Game::Elden);
}
