//! Placements of prototypes -> ReSkate Studio's normalized map folder: static geometry merged into one mesh per map
//! tile (one primitive per material), repeated prototypes kept as one mesh placed many times; then collision,
//! water, grind prisms, markers, lights, textures, map.json and trainer.json. The reference writer's MapWriter.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::Result;
use crate::collision::{self, Collision, CollisionWorld, NoWorld, PropSoup, Query};
use crate::dotnet::{JsonWriter, r4};
use crate::glb::{self, GlbPrim};
use crate::grind;
use crate::material::{MatLib, Material, VClass};
use crate::model::{CollisionMode, Hull, MapObject, Placement, ProtoInfo, SrcMesh, SrcPart, TileOut};
use crate::overlay::{self, OverlayStats, Parts, Piece};
use crate::pausemap::PauseMap;
use crate::scene::{GrindCurve, Marker, PlacedLight, SnapMode, SurfaceTable, WaterQuad};
use crate::texture::TexLib;
use crate::vec::{Num, Sdx};

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
    pub prop_col_tris: BTreeMap<String, i64>,
    pub collision: Option<collision::CollisionResult>,
    pub lights_written: usize,
    pub lights_dropped: usize,
    pub grind_curves: i64,
    pub grind_pieces: i64,
    pub grind_length: f32,
    pub trainer_spots: usize,
    pub travel_points: usize,
    pub budget_flattened: i64,
    pub warnings: Vec<String>,
}

/// One merged part: its geometry and its pieces (one per append).
#[derive(Clone, Debug, Default)]
struct Acc {
    pos: Vec<f32>,
    nrm: Vec<f32>,
    uv: Vec<f32>,
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
    entity: Sdx,
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
    centre: Sdx,
    drawable: String,
    entity: Sdx,
    shader: String,
    bucket: u8,
    model: i32,
    geom: i32,
    entity_no: i32,
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    degenerate: bool,
    sheared: bool,
    mirrored: bool,
    longest: f32,
}

fn shape_of(s: &Placement) -> Shape {
    let (lx, ly, lz) = (s.ex.length(), s.ey.length(), s.ez.length());
    let det = s.ex.cross(s.ey).dot(s.ez);
    // as written in the reference: any NaN counts as degenerate
    let small = |l: f32| l.is_nan() || l < 1e-6;
    let lim = 1e-6 * lx * ly * lz;
    let degenerate = small(lx) || small(ly) || small(lz) || det.is_nan() || lim.is_nan() || det.abs() <= lim;
    let sheared = !degenerate
        && (s.ex.dot(s.ey).abs() > 1e-4 * lx * ly
            || s.ey.dot(s.ez).abs() > 1e-4 * ly * lz
            || s.ez.dot(s.ex).abs() > 1e-4 * lz * lx);
    Shape {
        degenerate,
        sheared,
        mirrored: det < 0.0,
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
fn append_core(a: &mut Acc, p: &SrcPart, s: Option<(&Placement, Shape)>, origin: Sdx, only: Option<&[u32]>) {
    let vbase = (a.pos.len() / 3) as u32;
    let Some((s, sh)) = s else {
        a.pos.extend_from_slice(&p.pos);
        a.nrm.extend_from_slice(&p.nrm);
        a.uv.extend_from_slice(&p.uv);
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
    let flip = sh.mirrored;
    // the mesh to transform: every vertex (the part's order), or the kept triangles' vertices in the order they are
    // first used, corners taken in the order a mirrored placement writes them
    let m = match only {
        None => mesh_of(p.idx.clone(), None),
        Some(only) => {
            let mut corners: Vec<u32> = Vec::with_capacity(only.len());
            for t in only.as_chunks::<3>().0 {
                if flip {
                    corners.extend_from_slice(&[t[0], t[2], t[1]])
                } else {
                    corners.extend_from_slice(t)
                }
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
            if flip {
                // transform turns them over again
                for t in idx.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            mesh_of(idx, Some(&verts))
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
    let _ = sh.sheared;
    a.pos.extend(out.positions.iter().flatten());
    a.nrm.extend(out.normals.iter().flatten());
    a.uv.extend(out.uv.iter().flatten());
    a.idx.extend(out.indices.iter().map(|i| vbase + i));
}

/// Studio stores a mesh section with 16-bit indices.
pub const MAX_SECTION_VERTS: usize = 60000;

/// The GLB primitives of merged parts, in the order of their material names.
fn prims(parts: &BTreeMap<usize, Acc>, mats: &MatLib) -> Vec<GlbPrim> {
    let mut list = Vec::new();
    let mut order: Vec<(&usize, &Acc)> = parts.iter().collect();
    order.sort_by(|a, b| mats.by_id(*a.0).name.cmp(&mats.by_id(*b.0).name));
    for (mat, a) in order {
        let name = mats.by_id(*mat).name.clone();
        if a.pos.len() / 3 <= MAX_SECTION_VERTS {
            list.push(GlbPrim {
                material: Some(name),
                pos: a.pos.clone(),
                nrm: Some(a.nrm.clone()),
                uv: Some(a.uv.clone()),
                idx: a.idx.clone(),
            });
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
            list.push(GlbPrim {
                material: Some(name.clone()),
                pos: take(&a.pos, 3),
                nrm: Some(take(&a.nrm, 3)),
                uv: Some(take(&a.uv, 2)),
                idx: c.indices,
            });
        }
    }
    list
}

fn area(pos: &[f32], idx: &[u32]) -> f64 {
    let mut s = 0f64;
    for t in idx.as_chunks::<3>().0 {
        let a = Sdx::at(pos, t[0] as usize * 3);
        let b = Sdx::at(pos, t[1] as usize * 3);
        let c = Sdx::at(pos, t[2] as usize * 3);
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
    off: Sdx,
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
    pub fn new(o: WriterOptions, surfaces: SurfaceTable, tex_dir: &std::path::Path) -> Result<MapWriter> {
        let off = Sdx::new(o.offset[0] as f32, o.offset[1] as f32, o.offset[2] as f32);
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

    /// OverlayStack over a set of merged parts; logs the overlay pieces with --piece-log.
    fn stack_overlays(&mut self, parts: &mut BTreeMap<usize, Acc>, origin: Sdx, mesh: &str) {
        let step = self.o.overlay_step;
        if step <= 0.0 && self.piece_log.is_none() {
            return;
        }
        let mut pieces: Vec<Piece> = Vec::new();
        let mut src: Vec<(usize, usize)> = Vec::new();
        let keys: Vec<usize> = parts.keys().copied().collect();
        for (pi, mat) in keys.iter().enumerate() {
            let m = self.ctx.mats.by_id(*mat);
            let pass = if m.class == VClass::Water {
                2
            } else if m.alpha == 1 {
                0
            } else {
                1
            };
            let a = &parts[mat];
            for (i, pc) in a.pieces.iter().enumerate() {
                pieces.push(Piece {
                    part: pi,
                    t0: pc.t0,
                    t1: pc.t1,
                    v0: pc.v0,
                    v1: pc.v1,
                    overlay: matches!(m.class, VClass::Decal | VClass::Water),
                    water: m.class == VClass::Water,
                    pass,
                    order: pc.order,
                    entity: pc.entity,
                    shift: 0.0,
                });
                src.push((*mat, i));
            }
        }
        if step > 0.0 {
            let mut accs: Vec<&mut Acc> = parts.values_mut().collect();
            let idx: Vec<Vec<u32>> = accs.iter().map(|a| a.idx.clone()).collect();
            let mut view = Parts {
                pos: accs.iter_mut().map(|a| &mut a.pos).collect(),
                idx: idx.iter().collect(),
            };
            let st = overlay::apply(step, &mut view, &mut pieces);
            self.r.overlay.add(&st);
        }
        let Some(log) = &mut self.piece_log else { return };
        for (k, p) in pieces.iter().enumerate() {
            if !p.overlay {
                continue;
            }
            let (mat, i) = src[k];
            let a = &parts[&mat];
            let mut c = Sdx::ZERO;
            for v in p.v0..p.v1 {
                c += Sdx::new(a.pos[v * 3], a.pos[v * 3 + 1], a.pos[v * 3 + 2]);
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
    }

    #[allow(clippy::too_many_arguments)]
    fn append(
        a: &mut Acc,
        p: &SrcPart,
        s: Option<(&Placement, Shape)>,
        origin: Sdx,
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
                entity: s.map_or(Sdx::ZERO, |x| x.0.pos),
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
                parts.entry(mat).or_default(),
                p,
                None,
                Sdx::ZERO,
                order as i32,
                None,
                &m.name,
                0,
            );
        }
        self.stack_overlays(&mut parts, Sdx::ZERO, &format!("proto {}", m.name));
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
        let pr = prims(&parts, &self.ctx.mats);
        self.mesh_files
            .insert(format!("{stem}.glb"), glb::write(&stem, &pr));
        let origins = pr
            .iter()
            .map(|p| {
                let (mut lo, mut hi) = (Sdx::splat(f32::MAX), Sdx::splat(f32::MIN));
                for v in (0..p.pos.len().saturating_sub(2)).step_by(3) {
                    let q = Sdx::new(p.pos[v], p.pos[v + 1], p.pos[v + 2]);
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
                let origin = Sdx::new((tx as f32 + 0.5) * o.tile, (ty as f32 + 0.5) * o.tile, 0.0);
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
                        && (sh.sheared
                            || sh.mirrored
                            || s.no_instance
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
                            let mut ii = ci.clone();
                            if sh.mirrored {
                                for t in ii.as_chunks_mut::<3>().0 {
                                    t.swap(1, 2);
                                }
                            }
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
                            parts.entry(mat).or_default(),
                            p,
                            Some((s, sh)),
                            origin,
                            piece_order,
                            only,
                            &m.name,
                            entity_no,
                        );
                        piece_order += 1;
                    }
                }
                self.stack_overlays(&mut parts, origin, &format!("t_{tx}_{ty}"));
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
                                    Sdx::new(origin.x + off.x, origin.y + off.y, off.z),
                                    Sdx::UNIT_X,
                                    Sdx::UNIT_Y,
                                    Sdx::UNIT_Z,
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
                                let (mut cp, mut cn, mut cu) = (Vec::new(), Vec::new(), Vec::new());
                                for i in &mut keep_idx {
                                    let vi = *i as usize;
                                    if remap[vi] == u32::MAX {
                                        remap[vi] = (cp.len() / 3) as u32;
                                        cp.extend_from_slice(&a.pos[vi * 3..vi * 3 + 3]);
                                        cn.extend_from_slice(&a.nrm[vi * 3..vi * 3 + 3]);
                                        cu.extend_from_slice(&a.uv[vi * 2..vi * 2 + 2]);
                                    }
                                    *i = remap[vi];
                                }
                                a.pos = cp;
                                a.nrm = cn;
                                a.uv = cu;
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
                    let pr = prims(&parts, &self.ctx.mats);
                    let rows = MapObject::translation(
                        origin.x + off.x,
                        origin.y + off.y,
                        if off.z != 0.0 { origin.z + off.z } else { origin.z },
                    );
                    let file = to.mesh.clone().expect("mesh");
                    self.mesh_files
                        .insert(file.clone(), glb::write(&format!("t_{tx}_{ty}"), &pr));
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
                    for (k, a) in &parts {
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
    a.pos = out.positions.iter().flatten().copied().collect();
    a.nrm = out.normals.iter().flatten().copied().collect();
    a.uv = out.uv.iter().flatten().copied().collect();
    a.idx = out.indices;
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

/// `MathF.Round(x, digits)` (to even), in single precision.
pub fn round_f32(x: f32, digits: u32) -> f32 {
    if x.abs() < 1e8 {
        let p = 10f32.powi(digits as i32);
        (x * p).round_ties_even() / p
    } else {
        x
    }
}

fn map_vec(v: Sdx) -> [f32; 3] {
    [v.x, v.z, -v.y]
}

fn arr_r(w: &mut JsonWriter, k: &str, v: &[f32]) {
    w.start_array_named(k);
    for x in v {
        w.f64_value(r4(f64::from(*x)));
    }
    w.end_array();
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

fn write_lights(w: &mut JsonWriter, kept: &[PlacedLight], off: Sdx, prefix: &str) {
    for (n, l) in kept.iter().enumerate() {
        let fwd = l.dir;
        let mut right = l.tan - fwd * l.tan.dot(fwd);
        if right.length_squared() > 1e-12 {
            right = right.normalize();
        } else if fwd.x.abs() < 0.9 {
            right = Sdx::UNIT_X;
        } else {
            right = (Sdx::UNIT_Y - fwd * fwd.y).normalize();
        }
        let up = fwd.cross(right).normalize();
        w.start_object();
        w.string("name", &format!("{prefix}{n:06}"));
        w.string("source_path", &l.source);
        w.string("type", if l.spot { "spot" } else { "point" });
        arr_r(w, "position", &map_vec(l.pos + off));
        arr_r(w, "direction", &map_vec(fwd));
        w.start_object_named("basis");
        arr_r(w, "right", &map_vec(right));
        arr_r(w, "up", &map_vec(up));
        arr_r(w, "forward", &map_vec(fwd));
        w.end_object();
        arr_r(w, "color_linear", &[l.r, l.g, l.b]);
        w.f64("energy", r4(f64::from(l.energy)));
        w.bool("normalize_power", true);
        w.bool("uses_light_nodes", false);
        w.int("exposure", 0);
        w.bool("cast_shadow", l.cast_shadow);
        w.bool("use_custom_distance", true);
        w.f64("cutoff_distance", r4(f64::from(l.range)));
        w.int("diffuse_factor", 1);
        w.int("specular_factor", 1);
        w.int("time_of_day", i64::from(l.time_of_day));
        w.bool("native_entity_authored", false);
        w.f64("radius", r4(f64::from(l.radius)));
        if l.spot {
            w.f64("spot_size_radians", r4(f64::from(l.spot_size)));
            w.f64("spot_blend", r4(f64::from(l.spot_blend)));
        }
        w.end_object();
    }
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
    fn water(&mut self, quads: &[WaterQuad], material: &str) -> Option<MapObject> {
        let (r, off, limit) = (self.o.region, self.off, self.o.limit);
        let origin = Sdx::new((r[0] + r[2]) as f32 / 2.0, (r[1] + r[3]) as f32 / 2.0, 0.0);
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
            return None;
        }
        self.mesh_files.insert(
            "water.glb".into(),
            glb::write(
                "water",
                &[GlbPrim {
                    material: Some(material.to_owned()),
                    pos,
                    nrm: Some(nrm),
                    uv: Some(uv),
                    idx,
                }],
            ),
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
        Some(ob)
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
            items.sort_by(|a, b| {
                self.ctx
                    .mats
                    .by_id(*a.0)
                    .name
                    .cmp(&self.ctx.mats.by_id(*b.0).name)
            });
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
        let mut by_name = used.clone();
        by_name.sort_by(|a, b| self.ctx.mats.by_id(*a).name.cmp(&self.ctx.mats.by_id(*b).name));
        for id in by_name {
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
                .then(files[*a as usize].file.cmp(&files[*b as usize].file))
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
                    } else if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
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

    fn write_material(&self, w: &mut JsonWriter, m: &Material) {
        let files = self.ctx.tex.all();
        if m.ocean.unwrap_or(self.o.water_ocean) && m.class == VClass::Water {
            w.start_object_named(&m.name);
            w.string("surface", "water");
            w.bool("srgb", true);
            w.string("domain", "ocean");
            w.string("shader_override", "ocean");
            w.string("alpha", "opaque");
            w.end_object();
            return;
        }
        let domain = match m.domain {
            1 => "decal",
            2 => "foliage",
            _ => "surface",
        };
        let alpha = match m.alpha {
            2 => "mask",
            3 => "blend",
            _ => "opaque",
        };
        w.start_object_named(&m.name);
        w.string("surface", "default");
        w.bool("srgb", m.srgb);
        w.string("domain", domain);
        w.string("shader_override", domain);
        w.string(
            "alpha_source",
            if m.alpha == 1 {
                "constant"
            } else {
                "base_color_texture"
            },
        );
        w.f32("alpha_cutoff", m.cutoff);
        w.bool(
            "cast_shadows",
            m.cast_shadows
                .unwrap_or(m.class != VClass::Decal && m.class != VClass::Water),
        );
        w.bool("double_sided", m.two_sided);
        w.bool("transparent_shadow", m.alpha == 2);
        w.string("alpha", alpha);
        w.string("blend_method", if m.alpha == 3 { "blend" } else { "clip" });
        w.f64(
            "roughness",
            m.roughness
                .unwrap_or(if matches!(m.class, VClass::Glass | VClass::Water) {
                    0.15
                } else {
                    0.85
                }),
        );
        w.f64("metallic", m.metallic.unwrap_or(0.0));
        if m.tex >= 0 {
            w.string("texture", &format!("textures/{}", files[m.tex as usize].file));
        }
        if let Some(f) = m.base_factor {
            arr_r(w, "base_color_factor", &f);
        }
        if let Some(wr) = &m.wrap {
            w.string("base_color_extension", wr);
        }
        if m.normal >= 0 {
            w.string(
                "normal_texture",
                &format!("textures/{}", files[m.normal as usize].file),
            );
            w.bool("normal_srgb", false);
        }
        if m.mask >= 0 {
            w.string(
                "mask_texture",
                &format!("textures/{}", files[m.mask as usize].file),
            );
            w.bool("mask_srgb", false);
        }
        w.end_object();
    }

    fn write_effects(
        &mut self,
        w: &mut JsonWriter,
        key: &str,
        markers: &[Marker],
        kind: &str,
        col: &collision::CollisionResult,
    ) {
        w.start_array_named(key);
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
                    Sdx::new(yaw.cos() as f32, (-yaw.sin()) as f32, 0.0),
                    Sdx::new(yaw.sin() as f32, yaw.cos() as f32, 0.0),
                    Sdx::UNIT_Z,
                    Sdx::ZERO,
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
        for (name, bp, t) in fx {
            w.start_object();
            w.string("name", &name);
            w.string("blueprint", &bp);
            w.start_array_named("transform");
            for v in t {
                w.f32_value(v);
            }
            w.end_array();
            w.end_object();
        }
        w.end_array();
    }

    fn write_object(w: &mut JsonWriter, ob: &MapObject) {
        w.start_object();
        w.string("name", &ob.name);
        w.string("placement_mode", "authored_mesh");
        if ob.render {
            w.string("mesh", ob.mesh.as_deref().unwrap_or(""));
            w.int("primitive", ob.primitive as i64);
            w.string("material", ob.material.as_deref().unwrap_or(""));
            if let Some(o) = ob.shared_origin {
                w.bool("world_transform_baked", false);
                w.bool("shared_geometry", true);
                w.start_array_named("native_render_origin");
                for v in o {
                    w.f32_value(v);
                }
                w.end_array();
            }
        } else {
            w.bool("render", false);
            w.string("collision_mesh", ob.collision_mesh.as_deref().unwrap_or(""));
        }
        w.start_array_named("transform");
        for v in ob.transform {
            w.f32_value(v);
        }
        w.end_array();
        w.bool("static", true);
        w.string("collision_mode", ob.collision_mode);
        w.string(
            "collision_material",
            &format!("material_{:04}", ob.collision_packed),
        );
        w.int("collision_material_packed", i64::from(ob.collision_packed));
        if let Some(g) = &ob.grind_source {
            w.bool("world_transform_baked", true);
            w.string("source_object", g);
            w.bool("grind_curve", true);
        }
        w.end_object();
    }

    fn write_index(&mut self) {
        let o = &self.o;
        let mut out_tex: Vec<&crate::texture::OutTexture> =
            self.ctx.tex.all().iter().filter(|t| !t.removed).collect();
        out_tex.sort_by(|a, b| a.file.cmp(&b.file));
        let mut iw = JsonWriter::new(false);
        iw.start_object();
        iw.int("tex_cap", i64::from(o.tex_cap));
        iw.string("tex_caps", o.tex_caps.as_deref().unwrap_or(""));
        iw.start_array_named("textures");
        for t in &out_tex {
            iw.start_object();
            iw.string("file", &format!("textures/{}", t.file));
            iw.int("w", i64::from(t.w));
            iw.int("h", i64::from(t.h));
            iw.int("src_w", i64::from(t.source_w));
            iw.int("src_h", i64::from(t.source_h));
            iw.string("src_format", &t.source_format);
            iw.int("bytes", t.bytes as i64);
            iw.start_array_named("sources");
            for n in &t.sources {
                iw.string_value(n);
            }
            iw.end_array();
            if t.source_variants.len() > 1 {
                iw.start_array_named("src_variants");
                for n in &t.source_variants {
                    iw.string_value(n);
                }
                iw.end_array();
            }
            iw.end_object();
        }
        iw.end_array();
        iw.end_object();
        self.r.tex_bytes = out_tex.iter().map(|t| t.bytes).sum();
        self.r.tex_pixels = out_tex.iter().map(|t| u64::from(t.w) * u64::from(t.h)).sum();
        self.files
            .insert("textures_index.json".into(), iw.finish().into_bytes());
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
        for (f, b) in self.pm.save() {
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
        );
        let water_props = join(&self.water_parts);
        if x.water_world.is_some() || !water_props.is_empty() {
            let ww: &dyn CollisionWorld = x.water_world.unwrap_or(&NoWorld);
            let wc = collision.build_water(ww, r, &mut col_files, &water_props);
            col.objects.extend(wc.objects);
            col.pieces += wc.pieces;
            col.water_triangles += wc.water_triangles + wc.prop_triangles;
            col.split_cells += wc.split_cells;
        }
        for h in &hulls {
            collision.add_hull(&mut col, &h.name, &h.pos, &h.idx, h.surface, &mut col_files);
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
            let (mut lo, mut hi) = (Num::splat(f32::MAX), Num::splat(f32::MIN));
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
                        material: None,
                        pos: flat,
                        nrm: None,
                        uv: None,
                        idx: gidx,
                    }],
                ),
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
            self.water(&x.water, OCEAN)
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
        self.write_index();
        for m in &mut self.ctx.mats.list {
            m.tris = self.mat_tris.get(&m.id).copied().unwrap_or(0) as u64;
        }
        if self.piece_log.is_some() {
            self.write_pieces();
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

        let mut trainer: Vec<(String, [f64; 3])> = Vec::new();
        let mut w = JsonWriter::new(true);
        w.start_object();
        w.int("format", 1);
        w.string("name", &o.name);
        w.string("display_name", o.display_name.as_deref().unwrap_or(&o.name));
        w.string("units", "meters");
        w.string("up", "y");
        w.string("forward", "-z");
        w.string("generator", &o.generator);
        w.start_object_named("materials");
        let mut mats_sorted: Vec<Material> = self.ctx.mats.list.clone();
        mats_sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for m in &mats_sorted {
            if used_mats.contains(&m.name) {
                self.write_material(&mut w, m);
            }
        }
        if water.is_some() {
            w.start_object_named(OCEAN);
            w.string("surface", "water");
            w.bool("srgb", true);
            w.string("domain", "ocean");
            w.string("shader_override", "ocean");
            w.string("alpha", "opaque");
            w.end_object();
        }
        w.end_object();
        w.start_object_named("surface_profiles");
        w.end_object();
        w.start_object_named("streaming_distances");
        w.f64("near", o.streaming_distances[0]);
        w.f64("medium", o.streaming_distances[1]);
        w.f64("far", o.streaming_distances[2]);
        w.end_object();
        w.start_array_named("objects");
        for ob in &obj_arr {
            Self::write_object(&mut w, ob);
        }
        w.end_array();
        w.start_array_named("lights");
        write_lights(&mut w, &kept_lights, off, &o.light_prefix);
        w.end_array();
        for k in ["audio_volumes", "audio_emitters", "interaction_prefabs"] {
            w.start_array_named(k);
            w.end_array();
        }
        self.write_effects(&mut w, "trigger_effects", &markers, "trigger_effect", &col);
        self.write_effects(&mut w, "vfx_prefabs", &markers, "vfx", &col);
        w.start_array_named("npc_routes");
        w.end_array();
        w.start_array_named("warnings");
        for s in &o.warnings {
            w.string_value(s);
        }
        w.end_array();
        w.start_array_named("travel_points");
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
            w.start_object();
            w.string("name", &id);
            w.string("source_name", &id);
            w.string("source_path", &id);
            w.start_array_named("source_collections");
            w.string_value("Markers");
            w.end_array();
            let p = [r4(f64::from(sx + off.x)), r4(gy), r4(f64::from(-(sy + off.y)))];
            w.start_array_named("position");
            for v in p {
                w.f64_value(v);
            }
            w.end_array();
            w.f32("yaw", st.yaw as f32);
            w.string("stop_name", st.name.as_deref().unwrap_or(""));
            w.bool("shelter", st.shelter);
            w.end_object();
            travel += 1;
            if st.trainer {
                trainer.push((st.name.clone().unwrap_or_default(), p));
            }
        }
        w.end_array();
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
                sz = if col.ground[i].is_nan() {
                    if sp.z.is_nan() { 50.0 } else { sp.z }
                } else {
                    col.ground[i] + lift
                };
                if col.ground[i].is_nan() && sp.snap.mode != SnapMode::None {
                    self.r.warnings.push("spawn: its snap found nothing".into());
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
        let spawn_body = |w: &mut JsonWriter, s: &(f32, f32, f32, f64, String)| {
            w.start_array_named("position");
            w.f64_value(r4(f64::from(s.0 + off.x)));
            w.f64_value(r4(f64::from(s.2)));
            w.f64_value(r4(f64::from(-(s.1 + off.y))));
            w.end_array();
            w.f64("yaw", s.3);
        };
        w.start_object_named("spawn");
        w.string("name", "spawn");
        spawn_body(&mut w, &spawns[0]);
        w.end_object();
        if spawns.len() > 1 {
            w.start_array_named("spawn_points");
            for s in &spawns {
                w.start_object();
                w.string("name", &s.4);
                spawn_body(&mut w, s);
                w.end_object();
            }
            w.end_array();
        }
        w.end_object();
        self.files.insert("map.json".into(), w.finish().into_bytes());

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
                    r4(f64::from(sx + off.x)),
                    r4(f64::from(gz) + lift + f64::from(off.z)),
                    r4(f64::from(-(sy + off.y))),
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
            let mut t = JsonWriter::new(true);
            t.start_object();
            t.int("schema", 1);
            t.string(
                "note",
                o.trainer_note
                    .as_deref()
                    .or(o.display_name.as_deref())
                    .unwrap_or(&o.name),
            );
            t.start_array_named("spots");
            for (name, pos) in &trainer {
                t.start_object();
                t.string("name", name);
                t.start_array_named("position");
                for v in pos {
                    t.f64_value(*v);
                }
                t.end_array();
                t.end_object();
            }
            t.end_array();
            t.end_object();
            self.files.insert("trainer.json".into(), t.finish().into_bytes());
        }
        self.r.mesh_bytes = self.mesh_files.values().map(|b| b.len() as u64).sum();
        self.r.collision = Some(col);
        Ok(())
    }

    fn write_pieces(&mut self) {
        let Some(log) = &self.piece_log else { return };
        let mut recs: Vec<&PieceRec> = log.iter().collect();
        recs.sort_by(|a, b| {
            a.mesh
                .cmp(&b.mesh)
                .then(a.pass.cmp(&b.pass))
                .then(a.order.cmp(&b.order))
        });
        let mut w = JsonWriter::new(true);
        w.start_object();
        w.f32("overlay_step_m", self.o.overlay_step);
        w.start_array_named("pieces");
        for p in recs {
            w.start_object();
            w.string("mesh", &p.mesh);
            w.string("material", &p.material);
            w.int("pass", i64::from(p.pass));
            w.int("order", i64::from(p.order));
            w.f32("shift_mm", round_f32(p.shift_mm, 2));
            w.int("tris", p.tris as i64);
            w.start_array_named("gta");
            for v in [p.centre.x, p.centre.y, p.centre.z] {
                w.f32_value(round_f32(v, 2));
            }
            w.end_array();
            w.string("drawable", &p.drawable);
            w.start_array_named("entity");
            for v in [p.entity.x, p.entity.y, p.entity.z] {
                w.f32_value(round_f32(v, 2));
            }
            w.end_array();
            w.string("shader", &p.shader);
            w.int("bucket", i64::from(p.bucket));
            w.int("model", i64::from(p.model));
            w.int("geometry", i64::from(p.geom));
            w.int("entity_no", i64::from(p.entity_no));
            w.end_object();
        }
        w.end_array();
        w.end_object();
        self.files.insert("pieces.json".into(), w.finish().into_bytes());
    }
}
