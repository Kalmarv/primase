//! `retarget-clip`: clips moved onto a skeleton of other proportions and another rest pose, an approximation that
//! states its choices and reports them (the exact case is `map-clip`).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Channel, Clip, ClipBinding, ClipTarget, Component, Entity, Interpolation, JointId, JointMap, Property,
    Rest, RigInstance, Skeleton, World,
};

use crate::pose::{Affine, Trs, affine, compose, evaluate, inverse, locals, model_transforms};
use crate::quat;

/// Which poses of the two skeletons are taken to be the same pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// Both rests (they must be alike: both T, or both A).
    Rest,
    /// The source's rest, and the target's rest with each mapped joint that has one mapped joint below it turned (the
    /// least rotation) so that bone points the way the source's does: a built pose, reported.
    Aligned,
}

/// What the target's root does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Translation {
    /// The root-most mapped joint moves as the source's does from its reference, scaled by the ratio of their
    /// reference heights.
    Root,
    /// No joint moves: every target joint keeps its rest translation.
    None,
}

/// `retarget-clip{reference=rest|aligned,translation=root|none}` (both required): every `core.clip` whose joint
/// channels drive the `from` skeleton of a `core.joint_map` drives its `to` skeleton instead, as motion transferred by
/// rotation. At each key time of the clip, the source is posed (its rest under the clip) and each mapped target joint
/// is turned in the model's space by what turned its source joint from its reference (`R_t = R_s * R_s,ref^-1 *
/// R_t,ref`), its bone keeping the target's length; joints the map does not list keep their rests; the root moves as
/// `translation` says. The result is keyed at the clip's key times (a rotation channel a mapped joint, a translation
/// channel for the root), linear between keys. Both skeletons must face the same way in the model's space (a program
/// turns one first). Additive clips are refused; a clip's root motion and its morph and control channels are kept.
///
/// Approximations, reported: the transfer itself (a body of other proportions does not make the same motion: hands
/// meet where the source's do only if the arms are alike), the resampling (between keys the result is slerped, which
/// the source's own evaluation is not), and with `reference=aligned`, the built target reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetargetClip {
    pub reference: Reference,
    pub translation: Translation,
}

/// The pass factory.
pub fn retarget_clip(o: &Options) -> Result<Box<dyn Pass>, String> {
    let (mut reference, mut translation) = (None, None);
    for (k, v) in o {
        match k.as_str() {
            "reference" => {
                reference = Some(match v.as_str() {
                    "rest" => Reference::Rest,
                    "aligned" => Reference::Aligned,
                    _ => return Err(format!("reference={v}: rest or aligned")),
                });
            }
            "translation" => {
                translation = Some(match v.as_str() {
                    "root" => Translation::Root,
                    "none" => Translation::None,
                    _ => return Err(format!("translation={v}: root or none")),
                });
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(RetargetClip {
        reference: reference.ok_or("reference=rest|aligned is required")?,
        translation: translation.ok_or("translation=root|none is required")?,
    }))
}

/// The rotation part of an affine map with no shear (its columns normalised).
pub(crate) fn rotation(m: &Affine) -> [f64; 4] {
    let len = |c: &[f64; 3]| c.iter().map(|x| x * x).sum::<f64>().sqrt();
    let u: [[f64; 3]; 3] = std::array::from_fn(|i| {
        let l = len(&m[i]);
        m[i].map(|x| x / l)
    });
    quat::from_columns(u)
}

/// The rotation matrix of a unit quaternion, as an affine map's linear part.
fn rotation_matrix(q: [f64; 4]) -> [[f64; 3]; 3] {
    let m = Trs {
        translation: [0.0; 3],
        rotation: q,
        scale: [1.0; 3],
    }
    .matrix();
    [m[0], m[1], m[2]]
}

/// `q` applied to `v`.
pub(crate) fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let m = rotation_matrix(q);
    std::array::from_fn(|r| (0..3).map(|c| m[c][r] * v[c]).sum())
}

/// The least rotation taking direction `a` to direction `b` (none when either is zero).
pub(crate) fn between(a: [f64; 3], b: [f64; 3]) -> [f64; 4] {
    let n = |v: [f64; 3]| {
        let l = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        (l > 1e-12).then(|| v.map(|x| x / l))
    };
    let (Some(a), Some(b)) = (n(a), n(b)) else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if d < -1.0 + 1e-12 {
        // opposite: half a turn about any axis at right angles to a
        let axis = if a[0].abs() < 0.9 {
            [0.0, -a[2], a[1]]
        } else {
            [a[2], 0.0, -a[0]]
        };
        let axis = n(axis).unwrap_or([0.0, 1.0, 0.0]);
        return [axis[0], axis[1], axis[2], 0.0];
    }
    quat::normalize([c[0], c[1], c[2], 1.0 + d]).unwrap_or([0.0, 0.0, 0.0, 1.0])
}

/// An affine map from a translation, a rotation and the scale columns of `scale_of`.
pub(crate) fn with_rotation(translation: [f64; 3], q: [f64; 4], scale_of: &Affine) -> Affine {
    let len = |c: &[f64; 3]| c.iter().map(|x| x * x).sum::<f64>().sqrt();
    let r = rotation_matrix(q);
    let mut m: Affine = [[0.0; 3]; 4];
    for (i, col) in r.iter().enumerate() {
        let s = len(&scale_of[i]);
        m[i] = col.map(|x| x * s);
    }
    m[3] = translation;
    m
}

/// Target joints in an order where every parent comes before its children.
pub(crate) fn parents_first(s: &Skeleton) -> Result<Vec<usize>, String> {
    let index: BTreeMap<JointId, usize> = s.joints.iter().enumerate().map(|(i, j)| (j.id, i)).collect();
    let mut order = Vec::with_capacity(s.joints.len());
    let mut done = vec![false; s.joints.len()];
    while order.len() < s.joints.len() {
        let before = order.len();
        for (i, j) in s.joints.iter().enumerate() {
            if !done[i] && j.parent.is_none_or(|p| index.get(&p).is_some_and(|&k| done[k])) {
                done[i] = true;
                order.push(i);
            }
        }
        if order.len() == before {
            return Err("the skeleton's parents form a cycle".into());
        }
    }
    Ok(order)
}

/// The target's reference locals (see [`Reference`]).
fn reference_locals(
    reference: Reference,
    source_ref: &BTreeMap<JointId, Affine>,
    target: &Skeleton,
    pairs: &BTreeMap<JointId, JointId>,
) -> Result<BTreeMap<JointId, Affine>, String> {
    let mut local: BTreeMap<JointId, Affine> = target
        .joints
        .iter()
        .map(|j| (j.id, affine(&j.rest.matrix())))
        .collect();
    if reference == Reference::Rest {
        return Ok(local);
    }
    let back: BTreeMap<JointId, JointId> = pairs.iter().map(|(a, b)| (*b, *a)).collect();
    let parent: BTreeMap<JointId, Option<JointId>> = target.joints.iter().map(|j| (j.id, j.parent)).collect();
    // each mapped target joint's mapped joints below it with no mapped joint between
    let mut below: BTreeMap<JointId, Vec<JointId>> = BTreeMap::new();
    for j in target.joints.iter().filter(|j| back.contains_key(&j.id)) {
        let mut at = parent[&j.id];
        while let Some(p) = at {
            if back.contains_key(&p) {
                below.entry(p).or_default().push(j.id);
                break;
            }
            at = parent[&p];
        }
    }
    let order = parents_first(target)?;
    let mut model: BTreeMap<JointId, Affine> = BTreeMap::new();
    for i in order {
        let j = &target.joints[i];
        let base = |m: &BTreeMap<JointId, Affine>| j.parent.map(|p| m[&p]);
        let mut m = match base(&model) {
            Some(p) => compose(&p, &local[&j.id]),
            None => local[&j.id],
        };
        if let (Some(&src), Some([child])) = (back.get(&j.id), below.get(&j.id).map(Vec::as_slice)) {
            // the child's offset in this joint's space, through the rests between them
            let mut chain: Affine = affine(&primase_core::Transform::IDENTITY);
            let mut at = *child;
            while at != j.id {
                chain = compose(&local[&at], &chain);
                at = parent[&at].ok_or("a mapped joint below another is not under it")?;
            }
            let offset = rotate(rotation(&m), chain[3]);
            let src_child = back[child];
            let want: [f64; 3] =
                std::array::from_fn(|k| source_ref[&src_child][3][k] - source_ref[&src][3][k]);
            let turn = between(offset, want);
            m = with_rotation(m[3], quat::mul(turn, rotation(&m)), &m);
            let p = base(&model);
            let l = match p {
                Some(p) => compose(
                    &inverse(&p).ok_or("a joint's model transform has no inverse")?,
                    &m,
                ),
                None => m,
            };
            local.insert(j.id, l);
        }
        model.insert(j.id, m);
    }
    Ok(local)
}

impl Pass for RetargetClip {
    fn name(&self) -> &'static str {
        "retarget-clip"
    }
    fn options(&self) -> Options {
        let reference = match self.reference {
            Reference::Rest => "rest",
            Reference::Aligned => "aligned",
        };
        let translation = match self.translation {
            Translation::Root => "root",
            Translation::None => "none",
        };
        Options::from([
            ("reference".to_owned(), reference.to_owned()),
            ("translation".to_owned(), translation.to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Clip::NAME, JointMap::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Clip::NAME]
    }
    fn approximates(&self) -> &'static str {
        "motion transferred by rotation onto a body of other proportions; resampled at the clip's key times"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let maps: Vec<JointMap> = w.iter::<JointMap>().map(|(_, m)| m.clone()).collect();
        let bound: Vec<(Entity, Entity)> = w
            .iter::<ClipBinding>()
            .filter_map(|(_, b)| b.rig.map(|r| (b.clip, r)))
            .collect();
        let clips: Vec<Entity> = w.iter::<Clip>().map(|(e, _)| e).collect();
        for c in clips {
            let clip = w.get::<Clip>(c).ok_or("a clip is gone")?.clone();
            let skeletons: BTreeSet<Entity> = clip
                .channels
                .iter()
                .filter_map(|ch| match ch.target {
                    ClipTarget::Joint { skeleton, .. } => Some(skeleton),
                    _ => None,
                })
                .collect();
            let candidates: Vec<&JointMap> = maps.iter().filter(|m| skeletons.contains(&m.from)).collect();
            let map = match candidates.as_slice() {
                [] => continue,
                [m] => *m,
                _ => return Err(format!("clip {c:?}: several joint maps from its skeleton")),
            };
            if skeletons.len() > 1 {
                return Err(format!("clip {c:?} drives several skeletons"));
            }
            if clip.additive.is_some() {
                return Err(format!("clip {c:?} is additive: not retargeted"));
            }
            if bound.iter().any(|(clip, rig)| {
                *clip == c && w.get::<RigInstance>(*rig).is_some_and(|r| r.skeleton == map.from)
            }) {
                return Err(format!(
                    "clip {c:?} is bound to a rig of the source skeleton: retarget, then bind"
                ));
            }
            let source = w
                .get::<Skeleton>(map.from)
                .ok_or("the map's source is no skeleton")?;
            let target = w
                .get::<Skeleton>(map.to)
                .ok_or("the map's target is no skeleton")?;
            let pairs: BTreeMap<JointId, JointId> = map.pairs.iter().copied().collect();
            let source_ref = model_transforms(source, &locals(source, None)).map_err(|e| e.0)?;
            let target_ref_local = reference_locals(self.reference, &source_ref, target, &pairs)?;
            let target_ref = {
                let rests: BTreeMap<JointId, Rest> = target_ref_local
                    .iter()
                    .map(|(j, m)| (*j, Rest::Affine(crate::pose::transform(m))))
                    .collect();
                model_transforms(target, &rests).map_err(|e| e.0)?
            };
            let order = parents_first(target)?;
            // the root-most mapped joint, and its reference heights
            let root = order
                .iter()
                .map(|&i| target.joints[i].id)
                .find(|j| pairs.values().any(|t| t == j));
            let back: BTreeMap<JointId, JointId> = pairs.iter().map(|(a, b)| (*b, *a)).collect();
            let scale = match (self.translation, root) {
                (Translation::Root, Some(r)) => {
                    let (s, t) = (source_ref[&back[&r]][3][1], target_ref[&r][3][1]);
                    if s.abs() < 1e-9 {
                        return Err("the source root's reference height is 0: no scale for its motion".into());
                    }
                    t / s
                }
                _ => 0.0,
            };
            // key times: every key of the clip
            let mut times: Vec<f32> = clip
                .channels
                .iter()
                .flat_map(|ch| ch.times.iter().copied())
                .collect();
            times.sort_by(f32::total_cmp);
            times.dedup();
            let mut rotations: BTreeMap<JointId, Vec<f32>> = BTreeMap::new();
            let mut root_translation: Vec<f32> = Vec::new();
            for &t in &times {
                let posed = evaluate(&clip, map.from, source, None, f64::from(t)).map_err(|e| e.0)?;
                let src = model_transforms(source, &locals(source, Some(&posed.pose))).map_err(|e| e.0)?;
                let mut model: BTreeMap<JointId, Affine> = BTreeMap::new();
                for &i in &order {
                    let j = &target.joints[i];
                    let parent = j.parent.map(|p| model[&p]);
                    let rest_model = match parent {
                        Some(p) => compose(&p, &target_ref_local[&j.id]),
                        None => target_ref_local[&j.id],
                    };
                    let Some(&s) = back.get(&j.id) else {
                        model.insert(j.id, rest_model);
                        continue;
                    };
                    let turn = quat::mul(rotation(&src[&s]), quat::inverse(rotation(&source_ref[&s])));
                    let q = quat::mul(turn, rotation(&target_ref[&j.id]));
                    let mut position = rest_model[3];
                    if Some(j.id) == root && self.translation == Translation::Root {
                        position = std::array::from_fn(|k| {
                            target_ref[&j.id][3][k] + scale * (src[&s][3][k] - source_ref[&s][3][k])
                        });
                    }
                    let m = with_rotation(position, q, &rest_model);
                    let l = match parent {
                        Some(p) => compose(
                            &inverse(&p).ok_or("a joint's model transform has no inverse")?,
                            &m,
                        ),
                        None => m,
                    };
                    let trs = Trs::of(&Rest::Affine(crate::pose::transform(&l))).map_err(|e| e.0)?;
                    rotations
                        .entry(j.id)
                        .or_default()
                        .extend(trs.rotation.map(|x| x as f32));
                    if Some(j.id) == root && self.translation == Translation::Root {
                        root_translation.extend(trs.translation.map(|x| x as f32));
                    }
                    model.insert(j.id, m);
                }
            }
            let mut channels: Vec<Channel> = clip
                .channels
                .iter()
                .filter(|ch| !matches!(ch.target, ClipTarget::Joint { .. }))
                .cloned()
                .collect();
            for (j, values) in rotations {
                channels.push(Channel {
                    target: ClipTarget::Joint {
                        skeleton: map.to,
                        joint: j,
                    },
                    property: Property::Rotation,
                    interpolation: Interpolation::Linear,
                    times: times.clone(),
                    values,
                });
            }
            if let (Some(r), false) = (root, root_translation.is_empty()) {
                channels.push(Channel {
                    target: ClipTarget::Joint {
                        skeleton: map.to,
                        joint: r,
                    },
                    property: Property::Translation,
                    interpolation: Interpolation::Linear,
                    times: times.clone(),
                    values: root_translation,
                });
            }
            let mapped_joints = pairs.len() as u64;
            let mut out = clip;
            out.channels = channels;
            w.insert(c, out);
            log.approximate("clips retargeted onto a skeleton of other proportions", 1);
            log.approximate(
                "retargeted joint keys, slerped between the clip's key times",
                mapped_joints * times.len() as u64,
            );
            if self.reference == Reference::Aligned {
                log.approximate(
                    "target reference poses built by aligning mapped bones to the source's",
                    1,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{AdditiveReference, Extrapolation, Joint};

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn trs(t: [f32; 3], q: [f32; 4]) -> Rest {
        Rest::Trs {
            translation: t,
            rotation: q,
            scale: [1.0; 3],
        }
    }

    fn joint(id: u32, parent: Option<u32>, rest: Rest) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest,
        }
    }

    /// Source: a root at y=1 and an arm along +x (a T-pose). Target, taller: a root at y=2 and an arm turned 45
    /// degrees down (an A-pose), its bone 0.6 long. A clip raising the source's arm a quarter turn about z and moving
    /// its root one unit along z, both over a second: (world, target skeleton, clip).
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let id = [0.0, 0.0, 0.0, 1.0];
        let source = w.spawn();
        w.insert(
            source,
            Skeleton {
                joints: vec![
                    joint(1, None, trs([0.0, 1.0, 0.0], id)),
                    joint(2, Some(1), trs([0.2, 0.5, 0.0], id)),
                    joint(3, Some(2), trs([0.5, 0.0, 0.0], id)),
                ],
            },
        );
        let (s8, c8) = (
            (std::f32::consts::PI / 8.0).sin(),
            (std::f32::consts::PI / 8.0).cos(),
        );
        let target = w.spawn();
        w.insert(
            target,
            Skeleton {
                joints: vec![
                    joint(10, None, trs([0.0, 2.0, 0.0], id)),
                    joint(20, Some(10), trs([0.3, 0.6, 0.0], [0.0, 0.0, -s8, c8])),
                    joint(30, Some(20), trs([0.6, 0.0, 0.0], id)),
                ],
            },
        );
        let m = w.spawn();
        w.insert(
            m,
            JointMap {
                from: source,
                to: target,
                pairs: vec![
                    (JointId(1), JointId(10)),
                    (JointId(2), JointId(20)),
                    (JointId(3), JointId(30)),
                ],
            },
        );
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "raise".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![
                    Channel {
                        target: ClipTarget::Joint {
                            skeleton: source,
                            joint: JointId(2),
                        },
                        property: Property::Rotation,
                        interpolation: Interpolation::Linear,
                        times: vec![0.0, 1.0],
                        values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S],
                    },
                    Channel {
                        target: ClipTarget::Joint {
                            skeleton: source,
                            joint: JointId(1),
                        },
                        property: Property::Translation,
                        interpolation: Interpolation::Linear,
                        times: vec![0.0, 1.0],
                        values: vec![0.0, 1.0, 0.0, 0.0, 1.0, 1.0],
                    },
                ],
                root_motion: None,
                additive: None,
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        (w, target, clip)
    }

    fn run(w: &mut World, pipeline: &str) -> Result<RunReport, String> {
        let mut r = PassRegistry::new();
        crate::register(&mut r);
        let m = PassManager::new(r).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(
            w,
            &m.registry.parse(pipeline).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
        Ok(report)
    }

    /// The target posed by the retargeted clip at `t`: (root position, arm direction).
    fn posed(w: &World, target: Entity, clip: Entity, t: f64) -> ([f64; 3], [f64; 3]) {
        let s = w.get::<Skeleton>(target).unwrap();
        let e = evaluate(w.get::<Clip>(clip).unwrap(), target, s, None, t).unwrap();
        let m = model_transforms(s, &locals(s, Some(&e.pose))).unwrap();
        let (a, b) = (m[&JointId(20)][3], m[&JointId(30)][3]);
        let d: [f64; 3] = std::array::from_fn(|k| b[k] - a[k]);
        let l = d.iter().map(|x| x * x).sum::<f64>().sqrt();
        (m[&JointId(10)][3], d.map(|x| x / l))
    }

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// Aligned references: the target arm starts level like the source arm and rises with it to straight up; the
    /// target root moves twice the source root distance (it stands twice as high); the approximations are reported.
    #[test]
    fn aligned_references_make_the_target_move_as_the_source() {
        let (mut w, target, clip) = world();
        let report = run(&mut w, "retarget-clip{reference=aligned,translation=root}").unwrap();
        let (root0, arm0) = posed(&w, target, clip, 0.0);
        let (root1, arm1) = posed(&w, target, clip, 1.0);
        assert!(near(arm0, [1.0, 0.0, 0.0]), "{arm0:?}");
        assert!(near(arm1, [0.0, 1.0, 0.0]), "{arm1:?}");
        assert!(near(root0, [0.0, 2.0, 0.0]), "{root0:?}");
        assert!(near(root1, [0.0, 2.0, 2.0]), "{root1:?}");
        let log = &report.steps[0].log;
        assert_eq!(
            log.approximations["clips retargeted onto a skeleton of other proportions"],
            1
        );
        assert_eq!(
            log.approximations["target reference poses built by aligning mapped bones to the source's"],
            1
        );
        let c = w.get::<Clip>(clip).unwrap();
        assert!(
            c.channels
                .iter()
                .all(|ch| matches!(ch.target, ClipTarget::Joint { skeleton, .. } if skeleton == target))
        );
    }

    /// The rests as references: the arm keeps its 45 degrees from the source arm, so the quarter turn takes it from
    /// down-right to up-right; without root translation the root stays.
    #[test]
    fn rest_references_keep_the_target_offsets() {
        let (mut w, target, clip) = world();
        run(&mut w, "retarget-clip{reference=rest,translation=none}").unwrap();
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let (root0, arm0) = posed(&w, target, clip, 0.0);
        let (root1, arm1) = posed(&w, target, clip, 1.0);
        assert!(near(arm0, [h, -h, 0.0]), "{arm0:?}");
        assert!(near(arm1, [h, h, 0.0]), "{arm1:?}");
        assert!(near(root0, [0.0, 2.0, 0.0]) && near(root1, [0.0, 2.0, 0.0]));
    }

    #[test]
    fn retarget_clip_refuses_what_it_does_not_do() {
        let (mut w, _, clip) = world();
        w.get_mut::<Clip>(clip).unwrap().additive = Some(AdditiveReference::ClipFrame { clip, time: 0.0 });
        let err = run(&mut w, "retarget-clip{reference=rest,translation=none}").unwrap_err();
        assert!(err.contains("additive"), "{err}");
        let (mut w, ..) = world();
        for (bad, why) in [
            (
                "retarget-clip{reference=rest}",
                "translation=root|none is required",
            ),
            (
                "retarget-clip{translation=root}",
                "reference=rest|aligned is required",
            ),
            ("retarget-clip{reference=t,translation=root}", "rest or aligned"),
        ] {
            let err = run(&mut w, bad).unwrap_err();
            assert!(err.contains(why), "{bad}: {err}");
        }
    }
}
