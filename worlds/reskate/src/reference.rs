//! The reference build's (`scene-skate build`) command-line choices as passes a program runs before the map writer:
//! its level of detail (`--lod`) and its optional content (`--max-priority`), with the scene's warnings as the reference
//! reports them. I/O-free: the writer is [`crate::write::map`].

use std::path::Path;

use primase_core::World;

use crate::assembly::scene_warnings;
use crate::scene::Scene;
use crate::{Error, Result};

/// A level of detail as the reference build names it (`--lod`): `default` (the scene's `lod_default`), `highest`
/// (the finest) or a level N (the coarsest form at N or finer).
pub fn lod_choice(s: &str) -> Result<primase_core::LodChoice> {
    use primase_core::LodChoice;
    Ok(match s {
        "default" => LodChoice::Default,
        "highest" => LodChoice::Finest,
        n => LodChoice::AtMost(
            n.parse()
                .map_err(|_| Error::Scene(format!("--lod '{n}' is not default, highest or a level 0-255")))?,
        ),
    })
}

/// The reference build's `--lod` as a pass, run before [`write_map`] (which refuses a world with levels of detail):
/// each prototype keeps the form `lod` takes (`--lod`, default `default`).
pub fn select_lod(world: &mut World, lod: primase_core::LodChoice) -> Result<primase_core::pass::RunReport> {
    run_passes(world, vec![Box::new(primase_core::SelectLod { choice: lod })])
}

/// The reference build's `--max-priority N` as a pass, run last before [`write_map`]: the scene's warnings as the
/// reference reports them (from the whole scene, before its writer leaves placements out), for
/// [`EncodingOptions::scene_warnings`], then `drop-optional{max=N}` (nothing left out without N). The scene is read
/// with its files loaded (`interchange::load`): the warnings look at the meshes, and textures need pixels.
pub fn leave_out_optional(
    world: &mut World,
    dir: &Path,
    max_priority: Option<u8>,
) -> Result<(Vec<String>, primase_core::pass::RunReport)> {
    let warnings = scene_warnings(&Scene::from_world(world, dir)?, world)?;
    let passes: Vec<Box<dyn primase_core::pass::Pass>> = match max_priority {
        Some(max) => vec![Box::new(primase_core::DropOptional { max })],
        None => Vec::new(),
    };
    Ok((warnings, run_passes(world, passes)?))
}

fn run_passes(
    world: &mut World,
    passes: Vec<Box<dyn primase_core::pass::Pass>>,
) -> Result<primase_core::pass::RunReport> {
    let mut report = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new())
        .run(world, &primase_core::pass::Pipeline { passes }, &mut report)
        .map_err(|e| Error::Scene(e.to_string()))?;
    Ok(report)
}
