//! A census of the detail-blend programs against the detail model (`pbr.detail`, [`primase_core::detail`]), in two parts.
//! Both are `#[ignore]`d: they read the install (minutes), run alone with
//! `cargo test -p primase-fromsoft --test detail_census -- --ignored --nocapture`, and do nothing where the install, its
//! keys and the Oodle library are not configured (`PRIMASE_DATA`, `primase_testdata`).
//!
//! 1. `elden_ring_detail_blend_programs_by_family_and_switch` lists every `C[DetailBlend]` program of Elden Ring's shader
//!    bundles by family (the shader: `C[DetailBlend]`, `[Rich]`, `[S2]`, `_SSS`, `_cloth`, ...), switch and pass. It reads
//!    the bundles only, so it covers the whole install:
//!    - `PROGRAM <family> switch <n|-> <pass> <stage> <bytes>`: one program;
//!    - `FAMILY <family> programs <n> gbuffer <n> switches [..]`: a family's programs, how many are G-buffer pixel
//!      programs (the ones the equation is derived from), and its G-buffer switches;
//!    - `VERDICT <family> <derived|refused: reason>`: whether the pass derives the family
//!      ([`primase_fromsoft::sekiro::passes::detail::family`]) or refuses it by name.
//! 2. `detail_blend_materials_of_characters_by_program` reads characters (Elden Ring's `c2120`, `c3251`, `c2030`, `c2010`
//!    and Sekiro's `c1070` by default; `PRIMASE_DETAIL_CENSUS=elden:c2120,c3251;sekiro:c1070` chooses others), lowers them
//!    as the lab lowers a character up to its shader binding, and derives each derived-family material's equation
//!    ([`primase_fromsoft::detail_derive::derive`]) against the phase-1 vocabulary, one line per material:
//!    - `FITS <game> <char> <program> lowering <..> base_saturation <..> weights <n> proof <role=units ..>`: the
//!      equation is proven within [`BOUND_UNITS`]; the largest difference per role is the measured bound of that program
//!      (the exp2/log2 power against squaring included: record these per program);
//!    - `LACKS <game> <char> <program>: <reason>`: the program has a term the vocabulary lacks, or the probe could not
//!      run it, named;
//!    - `SUMMARY <game>: ..`: the counts by program, and by reason.
//!
//!    A program no sampled character's material uses is not classified; add characters to cover it.
#![allow(clippy::unwrap_used)]
use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::shader::{Shader, ShaderBinding, Stage};
use primase_fromsoft::detail_derive::{BOUND_UNITS, derive};
use primase_fromsoft::detail_probe::ProgramProbe;
use primase_fromsoft::materials::{Definition, LayeredMaterial, Material};
use primase_fromsoft::sekiro::passes::detail::{family, proof_seed};
use primase_fromsoft::sekiro::{self, CharacterOptions};
use primase_fromsoft::{Binder, Game, Install, Keys, dcx::Decoder};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The leaf of a binder member's path.
fn leaf(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}

/// A program's file name split into its family (the shader), its switch (the first all-digit part) and its pass (the
/// rest): `C[DetailBlend][Rich]_cloth_6_Gbuf_[A].ppo` is `C[DetailBlend][Rich]_cloth`, `6`, `Gbuf_[A]`. Parts are
/// separated by underscores outside brackets.
fn split(file: &str) -> (String, Option<u32>, String) {
    let stem = file.rsplit_once('.').map_or(file, |x| x.0);
    let mut parts: Vec<String> = Vec::new();
    let (mut depth, mut current) = (0u32, String::new());
    for c in stem.chars() {
        match c {
            '[' => {
                depth += 1;
                current.push(c);
            }
            ']' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            '_' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    let digits = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
    let pass_start = |p: &str| digits(p) || p.starts_with("Gbuf") || p.starts_with("Fwd");
    let at = parts.iter().position(|p| pass_start(p)).unwrap_or(parts.len());
    let family = parts[..at].join("_");
    let (switch, rest) = match parts.get(at) {
        Some(p) if digits(p) => (p.parse().ok(), &parts[at + 1..]),
        _ => (None, &parts[at.min(parts.len())..]),
    };
    (family, switch, rest.join("_"))
}

fn install_and_decoder() -> Result<Option<(std::path::PathBuf, Install, primase_compress::oodle::Oodle)>> {
    let (Some(root), Some(keys), Some(dll)) = (
        primase_testdata::install(primase_testdata::Game::Elden),
        primase_testdata::keys(primase_testdata::Game::Elden),
        primase_testdata::oodle(primase_testdata::Tool::Oodle6),
    ) else {
        return Ok(None);
    };
    let install = Install::new(root.clone(), Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
    Ok(Some((root, install, common::oodle(dll)?)))
}

/// Every `C[DetailBlend]` program of Elden Ring's shader bundles, by family and switch.
#[test]
#[ignore = "reads the install's shader bundles"]
fn elden_ring_detail_blend_programs_by_family_and_switch() -> Result<()> {
    let Some((_, install, dll)) = install_and_decoder()? else {
        return Ok(());
    };
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: u32::MAX as usize,
    };
    let archive = install.open()?;
    // family -> (programs, G-buffer pixel programs, G-buffer switches)
    let mut families: BTreeMap<String, (usize, usize, BTreeSet<Option<u32>>)> = BTreeMap::new();
    for path in Game::EldenRing.shader_binders() {
        if !archive.contains(path) {
            continue;
        }
        let outer = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
        for bundle in &outer.files {
            let Ok(inner) = Binder::read(&bundle.bytes, &decoder) else {
                continue;
            };
            for f in inner.files {
                let Some(name) = f.name.as_deref() else {
                    continue;
                };
                let file = leaf(name);
                if !file.to_ascii_lowercase().starts_with("c[detailblend]") || file.ends_with(".metaparam") {
                    continue;
                }
                let (family_name, switch, pass) = split(file);
                let stage = file.rsplit_once('.').map_or("", |x| x.1);
                println!(
                    "PROGRAM {family_name} switch {} {pass} {stage} {}",
                    switch.map_or("-".to_owned(), |n| n.to_string()),
                    f.bytes.len()
                );
                let e = families.entry(family_name).or_default();
                e.0 += 1;
                if stage == "ppo" && pass.starts_with("Gbuf") {
                    e.1 += 1;
                    e.2.insert(switch);
                }
            }
        }
    }
    for (name, (programs, gbuffer, switches)) in &families {
        let switches: Vec<String> = switches
            .iter()
            .map(|s| s.map_or("-".to_owned(), |n| n.to_string()))
            .collect();
        println!("FAMILY {name} programs {programs} gbuffer {gbuffer} switches [{}]", switches.join(", "));
        // the pass's verdict on the shader of this family (the bundle's shader name is the family without a pass)
        let verdict = match family(name) {
            Some(Ok(game)) => format!("derived ({game:?})"),
            Some(Err(reason)) => format!("refused: {reason}"),
            None => "none of the detail-blend families".to_owned(),
        };
        println!("VERDICT {name} {verdict}");
    }
    assert!(!families.is_empty(), "the install holds no C[DetailBlend] program");
    Ok(())
}

/// The characters to sample: `game:ids;game:ids`, each id `cNNNN`.
fn sampled() -> Vec<(Game, Vec<String>)> {
    let default = "elden:c2120,c3251,c2030,c2010;sekiro:c1070";
    let text = std::env::var("PRIMASE_DETAIL_CENSUS").unwrap_or_else(|_| default.to_owned());
    text.split(';')
        .filter_map(|group| {
            let (game, ids) = group.split_once(':')?;
            let game = match game.trim() {
                "elden" => Game::EldenRing,
                "sekiro" => Game::Sekiro,
                _ => return None,
            };
            Some((game, ids.split(',').map(|i| i.trim().to_owned()).collect()))
        })
        .collect()
}

/// The detail-blend materials of sampled characters, each program's equation against the phase-1 vocabulary.
#[test]
#[ignore = "reads characters from the install"]
fn detail_blend_materials_of_characters_by_program() -> Result<()> {
    let sekiro_install = primase_testdata::install(primase_testdata::Game::Sekiro);
    let Some((_, elden, dll)) = install_and_decoder()? else {
        return Ok(());
    };
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, ids) in sampled() {
        let install = match game {
            Game::EldenRing => elden.clone(),
            Game::Sekiro => {
                let (Some(root), Some(keys)) = (sekiro_install.clone(), primase_testdata::keys(primase_testdata::Game::Sekiro))
                else {
                    println!("SKIPPED sekiro: no install or keys configured");
                    continue;
                };
                Install::new(root, Game::Sekiro, Keys::from_folder(keys, Game::Sekiro)?)
            }
        };
        // program -> (materials, fit, reasons)
        let mut by_program: BTreeMap<String, (usize, usize, BTreeMap<String, usize>)> = BTreeMap::new();
        for id in &ids {
            let mut world = World::default();
            if let Err(e) = sekiro::read_character_with(&install, id, &decoder, CharacterOptions::default(), &mut world) {
                println!("UNREAD {game:?} {id}: {e}");
                continue;
            }
            if let Err(e) = common::run(
                &mut world,
                "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}, \
                 resolve-fromsoft-shaders{unbound-textures=preview}",
            ) {
                println!("UNLOWERED {game:?} {id}: {e}");
                continue;
            }
            let materials: Vec<(primase_core::Entity, Material)> =
                world.iter::<Material>().map(|(e, m)| (e, m.clone())).collect();
            for (e, material) in materials {
                let Some(verdict) = family(&material.shader) else {
                    continue;
                };
                let program = material
                    .view()
                    .map_or_else(|| format!("{} (no definition)", material.shader), |v| v.gbuffer_program());
                let entry = by_program.entry(program.clone()).or_default();
                entry.0 += 1;
                let outcome: std::result::Result<String, String> = (|| {
                    verdict.map_err(str::to_owned)?;
                    let binding = world.get::<ShaderBinding>(e).ok_or("no shader binding")?;
                    let stage = binding.stage(Stage::Pixel).ok_or("no pixel stage")?;
                    let shader = world.get::<Shader>(stage.shader).ok_or("no pixel program")?;
                    let mut probe = match game {
                        Game::Sekiro => {
                            let layered = match (&material.layered, &material.definition) {
                                (Some(l), _) => l.clone(),
                                (None, Some(Definition::Mtd(d))) => LayeredMaterial {
                                    mtd: material.mtd.clone(),
                                    definition: d.clone(),
                                    bindings: material.bindings.clone(),
                                },
                                _ => return Err("no MTD definition".to_owned()),
                            };
                            ProgramProbe::with_material(&shader.bytecode, &layered)
                        }
                        Game::EldenRing => {
                            let view = material.view().ok_or("no material definition")?;
                            ProgramProbe::with_view(game, &shader.bytecode, &view)
                        }
                    }
                    .map_err(|e| format!("its program cannot be probed: {e}"))?;
                    let d = derive(&mut probe, proof_seed(&material.mtd))?;
                    let c = &d.equation.constants;
                    let proof: Vec<String> = d
                        .equation
                        .proof
                        .largest
                        .iter()
                        .map(|(role, u)| format!("{role}={u:.1}"))
                        .collect();
                    let transition = d.equation.proof.transition.as_ref().map_or(String::new(), |(sets, largest)| {
                        let l: Vec<String> = largest.iter().map(|(role, u)| format!("{role}={u:.1}")).collect();
                        format!(" transition {sets} sets {}", l.join(" "))
                    });
                    Ok(format!(
                        "lowering {:?} base_saturation {} weights {} proof {}{} (bound {BOUND_UNITS})",
                        c.lowering,
                        c.base_saturation,
                        c.weights.len(),
                        proof.join(" "),
                        transition
                    ))
                })();
                match outcome {
                    Ok(fits) => {
                        entry.1 += 1;
                        println!("FITS {game:?} {id} {program} {fits}");
                    }
                    Err(reason) => {
                        let first: String = reason.chars().take(240).collect();
                        *entry.2.entry(first.clone()).or_default() += 1;
                        println!("LACKS {game:?} {id} {program}: {first}");
                    }
                }
            }
        }
        for (program, (materials, fit, reasons)) in &by_program {
            println!("SUMMARY {game:?}: {program} materials {materials} fit {fit} lack {}", materials - fit);
            for (reason, n) in reasons {
                println!("SUMMARY {game:?}:   {n} x {reason}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod splitting {
    use super::*;

    /// A program's file name is its family (the shader), its switch and its pass.
    #[test]
    fn a_program_name_splits_into_family_switch_and_pass() {
        assert_eq!(
            split("C[DetailBlend][Rich]_cloth_6_Gbuf_[A].ppo"),
            ("C[DetailBlend][Rich]_cloth".to_owned(), Some(6), "Gbuf_[A]".to_owned())
        );
        assert_eq!(
            split("C[DetailBlend]_0_Gbuf.ppo"),
            ("C[DetailBlend]".to_owned(), Some(0), "Gbuf".to_owned())
        );
        assert_eq!(
            split("C[DetailBlend]_Gbuf_Skin.vpo"),
            ("C[DetailBlend]".to_owned(), None, "Gbuf_Skin".to_owned())
        );
        assert_eq!(
            split("C[DetailBlend][1_0_0_0][S2]_Glow[Int]_3_Gbuf.ppo"),
            ("C[DetailBlend][1_0_0_0][S2]_Glow[Int]".to_owned(), Some(3), "Gbuf".to_owned())
        );
    }
}
