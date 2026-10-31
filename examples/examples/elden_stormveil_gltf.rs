//! A slice of Stormveil Castle (Elden Ring's `m10_00_00_00`) read, lowered at one hour of the day and written as one
//! glTF binary.
//!
//! Usage: `elden_stormveil_gltf <output.glb>`. The install and the archive keys come from the `GAME_DATA` configuration
//! (`tests/inputs/mod.rs`, `elden`). Prints what the selection, the lowering and the writer's preparation did, and what
//! the writer left out.
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
use primase_core::pass::{MissingInputs, PassManager, PassRegistry, RunReport};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::area_plan::AreaSelection;
use primase_fromsoft::sekiro::convert::AreaReader;

#[path = "../tests/inputs/mod.rs"]
mod inputs;

/// Stormveil Castle's map.
const AREA: &str = "m10_00_00_00";

/// The slice: the ground rectangle `[west, south, east, north]` in metres of the area's frame (x east, z along the
/// ground), and how far past it a placement's model may reach and still be kept.
const REGION: &str = "rect:-162:5:108:250";
const MARGIN: f32 = 30.0;

/// The draw-param file whose graphics environment (sun, ambient, fog, sky, tone) the area is lit with: the file of
/// this stem, one the install holds for the map.
const ENVIRONMENT: &str = "file:m10_00_0000";

/// The hour of the game's clock (0 to 24) at which each time-of-day field of that environment is sampled: noon.
const HOUR: &str = "12";

/// The area's lowering up to the environment: the frame the game draws (render graph), the area's collision cropped to
/// 50 m of the slice and its objects' collision (the high-resolution file where one is stored, else the low one; every
/// motion type; convex bodies rounded within 1 cm and drawn with the render surface), static meshes posed, the geometry
/// (UVs kept as authored, windings from the normals, degenerate triangles dropped; light volumes, shadow proxies and
/// billboard cards left out, blocker cards recorded; the sky is kept for `lower-fromsoft-sky`), and the finest level
/// of detail.
const LOWER_GEOMETRY: &[&str] = &[
    "lower-fromsoft-render-graph",
    "lower-fromsoft-collision{reach=50}",
    "lower-fromsoft-object-collision{resolution=high-else-low,convex=rounded:0.01,motion=all,convex-surface=render,reach=50}",
    "fromsoft-apply-rigid-skinning",
    "lower-fromsoft-geometry{pose=rigid,uv-tiling=keep,winding=normals,degenerates=drop,leave-out=light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}",
    "select-lod{form=finest}",
];

/// The rest of the lowering, after `sample-fromsoft-draw-params{hour=HOUR}`: the environment lowered with one lux for a
/// colour of 1, its tone map, the materials (layers preserved, the shader program of each bound and baked at 2048 px,
/// water of one colour where it has no texture), the lights (one candela for a unit of power, spot blend 0.15, a
/// range of at least 0.5 m), the sky, the extras, the map's regions and events (none left out) and the entities' names.
const LOWER_SURFACES: &[&str] = &[
    "lower-fromsoft-environment{lux-per-unit=1}",
    "lower-elden-ring-reinhard",
    "resolve-fromsoft-shaders{unbound-textures=preview}",
    "lower-fromsoft-materials{layers=preserve,water-colour=48,98,122,190}",
    "bake-fromsoft-materials{max-size=2048,sample-time=0}",
    "lower-fromsoft-lights{candela-per-power=1,spot-blend=0.15,min-range=0.5}",
    "lower-fromsoft-sky",
    "lower-fromsoft-extras",
    "lower-fromsoft-map-records{leave-out=}",
    "name-fromsoft-entities{game=elden}",
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

/// The load scope pass, with the stated region and environment.
fn selection() -> String {
    format!("select-fromsoft-area{{region={REGION},environment={ENVIRONMENT}}}")
}

/// The lowering and the writer's preparation, with the stated hour sampled before the environment is lowered.
fn lowering() -> Vec<String> {
    let hour = format!("sample-fromsoft-draw-params{{hour={HOUR}}}");
    [LOWER_GEOMETRY, &[hour.as_str()], LOWER_SURFACES, WRITER].concat().into_iter().map(str::to_owned).collect()
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
fn manager() -> PassManager {
    PassManager::new(registry()).with_missing_inputs(MissingInputs::Skip)
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let out = std::env::args().nth(1).ok_or("usage: elden_stormveil_gltf <output.glb>")?;
    let root = inputs::install(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` install")?;
    let keys = inputs::keys(inputs::Game::Elden).ok_or("GAME_DATA states no `elden` keys")?;

    let game = Game::EldenRing;
    let threads = std::thread::available_parallelism()?;
    let install =
        Install::new(&root, game, Keys::from_folder(keys, game, game.param_key_sha256())?).with_threads(threads);
    let oodle = sekiro::decoder::load_installed(&root)?;
    let decoder = Decoder { oodle: Some(oodle), max_output: 1 << 30 };
    let manager = manager();

    // The candidates: where the area's drawn parts stand and which draw-param files it holds.
    let mut candidates = sekiro::read_area_candidates(&install.open()?, &decoder, game, AREA)?;

    // The registered selection pass turns the stated region and environment into a load scope.
    let mut selected = RunReport::default();
    manager.run(&mut candidates, &manager.registry.parse(&selection())?, &mut selected)?;
    println!("{selected}");
    let scope = candidates.resource::<AreaSelection>().ok_or("select-fromsoft-area stated no load scope")?;

    // The area, read at that scope. Every option of the read is written out.
    let options = sekiro::Options {
        state_inputs: sekiro::source::StateReadScope { weather: false, common: false },
        region: scope.read_region(MARGIN),
        states: sekiro::StateClasses::None,
        display_groups: sekiro::groups::DisplayGroups::None,
        parents: sekiro::Parents::None,
        draw_param: scope.read_draw_params(),
        object_animations: false,
        map_records: true,
        f_collision: false,
        effect_lights: false,
        sky: sekiro::sky::Sky::Include,
        render_graph: true,
        irradiance: false,
        effects: false,
        low_textures: false,
        texture_quality: sekiro::texture_switch::TextureQuality::Max,
    };
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

    // The lowering and the writer's preparation, as registered passes.
    let pipeline = manager.registry.parse(&lowering().join(", "))?;
    let mut lowered = RunReport::default();
    manager.run(&mut world, &pipeline, &mut lowered)?;
    println!("{lowered}");

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
    fn the_stated_pipelines_parse_and_order() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let manager = manager();
        manager.registry.parse(&selection())?.check()?;
        manager.registry.parse(&lowering().join(", "))?.check()?;
        Ok(())
    }
}
