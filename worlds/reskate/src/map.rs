//! Placements of prototypes -> ReSkate Studio's normalized map folder: static geometry merged into one mesh per map
//! tile (one primitive per material), repeated prototypes kept as one mesh placed many times; then collision,
//! water, grind prisms, markers, lights, textures, map.json and trainer.json. The reference writer's MapWriter.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::assembly::{SoupSeg, SoupWorld, soup_cell};
use crate::blend_order::{self, Geometry};
use crate::collision::{Collision, CollisionResult, LeafVisit, PropSoup};
use crate::components::pause_map::{PauseColour, PauseDrawing, PauseLeaveOut};
use crate::components::{
    BlendBudgetMode, ChainGroup, ChainTile, OverlayPiece, OverlayTotals, SkateBlendBudget, SkateBlendChains,
    SkateOverlayStack, StackedPiece,
};
use crate::glb::{self, GlbPrim};
use crate::grind;
use crate::material::{MatLib, Material, VClass};
use crate::model::{CollisionMode, Hull, MapObject, Placement, ProtoInfo, SrcMesh, SrcPart, TileOut};
use crate::overlay::{self, OverlayStats, Parts, Piece};
use crate::passes::pause_map::SkatePausePicture;
use crate::pausemap::{Paint, PauseMap, PauseWrap};
use crate::records::{
    Basis, EffectJson, LightJson, MapJson, MaterialJson, ObjectJson, OceanMaterial, SpawnJson,
    StreamingDistances, SurfaceMaterial, TextureEntry, TexturesIndex, TrainerJson, TrainerSpot,
    TravelPointJson,
};
use crate::scene::{GrindCurve, Marker, PlacedLight, SnapMode, SurfaceTable, WaterQuad};

/// A collision cell: water (`true`) or the triangle mesh, its column and row of 256 m.
pub(crate) type CellKey = (bool, i32, i32);
use crate::texture::TexLib;
use crate::{Error, Result};
use primase_meshops::vector::Vec3;

/// What the writer is told (defaults are gta-skate's).
#[derive(Clone, Debug, PartialEq)]
pub struct WriterOptions {
    pub name: String,
    pub display_name: Option<String>,
    pub generator: String,
    /// Writer frame (east, north): x0, y0, x1, y1, half-open.
    pub region: [f64; 4],
    pub tile: f32,
    pub band_rows: i32,
    pub min_instances: i64,
    pub min_instance_tris: i64,
    pub offset: [f64; 3],
    /// The apron `clip-reskate-region` recorded around the region (0 when nothing is clipped).
    pub apron: f32,
    /// The cull IDs the map may take ([`crate::assembly::EncodingOptions::cull_ids`]).
    pub cull_ids: usize,
    pub limit: f32,
    pub light_prefix: String,
    pub streaming_distances: [f64; 3],
    pub max_render_edge: f32,
    pub trainer_note: Option<String>,
    pub warnings: Vec<String>,
    /// The pause-map picture to write (`prepare-reskate-pause-map`); `None`: none.
    pub pause: Option<SkatePausePicture>,
    /// How many PNG files are encoded at once (the program's choice, [`crate::MapSink::with_threads`]): the same files
    /// on any count.
    pub threads: primase_core::parallel::Threads,
}

impl WriterOptions {
    /// How the writer draws its geometry into the pause-map picture (`None`: it does not).
    pub(crate) fn pause_drawing(&self) -> Option<&PauseDrawing> {
        match &self.pause {
            Some(SkatePausePicture::Geometry { drawing, .. }) => Some(drawing),
            _ => None,
        }
    }
}

/// The placements of `world` (a map, or one part of it) that a pause-map picture drawn as `o` says leaves out by name
/// (`leave-out=placements`): those whose name starts with one of its prefixes, and the prefixes that matched one; none
/// for any other picture.
pub(crate) fn pause_left_of(
    world: &primase_core::World,
    o: &WriterOptions,
) -> (std::collections::HashSet<primase_core::Entity>, BTreeSet<String>) {
    let (mut left, mut matched) = (std::collections::HashSet::new(), BTreeSet::new());
    let Some(PauseLeaveOut::Placements(prefixes)) = o.pause_drawing().map(|d| &d.leave_out) else {
        return (left, matched);
    };
    for (e, _) in world.iter::<primase_core::Instance>() {
        let Some(n) = world.get::<primase_core::Name>(e) else {
            continue;
        };
        for p in prefixes.iter().filter(|p| n.0.starts_with(p.as_str())) {
            left.insert(e);
            matched.insert(p.clone());
        }
    }
    (left, matched)
}

/// How the pause-map picture draws a material's triangles.
#[derive(Clone, Copy, Debug)]
enum PausePick {
    /// Not drawn: a decal, glass or blended surface.
    Hidden,
    /// Left out: its material casts no shadow (`leave-out=shadowless`), counted.
    Shadowless,
    /// In one colour (0xRRGGBB): the mean colour.
    Flat(u32),
    /// In the mean colour though `colour=texture` asked for its texture, which could not be sampled; counted.
    Untextured(u32),
    /// Its base texture `tex` sampled, times `factor` (linear), wrapping as `wrap` says.
    Texture {
        tex: i32,
        factor: [f32; 3],
        wrap: PauseWrap,
    },
}

/// What the writer did, for stats.json.
#[derive(Debug, Default)]
pub struct WriteResult {
    pub proto_list: Vec<ProtoInfo>,
    pub tiles: Vec<TileOut>,
    pub object_records: usize,
    pub materials: usize,
    pub archetypes: usize,
    pub merged_entities: i64,
    pub merged_tris: i64,
    pub instances: i64,
    pub instanced_tris: i64,
    pub prototype_tris: i64,
    pub empty_mesh: i64,
    /// Placements of prototypes that collide without drawing (their collision placed, nothing drawn).
    pub collision_only: i64,
    pub peak_mesh_bytes: i64,
    /// Placements marked outside the region with nothing within its apron, left out (`clip-reskate-region`'s).
    pub far_dropped: i64,
    /// Placements marked outside the region trimmed to its apron (`clip-reskate-region`'s).
    pub apron_trimmed: i64,
    /// Instanced placements reaching past the map's limit, left out.
    pub limit_instances: i64,
    /// Merged triangles past the map's limit, left out.
    pub limit_triangles: i64,
    pub degenerate: i64,
    pub tex_removed: u64,
    pub tex_bytes: u64,
    pub tex_pixels: u64,
    pub mesh_bytes: u64,
    pub overlay: OverlayStats,
    /// The blend-chain overlays as objects of their own ([`crate::blend_order`]).
    pub blend: blend_order::Stats,
    pub prop_col_tris: BTreeMap<String, i64>,
    pub collision: Option<CollisionResult>,
    pub lights_written: usize,
    /// Water outlines the map's extent limit cut, and those left with no area (not drawn).
    pub water_cut_at_limit: usize,
    pub water_without_area: usize,
    /// Pause-map picture: triangles left out because their material casts no shadow (`leave-out=shadowless`).
    pub pause_shadowless: i64,
    /// Pause-map picture: triangles of the placements left out by name (`leave-out=placements`).
    pub pause_placements: i64,
    /// Pause-map picture: per material name, the triangles drawn in the mean colour because no texture could be
    /// sampled (`colour=texture`).
    pub pause_untextured: BTreeMap<String, i64>,
    /// Lights whose basis came from the fallback axis (no usable tangent).
    pub lights_without_tangent: usize,
    /// The overlay stack's lift record: per mesh file (`meshes/...`), per primitive in file order, each vertex's lift
    /// along its derived normal in metres (an empty list: nothing lifted). Meshes without a lifted vertex are left out.
    pub lifts: BTreeMap<String, Vec<Vec<f32>>>,
    pub grind_curves: i64,
    pub grind_pieces: i64,
    pub grind_length: f32,
    pub trainer_spots: usize,
    /// Trainer spots left out: without a height, or past the 64 trainer.json takes.
    pub trainer_left_out: usize,
    /// Grind curves whose prism has no usable points, left out.
    pub grind_unusable: usize,
    pub travel_points: usize,
    pub warnings: Vec<String>,
}

/// A tile's blend-chain overlays written in both forms [`MapWriter::finish`] chooses between in key order
/// ([`MapWriter::chain_candidates`]): objects of their own ([`crate::blend_order`]) or one plain object. Only the
/// records stay in memory; the form not taken has its files deleted.
#[derive(Debug)]
struct PendingChain {
    /// The chain form: its objects and their files.
    objects: Vec<MapObject>,
    files: Vec<String>,
    /// The plain form: its objects (a section per material) and its file.
    plain: Vec<MapObject>,
    plain_file: String,
    /// What the chain form counts in the stats: objects (one per cell, axis and rank), lift steps widened past
    /// [`blend_order::LIFT_STEP`] and the largest, overlay triangles and those clipped at the grid's borders.
    groups: usize,
    widened: usize,
    max_step: f32,
    tris: usize,
    clipped: usize,
}

/// ReSkate 2.1.0-pre's cull-ID pool per spatial queue (`SpatialQueryManager.InitialCullIdListSize`).
pub const CULL_ID_POOL: usize = 262_144;

/// The share of [`CULL_ID_POOL`] a map may fill (a 20% margin).
pub const CULL_ID_SHARE: f64 = 0.8;

/// One merged part: its geometry and its pieces (one per append).
#[derive(Clone, Debug, Default)]
struct Acc {
    /// First use of the part's material in the mesh (0, 1, ...): the stable source order.
    seq: usize,
    /// How far the overlay stack lifted each vertex along its derived normal, in metres (empty: none lifted).
    lift: Vec<f32>,
    pos: Vec<f32>,
    nrm: Vec<f32>,
    uv: Vec<f32>,
    /// Per-vertex blend weight (empty: the part's material is not a vertex blend).
    blend: Vec<f32>,
    idx: Vec<u32>,
    pieces: Vec<AccPiece>,
}

/// The props' collision of each collision tile, with the map tile that placed them.
type PropParts = BTreeMap<(i32, i32), Vec<(i32, i32, PropSoup)>>;

/// One append to a merged part: its triangles and vertices, draw order and entity.
#[derive(Clone, Copy, Debug)]
struct AccPiece {
    id: OverlayPiece,
    t0: usize,
    t1: usize,
    v0: usize,
    v1: usize,
    order: i32,
    entity: i32,
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    degenerate: bool,
    longest: f32,
}

/// What the writer does with one placement (see [`placement_fate`]).
pub(crate) enum Fate {
    /// Its prototype draws nothing and does not collide.
    EmptyMesh,
    /// Its basis is degenerate.
    Degenerate,
    /// Outside the region and no part reaches into it.
    FarDropped,
    /// It collides without drawing: its collision (clipped to the region's apron when outside), or none.
    CollisionOnly { collision: Option<Option<[f32; 4]>> },
    /// Drawn: instanced or merged, an outside placement's parts kept within the apron, left out entirely past the
    /// map's limit when instanced (`clipped`), and its collision (clipped to the apron when its parts are), or none.
    Drawn {
        inst: bool,
        kept_parts: Option<Vec<Option<Vec<u32>>>>,
        clipped: bool,
        collision: Option<Option<[f32; 4]>>,
    },
}

/// The writer's rules for one placement of mesh `m` (`None`: none built): what it draws and what of its collision
/// goes into the map. `count` gives a prototype's number of placements (instancing), `edge` is the region with its
/// apron, `off` the map offset. The writer and `snap-reskate-markers` both take a placement's collision from here.
pub(crate) fn placement_fate(
    m: Option<&SrcMesh>,
    s: &Placement,
    count: impl Fn(u32) -> i64,
    o: &PlacementRules,
    edge: [f32; 4],
    off: Vec3,
) -> Fate {
    // a mesh without triangles draws nothing; one with collision still collides (its collision clipped to the
    // region's edge when the placement is outside it)
    let Some(m) = m.filter(|m| m.tris != 0 || m.col_pos.is_some()) else {
        return Fate::EmptyMesh;
    };
    let sh = shape_of(s);
    if sh.degenerate {
        return Fate::Degenerate;
    }
    if m.tris == 0 {
        return Fate::CollisionOnly {
            collision: (!s.no_collision).then_some(s.outside.then_some(edge)),
        };
    }
    let mut inst = count(m.hash) >= o.min_instances && m.tris >= o.min_instance_tris;
    if inst && (s.no_instance || s.part_off.is_some() || (o.max_render_edge > 0.0 && sh.longest > 1.0001)) {
        inst = false;
    }
    let mut kept_parts: Option<Vec<Option<Vec<u32>>>> = None;
    if s.outside {
        let mut reaches = false;
        let k: Vec<Option<Vec<u32>>> = m
            .parts
            .iter()
            .map(|p| kept(p, s, edge, o.region, &mut reaches))
            .collect();
        if !reaches {
            return Fate::FarDropped;
        }
        let half = (m.max - m.min) * 0.5;
        if inst && half.x.max(half.y.max(half.z)) > 32.0 {
            inst = false;
        }
        if !inst {
            kept_parts = Some(k);
        }
    }
    let clipped = ((s.pos.x + off.x).abs() + s.radius > o.limit
        || (s.pos.y + off.y).abs() + s.radius > o.limit)
        && inst;
    let collision = (!clipped && !s.no_collision).then(|| kept_parts.as_ref().map(|_| edge));
    Fate::Drawn {
        inst,
        kept_parts,
        clipped,
        collision,
    }
}

/// Points (writer frame, x y z after each other) placed by `s`, as the writer places a prototype's collision.
pub(crate) fn placed_points(cp: &[f32], s: &Placement) -> Vec<f32> {
    let mut w = vec![0f32; cp.len()];
    for v in (0..w.len().saturating_sub(2)).step_by(3) {
        let (x, y, z) = (cp[v], cp[v + 1], cp[v + 2]);
        w[v] = s.pos.x + s.ex.x * x + s.ey.x * y + s.ez.x * z;
        w[v + 1] = s.pos.y + s.ex.y * x + s.ey.y * y + s.ez.y * z;
        w[v + 2] = s.pos.z + s.ex.z * x + s.ey.z * y + s.ez.z * z;
    }
    w
}

/// The writer's options [`placement_fate`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacementRules {
    pub min_instances: i64,
    pub min_instance_tris: i64,
    pub max_render_edge: f32,
    pub limit: f32,
    pub region: [f64; 4],
}

impl PlacementRules {
    pub(crate) fn of(o: &WriterOptions) -> PlacementRules {
        PlacementRules {
            min_instances: o.min_instances,
            min_instance_tris: o.min_instance_tris,
            max_render_edge: o.max_render_edge,
            limit: o.limit,
            region: o.region,
        }
    }
}

/// The region `r` with the writer's apron around it.
pub(crate) fn apron(r: [f64; 4], margin: f32) -> [f32; 4] {
    [
        r[0] as f32 - margin,
        r[1] as f32 - margin,
        r[2] as f32 + margin,
        r[3] as f32 + margin,
    ]
}

fn shape_of(s: &Placement) -> Shape {
    let (lx, ly, lz) = (s.ex.length(), s.ey.length(), s.ez.length());
    let det = s.ex.cross(s.ey).dot(s.ez);
    // as written in the reference: any NaN counts as degenerate
    let small = |l: f32| l.is_nan() || l < 1e-6;
    let lim = 1e-6 * lx * ly * lz;
    let degenerate = small(lx) || small(ly) || small(lz) || det.is_nan() || lim.is_nan() || det.abs() <= lim;
    // placements reach the writer rigid (the scene refuses a mirrored or sheared one: bake-nonrigid bakes them)
    Shape {
        degenerate,
        longest: lx.max(ly.max(lz)),
    }
}

/// The triangles of a part placed by `s` whose outline box reaches into `bbox`; `reaches` set when one reaches
/// into the region itself. `None`: every triangle (the part's own indices).
fn kept(
    p: &SrcPart,
    s: &Placement,
    bbox: [f32; 4],
    region: [f64; 4],
    reaches: &mut bool,
) -> Option<Vec<u32>> {
    let nv = p.nverts();
    let mut wx = vec![0f32; nv];
    let mut wy = vec![0f32; nv];
    for i in 0..nv {
        let (x, y, z) = (p.pos[i * 3], p.pos[i * 3 + 1], p.pos[i * 3 + 2]);
        wx[i] = s.pos.x + s.ex.x * x + s.ey.x * y + s.ez.x * z;
        wy[i] = s.pos.y + s.ex.y * x + s.ey.y * y + s.ez.y * z;
    }
    let mut keep: Option<Vec<u32>> = None;
    let idx = &p.idx;
    for t in (0..idx.len().saturating_sub(2)).step_by(3) {
        let (i0, i1, i2) = (idx[t] as usize, idx[t + 1] as usize, idx[t + 2] as usize);
        let x0 = wx[i0].min(wx[i1].min(wx[i2]));
        let x1 = wx[i0].max(wx[i1].max(wx[i2]));
        let y0 = wy[i0].min(wy[i1].min(wy[i2]));
        let y1 = wy[i0].max(wy[i1].max(wy[i2]));
        if x1 >= bbox[0] && x0 <= bbox[2] && y1 >= bbox[1] && y0 <= bbox[3] {
            if let Some(k) = &mut keep {
                k.extend_from_slice(&[idx[t], idx[t + 1], idx[t + 2]]);
            }
            if !*reaches
                && f64::from(x1) >= region[0]
                && f64::from(x0) < region[2]
                && f64::from(y1) >= region[1]
                && f64::from(y0) < region[3]
            {
                *reaches = true;
            }
        } else if keep.is_none() {
            keep = Some(idx[..t].to_vec());
        }
    }
    keep
}

/// Append a part, moved by pos + basis (`s` none: as it is); with `only`, just those triangles and their vertices.
fn append_core(a: &mut Acc, p: &SrcPart, s: Option<&Placement>, origin: Vec3, only: Option<&[u32]>) {
    let vbase = (a.pos.len() / 3) as u32;
    let Some(s) = s else {
        a.pos.extend_from_slice(&p.pos);
        a.nrm.extend_from_slice(&p.nrm);
        a.uv.extend_from_slice(&p.uv);
        a.blend.extend_from_slice(&p.blend);
        a.idx.extend(p.idx.iter().map(|i| vbase + i));
        return;
    };
    let mesh_of = |idx: Vec<u32>, verts: Option<&[u32]>| {
        let pick = |src: &[f32], n: usize| -> Vec<[f32; 3]> {
            match verts {
                None => src
                    .chunks_exact(n)
                    .map(|c| [c[0], c[1], if n == 3 { c[2] } else { 0.0 }])
                    .collect(),
                Some(vs) => vs
                    .iter()
                    .map(|&v| {
                        let o = v as usize * n;
                        [src[o], src[o + 1], if n == 3 { src[o + 2] } else { 0.0 }]
                    })
                    .collect(),
            }
        };
        primase_meshops::Mesh {
            positions: pick(&p.pos, 3),
            normals: pick(&p.nrm, 3),
            uv: pick(&p.uv, 2).into_iter().map(|c| [c[0], c[1]]).collect(),
            indices: idx,
        }
    };
    // the mesh to transform: every vertex (the part's order), or the kept triangles' vertices in the order they are
    // first used
    let mut picked: Option<Vec<u32>> = None;
    let m = match only {
        None => mesh_of(p.idx.clone(), None),
        Some(only) => {
            let mut corners: Vec<u32> = Vec::with_capacity(only.len());
            for t in only.as_chunks::<3>().0 {
                corners.extend_from_slice(t);
            }
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut verts: Vec<u32> = Vec::new();
            let mut idx: Vec<u32> = Vec::with_capacity(corners.len());
            for c in corners {
                let n = *remap.entry(c).or_insert_with(|| {
                    verts.push(c);
                    (verts.len() - 1) as u32
                });
                idx.push(n);
            }
            let m = mesh_of(idx, Some(&verts));
            picked = Some(verts);
            m
        }
    };
    let t = s.pos - origin;
    let out = primase_meshops::transform(
        &m,
        [
            [s.ex.x, s.ex.y, s.ex.z],
            [s.ey.x, s.ey.y, s.ey.z],
            [s.ez.x, s.ez.y, s.ez.z],
        ],
        [t.x, t.y, t.z],
    )
    .expect("a placement checked not degenerate");
    a.pos.extend(out.positions.iter().flatten());
    a.nrm.extend(out.normals.iter().flatten());
    a.uv.extend(out.uv.iter().flatten());
    // the transform keeps the picked vertices in order, so the blend weights follow the same pick
    if !p.blend.is_empty() {
        match &picked {
            None => a.blend.extend_from_slice(&p.blend),
            Some(v) => a.blend.extend(v.iter().map(|&i| p.blend[i as usize])),
        }
    }
    a.idx.extend(out.indices.iter().map(|i| vbase + i));
}

/// Studio stores a mesh section with 16-bit indices.
const MAX_SECTION_VERTS: usize = 60000;

/// The GLB primitives of merged parts, in the order of their material names.
/// The part of material `mat`, made on its first use in the mesh.
fn part(parts: &mut BTreeMap<usize, Acc>, mat: usize) -> &mut Acc {
    let seq = parts.len();
    parts.entry(mat).or_insert_with(|| Acc {
        seq,
        ..Acc::default()
    })
}

/// The overlay stack's pass of a material: opaque 0, alpha tested or blended 1, water 2.
fn overlay_pass(m: &Material) -> i32 {
    if m.class == VClass::Water {
        2
    } else if m.alpha == 1 {
        0
    } else {
        1
    }
}

/// A material's draw class: surfaces before overlays (decals, water), and in each, by the overlay pass.
fn draw_class(m: &Material) -> (bool, i32) {
    (matches!(m.class, VClass::Decal | VClass::Water), overlay_pass(m))
}

/// The order a mesh's parts are drawn in: by draw class; within one, under before over by the overlay stack's
/// relations between their pieces (`under`: (below, above) materials), cycles broken by first use; then by first use
/// in the mesh. Never by name: material names end in a content hash.
fn draw_order(
    parts: &BTreeMap<usize, Acc>,
    class: impl Fn(usize) -> (bool, i32),
    under: &[(usize, usize)],
) -> Vec<usize> {
    let mut by_class: BTreeMap<(bool, i32), BTreeSet<(usize, usize)>> = BTreeMap::new();
    for (&m, a) in parts {
        by_class.entry(class(m)).or_default().insert((a.seq, m));
    }
    let mut out = Vec::with_capacity(parts.len());
    for (_, mut left) in by_class {
        let members: HashMap<usize, usize> = left.iter().map(|&(s, m)| (m, s)).collect();
        let mut ups: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut indeg: HashMap<usize, usize> = HashMap::new();
        let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
        for &(lo, hi) in under {
            if lo != hi && members.contains_key(&lo) && members.contains_key(&hi) && edges.insert((lo, hi)) {
                ups.entry(lo).or_default().push(hi);
                *indeg.entry(hi).or_default() += 1;
            }
        }
        let mut ready: BTreeSet<(usize, usize)> = left
            .iter()
            .filter(|(_, m)| !indeg.contains_key(m))
            .copied()
            .collect();
        while let Some(&first) = left.first() {
            let next = ready.pop_first().unwrap_or(first);
            left.remove(&next);
            out.push(next.1);
            for &a in ups.get(&next.1).into_iter().flatten() {
                let key = (members[&a], a);
                if left.contains(&key) {
                    let d = indeg.get_mut(&a).expect("an edge counted");
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(key);
                    }
                }
            }
        }
    }
    out
}

/// The mesh's primitives, its parts in `order` (material ids; see [`draw_order`]), and each primitive's vertex lifts
/// (empty: none lifted).
fn prims(parts: &BTreeMap<usize, Acc>, mats: &MatLib, order: &[usize]) -> (Vec<GlbPrim>, Vec<Vec<f32>>) {
    let mut list = Vec::new();
    let mut lifts = Vec::new();
    for mat in order {
        let Some(a) = parts.get(mat) else { continue };
        let name = mats.by_id(*mat).name.clone();
        if a.pos.len() / 3 <= MAX_SECTION_VERTS {
            list.push(GlbPrim {
                material: Some(name),
                pos: a.pos.clone(),
                nrm: Some(a.nrm.clone()),
                uv: Some(a.uv.clone()),
                idx: a.idx.clone(),
                blend: (!a.blend.is_empty()).then(|| a.blend.clone()),
            });
            lifts.push(a.lift.clone());
            continue;
        }
        for c in primase_vertex::split_by_vertex_budget(&a.idx, a.pos.len() / 3, MAX_SECTION_VERTS)
            .expect("valid indices")
        {
            let take = |src: &[f32], n: usize| {
                c.vertices
                    .iter()
                    .flat_map(|&v| src[v as usize * n..v as usize * n + n].to_vec())
                    .collect::<Vec<f32>>()
            };
            lifts.push(if a.lift.is_empty() {
                Vec::new()
            } else {
                take(&a.lift, 1)
            });
            list.push(GlbPrim {
                material: Some(name.clone()),
                pos: take(&a.pos, 3),
                nrm: Some(take(&a.nrm, 3)),
                uv: Some(take(&a.uv, 2)),
                blend: (!a.blend.is_empty()).then(|| take(&a.blend, 1)),
                idx: c.indices,
            });
        }
    }
    (list, lifts)
}

/// Record a mesh's lifts (when it has a lifted vertex).
fn record_lifts(r: &mut WriteResult, file: String, lifts: Vec<Vec<f32>>) {
    if lifts.iter().any(|l| l.iter().any(|&x| x > 0.0)) {
        r.lifts.insert(file, lifts);
    }
}

/// What `tiles` needs from the scene: the prototypes of a band, and a part's material for a variant row.
pub trait MeshSource {
    fn load(&mut self, band: &[Placement], meshes: &mut HashMap<u32, SrcMesh>, ctx: &mut Ctx) -> Result<()>;
    fn material_of(&mut self, part: &SrcPart, row: u32, ctx: &mut Ctx) -> Result<usize>;
}

/// The writer's libraries, passed to the scene side to make materials and textures on first use.
#[derive(Debug)]
pub struct Ctx {
    pub tex: TexLib,
    pub mats: MatLib,
}

/// Studio's map, being written.
#[derive(Debug)]
pub struct MapWriter {
    pub o: WriterOptions,
    pub ctx: Ctx,
    pub surfaces: SurfaceTable,
    pub r: WriteResult,
    /// What it makes (everything, encoded, unless a pass's layout asks for less).
    layout: Layout,
    off: Vec3,
    pm: Option<PauseMap>,
    objects: Vec<MapObject>,
    mat_tris: HashMap<usize, i64>,
    mat_used: BTreeSet<usize>,
    prop_parts: PropParts,
    water_parts: PropParts,
    prop_hulls: Vec<Hull>,
    /// Tiles' chain overlays waiting for finish, by (row, column).
    pending_chains: BTreeMap<(i32, i32), PendingChain>,
    /// The overlay stack: none, made here for `prepare-reskate-overlays`, or the one it made, replayed.
    stack: Stack,
    /// The blend chains: none, chosen here for `prepare-reskate-blend-order`, or the ones it chose, built again.
    chains: Chains,
    /// The chain-object budget `plan-reskate-blend-budget` decided (`None`: none planned; tiles with chains refused).
    budget: Option<SkateBlendBudget>,
    /// How the collision is written, as `prepare-reskate-collision` planned it (`None`: exactly).
    collision_plan: Option<crate::components::SkateCollisionPlan>,
    /// The prototypes made, by key: each made and written once, by the first part of the map that draws one.
    protos: HashMap<[u8; 16], Proto>,
    /// The placements of each prototype key over the whole map, which decide what is instanced; `None`: the
    /// placements [`MapWriter::tiles`] is given are the whole map's.
    counts: Option<HashMap<u32, i64>>,
    /// The placements of the part being laid that the pause-map picture leaves out (`leave-out=placements`).
    pause_left: std::collections::HashSet<primase_core::Entity>,
    /// The prototype keys placed, over every part.
    placed_keys: BTreeSet<u32>,
    /// The world soups (triangle mesh, water) whose cells are not laid out yet ([`MapWriter::lay_cells`]).
    soups: SoupWorld,
    water_soups: SoupWorld,
    /// The collision laid out so far: the triangle mesh's cells and the hulls, the water's cells; the leaves packed.
    col: CollisionResult,
    water_col: CollisionResult,
    leaves_packed: usize,
    /// Mesh files a later decision dropped (a blend chain's form not taken) after they may have been written.
    pub discarded: Vec<String>,
    /// Mesh files (meshes/<name>), written at the end.
    pub mesh_files: BTreeMap<String, Vec<u8>>,
    /// Other files (relative to the map folder).
    pub files: BTreeMap<String, Vec<u8>>,
}

/// What a writer's layout makes. `Write`: everything, its meshes encoded (the map written). The others encode no mesh
/// and build only what their caller reads: `Count` everything but the encoding (the objects and the cull IDs they
/// take: `summarize-reskate-map`), `Render` the tiles and prototypes only, no collision (the overlay stack, the blend
/// chains, the materials), `Collision` the collision only, no render mesh (the collision plan, a part's shares).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Write,
    Count,
    Render,
    Collision,
}

impl Layout {
    /// Whether it encodes the meshes it makes (GLB files).
    pub(crate) fn encodes(self) -> bool {
        self == Layout::Write
    }
    /// Whether it builds the render tiles and prototypes.
    pub(crate) fn renders(self) -> bool {
        self != Layout::Collision
    }
    /// Whether it lays out the collision (soups, the placed props' collision, hulls).
    pub(crate) fn collides(self) -> bool {
        self != Layout::Render
    }
}

/// How the writer treats the overlay pieces of its meshes.
#[derive(Debug)]
pub(crate) enum Stack {
    /// Not stacked: decals drawn where they were authored (counted).
    Off { unstacked: u64 },
    /// Stacked by the step here, and recorded: what `prepare-reskate-overlays` runs the writer's assembly for.
    Compute(Box<SkateOverlayStack>),
    /// The stack `prepare-reskate-overlays` recorded, put on the same pieces again.
    Replay(Box<SkateOverlayStack>),
}

/// How the writer treats the vertex-blend chain overlays of its tiles.
#[derive(Debug)]
pub(crate) enum Chains {
    /// Left in their tiles, drawn in an unstable order (the tiles counted).
    Off { left: u64 },
    /// Chosen here, and recorded: what `prepare-reskate-blend-order` runs the writer's assembly for.
    Record(Box<SkateBlendChains>),
    /// The chains `prepare-reskate-blend-order` recorded, built again.
    Replay(Box<SkateBlendChains>),
}

impl Chains {
    fn grid(&self) -> f32 {
        match self {
            Chains::Off { .. } => 0.0,
            Chains::Record(r) | Chains::Replay(r) => r.grid,
        }
    }
}

impl Stack {
    pub(crate) fn step(&self) -> f32 {
        match self {
            Stack::Off { .. } => 0.0,
            Stack::Compute(r) | Stack::Replay(r) => r.step,
        }
    }
}

/// Meshes' overlay totals summed.
pub(crate) fn sum_totals<'a>(totals: impl IntoIterator<Item = &'a OverlayTotals>) -> OverlayTotals {
    let mut sum = OverlayStats::default();
    for t in totals {
        sum.add(&stats_of(t));
    }
    totals_of(&sum)
}

fn totals_of(s: &OverlayStats) -> OverlayTotals {
    OverlayTotals {
        pieces: s.pieces,
        shifted: s.shifted,
        water_shifted: s.water_shifted,
        relations: s.relations,
        cycles: s.cycles,
        conflicts: s.conflicts,
        conflicts_left: s.conflicts_left,
        max_shift: s.max_shift,
        shift_steps: s.shift_steps,
    }
}

fn stats_of(t: &OverlayTotals) -> OverlayStats {
    OverlayStats {
        pieces: t.pieces,
        shifted: t.shifted,
        water_shifted: t.water_shifted,
        relations: t.relations,
        cycles: t.cycles,
        conflicts: t.conflicts,
        conflicts_left: t.conflicts_left,
        max_shift: t.max_shift,
        shift_steps: t.shift_steps,
    }
}

/// A proto made once per (key, variant row).
#[derive(Debug)]
struct Proto {
    info: usize,
    materials: Vec<String>,
    origins: Vec<Option<[f32; 3]>>,
    /// Each primitive's local box (`None`: no positions), for the Studio cells a placement meets.
    boxes: Vec<Option<(Vec3, Vec3)>>,
}

impl MapWriter {
    pub fn new(o: WriterOptions, surfaces: SurfaceTable) -> Result<MapWriter> {
        let off = Vec3::new(o.offset[0] as f32, o.offset[1] as f32, o.offset[2] as f32);
        let pm = match &o.pause {
            Some(SkatePausePicture::Image {
                width,
                height,
                rgba,
                bounds,
            }) => Some(PauseMap::given(
                *width as usize,
                *height as usize,
                rgba,
                bounds.map(f64::from),
                off,
            )),
            Some(SkatePausePicture::Geometry { height, extent, .. }) => Some(PauseMap::new(
                extent.map_or(o.region, |e| e.map(f64::from)),
                off,
                *height,
            )),
            None => None,
        };
        let mut tex = TexLib::encoding().with_threads(o.threads);
        if o.pause_drawing()
            .is_some_and(|d| d.colour == PauseColour::Texture)
        {
            tex.keep_pyramids();
        }
        Ok(MapWriter {
            o,
            ctx: Ctx {
                tex,
                mats: MatLib::default(),
            },
            surfaces,
            r: WriteResult::default(),
            layout: Layout::Write,
            off,
            pm,
            objects: Vec::new(),
            mat_tris: HashMap::new(),
            mat_used: BTreeSet::new(),
            prop_parts: BTreeMap::new(),
            water_parts: BTreeMap::new(),
            prop_hulls: Vec::new(),
            collision_plan: None,
            protos: HashMap::new(),
            counts: None,
            pause_left: std::collections::HashSet::new(),
            placed_keys: BTreeSet::new(),
            soups: SoupWorld::default(),
            water_soups: SoupWorld::default(),
            col: CollisionResult::default(),
            water_col: CollisionResult::default(),
            leaves_packed: 0,
            discarded: Vec::new(),
            pending_chains: BTreeMap::new(),
            stack: Stack::Off { unstacked: 0 },
            chains: Chains::Off { left: 0 },
            budget: None,
            mesh_files: BTreeMap::new(),
            files: BTreeMap::new(),
        })
    }

    /// The overlay stack as `stack` says; a replayed stack's totals are the map's.
    pub(crate) fn with_stack(mut self, stack: Stack) -> MapWriter {
        if let Stack::Replay(rec) = &stack {
            self.r.overlay = stats_of(&rec.totals);
        }
        self.stack = stack;
        self
    }

    /// A layout that makes only what `layout` says (see [`Layout`]).
    pub(crate) fn with_layout(mut self, layout: Layout) -> MapWriter {
        self.layout = layout;
        self
    }

    /// What it makes.
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Texture ids only, no file encoded: the writer's assembly for a pass, nothing to save.
    pub(crate) fn without_encoding(mut self) -> MapWriter {
        let tex = std::mem::replace(&mut self.ctx.tex, TexLib::encoding());
        self.ctx.tex = tex.without_encoding();
        self
    }

    /// The overlay stack as it ended: the recorded one, or how many overlay pieces were left unstacked.
    pub(crate) fn stack(&self) -> &Stack {
        &self.stack
    }

    pub(crate) fn take_stack(&mut self) -> Stack {
        std::mem::replace(&mut self.stack, Stack::Off { unstacked: 0 })
    }

    /// The overlay stack for the meshes laid out next (a part of a map in several: its own).
    pub(crate) fn set_stack(&mut self, stack: Stack) {
        self.stack = stack;
    }

    /// The blend chains for the tiles laid out next (a part of a map in several: its own).
    pub(crate) fn set_chains(&mut self, chains: Chains) {
        self.chains = chains;
    }

    /// The overlay totals as the map's: given (the sum of every stacked mesh, a map in parts).
    pub(crate) fn set_overlay_totals(&mut self, t: &OverlayTotals) {
        self.r.overlay = stats_of(t);
    }

    /// The chain-object budget `plan-reskate-blend-budget` decided.
    pub(crate) fn with_budget(mut self, budget: Option<SkateBlendBudget>) -> MapWriter {
        self.budget = budget;
        self
    }

    /// The collision as `prepare-reskate-collision` planned it (`None`: exactly).
    pub(crate) fn with_collision_plan(
        mut self,
        plan: Option<crate::components::SkateCollisionPlan>,
    ) -> MapWriter {
        self.collision_plan = plan;
        self
    }

    /// The surfaces the map's collision indexes.
    pub(crate) fn surfaces(&self) -> &SurfaceTable {
        &self.surfaces
    }

    /// The collision's leaves as [`MapWriter::finish`] lays them out, each given to `visit`, nothing written: every
    /// cell still waiting laid out now.
    pub(crate) fn collision_leaves(&mut self, visit: &mut LeafVisit<'_>) -> Result<()> {
        let waiting = self.waiting_cells();
        self.lay_cells(&waiting, Some(visit))
    }

    /// The world soups of a part (triangle mesh, water), held until their cells are laid out.
    pub(crate) fn add_soups(&mut self, tri: SoupWorld, water: Option<SoupWorld>) {
        self.soups.absorb_all(tri);
        if let Some(w) = water {
            self.water_soups.absorb_all(w);
        }
    }

    /// The cells holding collision (world soups, placed props) not laid out yet.
    pub(crate) fn waiting_cells(&self) -> BTreeSet<CellKey> {
        let mut out = BTreeSet::new();
        out.extend(self.soups.cell_keys().map(|(x, y)| (false, x, y)));
        out.extend(self.water_soups.cell_keys().map(|(x, y)| (true, x, y)));
        out.extend(self.prop_parts.keys().map(|&(x, y)| (false, x, y)));
        out.extend(self.water_parts.keys().map(|&(x, y)| (true, x, y)));
        out
    }

    /// Cell `key`'s soups and props, taken out.
    fn take_cell(&mut self, (water, tx, ty): CellKey) -> CellContent {
        let (soups, parts) = if water {
            (&mut self.water_soups, &mut self.water_parts)
        } else {
            (&mut self.soups, &mut self.prop_parts)
        };
        CellContent {
            segs: soups.take_cell((tx, ty)),
            props: parts.remove(&(tx, ty)).unwrap_or_default(),
        }
    }

    /// Every waiting cell's soups and props, taken out: what a part of a map in parts shares of each cell
    /// (`share-reskate-collision`), or (dropped) what the map's gathered cells already laid out.
    pub(crate) fn take_cells(&mut self) -> BTreeMap<CellKey, CellContent> {
        self.waiting_cells()
            .into_iter()
            .map(|k| (k, self.take_cell(k)))
            .collect()
    }

    /// The collision of cells laid out apart (the gathered cells of a map in parts, each checked against its own
    /// plan), added to what it lays out: their objects and counts.
    pub(crate) fn add_laid_cells(&mut self, col: CollisionResult, water_col: CollisionResult) {
        self.col.absorb(col);
        self.water_col.absorb(water_col);
    }

    /// The cells `keys` laid out now, from the soups and props they hold (taken out): each leaf packed as the plan
    /// says into the mesh files and the collision's objects, or given to `visit` (nothing written).
    fn lay_cells(&mut self, keys: &BTreeSet<CellKey>, visit: Option<&mut LeafVisit<'_>>) -> Result<()> {
        let surf = self.surfaces.clone();
        let off = self.off;
        let mut collision = Collision::new(&surf, off.x, off.y, off.z, self.o.limit);
        let plan = self.collision_plan.take();
        let (mut col, mut water_col) = (std::mem::take(&mut self.col), std::mem::take(&mut self.water_col));
        let mut files = crate::collision_pack::Files::new();
        let encodes = self.layout.encodes();
        let laid = (|| -> Result<usize> {
            let mut pack =
                crate::collision_pack::Pack::new(&surf, off, plan.as_ref(), &mut files)?.encoding(encodes);
            let mut pack_leaf =
                |leaf: crate::collision::Leaf<'_>, res: &mut CollisionResult| pack.leaf(leaf, res);
            let visit: &mut LeafVisit<'_> = match visit {
                Some(v) => v,
                None => &mut pack_leaf,
            };
            for &key in keys {
                let content = self.take_cell(key);
                let res = if key.0 { &mut water_col } else { &mut col };
                lay_cell(&mut collision, key, content, res, visit)?;
            }
            drop(pack_leaf);
            Ok(pack.seen())
        })();
        self.collision_plan = plan;
        self.col = col;
        self.water_col = water_col;
        self.leaves_packed += laid?;
        self.mesh_files.extend(files);
        Ok(())
    }

    /// Hulls (a placed prop's, a hull soup's) packed now, each whole, among the triangle mesh's collision.
    pub(crate) fn lay_hulls(&mut self, hulls: &[Hull]) -> Result<()> {
        let surf = self.surfaces.clone();
        let mut files = crate::collision_pack::Files::new();
        let mut pack = crate::collision_pack::Pack::new(&surf, self.off, None, &mut files)?
            .encoding(self.layout.encodes());
        for h in hulls {
            pack.hull(&mut self.col, &h.name, &h.pos, &h.idx, h.surface)?;
        }
        self.mesh_files.extend(files);
        Ok(())
    }

    /// The placed props' hulls made so far, taken out (to be packed as they come: [`MapWriter::lay_hulls`]).
    pub(crate) fn take_prop_hulls(&mut self) -> Vec<Hull> {
        std::mem::take(&mut self.prop_hulls)
    }

    /// The map's instancing for the placements [`MapWriter::tiles`] is given next: the placements of each prototype
    /// key over the whole map (`None`: those placements are the whole map's).
    pub(crate) fn set_counts(&mut self, counts: Option<HashMap<u32, i64>>) {
        self.counts = counts;
    }

    /// The placements of the part about to be laid that the pause-map picture leaves out ([`pause_left_of`]).
    pub(crate) fn set_pause_left(&mut self, left: std::collections::HashSet<primase_core::Entity>) {
        self.pause_left = left;
    }

    /// The textures the materials drawn so far use.
    pub(crate) fn used_textures(&self) -> BTreeSet<i32> {
        self.mat_used
            .iter()
            .flat_map(|&id| {
                let m = self.ctx.mats.by_id(id);
                [m.tex, m.normal, m.mask]
            })
            .filter(|t| *t >= 0)
            .collect()
    }

    /// The collision cells a part's props fall in: those of the render tiles `tiles`.
    /// The blend chains as `chains` says.
    pub(crate) fn with_chains(mut self, chains: Chains) -> MapWriter {
        self.chains = chains;
        self
    }

    /// The blend chains as they ended: the recorded ones, or how many tiles' chain overlays were left in them.
    pub(crate) fn chains(&self) -> &Chains {
        &self.chains
    }

    pub(crate) fn take_chains(&mut self) -> Chains {
        std::mem::replace(&mut self.chains, Chains::Off { left: 0 })
    }

    /// Take a tile's blend-chain overlays out of its parts and write both forms finish may choose between
    /// ([`PendingChain`]): objects of their own per grid cell ([`WriterOptions::blend_grid`]), dominant axis and chain
    /// rank, and one plain object. Leaves them in the tile (counted) when an axis has no lift step at the tile's
    /// translation `at`; `None` then, or when the tile has none.
    fn chain_candidates(
        &mut self,
        parts: &mut BTreeMap<usize, Acc>,
        at: Vec3,
        (tx, ty): (i32, i32),
        tile: &str,
    ) -> Result<Option<PendingChain>> {
        let mut ranks: Vec<(usize, u8)> = Vec::new();
        for &k in parts.keys() {
            let m = self.ctx.mats.by_id(k);
            let Some(v) = m.vertex_blend else { continue };
            v.check().map_err(|e| Error::Scene(format!("{}: {e}", m.name)))?;
            if let Some(rank) = v.chain_rank() {
                ranks.push((k, rank));
            }
        }
        if ranks.is_empty() {
            return Ok(None);
        }
        let grid = match &mut self.chains {
            Chains::Off { left } => {
                // left in the tile, drawn in an unstable order
                *left += 1;
                return Ok(None);
            }
            Chains::Record(rec) | Chains::Replay(rec) => rec.grid,
        };
        let input: Vec<(usize, u8, usize, Geometry)> = ranks
            .iter()
            .map(|&(k, rank)| {
                let a = &parts[&k];
                // chain overlays are fixed: the overlay stack never lifts them
                debug_assert!(a.lift.iter().all(|&x| x == 0.0));
                let g = Geometry {
                    pos: a.pos.clone(),
                    nrm: a.nrm.clone(),
                    uv: a.uv.clone(),
                    blend: a.blend.clone(),
                    idx: a.idx.clone(),
                };
                (k, rank, a.seq, g)
            })
            .collect();
        let mut plan = blend_order::plan(&input, at, grid);
        // what this tile's chain form is, as the pass records it and the writer checks it
        let mats = &self.ctx.mats;
        let found = ChainTile {
            column: tx,
            row: ty,
            pieces: ranks
                .iter()
                .flat_map(|(k, _)| parts[k].pieces.iter().map(|p| p.id))
                .collect(),
            clipped: plan.clipped_total as u32,
            steps: plan.steps.clone(),
            groups: plan
                .groups
                .iter()
                .map(|(key, g)| {
                    let (cell, axis, _) = *key;
                    (
                        *key,
                        ChainGroup {
                            members: g.keys().map(|&k| mats.by_id(k).name.clone()).collect(),
                            clipped: plan.clipped.get(key).copied().unwrap_or(0) as u32,
                            pad: plan
                                .boxes
                                .get(&(cell, axis))
                                .map(|(lo, hi)| ([lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z])),
                        },
                    )
                })
                .collect(),
        };
        match &mut self.chains {
            Chains::Record(rec) => {
                rec.tiles.insert(tile.to_owned(), found);
            }
            Chains::Replay(rec) => {
                if rec.tiles.get(tile) != Some(&found) {
                    return Err(Error::Scene(format!(
                        "{tile}: the blend chains (skate.blend_chains) were chosen for other pieces, objects, clipping \
                         or boxes than the writer's tile has: run prepare-reskate-blend-order after the passes that \
                         change geometry, materials or textures"
                    )));
                }
            }
            Chains::Off { .. } => unreachable!("left above"),
        }
        let Some(steps) = plan.steps.clone() else {
            let st = &mut self.r.blend;
            st.tiles_unliftable += 1;
            st.overlay_triangles_unliftable += plan.tris;
            return Ok(None);
        };
        for (k, _) in &ranks {
            parts.remove(k);
        }
        let seq = plan.first_use.clone();
        let acc = |k: usize, h: Geometry| Acc {
            seq: seq[&k],
            pos: h.pos,
            nrm: h.nrm,
            uv: h.uv,
            blend: h.blend,
            idx: h.idx,
            ..Acc::default()
        };
        // the plain form: every rank's part of a material merged, unpadded, at the tile's translation
        let mut merged: BTreeMap<usize, Acc> = BTreeMap::new();
        for group in plan.groups.values() {
            for (&k, g) in group {
                let m = merged.entry(k).or_insert_with(|| acc(k, Geometry::default()));
                let base = (m.pos.len() / 3) as u32;
                m.pos.extend_from_slice(&g.pos);
                m.nrm.extend_from_slice(&g.nrm);
                m.uv.extend_from_slice(&g.uv);
                m.blend.extend_from_slice(&g.blend);
                m.idx.extend(g.idx.iter().map(|i| base + i));
            }
        }
        let mats = &self.ctx.mats;
        let order = draw_order(&merged, |k| draw_class(mats.by_id(k)), &[]);
        let (pr, _) = prims(&merged, mats, &order);
        drop(merged);
        let plain_stem = format!("{tile}_overlays");
        let plain = self.write_mesh(&plain_stem, &pr, at)?;
        // the chain form: every rank of a (cell, axis) padded to the box of all of them
        plan.pad();
        let n = plan.groups.len();
        let mut objects = Vec::new();
        let mut files = Vec::new();
        for (((cx, cy), axis, rank), group) in std::mem::take(&mut plan.groups) {
            let group: BTreeMap<usize, Acc> = group.into_iter().map(|(k, h)| (k, acc(k, h))).collect();
            let mats = &self.ctx.mats;
            let order = draw_order(&group, |k| draw_class(mats.by_id(k)), &[]);
            let (pr, _) = prims(&group, mats, &order);
            let stem = if grid > 0.0 {
                format!("{tile}_c{cx}_{cy}_v{axis}{rank}")
            } else {
                format!("{tile}_v{axis}{rank}")
            };
            let lifted = at + blend_order::axis_vector(axis) * blend_order::lift(rank, steps[&axis]);
            objects.extend(self.write_mesh(&stem, &pr, lifted)?);
            files.push(format!("{stem}.glb"));
        }
        Ok(Some(PendingChain {
            objects,
            files,
            plain,
            plain_file: format!("{plain_stem}.glb"),
            groups: n,
            widened: steps.values().filter(|&&s| s > blend_order::LIFT_STEP).count(),
            max_step: steps.values().copied().fold(0.0, f32::max),
            tris: plan.tris,
            clipped: plan.clipped_total,
        }))
    }

    /// Writes every tile's pending chain overlays in key order (row, then column) in the form the budget planned
    /// ([`blend_order::take_chains`]): the chain form for the tiles `plan-reskate-blend-budget` chose (for a count, or
    /// in pool mode from the map's summary, whose basis, the cull IDs and `other` (what the rest of the map takes),
    /// must be this layout's), the plain form for the rest. The writer chooses nothing: tiles with chains and no
    /// decided budget, a ranking for other tiles, or a pool basis other than this layout's are an error. The form not
    /// taken is deleted.
    fn emit_chains(&mut self, other: usize) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_chains);
        let names: Vec<String> = pending.keys().map(|&(ty, tx)| format!("t_{tx}_{ty}")).collect();
        if names.is_empty() {
            return Ok(());
        }
        // the budget is the preparation's decision (plan-reskate-blend-budget); the writer replays it, never choosing a
        // form of its own
        let b = self.budget.clone().ok_or_else(|| {
            Error::Scene(format!(
                "{} tiles have blend chains (prepare-reskate-blend-order) but no budget decides their form: run \
                 plan-reskate-blend-budget (objects=0 writes every tile plain)",
                names.len()
            ))
        })?;
        let d = b.decision.ok_or_else(|| {
            Error::Scene(
                "the blend budget (skate.blend_budget) states its mode and no decision: it is decided before the map \
                 is written (plan-reskate-blend-budget; a map in parts: finalize-reskate-blend-budget, in the \
                 conversion's finalize stage)"
                    .into(),
            )
        })?;
        if d.ranked != names {
            return Err(Error::Scene(
                "the blend budget (skate.blend_budget) ranks other tiles than the writer's blend chains: run \
                 plan-reskate-blend-budget after prepare-reskate-blend-order"
                    .into(),
            ));
        }
        let (cap, from_pool) = match b.mode {
            BlendBudgetMode::Objects(n) => (n, false),
            BlendBudgetMode::Pool => {
                let basis = d.pool.ok_or_else(|| {
                    Error::Scene(
                        "the blend budget for the pool names no cull-ID basis it was decided on".into(),
                    )
                })?;
                let mine = (self.o.cull_ids, other);
                if (basis.cull_ids, basis.other_cull_ids) != mine {
                    return Err(Error::Scene(format!(
                        "the blend budget was decided for {} of {} cull IDs taken besides the chains, but this map \
                         takes {} of {}: summarize-reskate-map and plan-reskate-blend-budget for the map as written",
                        basis.other_cull_ids, basis.cull_ids, mine.1, mine.0
                    )));
                }
                (self.o.cull_ids.saturating_sub(other), true)
            }
        };
        let take: Vec<bool> = names.iter().map(|t| d.chosen.contains(t)).collect();
        let st = &mut self.r.blend;
        st.cap = cap;
        st.cap_from_pool = from_pool;
        st.other_cull_ids = other;
        st.grid_m = self.chains.grid();
        for (p, chain) in pending.into_values().zip(take) {
            let st = &mut self.r.blend;
            if chain {
                st.objects += p.groups;
                st.tiles_split += 1;
                st.overlay_triangles_split += p.tris;
                st.triangles_clipped += p.clipped;
                st.steps_widened += p.widened;
                st.max_step_m = st.max_step_m.max(p.max_step);
                self.objects.extend(p.objects);
                self.discard_file(&p.plain_file);
            } else {
                st.tiles_shared += 1;
                st.overlay_triangles_shared += p.tris;
                self.objects.extend(p.plain);
                for f in &p.files {
                    self.discard_file(f);
                }
            }
        }
        Ok(())
    }

    /// Drops a mesh file written for a form finish did not take (written already when the map came in parts).
    fn discard_file(&mut self, file: &str) {
        self.mesh_files.remove(file);
        self.discarded.push(file.to_owned());
    }

    /// Writes `meshes/<stem>.glb` and returns an object per primitive (`<stem>`, or `<stem> / section N`) at
    /// translation `at`, each counting the Studio cells it meets.
    fn write_mesh(&mut self, stem: &str, pr: &[GlbPrim], at: Vec3) -> Result<Vec<MapObject>> {
        let rows = MapObject::translation(at.x, at.y, at.z);
        let file = format!("{stem}.glb");
        if self.layout.encodes() {
            self.mesh_files.insert(file.clone(), glb::write(stem, pr)?);
        }
        Ok(pr
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let name = if pr.len() == 1 {
                    stem.to_owned()
                } else {
                    format!("{stem} / section {}", i + 1)
                };
                let mut ob = MapObject::new(name, rows).meeting(&q.pos, at);
                ob.mesh = Some(format!("meshes/{file}"));
                ob.primitive = i;
                ob.material = q.material.clone();
                ob
            })
            .collect())
    }

    /// Stack the mesh's overlay pieces as the stack mode says (not at all; stacked here and recorded; the recorded
    /// stack replayed); returns which material lies under which, `(below, above)`.
    fn stack_overlays(
        &mut self,
        parts: &mut BTreeMap<usize, Acc>,
        origin: Vec3,
        mesh: &str,
    ) -> Result<Vec<(usize, usize)>> {
        let mut pieces: Vec<Piece> = Vec::new();
        let mut src: Vec<(usize, usize)> = Vec::new();
        let mut ids: Vec<OverlayPiece> = Vec::new();
        // the parts in first use (seq), not by material id: ids follow the order materials were first made anywhere in
        // the map, which a map written in parts does not share
        let mut keys: Vec<usize> = parts.keys().copied().collect();
        keys.sort_by_key(|k| parts[k].seq);
        for (pi, mat) in keys.iter().enumerate() {
            let m = self.ctx.mats.by_id(*mat);
            let pass = overlay_pass(m);
            let a = &parts[mat];
            for (i, pc) in a.pieces.iter().enumerate() {
                pieces.push(Piece {
                    part: pi,
                    t0: pc.t0,
                    t1: pc.t1,
                    v0: pc.v0,
                    v1: pc.v1,
                    overlay: matches!(m.class, VClass::Decal | VClass::Water),
                    fixed: m.vertex_blend.is_some(),
                    water: m.class == VClass::Water,
                    pass,
                    order: pc.order,
                    entity: pc.entity,
                    shift: 0.0,
                });
                src.push((*mat, i));
                ids.push(pc.id);
            }
        }
        let step = self.stack.step();
        let stacking = step > 0.0 && pieces.iter().any(|p| p.overlay);
        // which piece lies under which, by piece
        let mut below: Vec<(usize, usize)> = Vec::new();
        {
            let mut accs: Vec<&mut Acc> = parts.values_mut().collect();
            accs.sort_by_key(|a| a.seq);
            let idx: Vec<Vec<u32>> = accs.iter().map(|a| a.idx.clone()).collect();
            let mut view = Parts {
                pos: accs.iter_mut().map(|a| &mut a.pos).collect(),
                idx: idx.iter().collect(),
            };
            match &mut self.stack {
                Stack::Off { unstacked } => {
                    *unstacked += pieces.iter().filter(|p| p.overlay && !p.fixed).count() as u64;
                }
                Stack::Compute(rec) if stacking => {
                    let stacked = overlay::apply(step, &mut view, &mut pieces);
                    self.r.overlay.add(&stacked.stats);
                    let mut totals = stats_of(&rec.totals);
                    totals.add(&stacked.stats);
                    rec.totals = totals_of(&totals);
                    rec.meshes.insert(mesh.to_owned(), totals_of(&stacked.stats));
                    for &(k, u) in &stacked.left {
                        let name = |p: usize| self.ctx.mats.by_id(src[p].0).name.clone();
                        *rec.left.entry((mesh.to_owned(), name(k), name(u))).or_default() += 1;
                    }
                    for (k, p) in pieces.iter().enumerate() {
                        // its centre after the lift, the mesh's frame in scene axes
                        let pos = &*view.pos[p.part];
                        let mut c = Vec3::ZERO;
                        for v in p.v0..p.v1 {
                            c += Vec3::new(pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]);
                        }
                        c = c / (p.v1 - p.v0).max(1) as f32 + origin;
                        rec.pieces.insert(
                            ids[k],
                            StackedPiece {
                                mesh: mesh.to_owned(),
                                material: self.ctx.mats.by_id(src[k].0).name.clone(),
                                overlay: p.overlay,
                                pass: p.pass,
                                order: p.order,
                                shift: p.shift,
                                tris: (p.t1 - p.t0) as u32,
                                centre: [c.x, c.z, -c.y],
                            },
                        );
                    }
                    rec.under
                        .extend(stacked.under.iter().map(|&(lo, hi)| (ids[lo], ids[hi])));
                    below = stacked.under;
                }
                Stack::Replay(rec) if stacking => {
                    for (k, p) in pieces.iter_mut().enumerate() {
                        let Some(stacked) = rec
                            .pieces
                            .get(&ids[k])
                            .filter(|r| r.material == self.ctx.mats.by_id(src[k].0).name)
                        else {
                            return Err(Error::Scene(format!(
                                "{mesh}: the overlay stack (skate.overlay_stack) was made for other pieces or \
                                 materials than the writer draws: run prepare-reskate-overlays after the passes that \
                                 change geometry, materials or textures"
                            )));
                        };
                        p.shift = stacked.shift;
                    }
                    overlay::replay(&mut view, &pieces);
                    let at: HashMap<OverlayPiece, usize> =
                        ids.iter().enumerate().map(|(k, &id)| (id, k)).collect();
                    below = rec
                        .under
                        .iter()
                        .filter_map(|(lo, hi)| Some((*at.get(lo)?, *at.get(hi)?)))
                        .collect();
                }
                Stack::Compute(_) | Stack::Replay(_) => {}
            }
        }
        let mut under: Vec<(usize, usize)> = below
            .iter()
            .map(|&(lo, hi)| (src[lo].0, src[hi].0))
            .filter(|(lo, hi)| lo != hi)
            .collect();
        under.sort_unstable();
        under.dedup();
        for (k, p) in pieces.iter().enumerate() {
            if p.shift > 0.0 {
                let a = parts.get_mut(&src[k].0).expect("a piece's part");
                a.lift.resize(a.pos.len() / 3, 0.0);
                for l in &mut a.lift[p.v0..p.v1] {
                    *l = l.max(p.shift);
                }
            }
        }
        Ok(under)
    }

    #[allow(clippy::too_many_arguments)]
    fn append(
        a: &mut Acc,
        id: OverlayPiece,
        p: &SrcPart,
        s: Option<&Placement>,
        origin: Vec3,
        order: i32,
        only: Option<&[u32]>,
        entity: i32,
    ) {
        let (t0, v0) = (a.idx.len() / 3, a.pos.len() / 3);
        append_core(a, p, s, origin, only);
        if a.idx.len() / 3 > t0 {
            a.pieces.push(AccPiece {
                id,
                t0,
                t1: a.idx.len() / 3,
                v0,
                v1: a.pos.len() / 3,
                order,
                entity,
            });
        }
    }

    /// How the pause-map picture draws map material `mat`'s triangles (the picture's drawing choice `d`).
    fn pause_pick(&self, cache: &mut HashMap<usize, PausePick>, mat: usize, d: &PauseDrawing) -> PausePick {
        *cache.entry(mat).or_insert_with(|| {
            let m = self.ctx.mats.by_id(mat);
            if matches!(m.class, VClass::Decal | VClass::Glass | VClass::Alpha) {
                return PausePick::Hidden;
            }
            if d.leave_out == PauseLeaveOut::Shadowless && m.cast_shadows == Some(false) {
                return PausePick::Shadowless;
            }
            let c = self.ctx.tex.mean_of(m.tex);
            let mean = if c == 0 { 0x010101 } else { c };
            match d.colour {
                PauseColour::Mean => PausePick::Flat(mean),
                PauseColour::Texture if self.ctx.tex.pyramid(m.tex).is_none() => PausePick::Untextured(mean),
                PauseColour::Texture => PausePick::Texture {
                    tex: m.tex,
                    factor: m.base_factor.map_or([1.0; 3], |f| [f[0], f[1], f[2]]),
                    wrap: match m.wrap.as_deref() {
                        Some("extend") => PauseWrap::Clamp,
                        Some("mirror") => PauseWrap::Mirror,
                        _ => PauseWrap::Repeat,
                    },
                },
            }
        })
    }

    /// Triangles `idx` of `pos` (coordinates `uv`), drawn into the pause-map picture as `pick` says, map material `mat`'s,
    /// placed by `origin` and the basis; what is left out or drawn without its texture counted.
    #[allow(clippy::too_many_arguments)]
    fn pause_draw(
        &mut self,
        pick: PausePick,
        mat: usize,
        (pos, uv, idx): (&[f32], &[f32], &[u32]),
        origin: Vec3,
        ex: Vec3,
        ey: Vec3,
        ez: Vec3,
    ) {
        let tris = (idx.len() / 3) as i64;
        let paint = match pick {
            PausePick::Hidden => return,
            PausePick::Shadowless => {
                self.r.pause_shadowless += tris;
                return;
            }
            PausePick::Flat(c) => Paint::Flat(c),
            PausePick::Untextured(c) => {
                let name = self.ctx.mats.by_id(mat).name.clone();
                *self.r.pause_untextured.entry(name).or_default() += tris;
                Paint::Flat(c)
            }
            PausePick::Texture { tex, factor, wrap } => match self.ctx.tex.pyramid(tex) {
                Some(pyramid) => Paint::Texture {
                    pyramid,
                    uv,
                    factor,
                    wrap,
                },
                None => unreachable!("picked only with a pyramid"),
            },
        };
        if let Some(pm) = &mut self.pm {
            pm.draw(pos, idx, origin, ex, ey, ez, &paint);
        }
    }

    /// One placement's prototype collision: a hull written whole, else its triangles into the tile's solid or water
    /// soups (clipped to `edge` when given).
    #[allow(clippy::too_many_arguments)]
    fn add_collision(
        &mut self,
        m: &SrcMesh,
        s: &Placement,
        (tx, ty): (i32, i32),
        hull_no: &mut usize,
        soups: &mut BTreeMap<(i32, i32), PropSoup>,
        water_soups: &mut BTreeMap<(i32, i32), PropSoup>,
        soup_order: &mut Vec<(i32, i32)>,
        edge: Option<[f32; 4]>,
    ) {
        let (Some(cp), Some(ci)) = (&m.col_pos, &m.col_idx) else {
            return;
        };
        if m.col_mode == CollisionMode::Hull {
            let name = format!("h_{tx}_{ty}_{hull_no}_{}", m.name);
            *hull_no += 1;
            self.prop_hulls.push(Hull {
                entity: s.entity.expect("a scene placement has its entity"),
                name,
                pos: placed_points(cp, s),
                idx: ci.clone(),
                surface: m.col_surf.first().map_or(0, |b| (b & 0x7F) as usize),
                ground: m.col_ground,
            });
            return;
        }
        let target = if m.col_mode == CollisionMode::Water {
            water_soups
        } else {
            soups
        };
        let n = crate::collision::add_prop(
            target,
            soup_order,
            cp,
            ci,
            &m.col_surf,
            m.col_ground,
            s.pos,
            s.ex,
            s.ey,
            s.ez,
            edge,
        );
        if n > 0 {
            *self.r.prop_col_tris.entry(m.name.clone()).or_default() += n;
        }
    }

    /// The identity of a prototype drawn with variant row `row`, the same in every part: its parts' content
    /// ([`SrcMesh::content`]) and the names of the materials the row draws them with (names end in a content hash).
    /// SHA-256, first 128 bits.
    fn proto_key(&mut self, m: &SrcMesh, row: u32, source: &mut dyn MeshSource) -> Result<[u8; 16]> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(m.content);
        for p in &m.parts {
            let mat = source.material_of(p, row, &mut self.ctx)?;
            let name = &self.ctx.mats.by_id(mat).name;
            h.update((name.len() as u64).to_le_bytes());
            h.update(name.as_bytes());
        }
        let mut k = [0u8; 16];
        k.copy_from_slice(&h.finalize()[..16]);
        Ok(k)
    }

    fn make_proto(
        &mut self,
        m: &SrcMesh,
        row: u32,
        key: [u8; 16],
        source: &mut dyn MeshSource,
    ) -> Result<Proto> {
        let mut parts: BTreeMap<usize, Acc> = BTreeMap::new();
        let prototype = m.entity.expect("a scene prototype has its entity");
        for (order, p) in m.parts.iter().enumerate() {
            let mat = source.material_of(p, row, &mut self.ctx)?;
            Self::append(
                part(&mut parts, mat),
                OverlayPiece::Proto {
                    prototype,
                    row,
                    part: order as u32,
                },
                p,
                None,
                Vec3::ZERO,
                order as i32,
                None,
                0,
            );
        }
        let under = self.stack_overlays(&mut parts, Vec3::ZERO, &format!("proto {}", m.name))?;
        if self.o.max_render_edge > 0.0 {
            for a in parts.values_mut() {
                split_long(a, self.o.max_render_edge);
            }
        }
        for k in parts.keys() {
            self.mat_used.insert(*k);
        }
        let info = self.r.proto_list.len();
        self.r.proto_list.push(ProtoInfo {
            id: info,
            name: m.name.clone(),
            file: String::new(),
            tris: m.tris,
            verts: m.verts,
            parts: parts.len(),
            count: 0,
            tree: m.is_tree,
        });
        let stem = format!("p_{}", key.iter().map(|b| format!("{b:02x}")).collect::<String>());
        self.r.proto_list[info].file = format!("{stem}.glb");
        let mats = &self.ctx.mats;
        let order = draw_order(&parts, |k| draw_class(mats.by_id(k)), &under);
        let (pr, lifts) = prims(&parts, mats, &order);
        record_lifts(&mut self.r, format!("meshes/{stem}.glb"), lifts);
        if self.layout.encodes() {
            self.mesh_files
                .insert(format!("{stem}.glb"), glb::write(&stem, &pr)?);
        }
        let boxes: Vec<Option<(Vec3, Vec3)>> = pr
            .iter()
            .map(|p| {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for v in (0..p.pos.len().saturating_sub(2)).step_by(3) {
                    let q = Vec3::new(p.pos[v], p.pos[v + 1], p.pos[v + 2]);
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
                (!p.pos.is_empty()).then_some((lo, hi))
            })
            .collect();
        let origins = boxes
            .iter()
            .map(|b| {
                let (lo, hi) = (*b)?;
                let half = (hi - lo) * 0.5;
                let c = (hi + lo) * 0.5;
                (half.x.max(half.y.max(half.z)) <= 32.0).then_some([c.x, c.z, -c.y])
            })
            .collect();
        Ok(Proto {
            info,
            materials: pr
                .iter()
                .map(|p| p.material.clone().unwrap_or_default())
                .collect(),
            origins,
            boxes,
        })
    }

    /// Writes the tile meshes and prototypes of every placement, band by band.
    pub fn tiles(&mut self, sels: &[Placement], source: &mut dyn MeshSource) -> Result<()> {
        let o = self.o.clone();
        let r = o.region;
        let off = self.off;
        // what is instanced: the map's counts when given (a map in parts), else these placements'
        let arch_count: HashMap<u32, i64> = match &self.counts {
            Some(c) => c.clone(),
            None => {
                let mut c: HashMap<u32, i64> = HashMap::new();
                for s in sels {
                    *c.entry(s.proto).or_default() += 1;
                }
                c
            }
        };
        self.placed_keys.extend(sels.iter().map(|s| s.proto));
        self.r.archetypes = self.placed_keys.len();
        let row_min = sels.iter().map(|s| s.ty).min().unwrap_or(0);
        let row_max = sels.iter().map(|s| s.ty).max().unwrap_or(0);
        let n_bands = ((row_max - row_min) / o.band_rows + 1) as usize;
        let mut per_band: Vec<Vec<Placement>> = vec![Vec::new(); n_bands];
        let mut arch_last: HashMap<u32, usize> = HashMap::new();
        for s in sels {
            let b = ((s.ty - row_min) / o.band_rows) as usize;
            per_band[b].push(s.clone());
            let e = arch_last.entry(s.proto).or_insert(b);
            if *e < b {
                *e = b;
            }
        }
        let mut meshes: HashMap<u32, SrcMesh> = HashMap::new();
        let mut tiles: Vec<TileOut> = Vec::new();
        let edge = apron(r, o.apron);
        let rules = PlacementRules::of(&o);
        let mut colours: HashMap<usize, PausePick> = HashMap::new();
        for (band, band_sels) in per_band.iter().enumerate() {
            if band_sels.is_empty() {
                continue;
            }
            source.load(band_sels, &mut meshes, &mut self.ctx)?;
            self.r.peak_mesh_bytes = self.r.peak_mesh_bytes.max(meshes.values().map(|m| m.bytes).sum());
            // tiles of the band, in the order their first placement appears
            let mut groups: Vec<((i32, i32), Vec<&Placement>)> = Vec::new();
            let mut gi: HashMap<(i32, i32), usize> = HashMap::new();
            for s in band_sels {
                let k = (s.tx, s.ty);
                let i = *gi.entry(k).or_insert_with(|| {
                    groups.push((k, Vec::new()));
                    groups.len() - 1
                });
                groups[i].1.push(s);
            }
            for ((tx, ty), mut g) in groups {
                let origin = Vec3::new((tx as f32 + 0.5) * o.tile, (ty as f32 + 0.5) * o.tile, 0.0);
                let mut to = TileOut::new(tx, ty);
                let mut parts: BTreeMap<usize, Acc> = BTreeMap::new();
                let (mut piece_order, mut entity_no, mut hull_no) = (0i32, 0i32, 0usize);
                let mut soups: BTreeMap<(i32, i32), PropSoup> = BTreeMap::new();
                let mut water_soups: BTreeMap<(i32, i32), PropSoup> = BTreeMap::new();
                let mut soup_order = Vec::new();
                // a total order that every part shares: by content, not the scene's prototype key (an identity
                // within one part only), then everything of the placement that reaches the output (position, basis,
                // variant row, flags, parts left off); placements equal in all of these write the same thing in either
                // order
                let content = |s: &Placement| meshes.get(&s.proto).map(|m| m.content);
                let place = |s: &Placement| {
                    [
                        s.pos.x, s.pos.y, s.pos.z, s.ex.x, s.ex.y, s.ex.z, s.ey.x, s.ey.y, s.ey.z, s.ez.x,
                        s.ez.y, s.ez.z,
                    ]
                };
                g.sort_by(|a, b| {
                    content(a)
                        .cmp(&content(b))
                        .then_with(|| {
                            place(a)
                                .iter()
                                .zip(place(b).iter())
                                .map(|(x, y)| x.total_cmp(y))
                                .find(|o| o.is_ne())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .then(a.variant.cmp(&b.variant))
                        .then(a.no_collision.cmp(&b.no_collision))
                        .then(a.no_instance.cmp(&b.no_instance))
                        .then(a.outside.cmp(&b.outside))
                        .then_with(|| a.part_off.cmp(&b.part_off))
                });
                for s in g {
                    let fate = placement_fate(
                        meshes.get(&s.proto),
                        s,
                        |h| arch_count.get(&h).copied().unwrap_or(0),
                        &rules,
                        edge,
                        off,
                    );
                    let (m, inst, kept_parts, collision) = match fate {
                        Fate::EmptyMesh => {
                            self.r.empty_mesh += 1;
                            continue;
                        }
                        Fate::Degenerate => {
                            return Err(Error::Scene(format!(
                                "placement {:?}: its transform is degenerate (an axis of length 0 or not finite); \
                                 leave it out or fix it before the writer",
                                s.entity.expect("a scene placement has its entity")
                            )));
                        }
                        Fate::FarDropped => {
                            self.r.far_dropped += 1;
                            continue;
                        }
                        Fate::CollisionOnly { collision } => {
                            self.r.collision_only += 1;
                            if let (true, Some(clip), Some(m)) =
                                (self.layout.collides(), collision, meshes.get(&s.proto))
                            {
                                self.add_collision(
                                    m,
                                    s,
                                    (tx, ty),
                                    &mut hull_no,
                                    &mut soups,
                                    &mut water_soups,
                                    &mut soup_order,
                                    clip,
                                );
                            }
                            continue;
                        }
                        Fate::Drawn {
                            inst,
                            kept_parts,
                            clipped,
                            collision,
                        } => {
                            to.entities += 1;
                            if clipped {
                                self.r.limit_instances += 1;
                                continue;
                            }
                            if kept_parts.as_ref().is_some_and(|k| k.iter().any(Option::is_some)) {
                                self.r.apron_trimmed += 1;
                            }
                            (meshes.get(&s.proto).expect("drawn"), inst, kept_parts, collision)
                        }
                    };
                    let row = if m.has_variants { s.variant } else { 0 };
                    if let (true, Some(clip)) = (self.layout.collides(), collision) {
                        self.add_collision(
                            m,
                            s,
                            (tx, ty),
                            &mut hull_no,
                            &mut soups,
                            &mut water_soups,
                            &mut soup_order,
                            clip,
                        );
                    }
                    // a collision layout builds no render mesh
                    if !self.layout.renders() {
                        continue;
                    }
                    let m = m.clone();
                    if inst {
                        let key = self.proto_key(&m, row, source)?;
                        if !self.protos.contains_key(&key) {
                            let p = self.make_proto(&m, row, key, source)?;
                            self.protos.insert(key, p);
                        }
                        let info = self.protos[&key].info;
                        self.r.proto_list[info].count += 1;
                        let iname = format!("i_{tx}_{ty}_{}_{}", to.instances, self.r.proto_list[info].name);
                        let rows = MapObject::rows(s.ex, s.ey, s.ez, s.pos + off);
                        if o.pause_drawing().is_some()
                            && s.entity.is_some_and(|e| self.pause_left.contains(&e))
                        {
                            self.r.pause_placements +=
                                m.parts.iter().map(|p| (p.idx.len() / 3) as i64).sum::<i64>();
                        } else if let Some(d) = o.pause_drawing() {
                            for p in &m.parts {
                                let mat = source.material_of(p, row, &mut self.ctx)?;
                                let pick = self.pause_pick(&mut colours, mat, d);
                                self.pause_draw(
                                    pick,
                                    mat,
                                    (&p.pos, &p.uv, &p.idx),
                                    s.pos + off,
                                    s.ex,
                                    s.ey,
                                    s.ez,
                                );
                            }
                        }
                        let pr = &self.protos[&key];
                        let file = self.r.proto_list[pr.info].file.clone();
                        for (i, mat) in pr.materials.iter().enumerate() {
                            let name = if pr.materials.len() == 1 {
                                iname.clone()
                            } else {
                                format!("{iname} / section {}", i + 1)
                            };
                            let mut ob = MapObject::new(name, rows);
                            if let Some((lo, hi)) = pr.boxes[i] {
                                // the placed box: its eight corners through the placement
                                let (mut a, mut b) = ([f32::MAX; 2], [f32::MIN; 2]);
                                for c in 0..8 {
                                    let x = if c & 1 != 0 { hi.x } else { lo.x };
                                    let y = if c & 2 != 0 { hi.y } else { lo.y };
                                    let z = if c & 4 != 0 { hi.z } else { lo.z };
                                    let w = s.pos + off + s.ex * x + s.ey * y + s.ez * z;
                                    a = [a[0].min(w.x), a[1].min(w.y)];
                                    b = [b[0].max(w.x), b[1].max(w.y)];
                                }
                                ob.cells = crate::model::studio_cells(a, b);
                            }
                            ob.mesh = Some(format!("meshes/{file}"));
                            ob.primitive = i;
                            ob.material = Some(mat.clone());
                            ob.shared_origin = pr.origins[i];
                            self.objects.push(ob);
                        }
                        to.instances += 1;
                        self.r.instances += 1;
                        self.r.instanced_tris += m.tris;
                        continue;
                    }
                    self.r.merged_entities += 1;
                    entity_no += 1;
                    for (pi, p) in m.parts.iter().enumerate() {
                        let only = kept_parts.as_ref().map(|k| k[pi].as_deref());
                        // a clipped piece's part keeps nothing (or had nothing): left out, before its material is made
                        if only.is_some_and(|k| k.map_or(p.idx.is_empty(), <[u32]>::is_empty)) {
                            continue;
                        }
                        if s.part_off.as_ref().is_some_and(|off| off[pi]) {
                            continue;
                        }
                        let mat = source.material_of(p, row, &mut self.ctx)?;
                        let only = only.flatten();
                        Self::append(
                            part(&mut parts, mat),
                            OverlayPiece::Placed {
                                placement: s.entity.expect("a scene placement has its entity"),
                                part: pi as u32,
                            },
                            p,
                            Some(s),
                            origin,
                            piece_order,
                            only,
                            entity_no,
                        );
                        piece_order += 1;
                    }
                }
                let under = if self.layout.renders() {
                    self.stack_overlays(&mut parts, origin, &format!("t_{tx}_{ty}"))?
                } else {
                    Vec::new()
                };
                if !parts.is_empty() {
                    to.mesh = Some(format!("t_{tx}_{ty}.glb"));
                    if let Some(d) = o.pause_drawing() {
                        for (&k, a) in &parts {
                            let pick = self.pause_pick(&mut colours, k, d);
                            // the triangles of placements left out by name, by the pieces they were appended as
                            let kept = (!self.pause_left.is_empty()).then(|| {
                                let mut left = vec![false; a.idx.len() / 3];
                                for pc in &a.pieces {
                                    if let OverlayPiece::Placed { placement, .. } = pc.id
                                        && self.pause_left.contains(&placement)
                                    {
                                        left[pc.t0..pc.t1].iter_mut().for_each(|l| *l = true);
                                    }
                                }
                                let gone = left.iter().filter(|l| **l).count() as i64;
                                let idx: Vec<u32> = a
                                    .idx
                                    .as_chunks::<3>()
                                    .0
                                    .iter()
                                    .zip(&left)
                                    .filter(|(_, l)| !**l)
                                    .flat_map(|(t, _)| *t)
                                    .collect();
                                (idx, gone)
                            });
                            let idx = match &kept {
                                Some((idx, gone)) => {
                                    self.r.pause_placements += gone;
                                    idx.as_slice()
                                }
                                None => a.idx.as_slice(),
                            };
                            self.pause_draw(
                                pick,
                                k,
                                (&a.pos, &a.uv, idx),
                                Vec3::new(origin.x + off.x, origin.y + off.y, off.z),
                                Vec3::UNIT_X,
                                Vec3::UNIT_Y,
                                Vec3::UNIT_Z,
                            );
                        }
                    }
                    let lim = o.limit;
                    let (ox, oy) = (origin.x + off.x, origin.y + off.y);
                    if ox.abs() + 600.0 > lim || oy.abs() + 600.0 > lim {
                        for a in parts.values_mut() {
                            let mut keep_idx: Vec<u32> = Vec::with_capacity(a.idx.len());
                            for t in a.idx.as_chunks::<3>().0 {
                                let inside = t.iter().all(|&v| {
                                    let v = v as usize * 3;
                                    (a.pos[v] + ox).abs() <= lim && (a.pos[v + 1] + oy).abs() <= lim
                                });
                                if inside {
                                    keep_idx.extend_from_slice(t);
                                }
                            }
                            if keep_idx.len() != a.idx.len() {
                                self.r.limit_triangles += ((a.idx.len() - keep_idx.len()) / 3) as i64;
                                let mut remap = vec![u32::MAX; a.pos.len() / 3];
                                let (mut cp, mut cn, mut cu, mut cl, mut cb) =
                                    (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
                                for i in &mut keep_idx {
                                    let vi = *i as usize;
                                    if remap[vi] == u32::MAX {
                                        remap[vi] = (cp.len() / 3) as u32;
                                        cp.extend_from_slice(&a.pos[vi * 3..vi * 3 + 3]);
                                        cn.extend_from_slice(&a.nrm[vi * 3..vi * 3 + 3]);
                                        cu.extend_from_slice(&a.uv[vi * 2..vi * 2 + 2]);
                                        if !a.blend.is_empty() {
                                            cb.push(a.blend[vi]);
                                        }
                                        if !a.lift.is_empty() {
                                            cl.push(a.lift[vi]);
                                        }
                                    }
                                    *i = remap[vi];
                                }
                                a.pos = cp;
                                a.nrm = cn;
                                a.uv = cu;
                                a.blend = cb;
                                a.lift = cl;
                                a.idx = keep_idx;
                            }
                        }
                    }
                    parts.retain(|_, a| !a.idx.is_empty());
                    if o.max_render_edge > 0.0 {
                        for a in parts.values_mut() {
                            split_long(a, o.max_render_edge);
                        }
                    }
                    // the order over every part, before the chain overlays leave: the relations through them still
                    // order the parts that stay (prims skips the parts that left)
                    let order = {
                        let mats = &self.ctx.mats;
                        draw_order(&parts, |k| draw_class(mats.by_id(k)), &under)
                    };
                    let at = Vec3::new(
                        origin.x + off.x,
                        origin.y + off.y,
                        if off.z != 0.0 { origin.z + off.z } else { origin.z },
                    );
                    // the chain overlays' materials and triangles count as the tile's, whichever form finish takes
                    let mut chained: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
                    for (&k, a) in &parts {
                        if self
                            .ctx
                            .mats
                            .by_id(k)
                            .vertex_blend
                            .and_then(|v| v.chain_rank())
                            .is_some()
                        {
                            chained.insert(k, ((a.idx.len() / 3) as i64, (a.pos.len() / 3) as i64));
                        }
                    }
                    let tile = format!("t_{tx}_{ty}");
                    let chain = self.chain_candidates(&mut parts, at, (tx, ty), &tile)?;
                    if chain.is_none() {
                        // left in the tile: counted with its parts below
                        chained.clear();
                    }
                    let mats = &self.ctx.mats;
                    let (pr, lifts) = prims(&parts, mats, &order);
                    record_lifts(&mut self.r, format!("meshes/{tile}.glb"), lifts);
                    let obs = self.write_mesh(&tile, &pr, at)?;
                    self.objects.extend(obs);
                    to.parts = pr.len();
                    for (&k, &(t, v)) in &chained {
                        to.tris += t;
                        to.verts += v;
                        *self.mat_tris.entry(k).or_default() += t;
                        self.mat_used.insert(k);
                    }
                    for (k, a) in &parts {
                        to.tris += (a.idx.len() / 3) as i64;
                        to.verts += (a.pos.len() / 3) as i64;
                        *self.mat_tris.entry(*k).or_default() += (a.idx.len() / 3) as i64;
                        self.mat_used.insert(*k);
                    }
                    self.r.merged_tris += to.tris;
                    if let Some(c) = chain {
                        self.pending_chains.insert((ty, tx), c);
                    }
                }
                if to.mesh.is_some() || to.instances > 0 {
                    tiles.push(to);
                }
                for (k, v) in soups {
                    self.prop_parts.entry(k).or_default().push((tx, ty, v));
                }
                for (k, v) in water_soups {
                    self.water_parts.entry(k).or_default().push((tx, ty, v));
                }
            }
            // drop the meshes no later band uses
            for (k, last) in &arch_last {
                if *last == band {
                    meshes.remove(k);
                }
            }
        }
        self.r.tiles.extend(tiles);
        self.r.tiles.sort_by(|a, b| a.y.cmp(&b.y).then(a.x.cmp(&b.x)));
        self.r.prototype_tris = self.r.proto_list.iter().map(|p| p.tris).sum();
        Ok(())
    }
}

/// --max-render-edge on one merged part.
fn split_long(a: &mut Acc, max: f32) {
    let m = primase_meshops::Mesh {
        positions: a
            .pos
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
        normals: a
            .nrm
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
        uv: a.uv.as_chunks::<2>().0.iter().map(|c| [c[0], c[1]]).collect(),
        indices: a.idx.clone(),
    };
    let (out, _) = primase_meshops::split_long(&m, max, usize::MAX).expect("finite mesh");
    if !a.blend.is_empty() {
        // the split interpolates UVs only: the same split (same positions and triangles, so the same new vertices in
        // the same order) of the blend weights in the UV slot interpolates them the same way
        let wm = primase_meshops::Mesh {
            uv: a.blend.iter().map(|&w| [w, 0.0]).collect(),
            ..m.clone()
        };
        let (wout, _) = primase_meshops::split_long(&wm, max, usize::MAX).expect("finite mesh");
        assert!(
            wout.indices == out.indices,
            "the blend split follows the geometry split"
        );
        a.blend = wout.uv.iter().map(|w| w[0]).collect();
    }
    a.pos = out.positions.iter().flatten().copied().collect();
    a.nrm = out.normals.iter().flatten().copied().collect();
    a.uv = out.uv.iter().flatten().copied().collect();
    a.idx = out.indices;
    // a new vertex is the midpoint of two others, so its lift error is at most the larger of theirs
    if !a.lift.is_empty() {
        let top = a.lift.iter().copied().fold(0.0, f32::max);
        a.lift.resize(a.pos.len() / 3, top);
    }
}

/// One collision cell's content as the writer lays it out: its world soups' triangles and the props its render tiles
/// file in it, by tile.
#[derive(Clone, Debug, Default)]
pub(crate) struct CellContent {
    pub segs: Vec<SoupSeg>,
    pub props: Vec<(i32, i32, PropSoup)>,
}

impl CellContent {
    /// Its triangles: the soups' and the props', as filed (before the cell's own cleaning).
    pub(crate) fn triangles(&self) -> usize {
        self.segs.iter().map(|g| g.s.len()).sum::<usize>()
            + self.props.iter().map(|(_, _, p)| p.surf.len()).sum::<usize>()
    }
}

/// The cell `key` laid out alone ([`Collision::cell`]) from `content`, each leaf given to `visit`: its world triangles
/// in the scene's soup order, its props by tile, whatever part they came from. The whole map lays out each of its cells
/// so ([`MapWriter::lay_cells`]); a map in parts lays out each gathered cell so.
pub(crate) fn lay_cell(
    collision: &mut Collision<'_>,
    (water, tx, ty): CellKey,
    content: CellContent,
    res: &mut CollisionResult,
    visit: &mut LeafVisit<'_>,
) -> Result<()> {
    let props = (!content.props.is_empty()).then(|| join(content.props));
    collision.cell(
        soup_cell(&content.segs),
        props.as_ref(),
        (tx, ty),
        if water { "w" } else { "c" },
        water,
        res,
        visit,
    )?;
    Ok(())
}

/// A cell's props from every render tile that placed some in it, as one soup: by tile (row, then column), whatever
/// part drew the tile.
fn join(mut bag: Vec<(i32, i32, PropSoup)>) -> PropSoup {
    bag.sort_by(|x, y| x.1.cmp(&y.1).then(x.0.cmp(&y.0)));
    let mut soup = PropSoup::default();
    for p in &bag {
        soup.append(&p.2);
    }
    soup
}

/// What `finish` places besides the tiles.
/// The water surfaces' material name in map.json: Studio's ocean.
const OCEAN: &str = "ocean_water";

/// What [`MapWriter::lay_out`] gives [`MapWriter::finish`].
struct LaidOut {
    o: WriterOptions,
    off: Vec3,
    markers: Vec<Marker>,
    col: CollisionResult,
    water: Option<MapObject>,
    kept_lights: Vec<PlacedLight>,
    other: CullCount,
}

/// The cull IDs a map's content takes besides its blend chains, as the writer lays it out (an ID per entity Studio
/// makes, each object once per 200 m cell it meets, and per light), by how a map's parts merge them
/// ([`crate::components::SkateMapSummary`]).
#[derive(Debug, Default)]
pub(crate) struct CullCount {
    /// What only the part laid out writes: its tiles, instances, placed props' hulls and collision cells.
    pub summed: usize,
    /// Standalone content another part may write too: its lights, grind prisms and hull soups, each with its entity
    /// and its cull IDs. A list, not keyed: an entity is one of a part's, and a map written from several parts may hold
    /// two parts' made content under one local index (its count keys each by [`crate::passes::Owned::key`]).
    pub shared: Vec<(crate::components::SharedContent, primase_core::Entity, usize)>,
    /// Whether it writes the water (one object for the whole map).
    pub water: bool,
}

impl CullCount {
    /// Every cull ID counted.
    pub(crate) fn total(&self) -> usize {
        self.summed + self.shared.iter().map(|&(_, _, n)| n).sum::<usize>() + usize::from(self.water)
    }
}

#[derive(Debug)]
pub struct Extras {
    /// The hull soups not packed yet (named).
    pub hulls: Vec<Hull>,
    pub water: Vec<WaterQuad>,
    pub lights: Vec<PlacedLight>,
    pub markers: Vec<Marker>,
    pub grind: Vec<GrindCurve>,
}

fn map_vec(v: Vec3) -> [f32; 3] {
    [v.x, v.z, -v.y]
}

/// The light records of `kept`, named `prefix` + number, and how many had no usable tangent (their basis is built
/// from a fallback axis: X, or Z for a light along X, made square to the light, since Studio refuses a basis that is
/// not orthogonal).
fn lights_json(kept: &[PlacedLight], off: Vec3, prefix: &str) -> (Vec<LightJson>, usize) {
    let mut fallback = 0;
    let lights = kept
        .iter()
        .enumerate()
        .map(|(n, l)| {
            let fwd = l.dir;
            let mut right = l.tan - fwd * l.tan.dot(fwd);
            if right.length_squared() > 1e-12 {
                right = right.normalize();
            } else {
                let axis = if fwd.x.abs() < 0.9 {
                    Vec3::UNIT_X
                } else {
                    Vec3::UNIT_Z
                };
                right = (axis - fwd * axis.dot(fwd)).normalize();
                fallback += 1;
            }
            let up = fwd.cross(right).normalize();
            LightJson {
                name: format!("{prefix}{n:06}"),
                source_path: l.source.clone(),
                kind: if l.spot { "spot" } else { "point" },
                position: map_vec(l.pos + off),
                direction: map_vec(fwd),
                basis: Basis {
                    right: map_vec(right),
                    up: map_vec(up),
                    forward: map_vec(fwd),
                },
                color_linear: [l.r, l.g, l.b],
                energy: l.energy,
                normalize_power: true,
                uses_light_nodes: false,
                exposure: 0,
                cast_shadow: l.cast_shadow,
                use_custom_distance: true,
                cutoff_distance: l.range,
                diffuse_factor: 1,
                specular_factor: 1,
                time_of_day: l.time_of_day,
                native_entity_authored: false,
                radius: l.radius,
                spot_size_radians: l.spot.then_some(l.spot_size),
                spot_blend: l.spot.then_some(l.spot_blend),
            }
        })
        .collect();
    (lights, fallback)
}

impl MapWriter {
    /// Every water surface it is given (which and how far: `prepare-reskate-water`) as one mesh with Studio's ocean
    /// material, each outline clipped to the map's extent limit (counted when it cuts) and drawn as a fan; an outline
    /// with no area left is not drawn (counted).
    fn water(&mut self, quads: &[WaterQuad], material: &str) -> Result<Option<MapObject>> {
        let (r, off, limit) = (self.o.region, self.off, self.o.limit);
        let origin = Vec3::new((r[0] + r[2]) as f32 / 2.0, (r[1] + r[3]) as f32 / 2.0, 0.0);
        let bounds = [-limit - off.x, -limit - off.y, limit - off.x, limit - off.y];
        let (mut pos, mut nrm, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::<u32>::new());
        for q in quads {
            let z = q.z;
            let mut cs = crate::passes::water::clip_rect(&q.corners, bounds);
            if cs != q.corners {
                self.r.water_cut_at_limit += 1;
            }
            let a = crate::passes::water::signed_area2(&cs);
            if cs.len() < 3 || a == 0.0 {
                self.r.water_without_area += 1;
                continue;
            }
            if a < 0.0 {
                cs.reverse();
            }
            let v0 = (pos.len() / 3) as u32;
            for c in &cs {
                pos.extend_from_slice(&[c.0 - origin.x, c.1 - origin.y, z]);
                nrm.extend_from_slice(&[0.0, 0.0, 1.0]);
                uv.extend_from_slice(&[c.0 * 0.05, -c.1 * 0.05]);
            }
            for i in 1..cs.len() - 1 {
                idx.extend_from_slice(&[v0, v0 + i as u32, v0 + i as u32 + 1]);
            }
        }
        if idx.is_empty() {
            return Ok(None);
        }
        if self.layout.encodes() {
            self.mesh_files.insert(
                "water.glb".into(),
                glb::write(
                    "water",
                    &[GlbPrim {
                        blend: None,
                        material: Some(material.to_owned()),
                        pos,
                        nrm: Some(nrm),
                        uv: Some(uv),
                        idx,
                    }],
                )?,
            );
        }
        let mut ob = MapObject::new(
            "water".into(),
            MapObject::translation(
                origin.x + off.x,
                origin.y + off.y,
                if off.z != 0.0 { off.z } else { 0.0 },
            ),
        );
        ob.mesh = Some("meshes/water.glb".into());
        ob.material = Some(material.to_owned());
        ob.collision_mode = "water";
        ob.collision_packed = MapObject::WATER_PACKED;
        Ok(Some(ob))
    }

    fn material_json(&self, m: &Material) -> MaterialJson {
        let files = self.ctx.tex.all();
        if m.ocean == Some(true) && m.class == VClass::Water {
            return MaterialJson::Ocean(OceanMaterial::OCEAN);
        }
        let domain = match m.domain {
            1 => "decal",
            2 => "foliage",
            _ => "surface",
        };
        let file = |t: i32| (t >= 0).then(|| format!("textures/{}", files[t as usize].file));
        MaterialJson::Surface(Box::new(SurfaceMaterial {
            surface: "default",
            srgb: m.srgb,
            domain,
            shader_override: domain,
            alpha_source: if m.alpha == 1 {
                "constant"
            } else {
                "base_color_texture"
            },
            alpha_cutoff: m.cutoff,
            cast_shadows: m
                .cast_shadows
                .unwrap_or(m.class != VClass::Decal && m.class != VClass::Water),
            double_sided: m.two_sided,
            transparent_shadow: m.alpha == 2,
            alpha: match m.alpha {
                2 => "mask",
                3 => "blend",
                _ => "opaque",
            },
            blend_method: if m.alpha == 3 { "blend" } else { "clip" },
            roughness: m
                .roughness
                .unwrap_or(if matches!(m.class, VClass::Glass | VClass::Water) {
                    0.15
                } else {
                    0.85
                }),
            metallic: m.metallic.unwrap_or(0.0),
            texture: file(m.tex),
            base_color_factor: m.base_factor,
            base_color_extension: m.wrap.clone(),
            normal_texture: file(m.normal),
            normal_srgb: (m.normal >= 0).then_some(false),
            mask_texture: file(m.mask),
            mask_srgb: (m.mask >= 0).then_some(false),
            vertex_blend: m.vertex_blend.is_some().then_some(true),
        }))
    }

    fn effects_json(&mut self, markers: &[Marker], kind: &str, col: &CollisionResult) -> Vec<EffectJson> {
        let mut fx: Vec<(String, String, [f32; 12])> = Vec::new();
        for (i, m) in markers.iter().enumerate() {
            if m.kind != kind {
                continue;
            }
            let (mut x, mut y, mut z) = (m.x, m.y, if m.z.is_nan() { 0.0 } else { m.z });
            if m.snap.mode != SnapMode::None {
                if !col.ground[i].is_nan() {
                    (x, y) = col.placed[i];
                    z = col.ground[i];
                } else {
                    self.r.warnings.push(format!(
                        "{kind} '{}': its snap found nothing",
                        m.name.as_deref().unwrap_or("")
                    ));
                }
            }
            if !m.lift.is_nan() {
                z += m.lift as f32;
            }
            let yaw = m.yaw * std::f64::consts::PI / 180.0;
            let mut t = m.transform.unwrap_or_else(|| {
                MapObject::rows(
                    Vec3::new(yaw.cos() as f32, (-yaw.sin()) as f32, 0.0),
                    Vec3::new(yaw.sin() as f32, yaw.cos() as f32, 0.0),
                    Vec3::UNIT_Z,
                    Vec3::ZERO,
                )
            });
            t[9] = x + self.off.x;
            t[10] = z + self.off.z;
            t[11] = -(y + self.off.y);
            fx.push((
                m.name.clone().unwrap_or_default(),
                m.blueprint
                    .clone()
                    .unwrap_or_default()
                    .replace('\\', "/")
                    .to_lowercase(),
                t,
            ));
        }
        fx.sort_by(|a, b| {
            a.0.cmp(&b.0).then_with(|| {
                for k in 0..12 {
                    let c = a.2[k].partial_cmp(&b.2[k]).unwrap_or(std::cmp::Ordering::Equal);
                    if c != std::cmp::Ordering::Equal {
                        return c;
                    }
                }
                std::cmp::Ordering::Equal
            })
        });
        fx.into_iter()
            .map(|(name, blueprint, transform)| EffectJson {
                name,
                blueprint,
                transform,
            })
            .collect()
    }

    fn object_json(ob: &MapObject) -> ObjectJson<'_> {
        let shared = ob.render && ob.shared_origin.is_some();
        ObjectJson {
            name: &ob.name,
            placement_mode: "authored_mesh",
            mesh: ob.render.then(|| ob.mesh.as_deref().unwrap_or("")),
            primitive: ob.render.then_some(ob.primitive),
            material: ob.render.then(|| ob.material.as_deref().unwrap_or("")),
            shared_geometry: shared.then_some(true),
            native_render_origin: if ob.render { ob.shared_origin } else { None },
            render: (!ob.render).then_some(false),
            collision_mesh: (!ob.render).then(|| ob.collision_mesh.as_deref().unwrap_or("")),
            transform: ob.transform,
            is_static: true,
            collision_mode: ob.collision_mode,
            collision_material: format!("material_{:04}", ob.collision_packed),
            collision_material_packed: ob.collision_packed,
            world_transform_baked: if ob.grind_source.is_some() {
                Some(true)
            } else {
                shared.then_some(false)
            },
            source_object: ob.grind_source.as_deref(),
            grind_curve: ob.grind_source.is_some().then_some(true),
        }
    }

    fn write_index(&mut self) -> Result<()> {
        let mut out_tex: Vec<&crate::texture::OutTexture> =
            self.ctx.tex.all().iter().filter(|t| !t.removed).collect();
        out_tex.sort_by(|a, b| a.file.cmp(&b.file));
        let index = TexturesIndex {
            textures: out_tex
                .iter()
                .map(|t| TextureEntry {
                    file: format!("textures/{}", t.file),
                    w: t.w,
                    h: t.h,
                    src_w: t.source_w,
                    src_h: t.source_h,
                    src_format: &t.source_format,
                    bytes: t.bytes(),
                    sources: t.sources.iter().map(String::as_str).collect(),
                    src_variants: (t.source_variants.len() > 1)
                        .then(|| t.source_variants.iter().map(String::as_str).collect()),
                })
                .collect(),
        };
        let bytes = crate::json_bytes("textures_index.json", &index, false)?;
        self.r.tex_bytes = out_tex.iter().map(|t| t.bytes()).sum();
        self.r.tex_pixels = out_tex.iter().map(|t| u64::from(t.w) * u64::from(t.h)).sum();
        self.files.insert("textures_index.json".into(), bytes);
        Ok(())
    }

    /// The cull IDs the map takes besides its blend chains: an ID per entity Studio makes (each object once per 200 m
    /// cell it meets: tiles, instances, collision, grind prisms, water) and per light. The writer's own count, made as
    /// [`MapWriter::finish`] makes it, for `summarize-reskate-map`.
    pub(crate) fn non_chain_cull_ids(&mut self, x: Extras) -> Result<CullCount> {
        Ok(self.lay_out(x)?.other)
    }

    /// Collision, water, grind curves, markers, lights, textures_index.json, map.json, trainer.json.
    pub fn finish(&mut self, x: Extras) -> Result<()> {
        // the map's spawn is the program's (place-spawn at selection, or a skate.marker of kind spawn): the writer
        // makes none, and map.json cannot be written without one (a part's count, which takes no marker, needs none)
        if !x.markers.iter().any(|m| m.kind == "spawn") {
            return Err(Error::Scene(
                "the map has no spawn marker: place one before writing (place-spawn at selection, or a skate.marker \
                 of kind spawn)"
                    .into(),
            ));
        }
        let LaidOut {
            o,
            off,
            markers,
            mut col,
            water,
            kept_lights,
            other,
        } = self.lay_out(x)?;
        self.emit_chains(other.total())?;
        // textures
        self.ctx.tex.finish();
        let used = self.used_textures();
        self.r.tex_removed = self.ctx.tex.prune(&used);
        self.write_index()?;
        for m in &mut self.ctx.mats.list {
            m.tris = self.mat_tris.get(&m.id).copied().unwrap_or(0) as u64;
        }

        let used_mats: BTreeSet<String> = self.objects.iter().filter_map(|ob| ob.material.clone()).collect();
        self.r.materials = used_mats.len();
        let mut objs = std::mem::take(&mut self.objects);
        objs.sort_by(|a, b| a.name.cmp(&b.name));
        let mut cobjs = std::mem::take(&mut col.objects);
        cobjs.sort_by(|a, b| a.name.cmp(&b.name));
        let obj_arr: Vec<MapObject> = objs.into_iter().chain(cobjs).chain(water.clone()).collect();
        self.r.object_records = obj_arr.len();
        self.r.lights_written = kept_lights.len();

        let (lights, without_tangent) = lights_json(&kept_lights, off, &o.light_prefix);
        self.r.lights_without_tangent = without_tangent;
        let mut materials: BTreeMap<String, MaterialJson> = self
            .ctx
            .mats
            .list
            .iter()
            .filter(|m| used_mats.contains(&m.name))
            .map(|m| (m.name.clone(), self.material_json(m)))
            .collect();
        if water.is_some() {
            materials.insert(OCEAN.to_owned(), MaterialJson::Ocean(OceanMaterial::OCEAN));
        }
        let trigger_effects = self.effects_json(&markers, "trigger_effect", &col);
        let vfx_prefabs = self.effects_json(&markers, "vfx", &col);
        let mut trainer: Vec<(String, [f32; 3])> = Vec::new();
        let mut travel_points = Vec::new();
        let (mut travel, mut n) = (0usize, 0usize);
        for (i, st) in markers.iter().enumerate() {
            if st.kind != "travel" {
                continue;
            }
            let ti = n;
            n += 1;
            let (mut sx, mut sy, mut gz) = (st.x, st.y, st.z);
            if st.snap.mode != SnapMode::None {
                if !col.ground[i].is_nan() {
                    (sx, sy) = col.placed[i];
                    gz = col.ground[i];
                } else {
                    self.r.warnings.push(format!(
                        "travel point '{}': its snap found nothing",
                        st.name.as_deref().unwrap_or("")
                    ));
                }
            }
            if gz.is_nan() {
                continue;
            }
            let id = if ti == 0 {
                "TravelPoint".to_owned()
            } else {
                format!("TravelPoint.{ti:03}")
            };
            let lift = if st.lift.is_nan() { 0.05 } else { st.lift };
            let mut gy = f64::from(gz) + lift;
            if off.z != 0.0 {
                gy += f64::from(off.z);
            }
            let p = [sx + off.x, gy as f32, -(sy + off.y)];
            travel_points.push(TravelPointJson {
                name: id.clone(),
                source_name: id.clone(),
                source_path: id,
                source_collections: ["Markers"],
                position: p,
                yaw: st.yaw as f32,
                stop_name: st.name.clone().unwrap_or_default(),
                shelter: st.shelter,
            });
            travel += 1;
            if st.trainer {
                trainer.push((st.name.clone().unwrap_or_default(), p));
            }
        }
        self.r.travel_points = travel;
        if travel > 25 {
            self.r
                .warnings
                .push(format!("{travel} travel points: Studio takes at most 25"));
        }
        // spawn: float height + float lift after a snap
        let mut spawns: Vec<(f32, f32, f32, f64, String)> = Vec::new();
        for (i, sp) in markers.iter().enumerate() {
            if sp.kind != "spawn" {
                continue;
            }
            let (mut sx, mut sy) = (sp.x, sp.y);
            let mut sz;
            if sp.snap.mode == SnapMode::Street {
                (sx, sy) = col.placed[i];
            }
            if sp.snap.mode == SnapMode::None && !sp.z.is_nan() {
                sz = if sp.lift.is_nan() {
                    sp.z
                } else {
                    sp.z + sp.lift as f32
                };
            } else {
                if sp.snap.mode == SnapMode::Open && !col.ground[i].is_nan() {
                    (sx, sy) = col.placed[i];
                }
                let lift = if sp.lift.is_nan() { 0.3f32 } else { sp.lift as f32 };
                // a spawn whose snap finds no ground keeps its own height if it has one; without one the skater
                // would start at a guessed height (it used to be 50 m, a fall), so the map is refused
                sz = if col.ground[i].is_nan() {
                    if sp.z.is_nan() {
                        return Err(Error::Scene(format!(
                            "spawn '{}' at ({sx}, {sy}): its snap found no ground under it and it has no height; \
                             place it over collision or give it a height",
                            sp.name.as_deref().unwrap_or("")
                        )));
                    }
                    sp.z
                } else {
                    col.ground[i] + lift
                };
                if col.ground[i].is_nan() && sp.snap.mode != SnapMode::None {
                    self.r
                        .warnings
                        .push("spawn: its snap found nothing; its own height is kept".into());
                }
            }
            if off.z != 0.0 {
                sz += off.z;
            }
            spawns.push((
                sx,
                sy,
                sz,
                sp.yaw,
                sp.name.clone().unwrap_or_else(|| "spawn".into()),
            ));
        }
        let spawn_json = |s: &(f32, f32, f32, f64, String), name: &str| SpawnJson {
            name: name.to_owned(),
            position: [s.0 + off.x, s.2, -(s.1 + off.y)],
            yaw: s.3,
        };
        let map = MapJson {
            format: 1,
            name: &o.name,
            display_name: o.display_name.as_deref().unwrap_or(&o.name),
            units: "meters",
            up: "y",
            forward: "-z",
            generator: &o.generator,
            materials,
            surface_profiles: serde_json::Map::new(),
            streaming_distances: StreamingDistances {
                near: o.streaming_distances[0],
                medium: o.streaming_distances[1],
                far: o.streaming_distances[2],
            },
            objects: obj_arr.iter().map(Self::object_json).collect(),
            lights,
            audio_volumes: [],
            audio_emitters: [],
            interaction_prefabs: [],
            trigger_effects,
            vfx_prefabs,
            npc_routes: [],
            warnings: &o.warnings,
            travel_points,
            spawn: spawn_json(&spawns[0], "spawn"),
            spawn_points: (spawns.len() > 1).then(|| spawns.iter().map(|s| spawn_json(s, &s.4)).collect()),
        };
        let bytes = crate::json_bytes("map.json", &map, true)?;
        self.files.insert("map.json".into(), bytes);

        // vertex_blend.json (only when the map has vertex-blend materials): for the post-compile stage, each such
        // material and the objects (mesh, primitive) drawing it, so it retargets exactly those Studio keys
        let blended: BTreeSet<&str> = self
            .ctx
            .mats
            .list
            .iter()
            .filter(|m| m.vertex_blend.is_some() && used_mats.contains(&m.name))
            .map(|m| m.name.as_str())
            .collect();
        if !blended.is_empty() {
            let mut record: BTreeMap<&str, Vec<(&str, usize)>> =
                blended.iter().map(|&n| (n, Vec::new())).collect();
            for o in &map.objects {
                if let (Some(m), Some(mesh), Some(prim)) = (o.material, o.mesh, o.primitive)
                    && let Some(list) = record.get_mut(m)
                {
                    list.push((mesh, prim));
                }
            }
            let doc = serde_json::json!({ "format": 1, "materials": record });
            let record = crate::records::VERTEX_BLEND_RECORD;
            let bytes = crate::json_bytes(record, &doc, true)?;
            self.files.insert(record.into(), bytes);
        }

        // trainer.json: the trainer spots and the travel points listed too
        for (i, m) in markers.iter().enumerate() {
            if m.kind != "trainer" {
                continue;
            }
            let (mut sx, mut sy, mut gz) = (m.x, m.y, m.z);
            let snapped = m.snap.mode != SnapMode::None && !col.ground[i].is_nan();
            if snapped {
                (sx, sy) = col.placed[i];
                gz = col.ground[i];
            }
            if gz.is_nan() {
                self.r.warnings.push(format!(
                    "trainer spot '{}' has no height and is left out",
                    m.name.as_deref().unwrap_or("")
                ));
                self.r.trainer_left_out += 1;
                continue;
            }
            let lift = if m.lift.is_nan() {
                if snapped { 0.5 } else { 0.0 }
            } else {
                m.lift
            };
            let name = m
                .name
                .clone()
                .unwrap_or_else(|| format!("Spot {}", trainer.len()));
            trainer.push((
                name,
                [
                    sx + off.x,
                    (f64::from(gz) + lift + f64::from(off.z)) as f32,
                    -(sy + off.y),
                ],
            ));
        }
        if trainer.len() > 64 {
            self.r.warnings.push(format!(
                "{} trainer spots: trainer.json takes 64, the rest are left out",
                trainer.len()
            ));
            self.r.trainer_left_out += trainer.len() - 64;
            trainer.truncate(64);
        }
        self.r.trainer_spots = trainer.len();
        if !trainer.is_empty() {
            let t = TrainerJson {
                schema: 1,
                note: o
                    .trainer_note
                    .as_deref()
                    .or(o.display_name.as_deref())
                    .unwrap_or(&o.name),
                spots: trainer
                    .into_iter()
                    .map(|(name, position)| TrainerSpot { name, position })
                    .collect(),
            };
            let bytes = crate::json_bytes("trainer.json", &t, true)?;
            self.files.insert("trainer.json".into(), bytes);
        }
        self.r.mesh_bytes = self.mesh_files.values().map(|b| b.len() as u64).sum();
        self.r.collision = Some(col);
        Ok(())
    }

    /// Everything [`MapWriter::finish`] lays out before it decides the blend chains: the collision, grind prisms and
    /// water as objects, the lights in their total order, and the cull IDs all of it takes.
    fn lay_out(&mut self, mut x: Extras) -> Result<LaidOut> {
        let o = self.o.clone();
        let off = self.off;
        let markers = x.markers.clone();
        if self.r.degenerate > 0 {
            self.r.warnings.push(format!(
                "{} placements with a degenerate matrix were left out",
                self.r.degenerate
            ));
        }
        if let Some(pm) = &mut self.pm {
            pm.sea_tint = !x.water.is_empty();
            for (f, b) in pm.save()? {
                self.files.insert(f, b);
            }
        }
        // the collision: the cells still waiting (every cell, for a map written whole), the hulls not packed yet;
        // then the triangle mesh's and the water's counts as one
        let waiting = self.waiting_cells();
        self.lay_cells(&waiting, None)?;
        // the hull soups one by one, each one's objects counted as its own (another part may write it too), then the
        // placed props' hulls
        let mut shared = Vec::new();
        for h in std::mem::take(&mut x.hulls) {
            let before = self.col.objects.len();
            self.lay_hulls(std::slice::from_ref(&h))?;
            let cells: usize = self.col.objects[before..]
                .iter()
                .map(|ob| ob.cells as usize)
                .sum();
            if cells > 0 {
                shared.push((crate::components::SharedContent::HullSoup, h.entity, cells));
            }
        }
        let prop_hulls = self.take_prop_hulls();
        self.lay_hulls(&prop_hulls)?;
        crate::collision_pack::check_packed(self.collision_plan.as_ref(), self.leaves_packed)?;
        let mut col = std::mem::take(&mut self.col);
        let wc = std::mem::take(&mut self.water_col);
        col.objects.extend(wc.objects);
        col.pieces += wc.pieces;
        col.water_triangles += wc.water_triangles + wc.prop_triangles;
        col.split_cells += wc.split_cells;
        // the writer snaps nothing (snap-reskate-markers does): a marker still asking for a snap is taken as one whose
        // snap found nothing
        col.placed = markers.iter().map(|m| (m.x, m.y)).collect();
        col.ground = vec![f32::NAN; markers.len()];

        // grind curves: the invisible prism prepare-reskate-grinds made for each, world baked, numbered by what it is
        // (its surface, then its geometry), not the order the curves came in; equal prisms write the same either way
        let mut prisms: Vec<(usize, &crate::scene::GrindPrism, primase_core::Entity)> = x
            .grind
            .iter()
            .filter_map(|g| g.prism.as_ref().map(|p| (g.surface, p, g.entity)))
            .collect();
        prisms.sort_by(|(sa, a, _), (sb, b, _)| {
            let bits = |p: &crate::scene::GrindPrism| {
                p.positions
                    .iter()
                    .flat_map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
                    .collect::<Vec<u32>>()
            };
            sa.cmp(sb)
                .then_with(|| bits(a).cmp(&bits(b)))
                .then_with(|| a.indices.cmp(&b.indices))
        });
        for (i, (surface, p, entity)) in prisms.into_iter().enumerate() {
            let (gpos, gidx, pieces, length) = (&p.positions, p.indices.clone(), p.pieces, p.length);
            if gidx.is_empty() {
                self.r
                    .warnings
                    .push(format!("grind curve {i} has no usable points"));
                self.r.grind_unusable += 1;
                continue;
            }
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for q in gpos {
                lo = lo.min(*q);
                hi = hi.max(*q);
            }
            if gidx.len() as f32 / (hi - lo).length().max(1e-6) > 900.0 {
                self.r.warnings.push(format!(
                    "grind curve {i} is too detailed for its size to grind; use fewer points"
                ));
            }
            let flat: Vec<f32> = gpos
                .iter()
                .flat_map(|q| [q.x + off.x, q.y + off.y, q.z + off.z])
                .collect();
            let name = format!("grind_{i:05}");
            if self.layout.encodes() {
                self.mesh_files.insert(
                    format!("{name}.glb"),
                    glb::write(
                        &name,
                        &[GlbPrim {
                            blend: None,
                            material: None,
                            pos: flat,
                            nrm: None,
                            uv: None,
                            idx: gidx,
                        }],
                    )?,
                );
            }
            let mut ob = MapObject::new(name.clone(), MapObject::translation(0.0, 0.0, 0.0));
            ob.collision_mesh = Some(format!("meshes/{name}.glb"));
            ob.render = false;
            ob.collision_mode = "triangle_mesh";
            ob.collision_packed = grind::SURFACES[surface].1;
            ob.grind_source = Some(name);
            shared.push((
                crate::components::SharedContent::GrindPrism,
                entity,
                ob.cells as usize,
            ));
            col.objects.push(ob);
            self.r.grind_curves += 1;
            self.r.grind_pieces += i64::from(pieces);
            self.r.grind_length += length;
        }

        let water = if !x.water.is_empty() {
            self.water(&x.water, OCEAN)?
        } else {
            None
        };

        // the chain overlays, within what the rest of the level leaves of the cull-ID pool: an ID per entity Studio
        // makes (each object once per 200 m cell it meets) and per light
        // every light the world holds (a cap is a pass: select-reskate-lights), in a total order
        let mut kept_lights = x.lights.clone();
        kept_lights.sort_by(crate::scene::light_order);
        shared.extend(
            kept_lights
                .iter()
                .map(|l| (crate::components::SharedContent::Light, l.entity, 1)),
        );
        let all = self
            .objects
            .iter()
            .chain(&col.objects)
            .map(|ob| ob.cells as usize)
            .sum::<usize>();
        let other = CullCount {
            // the objects, less the hull soups' and grind prisms' (counted by entity)
            summed: all
                - shared
                    .iter()
                    .filter(|(kind, _, _)| *kind != crate::components::SharedContent::Light)
                    .map(|(_, _, n)| n)
                    .sum::<usize>(),
            shared,
            water: water.is_some(),
        };
        debug_assert_eq!(
            other.total(),
            all + water.as_ref().map_or(0, |ob| ob.cells as usize) + kept_lights.len()
        );
        Ok(LaidOut {
            o,
            off,
            markers,
            col,
            water,
            kept_lights,
            other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(dir: Vec3, tan: Vec3) -> PlacedLight {
        PlacedLight {
            entity: primase_core::Entity::from_index(0),
            spot: true,
            pos: Vec3::ZERO,
            dir,
            tan,
            r: 1.0,
            g: 1.0,
            b: 1.0,
            energy: 1.0,
            range: 10.0,
            radius: 0.1,
            spot_size: 1.0,
            spot_blend: 0.1,
            time_of_day: 0,
            cast_shadow: false,
            priority: 0.0,
            group: String::new(),
            source: String::new(),
        }
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f64 {
        (0..3).map(|i| f64::from(a[i]) * f64::from(b[i])).sum()
    }

    fn acc(seq: usize) -> Acc {
        Acc {
            seq,
            ..Acc::default()
        }
    }

    /// Draw order: by class, then under before over by the overlay relations, then by first use; material ids
    /// (and so names) chosen against first use, so neither can be what orders them.
    #[test]
    fn draw_order_is_class_then_overlay_then_first_use() {
        let parts: BTreeMap<usize, Acc> = [
            (9, acc(0)),
            (3, acc(1)),
            (7, acc(2)),
            (1, acc(3)),
            (5, acc(4)),
            (8, acc(5)),
        ]
        .into();
        // 9, 3: opaque surfaces; 7: an alpha-tested surface; 1, 5, 8: blended decals
        let class = |m: usize| match m {
            9 | 3 => (false, 0),
            7 => (false, 1),
            _ => (true, 1),
        };
        // without relations: first use within each class
        assert_eq!(draw_order(&parts, class, &[]), [9, 3, 7, 1, 5, 8]);
        // 8 under 5 under 1; a relation across classes does not move a class
        assert_eq!(
            draw_order(&parts, class, &[(5, 1), (8, 5), (1, 9)]),
            [9, 3, 7, 8, 5, 1]
        );
        // a cycle is broken at its first used part, after the parts that are free
        assert_eq!(draw_order(&parts, class, &[(1, 5), (5, 1)]), [9, 3, 7, 8, 1, 5]);
    }

    /// A light without a usable tangent gets a basis square to it: from X, or from Z for a light along X.
    #[test]
    fn a_tangentless_light_gets_an_orthogonal_basis() {
        let tilt = 0.7f32.to_radians();
        let lights = [
            // 0.7 degrees off straight down, no tangent
            light(Vec3::new(tilt.sin(), 0.0, -tilt.cos()), Vec3::ZERO),
            // along X, a tangent parallel to it
            light(Vec3::UNIT_X, Vec3::UNIT_X),
            // straight down: X is already square to it
            light(Vec3::new(0.0, 0.0, -1.0), Vec3::ZERO),
            // a usable tangent is kept
            light(Vec3::new(0.0, 0.0, -1.0), Vec3::UNIT_Y),
        ];
        let (json, fallback) = lights_json(&lights, Vec3::ZERO, "l");
        assert_eq!(fallback, 3);
        for (l, j) in lights.iter().zip(&json) {
            let b = &j.basis;
            for (u, v) in [(b.right, b.up), (b.right, b.forward), (b.up, b.forward)] {
                assert!(dot(u, v).abs() < 1e-6, "{:?}: not orthogonal {u:?} {v:?}", l.dir);
            }
            for u in [b.right, b.up, b.forward] {
                assert!((dot(u, u) - 1.0).abs() < 1e-6, "{:?}: not unit {u:?}", l.dir);
            }
        }
        // Studio's frame (x, z, -y): the fallback axes and the kept tangent
        assert!(json[0].basis.right[0] > 0.99, "from X: {:?}", json[0].basis.right);
        assert_eq!(json[1].basis.right, [0.0, 1.0, 0.0], "from Z (up)");
        assert_eq!(json[2].basis.right, [1.0, 0.0, 0.0], "X unchanged");
        assert_eq!(json[3].basis.right, [0.0, 0.0, -1.0], "the tangent");
    }
}
