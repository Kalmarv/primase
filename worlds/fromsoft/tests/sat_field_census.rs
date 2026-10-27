//! The material fields of every bound pixel program (`resolve-fromsoft-shaders` on the read): over Sekiro's Wolf in
//! the default outfit (`CharaInitParam` 9000's parts) and Elden Ring's c3000, a program that has material fields
//! (`_snp_` constants, or members of its `SAT_MtdParam`) binds at least one of the material's parameters to them, by
//! name (Sekiro's programs) or at the metaparam's start words (Elden Ring's). Before the fields were selected by the
//! parameters' names, Sekiro's `_cloth` shaders' programs bound none (their material and program name the parameters
//! after the base family, not after the `_cloth` shader).
//!
//! It prints, per read, how many bound programs have material fields and how many of those bind none, by program;
//! and each bound material's texture tiling (`FC_TextureTilingScale` rows by register), one `tiling` line a material,
//! for comparing two revisions' reads. The Wolf's scarf's rows are those Sekiro's capture shows (cb11-capture), and
//! no FLVER of c3000 stores a texture scale other than 1, so the tiling rule's first step (the FLVER's scale) changes
//! no row of Elden Ring's captured rule.
mod inputs;

use iridia_interface::{ConstantRole, ResourceKind};
use primase_core::World;
use primase_core::{shader::Shader, shader::ShaderBinding};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro::{self, CharacterOptions};
use primase_fromsoft::{Game, Install, Keys};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
mod common;

/// The Wolf's default outfit: `CharaInitParam` 9000 equips `EquipParamProtector` 100000 to 103000, whose models are
/// these parts.
const WOLF: [&str; 4] = ["fc_m_0200_l", "bd_m_9040_l", "am_m_9000_l", "lg_m_9000_l"];

/// Of `world`'s bound materials: the programs with material fields, and those of them binding none, with counts.
fn census(world: &World) -> Result<(usize, BTreeMap<String, usize>)> {
    let (mut with_fields, mut none) = (0, BTreeMap::new());
    for (e, _) in world.iter::<Material>() {
        let Some(binding) = world.get::<ShaderBinding>(e) else {
            continue;
        };
        let pixel = binding.stages.last().ok_or("no stage")?;
        let shader = world.get::<Shader>(pixel.shader).ok_or("no shader")?;
        let fields = shader.interface.resources.iter().any(|r| match &r.kind {
            ResourceKind::ConstantBuffer { layout } => layout.constants.iter().any(|c| {
                (c.name.contains("_snp_") && !c.name.ends_with("_PADDING"))
                    || (c.name == "SAT_MtdParam" && !c.ty.members().is_empty())
                    || c.ty.members().iter().any(|m| m.name.contains("_snp_"))
            }),
            _ => false,
        });
        if !fields {
            continue;
        }
        with_fields += 1;
        let bound = pixel.constants.iter().any(|c| {
            matches!(c.role, ConstantRole::Material(_))
                && c.name.as_deref().is_some_and(|n| n.contains("_snp_"))
        });
        if !bound {
            let name = world
                .get::<primase_core::Name>(pixel.shader)
                .map_or_else(|| format!("shader {}", pixel.shader.index()), |n| n.0.clone());
            *none.entry(name).or_default() += 1;
        }
    }
    Ok((with_fields, none))
}

/// Each bound material's `FC_TextureTilingScale` rows (byte offset, first two lanes), by material entity and MTD.
fn tilings(world: &World) -> Vec<String> {
    let mut out = Vec::new();
    for (e, m) in world.iter::<Material>() {
        let Some(pixel) = world.get::<ShaderBinding>(e).and_then(|b| b.stages.last()) else {
            continue;
        };
        let rows: Vec<String> = pixel
            .constants
            .iter()
            .filter(|c| c.name.as_deref() == Some("FC_TextureTilingScale"))
            .filter_map(|c| match &c.role {
                ConstantRole::Material(w) if w.len() >= 2 => Some(format!(
                    "{}:{}x{}",
                    c.offset / 16,
                    f32::from_bits(w[0]),
                    f32::from_bits(w[1])
                )),
                _ => None,
            })
            .collect();
        out.push(format!("e{} {} {}", e.index(), m.mtd, rows.join(" ")));
    }
    out
}

#[test]
fn every_program_with_material_fields_binds_the_materials_parameters() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut reads: Vec<(&str, World)> = Vec::new();
    if let Some(root) = inputs::install(inputs::Game::Sekiro) {
        let mut world = World::default();
        sekiro::read_parts(&root, &WOLF, &decoder, &mut world)?;
        reads.push(("Sekiro Wolf (default outfit)", world));
    }
    if let (Some(root), Some(keys)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
    ) {
        let install = Install::new(root, Game::EldenRing, Keys::from_folder(keys, Game::EldenRing)?);
        let mut world = World::default();
        sekiro::read_character_with(
            &install,
            "c3000",
            &decoder,
            CharacterOptions::default(),
            &mut world,
        )?;
        reads.push(("Elden Ring c3000", world));
    }
    let mut failures = Vec::new();
    for (what, mut world) in reads {
        let report = common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
        println!("{what}: {:?}", report.steps[0].log);
        let lines = tilings(&world);
        for line in &lines {
            println!("tiling {what}: {line}");
        }
        if what.starts_with("Sekiro") {
            // the scarf's program reads registers 0, 1, 2, 7, 8, 10 and 11: the capture's rows for them
            let scarf = " P_BD_M_9040_tops1_Maffler 0:18x6 1:1x1 2:1x1 7:51x17 8:1x1 10:51x17 11:18x6";
            if !lines.iter().any(|l| l.ends_with(scarf)) {
                failures.push(format!("{what}: the scarf's rows are not the capture's{scarf}"));
            }
        } else {
            let scaled: Vec<String> = world
                .iter::<Material>()
                .flat_map(|(_, m)| {
                    m.bindings
                        .iter()
                        .filter(|(_, b)| b.scale != [1.; 2])
                        .map(|(k, b)| format!("{} {k} {:?}", m.mtd, b.scale))
                        .collect::<Vec<_>>()
                })
                .collect();
            if !scaled.is_empty() {
                failures.push(format!("{what}: FLVER texture scales other than 1: {scaled:?}"));
            }
        }
        let (with_fields, none) = census(&world)?;
        println!(
            "{what}: {with_fields} bound programs with material fields, {} binding none: {none:?}",
            none.values().sum::<usize>()
        );
        if with_fields == 0 {
            failures.push(format!("{what}: no bound program has material fields"));
        }
        if !none.is_empty() {
            failures.push(format!(
                "{what}: programs binding none of the material's parameters: {none:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}
