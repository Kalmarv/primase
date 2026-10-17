//! A skate-scene v1 folder -> one GLB, for Blender: the skate-scene reader, an explicit pass pipeline, the glTF
//! writer.
//!
//! `cargo run --release -p primase-gltf --example scene_to_glb -- <scene folder> <out.glb> [prototype|tile|none]
//! [pipeline]`
//!
//! The default pipeline takes each prototype's finest level of detail, loads the prototypes' GLBs and the texture
//! files, lowers `skate.material` to `pbr.material` (keeping the skate material, which goes to extras) and converts
//! normal maps to glTF's convention:
//! `select-lod{form=finest}, load-mesh-files, load-textures, lower-skate-pbr, normalize-normals`. The world is verified after the reader and
//! after every pass; the run report lists what each pass did and approximated.

use std::path::PathBuf;

use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_gltf::{GroupBy, WriteOptions, write_file};
use primase_image::{LoadTextures, NormalizeNormals};
use primase_skate_scene::SkateScene;

type BoxError = Box<dyn std::error::Error>;

const DEFAULT_PIPELINE: &str =
    "select-lod{form=finest}, load-mesh-files, load-textures, lower-skate-pbr, normalize-normals";

fn registry() -> PassRegistry {
    let mut reg = PassRegistry::new();
    primase_gltf::register(&mut reg);
    primase_skate_scene::lower::register(&mut reg);
    reg.register("select-lod", primase_core::select_lod);
    reg.register("drop-optional", primase_core::drop_optional);
    reg.register("load-textures", |o| {
        let mut p = LoadTextures::default();
        for (k, v) in o {
            match k.as_str() {
                "base_dir" => p.base_dir = PathBuf::from(v),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Box::new(p))
    });
    reg.register("normalize-normals", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(NormalizeNormals)),
    });
    reg
}

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: scene_to_glb <scene folder> <out.glb> [prototype|tile|none] [pipeline]".into());
    }
    let folder = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    let group_by = match args.get(3).map(String::as_str) {
        None | Some("prototype") => GroupBy::Prototype,
        Some("tile") => GroupBy::Tile,
        Some("none") => GroupBy::None,
        Some(other) => return Err(format!("unknown grouping {other}").into()),
    };
    let t0 = std::time::Instant::now();
    let mut world = World::new();
    let read = primase_skate_scene::read(&folder, &mut world)?;
    let scene = world.resource::<SkateScene>().ok_or("no scene")?;
    let scene_name = scene.display_name.clone().unwrap_or_else(|| scene.name.clone());

    let manager = PassManager::new(registry());
    manager.verify(&world, "the reader")?;
    let pipeline = manager
        .registry
        .parse(args.get(4).map_or(DEFAULT_PIPELINE, String::as_str))?;
    let mut report = RunReport {
        target: "gltf".into(),
        pipeline: pipeline.to_string(),
        ..RunReport::default()
    };
    manager.run(&mut world, &pipeline, &mut report)?;

    let options = WriteOptions {
        group_by,
        base_dir: folder.clone(),
        include_unplaced: false,
        scene_name: Some(scene_name),
    };
    let written = write_file(&world, &options, &out)?;
    println!(
        "read: {} entities, {} warnings, tolerated {:?}",
        world.entity_count(),
        read.warnings.len(),
        read.tolerated
    );
    print!("{report}");
    println!("{written}");
    println!("wrote {} in {:.1} s", out.display(), t0.elapsed().as_secs_f64());
    Ok(())
}
