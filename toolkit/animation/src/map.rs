//! `map-clip`: clips made to drive another skeleton through a `core.joint_map`, where that is exact.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Channel, Clip, ClipBinding, ClipTarget, Component, Entity, Interpolation, JointId, JointMap, Property,
    Rest, RigInstance, Skeleton, World,
};

use crate::pose::{Affine, Trs, affine, compose, evaluate, inverse, model_transforms, transform};

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

/// What `map-clip` does with a pair whose target's parent is not the target of its source's parent (a hierarchy that
/// differs: Elden Ring's model parents its IK targets nowhere where its Havok skeleton parents them under the root).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reparented {
    /// Refuse the clip, naming the pair.
    Refuse,
    /// The target joint takes the source joint's model-space pose (the game's own rule: the animated model-space pose
    /// replaces the model bone's, whatever the two rests are), re-keyed at the clip's key times; target joints no pair
    /// names follow their parent at their rest. Exact at the keys, interpolated between them (counted as an
    /// approximation).
    ModelSpace,
}

/// `map-clip{unmapped=refuse|drop,rests=target|source,reparented=refuse|model-space}` (all required): every `core.clip` whose joint channels drive
/// the `from` skeleton of a `core.joint_map` drives its `to` skeleton instead: each joint channel's target becomes the
/// mapped joint, its values unchanged. A joint channel replaces its joint's local transform in its parent's space, so
/// each mapped joint's target must have the target of its source's parent as its parent (a root's, none): refused
/// otherwise (a different hierarchy needs retargeting). With `rests=source`, each mapped joint the clip does not drive
/// whose source rest differs from its target's (beyond 1e-6) gets that source rest as constant channels (one key at 0,
/// step; only the parts that differ), so every mapped joint composes to the source skeleton's model-space pose exactly,
/// and the hierarchy is checked for every pair; with `rests=target` the target's rests stand and only the driven
/// joints' hierarchy is checked. Channels on joints the map does not list are refused or left out (`unmapped`). Morph
/// and control channels are kept. Two maps from one skeleton are refused (which one is meant is not stated), and so is
/// a clip bound (`core.clip_binding`) to a rig of the source skeleton: map, then bind. A pair whose hierarchy differs
/// is refused with `reparented=refuse` (the joints a clip drives) and moved by its model-space pose with
/// `reparented=model-space` (every such pair, driven or not: the joint takes the source joint's model-space pose at
/// each of the clip's key times, as local keys from its target parent's pose; see [`Reparented`]). A map's
/// `core.joint_placement` (where the target joints stand inside a source mesh) is a statement about meshes, not
/// motion: this pass never reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapClip {
    pub unmapped: Unmapped,
    pub rests: Rests,
    pub reparented: Reparented,
}

impl Choice for Unmapped {
    const VALUES: &'static [(&'static str, Self)] = &[("refuse", Unmapped::Refuse), ("drop", Unmapped::Drop)];
}

impl Choice for Rests {
    const VALUES: &'static [(&'static str, Self)] = &[("target", Rests::Target), ("source", Rests::Source)];
}

impl Choice for Reparented {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("refuse", Reparented::Refuse),
        ("model-space", Reparented::ModelSpace),
    ];
}

/// `map-clip`'s declaration ([`primase_core::pass::PassRegistry::add`]): every option required.
pub const MAP_CLIP: PassSpec = PassSpec {
    name: "map-clip",
    options: &[
        OptionSpec::choice::<Unmapped>(
            "unmapped",
            "a channel of a joint the map leaves out: the clip refused, or the channel left out, counted",
        ),
        OptionSpec::choice::<Rests>(
            "rests",
            "whose rests the mapped clip plays over: the target skeleton's own, or the source's as constant channels",
        ),
        OptionSpec::choice::<Reparented>(
            "reparented",
            "a pair whose parents do not correspond: the clip refused, or the target joint given the source's \
             model-space pose",
        ),
    ],
    make: |g| {
        Ok(Box::new(MapClip {
            unmapped: g.choice::<Unmapped>("unmapped"),
            rests: g.choice::<Rests>("rests"),
            reparented: g.choice::<Reparented>("reparented"),
        }))
    },
};

/// A joint map as lookups, with its skeletons' parents and rests.
struct Map {
    from_skeleton: Skeleton,
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
        Options::from([
            ("unmapped".to_owned(), self.unmapped.text().to_owned()),
            ("rests".to_owned(), self.rests.text().to_owned()),
            ("reparented".to_owned(), self.reparented.text().to_owned()),
        ])
    }
    fn approximates(&self) -> &'static str {
        match self.reparented {
            Reparented::Refuse => "",
            Reparented::ModelSpace => {
                "joints whose hierarchy differs are keyed at the clip's key times; between keys their local transforms interpolate, not the source's model-space pose"
            }
        }
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
                from_skeleton: from.clone(),
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
            // an additive clip's reference is a pose or a frame of a clip of the source skeleton: not remapped here
            if clip.additive.is_some() {
                return Err(format!("clip {c:?} is additive: not mapped"));
            }
            if let Some(&(_, rig)) = bound.iter().find(|(clip, rig)| {
                *clip == c && w.get::<RigInstance>(*rig).is_some_and(|r| r.skeleton == from)
            }) {
                return Err(format!(
                    "clip {c:?} is bound to rig {rig:?} of the source skeleton: map the clip, then bind it"
                ));
            }
            if self.rests == Rests::Source
                && self.reparented == Reparented::Refuse
                && let Some((&a, &b)) = map.pairs.iter().find(|(a, b)| !map.corresponds(**a, **b))
            {
                return Err(refuse(a, b));
            }
            // the pairs whose hierarchy differs, moved by their model-space pose
            let baked: BTreeSet<JointId> = match self.reparented {
                Reparented::Refuse => BTreeSet::new(),
                Reparented::ModelSpace => map
                    .pairs
                    .iter()
                    .filter(|(a, b)| !map.corresponds(**a, **b))
                    .map(|(a, _)| *a)
                    .collect(),
            };
            let clip_for_keys = clip.clone();
            let mut mapped = clip.clone();
            let mut dropped = 0u64;
            let mut channels = Vec::with_capacity(mapped.channels.len());
            let mut driven: BTreeSet<(JointId, Property)> = BTreeSet::new();
            for mut ch in mapped.channels {
                if let ClipTarget::Joint { joint, .. } = ch.target {
                    if baked.contains(&joint) {
                        // replaced by the keys below
                        continue;
                    }
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
            if !baked.is_empty() {
                let keys = model_space_keys(&clip_for_keys, from, map, &baked)?;
                for k in keys {
                    channels.push(Channel {
                        target: ClipTarget::Joint {
                            skeleton: map.to,
                            joint: k.joint,
                        },
                        property: k.property,
                        interpolation: Interpolation::Linear,
                        times: k.times,
                        values: k.values,
                    });
                    driven.insert((k.joint, k.property));
                }
                log.count(
                    "joints moved by the source's model-space pose at the clip's keys (their parent is not the source parent's target)",
                    baked.len() as u64,
                );
                log.approximate(
                    "joints whose hierarchy differs are keyed at the clip's key times; between keys their local transforms interpolate, not the source's model-space pose",
                    baked.len() as u64,
                );
            }
            // the target follows the source: undriven mapped joints held at the source's rests
            let mut held = 0u64;
            if self.rests == Rests::Source {
                for (&a, &b) in &map.pairs {
                    let src = Trs::of(&map.from_rest[&a]).map_err(|e| format!("joint {a:?}: {e}"))?;
                    // a target rest with no TRS form (a bone scaled to nothing) differs from every source rest
                    let dst = Trs::of(&map.to_rest[&b]).ok();
                    let away = |n: usize| vec![f64::INFINITY; n];
                    let parts = [
                        (
                            Property::Translation,
                            src.translation.to_vec(),
                            dst.map_or_else(|| away(3), |d| d.translation.to_vec()),
                            false,
                        ),
                        (
                            Property::Rotation,
                            src.rotation.to_vec(),
                            dst.map_or_else(|| away(4), |d| d.rotation.to_vec()),
                            true,
                        ),
                        (
                            Property::Scale,
                            src.scale.to_vec(),
                            dst.map_or_else(|| away(3), |d| d.scale.to_vec()),
                            false,
                        ),
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

/// One channel of a target joint: its property, key times and values.
struct Keyed {
    joint: JointId,
    property: Property,
    times: Vec<f32>,
    values: Vec<f32>,
}

/// The local keys of the target joints of `baked` source joints: at each key time of `clip` (the union of its channels'
/// times on the source skeleton), every paired target joint takes the source joint's model-space pose, the target
/// joints no pair names follow their parent at their rest, and a baked joint's local is its target parent's model-space
/// pose's inverse times its own. Returns each joint's channels (translation, rotation, scale), a channel whose keys are
/// all alike as one key at 0.
fn model_space_keys(
    clip: &Clip,
    from: Entity,
    map: &Map,
    baked: &BTreeSet<JointId>,
) -> Result<Vec<Keyed>, String> {
    let mut times: Vec<f32> = clip
        .channels
        .iter()
        .filter(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == from))
        .flat_map(|c| c.times.iter().copied())
        .collect();
    times.sort_by(f32::total_cmp);
    times.dedup();
    if times.is_empty() {
        times.push(0.0);
    }
    let source_of: BTreeMap<JointId, JointId> = map.pairs.iter().map(|(a, b)| (*b, *a)).collect();
    // target joints in parents-first order
    let mut order: Vec<JointId> = Vec::with_capacity(map.to_parent.len());
    let mut seen: BTreeSet<JointId> = BTreeSet::new();
    for &j in map.to_parent.keys() {
        let mut chain = vec![j];
        let mut at = map.to_parent[&j];
        while let Some(p) = at {
            if seen.contains(&p) {
                break;
            }
            if chain.contains(&p) {
                return Err(format!("target joint {j:?}: its parents form a cycle"));
            }
            chain.push(p);
            at = map.to_parent.get(&p).copied().flatten();
        }
        for c in chain.into_iter().rev() {
            if seen.insert(c) {
                order.push(c);
            }
        }
    }
    // a rest as the matrix it is (a degenerate one, a bone scaled to nothing, has no TRS form but is a matrix all the same)
    let rest_affine = |rest: &Rest| -> Result<Affine, String> {
        match rest {
            Rest::Affine(t) => Ok(affine(t)),
            Rest::Trs { .. } => Ok(Trs::of(rest).map_err(|e| e.to_string())?.matrix()),
        }
    };
    let baked_targets: Vec<(JointId, JointId)> = map
        .pairs
        .iter()
        .filter(|(a, _)| baked.contains(a))
        .map(|(a, b)| (*a, *b))
        .collect();
    let mut keys: BTreeMap<JointId, Vec<Trs>> = BTreeMap::new();
    for &t in &times {
        let ev = evaluate(clip, from, &map.from_skeleton, None, f64::from(t)).map_err(|e| e.to_string())?;
        let pose: BTreeMap<JointId, Rest> = ev.pose.joints.iter().copied().collect();
        let source = model_transforms(&map.from_skeleton, &pose).map_err(|e| e.to_string())?;
        let mut target: BTreeMap<JointId, Affine> = BTreeMap::new();
        for &j in &order {
            let m = match source_of.get(&j) {
                Some(a) => source[a],
                None => {
                    let local = rest_affine(&map.to_rest[&j])?;
                    match map.to_parent[&j] {
                        Some(p) => compose(&target[&p], &local),
                        None => local,
                    }
                }
            };
            target.insert(j, m);
        }
        for &(_, b) in &baked_targets {
            let local = match map.to_parent[&b] {
                Some(p) => {
                    let inv =
                        inverse(&target[&p]).ok_or_else(|| format!("target joint {p:?}: a singular pose"))?;
                    compose(&inv, &target[&b])
                }
                None => target[&b],
            };
            let trs = Trs::of(&Rest::Affine(transform(&local))).map_err(|e| format!("joint {b:?}: {e}"))?;
            keys.entry(b).or_default().push(trs);
        }
    }
    let mut out = Vec::new();
    for (b, mut list) in keys {
        // one rotation by its nearer sign, so a lerp between keys takes the short way
        for i in 1..list.len() {
            let dot: f64 = list[i - 1]
                .rotation
                .iter()
                .zip(&list[i].rotation)
                .map(|(p, q)| p * q)
                .sum();
            if dot < 0.0 {
                list[i].rotation = list[i].rotation.map(|v| -v);
            }
        }
        let parts: [(Property, Vec<f32>, usize); 3] = [
            (
                Property::Translation,
                list.iter()
                    .flat_map(|t| t.translation.map(|v| v as f32))
                    .collect(),
                3,
            ),
            (
                Property::Rotation,
                list.iter().flat_map(|t| t.rotation.map(|v| v as f32)).collect(),
                4,
            ),
            (
                Property::Scale,
                list.iter().flat_map(|t| t.scale.map(|v| v as f32)).collect(),
                3,
            ),
        ];
        for (property, values, width) in parts {
            if values.chunks(width).all(|k| k == &values[..width]) {
                out.push(Keyed {
                    joint: b,
                    property,
                    times: vec![0.0],
                    values: values[..width].to_vec(),
                });
            } else {
                out.push(Keyed {
                    joint: b,
                    property,
                    times: times.clone(),
                    values,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::{evaluate, model_transforms};
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{
        AdditiveReference, Channel, Extrapolation, Interpolation, Joint, Property, Rest, Transform,
    };

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
        let report = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap();
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
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap_err();
        assert!(err.contains("does not map to the target's parent"), "{err}");
        // 3 unmapped: refused, or dropped and counted
        let (mut w, _, _, clip) = world(&[(1, 10), (2, 20)]);
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap_err();
        assert!(err.contains("which the map does not list"), "{err}");
        let report = run(&mut w, "map-clip{unmapped=drop,rests=target,reparented=refuse}").unwrap();
        assert_eq!(w.get::<Clip>(clip).unwrap().channels.len(), 1);
        assert_eq!(
            report.steps[0].log.omitted["clip channels on joints the map does not list (unmapped=drop)"],
            1
        );
        // an additive clip's reference is not remapped: refused, as retarget-clip does
        let (mut w, _, _, clip) = world(&[(1, 10), (2, 20), (3, 30)]);
        w.get_mut::<Clip>(clip).unwrap().additive = Some(AdditiveReference::ClipFrame { clip, time: 0.0 });
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap_err();
        assert!(err.contains("additive"), "{err}");
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
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap_err();
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
        let report = run(&mut w, "map-clip{unmapped=refuse,rests=source,reparented=refuse}").unwrap();
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

    fn trs_joint(id: u32, parent: Option<u32>, translation: [f32; 3]) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest: Rest::Trs {
                translation,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        }
    }

    /// Source A(1) -> B(2) -> C(3), one unit up each; target A'(10) with B'(20) and C'(30) both under it (C's parent is
    /// not B''s: a reparented joint) and an unpaired D'(40) under C'. The clip turns B a quarter about z and moves A
    /// along x over times 0, 1, 2 and, when `drive_c`, raises C. Returns the world, both skeleton entities and the clip.
    fn reparented_world(drive_c: bool) -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(
            a,
            Skeleton {
                joints: vec![
                    trs_joint(1, None, [0.0, 0.0, 0.0]),
                    trs_joint(2, Some(1), [0.0, 1.0, 0.0]),
                    trs_joint(3, Some(2), [0.0, 1.0, 0.0]),
                ],
            },
        );
        let b = w.spawn();
        w.insert(
            b,
            Skeleton {
                joints: vec![
                    trs_joint(10, None, [0.0, 0.0, 0.0]),
                    trs_joint(20, Some(10), [0.0, 5.0, 0.0]),
                    trs_joint(30, Some(10), [3.0, 0.0, 0.0]),
                    trs_joint(40, Some(30), [0.0, 0.5, 0.0]),
                ],
            },
        );
        let m = w.spawn();
        w.insert(
            m,
            JointMap {
                from: a,
                to: b,
                pairs: [(1, 10), (2, 20), (3, 30)]
                    .iter()
                    .map(|&(x, y)| (JointId(x), JointId(y)))
                    .collect(),
            },
        );
        let joint = |j: u32, property, times: Vec<f32>, values: Vec<f32>| Channel {
            target: ClipTarget::Joint {
                skeleton: a,
                joint: JointId(j),
            },
            property,
            interpolation: Interpolation::Linear,
            times,
            values,
        };
        let mut channels = vec![
            joint(
                2,
                Property::Rotation,
                vec![0.0, 1.0, 2.0],
                vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S, 0.0, 0.0, 0.0, 1.0],
            ),
            joint(
                1,
                Property::Translation,
                vec![0.0, 2.0],
                vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            ),
        ];
        if drive_c {
            channels.push(joint(
                3,
                Property::Translation,
                vec![0.0, 2.0],
                vec![0.0, 1.0, 0.0, 0.0, 2.0, 0.0],
            ));
        }
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "turn".into(),
                duration: 2.0,
                extrapolation: Extrapolation::Hold,
                channels,
                root_motion: None,
                additive: None,
            },
        );
        (w, a, b, clip)
    }

    /// The model-space pose of every paired joint after the mapping equals the source's at every key time (the
    /// reparented joint takes its source's model-space pose, the unpaired one follows its parent), and the pass says
    /// what it did and what it approximates.
    #[test]
    fn a_reparented_joint_takes_the_sources_model_space_pose_at_every_key() {
        for drive_c in [true, false] {
            let (mut w, a, b, clip) = reparented_world(drive_c);
            let source = w.get::<Clip>(clip).unwrap().clone();
            let report = run(
                &mut w,
                "map-clip{unmapped=refuse,rests=source,reparented=model-space}",
            )
            .unwrap();
            let log = &report.steps[0].log;
            assert_eq!(
                log.counts["joints moved by the source's model-space pose at the clip's keys (their parent is not the source parent's target)"],
                1
            );
            assert!(
                log.approximations.keys().any(|k| k.contains("between keys")),
                "{log:?}"
            );
            let mapped = w.get::<Clip>(clip).unwrap().clone();
            let (sa, sb) = (
                w.get::<Skeleton>(a).unwrap().clone(),
                w.get::<Skeleton>(b).unwrap().clone(),
            );
            for t in [0.0, 1.0, 2.0] {
                let es = evaluate(&source, a, &sa, None, t).unwrap();
                let et = evaluate(&mapped, b, &sb, None, t).unwrap();
                let ms = model_transforms(&sa, &es.pose.joints.iter().copied().collect()).unwrap();
                let mt = model_transforms(&sb, &et.pose.joints.iter().copied().collect()).unwrap();
                for (from, to) in [(1, 10), (2, 20), (3, 30)] {
                    let (x, y) = (ms[&JointId(from)], mt[&JointId(to)]);
                    let d = x
                        .iter()
                        .flatten()
                        .zip(y.iter().flatten())
                        .map(|(p, q)| (p - q).abs())
                        .fold(0.0, f64::max);
                    assert!(d < 1e-6, "drive_c {drive_c} t {t} joint {from}: {d}");
                }
                // the unpaired joint under the reparented one follows it at its rest (0.5 up)
                let (c, d) = (mt[&JointId(30)], mt[&JointId(40)]);
                let up = (0..3)
                    .map(|i| (d[3][i] - (c[3][i] + c[1][i] * 0.5)).abs())
                    .fold(0.0, f64::max);
                assert!(up < 1e-6, "{up}");
            }
        }
    }

    /// Without the option the clip that drives a reparented joint is refused, as before.
    #[test]
    fn a_reparented_joint_the_clip_drives_is_refused_without_model_space() {
        let (mut w, _, _, _) = reparented_world(true);
        let err = run(&mut w, "map-clip{unmapped=refuse,rests=target,reparented=refuse}").unwrap_err();
        assert!(err.contains("a different hierarchy"), "{err}");
        let e = run(
            &mut reparented_world(true).0,
            "map-clip{unmapped=refuse,rests=target}",
        )
        .unwrap_err();
        assert!(e.contains("reparented is required: "), "{e}");
    }
}
