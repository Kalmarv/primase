//! Poses: a clip evaluated at a time on a skeleton (`core.clip` on `core.skeleton`, over a base pose), and a pose's
//! joints composed through their parents into the skeleton's space.

use std::collections::BTreeMap;

use primase_core::{
    Clip, ClipTarget, Entity, Extrapolation, JointId, MorphId, Property, Rest, RigPose, Skeleton, Transform,
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
            Rest::Affine(m) => {
                let c = affine(&m);
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
        }
    }

    /// `T * R * S`.
    pub fn matrix(&self) -> Affine {
        let [x, y, z, w] = self.rotation;
        let r = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y + z * w),
                2.0 * (x * z - y * w),
            ],
            [
                2.0 * (x * y - z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z + x * w),
            ],
            [
                2.0 * (x * z + y * w),
                2.0 * (y * z - x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
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
/// replaces its joint's translation, rotation or scale (a joint it drives must have a TRS form, [`Trs::of`]). An
/// additive clip is not evaluated here (an error, never a silent rest pose).
pub fn evaluate(
    clip: &Clip,
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    base: Option<&RigPose>,
    t: f64,
) -> Result<Evaluated> {
    if clip.additive.is_some() {
        return Err(Error(format!("clip {:?} is additive: not evaluated", clip.name)));
    }
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
                match c.property {
                    Property::Translation => trs.translation = [v[0], v[1], v[2]],
                    Property::Rotation => trs.rotation = [v[0], v[1], v[2], v[3]],
                    Property::Scale => trs.scale = [v[0], v[1], v[2]],
                    Property::Scalar => return Err(Error("a scalar channel on a joint".into())),
                }
                driven.insert(*joint, trs);
            }
            ClipTarget::Joint { .. } => out.other_skeletons += 1,
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
            let local = pose
                .get(&id)
                .ok_or_else(|| Error(format!("joint {id:?} has no local transform")))?
                .matrix();
            let m = match parent.get(&id).copied().flatten() {
                Some(p) => compose(&out[&p], &affine(&local)),
                None => affine(&local),
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
}
