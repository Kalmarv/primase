//! A skate-scene v1 folder -> one GLB, for Blender: the skate-scene reader, an explicit pass pipeline, the glTF
//! writer.
//!
//! `cargo run --release -p primase-reskate --example scene_to_glb -- <scene folder> <out.glb> [prototype|tile|none]
//! [pipeline]`
//!
//! Each prototype's finest level of detail is selected (`select-lod{form=finest}`), then the reader's load step reads
//! the prototypes' GLBs and the texture files of what is left (`read::interchange::load`). The default pipeline then lowers
//! `skate.material` to `pbr.material` (keeping the skate material, which goes to extras) and converts normal maps to
//! glTF's convention: `lower-skate-pbr, normalize-normals, ...`. The world is verified after the reader and after
//! every pass; the run report lists what each pass did and approximated.

use std::path::PathBuf;

use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_gltf::{GroupBy, WriteOptions, write_file};
use primase_image::NormalizeNormals;
use primase_reskate::components::SkateScene;

type BoxError = Box<dyn std::error::Error>;

/// The skate scene's own rules for what it does not state are this program's choices: zeros for a texture coordinate
/// set a primitive lacks, and light grey, dielectric and rough for a material without PBR factors.
const DEFAULT_PIPELINE: &str = "lower-skate-pbr, normalize-normals, \
    fill-missing-texcoords, prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}, bake-nonrigid{mirrors=keep}";

fn registry() -> PassRegistry {
    let mut reg = PassRegistry::new();
    primase_gltf::register(&mut reg);
    primase_reskate::register(&mut reg);
    reg.register("select-lod", primase_core::select_lod);
    primase_meshops::register(&mut reg);
    reg.register("drop-optional", primase_core::drop_optional);
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
    let read = primase_reskate::read::interchange::read(&folder, &mut world)?;
    let scene = world.resource::<SkateScene>().ok_or("no scene")?;
    let scene_name = scene.display_name.clone().unwrap_or_else(|| scene.name.clone());

    let manager = PassManager::new(registry());
    manager.verify(&world, "the reader")?;
    // the finest form of each prototype, then only what is left loaded
    let select = manager.registry.parse("select-lod{form=finest}")?;
    let mut report = RunReport {
        target: "gltf".into(),
        ..RunReport::default()
    };
    manager.run(&mut world, &select, &mut report)?;
    primase_reskate::read::interchange::load(&folder, &mut world)?;
    manager.verify(&world, "the reader's load step")?;
    let pipeline = manager
        .registry
        .parse(args.get(4).map_or(DEFAULT_PIPELINE, String::as_str))?;
    report.pipeline = format!("select-lod{{form=finest}}, {pipeline}");
    manager.run(&mut world, &pipeline, &mut report)?;

    let options = WriteOptions {
        group_by,
        include_unplaced: false,
        scene_name: Some(scene_name),
        selection: None,
        all_variants: false,
        threads: primase_core::parallel::Threads::ONE,
        images: None,
        verified: false,
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
