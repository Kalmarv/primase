//! `correspond-skeletons`: the joint map between two skeletons of one character that name their joints alike and agree
//! on them, so `map-clip` can move a clip made for one onto the other.
//!
//! A map built by name states what else it checked ([`primase_core::JointMap`]): a joint is paired with the joint of the
//! other skeleton that has its name only where that joint is the only one of the name on each side, the two rests agree
//! (translations in metres and scales as ratios within `tolerance`, rotations within `rotation-tolerance` in radians,
//! which is `tolerance` where not given), and the parents correspond (the
//! parent of one is paired with the parent of the other; roots with roots). Anything else is left out, counted by reason:
//! a skeleton with extra joints between two it shares breaks the chain there, and a clip's channels on the joints below
//! are then dropped or refused by `map-clip`, as its option says.
//!
//! `parents=any` pairs by the unique name alone, for two skeletons of one character whose hierarchies and rests are
//! not alike (one source's Havok skeletons parent their IK targets under the root and a creature's chains otherwise
//! than its model does, and the model's bind pose is not the Havok reference pose): nothing but the name is checked,
//! and the pairs whose model-space rests differ by more than the tolerances (the translation in metres, the rotation and
//! scale entries against `rotation-tolerance`) are counted, not refused, since the game
//! poses the model by the Havok joint's model-space pose whatever the two rests are. A clip of the first skeleton then
//! moves to the second by `map-clip`'s `reparented=model-space`.

use std::collections::BTreeMap;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, JointId, JointMap, Name, Rest, Skeleton, World};

use crate::pose::{Trs, model_transforms};

/// A skeleton with this exact core name, as used by correspondence pass recipes.
/// Ambiguous names return None rather than choosing an unrelated definition.
pub fn skeleton_named<'a>(world: &'a World, name: &str) -> Option<&'a Skeleton> {
    let mut found = world
        .iter::<Skeleton>()
        .filter(|(entity, _)| world.get::<Name>(*entity).is_some_and(|n| n.0 == name));
    let (_, skeleton) = found.next()?;
    found.next().is_none().then_some(skeleton)
}

/// Evidence for choosing correspondence: unique shared names and same-hierarchy pairs,
/// using the same rest/parent tests as correspond-skeletons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HierarchyCounts {
    pub joints: usize,
    pub named: usize,
    pub paired: usize,
}

/// Counts for two named skeletons. None means a missing/ambiguous name or an invalid
/// tolerance; this does not choose a mapping policy for the caller.
pub fn hierarchy_counts(world: &World, from: &str, to: &str, tolerance: f64) -> Option<HierarchyCounts> {
    if !tolerance.is_finite() || tolerance < 0.0 {
        return None;
    }
    let (source, target) = (skeleton_named(world, from)?, skeleton_named(world, to)?);
    Some(HierarchyCounts {
        joints: source.joints.len(),
        named: shared_names(source, target),
        paired: same_hierarchy_pairs(source, target, tolerance),
    })
}

#[cfg(test)]
mod named_tests {
    use super::*;
    #[test]
    fn named_counts_use_the_correspondence_rules_and_refuse_ambiguous_names() {
        let mut world = World::new();
        let skeleton = Skeleton {
            joints: vec![primase_core::Joint {
                id: JointId(0),
                name: "Root".into(),
                parent: None,
                rest: Rest::Affine(primase_core::Transform::IDENTITY),
            }],
        };
        for name in ["source", "target"] {
            let e = world.spawn();
            world.insert(e, Name(name.into()));
            world.insert(e, skeleton.clone());
        }
        assert_eq!(
            hierarchy_counts(&world, "source", "target", 0.001),
            Some(HierarchyCounts {
                joints: 1,
                named: 1,
                paired: 1
            })
        );
        assert!(hierarchy_counts(&world, "source", "missing", 0.001).is_none());
        assert!(hierarchy_counts(&world, "source", "target", f64::NAN).is_none());
        let e = world.spawn();
        world.insert(e, Name("source".into()));
        world.insert(e, skeleton);
        assert!(skeleton_named(&world, "source").is_none());
    }
}

/// What a pair must have besides its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parents {
    /// Rests alike (within the tolerance) and parents that correspond: the pairs a clip's channels move across unchanged.
    Same,
    /// Nothing: the name is the pair (model-space rests that differ are counted).
    Any,
}

/// `correspond-skeletons{from=NAME,to=NAME,tolerance=T,parents=same|any}` (all required; `rotation-tolerance=R`
/// optional): the one skeleton (`core.skeleton`) whose `core.name` is `from` and the one named `to` get a
/// `core.joint_map` from the first to the second (see the module documentation for `parents`). `tolerance` is in
/// metres (and a ratio for scale), `rotation-tolerance` in radians (`tolerance` when not given).
#[derive(Clone, Debug, PartialEq)]
pub struct CorrespondSkeletons {
    pub from: String,
    pub to: String,
    pub tolerance: f64,
    pub rotation_tolerance: f64,
    pub parents: Parents,
}

impl Choice for Parents {
    const VALUES: &'static [(&'static str, Self)] = &[("same", Parents::Same), ("any", Parents::Any)];
}

/// `correspond-skeletons`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const CORRESPOND_SKELETONS: PassSpec = PassSpec {
    name: "correspond-skeletons",
    options: &[
        OptionSpec::required("from", "a", "the source skeleton's name"),
        OptionSpec::required("to", "b", "the target skeleton's name, another than from"),
        OptionSpec::required(
            "tolerance",
            "0.001",
            "above 0: how far rest translations and scales may differ for a pair",
        ),
        OptionSpec::optional(
            "rotation-tolerance",
            "0.01",
            "radians, above 0: how far rest rotations may differ for a pair",
            "tolerance's value",
        ),
        OptionSpec::choice::<Parents>(
            "parents",
            "what a name pair must also have: rests alike and corresponding parents, or nothing",
        ),
    ],
    make: correspond_skeletons,
};

fn correspond_skeletons(g: &Given) -> Result<Box<dyn Pass>, String> {
    let positive = |name: &str, v: &str| {
        v.parse::<f64>()
            .ok()
            .filter(|t| t.is_finite() && *t > 0.0)
            .ok_or_else(|| format!("{name}={v}: a positive number"))
    };
    let (from, to) = (g.required("from"), g.required("to"));
    if from == to {
        return Err("from and to name the same skeleton".into());
    }
    let tolerance = positive("tolerance", g.required("tolerance"))?;
    let rotation_tolerance = match g.optional("rotation-tolerance") {
        Some(v) => positive("rotation-tolerance", v)?,
        None => tolerance,
    };
    Ok(Box::new(CorrespondSkeletons {
        from: from.to_owned(),
        to: to.to_owned(),
        tolerance,
        rotation_tolerance,
        parents: g.choice::<Parents>("parents"),
    }))
}

/// The one skeleton entity named `name`.
fn find(w: &World, name: &str) -> Result<Entity, String> {
    let named: Vec<Entity> = w
        .iter::<Skeleton>()
        .map(|(e, _)| e)
        .filter(|e| w.get::<Name>(*e).is_some_and(|n| n.0 == name))
        .collect();
    match named.as_slice() {
        [e] => Ok(*e),
        [] => Err(format!("no skeleton is named {name}")),
        _ => Err(format!("{} skeletons are named {name}", named.len())),
    }
}

/// Whether two rests agree: translations and scales within `tolerance`, rotations within `rotation_tolerance` radians.
fn agree(a: &Trs, b: &Trs, tolerance: f64, rotation_tolerance: f64) -> bool {
    let near = |x: &[f64], y: &[f64]| x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tolerance);
    let dot: f64 = a.rotation.iter().zip(&b.rotation).map(|(p, q)| p * q).sum();
    near(&a.translation, &b.translation)
        && near(&a.scale, &b.scale)
        && 2.0 * dot.abs().min(1.0).acos() <= rotation_tolerance
}

/// The joint pairs of `from` onto `to` that [`CorrespondSkeletons`] makes under `parents`, each left-out joint counted
/// by its reason in `log` when one is given. A caller that only needs to know whether the two skeletons agree (the lab
/// chooses how a character's clips move across from it) passes none.
pub fn pair_joints(
    from: &Skeleton,
    to: &Skeleton,
    tolerance: f64,
    rotation_tolerance: f64,
    parents: Parents,
    mut log: Option<&mut PassLog>,
) -> BTreeMap<JointId, JointId> {
    let mut count = |what: &str, n: u64| {
        if let Some(log) = log.as_deref_mut() {
            log.count(what, n);
        }
    };
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, j) in to.joints.iter().enumerate() {
        by_name.entry(j.name.as_str()).or_default().push(i);
    }
    let mut from_names: BTreeMap<&str, usize> = BTreeMap::new();
    for j in &from.joints {
        *from_names.entry(j.name.as_str()).or_default() += 1;
    }
    // candidates: the one joint of the name on each side whose rest agrees
    let mut candidate: BTreeMap<JointId, JointId> = BTreeMap::new();
    for j in &from.joints {
        let others = by_name.get(j.name.as_str()).map_or(&[][..], Vec::as_slice);
        match others {
            [] => count("joints left out: no joint of the name in the target", 1),
            [t] if from_names[j.name.as_str()] == 1 && parents == Parents::Any => {
                candidate.insert(j.id, to.joints[*t].id);
            }
            [t] if from_names[j.name.as_str()] == 1 => {
                let t = &to.joints[*t];
                match (Trs::of(&j.rest), Trs::of(&t.rest)) {
                    (Ok(x), Ok(y)) if agree(&x, &y, tolerance, rotation_tolerance) => {
                        candidate.insert(j.id, t.id);
                    }
                    _ => count("joints left out: the rests differ", 1),
                }
            }
            _ => count("joints left out: the name is not unique", 1),
        }
    }
    // the parents must correspond: drop candidates whose parent is not paired with the target's parent, until none
    // (nothing to drop with parents=any)
    // (a loop that ends when a round drops nothing, run only when the parents must correspond)
    if parents == Parents::Same {
        loop {
            let target_parents: BTreeMap<JointId, Option<JointId>> =
                to.joints.iter().map(|j| (j.id, j.parent)).collect();
            let doomed: Vec<JointId> = from
                .joints
                .iter()
                .filter(|j| candidate.contains_key(&j.id))
                .filter(|j| {
                    let target_parent = target_parents[&candidate[&j.id]];
                    match j.parent {
                        None => target_parent.is_some(),
                        Some(p) => candidate.get(&p).copied() != target_parent,
                    }
                })
                .map(|j| j.id)
                .collect();
            if doomed.is_empty() {
                break;
            }
            count(
                "joints left out: the parents do not correspond",
                doomed.len() as u64,
            );
            for d in doomed {
                candidate.remove(&d);
            }
        }
    }
    candidate
}

/// How many joints of `from` pair with one of `to` where their rests agree within `tolerance` and their parents
/// correspond (`parents=same`): none says the two skeletons are not one hierarchy, and a clip of the first moves to the
/// second by `parents=any` and `reparented=model-space` or not at all.
pub fn same_hierarchy_pairs(from: &Skeleton, to: &Skeleton, tolerance: f64) -> usize {
    pair_joints(from, to, tolerance, tolerance, Parents::Same, None).len()
}

/// How many joints of `from` have a name that is the name of exactly one joint of `to` and of no other joint of `from`:
/// the joints `parents=any` pairs. Next to [`same_hierarchy_pairs`] it says why a hierarchy disagrees: joints that share
/// a name and still do not pair by `parents=same` are left out by their rests or by the chain above them.
pub fn shared_names(from: &Skeleton, to: &Skeleton) -> usize {
    pair_joints(from, to, f64::INFINITY, f64::INFINITY, Parents::Any, None).len()
}

impl Pass for CorrespondSkeletons {
    fn name(&self) -> &'static str {
        "correspond-skeletons"
    }
    fn options(&self) -> Options {
        Options::from([
            ("from".to_owned(), self.from.clone()),
            ("to".to_owned(), self.to.clone()),
            ("tolerance".to_owned(), self.tolerance.to_string()),
            (
                "rotation-tolerance".to_owned(),
                self.rotation_tolerance.to_string(),
            ),
            ("parents".to_owned(), self.parents.text().to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Skeleton::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[JointMap::NAME, Name::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let (a, b) = (find(w, &self.from)?, find(w, &self.to)?);
        let (from, to) = match (w.get::<Skeleton>(a), w.get::<Skeleton>(b)) {
            (Some(f), Some(t)) => (f.clone(), t.clone()),
            _ => return Err("a skeleton is gone".into()),
        };
        let candidate = pair_joints(
            &from,
            &to,
            self.tolerance,
            self.rotation_tolerance,
            self.parents,
            Some(&mut *log),
        );
        if candidate.is_empty() {
            return Err(format!(
                "no joint of {} corresponds to one of {} (names, rests within {} and parents)",
                self.from, self.to, self.tolerance
            ));
        }
        log.count("joints paired", candidate.len() as u64);
        if self.parents == Parents::Any {
            // information, not a condition: where the target's bind pose is not the source's reference pose
            let rest_of = |s: &Skeleton| -> BTreeMap<JointId, Rest> {
                s.joints.iter().map(|j| (j.id, j.rest)).collect()
            };
            let (fm, tm) = (
                model_transforms(&from, &rest_of(&from)).map_err(|e| format!("{}: {e}", self.from))?,
                model_transforms(&to, &rest_of(&to)).map_err(|e| format!("{}: {e}", self.to))?,
            );
            // the three columns of the linear part (rotation and scale, unitless: judged as radians) and the
            // translation column (metres)
            let apart = candidate
                .iter()
                .filter(|(f, t)| {
                    let (a, b) = (fm[f], tm[t]);
                    let off =
                        |k: usize, limit: f64| a[k].iter().zip(&b[k]).any(|(x, y)| (x - y).abs() > limit);
                    (0..3).any(|k| off(k, self.rotation_tolerance)) || off(3, self.tolerance)
                })
                .count();
            if apart > 0 {
                log.count(
                    "joints paired whose model-space rests differ by more than the tolerance (the target's bind pose is not the source's reference pose)",
                    apart as u64,
                );
            }
        }
        let map = w.spawn();
        w.insert(map, Name(format!("{} -> {}", self.from, self.to)));
        w.insert(
            map,
            JointMap {
                from: a,
                to: b,
                pairs: from
                    .joints
                    .iter()
                    .filter_map(|j| candidate.get(&j.id).map(|t| (j.id, *t)))
                    .collect(),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{Joint, Rest};

    use super::*;

    fn joint(id: u32, name: &str, parent: Option<u32>, x: f32) -> Joint {
        Joint {
            id: JointId(id),
            name: name.to_owned(),
            parent: parent.map(JointId),
            rest: Rest::Trs {
                translation: [x, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        }
    }

    fn skeleton(w: &mut World, name: &str, joints: Vec<Joint>) -> Entity {
        let e = w.spawn();
        w.insert(e, Name(name.to_owned()));
        w.insert(e, Skeleton { joints });
        e
    }

    fn run(w: &mut World, options: &str) -> Result<RunReport, String> {
        let mut reg = PassRegistry::new();
        reg.add(&CORRESPOND_SKELETONS);
        let m = PassManager::new(reg).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(
            w,
            &m.registry.parse(options).map_err(|e| e.to_string())?,
            &mut report,
        )
        .map_err(|e| e.to_string())?;
        Ok(report)
    }

    fn pairs(w: &World) -> Vec<(u32, u32)> {
        w.iter::<JointMap>()
            .flat_map(|(_, m)| m.pairs.iter().map(|(a, b)| (a.0, b.0)).collect::<Vec<_>>())
            .collect()
    }

    /// The target has an extra joint between a joint and its child: the child's parent does not correspond, and the
    /// joints with another rest or no counterpart are left out, each counted by its reason.
    #[test]
    fn joints_correspond_where_name_rest_and_parent_agree() {
        let mut w = World::new();
        skeleton(
            &mut w,
            "a",
            vec![
                joint(0, "Master", None, 0.0),
                joint(1, "Spine", Some(0), 1.0),
                joint(2, "Head", Some(1), 2.0),
                joint(3, "Arm", Some(0), 3.0),
                joint(4, "Hand", Some(3), 4.0),
                joint(5, "Tail", Some(0), 5.0),
            ],
        );
        skeleton(
            &mut w,
            "b",
            vec![
                joint(10, "Master", None, 0.0),
                joint(11, "Spine", Some(10), 1.0),
                joint(12, "Extra", Some(11), 0.5),
                // Head is under the extra joint here: its parent does not correspond
                joint(13, "Head", Some(12), 2.0),
                joint(14, "Arm", Some(10), 3.0),
                joint(15, "Hand", Some(14), 4.0),
                // Tail's rest differs
                joint(16, "Tail", Some(10), 9.0),
            ],
        );
        let report = run(
            &mut w,
            "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}",
        )
        .unwrap();
        assert_eq!(pairs(&w), [(0, 10), (1, 11), (3, 14), (4, 15)]);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["joints paired"], 4);
        assert_eq!(log.counts["joints left out: the rests differ"], 1);
        assert_eq!(log.counts["joints left out: the parents do not correspond"], 1);
    }

    /// The question the lab asks of a character's two skeletons before choosing how its clips move across: do any
    /// joints pair where the rests agree and the parents correspond. A model whose root hangs under a joint the Havok
    /// skeleton lacks pairs none (the root's parent must be paired too), and one of the same hierarchy pairs all.
    #[test]
    fn same_hierarchy_pairs_counts_the_joints_the_parents_rule_keeps() {
        let havok = Skeleton {
            joints: vec![
                joint(0, "Master", None, 0.0),
                joint(1, "Spine", Some(0), 1.0),
                joint(2, "Head", Some(1), 2.0),
            ],
        };
        let alike = Skeleton {
            joints: vec![
                joint(10, "Master", None, 0.0),
                joint(11, "Spine", Some(10), 1.0),
                joint(12, "Head", Some(11), 2.0),
            ],
        };
        // the model hangs the whole chain under a joint the Havok skeleton does not have
        let under_a_root = Skeleton {
            joints: vec![
                joint(20, "World", None, 0.0),
                joint(21, "Master", Some(20), 0.0),
                joint(22, "Spine", Some(21), 1.0),
                joint(23, "Head", Some(22), 2.0),
            ],
        };
        assert_eq!(same_hierarchy_pairs(&havok, &alike, 0.001), 3);
        assert_eq!(same_hierarchy_pairs(&havok, &under_a_root, 0.001), 0);
        assert_eq!(shared_names(&havok, &under_a_root), 3);
        // by name alone every joint pairs, which is what parents=any does for such a model
        assert_eq!(
            pair_joints(&havok, &under_a_root, 0.001, 0.001, Parents::Any, None).len(),
            3
        );
        // the pass counts its reasons through the same function
        let mut log = PassLog::default();
        let paired = pair_joints(&havok, &under_a_root, 0.001, 0.001, Parents::Same, Some(&mut log));
        assert!(paired.is_empty());
        assert_eq!(log.counts["joints left out: the parents do not correspond"], 3);
    }

    fn turned(id: u32, name: &str, parent: Option<u32>, x: f32, angle: f32) -> Joint {
        let mut j = joint(id, name, parent, x);
        let h = angle / 2.0;
        j.rest = Rest::Trs {
            translation: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, h.sin(), h.cos()],
            scale: [1.0; 3],
        };
        j
    }

    /// A rotation is compared in radians and a translation in metres, each against its own tolerance: two rests 0.01
    /// radians apart are the same joint under `rotation-tolerance=0.05` however small `tolerance` is, and two rests
    /// 0.01 m apart are not, however large the rotation tolerance is.
    #[test]
    fn rotations_and_translations_have_their_own_tolerances() {
        let build = |angle: f32, x: f32| {
            let mut w = World::new();
            skeleton(
                &mut w,
                "a",
                vec![joint(0, "Root", None, 0.0), turned(1, "Arm", Some(0), 1.0, 0.0)],
            );
            skeleton(
                &mut w,
                "b",
                vec![
                    joint(10, "Root", None, 0.0),
                    turned(11, "Arm", Some(10), x, angle),
                ],
            );
            w
        };
        let opts = "correspond-skeletons{from=a,to=b,tolerance=0.001,rotation-tolerance=0.05,parents=same}";
        let mut w = build(0.01, 1.0);
        run(&mut w, opts).unwrap();
        assert_eq!(pairs(&w), [(0, 10), (1, 11)]);
        // the same angle against the default rotation tolerance (the tolerance, 0.001 rad): left out
        let mut w = build(0.01, 1.0);
        run(
            &mut w,
            "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}",
        )
        .unwrap();
        assert_eq!(pairs(&w), [(0, 10)]);
        // 0.01 m apart: left out whatever the rotation tolerance
        let mut w = build(0.0, 1.01);
        run(&mut w, opts).unwrap();
        assert_eq!(pairs(&w), [(0, 10)]);
        assert!(
            CORRESPOND_SKELETONS
                .instance(&Options::from([
                    ("from".to_owned(), "a".to_owned()),
                    ("to".to_owned(), "b".to_owned()),
                    ("tolerance".to_owned(), "1".to_owned()),
                    ("parents".to_owned(), "same".to_owned()),
                    ("rotation-tolerance".to_owned(), "0".to_owned()),
                ]))
                .err()
                .unwrap_or_default()
                .contains("rotation-tolerance=0: a positive number")
        );
    }

    /// With parents=any the count of pairs whose model-space rests differ uses the same two tolerances: a 0.01 rad turn
    /// is within `rotation-tolerance=0.05` and not within the default, and a translation entry is never judged by it.
    #[test]
    fn the_model_space_count_uses_each_kind_s_tolerance() {
        let apart = "joints paired whose model-space rests differ by more than the tolerance (the target's bind pose is not the source's reference pose)";
        let count = |options: &str| {
            let mut w = World::new();
            skeleton(&mut w, "a", vec![joint(0, "Root", None, 0.0)]);
            skeleton(&mut w, "b", vec![turned(10, "Root", None, 0.0, 0.01)]);
            run(&mut w, options).unwrap().steps[0]
                .log
                .counts
                .get(apart)
                .copied()
                .unwrap_or(0)
        };
        assert_eq!(
            count("correspond-skeletons{from=a,to=b,tolerance=0.001,rotation-tolerance=0.05,parents=any}"),
            0
        );
        assert_eq!(
            count("correspond-skeletons{from=a,to=b,tolerance=0.001,parents=any}"),
            1
        );
    }

    #[test]
    fn a_name_that_is_not_unique_or_not_there_is_left_out_and_nothing_to_pair_is_refused() {
        let mut w = World::new();
        skeleton(
            &mut w,
            "a",
            vec![
                joint(0, "Root", None, 0.0),
                joint(1, "Twin", Some(0), 1.0),
                joint(2, "Twin", Some(0), 1.0),
                joint(3, "Only", Some(0), 2.0),
            ],
        );
        skeleton(
            &mut w,
            "b",
            vec![joint(10, "Root", None, 0.0), joint(11, "Twin", Some(10), 1.0)],
        );
        let report = run(
            &mut w,
            "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}",
        )
        .unwrap();
        let log = &report.steps[0].log;
        assert_eq!(pairs(&w), [(0, 10)]);
        assert_eq!(log.counts["joints left out: the name is not unique"], 2);
        assert_eq!(
            log.counts["joints left out: no joint of the name in the target"],
            1
        );
        let mut none = World::new();
        skeleton(&mut none, "a", vec![joint(0, "X", None, 0.0)]);
        skeleton(&mut none, "b", vec![joint(1, "Y", None, 0.0)]);
        assert!(
            run(
                &mut none,
                "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}"
            )
            .unwrap_err()
            .contains("no joint of a corresponds")
        );
    }

    #[test]
    fn the_skeletons_are_named_exactly_and_the_options_are_required() {
        let mut w = World::new();
        skeleton(&mut w, "a", vec![joint(0, "X", None, 0.0)]);
        let e = run(
            &mut w,
            "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}",
        )
        .unwrap_err();
        assert!(e.contains("no skeleton is named b"), "{e}");
        skeleton(&mut w, "b", vec![joint(0, "X", None, 0.0)]);
        skeleton(&mut w, "b", vec![joint(0, "X", None, 0.0)]);
        let e = run(
            &mut w,
            "correspond-skeletons{from=a,to=b,tolerance=0.001,parents=same}",
        )
        .unwrap_err();
        assert!(e.contains("2 skeletons are named b"), "{e}");
        for (options, why) in [
            ("to=b,tolerance=1,parents=same", "from is required"),
            ("from=a,tolerance=1,parents=same", "to is required"),
            ("from=a,to=b,parents=same", "tolerance is required"),
            ("from=a,to=a,tolerance=1,parents=same", "the same skeleton"),
            ("from=a,to=b,tolerance=0,parents=same", "positive"),
            (
                "from=a,to=b,tolerance=1,rotation-tolerance=0,parents=same",
                "positive",
            ),
            ("from=a,to=b,tolerance=1", "parents is required"),
            (
                "from=a,to=b,tolerance=1,parents=up",
                "parents=up is none of same or any",
            ),
        ] {
            let parsed: Options = options
                .split(',')
                .map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .unwrap()
                })
                .collect();
            let e = CORRESPOND_SKELETONS.instance(&parsed).err().unwrap_or_default();
            assert!(e.contains(why), "{options}: {e}");
        }
    }
}
