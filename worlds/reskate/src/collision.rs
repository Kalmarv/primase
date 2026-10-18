//! Collision for ReSkate Studio: the world's triangles per 256 m cell plus the placed props' own collision, each
//! cell cut until its pieces are sparse enough for skate.'s grind-edge analysis, one piece per surface; and the
//! marker snaps that search that collision (street, ground, open).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::Result;
use crate::glb::{self, GlbPrim};
use crate::model::{MapObject, cells_met};
use crate::scene::{SnapMode, SnapSpec, SurfaceTable, WaterQuad};
use primase_meshops::vector::Vec3;

pub const SRC_TILE: f32 = 256.0;
pub const MAX_HALF_EDGES_PER_METRE: f32 = 600.0;
pub const MIN_CELL: f32 = 8.0;
pub const MAX_PIECE_TRIS: usize = 20000;
pub const MAX_PIECE_VERTS: usize = 65535;

/// World triangles filed under one cell: loose or indexed, ground triangles first.
#[derive(Clone, Debug, Default)]
pub struct TileData {
    pub v: Vec<f32>,
    pub t: Vec<u32>,
    pub s: Vec<u8>,
    pub rock: Vec<bool>,
    /// The first this many triangles are ground; `None`: all.
    pub ground: Option<usize>,
    /// Which triangles are ground, when ground props are mixed in.
    pub mask: Option<Vec<bool>>,
}

/// Where the world's collision comes from.
pub trait CollisionWorld: std::fmt::Debug {
    fn cells(&self) -> Vec<(i32, i32)>;
    fn cell(&self, tx: i32, ty: i32) -> Option<TileData>;
}

/// No world collision.
#[derive(Debug)]
pub struct NoWorld;
impl CollisionWorld for NoWorld {
    fn cells(&self) -> Vec<(i32, i32)> {
        Vec::new()
    }
    fn cell(&self, _: i32, _: i32) -> Option<TileData> {
        None
    }
}

/// World-space collision triangles of placed props, per source tile.
#[derive(Clone, Debug, Default)]
pub struct PropSoup {
    pub pos: Vec<f32>,
    pub surf: Vec<u8>,
    pub ground: Option<Vec<bool>>,
}

impl PropSoup {
    pub fn append(&mut self, o: &PropSoup) {
        if o.ground.is_some() || self.ground.is_some() {
            let g = self.ground.get_or_insert_with(|| vec![false; self.surf.len()]);
            match &o.ground {
                Some(og) => g.extend_from_slice(og),
                None => g.extend(std::iter::repeat_n(false, o.surf.len())),
            }
        }
        self.pos.extend_from_slice(&o.pos);
        self.surf.extend_from_slice(&o.surf);
    }
}

/// Adds a prop's collision placed by (p, ex, ey, ez); with `bbox`, only triangles whose outline box reaches into it.
#[allow(clippy::too_many_arguments)]
pub fn add_prop(
    soups: &mut BTreeMap<(i32, i32), PropSoup>,
    order: &mut Vec<(i32, i32)>,
    col_pos: &[f32],
    col_idx: &[u32],
    col_surf: &[u8],
    col_ground: bool,
    p: Vec3,
    ex: Vec3,
    ey: Vec3,
    ez: Vec3,
    bbox: Option<[f32; 4]>,
) -> i64 {
    let key = ((p.x / SRC_TILE).floor() as i32, (p.y / SRC_TILE).floor() as i32);
    let mut added = 0;
    let mut wv = [0f32; 9];
    for t in (0..col_idx.len().saturating_sub(2)).step_by(3) {
        for k in 0..3 {
            let vi = col_idx[t + k] as usize * 3;
            let (x, y, z) = (col_pos[vi], col_pos[vi + 1], col_pos[vi + 2]);
            wv[k * 3] = p.x + ex.x * x + ey.x * y + ez.x * z;
            wv[k * 3 + 1] = p.y + ex.y * x + ey.y * y + ez.y * z;
            wv[k * 3 + 2] = p.z + ex.z * x + ey.z * y + ez.z * z;
        }
        if let Some(b) = bbox
            && !(wv[0].max(wv[3].max(wv[6])) >= b[0]
                && wv[0].min(wv[3].min(wv[6])) <= b[2]
                && wv[1].max(wv[4].max(wv[7])) >= b[1]
                && wv[1].min(wv[4].min(wv[7])) <= b[3])
        {
            continue;
        }
        if !soups.contains_key(&key) {
            order.push(key);
        }
        let soup = soups.entry(key).or_default();
        soup.pos.extend_from_slice(&wv);
        soup.surf.push(col_surf[t / 3]);
        if col_ground || soup.ground.is_some() {
            let n = soup.surf.len() - 1;
            soup.ground.get_or_insert_with(|| vec![false; n]).push(col_ground);
        }
        added += 1;
    }
    added
}

/// What the collision build did.
#[derive(Debug, Default)]
pub struct CollisionResult {
    pub objects: Vec<MapObject>,
    pub triangles: i64,
    pub prop_triangles: i64,
    pub interior_triangles: i64,
    pub pieces: i64,
    pub split_cells: i64,
    pub dense_leaves: i64,
    pub up_tris: i64,
    pub down_tris: i64,
    pub cut_degenerate: i64,
    pub cut_beyond_limit: i64,
    pub water_triangles: i64,
    pub hulls: i64,
    pub placed: Vec<(f32, f32)>,
    pub ground: Vec<f32>,
}

/// A marker to put on the ground.
#[derive(Clone, Debug)]
pub struct Query {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub snap: SnapSpec,
}

/// Files written by the collision build, by relative name.
pub type Files = BTreeMap<String, Vec<u8>>;

#[derive(Debug)]
pub struct Collision<'a> {
    pub surf: &'a SurfaceTable,
    pub limit: f32,
    pub off_x: f32,
    pub off_y: f32,
    pub off_z: f32,
    kept: HashMap<(i32, i32), (TileData, usize)>,
    extra: Vec<TileData>,
}

fn p3(v: &[f32], i: u32) -> Vec3 {
    let i = i as usize * 3;
    Vec3::new(v[i], v[i + 1], v[i + 2])
}

const PLACE_STEP: f32 = 4.0;
const PLACE_CELLS: i32 = 30;
const PLACE_REACH: f32 = PLACE_STEP * PLACE_CELLS as f32;

fn place_tiles(qx: f32, qy: f32) -> (i32, i32, i32, i32) {
    let span = PLACE_REACH + SRC_TILE;
    (
        ((qx - span) / SRC_TILE).floor() as i32,
        ((qy - span) / SRC_TILE).floor() as i32,
        ((qx + span) / SRC_TILE).floor() as i32,
        ((qy + span) / SRC_TILE).floor() as i32,
    )
}

impl<'a> Collision<'a> {
    pub fn new(surf: &'a SurfaceTable, off_x: f32, off_y: f32, off_z: f32, limit: f32) -> Self {
        Collision {
            surf,
            limit,
            off_x,
            off_y,
            off_z,
            kept: HashMap::new(),
            extra: Vec::new(),
        }
    }

    fn at(&self, o: Vec3) -> [f32; 12] {
        MapObject::translation(
            o.x + self.off_x,
            o.y + self.off_y,
            if self.off_z != 0.0 { o.z + self.off_z } else { o.z },
        )
    }

    /// Triangle-mesh collision of the world and the props, then the markers on the ground.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        source: &dyn CollisionWorld,
        region: [f64; 4],
        files: &mut Files,
        queries: &[Query],
        props: &BTreeMap<(i32, i32), PropSoup>,
        water: &[WaterQuad],
        snap_extra: &[(Vec<f32>, Vec<u32>, bool)],
    ) -> Result<CollisionResult> {
        let mut res = self.cells(source, region, files, queries, props, "c", false)?;
        self.extra = snap_extra
            .iter()
            .map(|(pos, idx, ground)| TileData {
                v: pos.clone(),
                t: idx.clone(),
                s: vec![0; idx.len() / 3],
                rock: vec![false; idx.len() / 3],
                ground: Some(if *ground { idx.len() / 3 } else { 0 }),
                mask: None,
            })
            .collect();
        let kinds = self.street_kinds();
        for (qi, q) in queries.iter().enumerate() {
            if q.snap.mode == SnapMode::None {
                continue;
            }
            let (near, n_static) = around(q.x, q.y, &self.kept, &self.extra);
            match q.snap.mode {
                SnapMode::Street => {
                    if let Some((px, py, pz)) = place(&near, n_static, water, &kinds, q.x, q.y) {
                        res.ground[qi] = pz;
                        res.placed[qi] = (px, py);
                    }
                }
                SnapMode::Ground => {
                    let n = if q.snap.from_all {
                        near.t.len() / 3
                    } else {
                        n_static
                    };
                    let gz = highest(
                        &near,
                        n,
                        q.x,
                        q.y,
                        if q.z.is_nan() {
                            f32::INFINITY
                        } else {
                            q.z + q.snap.above
                        },
                        if q.snap.min_up.is_nan() {
                            0.7
                        } else {
                            q.snap.min_up
                        },
                    );
                    if !gz.is_nan() {
                        res.ground[qi] = gz;
                    }
                }
                SnapMode::Open => {
                    if let Some((ox, oy, oz)) = open(&near, n_static, q) {
                        res.ground[qi] = oz;
                        res.placed[qi] = (ox, oy);
                    }
                }
                SnapMode::None => {}
            }
        }
        self.kept.clear();
        Ok(res)
    }

    /// Water-mode collision: cells like `build`, pieces in collision mode water.
    pub fn build_water(
        &mut self,
        source: &dyn CollisionWorld,
        region: [f64; 4],
        files: &mut Files,
        props: &BTreeMap<(i32, i32), PropSoup>,
    ) -> Result<CollisionResult> {
        self.cells(source, region, files, &[], props, "w", true)
    }

    /// One convex hull, written whole.
    pub fn add_hull(
        &self,
        res: &mut CollisionResult,
        name: &str,
        pos: &[f32],
        idx: &[u32],
        surface: usize,
        files: &mut Files,
    ) -> Result<()> {
        if idx.is_empty() {
            return Ok(());
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in (0..pos.len().saturating_sub(2)).step_by(3) {
            let p = Vec3::new(pos[v], pos[v + 1], pos[v + 2]);
            mn = mn.min(p);
            mx = mx.max(p);
        }
        let origin = (mn + mx) * 0.5;
        let mut local = vec![0f32; pos.len()];
        for v in (0..pos.len().saturating_sub(2)).step_by(3) {
            local[v] = pos[v] - origin.x;
            local[v + 1] = pos[v + 1] - origin.y;
            local[v + 2] = pos[v + 2] - origin.z;
        }
        let cells = cells_met(
            &local,
            Vec3::new(origin.x + self.off_x, origin.y + self.off_y, 0.0),
        );
        let file = format!("{name}.glb");
        files.insert(
            file.clone(),
            glb::write(
                name,
                &[GlbPrim {
                    blend: None,
                    material: None,
                    pos: local,
                    nrm: None,
                    uv: None,
                    idx: idx.to_vec(),
                }],
            )?,
        );
        let mut ob = MapObject::new(name.to_owned(), self.at(origin));
        ob.cells = cells;
        ob.collision_mesh = Some(format!("meshes/{file}"));
        ob.render = false;
        ob.collision_mode = "hull";
        ob.collision_packed = self.surf.list[surface].1;
        res.objects.push(ob);
        res.hulls += 1;
        res.triangles += (idx.len() / 3) as i64;
        Ok(())
    }

    fn street_kinds(&self) -> [u8; 256] {
        let mut k = [0u8; 256];
        for (i, s) in self.surf.list.iter().enumerate() {
            let slot = SurfaceTable::slot(s.1);
            k[i] = if slot == 2 {
                3
            } else if slot == 1 {
                2
            } else {
                1
            };
        }
        k
    }

    #[allow(clippy::too_many_arguments)]
    fn cells(
        &mut self,
        source: &dyn CollisionWorld,
        region: [f64; 4],
        files: &mut Files,
        queries: &[Query],
        props: &BTreeMap<(i32, i32), PropSoup>,
        prefix: &str,
        water: bool,
    ) -> Result<CollisionResult> {
        let mut res = CollisionResult {
            placed: queries.iter().map(|q| (q.x, q.y)).collect(),
            ground: vec![f32::NAN; queries.len()],
            ..CollisionResult::default()
        };
        self.kept.clear();
        let (limit, off_x, off_y) = (self.limit, self.off_x, self.off_y);
        let tx0 = (region[0] / f64::from(SRC_TILE)).floor() as i32;
        let tx1 = ((region[2] - 1e-6) / f64::from(SRC_TILE)).floor() as i32;
        let ty0 = (region[1] / f64::from(SRC_TILE)).floor() as i32;
        let ty1 = ((region[3] - 1e-6) / f64::from(SRC_TILE)).floor() as i32;
        let mut jobs: BTreeSet<(i32, i32)> = BTreeSet::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                jobs.insert((ty, tx));
            }
        }
        for &(tx, ty) in props.keys() {
            jobs.insert((ty, tx));
        }
        for (tx, ty) in source.cells() {
            jobs.insert((ty, tx));
        }
        let mut wanted: BTreeSet<(i32, i32)> = BTreeSet::new();
        for q in queries {
            let (ax0, ay0, ax1, ay1) = place_tiles(q.x, q.y);
            for ay in ay0..=ay1 {
                for ax in ax0..=ax1 {
                    wanted.insert((ax, ay));
                }
            }
        }
        for (ty, tx) in jobs {
            let mut td = self.clean(source.cell(tx, ty), &mut res);
            if water {
                res.water_triangles += (td.t.len() / 3) as i64;
            }
            let n_static = td.ground.unwrap_or(td.s.len());
            if let Some(soup) = props.get(&(tx, ty))
                && !soup.surf.is_empty()
            {
                let n_all = soup.surf.len();
                let mut kept_tris = Vec::with_capacity(n_all);
                for i in 0..n_all {
                    let p = i * 9;
                    let cx = (soup.pos[p] + soup.pos[p + 3] + soup.pos[p + 6]) / 3.0;
                    let cy = (soup.pos[p + 1] + soup.pos[p + 4] + soup.pos[p + 7]) / 3.0;
                    if (cx + off_x).abs() > limit - 20.0 || (cy + off_y).abs() > limit - 20.0 {
                        continue;
                    }
                    kept_tris.push(i);
                }
                let nt = kept_tris.len();
                if nt > 0 {
                    let v0 = (td.v.len() / 3) as u32;
                    let n0 = td.s.len();
                    let ground0 = td.ground.unwrap_or(n0);
                    let mut mask = None;
                    let mut rock = td.rock.clone();
                    if soup.ground.is_some() {
                        let mut m = vec![false; n0 + nt];
                        let mut r = vec![false; n0 + nt];
                        for k in 0..n0 {
                            m[k] = k < ground0;
                            r[k] = k < td.rock.len() && td.rock[k];
                        }
                        mask = Some(m);
                        rock = r;
                    }
                    for (k, &ti) in kept_tris.iter().enumerate() {
                        td.v.extend_from_slice(&soup.pos[ti * 9..ti * 9 + 9]);
                        let b = v0 + (k * 3) as u32;
                        td.t.extend_from_slice(&[b, b + 1, b + 2]);
                        let raw = soup.surf[ti];
                        td.s.push(raw & 0x7F);
                        if let Some(m) = &mut mask {
                            m[n0 + k] = soup.ground.as_ref().expect("ground")[ti];
                            rock[n0 + k] = raw & 0x80 != 0;
                        }
                    }
                    td.rock = rock;
                    td.mask = mask;
                    td.ground = Some(ground0);
                    res.prop_triangles += nt as i64;
                }
            }
            if td.t.is_empty() {
                continue;
            }
            let all: Vec<usize> = (0..td.t.len() / 3).collect();
            self.split(
                &td,
                &all,
                tx as f32 * SRC_TILE,
                ty as f32 * SRC_TILE,
                SRC_TILE,
                &format!("{prefix}_{tx}_{ty}"),
                files,
                &mut res,
                water,
            )?;
            if wanted.contains(&(tx, ty)) {
                self.kept.insert((tx, ty), (td, n_static));
            }
        }
        Ok(res)
    }

    /// The world triangles of a cell without degenerate ones and those beyond the map edge, with facing counts.
    fn clean(&self, td: Option<TileData>, res: &mut CollisionResult) -> TileData {
        let Some(td) = td.filter(|t| !t.t.is_empty()) else {
            return TileData {
                ground: Some(0),
                ..TileData::default()
            };
        };
        let n = td.t.len() / 3;
        let ground = td.ground.unwrap_or(n);
        let mut tris = Vec::with_capacity(td.t.len());
        let mut surf = Vec::with_capacity(n);
        let mut rock = Vec::with_capacity(ground);
        let mut kept_ground = 0usize;
        let v = &td.v;
        for t in 0..n {
            let (i0, i1, i2) = (td.t[t * 3], td.t[t * 3 + 1], td.t[t * 3 + 2]);
            let cx = (v[i0 as usize * 3] + v[i1 as usize * 3] + v[i2 as usize * 3]) / 3.0;
            let cy = (v[i0 as usize * 3 + 1] + v[i1 as usize * 3 + 1] + v[i2 as usize * 3 + 1]) / 3.0;
            if (cx + self.off_x).abs() > self.limit - 20.0 || (cy + self.off_y).abs() > self.limit - 20.0 {
                res.cut_beyond_limit += 1;
                continue;
            }
            let nn = (p3(v, i1) - p3(v, i0)).cross(p3(v, i2) - p3(v, i0));
            let len = nn.length();
            // not above 1e-9, NaN included
            if len.is_nan() || len <= 1e-9 {
                res.cut_degenerate += 1;
                continue;
            }
            if nn.z > 0.9 * len {
                res.up_tris += 1;
            } else if nn.z < -0.9 * len {
                res.down_tris += 1;
            }
            tris.extend_from_slice(&[i0, i1, i2]);
            surf.push(td.s[t]);
            if t < ground {
                rock.push(t < td.rock.len() && td.rock[t]);
                kept_ground += 1;
            }
        }
        res.interior_triangles += (surf.len() - kept_ground) as i64;
        TileData {
            v: td.v,
            t: tris,
            s: surf,
            rock,
            ground: Some(kept_ground),
            mask: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn split(
        &self,
        td: &TileData,
        tris: &[usize],
        x0: f32,
        y0: f32,
        size: f32,
        name: &str,
        files: &mut Files,
        res: &mut CollisionResult,
        water: bool,
    ) -> Result<()> {
        if tris.is_empty() {
            return Ok(());
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &t in tris {
            for k in 0..3 {
                let p = p3(&td.v, td.t[t * 3 + k]);
                mn = mn.min(p);
                mx = mx.max(p);
            }
        }
        let diag = (mx - mn).length();
        let dense =
            (tris.len() * 3) as f32 > MAX_HALF_EDGES_PER_METRE * diag.max(1.0) || tris.len() > MAX_PIECE_TRIS;
        if dense && size > MIN_CELL {
            res.split_cells += 1;
            let h = size / 2.0;
            let mut q: [Vec<usize>; 4] = Default::default();
            for &t in tris {
                let (a, b, c) = (
                    td.t[t * 3] as usize * 3,
                    td.t[t * 3 + 1] as usize * 3,
                    td.t[t * 3 + 2] as usize * 3,
                );
                let cx = (td.v[a] + td.v[b] + td.v[c]) / 3.0;
                let cy = (td.v[a + 1] + td.v[b + 1] + td.v[c + 1]) / 3.0;
                q[usize::from(cx >= x0 + h) + if cy >= y0 + h { 2 } else { 0 }].push(t);
            }
            for (i, qi) in q.iter().enumerate() {
                self.split(
                    td,
                    qi,
                    x0 + (i & 1) as f32 * h,
                    y0 + (i >> 1) as f32 * h,
                    h,
                    &format!("{name}_{i}"),
                    files,
                    res,
                    water,
                )?;
            }
            return Ok(());
        }
        if dense {
            res.dense_leaves += 1;
        }
        let ns = self.surf.list.len();
        let mut count = vec![0usize; ns];
        if water {
            count[0] = tris.len();
        } else {
            for &t in tris {
                count[td.s[t] as usize] += 1;
            }
        }
        let (mut hard, mut soft): (Option<usize>, Option<usize>) = (None, None);
        let standard = crate::scene::STANDARD_SURFACES.len();
        for s in 0..ns.min(standard) {
            if count[s] == 0 {
                continue;
            }
            if self.surf.soft(s) {
                if soft.is_none_or(|h| count[s] > count[h]) {
                    soft = Some(s);
                }
            } else if hard.is_none_or(|h| count[s] > count[h]) {
                hard = Some(s);
            }
        }
        let minor = 8.max(tris.len() / 20);
        let mut into: Vec<Option<usize>> = (0..ns)
            .map(|s| {
                if count[s] >= minor || s >= standard {
                    Some(s)
                } else if self.surf.soft(s) {
                    soft
                } else {
                    hard
                }
            })
            .collect();
        for (s, i) in into.iter_mut().enumerate().take(standard.min(ns)) {
            if i.is_none() {
                *i = Some(s);
            }
        }
        let origin = Vec3::new(x0 + size / 2.0, y0 + size / 2.0, 0.0);
        for s in 0..ns {
            if count[s] == 0 || into[s] != Some(s) {
                continue;
            }
            // weld by position (1 mm), in triangle order, collapsed triangles dropped
            let corners: Vec<[f32; 3]> = tris
                .iter()
                .filter(|&&t| water || into[td.s[t] as usize] == Some(s))
                .flat_map(|&t| {
                    (0..3).map(move |k| {
                        let vi = td.t[t * 3 + k] as usize * 3;
                        [td.v[vi] - origin.x, td.v[vi + 1] - origin.y, td.v[vi + 2]]
                    })
                })
                .collect();
            let welded = primase_meshops::weld_quantized(&corners, 1000.0)
                .map_err(|e| crate::Error::Scene(format!("collision {name}: {e}")))?;
            if welded.indices.is_empty() {
                continue;
            }
            let pos: Vec<f32> = welded.positions.iter().flatten().copied().collect();
            let idx = welded.indices;
            let stem = if water {
                format!("{name}_water")
            } else {
                format!("{name}_{}", self.surf.list[s].0)
            };
            let chunks: Vec<(Vec<f32>, Vec<u32>)> = if pos.len() / 3 <= MAX_PIECE_VERTS {
                vec![(pos, idx)]
            } else {
                primase_vertex::split_by_vertex_budget(&idx, pos.len() / 3, MAX_PIECE_VERTS)
                    .expect("valid indices")
                    .into_iter()
                    .map(|c| {
                        let p: Vec<f32> = c
                            .vertices
                            .iter()
                            .flat_map(|&v| pos[v as usize * 3..v as usize * 3 + 3].to_vec())
                            .collect();
                        (p, c.indices)
                    })
                    .collect()
            };
            let n = chunks.len();
            for (c, (p, i)) in chunks.into_iter().enumerate() {
                let stem_c = if n == 1 {
                    stem.clone()
                } else {
                    format!("{stem}_{c}")
                };
                let file = format!("{stem_c}.glb");
                let tris = (i.len() / 3) as i64;
                let cells = cells_met(&p, Vec3::new(origin.x + self.off_x, origin.y + self.off_y, 0.0));
                files.insert(
                    file.clone(),
                    glb::write(
                        &stem_c,
                        &[GlbPrim {
                            blend: None,
                            material: None,
                            pos: p,
                            nrm: None,
                            uv: None,
                            idx: i,
                        }],
                    )?,
                );
                let mut ob = MapObject::new(stem_c, self.at(origin));
                ob.cells = cells;
                ob.collision_mesh = Some(format!("meshes/{file}"));
                ob.render = false;
                ob.collision_mode = if water { "water" } else { "triangle_mesh" };
                ob.collision_packed = if water {
                    MapObject::WATER_PACKED
                } else {
                    self.surf.list[s].1
                };
                res.objects.push(ob);
                res.triangles += tris;
                res.pieces += 1;
            }
        }
        Ok(())
    }
}

/// The collision triangles that reach into the search square around a point, as loose triangles: the ground first.
fn around(
    qx: f32,
    qy: f32,
    tiles: &HashMap<(i32, i32), (TileData, usize)>,
    extra: &[TileData],
) -> (TileData, usize) {
    let (ax0, ay0, ax1, ay1) = place_tiles(qx, qy);
    let (mut v, mut s, mut rock) = (Vec::new(), Vec::new(), Vec::new());
    let mut n_static = 0;
    let reach = |a: Vec3, b: Vec3, c: Vec3| {
        !(a.x.max(b.x.max(c.x)) < qx - PLACE_REACH
            || a.x.min(b.x.min(c.x)) > qx + PLACE_REACH
            || a.y.max(b.y.max(c.y)) < qy - PLACE_REACH
            || a.y.min(b.y.min(c.y)) > qy + PLACE_REACH)
    };
    for pass in 0..2 {
        for ay in ay0..=ay1 {
            for ax in ax0..=ax1 {
                let Some((td, ns)) = tiles.get(&(ax, ay)) else {
                    continue;
                };
                let from = if pass == 0 || td.mask.is_some() { 0 } else { ns * 3 };
                let to = if pass == 0 && td.mask.is_none() {
                    ns * 3
                } else {
                    td.t.len()
                };
                for t in (from..to).step_by(3) {
                    if let Some(m) = &td.mask
                        && m[t / 3] != (pass == 0)
                    {
                        continue;
                    }
                    let (a, b, c) = (p3(&td.v, td.t[t]), p3(&td.v, td.t[t + 1]), p3(&td.v, td.t[t + 2]));
                    if !reach(a, b, c) {
                        continue;
                    }
                    v.extend_from_slice(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]);
                    s.push(td.s[t / 3]);
                    if pass == 0 {
                        rock.push(td.rock[t / 3]);
                    }
                }
            }
        }
        for td in extra {
            let g = td.ground.unwrap_or(0);
            let (from, to) = if pass == 0 {
                (0, g * 3)
            } else {
                (g * 3, td.t.len())
            };
            for t in (from..to).step_by(3) {
                let (a, b, c) = (p3(&td.v, td.t[t]), p3(&td.v, td.t[t + 1]), p3(&td.v, td.t[t + 2]));
                if !reach(a, b, c) {
                    continue;
                }
                v.extend_from_slice(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]);
                s.push(0);
                if pass == 0 {
                    rock.push(false);
                }
            }
        }
        if pass == 0 {
            n_static = s.len();
        }
    }
    let t: Vec<u32> = (0..(s.len() * 3) as u32).collect();
    (
        TileData {
            v,
            t,
            s,
            rock,
            ground: Some(n_static),
            mask: None,
        },
        n_static,
    )
}

/// Height of triangle t of `td` at (x, y), NaN when it does not cover the point; with its (unnormalised) normal.
fn height_at(td: &TileData, t: usize, x: f32, y: f32) -> (f32, Vec3) {
    let (a, b, c) = (p3(&td.v, td.t[t]), p3(&td.v, td.t[t + 1]), p3(&td.v, td.t[t + 2]));
    let n = (b - a).cross(c - a);
    let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
    if d.abs() < 1e-9 {
        return (f32::NAN, n);
    }
    let w0 = ((b.y - c.y) * (x - c.x) + (c.x - b.x) * (y - c.y)) / d;
    let w1 = ((c.y - a.y) * (x - c.x) + (a.x - c.x) * (y - c.y)) / d;
    let w2 = 1.0 - w0 - w1;
    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
        return (f32::NAN, n);
    }
    (a.z * w0 + b.z * w1 + c.z * w2, n)
}

fn highest(td: &TileData, n: usize, x: f32, y: f32, zmax: f32, min_up: f32) -> f32 {
    let mut best = f32::NAN;
    for t in (0..n * 3).step_by(3) {
        let (z, nn) = height_at(td, t, x, y);
        if z.is_nan() || nn.z < min_up * nn.length() {
            continue;
        }
        if z <= zmax && (best.is_nan() || z > best) {
            best = z;
        }
    }
    best
}

fn clear(td: &TileData, x: f32, y: f32, z: f32, h: f32) -> bool {
    for t in (0..td.t.len()).step_by(3) {
        let (zz, _) = height_at(td, t, x, y);
        if zz > z + 1e-3 && zz <= z + h {
            return false;
        }
    }
    true
}

fn open(td: &TileData, n_static: usize, q: &Query) -> Option<(f32, f32, f32)> {
    let s = &q.snap;
    let min_up = if s.min_up.is_nan() { 0.8 } else { s.min_up };
    let mut pts = vec![(q.x, q.y)];
    for &r in &s.rings {
        for d in 0..s.directions {
            let a = 2.0 * std::f32::consts::PI * d as f32 / s.directions as f32;
            pts.push((q.x + r * a.cos(), q.y + r * a.sin()));
        }
    }
    for (x, y) in pts {
        let terrain = highest(td, n_static, x, y, f32::INFINITY, 0.0);
        if terrain.is_nan() {
            continue;
        }
        let hit = highest(td, td.t.len() / 3, x, y, terrain + 60.0, min_up);
        if hit.is_nan() || hit - terrain > s.max_above_ground || !clear(td, x, y, hit, s.headroom) {
            continue;
        }
        return Some((x, y, hit));
    }
    let pz = highest(td, n_static, q.x, q.y, f32::INFINITY, 0.0);
    (!pz.is_nan()).then_some((q.x, q.y, pz))
}

/// Where a spawn or bus stop asked for at (qx, qy) goes: onto ground joined to the streets.
fn place(
    td: &TileData,
    n_static: usize,
    water: &[WaterQuad],
    street_kind: &[u8; 256],
    qx: f32,
    qy: f32,
) -> Option<(f32, f32, f32)> {
    const STEP: f32 = PLACE_STEP;
    const MAX_SLOPE: f32 = 0.3;
    const R: i32 = PLACE_CELLS;
    const W: i32 = 2 * R + 1;
    let wn = (W * W) as usize;
    let mut top = vec![f32::NAN; wn];
    let mut low = vec![f32::NAN; wn];
    let mut tri = vec![-1i64; wn];
    for t in (0..td.t.len()).step_by(3) {
        let (a, b, c) = (p3(&td.v, td.t[t]), p3(&td.v, td.t[t + 1]), p3(&td.v, td.t[t + 2]));
        let i0 = (-R).max(((a.x.min(b.x.min(c.x)) - qx) / STEP).ceil() as i32);
        let i1 = R.min(((a.x.max(b.x.max(c.x)) - qx) / STEP).floor() as i32);
        if i0 > i1 {
            continue;
        }
        let j0 = (-R).max(((a.y.min(b.y.min(c.y)) - qy) / STEP).ceil() as i32);
        let j1 = R.min(((a.y.max(b.y.max(c.y)) - qy) / STEP).floor() as i32);
        if j0 > j1 {
            continue;
        }
        let n = (b - a).cross(c - a);
        if n.z.abs() < 0.7 * n.length() {
            continue;
        }
        let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
        if d.abs() < 1e-9 {
            continue;
        }
        let floor = t < n_static * 3 && n.z > 0.0;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let x = qx + i as f32 * STEP;
                let y = qy + j as f32 * STEP;
                let w0 = ((b.y - c.y) * (x - c.x) + (c.x - b.x) * (y - c.y)) / d;
                let w1 = ((c.y - a.y) * (x - c.x) + (a.x - c.x) * (y - c.y)) / d;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = a.z * w0 + b.z * w1 + c.z * w2;
                let k = ((j + R) * W + i + R) as usize;
                if top[k].is_nan() || z > top[k] {
                    top[k] = z;
                    tri[k] = t as i64;
                }
                if floor && (low[k].is_nan() || z < low[k]) {
                    low[k] = z;
                }
            }
        }
    }
    let mut kind = vec![0u8; wn];
    let mut solid = vec![false; wn];
    let mut rock = vec![false; wn];
    for k in 0..wn {
        let t = tri[k];
        if t < 0 {
            continue;
        }
        let t = t as usize;
        let x = qx + ((k as i32 % W) - R) as f32 * STEP;
        let y = qy + ((k as i32 / W) - R) as f32 * STEP;
        if water.iter().any(|q| q.over(x, y, top[k])) {
            top[k] = f32::NAN;
            continue;
        }
        if t >= n_static * 3 {
            continue;
        }
        let a = p3(&td.v, td.t[t]);
        let n = (p3(&td.v, td.t[t + 1]) - a).cross(p3(&td.v, td.t[t + 2]) - a);
        let nl = n.length();
        if n.z < 0.7 * nl {
            continue;
        }
        let sf = td.s[t / 3] as usize;
        kind[k] = if n.z < 0.97 * nl { 1 } else { street_kind[sf] };
        rock[k] = kind[k] == 2 && td.rock[t / 3];
        solid[k] = top[k] - low[k] <= 2.0;
    }
    let mut comp = vec![-1i32; wn];
    let (mut street, mut ground): (Vec<i32>, Vec<i32>) = (Vec::new(), Vec::new());
    let mut queue = vec![0usize; wn];
    for s in 0..wn {
        if kind[s] == 0 || comp[s] >= 0 {
            continue;
        }
        let id = street.len() as i32;
        let (mut head, mut tail, mut n_street, mut n_ground) = (0, 0, 0, 0);
        comp[s] = id;
        queue[tail] = s;
        tail += 1;
        while head < tail {
            let k = queue[head];
            head += 1;
            let (i, j) = (k as i32 % W, k as i32 / W);
            if solid[k] {
                n_ground += 1;
                if kind[k] == 3 {
                    n_street += 1;
                }
            }
            for dj in -1..=1 {
                for di in -1..=1 {
                    let (i2, j2) = (i + di, j + dj);
                    if (di == 0 && dj == 0) || !(0..W).contains(&i2) || !(0..W).contains(&j2) {
                        continue;
                    }
                    let k2 = (j2 * W + i2) as usize;
                    if kind[k2] == 0 || comp[k2] >= 0 {
                        continue;
                    }
                    // the reference's own constant (not sqrt(2) to f32 precision): the result depends on it
                    #[allow(clippy::approx_constant)]
                    let diagonal = 1.4142;
                    let lim = MAX_SLOPE * STEP * if di != 0 && dj != 0 { diagonal } else { 1.0 };
                    if (top[k2] - top[k]).abs() > lim {
                        continue;
                    }
                    comp[k2] = id;
                    queue[tail] = k2;
                    tail += 1;
                }
            }
        }
        street.push(n_street);
        ground.push(n_ground);
    }
    let score = if !street.is_empty() && street.iter().copied().max().unwrap_or(0) >= 40 {
        &street
    } else {
        &ground
    };
    let most = score.iter().copied().max().unwrap_or(0);
    let joined =
        |k: usize| kind[k] != 0 && score[comp[k] as usize] > 0 && score[comp[k] as usize] * 2 >= most;
    let centre = (R * W + R) as usize;
    let mut best: Option<usize> = None;
    if kind[centre] >= 2 && !rock[centre] && solid[centre] && joined(centre) {
        best = Some(centre);
    }
    for pass in 0..3 {
        if best.is_some() {
            break;
        }
        let want = if pass < 2 { 2 } else { 1 };
        let mut best_cost = f32::MAX;
        for k in 0..wn {
            if kind[k] < want || (pass == 0 && rock[k]) || !joined(k) {
                continue;
            }
            let (i, j) = (k as i32 % W - R, k as i32 / W - R);
            let mut cost = (i * i + j * j) as f32 * STEP * STEP;
            if want == 2 && kind[k] != 3 {
                cost += 3600.0;
            }
            if !solid[k] {
                cost += 3600.0;
            }
            let mut clear = i > -R && i < R && j > -R && j < R;
            'outer: for dj in -1..=1i32 {
                if !clear {
                    break;
                }
                for di in -1..=1i32 {
                    if comp[(k as i32 + dj * W + di) as usize] != comp[k] {
                        clear = false;
                        break 'outer;
                    }
                }
            }
            if !clear {
                cost += 400.0;
            }
            if cost < best_cost {
                best_cost = cost;
                best = Some(k);
            }
        }
    }
    if best.is_none() && !top[centre].is_nan() {
        best = Some(centre);
    }
    let b = best?;
    Some((
        qx + ((b as i32 % W) - R) as f32 * STEP,
        qy + ((b as i32 / W) - R) as f32 * STEP,
        top[b],
    ))
}
