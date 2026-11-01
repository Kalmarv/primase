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
use primase_animation::simulation_schedule::{ConfigureSimulations, SimulationSchedule};
use primase_animation::{AssembleCharacter, Correspondence, ReferencePose, UnweightedRests, WeightedRests};
use primase_core::pass::{MissingInputs, PassManager, PassRegistry, RunReport};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::passes::bake_materials::BakeFromSoftMaterials;
use primase_fromsoft::sekiro::passes::object_collision::{ConvexBodies, LowerFromSoftPartCollision, MotionSelection};
use primase_fromsoft::sekiro::passes::pbr::Layers;
use primase_fromsoft::sekiro::passes::settings::{Metres, PixelCap, Seconds};
use primase_fromsoft::sekiro::passes::{
    Cards, Degenerates, LowerFromSoftGeometry, LowerFromSoftMaterials, Pose, ResolveFromSoftShaders, SpeedTreeFrame,
    Tiling, Unbound, Winding,
};
use primase_fromsoft::sekiro::program::{CharacterPreparation, Materials};
use primase_fromsoft::sekiro::standalone::{
    CharacterRole, InventoryRequest, SelectedModelRequest, StandaloneAsset, read_inventory, read_selected_models,
};
use primase_image::Srgba8;

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

/// The lowering of the selected character: the passes primase's FromSoft preparation program runs, in its order, each
/// as this program states it.
///
/// - The parts are skinned: the geometry keeps their stored bind pose (`pose=stored`, the reader's skeleton and rig
///   bindings being what the meshes draw through), their winding, degenerate triangles and UVs as stored (the surface
///   bake reads the authored UVs), leaves nothing out, draws blocker cards as any mesh, and frames a SpeedTree mesh
///   with the members its program reads.
/// - Each material's program is resolved, a texture slot with no texture reading a stated preview colour; the
///   materials keep their layers as source data (`Preserve`), water without a texture `48,98,122,190` (sRGB bytes, a
///   linear alpha byte); the surface bake makes images of at most 2048 pixels at time 0, storing nothing in a memo
///   store.
/// - A part's own collision (a weapon's) is lowered onto it, convex bodies as their rounded surface within 1 cm, every
///   stored motion type at its stored pose.
/// - An outfit of `parts` parts is assembled when there is more than one: joints correspond by name, each part's skins
///   are moved onto the assembled skeleton at the assembled rest, rest disagreements up to a millimetre are taken
///   (0.1 mm in model space for a joint a reference reparents), one a part does not weight is reported, not refused. A
///   weighted joint that disagrees by more keeps the part's own inverse bind (`weighted-rests=report`): the game poses
///   a part through its own inverse binds (Sekiro's Ashina's Memory costume stands its face's Head 4.7e-3 from the
///   other parts', 0.27 degrees, and the Wolf's default body stands its Head 4.675e-3 from the face's), so such a part
///   is drawn off its stored rest as the game draws it, every such joint and the displacement in the report as
///   approximations.
/// - Simulations step at 30 Hz, with 30 steps of pre-roll and at most 8 steps a frame.
/// - The selected outfit's display masks are resolved (`dressing`) and the entities named; no form of a part is
///   selected out, no animation is bound, no weapon is held.
fn preparation(parts: usize) -> Result<CharacterPreparation, Error> {
    Ok(CharacterPreparation {
        game: Game::EldenRing,
        materials: Materials::Preserve,
        parts,
        select_assets: Vec::new(),
        animations: None,
        dressing: true,
        entity_names: true,
        held: None,
        geometry: LowerFromSoftGeometry {
            pose: Pose::Stored,
            uv_tiling: Tiling::Keep,
            winding: Winding::Stored,
            degenerates: Degenerates::Keep,
            leave_out: Default::default(),
            blocker_cards: Cards::Draw,
            speedtree_frame: SpeedTreeFrame::Program,
        },
        resolve_shaders: ResolveFromSoftShaders::new(Unbound::Preview),
        lower_materials: LowerFromSoftMaterials::new(Some(Layers::Preserve), Srgba8([48, 98, 122, 190])),
        material_bake: BakeFromSoftMaterials::new(PixelCap::new(2048)?, Seconds::new(0.)?).with_cache(false),
        part_collision: LowerFromSoftPartCollision {
            convex: ConvexBodies::Rounded(Metres::new(0.01)?),
            motion: MotionSelection::All,
        },
        assembly: AssembleCharacter {
            correspondence: Correspondence::ByName,
            reference_pose: ReferencePose::Assembled,
            tolerance: 0.001,
            reference_tolerance: 0.0001,
            unweighted_rests: UnweightedRests::Report,
            weighted_rests: WeightedRests::Report,
            name: "assembled".into(),
            leave_out: Vec::new(),
        },
        simulations: ConfigureSimulations(SimulationSchedule::new(1. / 30., 30, 8)?),
    })
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
fn lowering(parts: usize) -> Result<Vec<String>, Error> {
    let writer = WRITER.iter().map(|text| (*text).to_owned());
    Ok(preparation(parts)?.texts()?.into_iter().chain(writer).collect())
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
    let asset = StandaloneAsset::Character { id: CHARACTER.to_owned(), role: CharacterRole::Player };
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
    let pipeline = manager.registry.parse(&lowering(parts)?.join(", "))?;
    let mut lowered = RunReport::default();
    manager.run(&mut world, &pipeline, &mut lowered)?;
    println!("{lowered}");
    Ok(world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::pass_text;

    /// Every stated pass text names a registered pass with the options it declares, and the order of the whole
    /// pipeline is one the passes' own declarations accept.
    #[test]
    fn the_stated_pipelines_parse_and_order() -> Result<(), Error> {
        let manager = manager();
        manager.registry.parse(&SELECT.join(", "))?.check()?;
        for parts in [1, 2] {
            manager.registry.parse(&lowering(parts)?.join(", "))?.check()?;
        }
        Ok(())
    }

    /// The lowering of an outfit of several parts, pass by pass as each pass states itself: the passes and values
    /// below, the writer's preparation after them.
    #[test]
    fn the_lowering_states_these_passes() -> Result<(), Error> {
        let stated = [
            "lower-fromsoft-geometry{pose=stored,uv-tiling=keep,winding=stored,degenerates=keep,leave-out=,blocker-cards=draw,speedtree-frame=program}",
            "lower-fromsoft-selected-outfit",
            "resolve-fromsoft-shaders{unbound-textures=preview}",
            "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
            "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=off}",
            "lower-fromsoft-part-collision{convex=rounded:0.01,motion=all}",
            "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=report,weighted-rests=report,name=assembled}",
            "configure-simulations{step-seconds=0.03333333333333333,pre-roll=30,max-steps-per-frame=8}",
            "lower-fromsoft-display-masks",
            "lower-fromsoft-material-events{game=elden}",
            "name-fromsoft-entities{game=elden}",
            "place-assets",
        ];
        let registry = registry();
        let canonical = |texts: &[String]| -> Result<Vec<String>, Error> {
            Ok(registry.parse(&texts.join(", "))?.passes.iter().map(|p| pass_text(p.as_ref())).collect())
        };
        let mut expected: Vec<String> = stated.iter().map(|t| (*t).to_owned()).collect();
        expected.extend(WRITER.iter().map(|t| (*t).to_owned()));
        assert_eq!(canonical(&lowering(4)?)?, canonical(&expected)?);
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
