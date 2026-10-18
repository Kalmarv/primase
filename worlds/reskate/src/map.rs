//! Placements of prototypes -> ReSkate Studio's normalized map folder: static geometry merged into one mesh per map
//! tile (one primitive per material), repeated prototypes kept as one mesh placed many times; then collision,
//! water, grind prisms, markers, lights, textures, map.json and trainer.json. The reference writer's MapWriter.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::blend_order::{self, Geometry};
use crate::collision::{self, Collision, CollisionWorld, NoWorld, PropSoup, Query};
use crate::glb::{self, GlbPrim};
use crate::grind;
use crate::material::{MatLib, Material, VClass};
use crate::model::{CollisionMode, Hull, MapObject, Placement, ProtoInfo, SrcMesh, SrcPart, TileOut};
use crate::overlay::{self, OverlayStats, Parts, Piece};
use crate::pausemap::PauseMap;
use crate::records::{
    Basis, EffectJson, LightJson, MapJson, MaterialJson, ObjectJson, OceanMaterial, PieceJson, PiecesJson,
    SpawnJson, StreamingDistances, SurfaceMaterial, TextureEntry, TexturesIndex, TrainerJson, TrainerSpot,
    TravelPointJson,
};
use crate::scene::{GrindCurve, Marker, PlacedLight, SnapMode, SurfaceTable, WaterQuad};
use crate::texture::TexLib;
use crate::{Error, Result};
use primase_meshops::vector::Vec3;

/// What the writer is told (defaults are gta-skate's).
#[derive(Clone, Debug)]
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
    pub tex_cap: u32,
    pub tex_caps: Option<String>,
    pub overlay_step: f32,
    /// The most objects the writer adds to draw blend-chain overlays in chain order (one per tile, dominant axis and
    /// chain rank: [`crate::blend_order`]); past it a tile's chain overlays stay in its mesh, in an unstable order.
    /// 0: none added.
    pub blend_objects: usize,
    pub piece_log: bool,
    pub offset: [f64; 3],
    pub limit: f32,
    pub map_height: usize,
    pub water: bool,
    pub water_ocean: bool,
    pub lights_max_per_cell: usize,
    pub light_prefix: String,
    pub streaming_distances: [f64; 3],
    pub max_render_edge: f32,
    pub tex_budget: i32,
    pub tex_aux_weight: f32,
    pub trainer_note: Option<String>,
    pub warnings: Vec<String>,
    pub pause_picture: Option<(usize, usize, Vec<u8>, [f64; 4])>,
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
    pub peak_mesh_bytes: i64,
    pub far_dropped: i64,
    pub clipped: i64,
    pub degenerate: i64,
    pub tex_removed: u64,
    pub tex_bytes: u64,
    pub tex_pixels: u64,
    pub mesh_bytes: u64,
    pub overlay: OverlayStats,
    /// The blend-chain overlays as objects of their own ([`crate::blend_order`]).
    pub blend: blend_order::Stats,
    pub prop_col_tris: BTreeMap<String, i64>,
    pub collision: Option<collision::CollisionResult>,
    pub lights_written: usize,
    pub lights_dropped: usize,
    /// Lights whose basis came from the fallback axis (no usable tangent).
    pub lights_without_tangent: usize,
    /// The overlay stack's lift record: per mesh file (`meshes/...`), per primitive in file order, each vertex's lift
    /// along its derived normal in metres (an empty list: nothing lifted). Meshes without a lifted vertex are left out.
    pub lifts: BTreeMap<String, Vec<Vec<f32>>>,
    pub grind_curves: i64,
    pub grind_pieces: i64,
    pub grind_length: f32,
    pub trainer_spots: usize,
    pub travel_points: usize,
    pub budget_flattened: i64,
    pub warnings: Vec<String>,
}

/// A tile's blend-chain overlays as objects of their own ([`MapWriter::chain_layers`]): the parts of each (axis,
/// rank), and each axis's lift step.
#[derive(Debug, Default)]
struct ChainLayers {
    groups: BTreeMap<(u8, u8), BTreeMap<usize, Acc>>,
    steps: BTreeMap<u8, f32>,
}

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
    src: Vec<PieceSrc>,
}

/// One append to a merged part: its triangles and vertices, draw order and entity.
#[derive(Clone, Copy, Debug)]
struct AccPiece {
    t0: usize,
    t1: usize,
    v0: usize,
    v1: usize,
    order: i32,
    entity: i32,
}

/// Where a piece came from, for pieces.json.
#[derive(Clone, Debug)]
struct PieceSrc {
    drawable: String,
    entity: Vec3,
    shader: String,
    bucket: u8,
    model: i32,
    geom: i32,
}

/// The props' collision of each collision tile, with the map tile that placed them.
type PropParts = BTreeMap<(i32, i32), Vec<(i32, i32, PropSoup)>>;

/// One overlay piece for pieces.json.
#[derive(Clone, Debug)]
struct PieceRec {
    mesh: String,
    material: String,
    pass: i32,
    order: i32,
    shift_mm: f32,
    tris: usize,
    centre: Vec3,
    drawable: String,
    entity: Vec3,
    shader: String,
    bucket: u8,
    model: i32,
    geom: i32,
    entity_no: i32,
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    degenerate: bool,
    longest: f32,
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
pub const MAX_SECTION_VERTS: usize = 60000;

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

fn area(pos: &[f32], idx: &[u32]) -> f64 {
    let mut s = 0f64;
    for t in idx.as_chunks::<3>().0 {
        let a = Vec3::at(pos, t[0] as usize * 3);
        let b = Vec3::at(pos, t[1] as usize * 3);
        let c = Vec3::at(pos, t[2] as usize * 3);
        s += f64::from((b - a).cross(c - a).length()) * 0.5;
    }
    s
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
    off: Vec3,
    pm: PauseMap,
    objects: Vec<MapObject>,
    mat_tris: HashMap<usize, i64>,
    tile_area: BTreeMap<(i32, i32), HashMap<usize, f64>>,
    mat_used: BTreeSet<usize>,
    prop_parts: PropParts,
    water_parts: PropParts,
    prop_hulls: Vec<Hull>,
    piece_log: Option<Vec<PieceRec>>,
    /// Mesh files (meshes/<name>), written at the end.
    pub mesh_files: BTreeMap<String, Vec<u8>>,
    /// Other files (relative to the map folder).
    pub files: BTreeMap<String, Vec<u8>>,
}

/// A proto made once per (key, variant row).
struct Proto {
    info: usize,
    materials: Vec<String>,
    origins: Vec<Option<[f32; 3]>>,
    area: Option<HashMap<usize, f64>>,
}

const APRON: f32 = 30.0;

impl MapWriter {
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "exception: reads a texture caps file; the caller should pass the caps"
    )]
    pub fn new(o: WriterOptions, surfaces: SurfaceTable, tex_dir: &std::path::Path) -> Result<MapWriter> {
        let off = Vec3::new(o.offset[0] as f32, o.offset[1] as f32, o.offset[2] as f32);
        let pm = match &o.pause_picture {
            Some((w, h, rgba, b)) => PauseMap::given(*w, *h, rgba, *b, off),
            None => PauseMap::new(o.region, off, o.map_height),
        };
        let mut tex = TexLib::new(tex_dir, o.tex_cap);
        if let Some(caps) = &o.tex_caps {
            let text = std::fs::read(caps).map_err(|source| crate::Error::Io {
                path: caps.into(),
                source,
            })?;
            let v: serde_json::Value =
                serde_json::from_slice(&text).map_err(|e| crate::Error::Scene(format!("{caps}: {e}")))?;
            if let Some(m) = v.get("caps").and_then(|c| c.as_object()) {
                for (k, c) in m {
                    tex.cap_by_name
                        .insert(k.to_lowercase(), c.as_i64().unwrap_or(0) as u32);
                }
            }
        }
        Ok(MapWriter {
            piece_log: o.piece_log.then(Vec::new),
            o,
            ctx: Ctx {
                tex,
                mats: MatLib::default(),
            },
            surfaces,
            r: WriteResult::default(),
            off,
            pm,
            objects: Vec::new(),
            mat_tris: HashMap::new(),
            tile_area: BTreeMap::new(),
            mat_used: BTreeSet::new(),
            prop_parts: BTreeMap::new(),
            water_parts: BTreeMap::new(),
            prop_hulls: Vec::new(),
            mesh_files: BTreeMap::new(),
            files: BTreeMap::new(),
        })
    }

    /// Take a tile's blend-chain overlays out of its parts as objects of their own ([`blend_order`]): per (dominant
    /// axis, chain rank), each rank of an axis padded to the box of all of them, with each axis's lift step for the
    /// tile's translation `at`. Leaves them in the tile (counted) when the objects would pass the cap or an axis has
    /// no lift step there.
    fn chain_layers(&mut self, parts: &mut BTreeMap<usize, Acc>, at: Vec3) -> Result<ChainLayers> {
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
            return Ok(ChainLayers::default());
        }
        let mut groups: BTreeMap<(u8, u8), BTreeMap<usize, Geometry>> = BTreeMap::new();
        let mut tris = 0;
        for &(k, rank) in &ranks {
            let a = &parts[&k];
            // chain overlays are fixed: the overlay stack never lifts them
            debug_assert!(a.lift.iter().all(|&x| x == 0.0));
            tris += a.idx.len() / 3;
            let g = Geometry {
                pos: a.pos.clone(),
                nrm: a.nrm.clone(),
                uv: a.uv.clone(),
                blend: a.blend.clone(),
                idx: a.idx.clone(),
            };
            for (axis, h) in blend_order::split_by_axis(&g) {
                groups.entry((axis, rank)).or_default().insert(k, h);
            }
        }
        let st = &mut self.r.blend;
        st.cap = self.o.blend_objects;
        if st.objects + groups.len() > self.o.blend_objects {
            st.tiles_shared += 1;
            st.overlay_triangles_shared += tris;
            return Ok(ChainLayers::default());
        }
        let axes: BTreeSet<u8> = groups.keys().map(|&(axis, _)| axis).collect();
        let mut steps = BTreeMap::new();
        for &axis in &axes {
            let Some(step) = blend_order::lift_step(at, axis) else {
                st.tiles_unliftable += 1;
                st.overlay_triangles_unliftable += tris;
                return Ok(ChainLayers::default());
            };
            steps.insert(axis, step);
        }
        for &step in steps.values() {
            st.steps_widened += usize::from(step > blend_order::LIFT_STEP);
            st.max_step_m = st.max_step_m.max(step);
        }
        st.objects += groups.len();
        st.tiles_split += 1;
        st.overlay_triangles_split += tris;
        for axis in axes {
            let span = (axis, 0)..=(axis, u8::MAX);
            let Some((lo, hi)) =
                blend_order::bounds(groups.range(span.clone()).flat_map(|(_, g)| g.values()))
            else {
                continue;
            };
            for (_, g) in groups.range_mut(span) {
                // into the rank's first part in draw order (first use)
                let first = *g
                    .keys()
                    .min_by_key(|&k| parts[k].seq)
                    .expect("a group has a part");
                blend_order::pad(g.get_mut(&first).expect("its part"), lo, hi, axis);
            }
        }
        let groups = groups
            .into_iter()
            .map(|(key, g)| {
                let accs = g
                    .into_iter()
                    .map(|(k, h)| {
                        let acc = Acc {
                            seq: parts[&k].seq,
                            pos: h.pos,
                            nrm: h.nrm,
                            uv: h.uv,
                            blend: h.blend,
                            idx: h.idx,
                            ..Acc::default()
                        };
                        (k, acc)
                    })
                    .collect();
                (key, accs)
            })
            .collect();
        for (k, _) in ranks {
            parts.remove(&k);
        }
        Ok(ChainLayers { groups, steps })
    }

    /// Write a tile's [`Self::chain_layers`]: one GLB and object per (axis, rank), named `<tile>_v<axis><rank>`,
    /// placed at the tile's translation `at` lifted along the axis by the rank's lift.
    fn emit_chain_layers(&mut self, layers: &ChainLayers, tile: &str, at: Vec3) -> Result<()> {
        for (&(axis, rank), group) in &layers.groups {
            let step = layers.steps[&axis];
            let mats = &self.ctx.mats;
            let order = draw_order(group, |k| draw_class(mats.by_id(k)), &[]);
            let (pr, lifts) = prims(group, mats, &order);
            let p = at + blend_order::axis_vector(axis) * blend_order::lift(rank, step);
            let rows = MapObject::translation(p.x, p.y, p.z);
            let stem = format!("{tile}_v{axis}{rank}");
            let file = format!("{stem}.glb");
            record_lifts(&mut self.r, format!("meshes/{file}"), lifts);
            self.mesh_files.insert(file.clone(), glb::write(&stem, &pr)?);
            for (i, q) in pr.iter().enumerate() {
                let name = if pr.len() == 1 {
                    stem.clone()
                } else {
                    format!("{stem} / section {}", i + 1)
                };
                let mut ob = MapObject::new(name, rows);
                ob.mesh = Some(format!("meshes/{file}"));
                ob.primitive = i;
                ob.material = q.material.clone();
                self.objects.push(ob);
            }
        }
        Ok(())
    }

    /// OverlayStack over a set of merged parts; logs the overlay pieces with --piece-log.
    /// Stack the mesh's overlay pieces; returns which material lies under which, `(below, above)`.
    fn stack_overlays(
        &mut self,
        parts: &mut BTreeMap<usize, Acc>,
        origin: Vec3,
        mesh: &str,
    ) -> Vec<(usize, usize)> {
        let step = self.o.overlay_step;
        if step <= 0.0 && self.piece_log.is_none() {
            return Vec::new();
        }
        let mut pieces: Vec<Piece> = Vec::new();
        let mut src: Vec<(usize, usize)> = Vec::new();
        let keys: Vec<usize> = parts.keys().copied().collect();
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
            }
        }
        let mut under: Vec<(usize, usize)> = Vec::new();
        if step > 0.0 {
            let mut accs: Vec<&mut Acc> = parts.values_mut().collect();
            let idx: Vec<Vec<u32>> = accs.iter().map(|a| a.idx.clone()).collect();
            let mut view = Parts {
                pos: accs.iter_mut().map(|a| &mut a.pos).collect(),
                idx: idx.iter().collect(),
            };
            let stacked = overlay::apply(step, &mut view, &mut pieces);
            self.r.overlay.add(&stacked.stats);
            under = stacked
                .under
                .iter()
                .map(|&(lo, hi)| (src[lo].0, src[hi].0))
                .filter(|(lo, hi)| lo != hi)
                .collect();
            under.sort_unstable();
            under.dedup();
        }
        for (k, p) in pieces.iter().enumerate() {
            if p.shift > 0.0 {
                let a = parts.get_mut(&src[k].0).expect("a piece's part");
                a.lift.resize(a.pos.len() / 3, 0.0);
                for l in &mut a.lift[p.v0..p.v1] {
                    *l = l.max(p.shift);
                }
            }
        }
        let Some(log) = &mut self.piece_log else {
            return under;
        };
        for (k, p) in pieces.iter().enumerate() {
            if !p.overlay {
                continue;
            }
            let (mat, i) = src[k];
            let a = &parts[&mat];
            let mut c = Vec3::ZERO;
            for v in p.v0..p.v1 {
                c += Vec3::new(a.pos[v * 3], a.pos[v * 3 + 1], a.pos[v * 3 + 2]);
            }
            c = c / (p.v1 - p.v0).max(1) as f32 + origin;
            let PieceSrc {
                drawable,
                entity,
                shader,
                bucket,
                model,
                geom,
            } = a.src[i].clone();
            log.push(PieceRec {
                mesh: mesh.to_owned(),
                material: self.ctx.mats.by_id(mat).name.clone(),
                pass: p.pass,
                order: p.order,
                shift_mm: p.shift * 1000.0,
                tris: p.t1 - p.t0,
                centre: c,
                drawable,
                entity,
                shader,
                bucket,
                model,
                geom,
                entity_no: p.entity,
            });
        }
        under
    }

    #[allow(clippy::too_many_arguments)]
    fn append(
        a: &mut Acc,
        p: &SrcPart,
        s: Option<&Placement>,
        origin: Vec3,
        order: i32,
        only: Option<&[u32]>,
        drawable: &str,
        entity: i32,
    ) {
        let (t0, v0) = (a.idx.len() / 3, a.pos.len() / 3);
        append_core(a, p, s, origin, only);
        if a.idx.len() / 3 > t0 {
            a.pieces.push(AccPiece {
                t0,
                t1: a.idx.len() / 3,
                v0,
                v1: a.pos.len() / 3,
                order,
                entity,
            });
            a.src.push(PieceSrc {
                drawable: drawable.to_owned(),
                entity: s.map_or(Vec3::ZERO, |x| x.pos),
                shader: p.shader.clone(),
                bucket: p.bucket,
                model: p.model,
                geom: p.geom,
            });
        }
    }

    fn map_colour(&self, cache: &mut HashMap<usize, u32>, mat: usize) -> u32 {
        *cache.entry(mat).or_insert_with(|| {
            let m = self.ctx.mats.by_id(mat);
            if matches!(m.class, VClass::Decal | VClass::Glass | VClass::Alpha) {
                return 0;
            }
            let c = self.ctx.tex.mean_of(m.tex);
            if c == 0 { 0x010101 } else { c }
        })
    }

    fn make_proto(&mut self, m: &SrcMesh, row: u32, source: &mut dyn MeshSource) -> Result<Proto> {
        let mut parts: BTreeMap<usize, Acc> = BTreeMap::new();
        for (order, p) in m.parts.iter().enumerate() {
            let mat = source.material_of(p, row, &mut self.ctx)?;
            Self::append(
                part(&mut parts, mat),
                p,
                None,
                Vec3::ZERO,
                order as i32,
                None,
                &m.name,
                0,
            );
        }
        let under = self.stack_overlays(&mut parts, Vec3::ZERO, &format!("proto {}", m.name));
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
        let area =
            (self.o.tex_budget > 0).then(|| parts.iter().map(|(k, a)| (*k, area(&a.pos, &a.idx))).collect());
        let stem = format!("p_{:08x}_{row}", m.hash);
        self.r.proto_list[info].file = format!("{stem}.glb");
        let mats = &self.ctx.mats;
        let order = draw_order(&parts, |k| draw_class(mats.by_id(k)), &under);
        let (pr, lifts) = prims(&parts, mats, &order);
        record_lifts(&mut self.r, format!("meshes/{stem}.glb"), lifts);
        self.mesh_files
            .insert(format!("{stem}.glb"), glb::write(&stem, &pr)?);
        let origins = pr
            .iter()
            .map(|p| {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for v in (0..p.pos.len().saturating_sub(2)).step_by(3) {
                    let q = Vec3::new(p.pos[v], p.pos[v + 1], p.pos[v + 2]);
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
                let half = (hi - lo) * 0.5;
                let c = (hi + lo) * 0.5;
                (!p.pos.is_empty() && half.x.max(half.y.max(half.z)) <= 32.0).then_some([c.x, c.z, -c.y])
            })
            .collect();
        Ok(Proto {
            info,
            materials: pr
                .iter()
                .map(|p| p.material.clone().unwrap_or_default())
                .collect(),
            origins,
            area,
        })
    }

    /// Writes the tile meshes and prototypes of every placement, band by band.
    pub fn tiles(&mut self, sels: &[Placement], source: &mut dyn MeshSource) -> Result<()> {
        let o = self.o.clone();
        let r = o.region;
        let off = self.off;
        let mut arch_count: HashMap<u32, i64> = HashMap::new();
        for s in sels {
            *arch_count.entry(s.proto).or_default() += 1;
        }
        self.r.archetypes = arch_count.len();
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
        let mut protos: HashMap<(u32, u32), Proto> = HashMap::new();
        let mut tiles: Vec<TileOut> = Vec::new();
        let edge = [
            r[0] as f32 - APRON,
            r[1] as f32 - APRON,
            r[2] as f32 + APRON,
            r[3] as f32 + APRON,
        ];
        let budget = o.tex_budget > 0;
        let mut colours: HashMap<usize, u32> = HashMap::new();
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
                let mut inst_area: Option<HashMap<usize, f64>> = budget.then(HashMap::new);
                let cmp = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
                g.sort_by(|a, b| {
                    a.proto
                        .cmp(&b.proto)
                        .then(cmp(a.pos.x, b.pos.x))
                        .then(cmp(a.pos.y, b.pos.y))
                        .then(cmp(a.pos.z, b.pos.z))
                });
                for s in g {
                    let Some(m) = meshes.get(&s.proto).filter(|m| m.tris != 0) else {
                        self.r.empty_mesh += 1;
                        continue;
                    };
                    let sh = shape_of(s);
                    if sh.degenerate {
                        self.r.degenerate += 1;
                        continue;
                    }
                    let mut inst = arch_count[&m.hash] >= o.min_instances && m.tris >= o.min_instance_tris;
                    if inst
                        && (s.no_instance
                            || s.part_off.is_some()
                            || (o.max_render_edge > 0.0 && sh.longest > 1.0001))
                    {
                        inst = false;
                    }
                    let mut kept_parts: Option<Vec<Option<Vec<u32>>>> = None;
                    if s.outside {
                        let mut reaches = false;
                        let k: Vec<Option<Vec<u32>>> = m
                            .parts
                            .iter()
                            .map(|p| kept(p, s, edge, r, &mut reaches))
                            .collect();
                        if !reaches {
                            self.r.far_dropped += 1;
                            continue;
                        }
                        let half = (m.max - m.min) * 0.5;
                        if inst && half.x.max(half.y.max(half.z)) > 32.0 {
                            inst = false;
                        }
                        if !inst {
                            kept_parts = Some(k);
                        }
                    }
                    to.entities += 1;
                    let row = if m.has_variants { s.variant } else { 0 };
                    if ((s.pos.x + off.x).abs() + s.radius > o.limit
                        || (s.pos.y + off.y).abs() + s.radius > o.limit)
                        && inst
                    {
                        self.r.clipped += 1;
                        continue;
                    }
                    if !s.no_collision
                        && let (Some(cp), Some(ci)) = (&m.col_pos, &m.col_idx)
                    {
                        if m.col_mode == CollisionMode::Hull {
                            let name = format!("h_{tx}_{ty}_{hull_no}_{}", m.name);
                            hull_no += 1;
                            let mut w = vec![0f32; cp.len()];
                            for v in (0..w.len().saturating_sub(2)).step_by(3) {
                                let (x, y, z) = (cp[v], cp[v + 1], cp[v + 2]);
                                w[v] = s.pos.x + s.ex.x * x + s.ey.x * y + s.ez.x * z;
                                w[v + 1] = s.pos.y + s.ex.y * x + s.ey.y * y + s.ez.y * z;
                                w[v + 2] = s.pos.z + s.ex.z * x + s.ey.z * y + s.ez.z * z;
                            }
                            let ii = ci.clone();
                            let surf = m.col_surf.first().map_or(0, |b| (b & 0x7F) as usize);
                            self.prop_hulls.push(Hull {
                                name,
                                pos: w,
                                idx: ii,
                                surface: surf,
                                ground: m.col_ground,
                            });
                        } else {
                            let target = if m.col_mode == CollisionMode::Water {
                                &mut water_soups
                            } else {
                                &mut soups
                            };
                            let n = collision::add_prop(
                                target,
                                &mut soup_order,
                                cp,
                                ci,
                                &m.col_surf,
                                m.col_ground,
                                s.pos,
                                s.ex,
                                s.ey,
                                s.ez,
                                kept_parts.as_ref().map(|_| edge),
                            );
                            if n > 0 {
                                *self.r.prop_col_tris.entry(m.name.clone()).or_default() += n;
                            }
                        }
                    }
                    let m = m.clone();
                    if inst {
                        if let std::collections::hash_map::Entry::Vacant(e) = protos.entry((m.hash, row)) {
                            let p = self.make_proto(&m, row, source)?;
                            e.insert(p);
                        }
                        let pr = &protos[&(m.hash, row)];
                        self.r.proto_list[pr.info].count += 1;
                        let iname =
                            format!("i_{tx}_{ty}_{}_{}", to.instances, self.r.proto_list[pr.info].name);
                        let rows = MapObject::rows(s.ex, s.ey, s.ez, s.pos + off);
                        if o.pause_picture.is_none() {
                            for p in &m.parts {
                                let mat = source.material_of(p, row, &mut self.ctx)?;
                                let pc = self.map_colour(&mut colours, mat);
                                if pc != 0 {
                                    self.pm.draw(&p.pos, &p.idx, s.pos + off, s.ex, s.ey, s.ez, pc);
                                }
                            }
                        }
                        let pr = &protos[&(m.hash, row)];
                        if let (Some(pa), Some(ia)) = (&pr.area, &mut inst_area) {
                            let lx = s.ex.length();
                            if (s.ey.length() - lx).abs() <= 1e-4 * lx
                                && (s.ez.length() - lx).abs() <= 1e-4 * lx
                            {
                                for (k, v) in pa {
                                    *ia.entry(*k).or_default() += v * f64::from(lx) * f64::from(lx);
                                }
                            } else {
                                for p in &m.parts {
                                    let mat = source.material_of(p, row, &mut self.ctx)?;
                                    let vv = |i: u32| {
                                        s.ex * p.pos[i as usize * 3]
                                            + s.ey * p.pos[i as usize * 3 + 1]
                                            + s.ez * p.pos[i as usize * 3 + 2]
                                    };
                                    let mut ar = 0f64;
                                    for t in p.idx.as_chunks::<3>().0 {
                                        let a0 = vv(t[0]);
                                        ar += f64::from((vv(t[1]) - a0).cross(vv(t[2]) - a0).length()) * 0.5;
                                    }
                                    *ia.entry(mat).or_default() += ar;
                                }
                            }
                        }
                        let pr = &protos[&(m.hash, row)];
                        let file = self.r.proto_list[pr.info].file.clone();
                        for (i, mat) in pr.materials.iter().enumerate() {
                            let name = if pr.materials.len() == 1 {
                                iname.clone()
                            } else {
                                format!("{iname} / section {}", i + 1)
                            };
                            let mut ob = MapObject::new(name, rows);
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
                            p,
                            Some(s),
                            origin,
                            piece_order,
                            only,
                            &m.name,
                            entity_no,
                        );
                        piece_order += 1;
                    }
                }
                let under = self.stack_overlays(&mut parts, origin, &format!("t_{tx}_{ty}"));
                if !parts.is_empty() {
                    to.mesh = Some(format!("t_{tx}_{ty}.glb"));
                    if o.pause_picture.is_none() {
                        let keys: Vec<usize> = parts.keys().copied().collect();
                        for k in keys {
                            let pc = self.map_colour(&mut colours, k);
                            if pc != 0 {
                                let a = &parts[&k];
                                self.pm.draw(
                                    &a.pos,
                                    &a.idx,
                                    Vec3::new(origin.x + off.x, origin.y + off.y, off.z),
                                    Vec3::UNIT_X,
                                    Vec3::UNIT_Y,
                                    Vec3::UNIT_Z,
                                    pc,
                                );
                            }
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
                                self.r.clipped += ((a.idx.len() - keep_idx.len()) / 3) as i64;
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
                    let layers = self.chain_layers(&mut parts, at)?;
                    let mats = &self.ctx.mats;
                    let (pr, lifts) = prims(&parts, mats, &order);
                    let rows = MapObject::translation(at.x, at.y, at.z);
                    let file = to.mesh.clone().expect("mesh");
                    record_lifts(&mut self.r, format!("meshes/{file}"), lifts);
                    self.mesh_files
                        .insert(file.clone(), glb::write(&format!("t_{tx}_{ty}"), &pr)?);
                    for (i, p) in pr.iter().enumerate() {
                        let name = if pr.len() == 1 {
                            format!("t_{tx}_{ty}")
                        } else {
                            format!("t_{tx}_{ty} / section {}", i + 1)
                        };
                        let mut ob = MapObject::new(name, rows);
                        ob.mesh = Some(format!("meshes/{file}"));
                        ob.primitive = i;
                        ob.material = p.material.clone();
                        self.objects.push(ob);
                    }
                    to.parts = pr.len();
                    self.emit_chain_layers(&layers, &format!("t_{tx}_{ty}"), at)?;
                    for (k, a) in parts.iter().chain(layers.groups.values().flatten()) {
                        to.tris += (a.idx.len() / 3) as i64;
                        to.verts += (a.pos.len() / 3) as i64;
                        *self.mat_tris.entry(*k).or_default() += (a.idx.len() / 3) as i64;
                        self.mat_used.insert(*k);
                        if let Some(ia) = &mut inst_area {
                            *ia.entry(*k).or_default() += area(&a.pos, &a.idx);
                        }
                    }
                    self.r.merged_tris += to.tris;
                }
                if let Some(ia) = inst_area {
                    self.tile_area.insert((tx, ty), ia);
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
        tiles.sort_by(|a, b| a.y.cmp(&b.y).then(a.x.cmp(&b.x)));
        self.r.tiles = tiles;
        self.r.prototype_tris = self.r.proto_list.iter().map(|p| p.tris).sum();
        if self.r.degenerate > 0 {
            self.r.warnings.push(format!(
                "{} placements with a degenerate matrix were left out",
                self.r.degenerate
            ));
        }
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

/// Joins the props of each collision tile in map-tile order (row, then column).
fn join(parts: &PropParts) -> BTreeMap<(i32, i32), PropSoup> {
    parts
        .iter()
        .map(|(k, bag)| {
            let mut b: Vec<&(i32, i32, PropSoup)> = bag.iter().collect();
            b.sort_by(|x, y| x.1.cmp(&y.1).then(x.0.cmp(&y.0)));
            let mut soup = PropSoup::default();
            for p in b {
                soup.append(&p.2);
            }
            (*k, soup)
        })
        .collect()
}

/// What `finish` places besides the tiles.
#[derive(Debug)]
pub struct Extras<'a> {
    pub world: &'a dyn CollisionWorld,
    pub water_world: Option<&'a dyn CollisionWorld>,
    pub hulls: Vec<Hull>,
    pub water: Vec<WaterQuad>,
    pub lights: Vec<PlacedLight>,
    pub markers: Vec<Marker>,
    pub grind: Vec<GrindCurve>,
}

fn map_vec(v: Vec3) -> [f32; 3] {
    [v.x, v.z, -v.y]
}

/// The lights a map keeps (at most `max` per 200 m cell: group "" first, then the highest priority), by position.
pub fn keep_lights(lights: &[PlacedLight], max: usize) -> Vec<PlacedLight> {
    let mut keep: Vec<PlacedLight> = if max > 0 {
        let mut groups: Vec<((i32, i32), Vec<&PlacedLight>)> = Vec::new();
        let mut gi: HashMap<(i32, i32), usize> = HashMap::new();
        for l in lights {
            let k = ((l.pos.x / 200.0).floor() as i32, (l.pos.y / 200.0).floor() as i32);
            let i = *gi.entry(k).or_insert_with(|| {
                groups.push((k, Vec::new()));
                groups.len() - 1
            });
            groups[i].1.push(l);
        }
        let mut out = Vec::new();
        for (_, mut g) in groups {
            g.sort_by(|a, b| {
                i32::from(!a.group.is_empty())
                    .cmp(&i32::from(!b.group.is_empty()))
                    .then(
                        b.priority
                            .partial_cmp(&a.priority)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            });
            out.extend(g.into_iter().take(max).cloned());
        }
        out
    } else {
        lights.to_vec()
    };
    let cmp = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    keep.sort_by(|a, b| {
        cmp(a.pos.x, b.pos.x)
            .then(cmp(a.pos.y, b.pos.y))
            .then(cmp(a.pos.z, b.pos.z))
    });
    keep
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

/// Sutherland-Hodgman, one step.
fn clip_half(p: &[(f32, f32)], along_y: bool, k: f32, keep_above: bool) -> Vec<(f32, f32)> {
    let mut o = Vec::with_capacity(p.len() + 1);
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (mut da, mut db) = (
            (if along_y { a.1 } else { a.0 }) - k,
            (if along_y { b.1 } else { b.0 }) - k,
        );
        if !keep_above {
            da = -da;
            db = -db;
        }
        if da >= 0.0 {
            o.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            let t = da / (da - db);
            o.push(if along_y {
                (a.0 + (b.0 - a.0) * t, k)
            } else {
                (k, a.1 + (b.1 - a.1) * t)
            });
        }
    }
    o
}

impl MapWriter {
    /// The water surfaces that touch the region as one mesh with Studio's ocean material.
    fn water(&mut self, quads: &[WaterQuad], material: &str) -> Result<Option<MapObject>> {
        let (r, off, limit) = (self.o.region, self.off, self.o.limit);
        let origin = Vec3::new((r[0] + r[2]) as f32 / 2.0, (r[1] + r[3]) as f32 / 2.0, 0.0);
        let (mut pos, mut nrm, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::<u32>::new());
        for q in quads {
            if f64::from(q.x1) <= r[0]
                || f64::from(q.x0) >= r[2]
                || f64::from(q.y1) <= r[1]
                || f64::from(q.y0) >= r[3]
            {
                continue;
            }
            let z = q.z;
            let bx0 = (r[0] as f32 - 400.0).max(-limit - off.x);
            let bx1 = (r[2] as f32 + 400.0).min(limit - off.x);
            let by0 = (r[1] as f32 - 400.0).max(-limit - off.y);
            let by1 = (r[3] as f32 + 400.0).min(limit - off.y);
            let mut cs = clip_half(
                &clip_half(
                    &clip_half(&clip_half(&q.corners, false, bx0, true), false, bx1, false),
                    true,
                    by0,
                    true,
                ),
                true,
                by1,
                false,
            );
            if cs.len() < 3 {
                continue;
            }
            let mut a = 0f32;
            for i in 0..cs.len() {
                let (p, n) = (cs[i], cs[(i + 1) % cs.len()]);
                a += p.0 * n.1 - n.0 * p.1;
            }
            if a.abs() < 1.0 {
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

    /// --tex-budget: the colour textures covering the least area become a shared white texture with their mean.
    fn budget(&mut self) {
        let used: Vec<usize> = self
            .ctx
            .mats
            .list
            .iter()
            .filter(|m| self.mat_used.contains(&m.id))
            .map(|m| m.id)
            .collect();
        let mut area: HashMap<usize, f64> = HashMap::new();
        let mut tiles: Vec<_> = self.tile_area.iter().collect();
        tiles.sort_by(|a, b| a.0.1.cmp(&b.0.1).then(a.0.0.cmp(&b.0.0)));
        for (_, kv) in tiles {
            let mut items: Vec<(&usize, &f64)> = kv.iter().collect();
            items.sort_by_key(|(m, _)| **m);
            for (m, a) in items {
                *area.entry(*m).or_default() += a;
            }
        }
        let mut weight: HashMap<i32, f64> = HashMap::new();
        let mut order: Vec<i32> = Vec::new();
        let mut add = |t: i32, v: f64, weight: &mut HashMap<i32, f64>| {
            if t >= 0 {
                if !weight.contains_key(&t) {
                    order.push(t);
                }
                *weight.entry(t).or_default() += v;
            }
        };
        // materials in first-use order, ties between textures by their first registration: never by name, since
        // names end in a content hash
        for &id in &used {
            let m = self.ctx.mats.by_id(id);
            let a = area.get(&id).copied().unwrap_or(0.0);
            let (t, n, k) = (m.tex, m.normal, m.mask);
            add(t, a, &mut weight);
            add(n, a * f64::from(self.o.tex_aux_weight), &mut weight);
            add(k, a * f64::from(self.o.tex_aux_weight), &mut weight);
        }
        if weight.len() <= self.o.tex_budget as usize {
            return;
        }
        let files = self.ctx.tex.all().to_vec();
        let mut ranked: Vec<i32> = order.clone();
        ranked.sort_by(|a, b| {
            weight[b]
                .partial_cmp(&weight[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(b))
        });
        let keep: BTreeSet<i32> = ranked.into_iter().take(self.o.tex_budget as usize - 1).collect();
        let mut white = -1;
        for id in used {
            let m = self.ctx.mats.list[id].clone();
            let mut m2 = m.clone();
            if m.normal >= 0 && !keep.contains(&m.normal) {
                m2.normal = -1;
            }
            if m.mask >= 0 && !keep.contains(&m.mask) {
                m2.mask = -1;
            }
            if m.tex >= 0 && !keep.contains(&m.tex) {
                let mean = files[m.tex as usize].mean;
                let lin = |c: u32| {
                    let v = c as f32 / 255.0;
                    if !m.srgb {
                        v
                    } else {
                        primase_image::srgb_to_linear_f32(v)
                    }
                };
                m2.base_factor = Some([
                    lin((mean >> 16) & 255),
                    lin((mean >> 8) & 255),
                    lin(mean & 255),
                    1.0,
                ]);
                if white < 0 {
                    white = self.ctx.tex.register_flat(0xFFFFFF, 255);
                }
                m2.tex = white;
                self.r.budget_flattened += 1;
            }
            self.ctx.mats.list[id] = m2;
        }
        self.r.warnings.push(format!(
            "texture budget {}: {} materials flattened to their mean colour",
            self.o.tex_budget, self.r.budget_flattened
        ));
    }

    fn material_json(&self, m: &Material) -> MaterialJson {
        let files = self.ctx.tex.all();
        if m.ocean.unwrap_or(self.o.water_ocean) && m.class == VClass::Water {
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

    fn effects_json(
        &mut self,
        markers: &[Marker],
        kind: &str,
        col: &collision::CollisionResult,
    ) -> Vec<EffectJson> {
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
        let o = &self.o;
        let mut out_tex: Vec<&crate::texture::OutTexture> =
            self.ctx.tex.all().iter().filter(|t| !t.removed).collect();
        out_tex.sort_by(|a, b| a.file.cmp(&b.file));
        let index = TexturesIndex {
            tex_cap: o.tex_cap,
            tex_caps: o.tex_caps.as_deref().unwrap_or(""),
            textures: out_tex
                .iter()
                .map(|t| TextureEntry {
                    file: format!("textures/{}", t.file),
                    w: t.w,
                    h: t.h,
                    src_w: t.source_w,
                    src_h: t.source_h,
                    src_format: &t.source_format,
                    bytes: t.bytes,
                    sources: t.sources.iter().map(String::as_str).collect(),
                    src_variants: (t.source_variants.len() > 1)
                        .then(|| t.source_variants.iter().map(String::as_str).collect()),
                })
                .collect(),
        };
        let bytes = crate::json_bytes("textures_index.json", &index, false)?;
        self.r.tex_bytes = out_tex.iter().map(|t| t.bytes).sum();
        self.r.tex_pixels = out_tex.iter().map(|t| u64::from(t.w) * u64::from(t.h)).sum();
        self.files.insert("textures_index.json".into(), bytes);
        Ok(())
    }

    /// Collision, water, grind curves, markers, lights, textures_index.json, pieces.json, map.json, trainer.json.
    pub fn finish(&mut self, x: Extras) -> Result<()> {
        let o = self.o.clone();
        let (r, off) = (o.region, self.off);
        let mut markers = x.markers.clone();
        if !markers.iter().any(|m| m.kind == "spawn") {
            markers.insert(
                0,
                Marker {
                    kind: "spawn",
                    name: Some("spawn".into()),
                    x: (r[0] + r[2]) as f32 / 2.0,
                    y: (r[1] + r[3]) as f32 / 2.0,
                    z: f32::NAN,
                    yaw: 0.0,
                    snap: crate::scene::SnapSpec::of(SnapMode::Street),
                    lift: f64::NAN,
                    shelter: false,
                    trainer: true,
                    blueprint: None,
                    transform: None,
                },
            );
        }
        let queries: Vec<Query> = markers
            .iter()
            .map(|m| Query {
                x: m.x,
                y: m.y,
                z: m.z,
                snap: m.snap.clone(),
            })
            .collect();
        self.pm.sea_tint = !x.water.is_empty();
        for (f, b) in self.pm.save()? {
            self.files.insert(f, b);
        }
        let surfaces = self.surfaces.clone();
        let mut collision = Collision::new(&surfaces, off.x, off.y, off.z, o.limit);
        let mut prop_hulls = std::mem::take(&mut self.prop_hulls);
        prop_hulls.sort_by(|a, b| a.name.cmp(&b.name));
        let hulls: Vec<Hull> = x.hulls.into_iter().chain(prop_hulls).collect();
        let snap_extra: Vec<(Vec<f32>, Vec<u32>, bool)> = hulls
            .iter()
            .map(|h| (h.pos.clone(), h.idx.clone(), h.ground))
            .collect();
        let mut col_files = collision::Files::new();
        let props = join(&self.prop_parts);
        let mut col = collision.build(
            x.world,
            r,
            &mut col_files,
            &queries,
            &props,
            &x.water,
            &snap_extra,
        )?;
        let water_props = join(&self.water_parts);
        if x.water_world.is_some() || !water_props.is_empty() {
            let ww: &dyn CollisionWorld = x.water_world.unwrap_or(&NoWorld);
            let wc = collision.build_water(ww, r, &mut col_files, &water_props)?;
            col.objects.extend(wc.objects);
            col.pieces += wc.pieces;
            col.water_triangles += wc.water_triangles + wc.prop_triangles;
            col.split_cells += wc.split_cells;
        }
        for h in &hulls {
            collision.add_hull(&mut col, &h.name, &h.pos, &h.idx, h.surface, &mut col_files)?;
        }
        self.mesh_files.extend(col_files);

        // grind curves: an invisible prism each, world baked
        for (i, g) in x.grind.iter().enumerate() {
            let (gpos, gidx, pieces, length) = grind::build(g);
            if gidx.is_empty() {
                self.r
                    .warnings
                    .push(format!("grind curve {i} has no usable points"));
                continue;
            }
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in &gpos {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            if gidx.len() as f32 / (hi - lo).length().max(1e-6) > 900.0 {
                self.r.warnings.push(format!(
                    "grind curve {i} is too detailed for its size to grind; use fewer points"
                ));
            }
            let flat: Vec<f32> = gpos
                .iter()
                .flat_map(|p| [p.x + off.x, p.y + off.y, p.z + off.z])
                .collect();
            let name = format!("grind_{i:05}");
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
            let mut ob = MapObject::new(name.clone(), MapObject::translation(0.0, 0.0, 0.0));
            ob.collision_mesh = Some(format!("meshes/{name}.glb"));
            ob.render = false;
            ob.collision_mode = "triangle_mesh";
            ob.collision_packed = grind::SURFACES[g.surface].1;
            ob.grind_source = Some(name);
            col.objects.push(ob);
            self.r.grind_curves += 1;
            self.r.grind_pieces += i64::from(pieces);
            self.r.grind_length += length;
        }

        const OCEAN: &str = "ocean_water";
        let water = if o.water && !x.water.is_empty() {
            self.water(&x.water, OCEAN)?
        } else {
            None
        };

        // textures
        self.ctx.tex.finish();
        if o.tex_budget > 0 {
            self.budget();
        }
        let used: BTreeSet<i32> = self
            .mat_used
            .iter()
            .flat_map(|&id| {
                let m = self.ctx.mats.by_id(id);
                [m.tex, m.normal, m.mask]
            })
            .filter(|t| *t >= 0)
            .collect();
        self.r.tex_removed = self.ctx.tex.prune(&used);
        self.write_index()?;
        for m in &mut self.ctx.mats.list {
            m.tris = self.mat_tris.get(&m.id).copied().unwrap_or(0) as u64;
        }
        if self.piece_log.is_some() {
            self.write_pieces()?;
        }

        let used_mats: BTreeSet<String> = self.objects.iter().filter_map(|ob| ob.material.clone()).collect();
        self.r.materials = used_mats.len();
        let mut objs = std::mem::take(&mut self.objects);
        objs.sort_by(|a, b| a.name.cmp(&b.name));
        let mut cobjs = std::mem::take(&mut col.objects);
        cobjs.sort_by(|a, b| a.name.cmp(&b.name));
        let obj_arr: Vec<MapObject> = objs.into_iter().chain(cobjs).chain(water.clone()).collect();
        self.r.object_records = obj_arr.len();
        let kept_lights = keep_lights(&x.lights, o.lights_max_per_cell);
        self.r.lights_written = kept_lights.len();
        self.r.lights_dropped = x.lights.len() - kept_lights.len();

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
            let bytes = crate::json_bytes("vertex_blend.json", &doc, true)?;
            self.files.insert("vertex_blend.json".into(), bytes);
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

    fn write_pieces(&mut self) -> Result<()> {
        let Some(log) = &self.piece_log else { return Ok(()) };
        let mut recs: Vec<&PieceRec> = log.iter().collect();
        recs.sort_by(|a, b| {
            a.mesh
                .cmp(&b.mesh)
                .then(a.pass.cmp(&b.pass))
                .then(a.order.cmp(&b.order))
        });
        let r2 = |v: Vec3| [v.x, v.y, v.z];
        let pieces = PiecesJson {
            overlay_step_m: self.o.overlay_step,
            pieces: recs
                .into_iter()
                .map(|p| PieceJson {
                    mesh: &p.mesh,
                    material: &p.material,
                    pass: p.pass,
                    order: p.order,
                    shift_mm: p.shift_mm,
                    tris: p.tris,
                    gta: r2(p.centre),
                    drawable: &p.drawable,
                    entity: r2(p.entity),
                    shader: &p.shader,
                    bucket: p.bucket,
                    model: p.model,
                    geometry: p.geom,
                    entity_no: p.entity_no,
                })
                .collect(),
        };
        let bytes = crate::json_bytes("pieces.json", &pieces, true)?;
        self.files.insert("pieces.json".into(), bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(dir: Vec3, tan: Vec3) -> PlacedLight {
        PlacedLight {
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
