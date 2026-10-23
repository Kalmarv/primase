#![allow(clippy::unwrap_used)]
//! Lifting real programs: every DXBC container in a folder of extracted programs (the user's own install) is lifted,
//! the lifted program validated and written as WGSL. A program the lifter refuses is counted by reason (the
//! refusals are by name, never silent); a program it lifts must validate and write, or the lifter is wrong.
//! Needs `PRIMASE_SHADER_CORPUS_SEKIRO`, `PRIMASE_SHADER_CORPUS_RR` or `PRIMASE_SHADER_CORPUS_UNITY` (each a folder,
//! searched recursively, of containers as the game ships them).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    refused: BTreeMap<String, usize>,
    invalid: Vec<String>,
}

fn census(variable: &str) {
    let Some(root) = primase_testdata::real_data(variable) else {
        return;
    };
    let mut paths = Vec::new();
    containers(Path::new(&root), &mut paths);
    assert!(
        !paths.is_empty(),
        "{variable}: no DXBC containers under {}",
        root.display()
    );
    let mut c = Census::default();
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        c.programs += 1;
        let program = match primase_dxbc::lift(&bytes) {
            Ok(p) => p,
            Err(e) => {
                *c.refused.entry(primase_dxbc::refusal_reason(&e)).or_default() += 1;
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
        match primase_shader::emit::wgsl(&program) {
            Ok(_) => c.written += 1,
            Err(e) => c.invalid.push(format!("{}: {e}", path.display())),
        }
    }
    println!(
        "{variable}: {} programs, {} lifted, {} validated, {} written as WGSL, {} refused",
        c.programs,
        c.lifted,
        c.validated,
        c.written,
        c.refused.values().sum::<usize>()
    );
    for (reason, n) in &c.refused {
        println!("  refused {n:5}  {reason}");
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
    census("PRIMASE_SHADER_CORPUS_SEKIRO");
}
#[test]
fn riders_republic_programs() {
    census("PRIMASE_SHADER_CORPUS_RR");
}
#[test]
fn unity_programs() {
    census("PRIMASE_SHADER_CORPUS_UNITY");
}
