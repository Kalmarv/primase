//! A skate-scene folder -> a ReSkate Studio normalized map folder: read, its meshes and textures loaded, the chosen
//! level of detail and placements, the scene's region clipped and its floor lifted when it states them
//! (`clip-reskate-region`, `prepare-reskate-floor`), the grind curves' prisms (`prepare-reskate-grinds`), a spawn
//! when the scene has none, the markers snapped (`snap-reskate-markers`), the textures capped (`cap-textures`, box-filtered as the map writer used to), tints
//! multiplied in (`prepare-reskate-textures`), the scene's texture budget when it states one
//! (`budget-reskate-textures`), the triangles over too many texture repeats (`prepare-reskate-uv-repeats`), the
//! overlays stacked (`prepare-reskate-overlays`, 4 mm), the blend chains split (`prepare-reskate-blend-order`, 200 m),
//! the collision planned (`prepare-reskate-collision`, minor surfaces coalesced, 1 mm weld) and the chains' budget
//! (`plan-reskate-blend-budget`, the cull-ID pool), then the writer.
//!
//! `cargo run --release -p primase-reskate --example build_map -- <scene folder> --out <root> [--tex-cap N]
//! [--tex-caps FILE] [--water-ocean] [--no-water] [--map-height N] [--lights-max-per-cell N] [--name N] [--lod default|highest|N]
//! [--max-priority N]`
//!
//! `--tex-cap` (256 when not given) caps every texture's larger side; `--tex-caps` is a JSON file whose `caps` object
//! caps textures by name.

use std::path::PathBuf;

use primase_core::World;
use primase_core::pass::{Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_reskate::EncodingOptions;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = EncodingOptions::default();
    let mut scene: Option<PathBuf> = None;
    let mut lod = primase_core::LodChoice::Default;
    let mut max_priority: Option<u8> = None;
    let mut tex_cap: u32 = 256;
    let mut lights_per_cell: Option<usize> = None;
    let mut ocean = primase_reskate::passes::OceanMaterials::Stated;
    let mut no_water = false;
    let mut map_height: usize = 2160;
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
            "--lights-max-per-cell" => lights_per_cell = Some(val()?.parse()?),
            "--water-ocean" => ocean = primase_reskate::passes::OceanMaterials::All,
            "--no-water" => no_water = true,
            "--map-height" => map_height = val()?.parse()?,
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
    primase_reskate::read::interchange::read(&scene, &mut w)?;
    let lods = primase_reskate::select_lod(&mut w, lod)?;
    // the meshes and textures the passes and the writer take (the writer reads no texture file); the scene's
    // warnings are read from them too
    primase_reskate::read::interchange::load(&scene, &mut w)?;
    let (warnings, dropped) = primase_reskate::leave_out_optional(&mut w, &scene, max_priority)?;
    o.scene_warnings = Some(warnings);
    let mut passes: Vec<Box<dyn Pass>> = Vec::new();
    // a scene that states its region is clipped to it (the writer clips nothing of its own)
    if let Some(region) = w
        .resource::<primase_reskate::components::SkateScene>()
        .and_then(|s| s.region)
    {
        // with the 30 m apron the writer kept around a region
        passes.push(Box::new(primase_reskate::passes::ClipReskateRegion::new(
            region, 30.0,
        )?));
    }
    // a scene that states a floor is lifted by it (the writer lifts nothing of its own): its height (100.01 m when it
    // gives none), the cutoff below the floor (250 m) and a pinned floor when it gives one, as the writer read them
    if let Some(m) = w
        .resource::<primase_reskate::components::SkateScene>()
        .and_then(|s| s.min_height.clone())
    {
        let mut floor = primase_reskate::passes::PrepareReskateFloor::new(
            m.height.unwrap_or(100.01) as f32,
            m.stub_below.unwrap_or(250.0) as f32,
        )?;
        if let Some(f) = m.floor {
            floor = floor.with_pinned_floor(f as f32)?;
        }
        passes.push(Box::new(floor));
    }
    // the grind curves' prisms, as Studio builds them, from the curves where the floor put them
    passes.push(Box::new(primase_reskate::passes::PrepareReskateGrinds::studio()));
    // the writer makes no spawn: a scene without one gets this example's choice, street-snapped at the centre of the
    // scene's region (else of its placements), where the writer used to place it
    passes.push(Box::new(primase_reskate::passes::PlaceReskateSpawn::centre()));
    // the markers snapped against the map's collision, for the encoding the map is written with
    passes.push(Box::new(
        primase_reskate::passes::SnapReskateMarkers::for_encoding(&o)?,
    ));
    // the water as the writer drew it: the surfaces touching the scene's region (its content without one), clipped
    // 400 m beyond it (none with --no-water). --water-ocean: water materials as Studio's ocean
    let region = w
        .resource::<primase_reskate::components::SkateScene>()
        .and_then(|s| s.region);
    let extent = if no_water {
        primase_reskate::passes::WaterExtent::None
    } else if region.is_some() {
        primase_reskate::passes::WaterExtent::Region { margin: 400.0 }
    } else {
        primase_reskate::passes::WaterExtent::Content { margin: 400.0 }
    };
    passes.push(Box::new(primase_reskate::passes::PrepareReskateWater::new(
        extent, ocean,
    )?));
    // the pause-map picture as the writer made it: the scene's own when it names one, else its geometry drawn top-down
    // over its region (its content without one), --map-height pixels high (2160)
    let named = w
        .resource::<primase_reskate::components::SkateScene>()
        .is_some_and(|s| s.pause_map.is_some());
    let pause = if named {
        primase_reskate::passes::PauseSource::Image
    } else {
        primase_reskate::passes::PauseSource::Geometry {
            height: map_height,
            extent: if region.is_some() {
                primase_reskate::passes::PauseExtent::Region
            } else {
                primase_reskate::passes::PauseExtent::Content
            },
            drawing: primase_reskate::passes::PauseDrawing {
                leave_out: primase_reskate::passes::PauseLeaveOut::Nothing,
                colour: primase_reskate::passes::PauseColour::Mean,
            },
        }
    };
    passes.push(Box::new(primase_reskate::passes::PrepareReskatePauseMap::new(
        pause,
    )?));
    // the lights capped per Studio 200 m cell when asked (the writer caps nothing of its own)
    if let Some(n) = lights_per_cell {
        passes.push(Box::new(primase_reskate::passes::SelectReskateLights::new(
            n, 200.0,
        )?));
    }
    // the writer's old cap: stored levels, else the 2 x 2 box down to the cap, normal maps alike
    let mut cap = primase_image::CapTextures::new(tex_cap)?.with_mode(primase_image::CapMode {
        normals: primase_image::NormalDownscale::Box,
        ..Default::default()
    });
    for (name, c) in name_caps {
        cap = cap.with_name_cap(&name, c)?;
    }
    passes.push(Box::new(cap));
    passes.push(Box::new(primase_reskate::passes::PrepareReskateTextures));
    let budget = w
        .resource::<primase_reskate::components::SkateScene>()
        .and_then(|s| s.texture_budget.clone());
    if let Some(max) = budget.as_ref().and_then(|b| b.max).filter(|&m| m >= 1.0) {
        let aux = budget
            .and_then(|b| b.aux_weight)
            .unwrap_or(primase_reskate::passes::budget::AUX_WEIGHT);
        passes.push(Box::new(
            primase_reskate::passes::BudgetReskateTextures::new(max as usize)?.with_aux_weight(aux)?,
        ));
    }
    // last, on the prepared geometry, materials and textures: triangles over more texture repeats than Studio draws,
    // decals' left out and blended surfaces drawn alpha tested, as the writer did them
    passes.push(Box::new(primase_reskate::passes::PrepareReskateUvRepeats::new(
        primase_reskate::passes::RepeatDecals::Omit,
        primase_reskate::passes::RepeatBlended::Cutout,
    )));
    // the overlays stacked 4 mm apart, as the writer stacked them, over the meshes this encoding makes
    passes.push(Box::new(
        primase_reskate::passes::PrepareReskateOverlays::new(0.004)?.for_encoding(&o)?,
    ));
    // and the blend chains split into objects of their own, cut on Studio's 200 m cells, as the writer split them
    passes.push(Box::new(
        primase_reskate::passes::PrepareReskateBlendOrder::new(200.0)?.for_encoding(&o)?,
    ));
    // the collision as the writer wrote it: a leaf's minor surfaces written as its most common one, its pieces welded
    // at a millimetre
    passes.push(Box::new(primase_reskate::passes::PrepareReskateCollision::new(
        primase_reskate::components::CollisionSurfaces::Coalesce,
        primase_reskate::components::CollisionWeld::Millimetre,
        &o,
    )?));
    // and as many chain objects as the cull-ID pool has room for, decided from the map's summary (the writer's own
    // count of what the rest of the map takes, the planned collision's included)
    passes.push(Box::new(
        primase_reskate::passes::SummarizeReskateMap::for_encoding(&o)?,
    ));
    passes.push(Box::new(primase_reskate::passes::PlanReskateBlendBudget::pool()));
    let mut prepared = RunReport::default();
    PassManager::new(PassRegistry::new()).run(&mut w, &Pipeline { passes }, &mut prepared)?;
    for step in lods.steps.iter().chain(&dropped.steps).chain(&prepared.steps) {
        if !step.log.counts.is_empty() {
            println!("{}: {:?}", step.pass, step.log.counts);
        }
    }
    let r = primase_reskate::write_map(&w, &scene, &o)?;
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
