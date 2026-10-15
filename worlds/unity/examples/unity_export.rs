//! A Unity level end to end: read, lowered, written as one GLB and (with a surface map) as a skate scene.
//!
//! Usage: `unity_export <Game>_Data <type tree package> <level> <output folder> [<surface map JSON>]`. The surface
//! map is an object of the game's surface names (its physic materials, `default` for none) onto skate surfaces; the
//! skate scene is written only with one. Writes `<output>/<level>.glb`, and `<output>/skate/assets` with
//! `<output>/skate/scene`; prints every pass's counts and approximations.

use std::collections::BTreeMap;
use std::path::Path;

use primase_core::World;
use primase_core::pass::PassLog;
use primase_unity::TypeTreePackage;
use primase_unity::level::read_level;
use primase_unity::player::Player;

fn show(pass: &str, log: &PassLog) {
    println!("{pass}: {:?}", log.counts);
    if !log.approximations.is_empty() {
        println!("{pass} approximates: {:?}", log.approximations);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if !(5..=6).contains(&args.len()) {
        return Err(
            "usage: unity_export <Game>_Data <type tree package> <level> <output folder> [<surface map>]"
                .into(),
        );
    }
    let (data, tpk, level, out) = (
        Path::new(&args[1]),
        Path::new(&args[2]),
        &args[3],
        Path::new(&args[4]),
    );
    let player = Player::open(data, TypeTreePackage::parse(&std::fs::read(tpk)?)?)?;
    let mut w = World::new();
    let started = std::time::Instant::now();
    let report = read_level(&player, level, &mut w)?;
    println!(
        "read {level}: {} renderers ({} left out), {} tree renderers, {} prototypes, {} primitives, {} materials, {} \
         textures, {} terrains, {} lights, {} colliders ({} by trees)",
        report.renderers,
        report.left_out,
        report.tree_renderers,
        report.prototypes,
        report.primitives,
        report.materials,
        report.textures,
        report.terrains,
        report.lights,
        report.colliders,
        report.tree_colliders
    );
    println!(
        "not decoded {:?}; not reproduced {:?}",
        report.textures_not_decoded, report.not_reproduced
    );
    show("lower-unity-pbr", &primase_unity::lower::lower_unity_pbr(&mut w)?);
    show(
        "lower-unity-terrain",
        &primase_unity::lower_terrain::lower_unity_terrain(&mut w)?,
    );
    show(
        "lower-unity-collision",
        &primase_unity::lower_collision::lower_unity_collision(&mut w)?,
    );
    show(
        "lower-unity-lights",
        &primase_unity::lower_lights::lower_unity_lights(&mut w)?,
    );
    show("normalize-normals", &primase_image::normalize_normals(&mut w)?);
    let problems = w.verify_components();
    if let Some((name, e, detail)) = problems.first() {
        return Err(format!(
            "{} invalid components; first {name} on {e:?}: {detail}",
            problems.len()
        )
        .into());
    }
    println!("lowered in {:.1} s", started.elapsed().as_secs_f64());
    std::fs::create_dir_all(out)?;
    let glb = out.join(format!("{level}.glb"));
    let r = primase_gltf::write_file(
        &w,
        &primase_gltf::WriteOptions {
            scene_name: Some(level.clone()),
            ..Default::default()
        },
        &glb,
    )?;
    println!("glTF {}: {r:?}", glb.display());
    let Some(map) = args.get(5) else { return Ok(()) };
    let surfaces: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(map)?)?;
    show(
        "bake-texture-transforms",
        &primase_meshops::bake_texture_transforms(&mut w),
    );
    show(
        "raise-skate",
        &primase_skate_scene::raise_skate(
            &mut w,
            &primase_skate_scene::RaiseOptions {
                name: level.replace(
                    |c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "_",
                ),
                surfaces,
            },
        )?,
    );
    primase_skate_scene::prepare_scene_assets(&mut w, &out.join("skate").join("assets"))?;
    let r = primase_skate_scene::write(&w, &out.join("skate").join("scene"))?;
    println!("skate scene: {r:?}");
    Ok(())
}
