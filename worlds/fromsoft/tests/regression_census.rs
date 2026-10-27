//! A census of what the FromSoft readers and passes make of the games' characters, against a committed baseline of counts.
//!
//! It reads Sekiro's Wolf in the default outfit (`CharaInitParam` 9000's parts), the Wolf in the costume of Ashina's Memory
//! (the `_m_9120_l` parts) and Elden Ring's c3000, runs the lab's character preparation on each (geometry, shader resolution,
//! material lowering, `bake-fromsoft-materials` at a size of 4 pixels: a bake's refusals do not depend on its size), and
//! counts: the primitives and materials; the programs; the roles of every constant, input, output and buffer of every bound
//! program (the Unknown ones by reason); the bake's and the resolution's log by reason class; and which skinned primitives a
//! game program can skin ([`primase_animation::game_skin`]) with the refusals by class. Counts and reason classes only: no
//! material or shader names, so a rename moves nothing, and a role that turns Unknown, a program that stops resolving or a
//! bake that starts refusing moves a number.
//!
//! The counts are compared exactly with `tests/fixtures/regression_census.txt`. A change of any number is a change of
//! what the readers make, and the baseline is replaced in the same commit as the change, with the reason in the message:
//! on a difference the test writes the new table to the scratch directory (`regression-census.txt`), prints each field with
//! the baseline's value and the new one, and fails. The worlds whose install is missing are skipped, and so are their
//! lines of the baseline. Phase times of each world are printed (`--nocapture`).
mod inputs;

use primase_fromsoft::game::ParamKey;
use std::collections::BTreeMap;
use std::time::Instant;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Install, Keys};
use iridia_interface::{BufferSource, Stage};
use primase_animation::game_skin;
use primase_core::pass::PassLog;
use primase_core::shader::ShaderBinding;
use primase_core::{Mesh, Skin, World};
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::{self, CharacterOptions};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

const BASELINE: &str = include_str!("fixtures/regression_census.txt");

/// The Wolf's default outfit: `CharaInitParam` 9000 equips `EquipParamProtector` 100000 to 103000, whose models are these.
const WOLF: [&str; 4] = ["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"];
/// The Wolf in the costume of Ashina's Memory.
const WOLF_ASHINA: [&str; 5] = [
    "fc_m_0220_l",
    "am_m_9120_l",
    "bd_m_9120_l",
    "lg_m_9120_l",
    "hd_m_9120_l",
];

/// The character preparation the lab runs for the Game shader view and the Shaded view's bake, one pass a step so each has its
/// own time and log.
const PASSES: [&str; 4] = [
    "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
    "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    "bake-fromsoft-materials{max-size=4,sample-time=0}",
];

/// A text with its numbers replaced by `N` and cut at 140 characters: the reason of a refusal as a class.
fn class(text: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('N');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out.chars().take(140).collect()
}

/// A role's debug text as a class: the variant's name alone for those that carry data, and for an Unknown its reason as the
/// source states it (its numbers are the program's, stable: a target's index, a register).
fn role_class(debug: &str) -> String {
    if debug.starts_with("Unknown") {
        let why = debug
            .find('"')
            .zip(debug.rfind('"'))
            .filter(|(a, b)| a < b)
            .map_or(debug, |(a, b)| &debug[a + 1..b]);
        return format!("Unknown: {}", why.chars().take(160).collect::<String>());
    }
    let name: String = debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    // a material's words and a lane list are not the role; `Object(BonePalette)` and `Frame(Time)`: the parameter is
    if matches!(name.as_str(), "Material" | "Lanes") {
        return name;
    }
    class(debug.split(['{', ' ']).next().unwrap_or(debug))
}

/// The census of one prepared world: its fields and counts.
fn count(world: &World, logs: &[(String, PassLog)]) -> BTreeMap<String, u64> {
    let mut c: BTreeMap<String, u64> = BTreeMap::new();
    let mut add = |field: String, n: u64| *c.entry(field).or_default() += n;
    // what is drawn
    let mut drawn = std::collections::BTreeSet::new();
    let mut primitives = 0u64;
    for (_, mesh) in world.iter::<Mesh>() {
        primitives += mesh.primitives().len() as u64;
        drawn.extend(mesh.primitives().iter().filter_map(|p| p.material));
    }
    add("primitives".into(), primitives);
    add("materials.drawn".into(), drawn.len() as u64);
    let mut pixel_programs = std::collections::BTreeSet::new();
    let mut vertex_programs = std::collections::BTreeSet::new();
    for &material in &drawn {
        if world
            .get::<Material>(material)
            .is_some_and(|m| m.layered.is_some())
        {
            add("materials.layered".into(), 1);
        }
        let Some(binding) = world.get::<ShaderBinding>(material) else {
            add("materials.without_binding".into(), 1);
            continue;
        };
        if binding.stage(Stage::Pixel).is_some() {
            add("materials.resolved".into(), 1);
        }
        for stage in &binding.stages {
            let (programs, side) = match stage.stage {
                Stage::Pixel => (&mut pixel_programs, "pixel"),
                _ => (&mut vertex_programs, "vertex"),
            };
            programs.insert(stage.shader);
            for k in &stage.constants {
                add(
                    format!("constants.{side}.{}", role_class(&format!("{:?}", k.role))),
                    1,
                );
            }
            for i in &stage.inputs {
                add(
                    format!("inputs.{side}.{}", role_class(&format!("{:?}", i.role))),
                    1,
                );
            }
            for o in &stage.outputs {
                add(
                    format!("outputs.{side}.{}", role_class(&format!("{:?}", o.role))),
                    1,
                );
            }
            for b in &stage.buffers {
                let what = match &b.source {
                    BufferSource::Table(t) => role_class(&format!("{t:?}")),
                    BufferSource::Unknown { why } => role_class(&format!("Unknown {{ why: {why:?} }}")),
                };
                add(format!("buffers.{side}.{what}"), 1);
            }
        }
    }
    add("programs.pixel".into(), pixel_programs.len() as u64);
    add("programs.vertex".into(), vertex_programs.len() as u64);
    // which skinned primitives a game program skins by the palette
    for (prototype, skin) in world.iter::<Skin>() {
        let Some(mesh) = world.get::<Mesh>(prototype) else {
            continue;
        };
        for (i, how) in skin.influences.iter().enumerate() {
            let (Some(how), Some(primitive)) = (how, mesh.primitives().get(i)) else {
                continue;
            };
            add("skin.primitives".into(), 1);
            let matrices = primitive
                .material
                .and_then(|m| world.get::<ShaderBinding>(m))
                .and_then(|b| {
                    b.stages
                        .iter()
                        .filter(|s| s.stage == Stage::Vertex)
                        .find_map(|s| s.bone_palette_matrices())
                });
            let Some(matrices) = matrices else {
                add(
                    format!("skin.refused.{}", game_skin::Refusal::NoPaletteConstant.class()),
                    1,
                );
                continue;
            };
            add(format!("skin.palette_matrices.{matrices}"), 1);
            match game_skin::skin_vertices(primitive, *how, matrices) {
                Ok(_) => add("skin.game_skinnable".into(), 1),
                Err(refusal) => add(format!("skin.refused.{}", refusal.class()), 1),
            }
        }
    }
    // the logs of the resolution and the bake, by class
    for (pass, log) in logs {
        let name = pass.split('{').next().unwrap_or(pass);
        if !matches!(name, "resolve-fromsoft-shaders" | "bake-fromsoft-materials") {
            continue;
        }
        for (what, n) in &log.counts {
            add(format!("log.{name}.count.{}", class(what)), *n);
        }
        for (what, n) in &log.approximations {
            add(format!("log.{name}.approximation.{}", class(what)), *n);
        }
        for (what, n) in &log.omitted {
            add(format!("log.{name}.omitted.{}", class(what)), *n);
        }
    }
    c
}

/// Read one world and prepare it, printing the phase times, and its census with every field under `name`.
fn census(name: &str, read: impl FnOnce(&mut World) -> Result<()>) -> Result<BTreeMap<String, u64>> {
    let started = Instant::now();
    let mut world = World::default();
    read(&mut world)?;
    println!("{name}: read {:.1} s", started.elapsed().as_secs_f32());
    let mut logs = Vec::new();
    for pass in PASSES {
        let at = Instant::now();
        let report = common::run(&mut world, pass)?;
        println!(
            "{name}: {} {:.1} s",
            pass.split('{').next().unwrap_or(pass),
            at.elapsed().as_secs_f32()
        );
        for step in report.steps {
            logs.push((step.pass, step.log));
        }
    }
    let at = Instant::now();
    let counts = count(&world, &logs);
    println!(
        "{name}: census {:.1} s, total {:.1} s",
        at.elapsed().as_secs_f32(),
        started.elapsed().as_secs_f32()
    );
    Ok(counts
        .into_iter()
        .map(|(field, n)| (format!("{name}.{field}"), n))
        .collect())
}

/// The baseline's lines `field=count`.
fn parse(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let (field, n) = l.rsplit_once('=')?;
            Some((field.to_owned(), n.trim().parse().ok()?))
        })
        .collect()
}

#[test]
fn what_the_readers_and_passes_make_of_the_characters_is_the_baseline() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut now: BTreeMap<String, u64> = BTreeMap::new();
    let mut ran: Vec<&str> = Vec::new();
    if let Some(root) = inputs::install(inputs::Game::Sekiro)
        && let Some(root) = common::sekiro(root)?
    {
        now.extend(census("wolf-default", |w| {
            sekiro::read_parts_with(&root, &WOLF, &decoder, &Default::default(), w)?;
            Ok(())
        })?);
        now.extend(census("wolf-ashina", |w| {
            sekiro::read_parts_with(&root, &WOLF_ASHINA, &decoder, &Default::default(), w)?;
            Ok(())
        })?);
        ran.extend(["wolf-default.", "wolf-ashina."]);
    }
    if let (Some(root), Some(keys)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
    ) {
        let install = Install::new(
            root,
            Game::EldenRing,
            Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
        );
        now.extend(census("elden-c3000", |w| {
            sekiro::read_character_with(&install, "c3000", &decoder, CharacterOptions::default(), w)?;
            Ok(())
        })?);
        ran.push("elden-c3000.");
    }
    if ran.is_empty() {
        return Ok(());
    }
    let baseline: BTreeMap<String, u64> = parse(BASELINE)
        .into_iter()
        .filter(|(field, _)| ran.iter().any(|w| field.starts_with(w)))
        .collect();
    if baseline == now {
        return Ok(());
    }
    // every field that moved, with both values
    let mut diff = Vec::new();
    for field in baseline
        .keys()
        .chain(now.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let (was, is) = (baseline.get(field), now.get(field));
        if was != is {
            let show = |v: Option<&u64>| v.map_or("absent".to_owned(), ToString::to_string);
            diff.push(format!("{field}: baseline {}, now {}", show(was), show(is)));
        }
    }
    let mut table = String::from(
        "# regression census of primase-fromsoft (tests/regression_census.rs): world.field=count\n\
         # replace this file in the commit that changes any number, with the reason in the message\n",
    );
    // the worlds this run did not make keep their lines
    let mut all: BTreeMap<String, u64> = parse(BASELINE)
        .into_iter()
        .filter(|(field, _)| !ran.iter().any(|w| field.starts_with(w)))
        .collect();
    all.extend(now);
    for (field, n) in &all {
        table.push_str(&format!("{field}={n}\n"));
    }
    let new = inputs::scratch().join("regression-census.txt");
    std::fs::write(&new, table)?;
    panic!(
        "the census is not the baseline ({} fields differ); the new table is {}\n{}",
        diff.len(),
        new.display(),
        diff.join("\n")
    );
}
