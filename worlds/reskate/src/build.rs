//! A skate-scene world -> a ReSkate Studio normalized map folder (the reference `scene-skate build`).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: writes the map folder"
)]

use crate::passes::pause_map::SkatePausePicture;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::assembly::{
    EncodingOptions, SoupWorld, apron_of, assemble, bounds, grouping, hulls, writer_options,
    writer_placements,
};
use crate::collision::CollisionWorld;
use crate::map::{Chains, Extras, MapWriter, Stack, WriterOptions};
use crate::model::CollisionMode;
use crate::records::{CollisionDropped, OverlayStack, SoupStats, StatsJson, Totals};
use crate::scene::Scene;

use crate::{Error, Result};

/// What the build did (the log line's numbers).
#[derive(Debug)]
pub struct BuildReport {
    pub out_dir: PathBuf,
    pub tiles: usize,
    pub merged_entities: i64,
    pub merged_tris: i64,
    pub instances: i64,
    pub prototypes: usize,
    pub object_records: usize,
    pub materials: usize,
    pub textures: usize,
    pub collision_tris: i64,
    /// Lights whose basis came from the fallback axis (no usable tangent).
    pub lights_without_tangent: usize,
    /// The overlay stack's lift record (see `WriteResult::lifts`).
    pub lifts: BTreeMap<String, Vec<Vec<f32>>>,
    /// The blend-chain overlays as objects of their own.
    pub blend: crate::blend_order::Stats,
    /// What the writer noticed (`map.json`'s warnings, then `stats.json`'s).
    pub warnings: Vec<String>,
    /// What the world holds that the writer does not write, by what, with counts.
    pub unsupported: Vec<(String, u64)>,
}

/// The scene reader's warnings as the reference build reports them: unused textures, primitives without normals,
/// unplaced prototypes and prototypes without geometry, too many travel points.
fn scene_warnings(s: &Scene, world: &World) -> Result<Vec<String>> {
    use crate::components::{SkateMaterial, SkateTexture};
    // each kind in a stable order (by its text), not the order the world's entities came in
    let mut out = Vec::new();
    let mut used: HashSet<primase_core::Entity> = HashSet::new();
    for (_, m) in world.iter::<SkateMaterial>() {
        used.extend(m.base.iter().chain(&m.normal).chain(&m.mask).copied());
    }
    let mut unused: Vec<String> = world
        .iter::<SkateTexture>()
        .filter(|(e, _)| !used.contains(e))
        .map(|(_, t)| format!("texture '{}' is not used", t.key))
        .collect();
    unused.sort();
    out.extend(unused);
    let mut flat = Vec::new();
    for p in &s.protos {
        if let Some(prims) = p.prims() {
            for (i, prim) in prims.iter().enumerate() {
                if prim.nrm.is_none() {
                    flat.push(format!(
                        "prototype '{}': primitive {i} has no NORMAL (flat normals are made)",
                        p.id
                    ));
                }
            }
        }
    }
    flat.sort();
    out.extend(flat);
    let mut placed = vec![false; s.protos.len()];
    for p in &s.placements {
        placed[p.proto as usize] = true;
    }
    let mut unplaced = Vec::new();
    for (p, placed) in s.protos.iter().zip(placed) {
        if !placed {
            unplaced.push(format!("prototype '{}' is not placed", p.id));
        } else if p.prims.is_none() {
            unplaced.push(format!(
                "prototype '{}' has no geometry; its placements draw nothing",
                p.id
            ));
        }
    }
    unplaced.sort();
    out.extend(unplaced);
    let travel = s.markers.iter().filter(|m| m.kind == "travel").count();
    if travel > 25 {
        out.push(format!("{travel} travel points: Studio takes at most 25"));
    }
    Ok(out)
}

/// A level of detail as the reference build names it (`--lod`): `default` (the scene's `lod_default`), `highest`
/// (the finest) or a level N (the coarsest form at N or finer).
pub fn lod_choice(s: &str) -> Result<primase_core::LodChoice> {
    use primase_core::LodChoice;
    Ok(match s {
        "default" => LodChoice::Default,
        "highest" => LodChoice::Finest,
        n => LodChoice::AtMost(
            n.parse()
                .map_err(|_| Error::Scene(format!("--lod '{n}' is not default, highest or a level 0-255")))?,
        ),
    })
}

/// The reference build's `--lod` as a pass, run before [`write_map`] (which refuses a world with levels of detail):
/// each prototype keeps the form `lod` takes (`--lod`, default `default`).
pub fn select_lod(world: &mut World, lod: primase_core::LodChoice) -> Result<primase_core::pass::RunReport> {
    run_passes(world, vec![Box::new(primase_core::SelectLod { choice: lod })])
}

/// The reference build's `--max-priority N` as a pass, run last before [`write_map`]: the scene's warnings as the
/// reference reports them (from the whole scene, before its writer leaves placements out), for
/// [`EncodingOptions::scene_warnings`], then `drop-optional{max=N}` (nothing left out without N). The scene is read
/// with its files loaded (`interchange::load`): the warnings look at the meshes, and textures need pixels.
pub fn leave_out_optional(
    world: &mut World,
    dir: &Path,
    max_priority: Option<u8>,
) -> Result<(Vec<String>, primase_core::pass::RunReport)> {
    let warnings = scene_warnings(&Scene::from_world(world, dir)?, world)?;
    let passes: Vec<Box<dyn primase_core::pass::Pass>> = match max_priority {
        Some(max) => vec![Box::new(primase_core::DropOptional { max })],
        None => Vec::new(),
    };
    Ok((warnings, run_passes(world, passes)?))
}

fn run_passes(
    world: &mut World,
    passes: Vec<Box<dyn primase_core::pass::Pass>>,
) -> Result<primase_core::pass::RunReport> {
    let mut report = primase_core::pass::RunReport::default();
    primase_core::pass::PassManager::new(primase_core::pass::PassRegistry::new())
        .run(world, &primase_core::pass::Pipeline { passes }, &mut report)
        .map_err(|e| Error::Scene(e.to_string()))?;
    Ok(report)
}

/// What [`write_map`] writes, as a conversion's target: the skate dialect (the `skate.` and `reskate.` names), the
/// core names the writer reads (names, placements, meshes and textures loaded, collision, material variants,
/// priorities, the surface table) and those `raise-skate` raises into the dialect, written in their raised form
/// (`pbr.material`, `core.material_usage` and `core.draw_order` as `skate.material`, `core.light` as `skate.light`,
/// `core.marker` as `skate.marker`). A `core.mesh_file` or `core.texture_file` only beside its loaded mesh or texture;
/// a blend (`core.layered`) is lowered first (`lower-layered-reskate`), one form per prototype chosen first
/// (`select-lod`). Every other name is reported as not written: the map has no place for it.
pub fn target() -> primase_core::pass::Target {
    use crate::components::{
        SkateBaseFactor, SkateBlendBudget, SkateBlendChains, SkateCollision, SkateFloorPlan, SkateForm,
        SkateGrindCurve, SkateGrindPrism, SkateLight, SkateMarker, SkateMarkerSnaps, SkateMaterial,
        SkateOverlayStack, SkateParts, SkatePartsOff, SkatePlacement, SkatePrototype, SkateRegionClip,
        SkateRock, SkateScene, SkateSoup, SkateTexture, SkateWater,
    };
    use crate::passes::layered::{BlendOpacity, VertexBlend};
    use crate::passes::pause_map::{SkatePauseImage, SkatePausePicture};
    use primase_core::pbr::PbrMaterial;
    use primase_core::{
        Collision, Component, DrawOrder, Instance, LayeredMaterial, Light, Lods, Marker, MaterialUsage,
        MaterialVariants, Mesh, MeshFile, Name, Priority, Resource, Surfaces, Texture, TextureFile,
        Transform,
    };
    primase_core::pass::Target::new("reskate")
        .legal(Name::NAME)
        .legal(Transform::NAME)
        .legal(Instance::NAME)
        .legal(Mesh::NAME)
        .legal(Texture::NAME)
        .legal(Collision::NAME)
        .legal(MaterialVariants::NAME)
        .legal(Priority::NAME)
        .legal(Surfaces::NAME)
        .legal(PbrMaterial::NAME)
        .legal(MaterialUsage::NAME)
        .legal(DrawOrder::NAME)
        .legal(Light::NAME)
        .legal(Marker::NAME)
        .dynamic(MeshFile::NAME, |w, e| w.has::<Mesh>(e))
        .dynamic(TextureFile::NAME, |w, e| w.has::<Texture>(e))
        .illegal(LayeredMaterial::NAME)
        .illegal(Lods::NAME)
        .legal(SkateScene::NAME)
        .legal(SkateTexture::NAME)
        .legal(SkateMaterial::NAME)
        .legal(SkatePrototype::NAME)
        .legal(SkateParts::NAME)
        .legal(SkatePartsOff::NAME)
        .legal(SkateForm::NAME)
        .legal(SkatePlacement::NAME)
        .legal(SkateCollision::NAME)
        .legal(SkateSoup::NAME)
        .legal(SkateRock::NAME)
        .legal(SkateLight::NAME)
        .legal(SkateGrindCurve::NAME)
        .legal(SkateGrindPrism::NAME)
        .legal(SkateWater::NAME)
        .legal(SkateMarker::NAME)
        .legal(SkateBaseFactor::NAME)
        .legal(SkateFloorPlan::NAME)
        .legal(SkateMarkerSnaps::NAME)
        .legal(SkateOverlayStack::NAME)
        .legal(SkateBlendChains::NAME)
        .legal(SkateBlendBudget::NAME)
        .legal(SkateRegionClip::NAME)
        .legal(SkatePauseImage::NAME)
        .legal(SkatePausePicture::NAME)
        .legal(VertexBlend::NAME)
        .legal(BlendOpacity::NAME)
}

/// The map folder of the prepared world `world` (`dir`: where its scene came from, recorded in the stats), encoded
/// as `a` says.
pub fn write_map(world: &World, dir: &Path, a: &EncodingOptions) -> Result<BuildReport> {
    let mut s = Scene::from_world(world, dir)?;
    let scene_warnings = match &a.scene_warnings {
        Some(w) => w.clone(),
        None => scene_warnings(&s, world)?,
    };
    let mut o = writer_options(&s, a)?;
    o.apron = apron_of(world, &s)?;
    o.pause = world.resource::<SkatePausePicture>().cloned();
    // markers snapped before the writer were snapped against this writer's collision: the same settings
    if let Some(b) = world.resource::<crate::components::SkateMarkerSnaps>() {
        let mine = (
            o.limit,
            [a.offset[0] as f32, a.offset[1] as f32],
            o.min_instances,
            o.min_instance_tris,
            o.max_render_edge,
        );
        let theirs = (
            b.limit,
            b.offset,
            b.min_instances,
            b.min_instance_tris,
            b.max_render_edge.unwrap_or(s.max_render_edge),
        );
        if mine != theirs {
            return Err(Error::Scene(format!(
                "the markers were snapped for the writer settings (limit, offset, min_instances, min_instance_tris, \
                 max_render_edge) {theirs:?}, but the writer's are {mine:?}: give snap-reskate-markers the writer's"
            )));
        }
    }
    let cut = s.region;
    let placements = writer_placements(&s, o.tile);
    o.region = cut.unwrap_or_else(|| bounds(&s, &placements));
    // the floor is prepare-reskate-floor's: the world comes lifted, its parts below the cutoff marked or gone
    let drop_below = f32::NEG_INFINITY;

    let out_dir = a.out_root.join(&o.name);
    // the overlay stack prepare-reskate-overlays made, for this grouping only; without one, none
    let stack = match world.resource::<crate::components::SkateOverlayStack>() {
        Some(rec) => {
            let mine = grouping(&s, a);
            if rec.grouping != Some(mine) {
                return Err(Error::Scene(format!(
                    "the overlays were stacked for the writer settings {:?}, but the writer's are {mine:?}: give \
                     prepare-reskate-overlays the writer's",
                    rec.grouping
                )));
            }
            Stack::Replay(Box::new(rec.clone()))
        }
        None => Stack::Off { unstacked: 0 },
    };
    // the blend chains prepare-reskate-blend-order chose, for this grouping only; without them, none
    let chains = match world.resource::<crate::components::SkateBlendChains>() {
        Some(rec) => {
            let mine = grouping(&s, a);
            if rec.grouping != Some(mine) {
                return Err(Error::Scene(format!(
                    "the blend chains were chosen for the writer settings {:?}, but the writer's are {mine:?}: give \
                     prepare-reskate-blend-order the writer's",
                    rec.grouping
                )));
            }
            Chains::Replay(Box::new(rec.clone()))
        }
        None => Chains::Off { left: 0 },
    };
    let budget = world.resource::<crate::components::SkateBlendBudget>().cloned();
    let mut writer = MapWriter::new(o.clone(), s.surfaces.clone())?
        .with_stack(stack)
        .with_chains(chains)
        .with_budget(budget);
    let tri_world = SoupWorld::new(&mut s, CollisionMode::TriangleMesh, cut, drop_below);
    let water_world = s
        .soups
        .iter()
        .any(|x| x.mode == CollisionMode::Water)
        .then(|| SoupWorld::new(&mut s, CollisionMode::Water, cut, drop_below));
    let hulls = hulls(&mut s, cut, drop_below);
    let failed = assemble(&s, &placements, &mut writer)?;
    writer.finish(Extras {
        world: &tri_world,
        water_world: water_world.as_ref().map(|w| w as &dyn CollisionWorld),
        hulls,
        water: s.water.clone(),
        lights: s.lights.clone(),
        markers: s.markers.clone(),
        grind: s.curves.clone(),
    })?;
    writer.r.warnings.extend(scene_warnings);

    // write the folder
    for (dir_name, files) in [("meshes", &writer.mesh_files)] {
        let d = out_dir.join(dir_name);
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(|source| Error::Io {
                path: d.clone(),
                source,
            })?;
        }
        std::fs::create_dir_all(&d).map_err(|source| Error::Io {
            path: d.clone(),
            source,
        })?;
        for (name, bytes) in files {
            let p = d.join(name);
            std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
        }
    }
    let td = out_dir.join("textures");
    if td.exists() {
        std::fs::remove_dir_all(&td).map_err(|source| Error::Io {
            path: td.clone(),
            source,
        })?;
    }
    std::fs::create_dir_all(&td).map_err(|source| Error::Io {
        path: td.clone(),
        source,
    })?;
    for (file, bytes) in writer.ctx.tex.files()? {
        let p = td.join(file);
        std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
    }
    for (name, bytes) in &writer.files {
        let p = out_dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
    }
    if !writer.files.contains_key("trainer.json") {
        let p = out_dir.join("trainer.json");
        if p.exists() {
            std::fs::remove_file(&p).map_err(|source| Error::Io { path: p, source })?;
        }
    }
    let stats = stats_json(&writer, &s, &o, placements.len(), failed)?;
    let p = out_dir.join("stats.json");
    std::fs::write(&p, stats).map_err(|source| Error::Io { path: p, source })?;
    let r = &writer.r;
    Ok(BuildReport {
        out_dir,
        tiles: r.tiles.len(),
        merged_entities: r.merged_entities,
        merged_tris: r.merged_tris,
        instances: r.instances,
        prototypes: r.proto_list.len(),
        object_records: r.object_records,
        materials: r.materials,
        textures: writer.ctx.tex.all().iter().filter(|t| !t.removed).count(),
        collision_tris: r.collision.as_ref().map_or(0, |c| c.triangles),
        lights_without_tangent: r.lights_without_tangent,
        lifts: r.lifts.clone(),
        blend: r.blend.clone(),
        warnings: o.warnings.iter().chain(&r.warnings).cloned().collect(),
        unsupported: s
            .unsupported
            .iter()
            .cloned()
            .chain(match writer.stack() {
                Stack::Off { unstacked } if *unstacked > 0 => Some((
                    "overlay pieces drawn where authored (prepare-reskate-overlays stacks them)".to_owned(),
                    *unstacked,
                )),
                _ => None,
            })
            .chain(
                [
                    (
                        "water outlines cut at the map's extent limit",
                        r.water_cut_at_limit,
                    ),
                    ("water outlines with no area, not drawn", r.water_without_area),
                ]
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .map(|(what, n)| (what.to_owned(), n as u64)),
            )
            .chain(
                [
                    (
                        "clip-reskate-region: placements outside the region with nothing within its apron, left out",
                        writer.r.far_dropped,
                    ),
                    (
                        "clip-reskate-region: placements outside the region trimmed to its apron",
                        writer.r.apron_trimmed,
                    ),
                    (
                        "instanced placements reaching past the map's limit (Studio's extent), left out",
                        writer.r.limit_instances,
                    ),
                    ("triangles past the map's limit, left out", writer.r.limit_triangles),
                    (
                        "collision triangles past the map's limit less 20 m, left out",
                        writer.r.collision.as_ref().map_or(0, |c| c.cut_beyond_limit + c.props_beyond_limit),
                    ),
                ]
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .map(|(what, n)| (what.to_owned(), n as u64)),
            )
            .chain(writer.r.chain_fallbacks.iter().map(|line| (line.clone(), 1)))
            .chain((writer.r.chains_unbudgeted > 0).then(|| {
                (
                    "tiles' blend chains written plain: no blend budget planned (plan-reskate-blend-budget)"
                        .to_owned(),
                    writer.r.chains_unbudgeted as u64,
                )
            }))
            .chain(match writer.chains() {
                Chains::Off { left } if *left > 0 => Some((
                    "tiles' vertex-blend chain overlays drawn in the tile, in an unstable order \
                     (prepare-reskate-blend-order splits them)"
                        .to_owned(),
                    *left,
                )),
                _ => None,
            })
            .collect(),
    })
}

fn stats_json(
    writer: &MapWriter,
    s: &Scene,
    o: &WriterOptions,
    placements: usize,
    failed: i64,
) -> Result<Vec<u8>> {
    let r = &writer.r;
    let col = r.collision.as_ref().expect("finished");
    let full = std::path::absolute(&s.dir).unwrap_or_else(|_| s.dir.clone());
    let kept: Vec<_> = writer.ctx.tex.all().iter().filter(|t| !t.removed).collect();
    let mut formats: BTreeMap<&str, i64> = BTreeMap::new();
    for t in &kept {
        *formats.entry(t.format.as_str()).or_default() += 1;
    }
    let mut top: Vec<(&String, &i64)> = r.prop_col_tris.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut seen = HashSet::new();
    let ov = &r.overlay;
    let stats = StatsJson {
        front_end: s.stats.as_ref(),
        name: &o.name,
        scene: full.to_string_lossy().into_owned(),
        region: o.region,
        tile_size: o.tile,
        lift_m: s.floor_plan.as_ref().map_or(0.0, |f| f.lift),
        totals: Totals {
            placements,
            placements_degenerate: r.degenerate,
            placements_stubs_left_out: s.floor_plan.as_ref().map_or(0, |f| f.placements_removed as i64),
            placed_parts_stubs_left_out: s.floor_plan.as_ref().map_or(0, |f| f.parts_removed as i64),
            placements_clipped_away: r.far_dropped,
            prototypes_placed: r.archetypes,
            prototypes_failed: failed,
            tiles: r.tiles.len(),
            object_records: r.object_records,
            merged_placements: r.merged_entities,
            merged_triangles: r.merged_tris,
            instances: r.instances,
            instanced_triangles_drawn: r.instanced_tris,
            prototypes: r.proto_list.len(),
            prototype_triangles: r.prototype_tris,
            collision_pieces: col.pieces,
            collision_triangles: col.triangles,
            collision_hulls: col.hulls,
            materials: r.materials,
            textures: kept.len(),
            texture_file_bytes: r.tex_bytes,
            texture_pixels: r.tex_pixels,
            mesh_file_bytes: r.mesh_bytes,
            textures_unsupported: writer.ctx.tex.unsupported,
            placements_with_empty_mesh: r.empty_mesh,
            placements_collision_only: r.collision_only,
            lights_written: r.lights_written,
            lights_without_tangent: r.lights_without_tangent,
            grind_curves: r.grind_curves,
            grind_pieces: r.grind_pieces,
            grind_length_m: r.grind_length,
            travel_points: r.travel_points,
            trainer_spots: r.trainer_spots,
            peak_mesh_bytes: r.peak_mesh_bytes,
            seconds: 0.0,
        },
        soups: s
            .soups
            .iter()
            .map(|sd| SoupStats {
                file: &sd.file,
                mode: match sd.mode {
                    CollisionMode::TriangleMesh => "triangle_mesh",
                    CollisionMode::Water => "water",
                    CollisionMode::Hull => "hull",
                },
                read: sd.surf.len(),
                cut_by_region: sd.cut,
                kept_before_cleaning: sd.kept,
            })
            .collect(),
        collision_dropped: CollisionDropped {
            degenerate: col.cut_degenerate,
            beyond_map_edge: col.cut_beyond_limit,
        },
        overlay_stack: OverlayStack {
            step_m: writer.stack().step(),
            overlay_pieces: ov.pieces,
            shifted: ov.shifted,
            water_shifted: ov.water_shifted,
            max_shift_m: ov.max_shift,
            order_cycles_broken: ov.cycles,
        },
        blend_order: {
            let b = &writer.r.blend;
            (b.tiles_split + b.tiles_shared > 0).then_some(b)
        },
        texture_formats: formats,
        prop_collision_top: top
            .into_iter()
            .take(40)
            .map(|(k, v)| (k.clone(), serde_json::Value::from(*v)))
            .collect(),
        warnings: r
            .warnings
            .iter()
            .filter(|x| seen.insert(x.as_str()))
            .map(String::as_str)
            .collect(),
    };
    crate::json_bytes("stats.json", &stats, true)
}
