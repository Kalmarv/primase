//! A skate-scene world -> a ReSkate Studio normalized map folder (the reference `scene-skate build`).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "writer I/O: writes the map folder"
)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::assembly::{
    EncodingOptions, Records, SceneWarnings, SoupWorld, assemble, bounds, hulls_numbered, part_options,
    scene_warnings, writer_placements,
};
use crate::components::{SkateContentBounds, SkateInstanceCounts, SkateLayoutSummary};
use crate::map::{CellKey, Chains, Extras, MapWriter, Stack, WriterOptions};
use crate::model::CollisionMode;
use crate::records::{CollisionDropped, OverlayStack, SoupStats, StatsJson, Totals};
use crate::scene::{GrindCurve, Marker, PlacedLight, Scene, SceneNotes, WaterQuad};

use crate::{Error, Result};

/// What the build did (the log line's numbers).
#[derive(Debug)]
pub struct BuildReport {
    pub out_dir: PathBuf,
    /// The parts the map came in (1: whole).
    pub parts: usize,
    /// How much collision the sink held between parts, and what it laid out at the end.
    pub held: HeldCells,
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
    /// The most source-mesh bytes the run held at once (loaded band by band): a measure of the run, not of the map,
    /// so the run's report and not `stats.json`.
    pub peak_mesh_bytes: i64,
    /// The overlay stack's lift record (see `WriteResult::lifts`).
    pub lifts: BTreeMap<String, Vec<Vec<f32>>>,
    /// The blend-chain overlays as objects of their own.
    pub blend: crate::blend_order::Stats,
    /// What the writer noticed (`map.json`'s warnings, then `stats.json`'s).
    pub warnings: Vec<String>,
    /// What the writer left out of the map, by what, with counts (a conversion reports each as not written).
    pub omitted: Vec<(String, u64)>,
    /// What it wrote, but not as the world says, by what, with counts (a conversion reports each as approximated).
    pub approximated: Vec<(String, u64)>,
    /// What the world holds that the target does not take ([`target`]'s check of the world written): names with the
    /// entities holding them, a covered name's uncovered entities as `<name> not <its cover>`, and resources (counted
    /// once). A conversion checks the target itself before writing, so this is for direct callers of [`write_map`].
    pub unsupported_names: Vec<(String, u64)>,
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
/// priorities, the surface table) and those `raise-skate` raises into the dialect, covered by their raised form
/// (`pbr.material`, `core.material_usage` and `core.draw_order` by `skate.material`, `core.light` by `skate.light`,
/// `core.marker` by `skate.marker`: one without it, as a directional light raise-skate does not raise, is reported as
/// not written). A `core.mesh_file` or `core.texture_file` only beside its loaded mesh or texture;
/// a blend (`core.layered`) is lowered first (`lower-layered-reskate`), one form per prototype chosen first
/// (`select-lod`). Every other name is reported as not written: the map has no place for it.
pub fn target() -> primase_core::pass::Target {
    use crate::components::{
        SkateBaseFactor, SkateBlendBudget, SkateBlendChains, SkateCollision, SkateCollisionPlan,
        SkateFloorPlan, SkateForm, SkateGrindCurve, SkateGrindPrism, SkateLight, SkateMapSummary,
        SkateMarker, SkateMarkerSnaps, SkateMaterial, SkateOverlayStack, SkateParts, SkatePartsOff,
        SkatePlacement, SkatePrototype, SkateRegionClip, SkateRock, SkateScene, SkateSoup, SkateTexture,
        SkateWater,
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
        .covered(PbrMaterial::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(MaterialUsage::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(DrawOrder::NAME, "covered by skate.material", |w, e| {
            w.has::<SkateMaterial>(e)
        })
        .covered(Light::NAME, "covered by skate.light", |w, e| {
            w.has::<SkateLight>(e)
        })
        .covered(Marker::NAME, "covered by skate.marker", |w, e| {
            w.has::<SkateMarker>(e)
        })
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
        // a map in parts: the floor's summary and decision, applied by prepare-reskate-floor
        .legal(crate::components::SkateFloorSummary::NAME)
        .legal(crate::components::SkateFloorDecision::NAME)
        .legal(crate::components::SkateLightsSummary::NAME)
        .legal(crate::components::SkateTextureWeights::NAME)
        .legal(crate::components::SkateContentBounds::NAME)
        .legal(crate::components::SkateSpawnSummary::NAME)
        .legal(crate::components::SkateInstanceCounts::NAME)
        .legal(crate::components::SkateWaterSurfaces::NAME)
        .legal(crate::components::SkateLayoutSummary::NAME)
        .legal(SkateMarkerSnaps::NAME)
        .legal(SkateOverlayStack::NAME)
        .legal(SkateBlendChains::NAME)
        .legal(SkateBlendBudget::NAME)
        // the map's summary is taken in through the budget planned from it (its basis is checked on writing)
        .legal(SkateMapSummary::NAME)
        .legal(SkateCollisionPlan::NAME)
        .legal(SkateRegionClip::NAME)
        .legal(SkatePauseImage::NAME)
        .legal(SkatePausePicture::NAME)
        // what the scene loader kept of the files it read: the payloads beside it are what is written
        .legal(crate::components::LoadedFile::NAME)
        .legal(VertexBlend::NAME)
        .legal(BlendOpacity::NAME)
}

/// The map folder of the prepared world `world` (`dir`: where its scene came from, recorded in the stats), encoded
/// as `a` says: the [`MapSink`] given the whole map as its one part.
pub fn write_map(world: &World, dir: &Path, a: &EncodingOptions) -> Result<BuildReport> {
    let mut sink = MapSink::new(dir, a);
    sink.part(world)?;
    sink.finish()
}

/// The map writer as a sink of a map's parts: [`MapSink::part`] lays a part out and writes its payloads at once (its
/// tiles' meshes, the prototypes and textures it is the first to use, the collision cells it completes), keeping only
/// what the map's index files and the report need; [`MapSink::finish`] lays out what is left (the cells still waiting,
/// the water, the grind prisms), writes the index files (`map.json`, the records, `stats.json`) and reports.
///
/// A map in several parts is written as the whole map is, byte for byte, whatever order the parts come in. Its parts
/// carry the map's layout summary (`summarize-reskate-layout`), instancing counts (`summarize-reskate-instances`) and,
/// without a stated region, content extent (`summarize-reskate-content`); a part without the layout summary is the
/// whole map, and a second one is refused. A render tile is drawn by the one part holding all of its placements (else
/// refused, with the tile and the placements of each part: partition aligned to the tile, `partition-grid{tile=T}`). A
/// collision cell is laid out once every part the summary announced for it has come, from the triangles of all of
/// them; a part adding to a cell it did not announce is refused. A part is told by the roots it holds; one holding
/// none of its roots any more (a pass removed them all) is not, and the cells waiting for it are laid out at the end.
pub struct MapSink {
    dir: PathBuf,
    encoding: EncodingOptions,
    taken: Option<Taken>,
}

impl std::fmt::Debug for MapSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapSink")
            .field("dir", &self.dir)
            .field("name", &self.encoding.name)
            .field("taken", &self.taken.as_ref().map(|t| (&t.out_dir, t.parts)))
            .finish()
    }
}

/// What the stats need of a collision soup of the scene: its place in the scene's order, its file, mode, triangles
/// read, cut by the region and kept.
struct SoupFacts {
    order: (bool, primase_core::Entity),
    file: String,
    mode: CollisionMode,
    read: usize,
    cut: i64,
    kept: i64,
}

/// What the map's index files and report need of its scene, over every part.
struct SceneFacts {
    dir: PathBuf,
    soups: Vec<SoupFacts>,
    stats: Option<serde_json::Value>,
    floor_plan: Option<crate::components::SkateFloorPlan>,
    stated_warnings: Vec<String>,
    notes: SceneNotes,
    /// The scene reader's warnings: given by the encoding (the same for every part), else found, merged.
    warnings: Result<Vec<String>, SceneWarnings>,
}

/// The map's markers, water, lights and grind curves, from every part: markers and water by entity (the scene's
/// order), each marker with whether it has a name of its own.
#[derive(Default)]
struct Content {
    markers: Vec<(primase_core::Entity, bool, Marker)>,
    water: Vec<(primase_core::Entity, WaterQuad)>,
    lights: Vec<PlacedLight>,
    grind: Vec<GrindCurve>,
}

/// A map in parts: the layout summary, which part each root is in, the parts each cell still waits for.
struct Layout {
    summary: SkateLayoutSummary,
    tag_of: HashMap<u64, u64>,
    cells_of: BTreeMap<u64, Vec<CellKey>>,
    waiting: BTreeMap<CellKey, BTreeSet<u64>>,
}

impl Layout {
    fn new(summary: &SkateLayoutSummary) -> Layout {
        let mut cells_of: BTreeMap<u64, Vec<CellKey>> = BTreeMap::new();
        for (c, parts) in &summary.cells {
            for &p in parts {
                cells_of.entry(p).or_default().push(*c);
            }
        }
        Layout {
            tag_of: summary
                .parts
                .iter()
                .flat_map(|(&t, roots)| roots.iter().map(move |&r| (r, t)))
                .collect(),
            cells_of,
            waiting: summary.cells.clone(),
            summary: summary.clone(),
        }
    }

    /// Which part `world` is, by the roots it holds; `None`: it holds none of its roots any more.
    fn part_of(&self, world: &World) -> Result<Option<u64>> {
        let mut tag = None;
        for e in world.entities() {
            if let Some(&t) = self.tag_of.get(&u64::from(e.index())) {
                match tag {
                    Some(seen) if seen != t => {
                        return Err(Error::Scene(format!(
                            "one part holds roots of the parts {seen} and {t} of the layout summary: the parts are \
                             not the ones it was made for"
                        )));
                    }
                    _ => tag = Some(t),
                }
            }
        }
        Ok(tag)
    }
}

/// How much collision the sink held between parts, and what it laid out at the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldCells {
    /// The most cells waiting for a part at once, and the most bytes of soups and props they held.
    pub peak_cells: usize,
    pub peak_bytes: usize,
    /// Cells laid out at the end, still waiting for a part that holds none of its roots any more (a map in parts), and
    /// those parts.
    pub at_finish: usize,
    pub parts_untold: usize,
}

/// What the sink keeps between parts: the writer (what it wrote, not its payloads), the first part's writer options
/// (every part's must be the same, the scene's notes aside) and preparation records, the scene's facts, the map's
/// markers, water, lights and grind curves, how many placements there were and how many prototypes failed, the names
/// the target does not take; for a map in parts its layout; how much collision it held.
struct Taken {
    out_dir: PathBuf,
    first: WriterOptions,
    records: Records,
    writer: MapWriter,
    scene: SceneFacts,
    content: Content,
    placements: usize,
    failed: i64,
    unsupported_names: BTreeMap<String, u64>,
    layout: Option<Layout>,
    parts: usize,
    held: HeldCells,
    /// The bytes of each mesh file written (the map's mesh bytes; a file a later decision removed taken out).
    mesh_sizes: BTreeMap<String, u64>,
}

/// The folder `d` made afresh (empty).
fn fresh_folder(d: &Path) -> Result<()> {
    if d.exists() {
        std::fs::remove_dir_all(d).map_err(|source| Error::Io {
            path: d.to_owned(),
            source,
        })?;
    }
    std::fs::create_dir_all(d).map_err(|source| Error::Io {
        path: d.to_owned(),
        source,
    })
}

/// The files `files` written into the folder `d`.
fn write_files(d: &Path, files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Result<()> {
    for (name, bytes) in files {
        let p = d.join(name);
        std::fs::write(&p, bytes).map_err(|source| Error::Io { path: p, source })?;
    }
    Ok(())
}

impl MapSink {
    /// A sink for the map encoded as `encoding`, its scene from `dir` (recorded in the stats).
    pub fn new(dir: &Path, encoding: &EncodingOptions) -> MapSink {
        MapSink {
            dir: dir.to_owned(),
            encoding: encoding.clone(),
            taken: None,
        }
    }

    /// Take the prepared part `world`: lay it out and write its payloads (see [`MapSink`]).
    pub fn part(&mut self, world: &World) -> Result<()> {
        let a = self.encoding.clone();
        let summary = world.resource::<SkateLayoutSummary>();
        if self.taken.as_ref().is_some_and(|t| t.layout.is_none()) {
            return Err(Error::Unsupported(
                "a second part of a map taken as whole (no layout summary, or one naming a single part): the parts \
                 of a map carry its layout summary (summarize-reskate-layout)"
                    .into(),
            ));
        }
        let prepared = target().check_prepared(world);
        let mut s = Scene::from_world(world, &self.dir)?;
        let warnings = match &a.scene_warnings {
            Some(w) => Ok(w.clone()),
            None => Err(SceneWarnings::of(&s, world)?),
        };
        // the writer's options, every part's the same: the region stated, else the content's (the map's, over every
        // part); the scene's notes are the map's, added at the end
        let mut o = part_options(world, &s, &a, true)?;
        let cut = s.region;
        let placements = writer_placements(&s, o.tile);
        o.region = match (cut, summary) {
            (Some(r), _) => r,
            (None, None) => bounds(&s, &placements),
            (None, Some(_)) => world
                .resource::<SkateContentBounds>()
                .ok_or_else(|| {
                    Error::Scene(
                        "a map in parts without a stated region takes its content's extent over every part: \
                         summarize-reskate-content"
                            .into(),
                    )
                })?
                .extent(),
        };
        o.warnings = s.stated_warnings.clone();
        let records = Records::of(world, &s, &a)?;
        // the map's instancing, in parts: its counts over every part, by prototype key
        let counts = match summary {
            None => None,
            Some(_) => {
                let c = world.resource::<SkateInstanceCounts>().ok_or_else(|| {
                    Error::Scene(
                        "a map in parts instances by its placements over every part: summarize-reskate-instances"
                            .into(),
                    )
                })?;
                let mut by_key: HashMap<u32, i64> = HashMap::new();
                for p in &s.protos {
                    let n = c
                        .by_prototype
                        .get(&u64::from(p.entity.index()))
                        .map_or(0, BTreeSet::len);
                    *by_key.entry(p.key).or_default() += n as i64;
                }
                Some(by_key)
            }
        };
        let out_dir = a.out_root.join(&o.name);
        if self.taken.is_none() {
            fresh_folder(&out_dir.join("meshes"))?;
            fresh_folder(&out_dir.join("textures"))?;
            let writer = records.clone().writer(o.clone(), s.surfaces.clone())?;
            self.taken = Some(Taken {
                out_dir: out_dir.clone(),
                first: o.clone(),
                records: records.clone(),
                writer,
                scene: SceneFacts {
                    dir: s.dir.clone(),
                    soups: Vec::new(),
                    stats: s.stats.clone(),
                    floor_plan: s.floor_plan.clone(),
                    stated_warnings: s.stated_warnings.clone(),
                    notes: SceneNotes::default(),
                    warnings: match &warnings {
                        Ok(w) => Ok(w.clone()),
                        Err(_) => Err(SceneWarnings::default()),
                    },
                },
                content: Content::default(),
                placements: 0,
                failed: 0,
                unsupported_names: BTreeMap::new(),
                layout: summary.filter(|l| l.parts.len() > 1).map(Layout::new),
                parts: 0,
                mesh_sizes: BTreeMap::new(),
                held: HeldCells::default(),
            });
        }
        let t = self.taken.as_mut().expect("taken");
        if o != t.first || records != t.records || s.surfaces != *t.writer.surfaces() {
            return Err(Error::Scene(
                "the parts of one map disagree on the writer's settings, the preparation records or the surface \
                 table: every part of a map is prepared and written alike"
                    .into(),
            ));
        }
        if let Some(l) = &t.layout
            && summary != Some(&l.summary)
        {
            return Err(Error::Scene(
                "the parts of one map carry different layout summaries".into(),
            ));
        }
        t.parts += 1;
        t.writer.set_counts(counts);
        // in parts: which part this is, and every tile it draws held whole by it
        let tiles: BTreeSet<(i32, i32)> = placements.iter().map(|p| (p.tx, p.ty)).collect();
        let tag = match &t.layout {
            None => None,
            Some(l) => {
                let tag = l.part_of(world)?;
                for tile in &tiles {
                    let by = l.summary.tiles.get(tile);
                    if tag.is_none()
                        || by.is_none_or(|by| by.len() != 1 || !by.contains_key(&tag.expect("told")))
                    {
                        return Err(Error::Scene(format!(
                            "render tile t_{}_{} is drawn by a part that does not hold all of its placements (by part, \
                             their planning ids: {:?}): partition the map aligned to the {} m tile \
                             (partition-grid{{tile={}}}, its cell a whole multiple)",
                            tile.0,
                            tile.1,
                            by.cloned().unwrap_or_default(),
                            o.tile,
                            o.tile
                        )));
                    }
                }
                tag
            }
        };
        // the part's collision: its world soups (each hull soup numbered among the map's) and its tiles' props
        let drop_below = f32::NEG_INFINITY;
        let tri_world = SoupWorld::new(&mut s, CollisionMode::TriangleMesh, cut, drop_below);
        let water_world = s
            .soups
            .iter()
            .any(|x| x.mode == CollisionMode::Water)
            .then(|| SoupWorld::new(&mut s, CollisionMode::Water, cut, drop_below));
        let ranks: Option<&BTreeSet<(bool, u64)>> = t.layout.as_ref().map(|l| &l.summary.hulls);
        let world_hulls = hulls_numbered(&mut s, cut, drop_below, &|sd, n| match ranks {
            None => Ok(n),
            Some(r) => {
                let key = (sd.raw, u64::from(sd.entity.index()));
                if r.contains(&key) {
                    Ok(r.range(..key).count())
                } else {
                    Err(Error::Scene(format!(
                        "hull soup {:?} is not in the map's layout summary",
                        sd.entity
                    )))
                }
            }
        })?;
        let mut contributed: BTreeSet<CellKey> = tri_world.cell_keys().map(|(x, y)| (false, x, y)).collect();
        if let Some(w) = &water_world {
            contributed.extend(w.cell_keys().map(|(x, y)| (true, x, y)));
        }
        let before: BTreeSet<CellKey> = t.writer.prop_cells_of(&tiles);
        t.writer.add_soups(tri_world, water_world);
        t.failed += assemble(&s, &placements, &mut t.writer)?;
        contributed.extend(t.writer.prop_cells_of(&tiles).difference(&before).copied());
        t.writer.lay_hulls(&world_hulls)?;
        let prop_hulls = t.writer.take_prop_hulls();
        t.writer.lay_hulls(&prop_hulls)?;
        // the cells it completes, laid out now: every cell, for a map written whole
        match &mut t.layout {
            None => {
                let waiting = t.writer.waiting_cells();
                t.writer.lay_cells(&waiting, None)?;
            }
            Some(l) => {
                for c in &contributed {
                    if !l
                        .summary
                        .cells
                        .get(c)
                        .is_some_and(|p| tag.is_some_and(|t| p.contains(&t)))
                    {
                        return Err(Error::Scene(format!(
                            "a part adds collision to the cell {c:?}, which the layout summary does not announce for \
                             it: content a pass added after the summary was made"
                        )));
                    }
                }
                let mut ready = BTreeSet::new();
                if let Some(tag) = tag {
                    for c in l.cells_of.get(&tag).into_iter().flatten() {
                        if let Some(w) = l.waiting.get_mut(c) {
                            w.remove(&tag);
                            if w.is_empty() {
                                l.waiting.remove(c);
                                ready.insert(*c);
                            }
                        }
                    }
                }
                let ready: BTreeSet<CellKey> =
                    t.writer.waiting_cells().intersection(&ready).copied().collect();
                t.writer.lay_cells(&ready, None)?;
                if tag.is_none() {
                    t.held.parts_untold += 1;
                }
            }
        }
        t.held.peak_cells = t.held.peak_cells.max(t.writer.waiting_cells().len());
        t.held.peak_bytes = t.held.peak_bytes.max(t.writer.waiting_bytes());
        // the part's payloads, written now and not kept
        let meshes = std::mem::take(&mut t.writer.mesh_files);
        t.mesh_sizes
            .extend(meshes.iter().map(|(f, b)| (f.clone(), b.len() as u64)));
        write_files(&out_dir.join("meshes"), meshes)?;
        let used = t.writer.used_textures();
        write_files(&out_dir.join("textures"), t.writer.ctx.tex.take_used(&used)?)?;
        // what the map's index files need of it
        for (name, n) in prepared.unsupported.into_iter().chain(
            prepared
                .unsupported_resources
                .into_iter()
                .map(|r| (format!("resource {r}"), 1)),
        ) {
            *t.unsupported_names.entry(name).or_default() += n;
        }
        t.placements += placements.len();
        t.scene.notes.merge(&s.notes);
        if let (Err(mine), Err(found)) = (&mut t.scene.warnings, warnings) {
            mine.merge(found);
        }
        t.scene.soups.extend(s.soups.iter().map(|sd| SoupFacts {
            order: (sd.raw, sd.entity),
            file: sd.file.clone(),
            mode: sd.mode,
            read: sd.surf.len(),
            cut: sd.cut,
            kept: sd.kept,
        }));
        let c = &mut t.content;
        c.markers.extend(
            world
                .iter::<crate::components::SkateMarker>()
                .map(|(e, _)| (e, world.has::<primase_core::Name>(e)))
                .zip(std::mem::take(&mut s.markers))
                .map(|((e, named), m)| (e, named, m)),
        );
        c.water.extend(
            world
                .iter::<crate::components::SkateWater>()
                .map(|(e, _)| e)
                .zip(std::mem::take(&mut s.water)),
        );
        c.lights.extend(std::mem::take(&mut s.lights));
        c.grind.extend(std::mem::take(&mut s.curves));
        Ok(())
    }

    /// The map laid out to the end (the cells still waiting, the water, the grind prisms, the lights and markers in
    /// the map's order) and its index files written; what the map writer did.
    pub fn finish(self) -> Result<BuildReport> {
        let Some(Taken {
            out_dir,
            mut writer,
            scene: mut s,
            content,
            placements,
            failed,
            unsupported_names,
            parts,
            mut held,
            mut mesh_sizes,
            ..
        }) = self.taken
        else {
            return Err(Error::Scene(
                "the map sink was given no part: a map needs its whole".into(),
            ));
        };
        held.at_finish = writer.waiting_cells().len();
        // the markers in the scene's order, those without a name of their own named as the scene names them
        let Content {
            mut markers,
            mut water,
            lights,
            grind,
        } = content;
        markers.sort_by_key(|(e, _, _)| *e);
        let mut spots = 0;
        for (_, named, m) in &mut markers {
            if !*named && m.kind != "spawn" {
                m.name = Some(format!("Spot {spots}"));
            }
            if matches!(m.kind, "travel" | "trainer") {
                spots += 1;
            }
        }
        water.sort_by_key(|(e, _)| *e);
        let (notes, omitted, approximated) = s.notes.render();
        writer.o.warnings = s.stated_warnings.iter().chain(&notes).cloned().collect();
        writer.finish(Extras {
            hulls: Vec::new(),
            water: water.into_iter().map(|(_, q)| q).collect(),
            lights,
            markers: markers.into_iter().map(|(_, _, m)| m).collect(),
            grind,
        })?;
        match &s.warnings {
            Ok(w) => writer.r.warnings.extend(w.iter().cloned()),
            Err(found) => writer.r.warnings.extend(found.render()),
        }
        // the payloads left (the last cells, water, grind prisms, textures not used before); files given out under a
        // name the texture no longer has renamed, files of a form not taken removed
        let meshes = std::mem::take(&mut writer.mesh_files);
        mesh_sizes.extend(meshes.iter().map(|(f, b)| (f.clone(), b.len() as u64)));
        write_files(&out_dir.join("meshes"), meshes)?;
        for f in &writer.discarded {
            mesh_sizes.remove(f);
            let p = out_dir.join("meshes").join(f);
            if p.exists() {
                std::fs::remove_file(&p).map_err(|source| Error::Io { path: p, source })?;
            }
        }
        let used = writer.used_textures();
        write_files(&out_dir.join("textures"), writer.ctx.tex.take_used(&used)?)?;
        for (given, now) in writer.ctx.tex.renamed() {
            let d = out_dir.join("textures");
            std::fs::rename(d.join(&given), d.join(&now)).map_err(|source| Error::Io {
                path: d.join(&given),
                source,
            })?;
        }
        // the soups in the scene's order, those placed by an entity of their own numbered among them
        s.soups.sort_by_key(|f| f.order);
        for (i, f) in s.soups.iter_mut().enumerate() {
            if f.order.0 {
                f.file = format!("collision/world_{i}.bin");
            }
        }
        writer.r.mesh_bytes = mesh_sizes.values().sum();
        let o = writer.o.clone();
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
        let stats = stats_json(&writer, &s, &o, placements, failed)?;
        let p = out_dir.join("stats.json");
        std::fs::write(&p, stats).map_err(|source| Error::Io { path: p, source })?;
        let r = &writer.r;
        Ok(BuildReport {
            out_dir,
            parts,
            held,
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
            peak_mesh_bytes: r.peak_mesh_bytes,
            lifts: r.lifts.clone(),
            blend: r.blend.clone(),
            warnings: o.warnings.iter().chain(&r.warnings).cloned().collect(),
            omitted: omitted
                .into_iter()
                .chain(
                    [
                        ("water outlines with no area", r.water_without_area as i64),
                        (
                            "clip-reskate-region: placements outside the region with nothing within its apron",
                            r.far_dropped,
                        ),
                        (
                            "instanced placements reaching past the map's limit (Studio's extent)",
                            r.limit_instances,
                        ),
                        ("triangles past the map's limit", r.limit_triangles),
                        (
                            "collision triangles past the map's limit less 20 m",
                            r.collision
                                .as_ref()
                                .map_or(0, |c| c.cut_beyond_limit + c.props_beyond_limit),
                        ),
                        ("placements with a degenerate matrix", r.degenerate),
                        (
                            "trainer spots without a height or past the 64 trainer.json takes",
                            r.trainer_left_out as i64,
                        ),
                        ("grind curves with no usable points", r.grind_unusable as i64),
                    ]
                    .into_iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(what, n)| (what.to_owned(), n as u64)),
                )
                .collect(),
            approximated: approximated
                .into_iter()
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
                            r.water_cut_at_limit as i64,
                        ),
                        (
                            "clip-reskate-region: placements outside the region trimmed to its apron",
                            r.apron_trimmed,
                        ),
                    ]
                    .into_iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(what, n)| (what.to_owned(), n as u64)),
                )
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
            unsupported_names: unsupported_names.into_iter().collect(),
        })
    }
}

fn stats_json(
    writer: &MapWriter,
    s: &SceneFacts,
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
                read: sd.read,
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
