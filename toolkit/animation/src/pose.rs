//! Poses: a clip evaluated at a time on a skeleton (`core.clip` on `core.skeleton`, over a base pose), and a pose's
//! joints composed through their parents into the skeleton's space.

use std::collections::BTreeMap;

use primase_core::{
    AdditiveReference, Clip, ClipTarget, Entity, Extrapolation, JointId, MorphId, Property, Rest, RigPose,
    Skeleton, Transform,
};

use crate::curve::sample;
use crate::quat;
use crate::{Error, Result};

/// An affine map in f64, column by column (images of x, y, z, then the translation), as `core.transform`.
pub type Affine = [[f64; 3]; 4];

/// `a * b` (apply `b`, then `a`).
pub fn compose(a: &Affine, b: &Affine) -> Affine {
    let lin = |v: [f64; 3]| -> [f64; 3] { std::array::from_fn(|r| (0..3).map(|k| a[k][r] * v[k]).sum()) };
    let t = lin(b[3]);
    [
        lin(b[0]),
        lin(b[1]),
        lin(b[2]),
        std::array::from_fn(|r| t[r] + a[3][r]),
    ]
}

/// The inverse of an affine map; `None` for a singular one.
pub fn inverse(m: &Affine) -> Option<Affine> {
    let c = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let cof = [c(m[1], m[2]), c(m[2], m[0]), c(m[0], m[1])];
    let det: f64 = (0..3).map(|r| m[0][r] * cof[0][r]).sum();
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // row i of M^-1 is cof[i] / det: column j holds entry j of each
    let lin: [[f64; 3]; 3] = std::array::from_fn(|j| std::array::from_fn(|i| cof[i][j] / det));
    let t: [f64; 3] = std::array::from_fn(|r| -(0..3).map(|k| lin[k][r] * m[3][k]).sum::<f64>());
    Some([lin[0], lin[1], lin[2], t])
}

/// A transform's entries as f64.
pub fn affine(t: &Transform) -> Affine {
    t.cols.map(|c| c.map(f64::from))
}

/// The map rounded once per entry to a transform.
pub fn transform(m: &Affine) -> Transform {
    Transform {
        cols: m.map(|c| c.map(|x| x as f32)),
    }
}

/// A joint's local transform as translation, rotation (unit quaternion `[x, y, z, w]`) and scale: what clip channels
/// replace one at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
}

/// How far from orthonormal an affine rest's normalised columns may be and still decompose into a rotation and scale.
const ORTHO_TOLERANCE: f64 = 1e-5;

impl Trs {
    /// A rest as TRS: a [`Rest::Trs`] as stated; a [`Rest::Affine`] decomposed when its columns are orthogonal with a
    /// positive determinant (exact up to rounding). A sheared or mirrored matrix has no such form: an error, since a
    /// channel replacing one of its parts would have no meaning.
    pub fn of(rest: &Rest) -> Result<Trs> {
        match *rest {
            Rest::Trs {
                translation,
                rotation,
                scale,
            } => Ok(Trs {
                translation: translation.map(f64::from),
                rotation: quat::normalize(rotation.map(f64::from))
                    .ok_or_else(|| Error("a zero rotation".into()))?,
                scale: scale.map(f64::from),
            }),
            Rest::Affine(m) => Trs::of_affine(&affine(&m)),
        }
    }

    /// An affine map as TRS, on the conditions [`Trs::of`] states for a [`Rest::Affine`], with no rounding to f32 between
    /// the map and its parts (a map rounded to f32 has columns whose lengths are not exactly the scale it was made from).
    pub fn of_affine(c: &Affine) -> Result<Trs> {
        let c = *c;
        let len: [f64; 3] = std::array::from_fn(|i| c[i].iter().map(|x| x * x).sum::<f64>().sqrt());
        if len.iter().any(|&l| l == 0.0 || !l.is_finite()) {
            return Err(Error("a degenerate rest has no rotation".into()));
        }
        let u: [[f64; 3]; 3] = std::array::from_fn(|i| c[i].map(|x| x / len[i]));
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        if [(0, 1), (0, 2), (1, 2)]
            .iter()
            .any(|&(i, j)| dot(u[i], u[j]).abs() > ORTHO_TOLERANCE)
        {
            return Err(Error(
                "a sheared rest has no translation-rotation-scale form".into(),
            ));
        }
        if dot(cross(u[0], u[1]), u[2]) < 0.0 {
            return Err(Error("a mirrored rest has no rotation".into()));
        }
        Ok(Trs {
            translation: c[3],
            rotation: quat::from_columns(u),
            scale: len,
        })
    }

    /// `T * R * S`.
    pub fn matrix(&self) -> Affine {
        let r = quat::to_columns(self.rotation);
        [
            r[0].map(|v| v * self.scale[0]),
            r[1].map(|v| v * self.scale[1]),
            r[2].map(|v| v * self.scale[2]),
            self.translation,
        ]
    }

    fn rest(&self) -> Rest {
        Rest::Trs {
            translation: self.translation.map(|x| x as f32),
            rotation: self.rotation.map(|x| x as f32),
            scale: self.scale.map(|x| x as f32),
        }
    }
}

/// Each joint's local transform: the base pose's where one is given (a rig's `core.rig_pose`), else the skeleton's
/// rest.
pub fn locals(skeleton: &Skeleton, base: Option<&RigPose>) -> BTreeMap<JointId, Rest> {
    let mut out: BTreeMap<JointId, Rest> = skeleton.joints.iter().map(|j| (j.id, j.rest)).collect();
    if let Some(p) = base {
        for (id, rest) in &p.joints {
            out.insert(*id, *rest);
        }
    }
    out
}

/// A clip evaluated at a time: the pose of its skeleton's joints, the values of its other channels, and its root
/// motion.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluated {
    /// Every joint of the skeleton, those the clip drives as TRS, the others as their base.
    pub pose: RigPose,
    /// Morph weights by the morph set (the prototype holding it) and target.
    pub morphs: Vec<(Entity, MorphId, f32)>,
    /// Source controls by name.
    pub controls: Vec<(String, f32)>,
    /// The root motion's transform in the rig's space at that time (identity parts where the clip has none).
    pub root: Option<Transform>,
    /// Joint channels on another skeleton (not this rig's): not evaluated here.
    pub other_skeletons: usize,
}

/// The clip's time for `t` seconds of playback: looped into `[0, duration]`, or as is (channels hold their ends).
fn clip_time(clip: &Clip, t: f64) -> f64 {
    match clip.extrapolation {
        Extrapolation::Loop if clip.duration > 0.0 => t.rem_euclid(f64::from(clip.duration)),
        _ => t,
    }
}

/// `clip` at `t` seconds on the skeleton held by `skeleton_entity`, over `base` (else the rests): each joint channel
/// replaces its joint's translation, rotation or scale (a joint it drives must have a TRS form, [`Trs::of`]).
///
/// An additive clip ([`AdditiveReference::Identity`]: values that are deltas) is composed onto `base` joint by joint, as
/// `core.clip` states: `T = T_base + T_clip`, `R = R_base * R_clip` (normalised), `S = S_base * S_clip`; a joint or a
/// property the clip does not drive keeps the base. One whose reference is a pose or a frame of a clip is evaluated by
/// [`evaluate_relative`], which is given that reference; here it is an error (never a silent rest pose), as is an additive
/// clip's morph or control channel, which has no base to add to.
pub fn evaluate(
    clip: &Clip,
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    t: f64,
) -> Result<Evaluated> {
    evaluate_over(clip, skeleton_entity, skeleton, base, None, t)
}

/// [`evaluate`] for an additive clip whose reference is a pose or a frame of a clip ([`AdditiveReference::Pose`],
/// [`AdditiveReference::ClipFrame`]): `reference` is that pose, or that clip evaluated at the frame's time (the evaluator
/// holds no world, so the caller resolves it). Composed as `core.clip` states: `T = T_base + (T_clip - T_ref)`,
/// `R = R_base * R_ref^-1 * R_clip`, `S = S_base * S_clip / S_ref`.
pub fn evaluate_relative(
    clip: &Clip,
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    reference: &RigPose,
    t: f64,
) -> Result<Evaluated> {
    evaluate_over(clip, skeleton_entity, skeleton, base, Some(reference), t)
}

/// One property of a joint of an additive clip composed onto `current`: `T_base + (T_clip - T_ref)`,
/// `R_base * R_ref^-1 * R_clip`, `S_base * S_clip / S_ref`, the reference zero, the identity and one for a delta clip.
fn compose_additive(property: Property, current: &mut Trs, reference: Option<&Trs>, v: &[f64]) -> Result<()> {
    match property {
        Property::Translation => {
            let r = reference.map_or([0.0; 3], |r| r.translation);
            current.translation = std::array::from_fn(|i| current.translation[i] + (v[i] - r[i]));
        }
        Property::Rotation => {
            let r = reference.map_or([0.0, 0.0, 0.0, 1.0], |r| r.rotation);
            let q = quat::mul(
                quat::mul(current.rotation, quat::inverse(r)),
                [v[0], v[1], v[2], v[3]],
            );
            current.rotation = quat::normalize(q)
                .ok_or_else(|| Error("an additive rotation composes to a zero one".into()))?;
        }
        Property::Scale => {
            let r = reference.map_or([1.0; 3], |r| r.scale);
            if r.contains(&0.0) {
                return Err(Error(
                    "an additive scale is relative to a reference scale of zero".into(),
                ));
            }
            current.scale = std::array::from_fn(|i| current.scale[i] * v[i] / r[i]);
        }
        Property::Scalar => return Err(Error("a scalar channel on a joint".into())),
    }
    Ok(())
}

fn evaluate_over(
    clip: &Clip,
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    reference: Option<&RigPose>,
    t: f64,
) -> Result<Evaluated> {
    // the reference's joints, where the clip is relative to a pose
    let reference_locals = match (clip.additive, reference) {
        (None, None) | (Some(AdditiveReference::Identity), None) => None,
        (Some(AdditiveReference::Pose(_) | AdditiveReference::ClipFrame { .. }), Some(r)) => {
            Some(locals(skeleton, Some(r)))
        }
        (Some(AdditiveReference::Pose(_) | AdditiveReference::ClipFrame { .. }), None) => {
            return Err(Error(format!(
                "clip {:?} is additive to a pose or a frame of a clip: evaluate_relative takes that reference",
                clip.name
            )));
        }
        (_, Some(_)) => {
            return Err(Error(format!(
                "clip {:?} is not additive to a pose or a frame of a clip: it has no reference to be given",
                clip.name
            )));
        }
    };
    let additive = clip.additive.is_some();
    let t = clip_time(clip, t);
    let mut locals = locals(skeleton, base);
    let mut driven: BTreeMap<JointId, Trs> = BTreeMap::new();
    let mut out = Evaluated {
        pose: RigPose {
            joints: Vec::new(),
            root_applied: base.and_then(|b| b.root_applied),
        },
        morphs: Vec::new(),
        controls: Vec::new(),
        root: None,
        other_skeletons: 0,
    };
    for c in &clip.channels {
        let width = match c.property {
            Property::Translation | Property::Scale => 3,
            Property::Rotation => 4,
            Property::Scalar => 1,
        };
        let v = sample(
            c.interpolation,
            &c.times,
            &c.values,
            width,
            c.property == Property::Rotation,
            t,
        )?;
        match &c.target {
            ClipTarget::Joint { skeleton: s, joint } if *s == skeleton_entity => {
                let base = locals
                    .get(joint)
                    .ok_or_else(|| Error(format!("joint {joint:?} is not in the skeleton")))?;
                let trs = match driven.get(joint) {
                    Some(t) => *t,
                    None => Trs::of(base).map_err(|e| Error(format!("joint {joint:?}: {e}")))?,
                };
                let mut trs = trs;
                if additive {
                    let reference = match &reference_locals {
                        Some(r) => Some(
                            Trs::of(&r[joint])
                                .map_err(|e| Error(format!("joint {joint:?}'s reference: {e}")))?,
                        ),
                        None => None,
                    };
                    compose_additive(c.property, &mut trs, reference.as_ref(), &v)
                        .map_err(|e| Error(format!("joint {joint:?}: {}", e.0)))?;
                } else {
                    match c.property {
                        Property::Translation => trs.translation = [v[0], v[1], v[2]],
                        Property::Rotation => trs.rotation = [v[0], v[1], v[2], v[3]],
                        Property::Scale => trs.scale = [v[0], v[1], v[2]],
                        Property::Scalar => return Err(Error("a scalar channel on a joint".into())),
                    }
                }
                driven.insert(*joint, trs);
            }
            ClipTarget::Joint { .. } => out.other_skeletons += 1,
            ClipTarget::Morph { .. } | ClipTarget::Control { .. } if additive => {
                return Err(Error(format!(
                    "clip {:?} is additive and drives a morph or control: there is no base to add to",
                    clip.name
                )));
            }
            ClipTarget::Morph { geometry, morph } => out.morphs.push((*geometry, *morph, v[0] as f32)),
            ClipTarget::Control { name } => out.controls.push((name.clone(), v[0] as f32)),
        }
    }
    for (id, trs) in &driven {
        locals.insert(*id, trs.rest());
    }
    out.pose.joints = skeleton.joints.iter().map(|j| (j.id, locals[&j.id])).collect();
    if let Some(r) = &clip.root_motion {
        let mut m = Trs {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        if let Some((i, times, values)) = &r.translation {
            let v = sample(*i, times, values, 3, false, t)?;
            m.translation = [v[0], v[1], v[2]];
        }
        if let Some((i, times, values)) = &r.rotation {
            let v = sample(*i, times, values, 4, true, t)?;
            m.rotation = [v[0], v[1], v[2], v[3]];
        }
        out.root = Some(transform(&m.matrix()));
    }
    Ok(out)
}

/// Poses blended: every part's joints, morph weights, controls and root motion, each weighted by the part's share of the
/// weights (relative: divided by their sum). A part of weight 0 is left out whatever its shape; the others must share
/// the skeleton (the same joints in the same order, the same `root_applied`), the same morphs and controls, and all have
/// root motion or none, since a blend of something and nothing has no stated meaning here (an error, not a guess).
///
/// A joint every part states alike stays as stated. Else translations and scales are the weighted sum, and rotations the
/// weighted sum of the unit quaternions, each flipped to the hemisphere of the first part's (`q` and `-q` are one
/// rotation), normalised. That sum-and-normalise form is this crate's reading of how weighted poses combine; it is the
/// weighted mean of the rotations for small differences and is not checked against an engine here.
pub fn blend(parts: &[(&Evaluated, f64)]) -> Result<Evaluated> {
    let mut total = 0.0;
    for (_, w) in parts {
        if !w.is_finite() || *w < 0.0 {
            return Err(Error(format!("a blend weight of {w}")));
        }
        total += w;
    }
    if total <= 0.0 {
        return Err(Error("a blend of no weight".into()));
    }
    let used: Vec<(&Evaluated, f64)> = parts
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(e, w)| (*e, w / total))
        .collect();
    let (first, _) = used[0];
    if used.len() == 1 {
        return Ok(first.clone());
    }
    for (e, _) in &used[1..] {
        if e.pose.root_applied != first.pose.root_applied {
            return Err(Error("blended poses differ in root_applied".into()));
        }
        if e.pose.joints.len() != first.pose.joints.len()
            || e.pose
                .joints
                .iter()
                .zip(&first.pose.joints)
                .any(|(a, b)| a.0 != b.0)
        {
            return Err(Error("blended poses are of different joints".into()));
        }
        if e.root.is_some() != first.root.is_some() {
            return Err(Error(
                "some of the blended poses have root motion and some do not".into(),
            ));
        }
    }
    let mut joints = Vec::with_capacity(first.pose.joints.len());
    for (k, (id, rest)) in first.pose.joints.iter().enumerate() {
        if used.iter().all(|(e, _)| e.pose.joints[k].1 == *rest) {
            joints.push((*id, *rest));
            continue;
        }
        let blended = used
            .iter()
            .map(|(e, w)| Trs::of(&e.pose.joints[k].1).map(|t| (t, *w)))
            .collect::<Result<Vec<_>>>()
            .and_then(|trs| blend_trs(&trs))
            .map_err(|e| Error(format!("joint {id:?}: {}", e.0)))?;
        joints.push((*id, blended.rest()));
    }
    let root = if first.root.is_some() {
        let blended = used
            .iter()
            .filter_map(|(e, w)| e.root.map(|r| (r, *w)))
            .map(|(r, w)| Trs::of(&Rest::Affine(r)).map(|t| (t, w)))
            .collect::<Result<Vec<_>>>()
            .and_then(|trs| blend_trs(&trs))
            .map_err(|e| Error(format!("root motion: {}", e.0)))?;
        Some(transform(&blended.matrix()))
    } else {
        None
    };
    // morph weights and controls: the same keys in each, summed by weight
    let mut morphs = first.morphs.clone();
    let mut controls = first.controls.clone();
    for m in &mut morphs {
        m.2 = 0.0;
    }
    for c in &mut controls {
        c.1 = 0.0;
    }
    for (e, w) in &used {
        if e.morphs.len() != morphs.len() || e.controls.len() != controls.len() {
            return Err(Error("blended poses drive different morphs or controls".into()));
        }
        for (geometry, morph, v) in &e.morphs {
            let at = morphs
                .iter_mut()
                .find(|m| m.0 == *geometry && m.1 == *morph)
                .ok_or_else(|| Error(format!("blended poses drive different morphs ({morph:?})")))?;
            at.2 += (f64::from(*v) * w) as f32;
        }
        for (name, v) in &e.controls {
            let at = controls
                .iter_mut()
                .find(|c| c.0 == *name)
                .ok_or_else(|| Error(format!("blended poses drive different controls ({name:?})")))?;
            at.1 += (f64::from(*v) * w) as f32;
        }
    }
    Ok(Evaluated {
        pose: RigPose {
            joints,
            root_applied: first.pose.root_applied,
        },
        morphs,
        controls,
        root,
        other_skeletons: used.iter().map(|(e, _)| e.other_skeletons).max().unwrap_or(0),
    })
}

/// Weighted transforms (weights summing to 1) as one: see [`blend`].
fn blend_trs(parts: &[(Trs, f64)]) -> Result<Trs> {
    let anchor = parts[0].0.rotation;
    let mut out = Trs {
        translation: [0.0; 3],
        rotation: [0.0; 4],
        scale: [0.0; 3],
    };
    for (t, w) in parts {
        for i in 0..3 {
            out.translation[i] += t.translation[i] * w;
            out.scale[i] += t.scale[i] * w;
        }
        let dot: f64 = t.rotation.iter().zip(&anchor).map(|(a, b)| a * b).sum();
        let side = if dot < 0.0 { -1.0 } else { 1.0 };
        for i in 0..4 {
            out.rotation[i] += t.rotation[i] * w * side;
        }
    }
    out.rotation = quat::normalize(out.rotation).ok_or_else(|| Error("blended rotations cancel".into()))?;
    Ok(out)
}

/// Each joint's transform into the skeleton's space under `pose` (every joint's local; a [`RigPose`] or the rests
/// from [`locals`]): its local composed with its parents' up to its root, in f64.
pub fn model_transforms(
    skeleton: &Skeleton,
    pose: &BTreeMap<JointId, Rest>,
) -> Result<BTreeMap<JointId, Affine>> {
    let parent: BTreeMap<JointId, Option<JointId>> =
        skeleton.joints.iter().map(|j| (j.id, j.parent)).collect();
    let mut out: BTreeMap<JointId, Affine> = BTreeMap::new();
    for j in &skeleton.joints {
        // the chain up to the first joint already composed (or a root), then down again
        let mut chain = vec![j.id];
        let mut at = j.parent;
        while let Some(p) = at {
            if out.contains_key(&p) {
                break;
            }
            if chain.contains(&p) || chain.len() > skeleton.joints.len() {
                return Err(Error(format!("joint {:?}: its parents form a cycle", j.id)));
            }
            chain.push(p);
            at = parent.get(&p).copied().flatten();
        }
        for id in chain.into_iter().rev() {
            if out.contains_key(&id) {
                continue;
            }
            // A [`Rest::Trs`] is composed in f64 from the rotation it stands for, made unit, and not rounded to f32 on
            // the way: a rotation stored in f32 is not exactly unit (the f32 nearest 1/sqrt(2) twice has norm squared
            // 1 - 3.4e-8) and a matrix rounded to f32 has columns of length 1 +- 6e-8, either of which a scale read back
            // from the model-space matrices (Trs::of_affine) would show as a scale the pose does not state.
            let local = match pose
                .get(&id)
                .ok_or_else(|| Error(format!("joint {id:?} has no local transform")))?
            {
                Rest::Affine(m) => affine(m),
                rest @ Rest::Trs { .. } => Trs::of(rest)
                    .map_err(|e| Error(format!("joint {id:?}: {}", e.0)))?
                    .matrix(),
            };
            let m = match parent.get(&id).copied().flatten() {
                Some(p) => compose(&out[&p], &local),
                None => local,
            };
            out.insert(id, m);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{Channel, Interpolation, Joint, RootMotion, RootSpace};

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn near(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    fn trs(t: [f32; 3], r: [f32; 4]) -> Rest {
        Rest::Trs {
            translation: t,
            rotation: r,
            scale: [1.0; 3],
        }
    }

    /// A root at (0, 1, 0) turned a quarter about z, a child 1 along the root's x.
    fn skeleton() -> Skeleton {
        Skeleton {
            joints: vec![
                Joint {
                    id: JointId(2),
                    name: "child".into(),
                    parent: Some(JointId(1)),
                    rest: trs([1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
                },
                Joint {
                    id: JointId(1),
                    name: "root".into(),
                    parent: None,
                    rest: trs([0.0, 1.0, 0.0], [0.0, 0.0, S, S]),
                },
            ],
        }
    }

    /// Hand-worked: the root's quarter turn takes the child's local (1, 0, 0) to (0, 1, 0), plus the root's (0, 1, 0):
    /// the child is at (0, 2, 0); a child listed before its parent composes the same.
    #[test]
    fn joints_compose_through_their_parents() {
        let s = skeleton();
        let m = model_transforms(&s, &locals(&s, None)).unwrap();
        assert!(near(&m[&JointId(2)][3], &[0.0, 2.0, 0.0]), "{:?}", m[&JointId(2)]);
        assert!(near(&m[&JointId(1)][3], &[0.0, 1.0, 0.0]));
    }

    /// The root's rotation is stored in f32 as (0, 0, S, S), S the f32 nearest 1/sqrt(2): its norm squared is 1 - 3.4e-8. The
    /// model-space matrices are composed from the rotation it stands for, in f64, so the scale `Trs::of_affine` reads back is
    /// the 1 the pose states to f64 rounding (the matrix of the stored quaternion taken as unit had columns 3.4e-8 short), and
    /// the rotation is the stored one made unit; the child, listed before its parent, composes through it the same.
    #[test]
    fn model_transforms_read_back_the_scale_the_pose_states() {
        let s = skeleton();
        let rests = locals(&s, None);
        let m = model_transforms(&s, &rests).unwrap();
        let root = Trs::of_affine(&m[&JointId(1)]).unwrap();
        assert!(
            root.scale.iter().all(|v| (v - 1.0).abs() < 1e-15),
            "{:?}",
            root.scale
        );
        assert!(near(&root.rotation, &[0.0, 0.0, f64::from(S), f64::from(S)]));
        let child = Trs::of_affine(&m[&JointId(2)]).unwrap();
        assert!(
            child.scale.iter().all(|v| (v - 1.0).abs() < 1e-15),
            "{:?}",
            child.scale
        );
        assert!(near(&child.translation, &[0.0, 2.0, 0.0]));
    }

    /// Hand-worked: an affine rest decomposes into its TRS (a quarter turn about z scaled 2 along its x: columns (0, 2,
    /// 0), (-1, 0, 0), (0, 0, 1)); a sheared or mirrored one does not.
    #[test]
    fn affine_rests_decompose_when_they_can() {
        let m = Transform {
            cols: [
                [0.0, 2.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [3.0, 0.0, 0.0],
            ],
        };
        let t = Trs::of(&Rest::Affine(m)).unwrap();
        assert!(near(&t.rotation, &[0.0, 0.0, f64::from(S), f64::from(S)]));
        assert!(near(&t.scale, &[2.0, 1.0, 1.0]) && near(&t.translation, &[3.0, 0.0, 0.0]));
        assert!(near(&t.matrix().concat(), &affine(&m).concat()));
        let mut sheared = Transform::IDENTITY;
        sheared.cols[1] = [0.5, 1.0, 0.0];
        assert!(Trs::of(&Rest::Affine(sheared)).is_err());
        let mut mirrored = Transform::IDENTITY;
        mirrored.cols[0] = [-1.0, 0.0, 0.0];
        assert!(Trs::of(&Rest::Affine(mirrored)).is_err());
    }

    /// A clip turning the child a half turn about z over 2 s (linear, so slerp), looping: at 1 s (and at 3 s, looped)
    /// the child's rotation is a quarter turn and its translation the rest's (only rotation is driven); its model
    /// position is the rest's, since a joint's own turn does not move it. A morph and a control are reported; root
    /// motion moves 2 along -z over the clip: -1 at 1 s.
    #[test]
    fn a_clip_replaces_only_what_it_drives() {
        let s = skeleton();
        let skel = Entity::from_index(5);
        let clip = Clip {
            name: "turn".into(),
            duration: 2.0,
            extrapolation: Extrapolation::Loop,
            channels: vec![
                Channel {
                    target: ClipTarget::Joint {
                        skeleton: skel,
                        joint: JointId(2),
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 2.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
                },
                Channel {
                    target: ClipTarget::Morph {
                        geometry: Entity::from_index(9),
                        morph: MorphId(3),
                    },
                    property: Property::Scalar,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 2.0],
                    values: vec![0.0, 1.0],
                },
                Channel {
                    target: ClipTarget::Control { name: "flip".into() },
                    property: Property::Scalar,
                    interpolation: Interpolation::Step,
                    times: vec![0.0],
                    values: vec![7.0],
                },
            ],
            root_motion: Some(RootMotion {
                space: RootSpace::Rig,
                translation: Some((
                    Interpolation::Linear,
                    vec![0.0, 2.0],
                    vec![0.0, 0.0, 0.0, 0.0, 0.0, -2.0],
                )),
                rotation: None,
            }),
            additive: None,
        };
        for t in [1.0, 3.0] {
            let e = evaluate(&clip, skel, &s, None, t).unwrap();
            let child = e.pose.joints.iter().find(|(j, _)| *j == JointId(2)).unwrap().1;
            let Rest::Trs {
                translation,
                rotation,
                ..
            } = child
            else {
                panic!("a driven joint is TRS")
            };
            assert_eq!(translation, [1.0, 0.0, 0.0]);
            assert!(
                near(&rotation.map(f64::from), &[0.0, 0.0, f64::from(S), f64::from(S)]),
                "{rotation:?}"
            );
            assert_eq!(e.morphs, [(Entity::from_index(9), MorphId(3), 0.5)]);
            assert_eq!(e.controls, [("flip".to_owned(), 7.0)]);
            assert!(near(&affine(&e.root.unwrap())[3], &[0.0, 0.0, -1.0]));
            let pose: BTreeMap<_, _> = e.pose.joints.iter().copied().collect();
            let m = model_transforms(&s, &pose).unwrap();
            assert!(near(&m[&JointId(2)][3], &[0.0, 2.0, 0.0]));
        }
        // a channel for another rig's skeleton is counted, not applied
        let e = evaluate(&clip, Entity::from_index(6), &s, None, 1.0).unwrap();
        assert_eq!(e.other_skeletons, 1);
        // an additive clip is refused, not posed at rest
        let mut additive = clip;
        additive.additive = Some(primase_core::AdditiveReference::Pose(Entity::from_index(1)));
        assert!(evaluate(&additive, skel, &s, None, 1.0).is_err());
    }

    fn joint_clip(
        skel: Entity,
        additive: Option<AdditiveReference>,
        channels: &[(Property, &[f32])],
    ) -> Clip {
        Clip {
            name: "add".into(),
            duration: 1.0,
            extrapolation: Extrapolation::Hold,
            channels: channels
                .iter()
                .map(|(property, values)| Channel {
                    target: ClipTarget::Joint {
                        skeleton: skel,
                        joint: JointId(1),
                    },
                    property: *property,
                    interpolation: Interpolation::Step,
                    times: vec![0.0],
                    values: values.to_vec(),
                })
                .collect(),
            root_motion: None,
            additive,
        }
    }

    fn root_of(e: &Evaluated) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let Rest::Trs {
            translation,
            rotation,
            scale,
        } = e.pose.joints.iter().find(|(j, _)| *j == JointId(1)).unwrap().1
        else {
            panic!("a driven joint is TRS")
        };
        (
            translation.map(f64::from).to_vec(),
            rotation.map(f64::from).to_vec(),
            scale.map(f64::from).to_vec(),
        )
    }

    /// Hand-worked, a clip of deltas (`AdditiveReference::Identity`) on the root, whose rest is (0, 1, 0), a quarter turn
    /// about z, scale 1: a translation delta (0.5, 0, 0) gives (0.5, 1, 0); a quarter turn about x composes on the right,
    /// `R_base * R_clip` = (0.5, 0.5, 0.5, 0.5) (the left product is (0.5, -0.5, 0.5, 0.5)); a scale delta of 3 gives 3. The
    /// child, which the clip does not drive, keeps its base, and so does the root's other property when one is left out.
    #[test]
    fn a_clip_of_deltas_composes_onto_the_base() {
        let s = skeleton();
        let skel = Entity::from_index(5);
        let all = joint_clip(
            skel,
            Some(AdditiveReference::Identity),
            &[
                (Property::Translation, &[0.5, 0.0, 0.0]),
                (Property::Rotation, &[S, 0.0, 0.0, S]),
                (Property::Scale, &[3.0, 3.0, 3.0]),
            ],
        );
        let e = evaluate(&all, skel, &s, None, 0.0).unwrap();
        let (t, r, sc) = root_of(&e);
        assert!(near(&t, &[0.5, 1.0, 0.0]), "{t:?}");
        assert!(near(&r, &[0.5, 0.5, 0.5, 0.5]), "{r:?}");
        assert!(near(&sc, &[3.0, 3.0, 3.0]), "{sc:?}");
        let child = e.pose.joints.iter().find(|(j, _)| *j == JointId(2)).unwrap().1;
        assert_eq!(
            child, s.joints[0].rest,
            "a joint the clip does not drive keeps its base"
        );
        // only the translation driven: the rotation and the scale are the base's
        let only = joint_clip(
            skel,
            Some(AdditiveReference::Identity),
            &[(Property::Translation, &[0.5, 0.0, 0.0])],
        );
        let (t, r, sc) = root_of(&evaluate(&only, skel, &s, None, 0.0).unwrap());
        assert!(near(&t, &[0.5, 1.0, 0.0]));
        assert!(near(&r, &[0.0, 0.0, f64::from(S), f64::from(S)]), "{r:?}");
        assert!(near(&sc, &[1.0, 1.0, 1.0]));
        // over a given base pose instead of the rests: the root at the origin, unturned
        let base = RigPose {
            joints: vec![(JointId(1), trs([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]))],
            root_applied: None,
        };
        let (t, r, _) = root_of(&evaluate(&all, skel, &s, Some(&base), 0.0).unwrap());
        assert!(near(&t, &[0.5, 0.0, 0.0]), "{t:?}");
        assert!(near(&r, &[f64::from(S), 0.0, 0.0, f64::from(S)]), "{r:?}");
    }

    /// Hand-worked, a clip relative to a pose: the base root at the origin and unturned; the reference root at (0, 3, 0),
    /// turned a quarter about z, scale 2; the clip's root at (2, 3, 0), (0.5, 0.5, 0.5, 0.5), scale 4. `T = T_base +
    /// (T_clip - T_ref)` = (2, 0, 0); `R = R_base * R_ref^-1 * R_clip` = a quarter turn about x, `(S, 0, 0, S)`; `S = S_base *
    /// S_clip / S_ref` = 2.
    #[test]
    fn a_clip_relative_to_a_pose_composes_by_the_stated_rule_and_needs_its_reference() {
        let s = skeleton();
        let skel = Entity::from_index(5);
        let clip = joint_clip(
            skel,
            Some(AdditiveReference::Pose(Entity::from_index(8))),
            &[
                (Property::Translation, &[2.0, 3.0, 0.0]),
                (Property::Rotation, &[0.5, 0.5, 0.5, 0.5]),
                (Property::Scale, &[4.0, 4.0, 4.0]),
            ],
        );
        let base = RigPose {
            joints: vec![(JointId(1), trs([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]))],
            root_applied: None,
        };
        let reference = RigPose {
            joints: vec![(
                JointId(1),
                Rest::Trs {
                    translation: [0.0, 3.0, 0.0],
                    rotation: [0.0, 0.0, S, S],
                    scale: [2.0; 3],
                },
            )],
            root_applied: None,
        };
        let e = evaluate_relative(&clip, skel, &s, Some(&base), &reference, 0.0).unwrap();
        let (t, r, sc) = root_of(&e);
        assert!(near(&t, &[2.0, 0.0, 0.0]), "{t:?}");
        assert!(near(&r, &[f64::from(S), 0.0, 0.0, f64::from(S)]), "{r:?}");
        assert!(near(&sc, &[2.0, 2.0, 2.0]), "{sc:?}");
        // the reference is the caller's to give: without it, or given to a clip with none, it is an error
        let err = evaluate(&clip, skel, &s, Some(&base), 0.0).unwrap_err();
        assert!(err.0.contains("evaluate_relative"), "{}", err.0);
        let mut plain = clip.clone();
        plain.additive = None;
        let err = evaluate_relative(&plain, skel, &s, Some(&base), &reference, 0.0).unwrap_err();
        assert!(err.0.contains("no reference to be given"), "{}", err.0);
        let mut delta = clip;
        delta.additive = Some(AdditiveReference::Identity);
        assert!(evaluate_relative(&delta, skel, &s, Some(&base), &reference, 0.0).is_err());
    }

    /// A morph or a control of an additive clip has no base to add to: an error, not a silent pass.
    #[test]
    fn an_additive_clip_with_a_morph_or_control_is_refused() {
        let s = skeleton();
        let skel = Entity::from_index(5);
        let mut clip = joint_clip(skel, Some(AdditiveReference::Identity), &[]);
        clip.channels.push(Channel {
            target: ClipTarget::Control { name: "flip".into() },
            property: Property::Scalar,
            interpolation: Interpolation::Step,
            times: vec![0.0],
            values: vec![1.0],
        });
        let err = evaluate(&clip, skel, &s, None, 0.0).unwrap_err();
        assert!(err.0.contains("no base to add to"), "{}", err.0);
    }

    fn evaluated(joints: Vec<(JointId, Rest)>) -> Evaluated {
        Evaluated {
            pose: RigPose {
                joints,
                root_applied: None,
            },
            morphs: vec![],
            controls: vec![],
            root: None,
            other_skeletons: 0,
        }
    }

    fn joint_trs(e: &Evaluated, id: u32) -> Trs {
        Trs::of(&e.pose.joints.iter().find(|(j, _)| *j == JointId(id)).unwrap().1).unwrap()
    }

    /// Hand-worked: weights 1 and 3 put a joint a quarter of the way from (0, 0, 0) to (4, 0, 0) and from scale 1 to 5; a
    /// quarter turn and no turn about z at weights 1 and 1 are an eighth of a turn (and the same with the quarter turn's
    /// quaternion negated: `q` and `-q` are one rotation); weights are relative (2 and 2 are 1 and 1); a joint both parts
    /// state alike is kept as stated; a part alone is itself.
    #[test]
    fn weighted_poses_blend_joint_by_joint() {
        let same = trs([5.0, 6.0, 7.0], [0.0, 0.0, 0.0, 1.0]);
        let a = evaluated(vec![
            (JointId(1), trs([0.0; 3], [0.0, 0.0, 0.0, 1.0])),
            (JointId(2), same),
        ]);
        let turned = |rotation: [f32; 4]| Rest::Trs {
            translation: [4.0, 0.0, 0.0],
            rotation,
            scale: [5.0, 5.0, 5.0],
        };
        let b = evaluated(vec![(JointId(1), turned([0.0, 0.0, S, S])), (JointId(2), same)]);
        let at = |wa: f64, wb: f64| blend(&[(&a, wa), (&b, wb)]).unwrap();
        let t = joint_trs(&at(1.0, 3.0), 1);
        assert!(
            near(&t.translation, &[3.0, 0.0, 0.0]) && near(&t.scale, &[4.0; 3]),
            "{t:?}"
        );
        let eighth = [
            0.0,
            0.0,
            (std::f64::consts::PI / 8.0).sin(),
            (std::f64::consts::PI / 8.0).cos(),
        ];
        assert!(near(&joint_trs(&at(1.0, 1.0), 1).rotation, &eighth));
        // the second rotation negated: the same rotation, the same blend
        let mut negated = b.clone();
        negated.pose.joints[0].1 = turned([0.0, 0.0, -S, -S]);
        let n = blend(&[(&a, 1.0), (&negated, 1.0)]).unwrap();
        assert!(near(&joint_trs(&n, 1).rotation, &eighth));
        assert_eq!(at(2.0, 2.0), at(1.0, 1.0));
        assert_eq!(at(1.0, 1.0).pose.joints[1], a.pose.joints[1]);
        assert_eq!(at(0.0, 2.0), b);
    }

    /// Morphs, controls and root motion are weighted too; poses of different shapes or of weights that mean nothing are
    /// refused; a part of weight 0 is not asked to match.
    #[test]
    fn blends_weight_morphs_and_refuse_what_does_not_match() {
        let g = Entity::from_index(9);
        let still = trs([0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        let mut a = evaluated(vec![(JointId(1), still)]);
        let mut b = a.clone();
        a.morphs = vec![(g, MorphId(3), 0.0)];
        b.morphs = vec![(g, MorphId(3), 1.0)];
        a.controls = vec![("flip".into(), 2.0)];
        b.controls = vec![("flip".into(), 4.0)];
        let root = |z: f64| {
            transform(
                &Trs {
                    translation: [0.0, 0.0, z],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                }
                .matrix(),
            )
        };
        a.root = Some(root(0.0));
        b.root = Some(root(-2.0));
        let m = blend(&[(&a, 1.0), (&b, 1.0)]).unwrap();
        assert_eq!(m.morphs, [(g, MorphId(3), 0.5)]);
        assert_eq!(m.controls, [("flip".to_owned(), 3.0)]);
        assert!(near(&affine(&m.root.unwrap())[3], &[0.0, 0.0, -1.0]));
        assert!(blend(&[]).is_err());
        assert!(blend(&[(&a, 0.0)]).is_err());
        assert!(blend(&[(&a, -1.0), (&b, 2.0)]).is_err());
        assert!(blend(&[(&a, f64::NAN)]).is_err());
        let mut other = b.clone();
        other.pose.joints[0].0 = JointId(2);
        assert!(blend(&[(&a, 1.0), (&other, 1.0)]).is_err());
        assert_eq!(blend(&[(&a, 1.0), (&other, 0.0)]).unwrap(), a);
        let mut no_root = b.clone();
        no_root.root = None;
        assert!(blend(&[(&a, 1.0), (&no_root, 1.0)]).is_err());
        let mut other_morph = b.clone();
        other_morph.morphs = vec![(g, MorphId(4), 1.0)];
        assert!(blend(&[(&a, 1.0), (&other_morph, 1.0)]).is_err());
        // two opposite quaternions are one rotation: their blend is that rotation (the sum would otherwise cancel)
        let x = |q: [f32; 4]| {
            evaluated(vec![(
                JointId(1),
                Rest::Trs {
                    translation: [0.0; 3],
                    rotation: q,
                    scale: [1.0; 3],
                },
            )])
        };
        let (p, q) = (x([1.0, 0.0, 0.0, 0.0]), x([-1.0, 0.0, 0.0, 0.0]));
        let r = joint_trs(&blend(&[(&p, 1.0), (&q, 1.0)]).unwrap(), 1).rotation;
        assert!(near(&r, &[1.0, 0.0, 0.0, 0.0]), "{r:?}");
    }
}
