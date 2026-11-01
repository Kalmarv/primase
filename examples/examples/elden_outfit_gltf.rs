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
use primase_fromsoft::sekiro::passes::bake_materials::Cache;
use primase_fromsoft::sekiro::program::{CharacterPreparation, Materials};
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

/// The lowering of the selected character, as primase's FromSoft preparation program states it, with this program's
/// choices: the materials keep their layers (`Preserve`) and are not resolved for the Game shader view beyond what the
/// bake needs, water without a texture is `water_colour` (sRGB bytes, a linear alpha byte), an outfit of `parts` parts
/// is assembled when there is more than one, the selected outfit's display masks are resolved (`dressing`), the
/// entities are named, no form of a part is selected out, no animation is bound, no weapon is held, and the material
/// bake stores nothing in a memo store (`Cache::Off`).
fn preparation(parts: usize) -> CharacterPreparation {
    CharacterPreparation {
        game: Game::EldenRing,
        materials: Materials::Preserve,
        game_shaders: false,
        water_colour: [48, 98, 122, 190],
        parts,
        select_assets: Vec::new(),
        animations: None,
        dressing: true,
        entity_names: true,
        held: None,
        bake_cache: Cache::Off,
    }
}

/// What the glTF writer needs of the world, in order: one form of every part (the one the source names, else the
/// finest; the writer chooses none and leaves a part with several out), skin weights that sum to 1 per vertex, unit
/// normals, a texture coordinate set on every primitive (zeros where one lacks it), a base colour of light grey, a metallic factor of 0 and a roughness of 0.85 for a
/// material that states none, collision as triangles, and every mirrored or sheared placement baked into geometry.
const WRITER: &[&str] = &[
    "select-lod{form=default}",
    "normalize-skin-weights",
    "normalize-normals",
    "fill-missing-texcoords",
    "prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}",
    "tessellate-colliders",
    "bake-nonrigid{mirrors=bake}",
];

/// The lowering and the writer's preparation of an outfit of `parts` parts.
fn lowering(parts: usize) -> Vec<String> {
    let writer = WRITER.iter().map(|text| (*text).to_owned());
    preparation(parts).texts().into_iter().chain(writer).collect()
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

type Error = Box<dyn std::error::Error + Send + Sync>;

/// The writer's options: the scene named after the character.
fn options() -> primase_gltf::WriteOptions {
    primase_gltf::WriteOptions { scene_name: Some(CHARACTER.to_owned()), ..Default::default() }
}

fn main() -> Result<(), Error> {
    let out = std::env::args().nth(1).ok_or("usage: elden_outfit_gltf <output.glb>")?;
    let root = inputs::install(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` install")?;
    let keys = inputs::keys(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` keys")?;
    let world = outfit(&root, &keys)?;
    let report = primase_gltf::write_file(&world, &options(), Path::new(&out))?;
    println!("glTF {out}:\n{report}");
    Ok(())
}

/// The outfit read from the install at `root` with the archive keys in `keys`, selected and lowered: the world the
/// program writes.
fn outfit(root: &Path, keys: &Path) -> Result<primase_core::World, Error> {
    let game = Game::EldenRing;
    let install = Install::new(root, game, Keys::from_folder(keys, game, game.param_key_sha256())?)
        .with_threads(std::thread::available_parallelism()?);
    let oodle = sekiro::decoder::load_installed(root)?;
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
    Ok(world)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stated pass text names a registered pass with the options it declares, and the order of the whole
    /// pipeline is one the passes' own declarations accept.
    #[test]
    fn the_stated_pipelines_parse_and_order() -> Result<(), Error> {
        let manager = manager();
        manager.registry.parse(&SELECT.join(", "))?.check()?;
        for parts in [1, 2] {
            manager.registry.parse(&lowering(parts).join(", "))?.check()?;
        }
        Ok(())
    }

    /// The written outfit reads back with its whole rig: the writer writes one rig of 238 joint nodes, and the glTF
    /// reader reads 238 joints (the unweighted ones too, each with its parent), the rig node as the rig, and every
    /// node as something. Skips without the `elden` install and keys.
    #[test]
    fn the_written_outfit_reads_back_its_whole_rig() -> Result<(), Error> {
        let (Some(root), Some(keys)) = (inputs::install(inputs::Game::Elden), inputs::keys(inputs::Game::Elden)) else {
            eprintln!("skipped: GAME_DATA states no `elden` install and keys");
            return Ok(());
        };
        let world = outfit(&root, &keys)?;
        let (glb, written) = primase_gltf::write(&world, &options())?;
        assert_eq!((written.rigs, written.joint_nodes), (1, 238));
        let mut read = primase_core::World::new();
        let report = primase_gltf::read_glb(&glb, "elden_outfit_gltf.glb", &mut read)?;
        assert_eq!((report.skeletons, report.joints, report.rig_nodes, report.nodes_not_read), (1, 238, 1, 0));
        // each joint by name with its parent's, as written
        let joints = |w: &primase_core::World| -> Vec<(String, Option<String>)> {
            let mut out: Vec<_> = w
                .iter::<primase_core::RigInstance>()
                .filter_map(|(_, r)| w.get::<primase_core::Skeleton>(r.skeleton))
                .flat_map(|sk| {
                    let name = |id| sk.joints.iter().find(|j| j.id == id).map(|j| j.name.clone());
                    sk.joints.iter().map(move |j| (j.name.clone(), j.parent.and_then(name)))
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(joints(&read), joints(&world));
        println!(
            "read back: {} joints, {} rig node, {} nodes not read",
            report.joints, report.rig_nodes, report.nodes_not_read
        );
        Ok(())
    }
}
