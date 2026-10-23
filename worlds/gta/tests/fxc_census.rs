#![allow(clippy::unwrap_used)]
//! Every compiled effect (`.fxc`) of an install: read to its last byte, each program's container checked (DXBC or
//! DXIL part), lifted to the shader representation where it is a vertex or pixel program, and the census printed
//! (effects, programs per stage, shader models, refusals by reason). The reader fails on a file it cannot read to
//! its end; the lift's refusals are counted, not failures, but a program that lifts must validate and write WGSL.
//! Needs `PRIMASE_GTA_SHADERS`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_gta::rage::fxc::{self, ProgramStage};

fn files(root: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fxc")) {
            out.push(p);
        }
    }
}

#[test]
fn every_effect_of_the_install() {
    let Some(root) = primase_testdata::real_data("PRIMASE_GTA_SHADERS") else {
        return;
    };
    let mut paths = Vec::new();
    files(Path::new(&root), &mut paths);
    assert!(!paths.is_empty(), "no .fxc files under {}", root.display());
    let (mut effects, mut programs, mut dxil, mut dxbc) = (0, 0, 0, 0);
    let mut per_stage: BTreeMap<ProgramStage, usize> = BTreeMap::new();
    let mut models: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    let (mut lifted, mut invalid) = (0, Vec::new());
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let effect = fxc::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        effects += 1;
        for stage in ProgramStage::ALL {
            for shader in effect.shaders(stage) {
                *per_stage.entry(stage).or_default() += 1;
                if let Some(v) = shader.version {
                    *models.entry(v).or_default() += 1;
                }
                if shader.bytecode.is_empty() {
                    continue;
                }
                programs += 1;
                let chunks = primase_dxbc::chunks(&shader.bytecode)
                    .unwrap_or_else(|e| panic!("{} {}: {e}", path.display(), shader.name));
                let is_dxil = chunks.iter().any(|c| &c.kind == b"DXIL");
                if is_dxil {
                    dxil += 1;
                } else {
                    dxbc += 1;
                }
                if !matches!(stage, ProgramStage::Vertex | ProgramStage::Pixel) {
                    continue;
                }
                let result = if is_dxil {
                    primase_dxil::lift(&shader.bytecode).map_err(|e| primase_dxil::refusal_reason(&e))
                } else {
                    primase_dxbc::lift(&shader.bytecode).map_err(|e| primase_dxbc::refusal_reason(&e))
                };
                match result {
                    Err(reason) => *refused.entry(reason).or_default() += 1,
                    Ok(program) => {
                        lifted += 1;
                        let name = format!("{} {}", path.display(), shader.name);
                        match program.validate() {
                            Err(p) => invalid.push(format!("{name}: {p:?}")),
                            Ok(_) => {
                                if let Err(e) = primase_shader::emit::wgsl(&program) {
                                    invalid.push(format!("{name}: {e}"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{effects} effects, {programs} programs ({dxbc} DXBC, {dxil} DXIL), per stage {per_stage:?}, models {models:?}; \
         {lifted} vertex and pixel programs lifted, {} refused",
        refused.values().sum::<usize>()
    );
    for (reason, n) in &refused {
        println!("  refused {n:5}  {reason}");
    }
    for line in invalid.iter().take(20) {
        println!("  INVALID {line}");
    }
    assert!(
        invalid.is_empty(),
        "{} lifted programs failed validation or writing",
        invalid.len()
    );
}
