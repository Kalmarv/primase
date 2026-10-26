//! `prepare-reskate-repeat-clipping`: remove the pieces Studio's repeat clipping discards before recording a map's
//! vertex-blend weights. Run after overlay and blend-order preparation, before map summaries and blend-budget planning.
//!
//! Studio 20261012 clips repeating/mirrored decals and textured blended surfaces at integer UV lines. A triangle
//! within one tile (including an outside tile) bypasses the area check. Otherwise it rejects each clipped polygon
//! whose summed fan cross-product magnitude is at most 1e-10 square metres (area 5e-11), before triangulating it.
//! This is a polygon threshold, not a triangle-fan or UV-area threshold. The tile range uses a 1e-5 UV boundary margin.
//!
//! Evidence: normalized fixture maps written by primase and compiled with the closed Studio 20261012 CLI, no LODs.
//! With UVs (-2.0941606,1.5334065), (-2.0486684,1.5344927), (-2.0486684,0.6637857), source areas 3.2e-11 and
//! 6.4e-11 m2 yield respectively zero and two LOD0 triangles (vista proxies excluded); 5.12e-10 yields three.
//! The largest piece bounds the area cutoff to (2.72e-11, 5.45e-11] m2. Thirty cases also varied world scale and UV extent:
//! the same 1e-10 m2 triangle disappears with UVs (-.5,-.5),(1.5,-.5),(-.5,1.5), but retains two triangles with the
//! first UVs; 1e-12 m2 and exactly collinear triangles survive unchanged inside one tile. Scaling geometry changes
//! survival; changing UV extent changes the individual pieces. These observations agree with the independently
//! published Studio repeat-cut analysis (peter-w07/los-santos-reskate, tools/studio_cut_np.py). No code is copied.
//!
//! Studio clips shared meshes once in their local space: a two-instance fixture at scales 1 and .001 retains
//! one two-triangle shared MeshSet; baking those placements retains only the unit-scale triangle's two pieces.
//! Baked tiles and padded blend chains have different local f32 coordinates. In a real emitted sliver, clipping
//! at v=-4 rounds two x coordinates to the same 22.436964, losing a weight that survives prototype-space clipping.
//! Therefore preparation uses the final assembly, including instancing, overlay lifts and chain/cell padding.
//!
//! Every triangle crossing UV tiles is prepared, including those with no area loss: Studio's 1e-5 tile-range
//! margin can exclude a corner just beyond an integer edge (measured at u=1.0000069), losing its distinctive
//! weight. Its surviving polygons keep their original UV tile,
//! winding and linearly interpolated streams and blend opacity; Studio then sees single-tile triangles.
//! Other triangles, opaque/clamped materials and collision are untouched. Core meshes and their per-vertex data
//! remain authored. The prepared dialect resource stores only changed assembled primitives plus every mesh's
//! content identity. The writer refuses stale records and encodes replacements without making clipping decisions.
//! Shared prototypes need no specialization: their local assembly is processed once; baked placements are processed
//! at their actual tile/chain coordinates. Both possible budget forms are prepared. The encoding must be supplied
//! when it differs from the default. Reported per material across both candidate budget forms: discarded fan triangles, plus wholly removed source
//! triangles. Later compiler LOD simplification remains guarded by exact postcompile weight containment.
//! Coordinates beyond one million UV tiles or a triangle over the existing repeat-tile limit are refused by name.

use crate::EncodingOptions;
use crate::components::{RepeatMesh, SkateRepeatClipping};
use crate::glb::GlbPrim;
use crate::material::{MatLib, VClass};
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::{Primitive, Semantic, Stream, Topology, VertexBlend, World};
use primase_meshops::point_blend::blend_streams;
use std::collections::{BTreeMap, BTreeSet};
const MIN_CROSS: f32 = 1e-10;
const UV_MARGIN: f32 = 1e-5;

/// Prepare the assembled render geometry for the stated map encoding.
#[derive(Clone, Debug, Default)]
pub struct PrepareReskateRepeatClipping {
    encoding: EncodingOptions,
}
impl PrepareReskateRepeatClipping {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn for_encoding(mut self, e: &EncodingOptions) -> Result<Self, String> {
        super::overlays::check_grouping(e)?;
        self.encoding = super::overlays::grouping_only(e);
        Ok(self)
    }
}
pub const PREPARE_RESKATE_REPEAT_CLIPPING: PassSpec = PassSpec {
    name: "prepare-reskate-repeat-clipping",
    options: &super::overlays::ENCODING,
    make: |g| {
        Ok(Box::new(
            PrepareReskateRepeatClipping::new().for_encoding(&super::overlays::encoding_from(g)?)?,
        ))
    },
};
#[derive(Clone, Copy)]
struct Corner {
    uv: [f32; 2],
    pos: [f32; 3],
    weights: [f32; 3],
}

fn clip(poly: &[Corner], axis: usize, bound: f32, above: bool) -> Vec<Corner> {
    primase_meshops::clip_half_plane(
        poly,
        |c| {
            if above {
                c.uv[axis] - bound
            } else {
                bound - c.uv[axis]
            }
        },
        |a, b, t| {
            let mut c = Corner {
                uv: std::array::from_fn(|i| a.uv[i] + (b.uv[i] - a.uv[i]) * t),
                pos: std::array::from_fn(|i| a.pos[i] + (b.pos[i] - a.pos[i]) * t),
                weights: std::array::from_fn(|i| a.weights[i] + (b.weights[i] - a.weights[i]) * t),
            };
            c.uv[axis] = bound;
            c
        },
    )
}

fn range(uv: [[f32; 2]; 3], axis: usize) -> Result<(i32, i32), String> {
    let lo = uv.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min);
    let hi = uv.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max);
    if !uv.iter().all(|p| p[axis].is_finite()) || lo.abs().max(hi.abs()) > 1e6 {
        return Err("non-finite or excessive UV tile coordinate".into());
    }
    let first = (lo + UV_MARGIN).floor() as i32;
    Ok((first, ((hi - UV_MARGIN).floor() as i32).max(first)))
}

fn pieces(uv: [[f32; 2]; 3], pos: [[f32; 3]; 3]) -> Result<Option<Vec<Vec<Corner>>>, String> {
    let (u0, u1) = range(uv, 0)?;
    let (v0, v1) = range(uv, 1)?;
    if u0 == u1 && v0 == v1 {
        return Ok(None);
    }
    if i64::from(u1 - u0 + 1) * i64::from(v1 - v0 + 1) > primase_meshops::MAX_UV_TILES as i64 {
        return Err("too many repeat tiles; prepare-reskate-uv-repeats first".into());
    }
    let tri: Vec<_> = (0..3)
        .map(|i| Corner {
            uv: uv[i],
            pos: pos[i],
            weights: std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }),
        })
        .collect();
    let mut out = Vec::new();
    for u in u0..=u1 {
        let col = if u0 == u1 {
            tri.clone()
        } else {
            clip(&clip(&tri, 0, u as f32, true), 0, (u + 1) as f32, false)
        };
        if col.len() < 3 {
            continue;
        }
        for v in v0..=v1 {
            let poly = if v0 == v1 {
                col.clone()
            } else {
                clip(&clip(&col, 1, v as f32, true), 1, (v + 1) as f32, false)
            };
            if poly.len() >= 3 {
                out.push(poly);
            }
        }
    }
    Ok(Some(out))
}

fn survives(poly: &[Corner]) -> bool {
    let origin = poly[0].pos;
    let mut cross = [0.0f32; 3];
    for i in 2..poly.len() {
        let a = poly[i - 1].pos;
        let b = poly[i].pos;
        let a: [f32; 3] = std::array::from_fn(|k| a[k] - origin[k]);
        let b: [f32; 3] = std::array::from_fn(|k| b[k] - origin[k]);
        for k in 0..3 {
            cross[k] += a[(k + 1) % 3] * b[(k + 2) % 3] - a[(k + 2) % 3] * b[(k + 1) % 3];
        }
    }
    cross.iter().map(|x| x * x).sum::<f32>().sqrt() > MIN_CROSS
}

/// Identity includes every stream and its material name; reordering or a changed encoding invalidates preparation.
pub(crate) fn identity(prims: &[GlbPrim]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for p in prims {
        let name = p.material.as_deref().unwrap_or("");
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        for stream in [Some(&p.pos), p.nrm.as_ref(), p.uv.as_ref(), p.blend.as_ref()] {
            h.update([u8::from(stream.is_some())]);
            if let Some(values) = stream {
                h.update((values.len() as u64).to_le_bytes());
                for v in values {
                    h.update(v.to_bits().to_le_bytes());
                }
            }
        }
        h.update((p.idx.len() as u64).to_le_bytes());
        for i in &p.idx {
            h.update(i.to_le_bytes());
        }
    }
    h.finalize().into()
}
fn opacity(values: &[f32], b: &VertexBlend) -> (f32, bool) {
    let value = b
        .terms()
        .map(|(i, w)| f64::from(values[i as usize]) * f64::from(w))
        .sum::<f64>() as f32;
    (value.clamp(0.0, 1.0), !(0.0..=1.0).contains(&value))
}

struct Changed {
    primitive: Option<GlbPrim>,
    fan_triangles: u64,
    source_triangles: u64,
    opacity_clamped: u64,
    normals_cancelled: u64,
    split: u64,
}
fn prepare(p: &GlbPrim, what: &str) -> Result<Option<Changed>, String> {
    let Some(uv) = &p.uv else {
        return Ok(None);
    };
    let n = p.pos.len() / 3;
    if !p.idx.len().is_multiple_of(3)
        || p.pos.len() != n * 3
        || uv.len() != n * 2
        || p.idx.iter().any(|&i| i as usize >= n)
    {
        return Err(format!("{what}: malformed assembled triangle streams"));
    }
    let positions: Vec<[f32; 3]> = p.pos.as_chunks::<3>().0.to_vec();
    let coords: Vec<[f32; 2]> = uv.as_chunks::<2>().0.to_vec();
    let mut blends = Vec::new();
    let mut out_pos = Vec::new();
    let mut out_uv = Vec::new();
    let mut dropped = 0;
    let mut removed = 0;
    let mut split = 0;
    for tri in p.idx.as_chunks::<3>().0 {
        let mut kept = Vec::new();
        let mut lost = 0;
        let Some(polys) = pieces(
            tri.map(|i| coords[i as usize]),
            tri.map(|i| positions[i as usize]),
        )?
        else {
            for &v in tri {
                blends.push(VertexBlend::copy(v));
                out_pos.push(positions[v as usize]);
                out_uv.push(coords[v as usize]);
            }
            continue;
        };
        split += 1;
        for poly in polys {
            if survives(&poly) {
                kept.push(poly);
            } else {
                lost += (poly.len() - 2) as u64;
            }
        }
        dropped += lost;
        removed += u64::from(kept.is_empty());
        for poly in kept {
            for j in 1..poly.len() - 1 {
                for k in [0, j, j + 1] {
                    blends.push(VertexBlend {
                        sources: *tri,
                        weights: poly[k].weights,
                    });
                    out_pos.push(poly[k].pos);
                    out_uv.push(poly[k].uv);
                }
            }
        }
    }
    if split == 0 {
        return Ok(None);
    }
    if blends.is_empty() {
        return Ok(Some(Changed {
            primitive: None,
            fan_triangles: dropped,
            source_triangles: removed,
            opacity_clamped: 0,
            normals_cancelled: 0,
            split,
        }));
    }
    let mut streams = BTreeMap::from([(Semantic::Position, Stream::F32x3(positions))]);
    if let Some(normals) = &p.nrm {
        streams.insert(
            Semantic::Normal,
            Stream::F32x3(normals.as_chunks::<3>().0.to_vec()),
        );
    }
    let src = Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(p.idx.clone()),
        material: None,
    };
    let interpolated = blend_streams(&src, &blends, None, what)?;
    let nrm = interpolated.streams.get(&Semantic::Normal).map(|s| match s {
        Stream::F32x3(v) => v.iter().flatten().copied().collect(),
        _ => unreachable!(),
    });
    let mut opacity_clamped = 0;
    let blend = p.blend.as_ref().map(|values| {
        blends
            .iter()
            .map(|b| {
                let (value, clamped) = opacity(values, b);
                opacity_clamped += u64::from(clamped);
                value
            })
            .collect()
    });
    let count = u32::try_from(blends.len()).map_err(|_| format!("{what}: too many prepared vertices"))?;
    Ok(Some(Changed {
        primitive: Some(GlbPrim {
            material: p.material.clone(),
            pos: out_pos.into_iter().flatten().collect(),
            nrm,
            uv: Some(out_uv.into_iter().flatten().collect()),
            idx: (0..count).collect(),
            blend,
        }),
        fan_triangles: dropped,
        source_triangles: removed,
        opacity_clamped,
        normals_cancelled: interpolated.normals_cancelled,
        split,
    }))
}

pub(crate) fn record(
    stem: &str,
    prims: &[GlbPrim],
    mats: &MatLib,
    rec: &mut SkateRepeatClipping,
) -> Result<Vec<GlbPrim>, String> {
    let names: BTreeSet<&str> = mats
        .list
        .iter()
        .filter(|m| {
            (m.class == VClass::Decal
                || (matches!(m.class, VClass::Alpha | VClass::Glass) && m.alpha == 3 && m.tex >= 0))
                && matches!(m.wrap.as_deref(), None | Some("repeat" | "mirror"))
        })
        .map(|m| m.name.as_str())
        .collect();
    let mut replacements = BTreeMap::new();
    for (i, p) in prims.iter().enumerate() {
        if !p.material.as_deref().is_some_and(|m| names.contains(m)) {
            continue;
        }
        if let Some(changed) = prepare(p, stem)? {
            *rec.discarded
                .entry(p.material.clone().unwrap_or_default())
                .or_default() += changed.fan_triangles;
            rec.source_triangles += changed.source_triangles;
            rec.opacity_clamped += changed.opacity_clamped;
            rec.normals_cancelled += changed.normals_cancelled;
            rec.split += changed.split;
            replacements.insert(i, changed.primitive);
        }
    }
    rec.meshes.insert(
        stem.into(),
        RepeatMesh {
            source: identity(prims),
            replacements,
        },
    );
    replay(stem, prims, rec)
}
pub(crate) fn replay(
    stem: &str,
    prims: &[GlbPrim],
    rec: &SkateRepeatClipping,
) -> Result<Vec<GlbPrim>, String> {
    let mesh=rec.meshes.get(stem).ok_or_else(|| format!("{stem}: no repeat-clipping preparation; run prepare-reskate-repeat-clipping after overlays and blend order"))?;
    if mesh.source != identity(prims) {
        return Err(format!(
            "{stem}: stale repeat-clipping preparation; geometry/materials changed after prepare-reskate-repeat-clipping"
        ));
    }
    Ok(prims
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match mesh.replacements.get(&i) {
            Some(q) => q.clone(),
            None => Some(p.clone()),
        })
        .collect())
}
impl Pass for PrepareReskateRepeatClipping {
    fn name(&self) -> &'static str {
        "prepare-reskate-repeat-clipping"
    }
    fn options(&self) -> Options {
        super::overlays::encoding_options(&self.encoding)
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.repeat_clipping"]
    }
    fn approximates(&self) -> &'static str {
        "Studio repeat-clipped render geometry interpolated in f32"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateRepeatClipping>().is_some() {
            return Err("repeat clipping is already prepared".into());
        }
        let rec = crate::assembly::repeat_clipping(w, &self.encoding).map_err(|e| e.to_string())?;
        for (material, n) in &rec.discarded {
            log.omit(&format!("{material}: Studio repeat-clipped fan triangles"), *n);
        }
        log.count("source triangles wholly discarded", rec.source_triangles);
        log.count("triangles crossing UV tiles prepared", rec.split);
        log.approximate("f32 opacity roundoff clamped to 0..1", rec.opacity_clamped);
        log.approximate(
            "cancelled interpolated normals keep the nearest corner",
            rec.normals_cancelled,
        );
        log.count("render meshes prepared", rec.meshes.len() as u64);
        w.insert_resource(rec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn convex_weight_roundoff_stays_inside_the_studio_opacity_contract() {
        let b = VertexBlend {
            sources: [0, 1, 2],
            weights: [0.3, 0.3, 0.4000001],
        };
        assert_eq!(opacity(&[1.; 3], &b), (1., true));
        assert_eq!(opacity(&[0.; 3], &b), (0., false));
        assert_eq!(opacity(&[0.25; 3], &VertexBlend::copy(0)), (0.25, false));
    }
}
