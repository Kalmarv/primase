//! Explicit per-pair animation transfer. No hub skeleton, implicit name mapping or mutation of the source World.
use std::collections::{BTreeMap, BTreeSet};
use primase_core::{Clip, ClipTarget, Entity, JointId, JointMap, Skeleton, World};
use primase_core::pass::{Pass, PassLog};
use serde::Serialize;
use crate::{Error, Result, RetargetClip, joint_table::JointTable, pose::{locals, model_transforms}};

/// Bone names and authored correspondence only; no extracted transforms, vertices or game payloads.
pub const SEKIRO_SKATE_TABLE: &str = include_str!("../tables/sekiro-human__skate-render.json");

/// A table can be used backwards for clip transfer, but a skin placement belongs to its original target frame.
/// Refuse such reversal rather than silently applying its offset in another frame.
pub fn reversed(table: &JointTable) -> Result<JointTable> {
    table.to_json()?;
    if !table.placements.is_empty() { return Err(Error("joint table with skin placements cannot be reversed without new placements".into())); }
    Ok(JointTable { version: table.version, from: table.to.clone(), to: table.from.clone(),
        pairs: table.pairs.iter().map(|(a, b)| (b.clone(), a.clone())).collect(),
        notes: table.pairs.iter().filter_map(|(a, b)| table.notes.get(a).map(|n| (b.clone(), n.clone()))).collect(),
        placements: BTreeMap::new() })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PairObservation {
    pub source: String,
    pub target: String,
    /// Missing or duplicate names prevent this pair from being resolved on this character.
    pub refusal: Option<String>,
    pub parents_agree: Option<bool>,
    /// Largest entry difference between the two stored local rest matrices. Not an angular measure.
    pub local_rest_max_delta: Option<f64>,
    /// Target/source distance from the nearest mapped ancestor at rest; None at a root or zero-length segment.
    pub length_ratio: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PairReport {
    pub pairs: Vec<PairObservation>,
    pub unmapped_source: Vec<String>,
    pub unmapped_target: Vec<String>,
}

/// Observe every stated pair on these actual skeletons. Missing pairs remain explicit, not suggestions to other
/// joints. A report does not confirm a table: the caller chooses whether to use only the pairs that exist.
pub fn inspect(from: &Skeleton, to: &Skeleton, table: &JointTable) -> Result<PairReport> {
    table.to_json()?;
    let names = |s: &Skeleton| {
        let mut out: BTreeMap<String, Vec<JointId>> = BTreeMap::new();
        for j in &s.joints { out.entry(j.name.clone()).or_default().push(j.id); }
        out
    };
    let (a, b) = (names(from), names(to));
    let (source_model, target_model) = (model_transforms(from, &locals(from, None))?, model_transforms(to, &locals(to, None))?);
    let mut resolved = BTreeMap::new();
    let mut pairs = Vec::new();
    for (s, t) in &table.pairs {
        let get = |names: &BTreeMap<String, Vec<JointId>>, name: &str, side: &str| -> std::result::Result<JointId, String> {
            match names.get(name).map(Vec::as_slice) {
                Some([id]) => Ok(*id), None => Err(format!("no {side} joint {name}")),
                _ => Err(format!("{side} joint name {name} is not unique")),
            }
        };
        let result = get(&a, s, "source").and_then(|sa| get(&b, t, "target").map(|tb| (sa, tb)));
        match result {
            Ok((sa, tb)) => { resolved.insert(sa, tb); pairs.push(PairObservation { source: s.clone(), target: t.clone(),
                refusal: None, parents_agree: None, local_rest_max_delta: None, length_ratio: None }); }
            Err(why) => pairs.push(PairObservation { source: s.clone(), target: t.clone(), refusal: Some(why),
                parents_agree: None, local_rest_max_delta: None, length_ratio: None }),
        }
    }
    let source: BTreeMap<_, _> = from.joints.iter().map(|j| (j.id, j)).collect();
    let target: BTreeMap<_, _> = to.joints.iter().map(|j| (j.id, j)).collect();
    for p in pairs.iter_mut().filter(|p| p.refusal.is_none()) {
        let (sa, tb) = (a[&p.source][0], b[&p.target][0]);
        p.parents_agree = Some(match (source[&sa].parent, target[&tb].parent) {
            (None, None) => true, (Some(parent), Some(other)) => resolved.get(&parent) == Some(&other), _ => false,
        });
        let (sm, tm) = (source[&sa].rest.matrix(), target[&tb].rest.matrix());
        p.local_rest_max_delta = Some(sm.cols.iter().flatten().zip(tm.cols.iter().flatten())
            .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs()).fold(0.0, f64::max));
        let mut parent = source[&sa].parent;
        while let Some(ancestor) = parent {
            if let Some(other) = resolved.get(&ancestor) {
                let distance = |x: [f64; 3], y: [f64; 3]| x.iter().zip(y).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt();
                let length = distance(source_model[&sa][3], source_model[&ancestor][3]);
                if length > 1e-9 { p.length_ratio = Some(distance(target_model[&tb][3], target_model[other][3]) / length); }
                break;
            }
            parent = source[&ancestor].parent;
        }
    }
    let targets: BTreeSet<_> = resolved.values().copied().collect();
    Ok(PairReport { pairs,
        unmapped_source: from.joints.iter().filter(|j| !resolved.contains_key(&j.id)).map(|j| j.name.clone()).collect(),
        unmapped_target: to.joints.iter().filter(|j| !targets.contains(&j.id)).map(|j| j.name.clone()).collect() })
}

/// Explicitly project a table onto pairs whose names exist uniquely on this character. Returns the full report so
/// a missing pair cannot vanish from a census/UI. Does not add guessed matches. Skin placements on absent pairs fail.
pub fn available_table(from: &Skeleton, to: &Skeleton, table: &JointTable) -> Result<(JointTable, PairReport)> {
    let report = inspect(from, to, table)?;
    let mut selected = table.clone();
    selected.pairs.retain(|(a, b)| report.pairs.iter().any(|p| &p.source == a && &p.target == b && p.refusal.is_none()));
    selected.notes.retain(|name, _| selected.pairs.iter().any(|(a, _)| a == name));
    selected.to_json()?;
    if selected.pairs.is_empty() { return Err(Error("no stated joint pairs resolve on these skeletons".into())); }
    Ok((selected, report))
}

/// Transfer one core clip through the strict table. Runs the existing retarget-clip arithmetic on two skeleton
/// copies and this clip only. Refused/unchanged output is an error with the pass's actual reasons. The returned
/// joint channels name `target`, suitable for insertion in another World without importing source meshes/images.
/// RootMotion and control channels stay as the existing pass states; morph channels require an explicit mesh map.
pub fn retarget_one(clip: &Clip, from: &Skeleton, to: &Skeleton, target: Entity, table: &JointTable,
    options: RetargetClip) -> Result<(Clip, PassLog)> {
    table.to_json()?;
    let pairs = table.resolve(from, to)?;
    if pairs.is_empty() { return Err(Error("retarget needs at least one stated joint pair".into())); }
    let drives: BTreeSet<_> = clip.channels.iter().filter_map(|c| match c.target { ClipTarget::Joint { skeleton, .. } => Some(skeleton), _ => None }).collect();
    if drives.len() != 1 { return Err(Error("retarget-one requires a clip driving exactly one source skeleton".into())); }
    if clip.channels.iter().any(|c| matches!(c.target, ClipTarget::Morph { .. })) {
        return Err(Error("retarget-one cannot copy morph channels without a stated mesh correspondence".into()));
    }
    let mut world = World::new();
    let source = world.spawn(); let destination = world.spawn(); let entity = world.spawn(); let map = world.spawn();
    world.insert(source, from.clone()); world.insert(destination, to.clone());
    let mut staged = clip.clone();
    for ch in &mut staged.channels { if let ClipTarget::Joint { skeleton, .. } = &mut ch.target { *skeleton = source; } }
    world.insert(entity, staged); world.insert(map, JointMap { from: source, to: destination, pairs });
    let failures = world.verify_components();
    if !failures.is_empty() { return Err(Error(format!("retarget input components: {failures:?}"))); }
    let mut log = PassLog::default();
    options.run(&mut world, &mut log).map_err(Error)?;
    let failures = world.verify_components();
    if !failures.is_empty() { return Err(Error(format!("retarget output components: {failures:?}"))); }
    let mut out = world.get::<Clip>(entity).ok_or_else(|| Error("retarget produced no clip".into()))?.clone();
    if !out.channels.iter().any(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton == destination))
        || out.channels.iter().any(|c| matches!(c.target, ClipTarget::Joint { skeleton, .. } if skeleton != destination)) {
        return Err(Error(format!("retarget left clip on its source: {:?}", log.omitted)));
    }
    for ch in &mut out.channels { if let ClipTarget::Joint { skeleton, .. } = &mut ch.target { *skeleton = target; } }
    Ok((out, log))
}
