//! The map writer's assembly of a prepared world, I/O-free: the encoding it is made for ([`EncodingOptions`]), the
//! placements and soups in the writer's frame, the tiles and prototypes the writer builds from the scene's meshes, and
//! the dry assemblies the preparation passes record their decisions from (the overlay stack, the blend chains, the
//! materials), so a pass and the writer share one calculation and no pass names the writer.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::collision::{Collision, CollisionResult, CollisionWorld, PropSoup, TileData};
use crate::components::{
    CellCount, CellKey, CellProps, CellSoup, CollisionSurfaces, CollisionWeld, PlannedLeaf, SkateCellCounts,
    SkateCellLayout, SkateCellShare, SkateCollisionPlan, SkatePausePicture,
};
use crate::map::{
    CellContent, Chains, Ctx, Extras, Layout, MapWriter, MeshSource, Stack, WriterOptions, lay_cell,
};
use crate::model::{CollisionMode, Hull, Placement, SrcMesh, SrcPart};
use crate::scene::{self, Materializer, MeshCounters, Scene, w_at};

use crate::{Error, Result};

/// How the writer encodes a prepared world ([`crate::write_map`]): where the map folder goes and the format's packing. None
/// of it chooses content.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodingOptions {
    /// The map folder is `out_root/name`.
    pub out_root: PathBuf,
    /// `None`: the scene's name.
    pub name: Option<String>,
    /// The edge (m) of the render tiles the writer merges placements on.
    pub tile: f32,
    /// A prototype placed at least this often, with at least this many triangles, is written as instances instead
    /// of merged into its tiles.
    pub min_instances: i64,
    pub min_instance_tris: i64,
    /// The longest render edge (m) a written triangle keeps before it is split; `None`: the scene's.
    pub max_render_edge: Option<f32>,
    /// The map's half extent (m): Studio's limit.
    pub limit: f32,
    /// Added to every written coordinate (east, north).
    pub offset: [f64; 2],
    /// The cull IDs the map may take (an ID per object per 200 m Studio cell it meets, and per light): the level's
    /// share of ReSkate 2.1.0-pre's pool ([`crate::map::CULL_ID_POOL`] x [`crate::map::CULL_ID_SHARE`]). In pool mode
    /// the blend chains take what the map's other objects leave of it.
    pub cull_ids: usize,
    /// The scene's warnings for the map's stats, taken before a pass left placements out ([`leave_out_optional`]):
    /// the reference reads them from the whole scene before its writer filters. `None`: from the world as given.
    pub scene_warnings: Option<Vec<String>>,
}

impl Default for EncodingOptions {
    fn default() -> Self {
        EncodingOptions {
            out_root: PathBuf::from("work/map"),
            name: None,
            tile: 200.0,
            min_instances: 3,
            min_instance_tris: 150,
            max_render_edge: None,
            limit: 4780.0,
            offset: [0.0, 0.0],
            cull_ids: (crate::map::CULL_ID_POOL as f64 * crate::map::CULL_ID_SHARE) as usize,
            scene_warnings: None,
        }
    }
}

/// The scene's placements in the writer's frame (degenerate matrices left for the writer to count).
pub(crate) fn writer_placements(s: &Scene, tile: f32) -> Vec<Placement> {
    s.placements
        .iter()
        .enumerate()
        .map(|(pi, p)| {
            let m = &p.m;
            let proto = &s.protos[p.proto as usize];
            let mut pl = Placement {
                entity: Some(p.entity),
                proto: proto.key,
                variant: p.variant,
                ex: w_at(m, 0),
                ez: w_at(m, 3),
                ey: -w_at(m, 6),
                pos: w_at(m, 9),
                no_collision: p.no_collision,
                no_instance: p.split,
                index: pi,
                split_whole: p.split && proto.prims.is_some(),
                part_off: p.parts_off.clone(),
                ..Placement::default()
            };
            // clipped as the placement says (clip-reskate-region marks them); the writer decides none
            pl.outside = p.clip.unwrap_or(false);
            // the tile it states, else the one its ground position stands in (the rule partition-grid{tile} aligns to)
            let t = p.tile.unwrap_or_else(|| {
                let t = primase_core::Tile::of([pl.pos.x, pl.pos.y], tile);
                [t.col, t.row]
            });
            (pl.tx, pl.ty) = (t[0], t[1]);
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
pub(crate) fn bounds(s: &Scene, ps: &[Placement]) -> [f64; 4] {
    content_bounds(s, ps, &|_| true).extent()
}

/// The ground box of the content `owned` accepts (placements and world soups by their entity), as [`bounds`] measures
/// it, unrounded.
pub(crate) fn content_bounds(
    s: &Scene,
    ps: &[Placement],
    owned: &dyn Fn(primase_core::Entity) -> bool,
) -> crate::components::SkateContentBounds {
    let mut b = crate::components::SkateContentBounds::EMPTY;
    let mut add = |x: f32, y: f32| b.add(f64::from(x), f64::from(y));
    let protos: HashMap<u32, &scene::ProtoDef> = s.protos.iter().map(|p| (p.key, p)).collect();
    for p in ps.iter().filter(|p| p.entity.is_none_or(owned)) {
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
    for sd in s.soups.iter().filter(|sd| owned(sd.entity)) {
        for v in (0..sd.pos.len().saturating_sub(2)).step_by(3) {
            add(sd.pos[v], -sd.pos[v + 2]);
        }
    }
    b
}

/// World soups of one mode filed by triangle centre into 256 m cells, each cell's triangles kept by soup. A cell is
/// laid out with its ground soups' triangles first, each soup's in the scene's soup order ([`SoupSeg::order`]): a
/// cell whose soups arrive from several parts ([`SoupWorld::absorb`]) is laid out as the whole map's.
#[derive(Debug, Default)]
pub(crate) struct SoupWorld {
    cells: BTreeMap<(i32, i32), Vec<SoupSeg>>,
}

/// One soup's triangles in one cell (loose), their surfaces, and (a ground soup's) natural rock.
#[derive(Clone, Debug)]
pub(crate) struct SoupSeg {
    /// Its soup's place in the scene's order: raised soups before raw ones ([`scene::SoupDef::raw`]), each by entity.
    pub order: (bool, primase_core::Entity),
    pub ground: bool,
    pub v: Vec<f32>,
    pub s: Vec<u8>,
    pub rock: Vec<bool>,
}

impl SoupWorld {
    /// The cells it files triangles in.
    pub(crate) fn cell_keys(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.cells.keys().copied()
    }

    /// Cell `key`'s soups, taken out.
    pub(crate) fn take_cell(&mut self, key: (i32, i32)) -> Vec<SoupSeg> {
        self.cells.remove(&key).unwrap_or_default()
    }

    /// Another part's soups added, cell by cell.
    pub(crate) fn absorb_all(&mut self, o: SoupWorld) {
        for (k, segs) in o.cells {
            self.cells.entry(k).or_default().extend(segs);
        }
    }
}

impl SoupWorld {
    pub(crate) fn new(
        s: &mut Scene,
        mode: CollisionMode,
        region: Option<[f64; 4]>,
        drop_below: f32,
    ) -> SoupWorld {
        let mut cells: BTreeMap<(i32, i32), Vec<SoupSeg>> = BTreeMap::new();
        for pass in [true, false] {
            for sd in s.soups.iter_mut().filter(|x| x.mode == mode && x.ground == pass) {
                let order = (sd.raw, sd.entity);
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
                    let key = crate::collision::triangle_cell(a, b, c);
                    let segs = cells.entry(key).or_default();
                    if segs.last().is_none_or(|g| g.order != order) {
                        segs.push(SoupSeg {
                            order,
                            ground: pass,
                            v: Vec::new(),
                            s: Vec::new(),
                            rock: Vec::new(),
                        });
                    }
                    let seg = segs.last_mut().expect("pushed");
                    seg.v
                        .extend_from_slice(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]);
                    let surf = sd.surf[t];
                    seg.s.push(surf & 0x7F);
                    if pass {
                        seg.rock.push(surf & 0x80 != 0);
                    }
                    sd.kept += 1;
                }
            }
        }
        SoupWorld { cells }
    }
}

/// A cell's world triangles from its soups: the ground soups' first, each kind's in the scene's soup order.
pub(crate) fn soup_cell(segs: &[SoupSeg]) -> Option<TileData> {
    if segs.is_empty() {
        return None;
    }
    let mut order: Vec<&SoupSeg> = segs.iter().collect();
    order.sort_by_key(|g| (!g.ground, g.order));
    let mut td = TileData {
        v: Vec::new(),
        t: Vec::new(),
        s: Vec::new(),
        rock: Vec::new(),
        ground: Some(0),
        mask: None,
    };
    let mut ground = 0;
    for g in order {
        td.v.extend_from_slice(&g.v);
        td.s.extend_from_slice(&g.s);
        if g.ground {
            td.rock.extend_from_slice(&g.rock);
            ground += g.s.len();
        }
    }
    td.t = (0..(td.s.len() * 3) as u32).collect();
    td.ground = Some(ground);
    Some(td)
}

impl CollisionWorld for SoupWorld {
    fn cells(&self) -> Vec<(i32, i32)> {
        self.cells.keys().copied().collect()
    }
    fn cell(&self, tx: i32, ty: i32) -> Option<TileData> {
        soup_cell(self.cells.get(&(tx, ty))?)
    }
}

/// The hull soups, each whole, named by their place among the scene's hull soups.
pub(crate) fn hulls(s: &mut Scene, region: Option<[f64; 4]>, drop_below: f32) -> Vec<Hull> {
    hulls_numbered(s, region, drop_below, &|_, n| Ok(n)).expect("numbered in order")
}

/// The hull soups, each whole, hull `n` of the scene named `hull_<number(soup, n)>`: a part of a map numbers its hull
/// soups among the whole map's.
pub(crate) fn hulls_numbered(
    s: &mut Scene,
    region: Option<[f64; 4]>,
    drop_below: f32,
    number: &dyn Fn(&scene::SoupDef, usize) -> Result<usize>,
) -> Result<Vec<Hull>> {
    let mut res = Vec::new();
    let mut n = 0;
    for sd in s.soups.iter_mut().filter(|x| x.mode == CollisionMode::Hull) {
        let number = number(sd, n)?;
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
            entity: sd.entity,
            name: format!("hull_{number:04}"),
            pos,
            idx: sd.idx.clone(),
            surface: sd.surf.first().map_or(0, |b| (b & 0x7F) as usize),
            ground: sd.ground,
        });
        n += 1;
        sd.kept = tris;
    }
    Ok(res)
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
            if let Some(m) = scene::build_mesh(self.scene, p, &mut self.mat, &mut ctx.mats, &mut ctx.tex)? {
                meshes.insert(k, m);
            }
        }
        Ok(())
    }

    fn material_of(&mut self, part: &SrcPart, row: u32, ctx: &mut Ctx) -> Result<usize> {
        scene::material_of(self.scene, part, row, &mut self.mat, &mut ctx.mats, &mut ctx.tex)
    }
}

/// The writer's options for the scene `s`: its encoding `a` and the choices `c` it still makes; the region and the
/// pause picture are the caller's to set. Refuses placements split by tile.
pub(crate) fn writer_options(s: &Scene, a: &EncodingOptions) -> Result<WriterOptions> {
    if s.placements.iter().any(|p| p.split) {
        return Err(Error::Unsupported("placements split by tile (flag 32)".into()));
    }
    Ok(WriterOptions {
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
        offset: [a.offset[0], a.offset[1], 0.0],
        apron: 0.0,
        cull_ids: a.cull_ids,
        limit: a.limit,
        light_prefix: s.light_prefix.clone().unwrap_or_else(|| "light_".into()),
        streaming_distances: s.streaming.unwrap_or([50.0, 250.0, 500.0]),
        max_render_edge: a.max_render_edge.unwrap_or(s.max_render_edge),
        trainer_note: s.trainer_note.clone(),
        warnings: s.warnings.clone(),
        pause: None,
    })
}

/// The apron `clip-reskate-region` recorded around the scene's region (0 when nothing is clipped). Refused: a record
/// for another region than the scene's, and placements marked outside the region with no record of its apron.
pub(crate) fn apron_of(world: &World, s: &Scene) -> Result<f32> {
    match world.resource::<crate::components::SkateRegionClip>() {
        Some(c) => {
            let mine = s.region.map(|r| r.map(|v| v as f32));
            if mine != Some(c.region) {
                return Err(Error::Scene(format!(
                    "the region clip (skate.region_clip) was made for the region {:?}, but the scene's is {mine:?}",
                    c.region
                )));
            }
            Ok(c.apron)
        }
        None if s.placements.iter().any(|p| p.clip == Some(true)) => Err(Error::Scene(
            "placements are marked outside the region, but no apron was recorded: run clip-reskate-region"
                .into(),
        )),
        None => Ok(0.0),
    }
}

/// The writer's tiles and prototypes of `placements` (the scene's, [`writer_placements`]), assembled by `writer` from
/// the scene's prototypes: what `write_map` writes and `prepare-reskate-overlays` stacks. The prototypes that failed
/// to build, counted.
pub(crate) fn assemble(s: &Scene, placements: &[Placement], writer: &mut MapWriter) -> Result<i64> {
    let mut meshes = Meshes {
        scene: s,
        by_key: s.protos.iter().enumerate().map(|(i, p)| (p.key, i)).collect(),
        tried: HashSet::new(),
        mat: Materializer::new(),
        counters: MeshCounters::default(),
    };
    writer.tiles(placements, &mut meshes)?;
    Ok(meshes.counters.failed)
}

/// The writer settings an overlay stack depends on, for the scene `s` written as `a` says.
pub(crate) fn grouping(s: &Scene, a: &EncodingOptions) -> crate::components::OverlayGrouping {
    crate::components::OverlayGrouping {
        tile: a.tile,
        min_instances: a.min_instances,
        min_instance_tris: a.min_instance_tris,
        max_render_edge: a.max_render_edge.unwrap_or(s.max_render_edge),
        limit: a.limit,
        offset: a.offset,
    }
}

/// The writer's assembly of the prepared world `world` for the encoding `a`: its tiles and prototypes only, no mesh
/// encoded and no collision laid out ([`Layout::Render`]), with the overlay stack `stack` and textures encoded or only
/// identified (`encode`).
fn assembly(
    world: &World,
    a: &EncodingOptions,
    stack: Stack,
    chains: Chains,
    encode: bool,
) -> Result<MapWriter> {
    let PartRead {
        scene: s,
        o,
        placements,
        counts,
        ..
    } = read_part(world, Path::new(""), a, false)?;
    let mut writer = MapWriter::new(o, s.surfaces.clone())?
        .with_stack(stack)
        .with_chains(chains)
        .with_layout(Layout::Render);
    writer.set_counts(counts);
    if !encode {
        writer = writer.without_encoding();
    }
    assemble(&s, &placements, &mut writer)?;
    Ok(writer)
}

/// The overlay stack of the prepared world `world` (`prepare-reskate-overlays`'s): the writer's own assembly of its
/// meshes for the encoding `a`, textures identified but not encoded, each mesh's overlays stacked by `step` and
/// recorded by piece.
pub(crate) fn overlay_stack(
    world: &World,
    a: &EncodingOptions,
    step: f32,
) -> Result<crate::components::SkateOverlayStack> {
    let s = Scene::from_world(world, Path::new(""))?;
    let rec = crate::components::SkateOverlayStack {
        step,
        grouping: Some(grouping(&s, a)),
        ..Default::default()
    };
    match assembly(
        world,
        a,
        Stack::Compute(Box::new(rec)),
        Chains::Off { left: 0 },
        false,
    )?
    .take_stack()
    {
        Stack::Compute(rec) => Ok(*rec),
        _ => unreachable!("the stack it was given"),
    }
}

/// The blend chains of the prepared world `world` (`prepare-reskate-blend-order`'s): the writer's own assembly of its
/// tiles for the encoding `a`, textures identified but not encoded, each tile's chain overlays cut on a `grid`-metre
/// grid and lifted by rank, recorded by tile.
pub(crate) fn blend_chains(
    world: &World,
    a: &EncodingOptions,
    grid: f32,
) -> Result<crate::components::SkateBlendChains> {
    let s = Scene::from_world(world, Path::new(""))?;
    let rec = crate::components::SkateBlendChains {
        grid,
        grouping: Some(grouping(&s, a)),
        ..Default::default()
    };
    let chains = Chains::Record(Box::new(rec));
    match assembly(world, a, Stack::Off { unstacked: 0 }, chains, false)?.take_chains() {
        Chains::Record(rec) => Ok(*rec),
        _ => unreachable!("the chains it was given"),
    }
}

/// One material of the writer's assembly: its id, name and texture ids (base, normal, mask, alpha; -1: none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledMaterial {
    pub id: usize,
    pub name: String,
    pub textures: [i32; 4],
}

/// The materials the writer's assembly of the prepared world `world` makes for the encoding `a`, in id order, with
/// its textures encoded (`encode`: as `write_map` makes them) or only identified (as `prepare-reskate-overlays` makes
/// them): the two must agree, or the overlay stack would be made for other meshes than the writer's.
pub fn assembled_materials(
    world: &World,
    a: &EncodingOptions,
    encode: bool,
) -> Result<Vec<AssembledMaterial>> {
    let writer = assembly(
        world,
        a,
        Stack::Off { unstacked: 0 },
        Chains::Off { left: 0 },
        encode,
    )?;
    Ok(writer
        .ctx
        .mats
        .list
        .iter()
        .map(|m| AssembledMaterial {
            id: m.id,
            name: m.name.clone(),
            textures: [m.tex, m.normal, m.mask, m.alpha],
        })
        .collect())
}

/// The scene reader's warnings as the reference build reports them: unused textures, primitives without normals,
/// unplaced prototypes and prototypes without geometry, too many travel points.
pub(crate) fn scene_warnings(s: &Scene, world: &World) -> Result<Vec<String>> {
    Ok(SceneWarnings::of(s, world)?.render())
}

/// The scene reader's warnings, as what they are judged from, so the parts of a map merge into the whole map's
/// ([`SceneWarnings::merge`]) before they are judged: a texture is unused when no material anywhere uses it, a prototype
/// unplaced when no placement anywhere places it (a part's own view of either is not the map's); the travel points
/// counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneWarnings {
    /// Texture keys present, and those a material uses.
    textures: BTreeSet<String>,
    textures_used: BTreeSet<String>,
    /// Prototype ids present, those a placement places, and those without geometry.
    protos: BTreeSet<String>,
    protos_placed: BTreeSet<String>,
    no_geometry: BTreeSet<String>,
    /// Primitives without normals, as their warnings.
    flat: BTreeSet<String>,
    travel: usize,
}

impl SceneWarnings {
    /// What `world` (its scene `s`) holds of them.
    pub(crate) fn of(s: &Scene, world: &World) -> Result<SceneWarnings> {
        use crate::components::{SkateMaterial, SkateTexture};
        let mut used: HashSet<primase_core::Entity> = HashSet::new();
        for (_, m) in world.iter::<SkateMaterial>() {
            used.extend(m.base.iter().chain(&m.normal).chain(&m.mask).copied());
        }
        let mut w = SceneWarnings::default();
        for (e, t) in world.iter::<SkateTexture>() {
            w.textures.insert(t.key.clone());
            if used.contains(&e) {
                w.textures_used.insert(t.key.clone());
            }
        }
        for p in &s.protos {
            w.protos.insert(p.id.clone());
            if p.prims.is_none() {
                w.no_geometry.insert(p.id.clone());
            }
            if let Some(prims) = p.prims() {
                for (i, prim) in prims.iter().enumerate() {
                    if prim.nrm.is_none() {
                        w.flat.insert(format!(
                            "prototype '{}': primitive {i} has no NORMAL (flat normals are made)",
                            p.id
                        ));
                    }
                }
            }
        }
        for p in &s.placements {
            w.protos_placed.insert(s.protos[p.proto as usize].id.clone());
        }
        w.travel = s.markers.iter().filter(|m| m.kind == "travel").count();
        Ok(w)
    }

    /// Another part's added.
    pub(crate) fn merge(&mut self, o: SceneWarnings) {
        self.textures.extend(o.textures);
        self.textures_used.extend(o.textures_used);
        self.protos.extend(o.protos);
        self.protos_placed.extend(o.protos_placed);
        self.no_geometry.extend(o.no_geometry);
        self.flat.extend(o.flat);
        self.travel += o.travel;
    }

    /// Each kind in a stable order (by its text): unused textures, primitives without normals, unplaced prototypes and
    /// placed ones without geometry; then the travel points when Studio takes fewer.
    pub(crate) fn render(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .textures
            .difference(&self.textures_used)
            .map(|k| format!("texture '{k}' is not used"))
            .collect();
        out.extend(self.flat.iter().cloned());
        let mut protos: Vec<String> = self
            .protos
            .iter()
            .filter_map(|id| {
                if !self.protos_placed.contains(id) {
                    Some(format!("prototype '{id}' is not placed"))
                } else if self.no_geometry.contains(id) {
                    Some(format!(
                        "prototype '{id}' has no geometry; its placements draw nothing"
                    ))
                } else {
                    None
                }
            })
            .collect();
        protos.sort();
        out.extend(protos);
        if self.travel > 25 {
            out.push(format!("{} travel points: Studio takes at most 25", self.travel));
        }
        out
    }
}

/// What the writer lays out of a prepared world before it emits anything (what a pass asks of the writer's layout):
/// the scene in its frame, the writer with its tiles and prototypes assembled and the world soups given it, the hulls.
pub(crate) struct Laid {
    pub scene: Scene,
    pub writer: MapWriter,
    pub hulls: Vec<Hull>,
}

impl Laid {
    /// What the writer finishes the map with, beside its tiles.
    pub(crate) fn extras(&mut self) -> (&mut MapWriter, Extras) {
        (
            &mut self.writer,
            Extras {
                hulls: std::mem::take(&mut self.hulls),
                water: self.scene.water.clone(),
                lights: self.scene.lights.clone(),
                markers: self.scene.markers.clone(),
                grind: self.scene.curves.clone(),
            },
        )
    }
}

/// The writer's layout of the prepared world `world` (`dir`: where its scene came from) for the encoding `a`, its
/// textures encoded or only identified (`encode`): the scene read, the preparation records checked against this
/// encoding (marker snaps, overlay stack, blend chains), the tiles and prototypes assembled. What [`crate::write_map`]
/// emits and `summarize-reskate-map` counts.
pub(crate) fn lay(world: &World, a: &EncodingOptions, layout: Layout) -> Result<Laid> {
    let PartRead {
        scene: mut s,
        o,
        cut,
        placements,
        records,
        counts,
    } = read_part(world, Path::new(""), a, false)?;
    let mut writer = records
        .writer(o, s.surfaces.clone())?
        .without_encoding()
        .with_layout(layout);
    writer.set_counts(counts);
    lay_part(&mut s, cut, &placements, &mut writer)?;
    let hulls = if layout.collides() {
        hulls(&mut s, cut, f32::NEG_INFINITY)
    } else {
        Vec::new()
    };
    Ok(Laid {
        scene: s,
        writer,
        hulls,
    })
}

/// A prepared world (a part of a map, or the whole) as the map writer reads it for the encoding `a` (`dir`: where its
/// scene came from; `encode`: its textures encoded, else only identified): what [`read_part`] gives.
pub(crate) struct PartRead {
    pub scene: Scene,
    /// The writer's options: the region stated, else the content's (a part of a map in parts: the map's, merged over
    /// every part, `skate.content_bounds`).
    pub o: WriterOptions,
    /// The region the scene states, which its soups are cut by.
    pub cut: Option<[f64; 4]>,
    /// Its placements in the writer's tiles.
    pub placements: Vec<Placement>,
    /// The preparation records the writer replays.
    pub records: Records,
    /// The map's instancing by prototype key (a part of a map in parts: over every part); `None`: these placements'.
    pub counts: Option<HashMap<u32, i64>>,
}

/// `world` as the map writer reads it for the encoding `a`: the one reading of a prepared part that the map sink, the
/// dry layouts the passes count and plan from ([`lay`]), and the collision a part shares ([`cell_shares`]) all make.
pub(crate) fn read_part(world: &World, dir: &Path, a: &EncodingOptions, encode: bool) -> Result<PartRead> {
    let scene = Scene::from_world(world, dir)?;
    let mut o = part_options(world, &scene, a, encode)?;
    let cut = scene.region;
    let placements = writer_placements(&scene, o.tile);
    let in_parts = world
        .resource::<crate::components::SkateLayoutSummary>()
        .is_some();
    o.region = map_region(world, &scene, &placements, in_parts)?;
    let records = Records::of(world, &scene, a)?;
    let counts = map_counts(world, &scene, in_parts)?;
    Ok(PartRead {
        scene,
        o,
        cut,
        placements,
        records,
        counts,
    })
}

/// A part's world soups (triangle mesh and water, cut by `cut`) and its placements (`placements`, of its scene `s`) laid
/// into `writer`: their cells' soups and props held in it, its tiles and prototypes assembled. The hulls are the
/// caller's. What the map sink lays a part out with, and what a part shares of its collision. The prototypes that
/// failed to build, counted.
pub(crate) fn lay_part(
    s: &mut Scene,
    cut: Option<[f64; 4]>,
    placements: &[Placement],
    writer: &mut MapWriter,
) -> Result<i64> {
    // the floor is prepare-reskate-floor's: the world comes lifted, its parts below the cutoff marked or gone; a
    // render layout files no soup
    if writer.layout().collides() {
        let drop_below = f32::NEG_INFINITY;
        let tri_world = SoupWorld::new(s, CollisionMode::TriangleMesh, cut, drop_below);
        let water_world = s
            .soups
            .iter()
            .any(|x| x.mode == CollisionMode::Water)
            .then(|| SoupWorld::new(s, CollisionMode::Water, cut, drop_below));
        writer.add_soups(tri_world, water_world);
    }
    assemble(s, placements, writer)
}

/// The writer's region for a prepared world (its scene `s`, its `placements`): the region the scene states; else the
/// content's, the whole map's merged over every part when given (`skate.content_bounds`), else these placements'. A
/// part of a map in several (`in_parts`) without either is refused.
pub(crate) fn map_region(
    world: &World,
    s: &Scene,
    placements: &[Placement],
    in_parts: bool,
) -> Result<[f64; 4]> {
    if let Some(r) = s.region {
        return Ok(r);
    }
    match world.resource::<crate::components::SkateContentBounds>() {
        Some(b) => Ok(b.extent()),
        None if !in_parts => Ok(bounds(s, placements)),
        None => Err(Error::Scene(
            "a map in parts without a stated region takes its content's extent over every part: \
             summarize-reskate-content"
                .into(),
        )),
    }
}

/// The map's instancing by prototype key, its placements merged over every part (`skate.instance_counts`): `None` when
/// not given and the world is the whole map; a part of a map in several (`in_parts`) without them is refused.
pub(crate) fn map_counts(world: &World, s: &Scene, in_parts: bool) -> Result<Option<HashMap<u32, i64>>> {
    let Some(c) = world.resource::<crate::components::SkateInstanceCounts>() else {
        return if in_parts {
            Err(Error::Scene(
                "a map in parts instances by its placements over every part: summarize-reskate-instances"
                    .into(),
            ))
        } else {
            Ok(None)
        };
    };
    let mut by_key: HashMap<u32, i64> = HashMap::new();
    for p in &s.protos {
        let n = c
            .by_prototype
            .get(&u64::from(p.entity.index()))
            .map_or(0, BTreeSet::len);
        *by_key.entry(p.key).or_default() += n as i64;
    }
    Ok(Some(by_key))
}

/// The writer's options for a prepared world (its scene `s`) encoded as `a`: the encoding's, the apron its region
/// clip recorded, the pause picture (when encoding: a count draws none, it adds no object), checked against the
/// settings its markers were snapped for. The region is the caller's to set.
pub(crate) fn part_options(
    world: &World,
    s: &Scene,
    a: &EncodingOptions,
    encode: bool,
) -> Result<WriterOptions> {
    let mut o = writer_options(s, a)?;
    o.apron = apron_of(world, s)?;
    if encode {
        o.pause = world.resource::<SkatePausePicture>().cloned();
    }
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
    Ok(o)
}

/// What the preparation recorded for the writer to replay, each checked against the writer settings it was made for:
/// the overlay stack (`prepare-reskate-overlays`), the blend chains (`prepare-reskate-blend-order`), their budget
/// (`plan-reskate-blend-budget`), the collision plan (`prepare-reskate-collision`); `None`: not recorded. Every part
/// of a map carries the same.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Records {
    pub stack: Option<crate::components::SkateOverlayStack>,
    pub chains: Option<crate::components::SkateBlendChains>,
    pub budget: Option<crate::components::SkateBlendBudget>,
    pub plan: Option<crate::components::SkateCollisionPlan>,
}

/// What the parts of one map must share of their records ([`Records::basis`]): the collision plan and the blend budget
/// (each part's whole), the overlay stack's and the blend chains' settings (each part's own meshes and tiles).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecordsBasis {
    pub plan: Option<crate::components::SkateCollisionPlan>,
    pub budget: Option<crate::components::SkateBlendBudget>,
    pub stack: Option<(f32, Option<crate::components::OverlayGrouping>)>,
    pub chains: Option<(f32, Option<crate::components::OverlayGrouping>)>,
}

impl Records {
    /// What every part of one map must share of its records: parts prepared alike have equal bases. A sink keeps this,
    /// not the records (an overlay stack and blend chains are a part's own, and replayed as they come).
    pub(crate) fn basis(&self) -> RecordsBasis {
        RecordsBasis {
            plan: self.plan.clone(),
            budget: self.budget.clone(),
            stack: self.stack.as_ref().map(|s| (s.step, s.grouping)),
            chains: self.chains.as_ref().map(|c| (c.grid, c.grouping)),
        }
    }

    /// The records of `world` (its scene `s`), for the encoding `a`.
    pub(crate) fn of(world: &World, s: &Scene, a: &EncodingOptions) -> Result<Records> {
        let mine = grouping(s, a);
        // the overlay stack prepare-reskate-overlays made, for this grouping only; without one, none
        let stack = world.resource::<crate::components::SkateOverlayStack>().cloned();
        if let Some(rec) = &stack
            && rec.grouping != Some(mine)
        {
            return Err(Error::Scene(format!(
                "the overlays were stacked for the writer settings {:?}, but the writer's are {mine:?}: give \
                 prepare-reskate-overlays the writer's",
                rec.grouping
            )));
        }
        // the blend chains prepare-reskate-blend-order chose, for this grouping only; without them, none
        let chains = world.resource::<crate::components::SkateBlendChains>().cloned();
        if let Some(rec) = &chains
            && rec.grouping != Some(mine)
        {
            return Err(Error::Scene(format!(
                "the blend chains were chosen for the writer settings {:?}, but the writer's are {mine:?}: give \
                 prepare-reskate-blend-order the writer's",
                rec.grouping
            )));
        }
        Ok(Records {
            stack,
            chains,
            budget: world.resource::<crate::components::SkateBlendBudget>().cloned(),
            plan: world.resource::<crate::components::SkateCollisionPlan>().cloned(),
        })
    }

    /// A writer with options `o` and the surface table `surfaces`, replaying these records.
    pub(crate) fn writer(self, o: WriterOptions, surfaces: crate::scene::SurfaceTable) -> Result<MapWriter> {
        Ok(MapWriter::new(o, surfaces)?
            .with_stack(match self.stack {
                Some(rec) => Stack::Replay(Box::new(rec)),
                None => Stack::Off { unstacked: 0 },
            })
            .with_chains(match self.chains {
                Some(rec) => Chains::Replay(Box::new(rec)),
                None => Chains::Off { left: 0 },
            })
            .with_budget(self.budget)
            .with_collision_plan(self.plan))
    }
}

/// The summary of the prepared world `world` for the encoding `a` (`summarize-reskate-map`'s): the cull IDs the writer's
/// own layout of it takes besides the blend chains, counted as [`MapWriter::finish`] counts them, with nothing encoded,
/// its collision cells as `cells` says (apart: left unlaid, so uncounted), its shared content by `key` (each entity's
/// key over the map's parts); and its tiles with a chain form.
pub(crate) fn map_summary(
    world: &World,
    a: &EncodingOptions,
    cells: crate::components::CellsCounted,
    key: &dyn Fn(primase_core::Entity) -> Option<u64>,
) -> Result<crate::components::SkateMapSummary> {
    // everything the map's objects take, nothing encoded
    let mut laid = lay(world, a, Layout::Count)?;
    let grouping = grouping(&laid.scene, a);
    let chains = world
        .resource::<crate::components::SkateBlendChains>()
        .map(crate::passes::blend_budget::candidates)
        .unwrap_or_default();
    let (writer, extras) = laid.extras();
    if cells == crate::components::CellsCounted::Apart {
        // its collision cells are their groups': taken out unlaid
        writer.take_cells();
    }
    let count = writer.non_chain_cull_ids(extras)?;
    let mut shared = BTreeMap::new();
    for (kind, e, n) in count.shared {
        let k = key(e).ok_or_else(|| {
            Error::Scene(format!(
                "{kind:?} {e:?} is counted, but it is not the part's output"
            ))
        })?;
        if shared.insert((kind, k), n).is_some() {
            return Err(Error::Scene(format!("{kind:?} {k} is counted twice")));
        }
    }
    Ok(crate::components::SkateMapSummary {
        grouping,
        cull_ids: a.cull_ids,
        summed: count.summed,
        shared,
        water: count.water,
        cells,
        chains,
    })
}

/// Collision triangles written as another surface, by (their surface, the surface written as).
pub(crate) type Rewritten = BTreeMap<(String, String), usize>;

/// The collision plan of the prepared world `world` for the encoding `a` (`prepare-reskate-collision`'s): the writer's
/// own layout of its collision, nothing encoded, each leaf's minor surfaces rewritten as `surfaces` says and its
/// pieces welded as `weld` says; with the triangles rewritten, by (surface, the surface written as).
pub(crate) fn collision_plan(
    world: &World,
    a: &EncodingOptions,
    surfaces: CollisionSurfaces,
    weld: CollisionWeld,
) -> Result<(SkateCollisionPlan, Rewritten)> {
    // the collision only: no render mesh, nothing encoded
    let mut laid = lay(world, a, Layout::Collision)?;
    let writer = &mut laid.writer;
    let surf = writer.surfaces().clone();
    let mut leaves: BTreeMap<String, PlannedLeaf> = BTreeMap::new();
    let mut rewritten = Rewritten::new();
    writer.collision_leaves(&mut |leaf, _| {
        let planned = crate::collision_pack::plan_leaf(&surf, &leaf, surfaces, weld)?;
        for (&s, &to) in &planned.into {
            *rewritten
                .entry((surf.list[s].0.clone(), surf.list[to].0.clone()))
                .or_default() += planned.counts[&s];
        }
        if leaves.insert(leaf.name.to_owned(), planned).is_some() {
            return Err(Error::Scene(format!(
                "two collision leaves are named {}",
                leaf.name
            )));
        }
        Ok(())
    })?;
    Ok((
        SkateCollisionPlan {
            surfaces,
            weld,
            table: surf.list.iter().map(|(n, _)| n.clone()).collect(),
            leaves,
        },
        rewritten,
    ))
}

/// What the prepared part `world` lays out in each collision cell, as the map sink would lay the part out for the
/// encoding `a` (the region, instancing and records as it takes them; a part of a map in parts takes the map's from its
/// summaries), nothing encoded: the cell layout and a share per cell it adds collision to
/// (`share-reskate-collision`'s). The hulls are not shared: each part writes its own whole.
pub(crate) fn cell_shares(
    world: &World,
    a: &EncodingOptions,
    key: &dyn Fn(primase_core::Entity) -> u64,
) -> Result<(SkateCellLayout, BTreeMap<CellKey, SkateCellShare>)> {
    let PartRead {
        scene: mut s,
        o,
        cut,
        placements,
        records,
        counts,
        ..
    } = read_part(world, Path::new(""), a, false)?;
    let layout = cell_layout(&s, &o);
    // the collision only: no render mesh, nothing encoded
    let mut writer = records
        .writer(o, s.surfaces.clone())?
        .without_encoding()
        .with_layout(Layout::Collision);
    writer.set_counts(counts);
    lay_part(&mut s, cut, &placements, &mut writer)?;
    let shares = writer
        .take_cells()
        .into_iter()
        .map(|(cell, c)| {
            let share = SkateCellShare {
                cell,
                soups: c
                    .segs
                    .into_iter()
                    .map(|g| CellSoup {
                        order: (g.order.0, g.order.1.index()),
                        key: key(g.order.1),
                        ground: g.ground,
                        v: g.v,
                        s: g.s,
                        rock: g.rock,
                    })
                    .collect(),
                props: c
                    .props
                    .into_iter()
                    .map(|(tx, ty, p)| CellProps {
                        tile: (tx, ty),
                        pos: p.pos,
                        surf: p.surf,
                        ground: p.ground,
                    })
                    .collect(),
            };
            (cell, share)
        })
        .collect();
    Ok((layout, shares))
}

/// The cell layout of a part read as `s` with the writer's options `o`: what its cells are laid out with.
pub(crate) fn cell_layout(s: &Scene, o: &WriterOptions) -> SkateCellLayout {
    SkateCellLayout {
        name: o.name.clone(),
        surfaces: s.surfaces.list.clone(),
        limit: o.limit,
        offset: [o.offset[0] as f32, o.offset[1] as f32, o.offset[2] as f32],
    }
}

/// One collision cell's content from its shares, every part's (the order they come in does not matter: the cell lays
/// its soups out in the scene's order and its props by tile). A soup two parts both write (one key) is laid out once;
/// shared differently, it is refused.
pub(crate) fn cell_content(shares: &[&SkateCellShare]) -> Result<CellContent> {
    let mut c = CellContent::default();
    let mut seen: BTreeMap<u64, &crate::components::CellSoup> = BTreeMap::new();
    for share in shares {
        for g in &share.soups {
            match seen.get(&g.key) {
                Some(first) if *first != g => {
                    return Err(Error::Scene(format!(
                        "the collision soup {} is shared differently by two parts in the cell {:?}",
                        g.order.1, share.cell
                    )));
                }
                Some(_) => continue,
                None => {
                    seen.insert(g.key, g);
                }
            }
            c.segs.push(SoupSeg {
                order: (g.order.0, primase_core::Entity::from_index(g.order.1)),
                ground: g.ground,
                v: g.v.clone(),
                s: g.s.clone(),
                rock: g.rock.clone(),
            });
        }
        c.props.extend(share.props.iter().map(|p| {
            (
                p.tile.0,
                p.tile.1,
                PropSoup {
                    pos: p.pos.clone(),
                    surf: p.surf.clone(),
                    ground: p.ground.clone(),
                },
            )
        }));
    }
    Ok(c)
}

/// The collision cell a group world holds (its shares, its `skate.cell_layout`): its key, its content and the layout's
/// surface table; an error for a world without one cell's shares.
pub(crate) fn group_cell(
    world: &World,
) -> Result<(CellKey, CellContent, crate::scene::SurfaceTable, SkateCellLayout)> {
    let layout = world
        .resource::<SkateCellLayout>()
        .cloned()
        .ok_or_else(|| Error::Scene("a collision cell without its layout (skate.cell_layout)".into()))?;
    let shares: Vec<&SkateCellShare> = world.iter::<SkateCellShare>().map(|(_, s)| s).collect();
    let key = shares
        .first()
        .map(|s| s.cell)
        .ok_or_else(|| Error::Scene("a collision cell with no share (skate.cell_share)".into()))?;
    if let Some(other) = shares.iter().find(|s| s.cell != key) {
        return Err(Error::Scene(format!(
            "a collision cell holds shares of two cells, {key:?} and {:?}",
            other.cell
        )));
    }
    let surf = crate::scene::SurfaceTable {
        list: layout.surfaces.clone(),
    };
    Ok((key, cell_content(&shares)?, surf, layout))
}

/// The collision plan of the collision cell a group world holds (`prepare-reskate-collision`'s, on a cell of a map in
/// parts): the cell laid out alone as the writer lays it out, each leaf's minor surfaces rewritten as `surfaces` says
/// and its pieces welded as `weld` says; with the triangles rewritten, and what the cell is written as (its pieces and
/// their cull IDs, counted by the writer's own packing of it as planned).
pub(crate) fn cell_plan(
    world: &World,
    surfaces: CollisionSurfaces,
    weld: CollisionWeld,
) -> Result<(SkateCollisionPlan, Rewritten, SkateCellCounts)> {
    let (key, content, surf, layout) = group_cell(world)?;
    let off = primase_meshops::vector::Vec3::new(layout.offset[0], layout.offset[1], layout.offset[2]);
    let mut leaves: BTreeMap<String, PlannedLeaf> = BTreeMap::new();
    let mut rewritten = Rewritten::new();
    let mut collision = Collision::new(&surf, off.x, off.y, off.z, layout.limit);
    let mut res = CollisionResult::default();
    lay_cell(&mut collision, key, content.clone(), &mut res, &mut |leaf, _| {
        let planned = crate::collision_pack::plan_leaf(&surf, &leaf, surfaces, weld)?;
        for (&s, &to) in &planned.into {
            *rewritten
                .entry((surf.list[s].0.clone(), surf.list[to].0.clone()))
                .or_default() += planned.counts[&s];
        }
        if leaves.insert(leaf.name.to_owned(), planned).is_some() {
            return Err(Error::Scene(format!(
                "two collision leaves are named {}",
                leaf.name
            )));
        }
        Ok(())
    })?;
    let plan = SkateCollisionPlan {
        surfaces,
        weld,
        table: surf.list.iter().map(|(n, _)| n.clone()).collect(),
        leaves,
    };
    // what the writer writes of it, as planned: its pieces and the cull IDs they take
    let mut files = crate::collision_pack::Files::new();
    let mut pack = crate::collision_pack::Pack::new(&surf, off, Some(&plan), &mut files)?.encoding(false);
    let mut res = CollisionResult::default();
    let mut collision = Collision::new(&surf, off.x, off.y, off.z, layout.limit);
    lay_cell(&mut collision, key, content, &mut res, &mut |leaf, r| {
        pack.leaf(leaf, r)
    })?;
    crate::collision_pack::check_packed(Some(&plan), pack.seen())?;
    let count = CellCount {
        pieces: res.objects.len(),
        cull_ids: res.objects.iter().map(|ob| ob.cells as usize).sum(),
    };
    Ok((
        plan,
        rewritten,
        SkateCellCounts {
            cells: BTreeMap::from([(key, count)]),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::SceneWarnings;

    /// A raised map: a slab with its collision placed three times (instanced: `min_instances` 3), a 2 x 2 m ground
    /// soup, a spawn.
    fn slabs() -> primase_core::World {
        use primase_core::{
            Collision, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform,
        };
        let mut w = primase_core::World::new();
        let mut surfaces = primase_core::Surfaces::default();
        let concrete = surfaces.index("concrete").unwrap();
        w.insert_resource(surfaces);
        let material = w.spawn();
        w.insert(material, Name("stone".into()));
        w.insert(material, primase_core::pbr::PbrMaterial::default());
        let pos = vec![
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [0.0, 0.0, -4.0],
            [4.0, 0.0, -4.0],
        ];
        let tris = vec![[0u32, 1, 2], [2, 1, 3]];
        let proto = w.spawn();
        w.insert(proto, Name("slab".into()));
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: std::collections::BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
                indices: Some(tris.iter().flatten().copied().collect()),
                material: Some(material),
            }]),
        );
        let mut c = Collision::default();
        c.extend(&pos, &tris, concrete, |p| p);
        w.insert(proto, c.clone());
        for x in [0.0, 10.0, 300.0] {
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 0.0, 0.0];
            w.insert(p, t);
        }
        let ground = w.spawn();
        w.insert(ground, c);
        crate::passes::raise_skate(
            &mut w,
            &crate::passes::RaiseOptions::new("slabs", crate::passes::LightCalibration::Standard).unwrap(),
        )
        .unwrap();
        w
    }

    /// Each layout makes only what its caller reads: a dry one encodes no mesh; a collision layout builds no render
    /// tile or prototype and files the same collision as the full layout; a render layout files no collision; a count
    /// lays out what the full layout does, without a file.
    #[test]
    fn each_layout_makes_only_what_its_caller_reads() {
        use crate::map::Layout;
        let w = slabs();
        let a = super::EncodingOptions {
            min_instances: 3,
            min_instance_tris: 1,
            ..super::EncodingOptions::default()
        };
        let mut full = super::lay(&w, &a, Layout::Write).unwrap();
        assert!(!full.writer.mesh_files.is_empty());
        assert!(full.writer.r.instances == 3 && !full.writer.r.tiles.is_empty());
        let cells = full.writer.waiting_cells();
        assert!(!cells.is_empty());
        let mut count = super::lay(&w, &a, Layout::Count).unwrap();
        assert!(count.writer.mesh_files.is_empty());
        assert_eq!(count.writer.r.tiles.len(), full.writer.r.tiles.len());
        assert_eq!(count.writer.r.instances, full.writer.r.instances);
        assert_eq!(count.writer.waiting_cells(), cells);
        let mut collision = super::lay(&w, &a, Layout::Collision).unwrap();
        assert!(collision.writer.mesh_files.is_empty());
        assert!(collision.writer.r.tiles.is_empty() && collision.writer.r.instances == 0);
        assert_eq!(collision.writer.waiting_cells(), cells);
        let render = super::lay(&w, &a, Layout::Render).unwrap();
        assert!(render.writer.mesh_files.is_empty());
        assert_eq!(render.writer.r.tiles.len(), full.writer.r.tiles.len());
        assert!(render.writer.waiting_cells().is_empty());
        // the cells' content, as filed, is the same whichever layout filed it
        let triangles = |l: &mut super::Laid| -> Vec<usize> {
            l.writer
                .take_cells()
                .values()
                .map(crate::map::CellContent::triangles)
                .collect()
        };
        assert_eq!(triangles(&mut collision), triangles(&mut full));
        assert_eq!(
            triangles(&mut count),
            triangles(&mut super::lay(&w, &a, Layout::Write).unwrap())
        );
    }

    fn set(items: &[&str]) -> std::collections::BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Judged over the whole map: a texture one part holds and no material of its uses, used by another part's
    /// material, is not unused; a prototype one part holds unplaced, placed by another part, is not unplaced. Merged in
    /// either order, the warnings are those of the whole map's facts.
    #[test]
    fn scene_warnings_are_judged_over_every_part() {
        let a = SceneWarnings {
            textures: set(&["stone", "moss"]),
            textures_used: set(&["moss"]),
            protos: set(&["bench", "bin"]),
            protos_placed: set(&["bin"]),
            ..SceneWarnings::default()
        };
        let b = SceneWarnings {
            textures: set(&["stone"]),
            textures_used: set(&["stone"]),
            protos: set(&["bench"]),
            protos_placed: set(&["bench"]),
            travel: 1,
            ..SceneWarnings::default()
        };
        let lone = SceneWarnings {
            textures: set(&["lost"]),
            protos: set(&["crate"]),
            ..SceneWarnings::default()
        };
        let whole = SceneWarnings {
            textures: set(&["stone", "moss", "lost"]),
            textures_used: set(&["moss", "stone"]),
            protos: set(&["bench", "bin", "crate"]),
            protos_placed: set(&["bin", "bench"]),
            travel: 1,
            ..SceneWarnings::default()
        };
        for order in [[&a, &b, &lone], [&lone, &b, &a]] {
            let mut merged = SceneWarnings::default();
            for w in order {
                merged.merge(w.clone());
            }
            assert_eq!(merged.render(), whole.render());
        }
        assert_eq!(
            whole.render(),
            ["texture 'lost' is not used", "prototype 'crate' is not placed"]
        );
    }
}
