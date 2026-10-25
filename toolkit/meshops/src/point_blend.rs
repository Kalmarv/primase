//! What the passes that cut triangles share: a point of a source triangle as a combination of its corners, the vertex
//! streams of the points a cut makes, and the replacement of a primitive's geometry by the cut one.
//!
//! **Points.** A new vertex lies in one source triangle, so it is a combination of that triangle's corners
//! ([`primase_core::VertexBlend`]), kept in a canonical form ([`Bary`]) so equal points are equal values: the weld
//! key, and what makes the cut of an edge two triangles share the same from both sides ([`Corner::cut_at`]).
//!
//! **Streams.** [`blend_streams`] makes every stream of the new vertices from the corners' with those weights:
//! positions, texture coordinates and colours as they are (8- and 16-bit colours rounded), normals and the xyz of
//! tangents renormalised (a tangent's w is kept: refused when the corners it combines differ in w; blended normals
//! that cancel keep the nearest corner's, counted), and skin influences as the union of the corners' joints with the
//! weights interpolated.
//!
//! **Replacing.** [`replace_primitives`] puts the cut primitives into their meshes, with their morph targets blended
//! the same way ([`primase_core::World::blend_vertices`]) and their skin influences, and removes the file link of a
//! changed mesh.

use std::collections::BTreeMap;

use primase_core::{
    Entity, Influences, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic, Stream, VertexBlend,
    World, skin::Skin,
};

/// A point of a source triangle as a combination of its corners, in a canonical form (sources ascending, none with
/// weight 0 but padding) so equal points are equal values: the weld key, and what makes the cut of a shared edge the
/// same from both sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bary {
    src: [u32; 3],
    w: [f32; 3],
}

impl Bary {
    pub fn corner(v: u32) -> Bary {
        Bary {
            src: [v; 3],
            w: [1.0, 0.0, 0.0],
        }
    }

    pub fn key(&self) -> Key {
        (self.src, self.w.map(f32::to_bits))
    }

    pub fn blend(&self) -> VertexBlend {
        VertexBlend {
            sources: self.src,
            weights: self.w,
        }
    }

    /// The key of the point with its weights rounded to multiples of 2^-`bits` (a weight that rounds to 0 is dropped),
    /// so points that differ by the rounding of the f32 weights (the same point reached by clipping in another
    /// order) are one. The weights are f32: `bits` is at most 20 for the rounding to absorb their error.
    pub fn welded_key(&self, bits: u32) -> Key {
        let scale = f64::from(1u32 << bits);
        let mut terms: Vec<(u32, u32)> = self
            .blend()
            .terms()
            .map(|(s, w)| (s, (f64::from(w) * scale).round() as u32))
            .filter(|&(_, q)| q > 0)
            .collect();
        if terms.is_empty() {
            terms.push((self.src[0], 1));
        }
        let mut key: Key = ([terms[0].0; 3], [0; 3]);
        for (k, (s, q)) in terms.into_iter().take(3).enumerate() {
            key.0[k] = s;
            key.1[k] = q;
        }
        key
    }

    /// `(1 - t) a + t b`, canonical.
    pub fn lerp(a: &Bary, b: &Bary, t: f64) -> Bary {
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

/// A point's canonical form: its sources and the bits of its weights.
pub type Key = ([u32; 3], [u32; 3]);

/// A vertex of a polygon being clipped: its point and its coordinates (exact on the line it was cut at).
#[derive(Clone, Copy, Debug)]
pub struct Corner {
    pub at: Bary,
    pub uv: [f64; 2],
}

impl Corner {
    /// The point on edge `a`-`b` at the parameter `along` gives (0 at `a`, 1 at `b`, clamped to that), with `uv`
    /// interpolated linearly. `along` is called with the edge's ends in a fixed order (the smaller point first), so
    /// both triangles sharing the edge get the same point for the same line.
    pub fn cut_at(a: &Corner, b: &Corner, along: impl Fn(&Corner, &Corner) -> f64) -> Corner {
        let (a, b) = if a.at.key() <= b.at.key() { (a, b) } else { (b, a) };
        let t = along(a, b).clamp(0.0, 1.0);
        Corner {
            at: Bary::lerp(&a.at, &b.at, t),
            uv: [0, 1].map(|k| (1.0 - t) * a.uv[k] + t * b.uv[k]),
        }
    }

    pub fn same(&self, other: &Corner) -> bool {
        self.at.key() == other.at.key()
    }
}

/// `p` without repeated neighbours (the last against the first too), `None` when fewer than three distinct corners
/// remain: no area to speak of.
pub fn tidy(mut p: Vec<Corner>) -> Option<Vec<Corner>> {
    p.dedup_by(|a, b| a.same(b));
    while p.len() > 1 && p[0].same(&p[p.len() - 1]) {
        p.pop();
    }
    (p.len() >= 3).then_some(p)
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
                if !single && matches!(sem, Semantic::Normal | Semantic::Tangent(_)) {
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
    if matches!(sem, Semantic::Tangent(_))
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
            if matches!(sem, Semantic::Tangent(_))
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
pub fn describe(w: &World, e: Entity, i: usize) -> String {
    match w.get::<Name>(e) {
        Some(n) => format!("{e:?} {:?} primitive {i}", n.0),
        None => format!("{e:?} primitive {i}"),
    }
}

/// The materials `e`'s primitive `i` is drawn with (its own and its variants').
pub fn drawn_with(w: &World, e: Entity, i: usize, p: &Primitive) -> Vec<Entity> {
    let mut ms: Vec<Entity> = p.material.into_iter().collect();
    if let Some(v) = w.get::<MaterialVariants>(e).and_then(|v| v.0.get(i)) {
        ms.extend(v.values().copied());
    }
    ms.sort();
    ms.dedup();
    ms
}

/// The streams of a primitive's new vertices, and what making them reported.
#[derive(Debug)]
pub struct Blended {
    pub streams: BTreeMap<Semantic, Stream>,
    /// The primitive's influences after the cut (see the module documentation).
    pub influences: Option<Influences>,
    /// New vertices whose skin weights are an interpolation that differs from their corners'.
    pub skin_interpolated: u64,
    /// Blended normals that cancel, which keep the nearest corner's.
    pub normals_cancelled: u64,
}

/// Every stream of `p` for the new vertices `blends` (each a combination of vertices of `p`), the skin influences
/// included. `influences` is the primitive's own (its entity's [`Skin`]). `what` names the primitive in refusals.
pub fn blend_streams(
    p: &Primitive,
    blends: &[VertexBlend],
    influences: Option<Influences>,
    what: &str,
) -> Result<Blended, String> {
    let mut streams = BTreeMap::new();
    let mut normals_cancelled = 0;
    for (sem, stream) in &p.streams {
        if matches!(sem, Semantic::Joints(_) | Semantic::Weights(_)) {
            continue;
        }
        streams.insert(
            *sem,
            blend_stream(*sem, stream, blends, &mut normals_cancelled, what)?,
        );
    }
    let (skin, influences, skin_interpolated) = blend_skin(p, influences, blends, what)?;
    streams.extend(skin);
    Ok(Blended {
        streams,
        influences,
        skin_interpolated,
        normals_cancelled,
    })
}

/// A primitive's cut geometry, to be put in place of its own.
#[derive(Debug)]
pub struct Replacement {
    pub entity: Entity,
    pub primitive: usize,
    /// Every stream of the new vertices ([`blend_streams`], with any the pass made itself replacing its own).
    pub streams: BTreeMap<Semantic, Stream>,
    pub indices: Vec<u32>,
    /// The combination of the old vertices each new vertex is.
    pub blends: Vec<VertexBlend>,
    /// The primitive's skin influences afterwards.
    pub influences: Option<Influences>,
}

/// Put every replacement in place. Everything is checked before anything changes (the morph targets and other
/// per-vertex components of each entity must be able to combine vertices: refused naming what cannot). A changed
/// mesh no longer matches its file, so its [`MeshFile`] goes.
pub fn replace_primitives(w: &mut World, plan: Vec<Replacement>) -> Result<(), String> {
    for r in &plan {
        w.check_blend_vertices(r.entity, r.primitive, &r.blends)?;
    }
    let mut by_entity: BTreeMap<Entity, Vec<Replacement>> = BTreeMap::new();
    for r in plan {
        by_entity.entry(r.entity).or_default().push(r);
    }
    for (e, replaced) in by_entity {
        for r in &replaced {
            w.blend_vertices(e, r.primitive, &r.blends)?;
        }
        w.edit_mesh(e, |mesh: &mut Mesh| {
            for r in &replaced {
                let mut target = mesh
                    .primitive_mut(r.primitive)
                    .ok_or_else(|| format!("{e:?}: primitive {} vanished", r.primitive))?;
                let fields = target.fields();
                *fields.streams = r.streams.clone();
                *fields.indices = Some(r.indices.clone());
            }
            Ok(None)
        })?;
        if let Some(skin) = w.get_mut::<Skin>(e) {
            for r in &replaced {
                if let Some(slot) = skin.influences.get_mut(r.primitive) {
                    *slot = r.influences;
                }
            }
        }
        w.remove::<MeshFile>(e);
    }
    Ok(())
}
