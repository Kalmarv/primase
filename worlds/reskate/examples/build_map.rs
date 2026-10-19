//! A skate-scene folder -> a ReSkate Studio normalized map folder: read, its meshes and textures loaded, the chosen
//! level of detail and placements, a spawn when the scene has none, the scene's region clipped when it states one
//! (`clip-reskate-region`), the textures capped (`cap-textures`, box-filtered as the map writer used to), tints
//! multiplied in (`prepare-reskate-textures`), the scene's texture budget when it states one
//! (`budget-reskate-textures`), then the writer.
//!
//! `cargo run --release -p primase-reskate --example build_map -- <scene folder> --out <root> [--tex-cap N]
//! [--tex-caps FILE] [--water-ocean] [--lights-max-per-cell N] [--piece-log] [--name N] [--lod default|highest|N]
//! [--max-priority N]`
//!
//! `--tex-cap` (256 when not given) caps every texture's larger side; `--tex-caps` is a JSON file whose `caps` object
//! caps textures by name.

use std::path::PathBuf;

use primase_core::World;
use primase_core::pass::{Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_reskate::BuildOptions;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = BuildOptions::default();
    let mut scene: Option<PathBuf> = None;
    let mut lod = primase_core::LodChoice::Default;
    let mut max_priority: Option<u8> = None;
    let mut tex_cap: u32 = 256;
    let mut name_caps: Vec<(String, u32)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let mut val = || -> Result<String, BoxError> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{a} needs a value").into())
        };
        match a.as_str() {
            "--out" => o.out_root = PathBuf::from(val()?),
            "--name" => o.name = Some(val()?),
            "--tex-cap" => tex_cap = val()?.parse()?,
            "--tex-caps" => {
                let v: serde_json::Value = serde_json::from_slice(&std::fs::read(val()?)?)?;
                for (k, c) in v.get("caps").and_then(|c| c.as_object()).into_iter().flatten() {
                    let c = c.as_u64().ok_or("--tex-caps: a cap is a number")?;
                    name_caps.push((k.clone(), u32::try_from(c)?));
                }
            }
            "--lights-max-per-cell" => o.lights_max_per_cell = val()?.parse()?,
            "--water-ocean" => o.water_ocean = true,
            "--no-water" => o.no_water = true,
            "--piece-log" => o.piece_log = true,
            "--lod" => lod = primase_reskate::lod_choice(&val()?)?,
            "--max-priority" => max_priority = Some(val()?.parse()?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}").into()),
            s => scene = Some(PathBuf::from(s)),
        }
        i += 1;
    }
    let scene = scene.ok_or("usage: build_map <scene folder> --out <root> [options]")?;
    let t0 = std::time::Instant::now();
    let mut w = World::new();
    primase_reskate::interchange::read(&scene, &mut w)?;
    let lods = primase_reskate::select_lod(&mut w, lod)?;
    let (warnings, dropped) = primase_reskate::leave_out_optional(&mut w, &scene, max_priority)?;
    o.scene_warnings = Some(warnings);
    // the meshes and textures the passes and the writer take (the writer reads no texture file)
    primase_reskate::interchange::load(&scene, &mut w)?;
    let mut passes: Vec<Box<dyn Pass>> = Vec::new();
    // a scene that states its region is clipped to it (the writer clips nothing of its own)
    if let Some(region) = w
        .resource::<primase_reskate::dialect::SkateScene>()
        .and_then(|s| s.region)
    {
        passes.push(Box::new(primase_reskate::prepare::ClipReskateRegion::new(
            region,
        )?));
    }
    // the writer makes no spawn: a scene without one gets this example's choice, street-snapped at the centre of the
    // scene's region (else of its placements), where the writer used to place it
    passes.push(Box::new(primase_reskate::prepare::PlaceReskateSpawn::centre()));
    // the writer's old cap: stored levels, else the 2 x 2 box down to the cap, normal maps alike
    let mut cap = primase_image::CapTextures::new(tex_cap)?.with_mode(primase_image::CapMode {
        normals: primase_image::NormalDownscale::Box,
        ..Default::default()
    });
    for (name, c) in name_caps {
        cap = cap.with_name_cap(&name, c)?;
    }
    passes.push(Box::new(cap));
    passes.push(Box::new(primase_reskate::prepare::PrepareReskateTextures));
    let budget = w
        .resource::<primase_reskate::dialect::SkateScene>()
        .and_then(|s| s.texture_budget.clone());
    if let Some(max) = budget.as_ref().and_then(|b| b.max).filter(|&m| m >= 1.0) {
        let aux = budget
            .and_then(|b| b.aux_weight)
            .unwrap_or(primase_reskate::prepare::budget::AUX_WEIGHT);
        passes.push(Box::new(
            primase_reskate::prepare::BudgetReskateTextures::new(max as usize)?.with_aux_weight(aux)?,
        ));
    }
    let mut prepared = RunReport::default();
    PassManager::new(PassRegistry::new()).run(&mut w, &Pipeline { passes }, &mut prepared)?;
    for step in lods.steps.iter().chain(&dropped.steps).chain(&prepared.steps) {
        if !step.log.counts.is_empty() {
            println!("{}: {:?}", step.pass, step.log.counts);
        }
    }
    let r = primase_reskate::build(&w, &scene, &o)?;
    println!(
        "map '{}': {} tiles, {} merged placements / {:.2} M tris, {} instances of {} prototypes, {} object records, {} materials, {} textures, collision {:.2} M tris, in {:.1}s",
        r.out_dir.display(),
        r.tiles,
        r.merged_entities,
        r.merged_tris as f64 / 1e6,
        r.instances,
        r.prototypes,
        r.object_records,
        r.materials,
        r.textures,
        r.collision_tris as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    if r.lights_without_tangent > 0 {
        println!(
            "lights: {} without a usable tangent (basis from the fallback axis)",
            r.lights_without_tangent
        );
    }
    Ok(())
}
