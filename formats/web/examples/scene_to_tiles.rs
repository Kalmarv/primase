//! Export a scene folder as web tiles and (unless `--no-plain`) a plain comparison set, one cell at a time: each
//! cell's closure of the scene is loaded, lowered and written, then dropped, so memory holds one cell and the index.
// a path of its own, so a test can include this driver as a module (formats/web/tests/scene_to_tiles.rs)
#[path = "support/mod.rs"]
mod support;
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Entity, Instance, MeshFile, Name, World};
use primase_skate_scene::{SkateGrindCurve, SkateMarker, SkatePlacement};
use primase_web::{LocalBox, Options, PIVOT_MARGIN, ViewerItem};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;
const USAGE: &str = "usage: scene_to_tiles <scene folder> <output folder> [--ktx2 [--texture-cap <image name prefix>=<colour>,<normal>,<mask>]...] [--budget-mib N | --no-budget] [--max-priority N] [--no-plain]";

/// A prototype file mesh's local box, from its POSITION accessors' `min` and `max` (glTF requires them), reading only
/// the GLB's JSON chunk; `None` for a mesh without positions.
fn local_box(folder: &Path, f: &MeshFile) -> Result<Option<LocalBox>, Box<dyn std::error::Error>> {
    let path = folder.join(&f.path);
    let mut file = std::fs::File::open(&path)?;
    let mut head = [0u8; 20];
    file.read_exact(&mut head)?;
    if &head[..4] != b"glTF" || &head[16..20] != b"JSON" {
        return Err(format!("{}: not a GLB with a JSON chunk", path.display()).into());
    }
    let mut json = vec![0u8; u32::from_le_bytes(head[12..16].try_into()?) as usize];
    file.read_exact(&mut json)?;
    let doc: serde_json::Value = serde_json::from_slice(&json)?;
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for p in doc["meshes"][f.mesh as usize]["primitives"]
        .as_array()
        .ok_or("mesh primitives")?
    {
        let Some(a) = p["attributes"]["POSITION"].as_u64() else {
            continue;
        };
        let a = &doc["accessors"][a as usize];
        for k in 0..3 {
            b[0][k] = b[0][k].min(a["min"][k].as_f64().ok_or("POSITION min")?);
            b[1][k] = b[1][k].max(a["max"][k].as_f64().ok_or("POSITION max")?);
        }
    }
    Ok(b[0][0].is_finite().then_some(b))
}

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
        primase_skate_scene::LowerSkatePbr.run(&mut w, &mut PassLog::default())?;
        // core.layered stays as it is: the writer carries it (PRIMASE_layered_material) for per-pixel blending. The
        // strongest-layer lowering is the skate target's lossy path and leaves the replaced layered and layer materials
        // in the world, which the per-cell closure check rejects.
        primase_image::normalize_normals(&mut w)?;
    }
    Ok(w)
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
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ktx2" => ktx2 = true,
            "--texture-cap" => caps.push(args.next().ok_or(USAGE)?),
            "--no-budget" => budget = Some(None),
            // the plain comparison set is left out (a large scene's would not fit beside it)
            "--no-plain" => with_plain = false,
            "--max-priority" => max_priority = Some(args.next().ok_or(USAGE)?.parse()?),
            "--budget-mib" => {
                let mib: u64 = args.next().ok_or(USAGE)?.parse()?;
                budget = Some(Some(mib * 1024 * 1024));
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}; {USAGE}").into()),
            _ => paths.push(arg),
        }
    }
    if paths.len() != 2 || (!caps.is_empty() && !ktx2) {
        return Err(USAGE.into());
    }
    let folder = Path::new(&paths[0]);
    let output = Path::new(&paths[1]);
    let mut world = World::new();
    primase_skate_scene::read(folder, &mut world)?;
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
    primase_skate_scene::LowerSkateLights.run(&mut world, &mut PassLog::default())?;
    let (scene_lights, lowered) = (
        world.iter::<primase_skate_scene::SkateLight>().count(),
        world.iter::<primase_core::Light>().count(),
    );
    if lowered != scene_lights {
        return Err(format!("{lowered} lights lowered of the scene's {scene_lights}").into());
    }
    let mut options = Options {
        base_dir: folder.into(),
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
        }
    }
    options.split_by_tile = world
        .iter::<SkatePlacement>()
        .filter_map(|(e, p)| p.split_by_tile.then_some(e))
        .collect();
    let name = |e| {
        world
            .get::<Name>(e)
            .map_or_else(|| format!("Entity_{}", e.index()), |n| n.0.clone())
    };
    for (e, c) in world.iter::<SkateGrindCurve>() {
        if let Some(p) = c.points.first() {
            options.viewer_items.push(ViewerItem {
                name: name(e),
                kind: "grind_curve".into(),
                anchor: p.map(f64::from),
                data: serde_json::to_value(c)?,
            });
        }
    }
    for (e, m) in world.iter::<SkateMarker>() {
        let anchor = m
            .xyz()
            .map(|(x, y, z)| [x as f64, y.unwrap_or(0.) as f64, z as f64])
            .or_else(|| m.m.map(|v| [v[9] as f64, v[10] as f64, v[11] as f64]))
            .unwrap_or([0.; 3]);
        options.viewer_items.push(ViewerItem {
            name: name(e),
            kind: if m.kind == primase_skate_scene::MarkerKind::Spawn {
                "spawn"
            } else {
                "marker"
            }
            .into(),
            anchor,
            data: serde_json::to_value(m)?,
        });
    }
    // collision: the writer takes the world's core.collision itself
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
                Some(f) => local_box(folder, f)?,
                None => None,
            });
        }
    }
    let spans = primase_web::spanning(&world, sets[0].0.tile_size, |p| {
        Ok(boxes.get(&p).copied().flatten())
    })?;
    let mut plans = Vec::new();
    let mut marked = 0;
    for (k, (options, out)) in sets.iter().enumerate() {
        let mut options = options.clone();
        let before = options.split_by_tile.len();
        options.split_by_tile.extend(spans.marked.iter().copied());
        if k == 0 {
            marked = options.split_by_tile.len() - before;
        }
        let geometry = |p: Entity| boxes.get(&p).is_some_and(Option::is_some);
        plans.push((primase_web::plan_with_geometry(&world, &options, &geometry)?, out));
    }
    println!(
        "placements filed by triangle centre as their pivot does not locate them: {marked} (wider than a cell {}, pivot over {PIVOT_MARGIN} m outside their box {}, either {}, already split by the scene {}; straddling a cell edge {}; of {} measured)",
        spans.wider_than_a_cell,
        spans.pivot_outside,
        spans.marked.len(),
        spans.marked.len() - marked,
        spans.straddling,
        spans.measured
    );
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
    let index = &indexes[0];
    println!(
        "tiles={} triangles={} placements={} instanced={} viewer_items={} lights={} bytes={}{}",
        index.tiles.len(),
        index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
        index.tiles.iter().map(|t| t.placements).sum::<u64>(),
        index.tiles.iter().map(|t| t.instanced_placements).sum::<u64>(),
        sets[0].0.viewer_items.len(),
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
