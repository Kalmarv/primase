//! A skate-scene world -> a ReSkate Studio normalized map folder (the reference `scene-skate build`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::collision::{CollisionWorld, SRC_TILE, TileData};
use crate::map::{Ctx, Extras, MapWriter, MeshSource, WriterOptions};
use crate::model::{CollisionMode, Hull, Placement, SrcMesh, SrcPart};
use crate::records::{CollisionDropped, OverlayStack, SoupStats, StatsJson, Totals};
use crate::scene::{self, Materializer, MeshCounters, Scene, w_at};
use crate::texture::TexSource;

use crate::{Error, Result};

/// The options of `scene-skate build`.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// The map folder is `out_root/name`.
    pub out_root: PathBuf,
    pub name: Option<String>,
    pub tex_cap: u32,
    /// The caps file as given (its text is written into textures_index.json).
    pub tex_caps: Option<String>,
    pub tile: f32,
    pub min_instances: i64,
    pub min_instance_tris: i64,
    pub overlay_step: f32,
    pub piece_log: bool,
    pub limit: f32,
    pub map_height: usize,
    pub no_water: bool,
    pub water_ocean: bool,
    pub lights_max_per_cell: usize,
    pub max_render_edge: Option<f32>,
    pub tex_budget: Option<i32>,
    pub min_height: Option<f32>,
    pub stub_below: Option<f32>,
    pub floor: Option<f32>,
    pub offset: [f64; 2],
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            out_root: PathBuf::from("work/map"),
            name: None,
            tex_cap: 256,
            tex_caps: None,
            tile: 200.0,
            min_instances: 3,
            min_instance_tris: 150,
            overlay_step: 0.004,
            piece_log: false,
            limit: 4780.0,
            map_height: 2160,
            no_water: false,
            water_ocean: false,
            lights_max_per_cell: 0,
            max_render_edge: None,
            tex_budget: None,
            min_height: None,
            stub_below: None,
            floor: None,
            offset: [0.0, 0.0],
        }
    }
}

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
}

/// The scene's placements in the writer's frame (degenerate matrices left for the writer to count).
fn writer_placements(s: &Scene, tile: f32, region: Option<[f64; 4]>) -> Vec<Placement> {
    s.placements
        .iter()
        .enumerate()
        .map(|(pi, p)| {
            let m = &p.m;
            let proto = &s.protos[p.proto as usize];
            let mut pl = Placement {
                proto: proto.key,
                variant: p.variant,
                ex: w_at(m, 0),
                ez: w_at(m, 3),
                ey: -w_at(m, 6),
                pos: w_at(m, 9),
                no_collision: p.no_collision,
                no_instance: p.split,
                index: pi,
                split_whole: p.split && proto.glb.is_some(),
                ..Placement::default()
            };
            pl.outside = match p.clip {
                Some(c) => c,
                None => region.is_some_and(|r| {
                    let (x, y) = (f64::from(pl.pos.x), f64::from(pl.pos.y));
                    !(x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
                }),
            };
            match p.tile {
                Some(t) => {
                    pl.tx = t[0];
                    pl.ty = t[1];
                }
                None => {
                    pl.tx = (pl.pos.x / tile).floor() as i32;
                    pl.ty = (pl.pos.y / tile).floor() as i32;
                }
            }
            if let Some(r) = p.radius {
                pl.radius = r;
            } else if let (Some(mn), Some(mx)) = (proto.min, proto.max) {
                let mut reach = 0f32;
                for c in 0..8 {
                    let x = if c & 1 != 0 { mx[0] } else { mn[0] };
                    let y = if c & 2 != 0 { mx[1] } else { mn[1] };
                    let z = if c & 4 != 0 { mx[2] } else { mn[2] };
                    reach = reach.max((x * x + y * y + z * z).sqrt());
                }
                pl.radius = reach * pl.ex.length().max(pl.ey.length().max(pl.ez.length()));
            }
            pl
        })
        .collect()
}

/// The ground-frame box of the placements and soups.
fn bounds(s: &Scene, ps: &[Placement]) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut add = |x: f32, y: f32| {
        x0 = x0.min(f64::from(x));
        y0 = y0.min(f64::from(y));
        x1 = x1.max(f64::from(x));
        y1 = y1.max(f64::from(y));
    };
    let protos: HashMap<u32, &scene::ProtoDef> = s.protos.iter().map(|p| (p.key, p)).collect();
    for p in ps {
        add(p.pos.x, p.pos.y);
        let Some(d) = protos.get(&p.proto) else { continue };
        let (Some(mn), Some(mx)) = (d.min, d.max) else {
            continue;
        };
        for c in 0..8 {
            let q = scene::w(
                if c & 1 != 0 { mx[0] } else { mn[0] },
                if c & 2 != 0 { mx[1] } else { mn[1] },
                if c & 4 != 0 { mx[2] } else { mn[2] },
            );
            let w = p.pos + p.ex * q.x + p.ey * q.y + p.ez * q.z;
            add(w.x, w.y);
        }
    }
    for sd in &s.soups {
        for v in (0..sd.pos.len().saturating_sub(2)).step_by(3) {
            add(sd.pos[v], -sd.pos[v + 2]);
        }
    }
    if x0 > x1 {
        return [-100.0, -100.0, 100.0, 100.0];
    }
    [x0.floor(), y0.floor(), x1.floor() + 1.0, y1.floor() + 1.0]
}

/// World soups of one mode filed by triangle centre into 256 m cells (ground soups first in each cell).
#[derive(Debug)]
struct SoupWorld {
    cells: BTreeMap<(i32, i32), SoupCell>,
}

/// One cell's triangles (loose), surfaces, natural rock of the ground ones, and how many are ground.
#[derive(Debug, Default)]
struct SoupCell {
    v: Vec<f32>,
    s: Vec<u8>,
    rock: Vec<bool>,
    ground: usize,
}

impl SoupWorld {
    fn new(s: &mut Scene, mode: CollisionMode, region: Option<[f64; 4]>, drop_below: f32) -> SoupWorld {
        let mut cells: BTreeMap<(i32, i32), SoupCell> = BTreeMap::new();
        for pass in [true, false] {
            for sd in s.soups.iter_mut().filter(|x| x.mode == mode && x.ground == pass) {
                for t in 0..sd.surf.len() {
                    sd.read += 1;
                    let a = w_at(&sd.pos, sd.idx[t * 3] as usize * 3);
                    let b = w_at(&sd.pos, sd.idx[t * 3 + 1] as usize * 3);
                    let c = w_at(&sd.pos, sd.idx[t * 3 + 2] as usize * 3);
                    let cx = (a.x + b.x + c.x) / 3.0;
                    let cy = (a.y + b.y + c.y) / 3.0;
                    if sd.region_cut
                        && let Some(r) = region
                        && (f64::from(cx) < r[0]
                            || f64::from(cx) >= r[2]
                            || f64::from(cy) < r[1]
                            || f64::from(cy) >= r[3])
                    {
                        sd.cut += 1;
                        continue;
                    }
                    if a.z.max(b.z.max(c.z)) < drop_below {
                        sd.cut += 1;
                        continue;
                    }
                    let key = ((cx / SRC_TILE).floor() as i32, (cy / SRC_TILE).floor() as i32);
                    let cell = cells.entry(key).or_default();
                    cell.v
                        .extend_from_slice(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]);
                    let surf = sd.surf[t];
                    cell.s.push(surf & 0x7F);
                    if pass {
                        cell.rock.push(surf & 0x80 != 0);
                        cell.ground += 1;
                    }
                    sd.kept += 1;
                }
            }
        }
        SoupWorld { cells }
    }
}

impl CollisionWorld for SoupWorld {
    fn cells(&self) -> Vec<(i32, i32)> {
        self.cells.keys().copied().collect()
    }
    fn cell(&self, tx: i32, ty: i32) -> Option<TileData> {
        let c = self.cells.get(&(tx, ty))?;
        Some(TileData {
            v: c.v.clone(),
            t: (0..(c.s.len() * 3) as u32).collect(),
            s: c.s.clone(),
            rock: c.rock.clone(),
            ground: Some(c.ground),
            mask: None,
        })
    }
}

/// The hull soups, each whole.
fn hulls(s: &mut Scene, region: Option<[f64; 4]>, drop_below: f32) -> Vec<Hull> {
    let mut res = Vec::new();
    let mut n = 0;
    for sd in s.soups.iter_mut().filter(|x| x.mode == CollisionMode::Hull) {
        let mut pos = vec![0f32; sd.pos.len()];
        let (mut cx, mut cy, mut top) = (0f64, 0f64, f32::MIN);
        for v in (0..sd.pos.len()).step_by(3) {
            let w = w_at(&sd.pos, v);
            pos[v] = w.x;
            pos[v + 1] = w.y;
            pos[v + 2] = w.z;
            cx += f64::from(w.x);
            cy += f64::from(w.y);
            top = top.max(w.z);
        }
        let nv = (sd.pos.len() / 3).max(1) as f64;
        cx /= nv;
        cy /= nv;
        let tris = sd.surf.len() as i64;
        sd.read = tris;
        let outside = region.is_some_and(|r| cx < r[0] || cx >= r[2] || cy < r[1] || cy >= r[3]);
        if (sd.region_cut && outside) || top < drop_below {
            sd.cut = tris;
            n += 1;
            continue;
        }
        res.push(Hull {
            name: format!("hull_{n:04}"),
            pos,
            idx: sd.idx.clone(),
            surface: sd.surf.first().map_or(0, |b| (b & 0x7F) as usize),
            ground: sd.ground,
        });
        n += 1;
        sd.kept = tris;
    }
    res
}

/// The scene's prototypes, built when a band needs them.
struct Meshes<'a> {
    scene: &'a Scene,
    by_key: HashMap<u32, usize>,
    tried: HashSet<u32>,
    mat: Materializer,
    counters: MeshCounters,
}

impl MeshSource for Meshes<'_> {
    fn load(&mut self, band: &[Placement], meshes: &mut HashMap<u32, SrcMesh>, ctx: &mut Ctx) -> Result<()> {
        let mut want: Vec<u32> = band
            .iter()
            .map(|p| p.proto)
            .filter(|k| !meshes.contains_key(k) && !self.tried.contains(k))
            .collect();
        want.sort_unstable();
        want.dedup();
        for k in want {
            self.tried.insert(k);
            let p = &self.scene.protos[self.by_key[&k]];
            if let Some(m) = scene::build_mesh(
                self.scene,
                p,
                &mut self.mat,
                &mut ctx.mats,
                &mut ctx.tex,
                &mut self.counters,
            )? {
                meshes.insert(k, m);
            }
        }
        Ok(())
    }

    fn material_of(&mut self, part: &SrcPart, row: u32, ctx: &mut Ctx) -> Result<usize> {
        scene::material_of(self.scene, part, row, &mut self.mat, &mut ctx.mats, &mut ctx.tex)
    }
}

/// The scene reader's warnings as the reference build reports them: unused textures, primitives without normals,
/// unplaced prototypes and prototypes without geometry, too many travel points.
fn scene_warnings(s: &Scene, world: &World) -> Result<Vec<String>> {
    use primase_skate_scene::{SkateMaterial, SkateTexture};
    let mut out = Vec::new();
    let mut used: HashSet<primase_core::Entity> = HashSet::new();
    for (_, m) in world.iter::<SkateMaterial>() {
        used.extend(m.base.iter().chain(&m.normal).chain(&m.mask).copied());
    }
    for (e, t) in world.iter::<SkateTexture>() {
        if !used.contains(&e) {
            out.push(format!("texture '{}' is not used", t.key));
        }
    }
    for p in &s.protos {
        if let Some(glb) = &p.glb {
            for (i, prim) in scene::read_glb(glb, p.mesh)?.iter().enumerate() {
                if prim.nrm.is_none() {
                    out.push(format!(
                        "prototype '{}': primitive {i} has no NORMAL (flat normals are made)",
                        p.id
                    ));
                }
            }
        }
    }
    let mut placed = vec![false; s.protos.len()];
    for p in &s.placements {
        placed[p.proto as usize] = true;
    }
    for (p, placed) in s.protos.iter().zip(placed) {
        if !placed {
            out.push(format!("prototype '{}' is not placed", p.id));
        } else if p.glb.is_none() {
            out.push(format!(
                "prototype '{}' has no geometry; its placements draw nothing",
                p.id
            ));
        }
    }
    let travel = s.markers.iter().filter(|m| m.kind == "travel").count();
    if travel > 25 {
        out.push(format!("{travel} travel points: Studio takes at most 25"));
    }
    Ok(out)
}

/// Build the map folder of the scene in `world` (read from `dir`).
pub fn build(world: &World, dir: &Path, a: &BuildOptions) -> Result<BuildReport> {
    let mut s = Scene::from_world(world, dir)?;
    let scene_warnings = scene_warnings(&s, world)?;
    if s.placements.iter().any(|p| p.split) {
        return Err(Error::Unsupported("placements split by tile (flag 32)".into()));
    }
    let mut o = WriterOptions {
        name: a.name.clone().unwrap_or_else(|| s.name.clone()),
        display_name: s.display_name.clone(),
        generator: format!(
            "scene-skate 1 (normalized map from a scene interchange folder{})",
            s.generator.as_ref().map(|g| format!(", {g}")).unwrap_or_default()
        ),
        region: [0.0; 4],
        tile: a.tile,
        band_rows: 8,
        min_instances: a.min_instances,
        min_instance_tris: a.min_instance_tris,
        tex_cap: a.tex_cap,
        tex_caps: a.tex_caps.clone(),
        overlay_step: a.overlay_step,
        piece_log: a.piece_log,
        offset: [a.offset[0], a.offset[1], 0.0],
        limit: a.limit,
        map_height: a.map_height,
        water: !a.no_water,
        water_ocean: a.water_ocean,
        lights_max_per_cell: a.lights_max_per_cell,
        light_prefix: s.light_prefix.clone().unwrap_or_else(|| "light_".into()),
        streaming_distances: s.streaming.unwrap_or([50.0, 250.0, 500.0]),
        max_render_edge: a.max_render_edge.unwrap_or(s.max_render_edge),
        tex_budget: a.tex_budget.unwrap_or(s.tex_budget),
        tex_aux_weight: s.tex_aux_weight,
        trainer_note: s.trainer_note.clone(),
        warnings: s.warnings.clone(),
        pause_picture: None,
    };
    let sc = world
        .resource::<primase_skate_scene::SkateScene>()
        .expect("checked by Scene::from_world");
    if let Some(pm) = &sc.pause_map {
        let src = TexSource::open(&pm.image_file, None, None)?;
        let px = src
            .decode(0, src.width, src.height)?
            .ok_or_else(|| Error::Scene("pause_map: the image cannot be decoded".into()))?;
        o.pause_picture = Some((
            src.width as usize,
            src.height as usize,
            px,
            pm.bounds.map(f64::from),
        ));
    }
    let cut = s.region;
    let placements = writer_placements(&s, o.tile, cut);
    o.region = cut.unwrap_or_else(|| bounds(&s, &placements));
    let min_height = a.min_height.unwrap_or(s.min_height);
    if !min_height.is_nan() {
        return Err(Error::Unsupported("min_height (the lift to a floor)".into()));
    }
    let drop_below = f32::NEG_INFINITY;
    let lift = 0f32;
    o.offset = [a.offset[0], a.offset[1], f64::from(lift)];

    let out_dir = a.out_root.join(&o.name);
    let mut writer = MapWriter::new(o.clone(), s.surfaces.clone(), &out_dir.join("textures"))?;
    let tri_world = SoupWorld::new(&mut s, CollisionMode::TriangleMesh, cut, drop_below);
    let water_world = s
        .soups
        .iter()
        .any(|x| x.mode == CollisionMode::Water)
        .then(|| SoupWorld::new(&mut s, CollisionMode::Water, cut, drop_below));
    let hulls = hulls(&mut s, cut, drop_below);
    let mut meshes = Meshes {
        scene: &s,
        by_key: s.protos.iter().enumerate().map(|(i, p)| (p.key, i)).collect(),
        tried: HashSet::new(),
        mat: Materializer::new(),
        counters: MeshCounters::default(),
    };
    writer.tiles(&placements, &mut meshes)?;
    writer.finish(Extras {
        world: &tri_world,
        water_world: water_world.as_ref().map(|w| w as &dyn CollisionWorld),
        hulls,
        water: s.water.clone(),
        lights: s.lights.clone(),
        markers: s.markers.clone(),
        grind: s.curves.clone(),
    })?;
    let failed = meshes.counters.failed;
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
    writer.ctx.tex.save()?;
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
    let stats = stats_json(&writer, &s, &o, placements.len(), lift, failed)?;
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
    })
}

fn stats_json(
    writer: &MapWriter,
    s: &Scene,
    o: &WriterOptions,
    placements: usize,
    lift: f32,
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
        name: &o.name,
        scene: full.to_string_lossy().into_owned(),
        region: o.region,
        tile_size: o.tile,
        lift_m: lift,
        totals: Totals {
            placements,
            placements_degenerate: r.degenerate,
            placements_stubs_left_out: 0,
            placed_parts_stubs_left_out: 0,
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
            textures_flattened_by_budget: r.budget_flattened,
            texture_file_bytes: r.tex_bytes,
            texture_pixels: r.tex_pixels,
            mesh_file_bytes: r.mesh_bytes,
            textures_unsupported: writer.ctx.tex.unsupported,
            placements_with_empty_mesh: r.empty_mesh,
            lights_written: r.lights_written,
            lights_dropped_by_cell_cap: r.lights_dropped,
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
            step_m: o.overlay_step,
            overlay_pieces: ov.pieces,
            shifted: ov.shifted,
            water_shifted: ov.water_shifted,
            max_shift_m: ov.max_shift,
            order_cycles_broken: ov.cycles,
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
