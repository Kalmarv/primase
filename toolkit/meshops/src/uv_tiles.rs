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

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, UvTransform, Wrap};
use primase_core::skin::Skin;
use primase_core::{
    Entity, Influences, Mesh, MeshFile, Primitive, Semantic, Stream, Topology, VertexBlend, World,
};

use crate::point_blend::{
    Bary, Corner, Key, Replacement, blend_streams, describe, drawn_with, replace_primitives, tidy,
};

/// The most tiles one triangle may cross; a triangle crossing more is refused (its pieces would number about that).
pub const MAX_TILES_PER_TRIANGLE: usize = 1024;

/// See the module documentation. Made by [`SplitUvTiles::new`]; from options by [`SPLIT_UV_TILES`].
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

    /// From its declared options.
    fn from_given(g: &Given) -> Result<SplitUvTiles, String> {
        let v = g.required("set");
        Ok(SplitUvTiles::new(v.parse().map_err(|_| {
            format!("set={v} is not a texture coordinate set")
        })?))
    }
}

/// `split-uv-tiles`' declaration ([`primase_core::pass::PassRegistry::add`]): which coordinate set is cut is the
/// program's, no default.
pub const SPLIT_UV_TILES: PassSpec = PassSpec {
    name: "split-uv-tiles",
    options: &[OptionSpec::required(
        "set",
        "0",
        "the texture-coordinate set whose tiles are cut apart, a number",
    )],
    make: |g| Ok(Box::new(SplitUvTiles::from_given(g)?)),
};

/// The point where edge `a`-`b` crosses `uv[axis] = k`, its coordinate on the line exact.
fn cut_at_line(a: &Corner, b: &Corner, axis: usize, k: f64) -> Corner {
    let mut c = Corner::cut_at(a, b, |a, b| (k - a.uv[axis]) / (b.uv[axis] - a.uv[axis]));
    c.uv[axis] = k;
    c
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
            let cut = cut_at_line(cur, next, axis, k);
            low.push(cut);
            high.push(cut);
        }
    }
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
    streams: BTreeMap<Semantic, Stream>,
    indices: Vec<u32>,
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
    let blended = blend_streams(p, &blends, influences, what)?;
    let mut streams = blended.streams;
    streams.insert(
        Semantic::TexCoord(set),
        Stream::F32x2(out.points.iter().map(|(_, uv)| *uv).collect()),
    );
    Ok(Split {
        streams,
        indices: tris,
        blends,
        influences: blended.influences,
        triangles_before: (indices.len() / 3) as u64,
        vertices_before: n as u64,
        interpolated,
        skin_interpolated: blended.skin_interpolated,
        normals_cancelled: blended.normals_cancelled,
    })
}

/// Whether a primitive is split: its material checks, or why it is left.
enum Verdict {
    Split,
    Left(&'static str),
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
        let (mut replacements, mut reports) = (Vec::new(), Vec::new());
        for pl in plan {
            let s = pl.split;
            replacements.push(Replacement {
                entity: pl.entity,
                primitive: pl.primitive,
                streams: s.streams,
                indices: s.indices,
                blends: s.blends,
                influences: s.influences,
            });
            reports.push((
                s.triangles_before,
                s.vertices_before,
                s.interpolated,
                s.skin_interpolated,
                s.normals_cancelled,
            ));
        }
        let after: Vec<(u64, u64)> = replacements
            .iter()
            .map(|r| (r.indices.len() as u64 / 3, r.blends.len() as u64))
            .collect();
        replace_primitives(w, replacements)?;
        for ((tb, vb, interpolated, skin, cancelled), (ta, va)) in reports.into_iter().zip(after) {
            log.count("primitives split", 1);
            log.count("triangles before", tb);
            log.count("triangles after", ta);
            log.count("vertices before", vb);
            log.count("vertices after", va);
            log.count("vertices interpolated at tile lines", interpolated);
            log.approximate("skinned vertices with interpolated weights", skin);
            log.approximate(
                "blended normals that cancel, the nearest corner's kept",
                cancelled,
            );
        }
        Ok(())
    }
}
