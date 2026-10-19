//! A skate-scene folder -> a ReSkate Studio normalized map folder.
//!
//! `cargo run --release -p primase-reskate --example build_map -- <scene folder> --out <root> [--tex-cap N]
//! [--tex-caps FILE] [--water-ocean] [--lights-max-per-cell N] [--piece-log] [--name N] [--lod default|highest|N]
//! [--max-priority N]`

use std::path::PathBuf;

use primase_core::World;
use primase_reskate::BuildOptions;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = BuildOptions::default();
    let mut scene: Option<PathBuf> = None;
    let mut lod = primase_core::LodChoice::Default;
    let mut max_priority: Option<u8> = None;
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
            "--tex-cap" => o.tex_cap = val()?.parse()?,
            "--tex-caps" => o.tex_caps = Some(val()?),
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
    for step in lods.steps.iter().chain(&dropped.steps) {
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
