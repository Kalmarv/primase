//! A primitive deformed: its morph targets at given weights (core's frame rule, `core.morphs`), then its skin by a
//! pose (`core.skin`'s [`Deformation::Linear`]).
//!
//! Linear blend skinning: each vertex's matrix is `sum_i w_i J_i B_i` (`J_i` its slot's joint in the skeleton's space
//! under the pose, `B_i` the slot's inverse bind); its position goes through that matrix, its normal through the
//! inverse transpose of its linear part and its tangent through the linear part, both then of unit length (a tangent's
//! w kept). Weights are used as stored, except under [`WeightRule::EngineNormalizes`], where they are divided by their
//! sum.

use std::collections::BTreeMap;

use primase_core::{
    Deformation, Influences, JointId, MorphId, Morphs, Primitive, Semantic, Slot, Stream, WeightRule,
};

use crate::pose::{Affine, affine, compose};
use crate::{Error, Result};

/// One primitive's vertex positions, normals and tangents (`[x, y, z, w]`, w the bitangent sign).
#[derive(Clone, Debug, PartialEq)]
pub struct Vertices {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
}

impl Vertices {
    /// The primitive's own streams.
    pub fn of(p: &Primitive) -> Result<Vertices> {
        let f3 = |s: Semantic| match p.streams.get(&s) {
            Some(Stream::F32x3(v)) => Ok(Some(v.clone())),
            None => Ok(None),
            Some(_) => Err(Error(format!("{s:?} is not three floats"))),
        };
        Ok(Vertices {
            positions: f3(Semantic::Position)?.ok_or_else(|| Error("no positions".into()))?,
            normals: f3(Semantic::Normal)?,
            tangents: match p.streams.get(&Semantic::Tangent) {
                Some(Stream::F32x4(v)) => Some(v.clone()),
                None => None,
                Some(_) => return Err(Error("Tangent is not four floats".into())),
            },
        })
    }
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 { v.map(|x| x / len) } else { v }
}

/// A morph target's frame factors at weight `w`, given its frames' weights (increasing, none zero: what
/// `core.morphs` verifies): the deltas to add are each listed frame's deltas times its factor, `(frame index, factor)`.
/// The delta is core's one rule ([`primase_core::morphs`]): piecewise linear in `w` through the zero point `(0, no
/// deltas)` and each frame's `(weight, deltas)`, the zero point in its place among them, extrapolated along the first
/// segment below the first point and along the last above the last. [`apply_morphs`] uses it; it is public so a
/// consumer deforming elsewhere (a GPU) uses the same factors.
pub fn frame_blend(weights: &[f32], w: f64) -> Vec<(usize, f64)> {
    if weights.is_empty() {
        return Vec::new();
    }
    let mut points: Vec<(f64, Option<usize>)> = weights
        .iter()
        .enumerate()
        .map(|(i, &f)| (f64::from(f), Some(i)))
        .collect();
    let zero = points.partition_point(|p| p.0 < 0.0);
    points.insert(zero, (0.0, None));
    let k = points
        .windows(2)
        .position(|p| w <= p[1].0)
        .unwrap_or(points.len() - 2);
    let ((w0, a), (w1, b)) = (points[k], points[k + 1]);
    let s = (w - w0) / (w1 - w0);
    let mut out = Vec::new();
    if let Some(a) = a {
        out.push((a, 1.0 - s));
    }
    if let Some(b) = b {
        out.push((b, s));
    }
    out
}

/// `v` with the morph targets of `morphs` applied to its primitive `primitive`, each at its weight in `weights` (a
/// target not there at its default).
pub fn apply_morphs(
    v: &Vertices,
    primitive: usize,
    morphs: &Morphs,
    weights: &BTreeMap<MorphId, f32>,
) -> Result<Vertices> {
    let n = v.positions.len();
    let mut pos: Vec<[f64; 3]> = v.positions.iter().map(|p| p.map(f64::from)).collect();
    let mut nor: Option<Vec<[f64; 3]>> = v
        .normals
        .as_ref()
        .map(|x| x.iter().map(|p| p.map(f64::from)).collect());
    let mut tan: Option<Vec<[f64; 3]>> = v
        .tangents
        .as_ref()
        .map(|x| x.iter().map(|t| [t[0], t[1], t[2]].map(f64::from)).collect());
    // the vertices a delta reached: only they are renormalised, the others keep their stored lengths
    let (mut normals_moved, mut tangents_moved) = (vec![false; n], vec![false; n]);
    for t in &morphs.targets {
        let w = f64::from(weights.get(&t.id).copied().unwrap_or(t.default_weight));
        if w == 0.0 {
            continue;
        }
        let frame_weights: Vec<f32> = t.frames.iter().map(|f| f.weight).collect();
        for (frame, factor) in frame_blend(&frame_weights, w) {
            let Some(Some(d)) = t.frames[frame].primitives.get(primitive) else {
                continue;
            };
            let at = |k: usize| -> Result<usize> {
                let i = match &d.indices {
                    Some(idx) => idx[k] as usize,
                    None => k,
                };
                (i < n)
                    .then_some(i)
                    .ok_or_else(|| Error(format!("target {:?}: vertex {i} of {n}", t.id)))
            };
            let add =
                |dst: &mut Vec<[f64; 3]>, moved: Option<&mut Vec<bool>>, src: &[[f32; 3]]| -> Result<()> {
                    let mut moved = moved;
                    for (k, delta) in src.iter().enumerate() {
                        let i = at(k)?;
                        for c in 0..3 {
                            dst[i][c] += factor * f64::from(delta[c]);
                        }
                        if let Some(m) = moved.as_deref_mut() {
                            m[i] = true;
                        }
                    }
                    Ok(())
                };
            if let Some(dp) = &d.positions {
                add(&mut pos, None, dp)?;
            }
            if let (Some(dn), Some(nor)) = (&d.normals, nor.as_mut()) {
                add(nor, Some(&mut normals_moved), dn)?;
            }
            if let (Some(dt), Some(tan)) = (&d.tangents, tan.as_mut()) {
                add(tan, Some(&mut tangents_moved), dt)?;
            }
        }
    }
    Ok(Vertices {
        positions: pos.iter().map(|p| p.map(|x| x as f32)).collect(),
        normals: nor.map(|x| {
            x.iter()
                .zip(&normals_moved)
                .map(|(n, &moved)| (if moved { unit(*n) } else { *n }).map(|c| c as f32))
                .collect()
        }),
        tangents: tan.map(|x| {
            x.iter()
                .zip(v.tangents.iter().flatten())
                .zip(&tangents_moved)
                .map(|((t, orig), &moved)| {
                    let t = if moved { unit(*t) } else { *t };
                    [t[0] as f32, t[1] as f32, t[2] as f32, orig[3]]
                })
                .collect()
        }),
    })
}

/// A primitive's skin influences, per vertex its `(joint slot, weight)` pairs, read from its integer-or-float joint
/// and weight streams as `how` says and weighted by `rule`: the influences [`skin`] uses, public so a consumer
/// deforming elsewhere (a GPU) uses the same ones.
pub fn influences(p: &Primitive, how: Influences, rule: WeightRule) -> Result<Vec<Vec<(usize, f64)>>> {
    let n = p.vertex_count();
    let stream = |s: Semantic| p.streams.get(&s).ok_or_else(|| Error(format!("no {s:?} stream")));
    let ints = |s: &Stream| -> Result<Vec<[u32; 4]>> {
        Ok(match s {
            Stream::U8x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
            Stream::U16x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
            _ => return Err(Error("joints are not an integer stream".into())),
        })
    };
    let floats = |s: &Stream| -> Result<Vec<[f64; 4]>> {
        Ok(match s {
            Stream::F32x4(v) => v.iter().map(|w| w.map(f64::from)).collect(),
            Stream::U8x4(v) => v.iter().map(|w| w.map(|x| f64::from(x) / 255.0)).collect(),
            Stream::U16x4(v) => v.iter().map(|w| w.map(|x| f64::from(x) / 65535.0)).collect(),
            _ => return Err(Error("weights are not a four-value stream".into())),
        })
    };
    let mut out: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    match how {
        Influences::Whole { slot } => out.iter_mut().for_each(|v| v.push((slot as usize, 1.0))),
        Influences::SingleIndex => {
            for (v, j) in out.iter_mut().zip(ints(stream(Semantic::Joints(0))?)?) {
                v.push((j[0] as usize, 1.0));
            }
        }
        Influences::Streams { sets } => {
            for k in 0..sets {
                let j = ints(stream(Semantic::Joints(k))?)?;
                let w = floats(stream(Semantic::Weights(k))?)?;
                for (v, (j, w)) in out.iter_mut().zip(j.iter().zip(&w)) {
                    v.extend(
                        j.iter()
                            .zip(w)
                            .filter(|(_, w)| **w != 0.0)
                            .map(|(&j, &w)| (j as usize, w)),
                    );
                }
            }
        }
    }
    for (i, v) in out.iter().enumerate() {
        // a NaN weight is no zero and would pass every comparison below it
        if let Some((_, w)) = v.iter().find(|(_, w)| !w.is_finite()) {
            return Err(Error(format!("vertex {i}: a weight is {w}")));
        }
        // no influence at all: the vertex would collapse to the origin whatever the rule says about sums
        if v.is_empty() {
            return Err(Error(format!("vertex {i}: all its weights are zero")));
        }
    }
    if rule == WeightRule::EngineNormalizes {
        for (i, v) in out.iter_mut().enumerate() {
            let sum: f64 = v.iter().map(|(_, w)| w).sum();
            if !sum.is_finite() || sum <= 0.0 {
                return Err(Error(format!(
                    "vertex {i}: weights sum to {sum}, nothing to normalise"
                )));
            }
            v.iter_mut().for_each(|(_, w)| *w /= sum);
        }
    }
    Ok(out)
}

/// `v` (primitive `p`'s vertices, perhaps already morphed) skinned by `palette` under a pose giving each joint's
/// transform in the skeleton's space (`model`, from [`crate::pose::model_transforms`]).
pub fn skin(
    v: &Vertices,
    p: &Primitive,
    how: Influences,
    rule: WeightRule,
    deformation: Deformation,
    palette: &[Slot],
    model: &BTreeMap<JointId, Affine>,
) -> Result<Vertices> {
    match deformation {
        Deformation::Linear => {}
    }
    let slots: Vec<Affine> = palette
        .iter()
        .map(|s| {
            let j = model
                .get(&s.joint)
                .ok_or_else(|| Error(format!("joint {:?} is not posed", s.joint)))?;
            Ok(compose(j, &affine(&s.inverse_bind)))
        })
        .collect::<Result<_>>()?;
    let per_vertex = influences(p, how, rule)?;
    let mut out = Vertices {
        positions: Vec::with_capacity(v.positions.len()),
        normals: v.normals.as_ref().map(|_| Vec::new()),
        tangents: v.tangents.as_ref().map(|_| Vec::new()),
    };
    for (i, inf) in per_vertex.iter().enumerate() {
        let mut m = [[0.0f64; 3]; 4];
        for &(slot, w) in inf {
            let s = slots
                .get(slot)
                .ok_or_else(|| Error(format!("vertex {i} names slot {slot} of {}", slots.len())))?;
            for (c, sc) in m.iter_mut().zip(s) {
                for r in 0..3 {
                    c[r] += w * sc[r];
                }
            }
        }
        let apply = |x: [f64; 3], t: f64| -> [f64; 3] {
            std::array::from_fn(|r| (0..3).map(|k| m[k][r] * x[k]).sum::<f64>() + t * m[3][r])
        };
        let p = v
            .positions
            .get(i)
            .ok_or_else(|| Error(format!("vertex {i} has no position")))?;
        out.positions.push(apply(p.map(f64::from), 1.0).map(|x| x as f32));
        if let (Some(src), Some(dst)) = (&v.normals, out.normals.as_mut()) {
            // the inverse transpose of the linear part, up to its determinant's scale: the cofactors
            let c = |a: [f64; 3], b: [f64; 3]| {
                [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ]
            };
            let cof = [c(m[1], m[2]), c(m[2], m[0]), c(m[0], m[1])];
            let det: f64 = (0..3).map(|r| m[0][r] * cof[0][r]).sum();
            let n = src[i].map(f64::from);
            let turned: [f64; 3] =
                std::array::from_fn(|r| (0..3).map(|k| cof[k][r] * n[k]).sum::<f64>() * det.signum());
            dst.push(unit(turned).map(|x| x as f32));
        }
        if let (Some(src), Some(dst)) = (&v.tangents, out.tangents.as_mut()) {
            let t = src[i];
            let d = unit(apply([t[0], t[1], t[2]].map(f64::from), 0.0));
            dst.push([d[0] as f32, d[1] as f32, d[2] as f32, t[3]]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Deltas, MorphFrame, MorphTarget, Topology, Transform};

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    fn translate(t: [f64; 3]) -> Affine {
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t]
    }

    fn primitive(streams: Vec<(Semantic, Stream)>) -> Primitive {
        Primitive {
            topology: Topology::Points,
            streams: streams.into_iter().collect(),
            indices: None,
            material: None,
        }
    }

    /// Hand-worked LBS: joint 1 at rest at the origin, joint 2 at rest at y=1 (inverse bind T(0,-1,0)), posed raised to
    /// y=2. A vertex at (0,1,0) half on each: 0.5 (0,1,0) + 0.5 (0,2,0) = (0,1.5,0); one wholly on joint 2 rises by
    /// 1; one on joint 1 stays. Normals of a pure translation stay. EngineNormalizes divides weights 1, 1 by 2.
    #[test]
    fn linear_blend_skinning_by_hand() {
        let palette = [
            Slot {
                joint: JointId(1),
                inverse_bind: Transform::IDENTITY,
            },
            Slot {
                joint: JointId(2),
                inverse_bind: {
                    let mut t = Transform::IDENTITY;
                    t.cols[3] = [0.0, -1.0, 0.0];
                    t
                },
            },
        ];
        let model = BTreeMap::from([
            (JointId(1), translate([0.0; 3])),
            (JointId(2), translate([0.0, 2.0, 0.0])),
        ]);
        let p = primitive(vec![
            (Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]; 3])),
            (Semantic::Normal, Stream::F32x3(vec![[1.0, 0.0, 0.0]; 3])),
            (
                Semantic::Joints(0),
                Stream::U8x4(vec![[0, 1, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]]),
            ),
            (
                Semantic::Weights(0),
                Stream::F32x4(vec![
                    [0.5, 0.5, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                ]),
            ),
        ]);
        let v = Vertices::of(&p).unwrap();
        let s = skin(
            &v,
            &p,
            Influences::Streams { sets: 1 },
            WeightRule::SumsToOne,
            Deformation::Linear,
            &palette,
            &model,
        )
        .unwrap();
        assert!(near(s.positions[0], [0.0, 1.5, 0.0]), "{:?}", s.positions);
        assert!(near(s.positions[1], [0.0, 2.0, 0.0]));
        assert!(near(s.positions[2], [0.0, 1.0, 0.0]));
        assert!(near(s.normals.as_ref().unwrap()[0], [1.0, 0.0, 0.0]));
        // whole to slot 1: every vertex rises by 1
        let w = skin(
            &v,
            &p,
            Influences::Whole { slot: 1 },
            WeightRule::AsStored,
            Deformation::Linear,
            &palette,
            &model,
        )
        .unwrap();
        assert!(w.positions.iter().all(|q| near(*q, [0.0, 2.0, 0.0])));
        // weights 1 and 1 normalised by the engine: halves
        let doubled = primitive(vec![
            (Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]])),
            (Semantic::Joints(0), Stream::U8x4(vec![[0, 1, 0, 0]])),
            (Semantic::Weights(0), Stream::F32x4(vec![[1.0, 1.0, 0.0, 0.0]])),
        ]);
        let d = skin(
            &Vertices::of(&doubled).unwrap(),
            &doubled,
            Influences::Streams { sets: 1 },
            WeightRule::EngineNormalizes,
            Deformation::Linear,
            &palette,
            &model,
        )
        .unwrap();
        assert!(near(d.positions[0], [0.0, 1.5, 0.0]));
    }

    /// Hand-worked: a joint turned a quarter about z turns a skinned normal (1,0,0) to (0,1,0), and a non-uniform
    /// scale (2 along x) turns the normal (1,1,0)/sqrt2 by the inverse transpose to (0.5,1,0) normalised, not (2,1,0).
    #[test]
    fn normals_follow_the_inverse_transpose() {
        let palette = [Slot {
            joint: JointId(1),
            inverse_bind: Transform::IDENTITY,
        }];
        let p = primitive(vec![
            (Semantic::Position, Stream::F32x3(vec![[0.0; 3]])),
            (Semantic::Normal, Stream::F32x3(vec![[1.0, 0.0, 0.0]])),
            (Semantic::Tangent, Stream::F32x4(vec![[0.0, 1.0, 0.0, -1.0]])),
        ]);
        let v = Vertices::of(&p).unwrap();
        let turn = BTreeMap::from([(
            JointId(1),
            [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
        )]);
        let s = skin(
            &v,
            &p,
            Influences::Whole { slot: 0 },
            WeightRule::AsStored,
            Deformation::Linear,
            &palette,
            &turn,
        )
        .unwrap();
        assert!(near(s.normals.as_ref().unwrap()[0], [0.0, 1.0, 0.0]));
        assert_eq!(s.tangents.as_ref().unwrap()[0][3], -1.0);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let q = primitive(vec![
            (Semantic::Position, Stream::F32x3(vec![[0.0; 3]])),
            (Semantic::Normal, Stream::F32x3(vec![[r, r, 0.0]])),
        ]);
        let stretch = BTreeMap::from([(
            JointId(1),
            [[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]],
        )]);
        let s = skin(
            &Vertices::of(&q).unwrap(),
            &q,
            Influences::Whole { slot: 0 },
            WeightRule::AsStored,
            Deformation::Linear,
            &palette,
            &stretch,
        )
        .unwrap();
        let len = (0.25f32 + 1.0).sqrt();
        assert!(near(s.normals.as_ref().unwrap()[0], [0.5 / len, 1.0 / len, 0.0]));
    }

    /// Hand-worked frames: a target with frames at 0.5 (delta 1 along y) and 1.0 (delta 3). At 0.25: half of the first
    /// frame, 0.5; at 0.75: halfway between, 2; at 1.5: extrapolated along the last segment, 3 + (3 - 1) = 5. A
    /// one-frame target at 1 with weight 0.5 is half its delta. Sparse deltas touch only their vertices.
    #[test]
    fn morph_frames_by_hand() {
        let target = |id: u32, frames: Vec<(f32, f32)>, indices: Option<Vec<u32>>| MorphTarget {
            id: MorphId(id),
            name: format!("t{id}"),
            default_weight: 0.0,
            frames: frames
                .into_iter()
                .map(|(w, dy)| MorphFrame {
                    weight: w,
                    primitives: vec![Some(Deltas {
                        positions: Some(vec![[0.0, dy, 0.0]; indices.as_ref().map_or(2, Vec::len)]),
                        indices: indices.clone(),
                        ..Deltas::default()
                    })],
                })
                .collect(),
        };
        let v = Vertices {
            positions: vec![[0.0; 3]; 2],
            normals: None,
            tangents: None,
        };
        let inbetween = Morphs {
            targets: vec![target(1, vec![(0.5, 1.0), (1.0, 3.0)], None)],
        };
        for (w, y) in [(0.25, 0.5), (0.75, 2.0), (1.5, 5.0), (0.0, 0.0)] {
            let m = apply_morphs(&v, 0, &inbetween, &BTreeMap::from([(MorphId(1), w)])).unwrap();
            assert!(
                near(m.positions[0], [0.0, y, 0.0]),
                "weight {w}: {:?}",
                m.positions[0]
            );
        }
        let sparse = Morphs {
            targets: vec![target(2, vec![(1.0, 4.0)], Some(vec![1]))],
        };
        let m = apply_morphs(&v, 0, &sparse, &BTreeMap::from([(MorphId(2), 0.5)])).unwrap();
        assert!(near(m.positions[0], [0.0; 3]) && near(m.positions[1], [0.0, 2.0, 0.0]));
    }

    /// Only the normals and tangents a delta reached are made unit length: a delta on vertex 1 leaves vertex 0's stored
    /// normal (2, 0, 0) and tangent (0, 3, 0) as they are, and takes vertex 1's (0, 0, 1) + (0, 1, 0) to
    /// (0, 1, 1) / sqrt 2.
    #[test]
    fn morphs_renormalise_only_what_they_moved() {
        let v = Vertices {
            positions: vec![[0.0; 3]; 2],
            normals: Some(vec![[2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            tangents: Some(vec![[0.0, 3.0, 0.0, 1.0], [1.0, 0.0, 0.0, -1.0]]),
        };
        let morphs = Morphs {
            targets: vec![MorphTarget {
                id: MorphId(1),
                name: "t".into(),
                default_weight: 0.0,
                frames: vec![MorphFrame {
                    weight: 1.0,
                    primitives: vec![Some(Deltas {
                        normals: Some(vec![[0.0, 1.0, 0.0]]),
                        tangents: Some(vec![[0.0, 1.0, 0.0]]),
                        indices: Some(vec![1]),
                        ..Deltas::default()
                    })],
                }],
            }],
        };
        let m = apply_morphs(&v, 0, &morphs, &BTreeMap::from([(MorphId(1), 1.0)])).unwrap();
        let (n, t) = (m.normals.unwrap(), m.tangents.unwrap());
        assert_eq!(n[0], [2.0, 0.0, 0.0]);
        assert_eq!(t[0], [0.0, 3.0, 0.0, 1.0]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(n[1], [0.0, r, r]), "{:?}", n[1]);
        assert!(near([t[1][0], t[1][1], t[1][2]], [r, r, 0.0]), "{:?}", t[1]);
        assert_eq!(t[1][3], -1.0);
    }

    /// A vertex with no weight is refused whatever the rule (it would collapse to the origin), and a NaN weight or a
    /// NaN sum is refused by name rather than compared past.
    #[test]
    fn unweighted_and_nan_vertices_are_refused() {
        let palette = [Slot {
            joint: JointId(1),
            inverse_bind: Transform::IDENTITY,
        }];
        let model = BTreeMap::from([(JointId(1), translate([0.0; 3]))]);
        let with = |weights: [f32; 4]| {
            primitive(vec![
                (Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]])),
                (Semantic::Joints(0), Stream::U8x4(vec![[0; 4]])),
                (Semantic::Weights(0), Stream::F32x4(vec![weights])),
            ])
        };
        for (weights, rule, what) in [
            ([0.0; 4], WeightRule::AsStored, "all its weights are zero"),
            ([0.0; 4], WeightRule::SumsToOne, "all its weights are zero"),
            ([f32::NAN, 1.0, 0.0, 0.0], WeightRule::AsStored, "a weight is NaN"),
            (
                [f32::NAN, 1.0, 0.0, 0.0],
                WeightRule::EngineNormalizes,
                "a weight is NaN",
            ),
            ([0.0; 4], WeightRule::EngineNormalizes, "all its weights are zero"),
        ] {
            let p = with(weights);
            let err = skin(
                &Vertices::of(&p).unwrap(),
                &p,
                Influences::Streams { sets: 1 },
                rule,
                Deformation::Linear,
                &palette,
                &model,
            )
            .unwrap_err();
            assert!(
                err.0.contains("vertex 0") && err.0.contains(what),
                "{rule:?}: {}",
                err.0
            );
        }
    }

    /// One rule, through the zero point wherever the frames lie: frames at -1 and 1 put it between them; a single frame
    /// below zero is a line through (0, 0) too, not a frame held at factor 1.
    #[test]
    fn frames_blend_through_the_zero_point() {
        let near = |a: &[(usize, f64)], b: &[(usize, f64)]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() < 1e-12)
        };
        // -1 and 1: the zero point between them
        assert!(near(&frame_blend(&[-1.0, 1.0], 0.5), &[(1, 0.5)]));
        assert!(near(&frame_blend(&[-1.0, 1.0], -0.25), &[(0, 0.25)]));
        assert!(
            frame_blend(&[-1.0, 1.0], 0.0)
                .iter()
                .all(|(_, f)| f.abs() < 1e-12)
        );
        // one frame at -2: a line through (0, 0), extrapolated both ways
        assert!(near(&frame_blend(&[-2.0], -1.0), &[(0, 0.5)]));
        assert!(near(&frame_blend(&[-2.0], 1.0), &[(0, -0.5)]));
        // above the last frame, along the last segment
        assert!(near(&frame_blend(&[0.5, 1.0], 1.5), &[(0, -1.0), (1, 2.0)]));
        assert!(frame_blend(&[], 1.0).is_empty());
    }
}
