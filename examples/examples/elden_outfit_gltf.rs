//! An Elden Ring starting outfit (the player's, initializer row 3002, the female presentation) read, dressed and
//! written as one glTF binary.
//!
//! Usage: `elden_outfit_gltf <output.glb>`. The install and the archive keys come from the `GAME_DATA` configuration
//! (`tests/inputs/mod.rs`, `elden`). Prints what the selection, the lowering and the writer's preparation did, and what
//! the writer left out.
//!
//! The program reads in three steps, each one a choice stated below: the candidate outfits (nothing is chosen by the
//! reader), the registered selection passes that choose among them, and the selected model scope, which is then lowered
//! by registered passes and written. Every option a pass takes is written out; a pass that names no default has none
//! here.
use std::path::Path;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Install, Keys};
use primase_core::pass::{MissingInputs, PassManager, PassRegistry, RunReport};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::standalone::{
    InventoryRequest, SelectedModelRequest, StandaloneAsset, read_inventory, read_selected_models,
};

#[path = "../tests/inputs/mod.rs"]
mod inputs;

/// The model whose outfit is dressed: the player's.
const CHARACTER: &str = "c0000";

/// The outfit selection. `initializer`, `presentation` and `level` list the candidate variants (initializer row 3002,
/// the female presentation; `prefer-low` takes a part's low-detail binder where the install holds both), and `outfit`
/// names the listed variant to dress. No NPC row, no NPC initializer: the player has neither.
const SELECT: &[&str] = &[
    "select-fromsoft-initializer-outfits{initializer=3002,presentation=f,level=prefer-low}",
    "select-fromsoft-npc-outfits{level=prefer-low}",
    "select-fromsoft-character-read{outfit=initializer/3002/f,initializer=none}",
    "select-fromsoft-dressing{npc=none,empty=show}",
];

/// The lowering of the selected character, first part: geometry as stored (a skinned model keeps its bind pose,
/// windings and UVs), the selected outfit's display masks resolved, the materials with their layers preserved and the
/// shader program of each bound and baked at 2048 px, and the parts' collision.
const LOWER_PARTS: &[&str] = &[
    "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
    "lower-fromsoft-selected-outfit",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
    "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    "bake-fromsoft-materials{max-size=2048,sample-time=0}",
    "lower-fromsoft-part-collision{convex=rounded:0.01,motion=all}",
];

/// An outfit of several parts, each carrying a skeleton of its own, is related into one: joints correspond by name,
/// each part's skins move onto the assembled skeleton at the assembled rest, rest disagreements up to a millimetre are
/// taken, and a joint a part does not weight is reported, not refused. A single part has nothing to relate.
const ASSEMBLE: &str = "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled}";

/// The lowering of the selected character, second part: the simulation schedule (a 30 Hz step, 30 steps of pre-roll,
/// at most 8 steps per frame), the display masks, the material events, the entities' names, and the asset placed at the
/// origin.
const LOWER_PLACED: &[&str] = &[
    "configure-simulations{step-seconds=0.03333333333333333,pre-roll=30,max-steps-per-frame=8}",
    "lower-fromsoft-display-masks",
    "lower-fromsoft-material-events{game=elden}",
    "name-fromsoft-entities{game=elden}",
    "place-assets",
];

/// What the glTF writer needs of the world, in order: unit normals, a texture coordinate set on every primitive
/// (zeros where one lacks it), a base colour of light grey, a metallic factor of 0 and a roughness of 0.85 for a
/// material that states none, collision as triangles, and every mirrored or sheared placement baked into geometry.
const WRITER: &[&str] = &[
    "normalize-normals",
    "fill-missing-texcoords",
    "prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}",
    "tessellate-colliders",
    "bake-nonrigid{mirrors=bake}",
];

/// The lowering and the writer's preparation of an outfit of `parts` parts.
fn lowering(parts: usize) -> Vec<&'static str> {
    let assemble: &[&str] = if parts > 1 { &[ASSEMBLE] } else { &[] };
    [LOWER_PARTS, assemble, LOWER_PLACED, WRITER].concat()
}

fn registry() -> PassRegistry {
    let mut registry = PassRegistry::new();
    primase_core::register(&mut registry);
    primase_meshops::register(&mut registry);
    primase_image::register(&mut registry);
    primase_animation::register(&mut registry);
    primase_gltf::register(&mut registry);
    sekiro::passes::register(&mut registry);
    registry
}

/// A step the world cannot feed (a pass whose input the model lacks) is skipped and reported, never refused.
fn manager() -> PassManager {
    PassManager::new(registry()).with_missing_inputs(MissingInputs::Skip)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let out = std::env::args().nth(1).ok_or("usage: elden_outfit_gltf <output.glb>")?;
    let root = inputs::install(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` install")?;
    let keys = inputs::keys(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` keys")?;

    let game = Game::EldenRing;
    let install = Install::new(&root, game, Keys::from_folder(keys, game, game.param_key_sha256())?)
        .with_threads(std::thread::available_parallelism()?);
    let oodle = sekiro::decoder::load_installed(&root)?;
    let decoder = || Decoder { oodle: Some(oodle), max_output: 1 << 30 };
    let manager = manager();

    // The candidates: every variant the install lists for the stated initializer, with no choice made by the reader.
    let asset = StandaloneAsset::Character(CHARACTER.to_owned());
    let mut candidates = read_inventory(
        &install,
        decoder(),
        InventoryRequest { asset: &asset, names: None, npc: None, weapons: false, save: None, attack_of: None },
    )?;

    // The registered selection passes choose among them.
    let selection = manager.registry.parse(&SELECT.join(", "))?;
    let mut selected = RunReport::default();
    manager.run(&mut candidates, &selection, &mut selected)?;
    println!("{selected}");

    // The selected scope is read, with no animations, weapons or effects: none were asked for.
    let read = read_selected_models(
        install,
        decoder(),
        &candidates,
        SelectedModelRequest { asset, animations: None, weapons: Default::default(), grip: None, clip_effects: false },
    )?;
    for notice in &read.models.notices {
        println!("notice: {notice}");
    }
    let parts = read.models.dressed_parts.len();
    let mut world = read.models.world;

    // The lowering and the writer's preparation, as registered passes.
    let pipeline = manager.registry.parse(&lowering(parts).join(", "))?;
    let mut lowered = RunReport::default();
    manager.run(&mut world, &pipeline, &mut lowered)?;
    println!("{lowered}");

    let options = primase_gltf::WriteOptions { scene_name: Some(CHARACTER.to_owned()), ..Default::default() };
    let report = primase_gltf::write_file(&world, &options, Path::new(&out))?;
    println!("glTF {out}:\n{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stated pass text names a registered pass with the options it declares, and the order of the whole
    /// pipeline is one the passes' own declarations accept.
    #[test]
    fn the_stated_pipelines_parse_and_order() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let manager = manager();
        manager.registry.parse(&SELECT.join(", "))?.check()?;
        for parts in [1, 2] {
            manager.registry.parse(&lowering(parts).join(", "))?.check()?;
        }
        Ok(())
    }
}
