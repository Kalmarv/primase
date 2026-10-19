//! The map writer's assembly of a prepared world, I/O-free: the encoding it is made for ([`EncodingOptions`]), the
//! placements and soups in the writer's frame, the tiles and prototypes the writer builds from the scene's meshes, and
//! the dry assemblies the preparation passes record their decisions from (the overlay stack, the blend chains, the
//! materials), so a pass and the writer share one calculation and no pass names the writer.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use primase_core::World;

use crate::collision::{CollisionWorld, TileData};
use crate::components::{
    CollisionSurfaces, CollisionWeld, PlannedLeaf, SkateCollisionPlan, SkatePausePicture,
};
use crate::map::{Chains, Ctx, Extras, MapWriter, MeshSource, Stack, WriterOptions};
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

    /// Every cell's soups, for the bytes a sink holds.
    pub(crate) fn bytes(&self) -> usize {
        self.cells
            .values()
            .flatten()
            .map(|g| 4 * g.v.len() + g.s.len() + g.rock.len() + 24)
            .sum()
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

/// The writer's assembly of the prepared world `world` for the encoding `a` (its tiles and prototypes, nothing
/// written), with the overlay stack `stack` and textures encoded or only identified (`encode`).
fn assembly(
    world: &World,
    a: &EncodingOptions,
    stack: Stack,
    chains: Chains,
    encode: bool,
) -> Result<MapWriter> {
    let s = Scene::from_world(world, Path::new(""))?;
    let mut o = writer_options(&s, a)?;
    o.apron = apron_of(world, &s)?;
    let placements = writer_placements(&s, o.tile);
    let in_parts = world
        .resource::<primase_core::convert::PartScope>()
        .is_some_and(|p| !p.whole);
    o.region = map_region(world, &s, &placements, in_parts)?;
    let mut writer = MapWriter::new(o, s.surfaces.clone())?
        .with_stack(stack)
        .with_chains(chains);
    writer.set_counts(map_counts(world, &s, in_parts)?);
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

/// The scene reader's warnings by kind, each kind a set (rendered in its order, `render`), and the travel points
/// counted: the parts of a map merge into the whole map's ([`SceneWarnings::merge`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneWarnings {
    kinds: [BTreeSet<String>; 3],
    travel: usize,
}

impl SceneWarnings {
    pub(crate) fn of(s: &Scene, world: &World) -> Result<SceneWarnings> {
        let found = scene_warning_kinds(s, world)?;
        Ok(SceneWarnings {
            kinds: found.0.map(|k| k.into_iter().collect()),
            travel: found.1,
        })
    }

    /// Another part's added.
    pub(crate) fn merge(&mut self, o: SceneWarnings) {
        for (mine, theirs) in self.kinds.iter_mut().zip(o.kinds) {
            mine.extend(theirs);
        }
        self.travel += o.travel;
    }

    /// Each kind in a stable order (by its text), then the travel points when Studio takes fewer.
    pub(crate) fn render(&self) -> Vec<String> {
        let mut out: Vec<String> = self.kinds.iter().flatten().cloned().collect();
        if self.travel > 25 {
            out.push(format!("{} travel points: Studio takes at most 25", self.travel));
        }
        out
    }
}

/// The scene reader's warnings of each kind (unused textures, primitives without normals, unplaced prototypes and
/// prototypes without geometry), and the travel points.
fn scene_warning_kinds(s: &Scene, world: &World) -> Result<([Vec<String>; 3], usize)> {
    use crate::components::{SkateMaterial, SkateTexture};
    // each kind in a stable order (by its text), not the order the world's entities came in
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
    let travel = s.markers.iter().filter(|m| m.kind == "travel").count();
    Ok(([unused, flat, unplaced], travel))
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
pub(crate) fn lay(world: &World, dir: &Path, a: &EncodingOptions, encode: bool) -> Result<Laid> {
    let mut s = Scene::from_world(world, dir)?;
    let mut o = part_options(world, &s, a, encode)?;
    let cut = s.region;
    let placements = writer_placements(&s, o.tile);
    o.region = cut.unwrap_or_else(|| bounds(&s, &placements));
    // the floor is prepare-reskate-floor's: the world comes lifted, its parts below the cutoff marked or gone
    let drop_below = f32::NEG_INFINITY;
    let mut writer = Records::of(world, &s, a)?.writer(o.clone(), s.surfaces.clone())?;
    if !encode {
        writer = writer.without_encoding();
    }
    let tri_world = SoupWorld::new(&mut s, CollisionMode::TriangleMesh, cut, drop_below);
    let water_world = s
        .soups
        .iter()
        .any(|x| x.mode == CollisionMode::Water)
        .then(|| SoupWorld::new(&mut s, CollisionMode::Water, cut, drop_below));
    writer.add_soups(tri_world, water_world);
    let hulls = hulls(&mut s, cut, drop_below);
    assemble(&s, &placements, &mut writer)?;
    Ok(Laid {
        scene: s,
        writer,
        hulls,
    })
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

impl Records {
    /// Whether the parts of one map were prepared alike: the same collision plan and blend budget (each part's whole:
    /// a rule, the writer deciding over the whole map), an overlay stack and blend chains of the same settings or
    /// neither (each part's own meshes and tiles).
    pub(crate) fn alike(&self, o: &Records) -> bool {
        self.plan == o.plan
            && self.budget == o.budget
            && self.stack.as_ref().map(|s| (s.step, s.grouping))
                == o.stack.as_ref().map(|s| (s.step, s.grouping))
            && self.chains.as_ref().map(|c| (c.grid, c.grouping))
                == o.chains.as_ref().map(|c| (c.grid, c.grouping))
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
/// own layout of it takes besides the blend chains, counted as [`MapWriter::finish`] counts them, with nothing encoded.
pub(crate) fn map_summary(world: &World, a: &EncodingOptions) -> Result<crate::components::SkateMapSummary> {
    let mut laid = lay(world, Path::new(""), a, false)?;
    let grouping = grouping(&laid.scene, a);
    let (writer, extras) = laid.extras();
    let other_cull_ids = writer.non_chain_cull_ids(extras)?;
    Ok(crate::components::SkateMapSummary {
        grouping,
        cull_ids: a.cull_ids,
        other_cull_ids,
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
    let mut laid = lay(world, Path::new(""), a, false)?;
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
            leaves: Some(leaves),
        },
        rewritten,
    ))
}
