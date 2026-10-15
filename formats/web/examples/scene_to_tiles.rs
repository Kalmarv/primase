//! Export a scene folder as web tiles and a plain comparison set, one cell at a time: each cell's closure of the
//! scene is loaded, lowered and written, then dropped, so memory holds one cell and the index.
mod support;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, Name, World};
use primase_skate_scene::{SkateGrindCurve, SkateMarker, SkatePlacement};
use primase_web::{Options, TilePlan, ViewerItem};
use std::collections::BTreeMap;
use std::path::Path;
const USAGE: &str = "usage: scene_to_tiles <scene folder> <output folder> [--ktx2 [--texture-cap <image name prefix>=<colour>,<normal>,<mask>]...]";

/// `world`'s closure of `roots` with its meshes loaded, and (`render`) its textures loaded and the materials lowered
/// as the web writer requires.
fn prepare(
    world: &World,
    roots: impl IntoIterator<Item = Entity>,
    folder: &Path,
    render: bool,
) -> Result<World, Box<dyn std::error::Error>> {
    let mut w = world.subset(&world.closure(roots));
    primase_gltf::load_mesh_files(&mut w, folder)?;
    if render {
        primase_image::load_textures(&mut w, folder)?;
        primase_skate_scene::LowerSkatePbr.run(&mut w, &mut PassLog::default())?;
        primase_image::normalize_normals(&mut w)?;
    }
    Ok(w)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut paths, mut ktx2, mut caps) = (Vec::new(), false, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ktx2" => ktx2 = true,
            "--texture-cap" => caps.push(args.next().ok_or(USAGE)?),
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
    let mut options = Options {
        base_dir: folder.into(),
        ..Default::default()
    };
    if ktx2 {
        #[cfg(feature = "ktx2")]
        {
            options.ktx2 = true;
            options.texture_caps_by_name = caps
                .iter()
                .map(|rule| texture_cap(rule))
                .collect::<Result<_, _>>()?;
        }
        #[cfg(not(feature = "ktx2"))]
        return Err("--ktx2 requires the ktx2 cargo feature".into());
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
    let plain_output = output.join("plain");
    let mut plans: Vec<(TilePlan, &Path)> = vec![
        (primase_web::plan(&world, &options)?, output),
        (primase_web::plan(&world, &plain_options)?, &plain_output),
    ];
    for e in plans[0].0.unsplit() {
        let w = prepare(&world, [e], folder, false)?;
        for (plan, _) in &mut plans {
            plan.split(&w, e)?;
        }
    }
    let (mut proof, mut bytes) = (BTreeMap::new(), [0usize; 2]);
    for cell in plans[0].0.cells() {
        let mut w = prepare(&world, plans[0].0.roots(cell), folder, true)?;
        for (i, (plan, out)) in plans.iter_mut().enumerate() {
            for tile in plan.write_cell(cell, &mut w)? {
                tile.write_to(out)?;
                bytes[i] += tile.bytes.len();
                if i == 0 {
                    proof.insert(
                        (tile.info.col, tile.info.row),
                        support::tile_proof(&tile.info.file, &tile.bytes)?,
                    );
                }
            }
        }
    }
    let mut indexes = Vec::new();
    for (plan, out) in plans {
        let index = plan.finish()?;
        index.write_to(out)?;
        indexes.push(index);
    }
    support::write_entries(&proof.into_values().collect::<Vec<_>>(), output)?;
    let index = &indexes[0];
    println!(
        "tiles={} triangles={} placements={} instanced={} viewer_items={} bytes={} plain_bytes={}",
        index.tiles.len(),
        index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
        index.tiles.iter().map(|t| t.placements).sum::<u64>(),
        index.tiles.iter().map(|t| t.instanced_placements).sum::<u64>(),
        options.viewer_items.len(),
        bytes[0],
        bytes[1]
    );
    Ok(())
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
