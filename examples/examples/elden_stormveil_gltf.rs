//! A slice of Stormveil Castle (Elden Ring's `m10_00_00_00`) read, lowered at one hour of the day and written as one
//! glTF binary.
//!
//! Usage: `elden_stormveil_gltf <output.glb>`. The install and the archive keys come from the `GAME_DATA` configuration
//! (`tests/inputs/mod.rs`, `elden`). Prints what the selection, the lowering and the writer's preparation did, and what
//! the writer left out, and writes what it ran beside the output (`<output>.preparation.txt`, [`Stages`]): the
//! selection, the read, the lowering and the writer's preparation, each stage's texts one per line as each states
//! itself, for another program to read and prepare the same area the same way.
//!
//! The program reads in four steps: the area's candidates (its parts' positions and the draw-param files it holds, no
//! payload), the registered `select-fromsoft-area` pass that turns the stated region and environment into a load
//! scope, the area read at that scope, and the lowering by registered passes. The region, the environment file and the
//! hour of the day are this program's choices; so is every other option below, written out where the reader or a pass
//! takes it.
use std::path::Path;

use fromsoft_formats::dcx::Decoder;
use fromsoft_formats::{Game, Install, Keys};
use primase_core::convert::Reader;
use primase_core::pass::{
    MissingInputs, PassManager, PassRegistry, Pipeline, RunReport, Settings, Stages, pass_text, text_form,
};
use primase_core::{LodChoice, SelectLod};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::materials::SkipClass;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::area_plan::AreaSelection;
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::groups::DisplayGroups;
use primase_fromsoft::sekiro::passes::bake_materials::BakeFromSoftMaterials;
use primase_fromsoft::sekiro::passes::collision::{FMeshes, LowerFromSoftCollision};
use primase_fromsoft::sekiro::passes::draw_params::SampleFromSoftDrawParams;
use primase_fromsoft::sekiro::passes::environment::LowerFromSoftEnvironment;
use primase_fromsoft::sekiro::passes::lights::{Brightness, Disabled, LowerFromSoftLights};
use primase_fromsoft::sekiro::passes::object_collision::{
    ConvexBodies, ConvexSurface, LowerFromSoftObjectCollision, MotionSelection, ResolutionChoice,
};
use primase_fromsoft::sekiro::passes::pbr::Layers;
use primase_fromsoft::sekiro::passes::records::LowerFromSoftMapRecords;
use primase_fromsoft::sekiro::passes::settings::{Factor, Fraction, Metres, PixelCap, Seconds};
use primase_fromsoft::sekiro::passes::{
    Cards, Degenerates, LowerFromSoftGeometry, LowerFromSoftMaterials, Pose, ResolveFromSoftShaders, SpeedTreeFrame,
    Tiling, Unbound, Winding,
};
use primase_fromsoft::sekiro::program::{AreaGame, AreaPreparation};
use primase_fromsoft::sekiro::sky::Sky;
use primase_fromsoft::sekiro::source::StateReadScope;
use primase_fromsoft::sekiro::texture_switch::TextureQuality;
use primase_fromsoft::sekiro::{AreaRead, AreaReadText, Parents, StateClasses};
use primase_image::Srgba8;

#[path = "../tests/inputs/mod.rs"]
mod inputs;

/// Stormveil Castle's map.
const AREA: &str = "m10_00_00_00";

/// The slice: the ground rectangle `[west, south, east, north]` in metres of the area's frame (x east, z along the
/// ground).
const REGION: &str = "rect:-162:5:108:250";

/// The draw-param file whose graphics environment (sun, ambient, fog, sky, tone) the area is lit with: the file of
/// this stem, one the install holds for the map.
const ENVIRONMENT: &str = "file:m10_00_0000";

/// The hour of the game's clock (0 to 24) at which each time-of-day field of that environment is sampled: noon.
const HOUR: f32 = 12.0;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// The area's lowering: the passes primase's FromSoft preparation program runs, in its order, each as this program
/// states it.
///
/// - What the read took: the sky, lowered after the environment; the environment sampled at `HOUR`; no environment
///   probes, weather, irradiance or effects.
/// - Collision: the map's and the assets' triangles whose centre lies more than 50 m outside the read region are left
///   out, and the map's `f` binder is not lowered. An asset's collision is the high-resolution file where the game
///   stores one, else the low one every collidable asset has, no motion type left out (its stored motion types are not
///   mapped), each convex body its rounded surface (the core swept by the Havok radius) within 0.01 m rather than the
///   sharp intersection of its offset planes, with the surface of the asset's largest colliding render area.
/// - Geometry: the posed static models, UVs as stored (the surface bake reads the authored UVs), a part's winding
///   reversed where its stored normals oppose it, degenerate triangles dropped; light volumes, shadow proxies and
///   SpeedTree's billboard cards left out (a billboard's program turns it to the camera; the trees' own meshes are
///   drawn), the sky's meshes kept for the sky; blocker cards recorded, not drawn; a SpeedTree mesh framed by the
///   members its program reads. Every prototype keeps its finest level of detail.
/// - The environment's stored colours at 1 lux for a colour of 1: the display shows the sun as its key light, at an
///   intensity that is its own setting, so the scale is all the program states.
/// - Each material's program is resolved, a texture slot with no texture reading a stated preview colour; the
///   materials keep their layers as source data, water without a texture `48,98,122,190` (sRGB bytes, a linear alpha
///   byte); the surface bake makes images of at most 2048 pixels at time 0, storing nothing in a memo store.
/// - The lights at 1 candela per unit of stored power, a spot's inner cone at 0.85 of its outer (the game's spot has
///   none), an effect light at its settled brightness, every light reaching at least 0.5 m, the lights of regions that
///   start disabled left out.
/// - Every map record lowered: the display lists them all, and the markers' kinds tell them apart.
fn preparation() -> Result<AreaPreparation, Error> {
    let reach = Some(Metres::new(50.)?);
    Ok(AreaPreparation {
        game: AreaGame::EldenRing,
        sky: true,
        draw_params: Some(SampleFromSoftDrawParams::new(HOUR)?),
        environment_probes: false,
        weather: false,
        irradiance_package: None,
        irradiance_states: false,
        effects: None,
        collision: LowerFromSoftCollision { reach, f_meshes: FMeshes::Off },
        object_collision: LowerFromSoftObjectCollision {
            resolution: ResolutionChoice::HighElseLow,
            convex: ConvexBodies::Rounded(Metres::new(0.01)?),
            motion: MotionSelection::All,
            convex_surface: ConvexSurface::Render,
            reach,
        },
        geometry: LowerFromSoftGeometry {
            pose: Pose::Rigid,
            uv_tiling: Tiling::Keep,
            winding: Winding::Normals,
            degenerates: Degenerates::Drop,
            leave_out: [SkipClass::LightVolume, SkipClass::Shadow, SkipClass::Billboard].into(),
            blocker_cards: Cards::Record,
            speedtree_frame: SpeedTreeFrame::Program,
        },
        lod: SelectLod { choice: LodChoice::Finest },
        environment: LowerFromSoftEnvironment::new(Factor::new(1.)?),
        resolve_shaders: ResolveFromSoftShaders::new(Unbound::Preview),
        lower_materials: LowerFromSoftMaterials::new(Some(Layers::Preserve), Srgba8([48, 98, 122, 190])),
        material_bake: BakeFromSoftMaterials::new(PixelCap::new(2048)?, Seconds::new(0.)?).with_cache(false),
        lights: LowerFromSoftLights {
            candela_per_power: Factor::new(1.)?,
            spot_blend: Fraction::new(0.15)?,
            brightness: Brightness::Settled,
            min_range: Metres::new(0.5)?,
            include_disabled: Disabled::Skip,
        },
        map_records: LowerFromSoftMapRecords { leave_out: Default::default() },
    })
}

/// The area read, with the reader's choices beyond the selected region and environment file: a 30 m margin around the
/// slice, the map records and the render graph read, the sky included, and everything else off or at its highest
/// quality.
fn read() -> AreaReadText {
    AreaReadText {
        area: Some(AREA.to_owned()),
        read: AreaRead {
            margin: 30.0,
            state_inputs: StateReadScope { weather: false, common: false },
            states: StateClasses::None,
            display_groups: DisplayGroups::None,
            parents: Parents::None,
            environment_collision: None,
            object_animations: false,
            map_records: true,
            f_collision: false,
            effect_lights: false,
            sky: Sky::Include,
            render_graph: true,
            irradiance: false,
            effects: false,
            low_textures: false,
            texture_quality: TextureQuality::Max,
        },
    }
}

/// What the glTF writer needs of the world, in order: the textures nothing refers to left out (the source textures the
/// surface bake replaced: the writer writes what the materials bind), unit normals, a texture coordinate set on every
/// primitive (zeros where one lacks it), a base colour of light grey, a metallic factor of 0 and a roughness of 0.85
/// for a material that states none, collision as triangles, and every mirrored or sheared placement baked into
/// geometry.
const WRITER: &[&str] = &[
    "drop-unreferenced-textures",
    "normalize-normals",
    "fill-missing-texcoords",
    "prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}",
    "tessellate-colliders",
    "bake-nonrigid{mirrors=bake}",
];

/// The load scope pass, with the stated region and environment.
fn selection() -> String {
    format!("select-fromsoft-area{{region={REGION},environment={ENVIRONMENT}}}")
}

/// The lowering: the preparation program's passes.
fn lowering() -> Result<Vec<String>, Error> {
    Ok(preparation()?.texts()?)
}

/// The writer's preparation.
fn writer() -> Vec<String> {
    WRITER.iter().map(|text| (*text).to_owned()).collect()
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

/// A step the world cannot feed (a pass whose input the area lacks) is skipped and reported, never refused.
fn manager(settings: Settings) -> PassManager {
    PassManager::new(registry()).with_missing_inputs(MissingInputs::Skip).with_settings(settings)
}

/// Each pass of `pipeline` as it states itself.
fn texts(pipeline: &Pipeline) -> Vec<String> {
    pipeline.passes.iter().map(|pass| pass_text(pass.as_ref())).collect()
}

/// What the program ran, as stages: the selection, the read, the lowering and the writer's preparation.
fn stages(selection: &Pipeline, read: &AreaReadText, lowering: &Pipeline, writer: &Pipeline) -> Result<Stages, Error> {
    Ok(Stages::new()
        .with("selection", texts(selection))?
        .with("read", vec![text_form(read)])?
        .with("lowering", texts(lowering))?
        .with("writer", texts(writer))?)
}

fn main() -> Result<(), Error> {
    let out = std::env::args().nth(1).ok_or("usage: elden_stormveil_gltf <output.glb>")?;
    let root = inputs::install(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` install")?;
    let keys = inputs::keys(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` keys")?;

    let game = Game::EldenRing;
    let threads = std::thread::available_parallelism()?;
    let install =
        Install::new(&root, game, Keys::from_folder(keys, game, game.param_key_sha256())?).with_threads(threads);
    let oodle = sekiro::decoder::load_installed(&root)?;
    let decoder = Decoder { oodle: Some(oodle), max_output: 1 << 30 };
    // the passes run on the machine's threads, as the program states them (each step's report says them)
    let manager =
        manager(Settings { threads: primase_core::parallel::Threads::new(threads.get()).ok_or("no threads")? });

    // The candidates: where the area's drawn parts stand and which draw-param files it holds.
    let mut candidates = sekiro::read_area_candidates(&install.open()?, &decoder, game, AREA)?;

    // The registered selection pass turns the stated region and environment into a load scope.
    let selection = manager.registry.parse(&selection())?;
    let mut selected = RunReport::default();
    manager.run(&mut candidates, &selection, &mut selected)?;
    println!("{selected}");
    let scope = candidates.resource::<AreaSelection>().ok_or("select-fromsoft-area stated no load scope")?;

    // The area, read at that scope, with the read's stated choices.
    let read = read();
    let options = read.read.options(scope);
    let mut reader = AreaReader::of(install, AREA, decoder, options);
    let (mut world, _) = reader.read()?;
    scope.scope.record_in(&mut world);
    for notice in reader.notices() {
        println!("notice: {notice}");
    }
    let (mut world, _) = reader.load_all(world)?;
    for notice in reader.notices() {
        println!("notice: {notice}");
    }

    // The lowering, then the writer's preparation, as registered passes.
    let lowering = manager.registry.parse(&lowering()?.join(", "))?;
    let writer = manager.registry.parse(&writer().join(", "))?;
    let mut lowered = RunReport::default();
    manager.run(&mut world, &lowering, &mut lowered)?;
    manager.run(&mut world, &writer, &mut lowered)?;
    println!("{lowered}");
    let path = Path::new(&out).with_extension("preparation.txt");
    let comment = format!("What elden_stormveil_gltf ran to write {out}: its selection, read, lowering and writer.");
    std::fs::write(&path, stages(&selection, &read, &lowering, &writer)?.text(&comment))?;
    println!("preparation {}", path.display());

    let options = primase_gltf::WriteOptions {
        scene_name: Some(AREA.to_owned()),
        threads: primase_foundations::parallel::Threads::new(threads.get()).ok_or("no threads")?,
        ..Default::default()
    };
    let report = primase_gltf::write_file(&world, &options, Path::new(&out))?;
    println!("glTF {out}:\n{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stated pass text names a registered pass with the options it declares, and the order of the whole pipeline
    /// is one the passes' own declarations accept.
    #[test]
    fn the_stated_pipelines_parse_and_order() -> Result<(), Error> {
        let manager = manager(Settings { threads: primase_core::parallel::Threads::ONE });
        manager.registry.parse(&selection())?.check()?;
        manager.registry.parse(&[lowering()?, writer()].concat().join(", "))?.check()?;
        Ok(())
    }

    /// The stages the program writes read back as what it ran: the read's text as the read, every stage's texts as
    /// the passes it parsed.
    #[test]
    fn the_stages_read_back_as_what_ran() -> Result<(), Error> {
        let registry = registry();
        let (selection, lowering, writer) = (
            registry.parse(&selection())?,
            registry.parse(&lowering()?.join(", "))?,
            registry.parse(&writer().join(", "))?,
        );
        let written = stages(&selection, &read(), &lowering, &writer)?;
        let stages = Stages::parse(&written.text("a comment"))?;
        assert_eq!(stages, written);
        assert_eq!(stages.names().collect::<Vec<_>>(), ["selection", "read", "lowering", "writer"]);
        let [read_text] = stages.stage("read").ok_or("no read")? else { return Err("one read text".into()) };
        assert_eq!(primase_core::pass::parse_text_form::<AreaReadText>(read_text)?, read());
        for (name, pipeline) in [("selection", &selection), ("lowering", &lowering), ("writer", &writer)] {
            let again = registry.parse(&stages.stage(name).ok_or(name)?.join(", "))?;
            assert_eq!(texts(&again), texts(pipeline), "{name}");
        }
        Ok(())
    }

    /// The area's lowering, pass by pass as each pass states itself: the passes and values below.
    #[test]
    fn the_lowering_states_these_passes() -> Result<(), Error> {
        let stated = [
            "lower-fromsoft-render-graph",
            "lower-fromsoft-collision{reach=50}",
            "lower-fromsoft-object-collision{resolution=high-else-low,convex=rounded:0.01,motion=all,convex-surface=render,reach=50}",
            "fromsoft-apply-rigid-skinning",
            "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
            "select-lod{form=finest}",
            "sample-fromsoft-draw-params{hour=12}",
            "lower-fromsoft-environment{lux-per-unit=1}",
            "lower-elden-ring-reinhard",
            "resolve-fromsoft-shaders{unbound-textures=preview}",
            "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
            "bake-fromsoft-materials{max-size=2048,sample-time=0,cache=off}",
            "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,brightness=settled,min-range=0.5,include-disabled=off}",
            "lower-fromsoft-sky",
            "lower-fromsoft-extras",
            "lower-fromsoft-map-records{leave-out=}",
            "name-fromsoft-entities{game=elden}",
        ];
        let registry = registry();
        let canonical = |texts: &[String]| -> Result<Vec<String>, Error> {
            Ok(registry.parse(&texts.join(", "))?.passes.iter().map(|p| pass_text(p.as_ref())).collect())
        };
        let expected: Vec<String> = stated.iter().map(|t| (*t).to_owned()).collect();
        assert_eq!(canonical(&lowering()?)?, canonical(&expected)?);
        Ok(())
    }
}
