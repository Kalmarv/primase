//! `map-clip`: clips made to drive another skeleton through a `core.joint_map`, where that is exact.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Channel, Clip, ClipBinding, ClipTarget, Component, Entity, Interpolation, JointId, JointMap, Property,
    Rest, RigInstance, Skeleton, World,
};

use crate::pose::Trs;

/// What `map-clip` does with a channel on a joint the map does not list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmapped {
    /// Refuse the clip, naming the joint.
    Refuse,
    /// Leave the channel out, counted.
    Drop,
}

/// Whose rests the mapped joints the clip does not drive take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rests {
    /// The target skeleton's own: the clip plays on the target as on any skeleton.
    Target,
    /// The source skeleton's, as constant channels where they differ: the mapped joints compose to the source's
    /// model-space pose exactly (the target follows the source).
    Source,
}

/// `map-clip{unmapped=refuse|drop,rests=target|source}` (both required): every `core.clip` whose joint channels drive
/// the `from` skeleton of a `core.joint_map` drives its `to` skeleton instead: each joint channel's target becomes the
/// mapped joint, its values unchanged. A joint channel replaces its joint's local transform in its parent's space, so
/// each mapped joint's target must have the target of its source's parent as its parent (a root's, none): refused
/// otherwise (a different hierarchy needs retargeting). With `rests=source`, each mapped joint the clip does not drive
/// whose source rest differs from its target's (beyond 1e-6) gets that source rest as constant channels (one key at 0,
/// step; only the parts that differ), so every mapped joint composes to the source skeleton's model-space pose exactly,
/// and the hierarchy is checked for every pair; with `rests=target` the target's rests stand and only the driven
/// joints' hierarchy is checked. Channels on joints the map does not list are refused or left out (`unmapped`). Morph
/// and control channels are kept. Two maps from one skeleton are refused (which one is meant is not stated), and so is
/// a clip bound (`core.clip_binding`) to a rig of the source skeleton: map, then bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapClip {
    pub unmapped: Unmapped,
    pub rests: Rests,
}

/// The pass factory.
pub fn map_clip(o: &Options) -> Result<Box<dyn Pass>, String> {
    let (mut unmapped, mut rests) = (None, None);
    for (k, v) in o {
        match k.as_str() {
            "unmapped" => {
                unmapped = Some(match v.as_str() {
                    "refuse" => Unmapped::Refuse,
                    "drop" => Unmapped::Drop,
                    _ => return Err(format!("unmapped={v}: refuse or drop")),
                });
            }
            "rests" => {
                rests = Some(match v.as_str() {
                    "target" => Rests::Target,
                    "source" => Rests::Source,
                    _ => return Err(format!("rests={v}: target or source")),
                });
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(MapClip {
        unmapped: unmapped.ok_or("unmapped=refuse|drop is required")?,
        rests: rests.ok_or("rests=target|source is required")?,
    }))
}

/// A joint map as lookups, with its skeletons' parents and rests.
struct Map {
    to: Entity,
    pairs: BTreeMap<JointId, JointId>,
    from_parent: BTreeMap<JointId, Option<JointId>>,
    to_parent: BTreeMap<JointId, Option<JointId>>,
    from_rest: BTreeMap<JointId, Rest>,
    to_rest: BTreeMap<JointId, Rest>,
}

impl Map {
    /// Whether `joint`'s target has the target of its parent as its parent.
    fn corresponds(&self, joint: JointId, target: JointId) -> bool {
        match self.from_parent[&joint] {
            None => self.to_parent[&target].is_none(),
            Some(p) => self
                .pairs
                .get(&p)
                .is_some_and(|&q| self.to_parent[&target] == Some(q)),
        }
    }
}

/// How far apart two TRS parts are (a rotation and its negation are one rotation).
fn apart(a: &[f64], b: &[f64], rotation: bool) -> f64 {
    let d = |sign: f64| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - sign * y).abs())
            .fold(0.0, f64::max)
    };
    if rotation { d(1.0).min(d(-1.0)) } else { d(1.0) }
}

const REST_TOLERANCE: f64 = 1e-6;

fn refuse(joint: JointId, target: JointId) -> String {
    format!(
        "joint {joint:?} -> {target:?}: its parent does not map to the target's parent (a different hierarchy: \
         retarget instead)"
    )
}

impl Pass for MapClip {
    fn name(&self) -> &'static str {
        "map-clip"
    }
    fn options(&self) -> Options {
        let unmapped = match self.unmapped {
            Unmapped::Refuse => "refuse",
            Unmapped::Drop => "drop",
        };
        let rests = match self.rests {
            Rests::Target => "target",
            Rests::Source => "source",
        };
        Options::from([
            ("unmapped".to_owned(), unmapped.to_owned()),
            ("rests".to_owned(), rests.to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Clip::NAME, JointMap::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Clip::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut maps: BTreeMap<Entity, Map> = BTreeMap::new();
        for (e, m) in w.iter::<JointMap>() {
            let from = w
                .get::<Skeleton>(m.from)
                .ok_or("a joint map's source is no skeleton")?;
            let to = w
                .get::<Skeleton>(m.to)
                .ok_or("a joint map's target is no skeleton")?;
            let map = Map {
                to: m.to,
                pairs: m.pairs.iter().copied().collect(),
                from_parent: from.joints.iter().map(|j| (j.id, j.parent)).collect(),
                to_parent: to.joints.iter().map(|j| (j.id, j.parent)).collect(),
                from_rest: from.joints.iter().map(|j| (j.id, j.rest)).collect(),
                to_rest: to.joints.iter().map(|j| (j.id, j.rest)).collect(),
            };
            if maps.insert(m.from, map).is_some() {
                return Err(format!(
                    "two joint maps from skeleton {:?} ({e:?} and another): which is meant is not stated",
                    m.from
                ));
            }
        }
        let bound: Vec<(Entity, Entity)> = w
            .iter::<ClipBinding>()
            .filter_map(|(_, b)| b.rig.map(|r| (b.clip, r)))
            .collect();
        let clips: Vec<Entity> = w.iter::<Clip>().map(|(e, _)| e).collect();
        for c in clips {
            let clip = w.get::<Clip>(c).ok_or("a clip is gone")?;
            let skeletons: BTreeSet<Entity> = clip
                .channels
                .iter()
                .filter_map(|ch| match ch.target {
                    ClipTarget::Joint { skeleton, .. } => Some(skeleton),
                    _ => None,
                })
                .collect();
            let Some((from, map)) = skeletons.iter().find_map(|s| maps.get(s).map(|m| (*s, m))) else {
                continue;
            };
            if skeletons.len() > 1 {
                return Err(format!("clip {c:?} drives several skeletons"));
            }
            if let Some(&(_, rig)) = bound.iter().find(|(clip, rig)| {
                *clip == c && w.get::<RigInstance>(*rig).is_some_and(|r| r.skeleton == from)
            }) {
                return Err(format!(
                    "clip {c:?} is bound to rig {rig:?} of the source skeleton: map the clip, then bind it"
                ));
            }
            if self.rests == Rests::Source
                && let Some((&a, &b)) = map.pairs.iter().find(|(a, b)| !map.corresponds(**a, **b))
            {
                return Err(refuse(a, b));
            }
            let mut mapped = clip.clone();
            let mut dropped = 0u64;
            let mut channels = Vec::with_capacity(mapped.channels.len());
            let mut driven: BTreeSet<(JointId, Property)> = BTreeSet::new();
            for mut ch in mapped.channels {
                if let ClipTarget::Joint { joint, .. } = ch.target {
                    let Some(&target) = map.pairs.get(&joint) else {
                        match self.unmapped {
                            Unmapped::Refuse => {
                                return Err(format!(
                                    "clip {c:?} drives joint {joint:?}, which the map does not list"
                                ));
                            }
                            Unmapped::Drop => {
                                dropped += 1;
                                continue;
                            }
                        }
                    };
                    if !map.corresponds(joint, target) {
                        return Err(refuse(joint, target));
                    }
                    ch.target = ClipTarget::Joint {
                        skeleton: map.to,
                        joint: target,
                    };
                    driven.insert((target, ch.property));
                }
                channels.push(ch);
            }
            // the target follows the source: undriven mapped joints held at the source's rests
            let mut held = 0u64;
            if self.rests == Rests::Source {
                for (&a, &b) in &map.pairs {
                    let src = Trs::of(&map.from_rest[&a]).map_err(|e| format!("joint {a:?}: {e}"))?;
                    let dst = Trs::of(&map.to_rest[&b]).map_err(|e| format!("joint {b:?}: {e}"))?;
                    let parts = [
                        (
                            Property::Translation,
                            src.translation.to_vec(),
                            dst.translation.to_vec(),
                            false,
                        ),
                        (
                            Property::Rotation,
                            src.rotation.to_vec(),
                            dst.rotation.to_vec(),
                            true,
                        ),
                        (Property::Scale, src.scale.to_vec(), dst.scale.to_vec(), false),
                    ];
                    let mut any = false;
                    for (property, s, d, rotation) in parts {
                        if driven.contains(&(b, property)) || apart(&s, &d, rotation) <= REST_TOLERANCE {
                            continue;
                        }
                        channels.push(Channel {
                            target: ClipTarget::Joint {
                                skeleton: map.to,
                                joint: b,
                            },
                            property,
                            interpolation: Interpolation::Step,
                            times: vec![0.0],
                            values: s.iter().map(|&x| x as f32).collect(),
                        });
                        any = true;
                    }
                    held += u64::from(any);
                }
            }
            mapped.channels = channels;
            log.count("clips mapped onto another skeleton", 1);
            if held > 0 {
                log.count("undriven joints held at the source skeleton's rest", held);
            }
            if dropped > 0 {
                log.omit(
                    "clip channels on joints the map does not list (unmapped=drop)",
                    dropped,
                );
            }
            w.insert(c, mapped);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::{evaluate, model_transforms};
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{Channel, Extrapolation, Interpolation, Joint, Property, Rest, Transform};

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn joint(id: u32, parent: Option<u32>, y: f32) -> Joint {
        let mut t = Transform::IDENTITY;
        t.cols[3] = [0.0, y, 0.0];
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest: Rest::Affine(t),
        }
    }

    /// Skeleton a (1 -> 2 -> 3), skeleton b (10 -> 20 -> 30, and 40 under 10) with b's rests different, a map
    /// 1->10, 2->20, 3->30 (or as given), and a clip turning a's joint 2 and raising its joint 3: (world, a, b, clip).
    fn world(pairs: &[(u32, u32)]) -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(
            a,
            Skeleton {
                joints: vec![
                    joint(1, None, 0.0),
                    joint(2, Some(1), 1.0),
                    joint(3, Some(2), 1.0),
                ],
            },
        );
        let b = w.spawn();
        w.insert(
            b,
            Skeleton {
                joints: vec![
                    joint(10, None, 0.0),
                    joint(20, Some(10), 2.0),
                    joint(30, Some(20), 2.0),
                    joint(40, Some(10), 5.0),
                ],
            },
        );
        let m = w.spawn();
        w.insert(
            m,
            JointMap {
                from: a,
                to: b,
                pairs: pairs.iter().map(|&(x, y)| (JointId(x), JointId(y))).collect(),
            },
        );
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "bend".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![
                    Channel {
                        target: ClipTarget::Joint {
                            skeleton: a,
                            joint: JointId(2),
                        },
                        property: Property::Rotation,
                        interpolation: Interpolation::Linear,
                        times: vec![0.0, 1.0],
                        values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S],
                    },
                    Channel {
                        target: ClipTarget::Joint {
                            skeleton: a,
                            joint: JointId(3),
                        },
                        property: Property::Translation,
                        interpolation: Interpolation::Linear,
                        times: vec![0.0, 1.0],
                        values: vec![0.0, 1.0, 0.0, 0.0, 1.5, 0.0],
                    },
                ],
                root_motion: None,
                additive: None,
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        (w, a, b, clip)
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

    /// Mapped, the clip drives b: the joints it drives get the same local values, so where the hierarchies
    /// correspond the driven chain composes to the same model-space transforms relative to the mapped root.
    #[test]
    fn a_mapped_clip_drives_the_target_skeleton_exactly() {
        let (mut w, a, b, clip) = world(&[(1, 10), (2, 20), (3, 30)]);
        let before = {
            let c = w.get::<Clip>(clip).unwrap();
            let e = evaluate(c, a, w.get::<Skeleton>(a).unwrap(), None, 0.5).unwrap();
            model_transforms(
                w.get::<Skeleton>(a).unwrap(),
                &crate::pose::locals(w.get::<Skeleton>(a).unwrap(), Some(&e.pose)),
            )
            .unwrap()
        };
        let report = run(&mut w, "map-clip{unmapped=refuse,rests=target}").unwrap();
        assert_eq!(
            report.steps[0].log.counts["clips mapped onto another skeleton"],
            1
        );
        let c = w.get::<Clip>(clip).unwrap();
        assert!(
            c.channels
                .iter()
                .all(|ch| matches!(ch.target, ClipTarget::Joint { skeleton, .. } if skeleton == b))
        );
        let after = {
            let e = evaluate(c, b, w.get::<Skeleton>(b).unwrap(), None, 0.5).unwrap();
            model_transforms(
                w.get::<Skeleton>(b).unwrap(),
                &crate::pose::locals(w.get::<Skeleton>(b).unwrap(), Some(&e.pose)),
            )
            .unwrap()
        };
        // joint 3 relative to joint 2 (both driven) is the same on both skeletons
        let rel = |m: &BTreeMap<JointId, crate::pose::Affine>, p: u32, c: u32| {
            crate::pose::compose(&crate::pose::inverse(&m[&JointId(p)]).unwrap(), &m[&JointId(c)])
        };
        let (x, y) = (rel(&before, 2, 3), rel(&after, 20, 30));
        for (p, q) in x.iter().flatten().zip(y.iter().flatten()) {
            assert!((p - q).abs() < 1e-9, "{x:?} vs {y:?}");
        }
    }

    #[test]
    fn map_clip_refuses_what_it_cannot_map_exactly() {
        // 3 -> 40: 40's parent is 10, not 2's target 20
        let (mut w, ..) = world(&[(1, 10), (2, 20), (3, 40)]);
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target}").unwrap_err();
        assert!(err.contains("does not map to the target's parent"), "{err}");
        // 3 unmapped: refused, or dropped and counted
        let (mut w, _, _, clip) = world(&[(1, 10), (2, 20)]);
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target}").unwrap_err();
        assert!(err.contains("which the map does not list"), "{err}");
        let report = run(&mut w, "map-clip{unmapped=drop,rests=target}").unwrap();
        assert_eq!(w.get::<Clip>(clip).unwrap().channels.len(), 1);
        assert_eq!(
            report.steps[0].log.omitted["clip channels on joints the map does not list (unmapped=drop)"],
            1
        );
        // bound to a rig of the source skeleton: map first
        let (mut w, a, _, clip) = world(&[(1, 10), (2, 20), (3, 30)]);
        let rig = w.spawn();
        w.insert(rig, RigInstance { skeleton: a });
        w.insert(rig, Transform::IDENTITY);
        let bind = w.spawn();
        w.insert(
            bind,
            ClipBinding {
                clip,
                rig: Some(rig),
                meshes: Vec::new(),
            },
        );
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target}").unwrap_err();
        assert!(err.contains("map the clip, then bind it"), "{err}");
        // a joint mapped twice is refused by the map's own check
        let (mut w, ..) = world(&[(1, 10), (2, 20)]);
        let (m, _) = w.iter::<JointMap>().next().unwrap();
        w.get_mut::<JointMap>(m)
            .unwrap()
            .pairs
            .push((JointId(3), JointId(10)));
        assert!(
            w.verify_components()
                .iter()
                .any(|(_, _, m)| m.contains("mapped twice"))
        );
    }

    /// With the source's rests, the target follows the source: every mapped joint composes to the source's model-space
    /// transform (b's joint 20 rests at y=2, a's joint 2 at y=1: held at 1 by a constant channel).
    #[test]
    fn with_the_source_rests_the_target_follows_the_source() {
        let (mut w, a, b, clip) = world(&[(1, 10), (2, 20), (3, 30)]);
        let model = |w: &World, skel: Entity, t: f64| {
            let c = w.get::<Clip>(clip).unwrap();
            let s = w.get::<Skeleton>(skel).unwrap();
            let e = evaluate(c, skel, s, None, t).unwrap();
            model_transforms(s, &crate::pose::locals(s, Some(&e.pose))).unwrap()
        };
        let before: Vec<_> = [0.0, 0.5, 1.0].iter().map(|&t| model(&w, a, t)).collect();
        let report = run(&mut w, "map-clip{unmapped=refuse,rests=source}").unwrap();
        assert_eq!(
            report.steps[0].log.counts["undriven joints held at the source skeleton's rest"],
            1
        );
        for (k, &t) in [0.0, 0.5, 1.0].iter().enumerate() {
            let after = model(&w, b, t);
            for (x, y) in [(1, 10), (2, 20), (3, 30)] {
                let (p, q) = (&before[k][&JointId(x)], &after[&JointId(y)]);
                for (u, v) in p.iter().flatten().zip(q.iter().flatten()) {
                    assert!((u - v).abs() < 1e-6, "t={t} joint {x}: {p:?} vs {q:?}");
                }
            }
        }
    }
}
