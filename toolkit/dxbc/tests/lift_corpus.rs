#![allow(clippy::unwrap_used)]
//! Lifting real programs: every DXBC container in a folder of extracted programs (the user's own install) is lifted,
//! the lifted program validated and written as WGSL. A program the lifter refuses is counted by reason (the
//! refusals are by name, never silent); a program it lifts must validate and write, or the lifter is wrong.
//! Needs the corpus oracle set's `sekiro/shaders`, `riders/shaders` or `sledding/shaders` (each a folder, searched
//! recursively, of containers as the game ships them).
use std::collections::BTreeMap;

use primase_testdata::Game;

mod common;

#[derive(Default)]
struct Census {
    programs: usize,
    lifted: usize,
    validated: usize,
    written: usize,
    /// Refusals by reason: how many, and the first program.
    refused: BTreeMap<String, (usize, String)>,
    invalid: Vec<String>,
    /// Lifted programs the executor refuses, by reason, with the first program of each.
    unrunnable: BTreeMap<String, (usize, String)>,
}

fn census(game: Game) {
    let Some((name, paths)) = common::corpus(game) else {
        return;
    };
    let mut c = Census::default();
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        c.programs += 1;
        let program = match primase_dxbc::lift(&bytes) {
            Ok(p) => p,
            Err(e) => {
                c.refused
                    .entry(primase_dxbc::refusal_reason(&e))
                    .or_insert((0, path.display().to_string()))
                    .0 += 1;
                continue;
            }
        };
        c.lifted += 1;
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
            Ok(_) => {
                c.written += 1;
                // the executor compiles it too, or refuses it by name
                if let Err(e) = primase_shader::exec::Executable::new(&program) {
                    let entry = c
                        .unrunnable
                        .entry(e.to_string())
                        .or_insert((0, path.display().to_string()));
                    entry.0 += 1;
                }
            }
            Err(e) => c.invalid.push(format!("{}: {e}", path.display())),
        }
    }
    println!(
        "{name}: {} programs, {} lifted, {} validated, {} written as WGSL, {} refused",
        c.programs,
        c.lifted,
        c.validated,
        c.written,
        c.refused.values().map(|(n, _)| n).sum::<usize>()
    );
    for (reason, (n, example)) in &c.refused {
        println!("  refused {n:5}  {reason}  (e.g. {example})");
    }
    for (reason, (n, example)) in &c.unrunnable {
        println!("  not executable {n:5}  {reason}  (e.g. {example})");
    }
    for line in c.invalid.iter().take(20) {
        println!("  INVALID {line}");
    }
    assert!(
        c.invalid.is_empty(),
        "{} lifted programs failed validation or writing",
        c.invalid.len()
    );
}

#[test]
fn sekiro_programs() {
    census(Game::Sekiro);
}
#[test]
fn riders_republic_programs() {
    census(Game::Riders);
}
#[test]
fn unity_programs() {
    census(Game::Sledding);
}
