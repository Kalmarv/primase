//! A Unity level end to end: read, lowered, written as one GLB and as web tiles.
//!
//! Usage: `unity_export <Game>_Data <type tree package> <level> <output folder> [--web] [--ktx2]`.
//! Writes `<output>/<level>.glb`; with `--web`, the web tile set in `<output>/web` (`--ktx2`: textures as KTX2).
//! Prints every pass's counts and approximations. A ReSkate map of a Unity level is a program's recipe (raising,
//! surfaces, spawn and travel points are its choices), composed with `primase-reskate` in the consumer program.

use std::path::Path;

use primase_core::World;
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_unity::TypeTreePackage;
use primase_unity::level::read_level;
use primase_unity::player::Player;

const USAGE: &str =
    "usage: unity_export <Game>_Data <type tree package> <level> <output folder> [--web] [--ktx2]";

fn show(pass: &str, log: &PassLog) {
    println!("{pass}: {:?}", log.counts);
    if !log.approximations.is_empty() {
        println!("{pass} approximates: {:?}", log.approximations);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut paths, mut web, mut ktx2) = (Vec::new(), false, false);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--web" => web = true,
            "--ktx2" => ktx2 = true,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}; {USAGE}").into()),
            _ => paths.push(arg),
        }
    }
    let [data, tpk, level, out] = &paths[..] else {
        return Err(USAGE.into());
    };
    if ktx2 && !web {
        return Err(format!("--ktx2 needs --web; {USAGE}").into());
    }
    let out = Path::new(out);
    let player = Player::open(Path::new(data), TypeTreePackage::parse(&std::fs::read(tpk)?)?)?;
    let mut w = World::new();
    let started = std::time::Instant::now();
    let report = read_level(&player, level, &mut w)?;
    println!(
        "read {level}: {} renderers, {} tree renderers, {} prototypes, {} primitives, {} materials, {} \
         textures, {} terrains, {} lights (sun named: {}), {} colliders ({} by trees); axes {:?}, conditioned {:?}, \n         LODGroups {:?}, {} LOD forms",
        report.renderers,
        report.tree_renderers,
        report.prototypes,
        report.primitives,
        report.materials,
        report.textures,
        report.terrains,
        report.lights,
        report.sun_named,
        report.colliders,
        report.tree_colliders,
        report.axes,
        report.conditioned,
        report.lod_groups,
        report.lod_forms
    );
    println!(
        "not decoded {:?}; not reproduced {:?}",
        report.textures_not_decoded, report.not_reproduced
    );
    // the lowering, as a pipeline: each pass whose input the level has, verified after each
    let unit = |v| primase_gltf::prepare::UnitValue::new(v).expect("a unit value");
    let passes: Vec<Box<dyn Pass>> = vec![
        // the level as stored: what is off stays off, LOD 0 draws
        Box::new(primase_core::state::ResolveState::default()),
        Box::new(primase_core::SelectLod {
            choice: primase_core::LodChoice::Finest,
        }),
        Box::new(primase_unity::drawn::UnityDrawn),
        Box::new(primase_unity::lower::LowerUnityPbr),
        Box::new(primase_unity::lower_terrain::LowerUnityTerrain),
        Box::new(primase_unity::lower_collision::LowerUnityCollision),
        Box::new(primase_unity::lower_lights::LowerUnityLights),
        Box::new(primase_image::NormalizeNormals),
        // what the scene does not state, this program's choices: zeros for a texture coordinate set a primitive lacks,
        // light grey, dielectric and rough for a material without PBR factors
        Box::new(primase_gltf::prepare::FillMissingTexcoords),
        Box::new(primase_gltf::prepare::PrepareGltfMaterials {
            base_color: [0.8, 0.8, 0.8, 1.].map(unit),
            metallic: unit(0.),
            roughness: unit(0.85),
        }),
        // the writers here take collision as triangles
        Box::new(primase_meshops::TessellateColliders),
        // last before the writers: authored mirrors and shears into geometry, so every placement is rigid
        Box::new(primase_meshops::BakeNonrigid::default()),
    ];
    let mut lowering = Vec::new();
    for p in passes {
        // a pass's input may come from an earlier pass (terrain textures for normalize-normals)
        let present = |w: &World, p: &dyn Pass| p.requires().iter().all(|r| w.has_component_named(r));
        if present(&w, p.as_ref()) {
            let mut report = RunReport::default();
            PassManager::new(PassRegistry::new()).run(&mut w, &Pipeline { passes: vec![p] }, &mut report)?;
            lowering.extend(report.steps);
        } else {
            println!("{}: nothing to lower", p.name());
        }
    }
    for step in &lowering {
        show(&step.pass, &step.log);
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
    if web {
        // this program's choices: 200 m cells, files of at most 96 MiB, with KTX2 textures of at most 512 (colour,
        // normal) and 256 (mask) pixels, encodings of at most 360448 (colour) and 98304 (mask) bytes
        let options = primase_web::Options {
            ktx2,
            tile_byte_budget: Some(96 << 20),
            ..primase_web::Options::new(primase_web::TileSize::new(200.)?)
        };
        if ktx2 {
            use primase_web::prepare::{ByteBudget, PrepareWebTextures, RoleCaps, WebCap};
            let mut pass = PrepareWebTextures::new(RoleCaps {
                color: WebCap::new(512)?,
                normal: WebCap::new(512)?,
                mask: WebCap::new(256)?,
            });
            pass.color_bytes = Some(ByteBudget::new(360_448)?);
            pass.mask_bytes = Some(ByteBudget::new(98_304)?);
            let mut report = RunReport::default();
            PassManager::new(PassRegistry::new()).run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(pass)],
                },
                &mut report,
            )?;
            for step in &report.steps {
                show(&step.pass, &step.log);
            }
        }
        let set = primase_web::write(&w, &options)?;
        let folder = out.join("web");
        set.write_to(&folder)?;
        for o in &set.over_budget {
            println!("over the budget: {} ({} bytes): {}", o.file, o.bytes, o.reason);
        }
        let tiles = &set.index.tiles;
        println!(
            "web {} (format {}): tiles={} triangles={} placements={} instanced={} lights={} collision records={} \
             images={} sun={}",
            folder.display(),
            set.index.format_version,
            tiles.len(),
            tiles.iter().map(|t| t.triangles).sum::<u64>(),
            tiles.iter().map(|t| t.placements).sum::<u64>(),
            tiles.iter().map(|t| t.instanced_placements).sum::<u64>(),
            tiles.iter().map(|t| t.lights).sum::<u64>(),
            tiles
                .iter()
                .filter_map(|t| t.collision.as_ref())
                .map(|c| c.records)
                .sum::<usize>(),
            set.index.images.len(),
            set.index
                .environment
                .as_ref()
                .map_or("none", |e| e.sun.name.as_str())
        );
    }
    Ok(())
}
