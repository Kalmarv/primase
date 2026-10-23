#![allow(clippy::unwrap_used)]
//! Every compiled effect (`.fxc`) of an install: read to its last byte, each program's container checked (DXBC or
//! DXIL part), lifted to the shader representation where it is a vertex or pixel program, and the census printed
//! (effects, programs per stage, shader models, refusals by reason). The reader fails on a file it cannot read to
//! its end; the lift's refusals are counted, not failures, but a program that lifts must validate and write WGSL.
//! Needs `PRIMASE_GTA_SHADERS`, a folder of Legacy's `.fxc` files (the Enhanced edition has none: see `sgd2.rs`).
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
         {lifted} vertex, pixel and compute programs lifted, {} refused",
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

/// GTA V Enhanced's shader database: `PRIMASE_GTA_SHADER_DB`, the `sga_win32_60_final.awc` file (or its `_init`
/// sibling) as the install stores it after decryption and decompression. Every group and effect is read to the end
/// of the file; every program's container is checked and every vertex, pixel and compute program lifted (DXIL), validated and
/// written as WGSL; the effects' program lists are checked to name programs of the right stage.
#[test]
fn the_enhanced_shader_database() {
    use primase_gta::rage::sgd2;
    let Some(path) = primase_testdata::real_data("PRIMASE_GTA_SHADER_DB") else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let db = sgd2::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let counts: Vec<usize> = db.groups.iter().map(Vec::len).collect();
    println!("groups {counts:?}, {} effects", db.effects.len());
    let mut world = primase_core::World::new();
    let table = primase_gta::read::shaders::add_shader_database(&mut world, &db);
    println!(
        "world: {} effects, {} shader entities, {} interfaces not read",
        table.effects.len(),
        table.shaders,
        table.unread.len()
    );
    for (name, reason) in table.unread.iter().take(10) {
        println!("  unread {name}: {reason}");
    }
    assert_eq!(table.effects.len(), db.effects.len());
    let mut tags: BTreeMap<u8, usize> = BTreeMap::new();
    for g in &db.groups {
        for p in g {
            *tags.entry(p.tag).or_default() += 1;
        }
    }
    println!("TAGS {tags:?}");
    let (mut lifted, mut programs) = (0, 0);
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    let mut invalid = Vec::new();
    let mut unrunnable: BTreeMap<String, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<(usize, u32), usize> = BTreeMap::new();
    // per group: programs, lifted
    let mut tally: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    let mut other_ops: BTreeMap<(usize, String), usize> = BTreeMap::new();
    for (g, stage) in sgd2::GROUP_STAGES.iter().enumerate() {
        for program in &db.groups[g] {
            programs += 1;
            let chunks =
                primase_dxbc::chunks(program.container).unwrap_or_else(|e| panic!("{}: {e}", program.name));
            assert!(
                chunks.iter().any(|c| &c.kind == b"DXIL"),
                "{}: not a DXIL container",
                program.name
            );
            if std::env::var_os("OPS").is_some()
                && matches!(stage, Some(ProgramStage::Compute))
                && let Ok((_, calls)) = primase_dxil::operations(program.container)
            {
                for (name, n) in calls {
                    *other_ops.entry((g, name)).or_default() += n;
                }
            }
            if !matches!(
                stage,
                Some(ProgramStage::Vertex | ProgramStage::Pixel | ProgramStage::Compute)
            ) {
                // the stages with no lifter yet: which kind the container is, and the operations they call
                let (kind, calls) = match primase_dxil::operations(program.container) {
                    Ok(r) => r,
                    Err(e) => {
                        *other_ops
                            .entry((g, format!("UNREADABLE {}", primase_dxil::refusal_reason(&e))))
                            .or_default() += 1;
                        continue;
                    }
                };
                *kinds.entry((g, kind)).or_default() += 1;
                for (name, n) in calls {
                    *other_ops.entry((g, name)).or_default() += n;
                }
                continue;
            }
            tally.entry(g).or_default().0 += 1;
            match primase_dxil::lift(program.container) {
                Err(e) => {
                    if std::env::var_os("SHOW").is_some_and(|k| e.detail.contains(&*k.to_string_lossy())) {
                        println!("SHOW {}: {}", program.name, e.detail);
                    }
                    *refused.entry(primase_dxil::refusal_reason(&e)).or_default() += 1
                }
                Ok(p) => {
                    lifted += 1;
                    tally.entry(g).or_default().1 += 1;
                    match p.validate() {
                        Err(problems) => invalid.push(format!("{}: {problems:?}", program.name)),
                        Ok(_) => {
                            if let Err(e) = primase_shader::emit::wgsl(&p) {
                                invalid.push(format!("{}: {e}", program.name));
                            }
                            // the executor compiles it too, or refuses it by name
                            if let Err(e) = primase_shader::exec::Executable::new(&p) {
                                *unrunnable.entry(e.to_string()).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "{programs} programs, {lifted} vertex, pixel and compute programs lifted, {} refused",
        refused.values().sum::<usize>()
    );
    let mut reasons: Vec<_> = refused.iter().collect();
    reasons.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (reason, n) in reasons.iter().take(40) {
        println!("  refused {n:5}  {reason}");
    }
    println!("KINDS (group, program kind): {kinds:?}");
    println!("LIFTED per group (programs, lifted): {tally:?}");
    for ((g, name), n) in other_ops.iter().filter(|((_, name), _)| {
        name.starts_with("UNREADABLE")
            || (std::env::var_os("OPS").is_some() && (name.contains("wave") || name.contains("quad")))
    }) {
        println!("  group {g}: {n} {name}");
    }
    for (reason, n) in &unrunnable {
        println!("  not executable {n:5}  {reason}");
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
