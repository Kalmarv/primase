//! Export a scene folder as web tiles and (unless `--no-plain`) a plain comparison set, one cell at a time: each
//! cell's closure of the scene is loaded, lowered and written, then dropped, so memory holds one cell and the index.
// a path of its own, so a test can include this driver as a module (cli/tests/scene_to_tiles.rs); the tile proof is
// the web examples'
#[path = "../../formats/web/examples/support/mod.rs"]
mod support;
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Entity, Instance, MeshFile, World};
use primase_web::{Options, PIVOT_MARGIN};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
const USAGE: &str = "usage: scene_to_tiles <scene folder> <output folder> [--ktx2 [--texture-cap <image name prefix>=<colour>,<normal>,<mask>]...] [--budget-mib N | --no-budget] [--max-priority N] [--no-plain] [--camera <eye x,y,z>:<target x,y,z>] [--cache <folder> [--cache-max-gib N] [--cache-verify]] (KTX2 encodings are kept in the cache folder, else in PRIMASE_CACHE when set, across runs)";

/// `world`'s closure of `roots` with its meshes loaded, and (for rendering) its textures loaded and the materials
/// lowered as the web writer requires.
fn prepare(
    world: &World,
    roots: &BTreeSet<Entity>,
    folder: &Path,
    render: bool,
) -> Result<World, Box<dyn std::error::Error>> {
    let mut w = world.subset(&world.closure(roots.iter().copied()));
    primase_gltf::load_mesh_files(&mut w, folder)?;
    if render {
        primase_image::load_textures(&mut w, folder)?;
        primase_reskate::dialect::LowerSkatePbr.run(&mut w, &mut PassLog::default())?;
        // core.layered stays as it is: the writer carries it (PRIMASE_layered_material) for per-pixel blending. The
        // strongest-layer lowering is the skate target's lossy path and leaves the replaced layered and layer materials
        // in the world, which the per-cell closure check rejects.
        primase_image::normalize_normals(&mut w)?;
        // the skate scene's own rules for what it does not state: zeros for a texture coordinate set a primitive lacks,
        // light grey, dielectric and rough for a material without PBR factors
        let unit = |v| primase_gltf::prepare::UnitValue::new(v).expect("a unit value");
        primase_gltf::prepare::FillMissingTexcoords.run(&mut w, &mut PassLog::default())?;
        primase_gltf::prepare::PrepareGltfMaterials {
            base_color: [0.8, 0.8, 0.8, 1.].map(unit),
            metallic: unit(0.),
            roughness: unit(0.85),
        }
        .run(&mut w, &mut PassLog::default())?;
    }
    Ok(w)
}
/// `x,y,z:x,y,z`: the eye, then the point it looks at.
fn parse_camera(arg: &str) -> Result<primase_web::Camera, Box<dyn std::error::Error>> {
    let point = |p: &str| -> Result<[f64; 3], Box<dyn std::error::Error>> {
        let v: Vec<f64> = p.split(',').map(str::parse).collect::<Result<_, _>>()?;
        v.try_into()
            .map_err(|_| format!("--camera {arg}: a point is three numbers").into())
    };
    let (eye, target) = arg
        .split_once(':')
        .ok_or_else(|| format!("--camera {arg}: <eye x,y,z>:<target x,y,z>"))?;
    Ok(primase_web::Camera {
        eye: point(eye)?,
        target: point(target)?,
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(std::env::args().skip(1)).map(|_| ())
}

/// The conversion of [`USAGE`]'s arguments, printing what it did; the tile set's index (the plain set's is written
/// beside it). Refuses a set that lost a scene light.
pub fn run(args: impl IntoIterator<Item = String>) -> Result<primase_web::Index, Box<dyn std::error::Error>> {
    let (mut paths, mut ktx2, mut caps, mut budget) = (Vec::new(), false, Vec::new(), None);
    let mut max_priority: Option<u8> = None;
    let mut with_plain = true;
    let mut camera = None;
    let (mut cache_dir, mut cache_max_gib, mut cache_verify) = (None::<PathBuf>, None::<u64>, false);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ktx2" => ktx2 = true,
            "--texture-cap" => caps.push(args.next().ok_or(USAGE)?),
            "--no-budget" => budget = Some(None),
            // the plain comparison set is left out (a large scene's would not fit beside it)
            "--no-plain" => with_plain = false,
            "--cache" => cache_dir = Some(args.next().ok_or(USAGE)?.into()),
            "--cache-max-gib" => cache_max_gib = Some(args.next().ok_or(USAGE)?.parse()?),
            "--cache-verify" => cache_verify = true,
            "--max-priority" => max_priority = Some(args.next().ok_or(USAGE)?.parse()?),
            // the index's starting view, world frame metres
            "--camera" => camera = Some(parse_camera(&args.next().ok_or(USAGE)?)?),
            "--budget-mib" => {
                let mib: u64 = args.next().ok_or(USAGE)?.parse()?;
                budget = Some(Some(mib * 1024 * 1024));
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}; {USAGE}").into()),
            _ => paths.push(arg),
        }
    }
    if paths.len() != 2
        || (!ktx2 && (!caps.is_empty() || cache_dir.is_some() || cache_max_gib.is_some() || cache_verify))
    {
        return Err(USAGE.into());
    }
    let folder = Path::new(&paths[0]);
    let output = Path::new(&paths[1]);
    let mut world = World::new();
    primase_reskate::interchange::read(folder, &mut world)?;
    // placements more optional than --max-priority are left out, only when asked; each prototype is drawn with its
    // finest level of detail (the viewer does not choose between them yet)
    let mut passes: Vec<Box<dyn Pass>> = Vec::new();
    if let Some(max) = max_priority {
        passes.push(Box::new(primase_core::DropOptional { max }));
    }
    passes.push(Box::new(primase_core::SelectLod {
        choice: primase_core::LodChoice::Finest,
    }));
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new()).run(&mut world, &Pipeline { passes }, &mut report)?;
    for step in &report.steps {
        if !step.log.counts.is_empty() {
            println!("{}: {:?}", step.pass, step.log.counts);
        }
    }
    // lights are planned from the whole scene, like viewer records, not loaded per cell; every scene light must be
    // lowered (the writer then checks each planned light reaches a tile)
    primase_reskate::dialect::LowerSkateLights.run(&mut world, &mut PassLog::default())?;
    let (scene_lights, lowered) = (
        world.iter::<primase_reskate::dialect::SkateLight>().count(),
        world.iter::<primase_core::Light>().count(),
    );
    if lowered != scene_lights {
        return Err(format!("{lowered} lights lowered of the scene's {scene_lights}").into());
    }
    let mut options = Options {
        base_dir: folder.into(),
        camera,
        ..Default::default()
    };
    if let Some(budget) = budget {
        options.tile_byte_budget = budget;
    }
    if ktx2 {
        #[cfg(feature = "ktx2")]
        {
            options.ktx2 = true;
            options.texture_caps_by_name = caps
                .iter()
                .map(|rule| texture_cap(rule))
                .collect::<Result<_, _>>()?;
            // --cache, else PRIMASE_CACHE when set and not empty, else no cache
            let configured = cache_dir.or_else(|| {
                std::env::var_os("PRIMASE_CACHE")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            });
            if let Some(dir) = configured {
                let max = cache_max_gib.map_or(primase_cache::DEFAULT_MAX_BYTES, |g| g << 30);
                options.cache = Some(std::sync::Arc::new(primase_cache::Cache::open(&dir, max)?));
                options.cache_verify = cache_verify;
                println!("KTX2 cache: {} (cap {max} bytes)", dir.display());
            }
        }
    }
    // viewer records: the writer takes the world's core.curve, core.marker and core.collision itself
    let plain_options = Options {
        compress: false,
        gpu_instancing: false,
        #[cfg(feature = "ktx2")]
        ktx2: false,
        ..options.clone()
    };
    let mut sets = vec![(options, output.to_path_buf())];
    if with_plain {
        sets.push((plain_options, output.join("plain")));
    }
    // plan: prototype boxes from the GLB headers, placements whose pivot does not locate them filed by triangle centre
    let mut boxes = BTreeMap::new();
    for (_, i) in world.iter::<Instance>() {
        if let std::collections::btree_map::Entry::Vacant(v) = boxes.entry(i.prototype) {
            v.insert(match world.get::<MeshFile>(i.prototype) {
                Some(f) => primase_gltf::mesh_file_box(folder, f)?,
                None => None,
            });
        }
    }
    let spans = primase_web::spanning(&world, sets[0].0.tile_size, |p| {
        Ok(boxes.get(&p).copied().flatten())
    })?;
    let mut plans = Vec::new();
    for (options, out) in &sets {
        let options = Options {
            split_by_tile: spans.marked.iter().copied().collect(),
            ..options.clone()
        };
        let geometry = |p: Entity| boxes.get(&p).is_some_and(Option::is_some);
        plans.push((primase_web::plan_with_geometry(&world, &options, &geometry)?, out));
    }
    println!(
        "placements filed by triangle centre as their pivot does not locate them: {} (wider than a cell {}, pivot over {PIVOT_MARGIN} m outside their box {}; straddling a cell edge {}; of {} measured)",
        spans.marked.len(),
        spans.wider_than_a_cell,
        spans.pivot_outside,
        spans.straddling,
        spans.measured
    );
    if plans[0].0.ground_markers() > 0 {
        println!(
            "markers without a height, shown at height 0: {}",
            plans[0].0.ground_markers()
        );
    }
    for e in plans[0].0.unsplit() {
        let proto = world.get::<Instance>(e).map_or(e, |i| i.prototype);
        let w = prepare(&world, &BTreeSet::from([e, proto]), folder, false)?;
        for (plan, _) in &mut plans {
            plan.split(&w, e)?;
        }
    }
    // each cell loaded once, written by every set, dropped
    let (mut proof, mut bytes) = (BTreeMap::new(), vec![0usize; plans.len()]);
    for cell in plans[0].0.cells() {
        let w = prepare(&world, &plans[0].0.roots(cell), folder, true)?;
        for (k, (plan, out)) in plans.iter_mut().enumerate() {
            for tile in plan.write_cell(cell, &w)? {
                tile.write_to(out)?;
                bytes[k] += tile.bytes.len();
                if k == 0 {
                    proof.insert(
                        (tile.info.col, tile.info.row, tile.info.cell),
                        support::tile_proof(&tile.info.file, &tile.bytes)?,
                    );
                }
            }
        }
    }
    let mut indexes = Vec::new();
    for (plan, out) in plans {
        for o in plan.over_budget() {
            println!(
                "over the budget: {}/{} ({} bytes): {}",
                out.display(),
                o.file,
                o.bytes,
                o.reason
            );
        }
        let index = plan.finish()?;
        index.write_to(out)?;
        indexes.push(index);
    }
    support::write_entries(&proof.into_values().collect::<Vec<_>>(), output)?;
    #[cfg(feature = "ktx2")]
    if let Some(cache) = &sets[0].0.cache {
        cache.trim()?;
        let s = cache.stats();
        println!(
            "KTX2 cache: hits={} misses={} corrupt={} written={} ({} bytes) write_errors={} evicted={} ({} bytes)",
            s.hits,
            s.misses,
            s.corrupt,
            s.written,
            s.written_bytes,
            s.write_errors,
            s.evicted,
            s.evicted_bytes
        );
    }
    let index = &indexes[0];
    println!(
        "tiles={} triangles={} placements={} instanced={} viewer_items={} lights={} bytes={}{}",
        index.tiles.len(),
        index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
        index.tiles.iter().map(|t| t.placements).sum::<u64>(),
        index.tiles.iter().map(|t| t.instanced_placements).sum::<u64>(),
        index.tiles.iter().map(|t| t.viewer_items).sum::<u64>(),
        index.tiles.iter().map(|t| t.lights).sum::<u64>(),
        bytes[0],
        if with_plain {
            format!(" plain_bytes={}", bytes[1])
        } else {
            String::new()
        }
    );
    Ok(indexes.swap_remove(0))
}

/// `<prefix>=<colour>,<normal>,<mask>`, e.g. `bake_=256,256,128`.
#[cfg(feature = "ktx2")]
fn texture_cap(rule: &str) -> Result<(String, primase_web::ktx2::TextureCaps), String> {
    let bad = || format!("--texture-cap {rule}: expected <prefix>=<colour>,<normal>,<mask>");
    let (prefix, values) = rule.split_once('=').ok_or_else(bad)?;
    let values: Vec<u32> = values
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    let [colour, normal, mask] = values[..] else {
        return Err(bad());
    };
    Ok((
        prefix.into(),
        primase_web::ktx2::TextureCaps { colour, normal, mask },
    ))
}
