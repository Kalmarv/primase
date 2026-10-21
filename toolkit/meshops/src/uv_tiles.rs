//! `split-uv-tiles{set=N}` (required): every triangle primitive whose `TexCoord(N)` leaves 0-1 is cut at the integer
//! lines of the texture plane and each piece moved into 0-1, for a destination that has no tiling (a texture atlas).
//!
//! **Which primitives.** Those with a `TexCoord(N)` stream and some drawn vertex outside 0-1 on u or v, and a material
//! (or a per-placement variant of it) with a texture slot sampling set N; any other primitive is left as it is and
//! counted (no material, or no texture samples the set: its coordinates change nothing that is drawn). A primitive
//! that is not a triangle list is refused, naming it. A slot sampling set N must be `Repeat` on each axis the primitive
//! leaves 0-1 on (`Clamp` and `Mirror` tile differently: refused, naming the material, slot and axis) and carry no
//! transform (`bake-texture-transforms` first): a transform would turn the integer shift into a fractional one.
//!
//! **Cutting.** Each triangle is clipped (Sutherland-Hodgman) at every integer line `u = k` and `v = k` it strictly
//! crosses, u first, ascending; the convex pieces are fanned back into triangles in the polygon's order, so winding
//! is kept, and each piece is translated by the integer offset of the tile it lies in, which puts it in 0-1. A triangle
//! already in 0-1 on both axes is kept as it is, with its vertices. Textures sampled with `Repeat` look identical before
//! and after (a piece shifted by an integer samples the same texels), to the rounding of the f32 interpolation: the
//! cut coordinate itself is exact. A triangle crossing more than [`MAX_TILES_PER_TRIANGLE`] tiles is refused, naming it.
//!
//! **Vertices.** A new vertex lies in one source triangle, so it is a combination of that triangle's corners
//! ([`primase_core::VertexBlend`]). Every stream is interpolated linearly with those weights: positions, other
//! texture-coordinate sets and colours as they are (8- and 16-bit colours rounded), normals and the xyz of tangents
//! renormalised (a tangent's w is kept: the pass is refused when the corners it combines differ in w; blended
//! normals that cancel keep the nearest corner's, counted as an approximation). A vertex shared by pieces of the same
//! tile (the cut of an edge two triangles share is computed from the edge alone, so both get the same vertex) is one
//! vertex; a corner kept with its own coordinates is the source vertex. Vertices are numbered in first use, which
//! drops source vertices nothing uses any more. Morph targets follow through [`primase_core::World::blend_vertices`]
//! (their deltas interpolate the same way); any other per-vertex component of the entity that cannot combine
//! vertices refuses the pass, naming it, before anything changes.
//!
//! **Skins.** A new vertex's influences are the union of its corners' joints with the weights interpolated, kept as
//! [`primase_core::Influences::Streams`] with as many sets as the union needs (none dropped: `limit-skin-influences`
//! does that, reported there), the weights as `F32x4` and the joints as the source's type; a primitive bound by
//! [`primase_core::Influences::SingleIndex`] stays so unless a new vertex joins two different slots. A new vertex's
//! deformation is the interpolation of weights, not of skinned positions, so it differs from the skinned surface
//! between its corners unless they carry the same influences: counted as an approximation, per vertex whose corners
//! differ.
//!
//! Reported: primitives split, triangles and vertices before and after (of the split primitives), vertices
//! interpolated, primitives left and why. The file link of a changed mesh goes.

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, UvTransform, Wrap};
use primase_core::skin::Skin;
use primase_core::{
    Entity, Influences, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Topology,
    VertexBlend, World,
};

/// The most tiles one triangle may cross; a triangle crossing more is refused (its pieces would number about that).
pub const MAX_TILES_PER_TRIANGLE: usize = 1024;

/// See the module documentation. Made by [`SplitUvTiles::new`]; from options by [`SplitUvTiles::from_options`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitUvTiles {
    set: u8,
}

impl SplitUvTiles {
    /// Split the coordinates of `TexCoord(set)`.
    pub fn new(set: u8) -> SplitUvTiles {
        SplitUvTiles { set }
    }

    pub fn set(&self) -> u8 {
        self.set
    }

    /// From its options: `set`, required (the texture-coordinate set, a number).
    pub fn from_options(o: &Options) -> Result<SplitUvTiles, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "set") {
            return Err(format!("unknown option {k}"));
        }
        let v = o
            .get("set")
            .ok_or("split-uv-tiles needs set=<texture coordinate set>")?;
        Ok(SplitUvTiles::new(v.parse().map_err(|_| {
            format!("set={v} is not a texture coordinate set")
        })?))
    }
}

/// [`SplitUvTiles::from_options`] as a pass factory.
pub fn split_uv_tiles(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SplitUvTiles::from_options(o)?))
}

/// A point of a source triangle as a combination of its corners, in a canonical form (sources ascending, none with
/// weight 0 but padding) so equal points are equal values: the weld key, and what makes the cut of a shared edge the
/// same from both sides.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bary {
    src: [u32; 3],
    w: [f32; 3],
}

impl Bary {
    fn corner(v: u32) -> Bary {
        Bary {
            src: [v; 3],
            w: [1.0, 0.0, 0.0],
        }
    }

    fn key(&self) -> Key {
        (self.src, self.w.map(f32::to_bits))
    }

    fn blend(&self) -> VertexBlend {
        VertexBlend {
            sources: self.src,
            weights: self.w,
        }
    }

    /// `(1 - t) a + t b`, canonical.
    fn lerp(a: &Bary, b: &Bary, t: f64) -> Bary {
        let mut terms: Vec<(u32, f64)> = Vec::with_capacity(6);
        terms.extend(a.blend().terms().map(|(s, w)| (s, (1.0 - t) * f64::from(w))));
        terms.extend(b.blend().terms().map(|(s, w)| (s, t * f64::from(w))));
        terms.sort_by_key(|&(s, _)| s);
        let mut merged: Vec<(u32, f64)> = Vec::with_capacity(3);
        for (s, w) in terms {
            match merged.last_mut() {
                Some(last) if last.0 == s => last.1 += w,
                _ => merged.push((s, w)),
            }
        }
        merged.retain(|&(_, w)| w > 0.0);
        debug_assert!(
            merged.len() <= 3,
            "a point of a triangle combines at most its three corners"
        );
        let first = merged.first().map_or(0, |&(s, _)| s);
        let mut out = Bary {
            src: [first; 3],
            w: [0.0; 3],
        };
        for (k, (s, w)) in merged.into_iter().take(3).enumerate() {
            out.src[k] = s;
            out.w[k] = w as f32;
        }
        out
    }
}

/// A vertex of a polygon being clipped: its point and its coordinates (exact on the line it was cut at).
#[derive(Clone, Copy, Debug)]
struct Corner {
    at: Bary,
    uv: [f64; 2],
}

impl Corner {
    /// The point where edge `a`-`b` crosses `uv[axis] = k`, computed from the edge's ends in a fixed order so both
    /// triangles sharing the edge get the same point.
    fn cut(a: &Corner, b: &Corner, axis: usize, k: f64) -> Corner {
        let (a, b) = if a.at.key() <= b.at.key() { (a, b) } else { (b, a) };
        let t = ((k - a.uv[axis]) / (b.uv[axis] - a.uv[axis])).clamp(0.0, 1.0);
        let other = 1 - axis;
        let mut uv = [0.0; 2];
        uv[axis] = k;
        uv[other] = (1.0 - t) * a.uv[other] + t * b.uv[other];
        Corner {
            at: Bary::lerp(&a.at, &b.at, t),
            uv,
        }
    }

    fn same(&self, other: &Corner) -> bool {
        self.at.key() == other.at.key()
    }
}

/// `poly` (convex, in order) cut at `uv[axis] = k` into the part at or below and the part at or above, each without
/// repeated neighbours and `None` when it has no area to speak of (fewer than three distinct corners).
fn split_at(poly: &[Corner], axis: usize, k: f64) -> (Option<Vec<Corner>>, Option<Vec<Corner>>) {
    let (mut low, mut high) = (Vec::new(), Vec::new());
    for (i, cur) in poly.iter().enumerate() {
        let next = &poly[(i + 1) % poly.len()];
        let (c, d) = (cur.uv[axis], next.uv[axis]);
        if c <= k {
            low.push(*cur);
        }
        if c >= k {
            high.push(*cur);
        }
        if (c < k && d > k) || (c > k && d < k) {
            let cut = Corner::cut(cur, next, axis, k);
            low.push(cut);
            high.push(cut);
        }
    }
    let tidy = |mut p: Vec<Corner>| {
        p.dedup_by(|a, b| a.same(b));
        while p.len() > 1 && p[0].same(&p[p.len() - 1]) {
            p.pop();
        }
        (p.len() >= 3).then_some(p)
    };
    (tidy(low), tidy(high))
}

/// How many integer lines lie strictly inside `(lo, hi)`, and the first tile index: a range inside 0-1 is tile 0.
fn lines_in(lo: f64, hi: f64) -> (i64, i64) {
    if lo >= 0.0 && hi <= 1.0 {
        return (0, 0);
    }
    let first = lo.floor() as i64;
    (first, ((hi.ceil() as i64) - 1 - first).max(0))
}

/// `poly` cut at the integer lines of `axis` it crosses, with the tile index each piece lies in on that axis.
fn pieces(poly: Vec<Corner>, axis: usize) -> Vec<(i64, Vec<Corner>)> {
    let lo = poly.iter().map(|c| c.uv[axis]).fold(f64::INFINITY, f64::min);
    let hi = poly.iter().map(|c| c.uv[axis]).fold(f64::NEG_INFINITY, f64::max);
    let (first, lines) = lines_in(lo, hi);
    let mut out = Vec::new();
    let mut rest = Some(poly);
    let mut tile = first;
    for n in 1..=lines {
        let k = (first + n) as f64;
        let Some(p) = rest.take() else { break };
        let (low, high) = split_at(&p, axis, k);
        out.extend(low.map(|p| (tile, p)));
        rest = high;
        tile = first + n;
    }
    out.extend(rest.map(|p| (tile, p)));
    out
}

/// A point's canonical form: its sources and the bits of its weights.
type Key = ([u32; 3], [u32; 3]);

/// Output vertices in first-use order, one per point and tile.
#[derive(Default)]
struct Vertices {
    index: BTreeMap<(Key, i64, i64), u32>,
    points: Vec<(Bary, [f32; 2])>,
}

impl Vertices {
    fn get(&mut self, c: &Corner, tile: [i64; 2]) -> u32 {
        let key = (c.at.key(), tile[0], tile[1]);
        let points = &mut self.points;
        *self.index.entry(key).or_insert_with(|| {
            let uv = [0, 1].map(|a| (c.uv[a] - tile[a] as f64).clamp(0.0, 1.0) as f32);
            points.push((c.at, uv));
            (points.len() - 1) as u32
        })
    }
}

/// What splitting one primitive made.
struct Split {
    primitive: Primitive,
    blends: Vec<VertexBlend>,
    influences: Option<Influences>,
    triangles_before: u64,
    vertices_before: u64,
    interpolated: u64,
    skin_interpolated: u64,
    normals_cancelled: u64,
}

fn in_unit(uv: [f32; 2]) -> bool {
    uv.iter().all(|c| (0.0..=1.0).contains(c))
}

/// Whether the primitive's drawn vertices leave 0-1 on u and on v.
fn leaves_unit(uv: &[[f32; 2]], corners: impl Iterator<Item = u32>, what: &str) -> Result<[bool; 2], String> {
    let mut leaves = [false; 2];
    for i in corners {
        let c = uv
            .get(i as usize)
            .ok_or_else(|| format!("{what}: index {i} outside its vertices"))?;
        if !c.iter().all(|x| x.is_finite()) {
            return Err(format!("{what}: a texture coordinate is not finite"));
        }
        for a in 0..2 {
            leaves[a] |= !(0.0..=1.0).contains(&c[a]);
        }
    }
    Ok(leaves)
}

fn split_primitive(
    p: &Primitive,
    set: u8,
    influences: Option<Influences>,
    what: &str,
) -> Result<Split, String> {
    let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(set)) else {
        return Err(format!("{what}: TexCoord({set}) is not two floats"));
    };
    let n = p.vertex_count();
    let indices: Vec<u32> = p.indices.clone().unwrap_or_else(|| (0..n as u32).collect());
    let mut out = Vertices::default();
    let mut tris: Vec<u32> = Vec::new();
    for (t, tri) in indices.as_chunks::<3>().0.iter().enumerate() {
        let corner = |v: u32| Corner {
            at: Bary::corner(v),
            uv: uv[v as usize].map(f64::from),
        };
        let cs = tri.map(corner);
        if tri.iter().all(|&v| in_unit(uv[v as usize])) {
            tris.extend(cs.iter().map(|c| out.get(c, [0, 0])));
            continue;
        }
        let range = |a: usize| {
            let lo = cs.iter().map(|c| c.uv[a]).fold(f64::INFINITY, f64::min);
            let hi = cs.iter().map(|c| c.uv[a]).fold(f64::NEG_INFINITY, f64::max);
            lines_in(lo, hi).1 as usize + 1
        };
        let tiles = range(0).saturating_mul(range(1));
        if tiles > MAX_TILES_PER_TRIANGLE {
            return Err(format!(
                "{what}: triangle {t} crosses {tiles} tiles (at most {MAX_TILES_PER_TRIANGLE})"
            ));
        }
        for (tu, along_u) in pieces(cs.to_vec(), 0) {
            for (tv, piece) in pieces(along_u, 1) {
                let ids: Vec<u32> = piece.iter().map(|c| out.get(c, [tu, tv])).collect();
                for k in 1..ids.len() - 1 {
                    tris.extend([ids[0], ids[k], ids[k + 1]]);
                }
            }
        }
    }
    let blends: Vec<VertexBlend> = out.points.iter().map(|(b, _)| b.blend()).collect();
    let interpolated = blends.iter().filter(|b| b.terms().count() > 1).count() as u64;
    let mut streams = BTreeMap::new();
    let mut normals_cancelled = 0;
    for (sem, stream) in &p.streams {
        let made = match sem {
            Semantic::Joints(_) | Semantic::Weights(_) => continue,
            Semantic::TexCoord(s) if *s == set => {
                Stream::F32x2(out.points.iter().map(|(_, uv)| *uv).collect())
            }
            _ => blend_stream(*sem, stream, &blends, &mut normals_cancelled, what)?,
        };
        streams.insert(*sem, made);
    }
    let (skin, influences, skin_interpolated) = blend_skin(p, influences, &blends, what)?;
    streams.extend(skin);
    Ok(Split {
        primitive: Primitive {
            topology: p.topology,
            streams,
            indices: Some(tris),
            material: p.material,
        },
        blends,
        influences,
        triangles_before: (indices.len() / 3) as u64,
        vertices_before: n as u64,
        interpolated,
        skin_interpolated,
        normals_cancelled,
    })
}

/// `v[..3]` as a unit vector; `None` when it is too short to have a direction.
fn unit3(v: &[f64]) -> Option<[f64; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (len > 1e-12).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// One stream (not joints or weights) of the new vertices.
fn blend_stream(
    sem: Semantic,
    stream: &Stream,
    blends: &[VertexBlend],
    cancelled: &mut u64,
    what: &str,
) -> Result<Stream, String> {
    macro_rules! floats {
        ($v:expr, $n:literal, $kind:ident) => {{
            let mut out: Vec<[f32; $n]> = Vec::with_capacity(blends.len());
            for b in blends {
                let mut terms = b.terms();
                let (first, w0) = terms
                    .next()
                    .ok_or_else(|| format!("{what}: a vertex combines nothing"))?;
                let mut sum = $v[first as usize].map(|x| f64::from(x) * f64::from(w0));
                let mut single = true;
                for (s, w) in terms {
                    single = false;
                    for (acc, x) in sum.iter_mut().zip($v[s as usize]) {
                        *acc += f64::from(x) * f64::from(w);
                    }
                }
                let mut value = if single {
                    $v[first as usize]
                } else {
                    sum.map(|x| x as f32)
                };
                if !single && matches!(sem, Semantic::Normal | Semantic::Tangent) {
                    if let (Some(xyz), Some(dst)) = (sum.get(..3), value.get_mut(..3)) {
                        match unit3(xyz) {
                            Some(u) => dst.iter_mut().zip(u).for_each(|(d, u)| *d = u as f32),
                            None => {
                                // opposite normals: the corner with the most weight keeps its own
                                let nearest = b
                                    .terms()
                                    .fold((first, -1.0f32), |m, (s, w)| if w > m.1 { (s, w) } else { m })
                                    .0;
                                if let Some(own) = $v[nearest as usize].get(..3) {
                                    dst.copy_from_slice(own);
                                }
                                *cancelled += 1;
                            }
                        }
                    }
                }
                out.push(value);
            }
            Stream::$kind(out)
        }};
    }
    if sem == Semantic::Tangent
        && let Stream::F32x4(v) = stream
    {
        for b in blends {
            let mut ws = b.terms().map(|(s, _)| v[s as usize][3].to_bits());
            let first = ws.next();
            if ws.any(|w| Some(w) != first) {
                return Err(format!(
                    "{what}: the tangents of a triangle's corners differ in w (handedness), so a tangent between them has none"
                ));
            }
        }
    }
    Ok(match stream {
        Stream::F32x2(v) => floats!(v, 2, F32x2),
        Stream::F32x3(v) => floats!(v, 3, F32x3),
        Stream::F32x4(v) => {
            let mut s = floats!(v, 4, F32x4);
            if sem == Semantic::Tangent
                && let Stream::F32x4(o) = &mut s
            {
                for (o, b) in o.iter_mut().zip(blends) {
                    if let Some((src, _)) = b.terms().next() {
                        o[3] = v[src as usize][3];
                    }
                }
            }
            s
        }
        Stream::U8x4(v) => Stream::U8x4(
            blends
                .iter()
                .map(|b| round_mix(b, |s| v[s as usize].map(f64::from), 255.0).map(|x| x as u8))
                .collect(),
        ),
        Stream::U16x4(v) => Stream::U16x4(
            blends
                .iter()
                .map(|b| round_mix(b, |s| v[s as usize].map(f64::from), 65535.0).map(|x| x as u16))
                .collect(),
        ),
    })
}

/// The blend of integer 4-vectors, rounded to nearest and kept within `0..=max`.
fn round_mix(b: &VertexBlend, at: impl Fn(u32) -> [f64; 4], max: f64) -> [f64; 4] {
    let mut sum = [0.0; 4];
    for (s, w) in b.terms() {
        for (acc, x) in sum.iter_mut().zip(at(s)) {
            *acc += x * f64::from(w);
        }
    }
    sum.map(|x| x.round().clamp(0.0, max))
}

/// A vertex's influences: slot and weight, strongest first.
type Influence = Vec<(u32, f32)>;

/// The joint and weight streams of the new vertices, the primitive's new influences and how many vertices have
/// interpolated weights that differ from their corners'.
#[allow(clippy::type_complexity)]
fn blend_skin(
    p: &Primitive,
    influences: Option<Influences>,
    blends: &[VertexBlend],
    what: &str,
) -> Result<(Vec<(Semantic, Stream)>, Option<Influences>, u64), String> {
    let has_streams = p
        .streams
        .keys()
        .any(|s| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)));
    let sets = match influences {
        Some(Influences::Streams { sets }) => usize::from(sets),
        Some(Influences::SingleIndex) => 1,
        _ if has_streams => {
            return Err(format!(
                "{what}: joint or weight streams without influences naming them"
            ));
        }
        _ => return Ok((Vec::new(), influences, 0)),
    };
    let single_index = influences == Some(Influences::SingleIndex);
    let mut joints: Vec<Vec<[u32; 4]>> = Vec::new();
    let mut weights: Vec<Vec<[f32; 4]>> = Vec::new();
    let mut wide = false;
    for k in 0..sets as u8 {
        match p.streams.get(&Semantic::Joints(k)) {
            Some(Stream::U8x4(v)) => joints.push(v.iter().map(|j| j.map(u32::from)).collect()),
            Some(Stream::U16x4(v)) => {
                wide = true;
                joints.push(v.iter().map(|j| j.map(u32::from)).collect());
            }
            _ => return Err(format!("{what}: Joints({k}) is not an integer stream")),
        }
        if single_index {
            continue;
        }
        weights.push(match p.streams.get(&Semantic::Weights(k)) {
            Some(Stream::F32x4(v)) => v.clone(),
            Some(Stream::U8x4(v)) => v.iter().map(|w| w.map(|x| f32::from(x) / 255.0)).collect(),
            Some(Stream::U16x4(v)) => v.iter().map(|w| w.map(|x| f32::from(x) / 65535.0)).collect(),
            _ => return Err(format!("{what}: Weights({k}) is not a four-value stream")),
        });
    }
    // a source vertex's influences as stored (every lane, in order), and those with a weight, strongest first
    let raw = |v: u32| -> Influence {
        if single_index {
            return vec![(joints[0][v as usize][0], 1.0)];
        }
        let mut lanes = Vec::with_capacity(sets * 4);
        for (js, ws) in joints.iter().zip(&weights) {
            lanes.extend(js[v as usize].into_iter().zip(ws[v as usize]));
        }
        lanes
    };
    let strongest = |v: u32| -> Influence {
        let mut all: Influence = raw(v).into_iter().filter(|&(_, w)| w > 0.0).collect();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        all
    };
    let mut per_vertex: Vec<Influence> = Vec::with_capacity(blends.len());
    let mut differing = 0;
    for b in blends {
        let terms: Vec<(u32, f32)> = b.terms().collect();
        // a copy of one vertex, or a point whose corners all carry the same influences, keeps them as stored
        let first = strongest(terms[0].0);
        if terms[1..].iter().all(|&(s, _)| strongest(s) == first) {
            per_vertex.push(raw(terms[0].0));
            continue;
        }
        differing += 1;
        let mut mix: BTreeMap<u32, f64> = BTreeMap::new();
        for &(s, w) in &terms {
            for (slot, weight) in strongest(s) {
                *mix.entry(slot).or_default() += f64::from(w) * f64::from(weight);
            }
        }
        let mut mixed: Influence = mix.into_iter().map(|(s, w)| (s, w as f32)).collect();
        mixed.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        per_vertex.push(mixed);
    }
    let needed = per_vertex
        .iter()
        .map(|i| i.len().div_ceil(4))
        .max()
        .unwrap_or(1)
        .max(1);
    let stays_single = single_index && per_vertex.iter().all(|i| i.len() <= 1);
    if stays_single {
        let slots = per_vertex.iter().map(|i| i.first().map_or(0, |&(s, _)| s));
        let stream = if wide {
            Stream::U16x4(slots.map(|s| [s as u16, 0, 0, 0]).collect())
        } else {
            Stream::U8x4(slots.map(|s| [s as u8, 0, 0, 0]).collect())
        };
        return Ok((vec![(Semantic::Joints(0), stream)], influences, differing));
    }
    let new_sets = needed.max(if single_index { 1 } else { sets });
    let new_sets_u8 = u8::try_from(new_sets).map_err(|_| format!("{what}: more influence sets than fit"))?;
    let mut out = Vec::new();
    for k in 0..new_sets {
        let lane = |v: &Influence, j: usize| -> (u32, f32) {
            v.get(k * 4 + j)
                .copied()
                .unwrap_or((v.first().map_or(0, |&(s, _)| s), 0.0))
        };
        let js: Vec<[u32; 4]> = per_vertex
            .iter()
            .map(|v| std::array::from_fn(|j| lane(v, j).0))
            .collect();
        let ws: Vec<[f32; 4]> = per_vertex
            .iter()
            .map(|v| std::array::from_fn(|j| lane(v, j).1))
            .collect();
        let joints = if wide {
            Stream::U16x4(js.iter().map(|j| j.map(|s| s as u16)).collect())
        } else {
            Stream::U8x4(js.iter().map(|j| j.map(|s| s as u8)).collect())
        };
        out.push((Semantic::Joints(k as u8), joints));
        out.push((Semantic::Weights(k as u8), Stream::F32x4(ws)));
    }
    Ok((out, Some(Influences::Streams { sets: new_sets_u8 }), differing))
}

/// What a primitive is: its entity (by name when it has one) and index.
fn describe(w: &World, e: Entity, i: usize) -> String {
    match w.get::<Name>(e) {
        Some(n) => format!("{e:?} {:?} primitive {i}", n.0),
        None => format!("{e:?} primitive {i}"),
    }
}

/// Whether a primitive is split: its material checks, or why it is left.
enum Verdict {
    Split,
    Left(&'static str),
}

/// The materials `e`'s primitive `i` is drawn with (its own and its variants').
fn drawn_with(w: &World, e: Entity, i: usize, p: &Primitive) -> Vec<Entity> {
    let mut ms: Vec<Entity> = p.material.into_iter().collect();
    if let Some(v) = w.get::<MaterialVariants>(e).and_then(|v| v.0.get(i)) {
        ms.extend(v.values().copied());
    }
    ms.sort();
    ms.dedup();
    ms
}

/// Whether the textures sampling `set` allow cutting a primitive that leaves 0-1 on `leaves`, naming what does not.
fn check_materials(
    w: &World,
    ms: &[Entity],
    set: u8,
    leaves: [bool; 2],
    what: &str,
) -> Result<Verdict, String> {
    if ms.is_empty() {
        return Ok(Verdict::Left("primitives left as they are: no material"));
    }
    let mut sampled = false;
    for &m in ms {
        let mat = w.get::<PbrMaterial>(m).ok_or_else(|| {
            format!(
                "{what}: material {m:?} has no pbr.material, so how it samples TexCoord({set}) is not known"
            )
        })?;
        for (slot, r) in mat.slots().filter(|(_, r)| r.texcoord == set) {
            sampled = true;
            if r.transform.is_some_and(|t| t != UvTransform::IDENTITY) {
                return Err(format!(
                    "{what}: material {m:?} slot {} has a texture transform (bake-texture-transforms first)",
                    slot.name()
                ));
            }
            for (a, axis) in ["u", "v"].into_iter().enumerate() {
                if leaves[a] && r.wrap[a] != Wrap::Repeat {
                    return Err(format!(
                        "{what}: material {m:?} slot {} is {:?} on {axis} but the coordinates leave 0-1 there \
                         (only Repeat tiles as a shifted piece does)",
                        slot.name(),
                        r.wrap[a]
                    ));
                }
            }
        }
    }
    Ok(if sampled {
        Verdict::Split
    } else {
        Verdict::Left("primitives left as they are: no texture samples the set")
    })
}

struct Planned {
    entity: Entity,
    primitive: usize,
    split: Split,
}

impl Pass for SplitUvTiles {
    fn name(&self) -> &'static str {
        "split-uv-tiles"
    }
    fn options(&self) -> Options {
        Options::from([("set".to_owned(), self.set.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &["core.mesh", "pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.morphs", "core.skin"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    /// The file links it removes are a side effect: the changed meshes no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "skin weights interpolated at new vertices (not the skinned positions); normals that cancel keep the nearest \
         corner's"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let set = self.set;
        if let Some(e) = w.iter::<MeshFile>().map(|(e, _)| e).find(|&e| !w.has::<Mesh>(e)) {
            return Err(format!("{e:?}: geometry still in its file (load it first)"));
        }
        // everything is planned and checked before anything changes
        let mut plan: Vec<Planned> = Vec::new();
        for (e, mesh) in w.iter::<Mesh>() {
            let skin = w.get::<Skin>(e);
            for (i, p) in mesh.primitives().iter().enumerate() {
                let Some(uv) = p.streams.get(&Semantic::TexCoord(set)) else {
                    continue;
                };
                let what = describe(w, e, i);
                let Stream::F32x2(uv) = uv else {
                    return Err(format!("{what}: TexCoord({set}) is not two floats"));
                };
                let drawn: Vec<u32> = match &p.indices {
                    Some(ix) => ix.clone(),
                    None => (0..uv.len() as u32).collect(),
                };
                let leaves = leaves_unit(uv, drawn.iter().copied(), &what)?;
                if !leaves.iter().any(|&l| l) {
                    continue;
                }
                if p.topology != Topology::Triangles {
                    return Err(format!(
                        "{what}: {:?}, not triangles, with TexCoord({set}) outside 0-1",
                        p.topology
                    ));
                }
                match check_materials(w, &drawn_with(w, e, i, p), set, leaves, &what)? {
                    Verdict::Left(why) => {
                        log.count(why, 1);
                        continue;
                    }
                    Verdict::Split => {}
                }
                let influences = skin.and_then(|s| s.influences.get(i).copied().flatten());
                let split = split_primitive(p, set, influences, &what)?;
                w.check_blend_vertices(e, i, &split.blends)?;
                plan.push(Planned {
                    entity: e,
                    primitive: i,
                    split,
                });
            }
        }
        let mut by_entity: BTreeMap<Entity, Vec<Planned>> = BTreeMap::new();
        for pl in plan {
            by_entity.entry(pl.entity).or_default().push(pl);
        }
        for (e, planned) in by_entity {
            for pl in &planned {
                w.blend_vertices(e, pl.primitive, &pl.split.blends)?;
            }
            w.edit_mesh(e, |mesh| {
                for pl in &planned {
                    let mut target = mesh
                        .primitive_mut(pl.primitive)
                        .ok_or_else(|| format!("{e:?}: primitive {} vanished", pl.primitive))?;
                    let fields = target.fields();
                    *fields.streams = pl.split.primitive.streams.clone();
                    *fields.indices = pl.split.primitive.indices.clone();
                }
                Ok(None)
            })?;
            if let Some(skin) = w.get_mut::<Skin>(e) {
                for pl in &planned {
                    if let Some(slot) = skin.influences.get_mut(pl.primitive) {
                        *slot = pl.split.influences;
                    }
                }
            }
            w.remove::<MeshFile>(e);
            for pl in planned {
                let s = pl.split;
                let after = s.primitive.vertex_count() as u64;
                log.count("primitives split", 1);
                log.count("triangles before", s.triangles_before);
                log.count(
                    "triangles after",
                    s.primitive.indices.as_ref().map_or(0, |i| i.len() as u64 / 3),
                );
                log.count("vertices before", s.vertices_before);
                log.count("vertices after", after);
                log.count("vertices interpolated at tile lines", s.interpolated);
                log.approximate("skinned vertices with interpolated weights", s.skin_interpolated);
                log.approximate(
                    "blended normals that cancel, the nearest corner's kept",
                    s.normals_cancelled,
                );
            }
        }
        Ok(())
    }
}

pub(crate) fn register(reg: &mut primase_core::pass::PassRegistry) {
    // which coordinate set is cut is the program's: no default
    reg.register_requiring("split-uv-tiles", split_uv_tiles, &[("set", "0")]);
}
