//! `prepare-reskate-repeat-clipping`: remove the pieces Studio's repeat clipping discards before recording a map's
//! vertex-blend weights. Run after material/UV/geometry preparation, before overlay, blend-order and map summaries.
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
//! Only a source triangle that loses a polygon is rebuilt. Its surviving polygons keep their original UV tile,
//! winding and linearly interpolated streams, morph deltas and blend opacity; Studio then sees single-tile triangles.
//! Other triangles, opaque/clamped materials and collision are untouched. The shared meshops clip and vertex-blend
//! mechanisms perform the interpolation. Skin deformation cannot be predicted statically and is refused on a changed
//! primitive. Unknown per-vertex data refuses through the core geometry hooks. Material variants must agree about
//! repeat clipping, and all placements of a shared prototype must agree about each piece's survival (their linear
//! transforms supply the world-space area); disagreement is refused by name, before changing geometry. This operation
//! is local to owned prototypes in a conversion part. It reports discarded fan triangles per material,
//! wholly removed source triangles, and rebuilt triangles. It cannot predict later compiler LOD simplification or
//! float-position collapse caused by subsequent spatial batching; the postcompile containment check remains strict.
//! Coordinates beyond one million UV tiles or a triangle over the existing repeat-tile limit are refused by name.

use std::collections::BTreeMap;

use primase_core::ledger::Measure;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Entity, Instance, Mesh, Semantic, Stream, Topology, Transform, VertexBlend, World};
use primase_meshops::point_blend::{Replacement, blend_streams, describe, drawn_with, replace_primitives};

use crate::components::{SkateMaterial, SkatePrototype};
use crate::scene::{cut_at_repeats, repeats_texture};

/// Studio's summed polygon cross-product magnitude cutoff, in square metres (twice area).
const MIN_CROSS: f32 = 1e-10;
const UV_MARGIN: f32 = 1e-5;

/// See the module documentation. Selecting this pass explicitly selects Studio's clipped-piece exclusions.
#[derive(Clone, Copy, Debug)]
pub struct PrepareReskateRepeatClipping;

pub const PREPARE_RESKATE_REPEAT_CLIPPING: PassSpec = PassSpec {
    name: "prepare-reskate-repeat-clipping",
    options: &[],
    make: |_| Ok(Box::new(PrepareReskateRepeatClipping)),
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

fn survives(poly: &[Corner], t: &Transform) -> bool {
    // Translation does not change area. Include the placement's scale/shear without adding its world offset.
    let map =
        |p: [f32; 3]| -> [f32; 3] { std::array::from_fn(|a| (0..3).map(|b| t.cols[b][a] * p[b]).sum()) };
    let origin = map(poly[0].pos);
    let mut cross = [0.0f32; 3];
    for i in 2..poly.len() {
        let a = map(poly[i - 1].pos);
        let b = map(poly[i].pos);
        let a: [f32; 3] = std::array::from_fn(|k| a[k] - origin[k]);
        let b: [f32; 3] = std::array::from_fn(|k| b[k] - origin[k]);
        for k in 0..3 {
            cross[k] += a[(k + 1) % 3] * b[(k + 2) % 3] - a[(k + 2) % 3] * b[(k + 1) % 3];
        }
    }
    cross.iter().map(|x| x * x).sum::<f32>().sqrt() > MIN_CROSS
}

fn repeats(w: &World, m: Entity) -> bool {
    w.get::<SkateMaterial>(m).is_some_and(|s| {
        let (decal, blend) = cut_at_repeats(s);
        (decal || blend) && repeats_texture(s)
    })
}

impl Pass for PrepareReskateRepeatClipping {
    fn name(&self) -> &'static str {
        "prepare-reskate-repeat-clipping"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.morphs", "reskate.blend_opacity"]
    }
    fn approximates(&self) -> &'static str {
        "repeat-clipped interpolation in f32; cancelling normals keep the nearest corner"
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let owned = super::Owned::of(w);
        let mut transforms: BTreeMap<Entity, Vec<Transform>> = BTreeMap::new();
        for (e, i) in w.iter::<Instance>() {
            transforms
                .entry(i.prototype)
                .or_default()
                .push(w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY));
        }
        let mut plan = Vec::new();
        for (e, mesh) in w
            .iter::<Mesh>()
            .filter(|(e, _)| owned.contains(*e) && w.has::<SkatePrototype>(*e))
        {
            for (i, p) in mesh.primitives().iter().enumerate() {
                let materials = drawn_with(w, e, i, p);
                if !materials.iter().any(|&m| repeats(w, m)) {
                    continue;
                }
                let what = describe(w, e, i);
                if p.topology != Topology::Triangles {
                    return Err(format!("{what}: not triangles"));
                }
                let Some(Stream::F32x2(uv)) = p.streams.get(&Semantic::TexCoord(0)) else {
                    return Err(format!("{what}: no float2 UV0"));
                };
                let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) else {
                    return Err(format!("{what}: no float3 positions"));
                };
                let indices = p
                    .indices
                    .clone()
                    .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
                if !indices.len().is_multiple_of(3)
                    || indices
                        .iter()
                        .any(|&v| v as usize >= pos.len() || v as usize >= uv.len())
                {
                    return Err(format!("{what}: invalid triangle indices"));
                }
                if !pos.iter().flatten().all(|x| x.is_finite()) {
                    return Err(format!("{what}: non-finite positions"));
                }
                let placements = transforms
                    .get(&e)
                    .map(Vec::as_slice)
                    .unwrap_or(&[Transform::IDENTITY]);
                if !placements
                    .iter()
                    .all(|t| t.cols.iter().flatten().all(|x| x.is_finite()))
                {
                    return Err(format!("{what}: non-finite placement transform"));
                }
                let mut blends = Vec::new();
                let mut coords = Vec::new();
                let mut dropped = 0u64;
                let mut omitted = 0u64;
                for tri in indices.as_chunks::<3>().0 {
                    let polys = pieces(tri.map(|v| uv[v as usize]), tri.map(|v| pos[v as usize]))
                        .map_err(|err| format!("{what}: {err}"))?;
                    let mut retained = Vec::new();
                    let mut lost = 0;
                    if let Some(polys) = polys {
                        for poly in polys {
                            let keep = survives(&poly, &placements[0]);
                            if placements.iter().skip(1).any(|t| survives(&poly, t) != keep) {
                                return Err(format!(
                                    "{what}: shared placements disagree on repeat-clipped piece area; specialize the prototype by placement first"
                                ));
                            }
                            if keep {
                                retained.push(poly);
                            } else {
                                lost += (poly.len() - 2) as u64;
                            }
                        }
                    }
                    if lost == 0 {
                        for &v in tri {
                            blends.push(VertexBlend::copy(v));
                            coords.push(uv[v as usize]);
                        }
                        continue;
                    }
                    if materials.iter().any(|&m| !repeats(w, m)) {
                        return Err(format!(
                            "{what}: material variants disagree on repeat clipping; specialize-material-variants first"
                        ));
                    }
                    if w.has::<primase_core::skin::Skin>(e) {
                        return Err(format!(
                            "{what}: skinned repeat clipping needs the evaluated pose"
                        ));
                    }
                    dropped += lost;
                    omitted += u64::from(retained.is_empty());
                    for poly in retained {
                        for j in 1..poly.len() - 1 {
                            for k in [0, j, j + 1] {
                                blends.push(VertexBlend {
                                    sources: *tri,
                                    weights: poly[k].weights,
                                });
                                coords.push(poly[k].uv);
                            }
                        }
                    }
                }
                if dropped == 0 {
                    continue;
                }
                let mut blended = blend_streams(p, &blends, None, &what)?;
                blended
                    .streams
                    .insert(Semantic::TexCoord(0), Stream::F32x2(coords));
                log.approximate(
                    "repeat-clipped interpolation with cancelling normals (nearest corner kept)",
                    blended.normals_cancelled,
                );
                for m in materials {
                    let key = &w.get::<SkateMaterial>(m).expect("checked repeating").key;
                    log.count(&format!("{key}: discarded repeat-clipped fan triangles"), dropped);
                }
                log.dropped(
                    Measure::Triangles,
                    "source triangles discarded by Studio repeat clipping",
                    omitted,
                );
                log.count("repeat-clipped fan triangles discarded", dropped);
                log.count("replacement triangles", (blends.len() / 3) as u64);
                let count = u32::try_from(blends.len())
                    .map_err(|_| format!("{what}: too many replacement vertices"))?;
                plan.push(Replacement {
                    entity: e,
                    primitive: i,
                    indices: (0..count).collect(),
                    streams: blended.streams,
                    blends,
                    influences: None,
                });
            }
        }
        replace_primitives(w, plan)?;
        Ok(())
    }
}
