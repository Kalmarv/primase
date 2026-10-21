//! `assemble-character`: the assets of a world that each hold a rig (a character's parts, each read with the skeleton
//! and bindings its source stores for it) related into one skeleton for animation, by a stated rule.
//!
//! The parts are not merged. Every part keeps its own skeleton entity and its own skin; what the pass adds is the
//! relation between them: one assembled skeleton, and for each part a `core.joint_map` from its skeleton to the
//! assembled one that lists every joint of the part. A clip on the assembled skeleton reaches a part through
//! `map-clip`, a part's skin moves onto the assembled skeleton through `retarget-skin`, and `reference-pose` says
//! whether this pass itself leaves each part bound to its own skeleton and rest (`own`) or rebinds it (`assembled`).
//! Name equality alone is never taken as evidence that two joints behave alike: the pass compares their rests and
//! refuses what disagrees, and a part's rest cancelling out at its own rest says nothing about how the relation holds
//! once the joints move.
//!
//! **Correspondence** (`correspondence=`) says which joints of different parts are one joint of the assembled
//! skeleton:
//! - `by-name`: joints with equal names. A name repeated inside one part cannot say which joint it means: it is
//!   refused when another part holds the name too, and otherwise its joints stay distinct (nothing to correspond).
//! - `table`: the `core.joint_map` entities of the world whose two ends are skeletons of the parts (a person's reviewed
//!   table resolved by [`crate::joint_table::JointTable::resolve`]); pairs join their joints, transitively, and joints
//!   no pair names stay distinct even where their names are equal.
//!
//! One joint holds at most one joint of each part. An assembled joint takes the name of its first joint in part order
//! (the order of the assets in the world).
//!
//! **Hierarchy and rest.** A part stores each bone with the parent it has in that part: a bone whose parent the part
//! does not hold is a root there, and its stored rest then lies in a space the part does not hold (a *detached* root;
//! the Wolf's parts keep dozens, mostly joints no vertex uses, with rests that differ from part to part by metres).
//! An assembled joint takes its parent from the parts that state one (refused where they name different joints, or
//! where the parents form a cycle); a joint no part gives a parent is a root. Its rest is taken, as stored, from the
//! first part that states a parent for it (any part, for a root), preferring a part whose skin weights the joint. Every
//! part's rest for it is compared with the assembled one, in the space of the highest joint of that part's chain above
//! it (the model space when that joint is a root of the assembled skeleton too, so a detached root's own rest is left
//! out and counted): the largest entry of the difference of the two 4x3 matrices (metres for the translation, the
//! rotation and scale entries as they are). Which disagreements are refused:
//! - a part whose skin weights the joint and whose rest differs by more than `tolerance=`: always, the vertices
//!   depend on it;
//! - `unweighted-rests=refuse`: any part's, weighted or not, beyond `tolerance=`;
//! - `unweighted-rests=report`: a part that does not weight the joint is not refused; its disagreement is reported.
//!
//! Each refused joint is named with its disagreement, every one in the one refusal; every disagreement above 1e-6
//! that is not refused is reported as an approximation, with its amount.
//!
//! **Reference pose** (`reference-pose=`): a part's skins are all those that bind to its skeleton (a prototype's and
//! those of its level-of-detail forms).
//! - `own`: every part stays bound to its own skeleton, rest and inverse binds as stored. Reported, per part, how far
//!   its vertices would sit from their rest if the assembled rest posed its joints directly (the part's rest and the
//!   assembled one differ where the parts disagree); animating through the joint maps with the part's own rest as the
//!   base (`map-clip{rests=target}`) has no such displacement.
//! - `assembled`: every skinned binding of a part moves onto the assembled skeleton: each slot names the assembled
//!   joint and takes the inverse of the assembled rest as its inverse bind, the vertices as stored. That is exact only
//!   where the part's own model-space rest of each joint that carries weight agrees with the assembled one, so a
//!   weighted joint that differs by more than `tolerance=` is refused, and one that differs at all is reported with the
//!   amount its inverse bind moved. The parts' skeletons stay in the world, as the maps' sources.
//!
//! The result is one asset named `name=` (`core.source` `assembled/<name>`) holding the assembled skeleton as its rig
//! (and, with `reference-pose=own`, each part's skeleton too: its skins bind to them) and every member of the parts'
//! assets other than their rigs; the parts' own asset roots are removed.
//!
//! All five options are required. Approximations, reported: joints whose rests differ within the tolerance, the
//! vertex displacement measure of `own`, and the inverse binds `assembled` moves.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Asset, Component, Entity, Joint, JointId, JointMap, Mesh, Name, Role, Skeleton, Skin, Source, World,
};

use crate::deform::{Vertices, influences, skin};
use crate::pose::{Affine, affine, compose, inverse, locals, model_transforms, transform};

/// A rest difference at or below this (single-precision noise of stored values) is not a disagreement.
const REPORT_FLOOR: f64 = 1e-6;

/// Which joints of different parts are one joint (`correspondence=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Correspondence {
    /// Equal names.
    ByName,
    /// The world's `core.joint_map` entities between the parts' skeletons.
    Table,
}

/// What the pass does with the parts' bindings (`reference-pose=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferencePose {
    /// Each part stays bound to its own skeleton and rest.
    Own,
    /// Each part is rebound to the assembled skeleton at its rest.
    Assembled,
}

/// What the pass does with a rest disagreement of a joint the part does not weight (`unweighted-rests=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnweightedRests {
    /// Reported, not refused.
    Report,
    /// Refused beyond the tolerance like any other.
    Refuse,
}

/// `assemble-character{correspondence=by-name|table, reference-pose=own|assembled, tolerance=<number>,
/// unweighted-rests=report|refuse, name=<word>}` (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct AssembleCharacter {
    pub correspondence: Correspondence,
    pub reference_pose: ReferencePose,
    /// The largest rest disagreement accepted.
    pub tolerance: f64,
    pub unweighted_rests: UnweightedRests,
    /// The assembled asset's name.
    pub name: String,
}

/// The pass factory.
pub fn assemble_character(o: &Options) -> Result<Box<dyn Pass>, String> {
    let (mut correspondence, mut reference_pose, mut tolerance, mut unweighted, mut name) =
        (None, None, None, None, None);
    for (k, v) in o {
        match k.as_str() {
            "correspondence" => {
                correspondence = Some(match v.as_str() {
                    "by-name" => Correspondence::ByName,
                    "table" => Correspondence::Table,
                    _ => return Err(format!("correspondence={v}: by-name or table")),
                });
            }
            "reference-pose" => {
                reference_pose = Some(match v.as_str() {
                    "own" => ReferencePose::Own,
                    "assembled" => ReferencePose::Assembled,
                    _ => return Err(format!("reference-pose={v}: own or assembled")),
                });
            }
            "tolerance" => {
                let t: f64 = v.parse().map_err(|_| format!("tolerance={v} is not a number"))?;
                if !t.is_finite() || t < 0.0 {
                    return Err(format!("tolerance={v}: a finite number, zero or more"));
                }
                tolerance = Some(t);
            }
            "unweighted-rests" => {
                unweighted = Some(match v.as_str() {
                    "report" => UnweightedRests::Report,
                    "refuse" => UnweightedRests::Refuse,
                    _ => return Err(format!("unweighted-rests={v}: report or refuse")),
                });
            }
            "name" => {
                if v.is_empty() {
                    return Err("name= is empty".into());
                }
                name = Some(v.clone());
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(AssembleCharacter {
        correspondence: correspondence.ok_or("correspondence=by-name|table is required")?,
        reference_pose: reference_pose.ok_or("reference-pose=own|assembled is required")?,
        tolerance: tolerance.ok_or("tolerance=<largest rest disagreement accepted> is required")?,
        unweighted_rests: unweighted.ok_or("unweighted-rests=report|refuse is required")?,
        name: name.ok_or("name=<the assembled character's name> is required")?,
    }))
}

/// One asset that holds a rig.
struct Part {
    root: Entity,
    label: String,
    rig: Entity,
    skeleton: Skeleton,
    /// Every joint's transform into the skeleton's space at rest.
    rests: BTreeMap<JointId, Affine>,
    /// The members of its asset other than the rig.
    members: Vec<(Entity, Role)>,
}

impl Part {
    /// The index of the highest joint above (or at) joint `j` of this part.
    fn top(&self, mut j: usize) -> usize {
        while let Some(p) = self.skeleton.joints[j]
            .parent
            .and_then(|p| self.skeleton.index_of(p))
        {
            j = p;
        }
        j
    }
}

/// The largest entry of the difference of two affine maps.
fn gap(a: &Affine, b: &Affine) -> f64 {
    a.iter()
        .zip(b)
        .flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs()))
        .fold(0.0, f64::max)
}

fn gather(w: &World) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    for (root, asset) in w.iter::<Asset>() {
        let label = w
            .get::<Name>(root)
            .map_or_else(|| format!("{root:?}"), |n| n.0.clone());
        let rigs: Vec<Entity> = asset
            .members
            .iter()
            .filter(|(_, r)| *r == Role::Rig)
            .map(|(e, _)| *e)
            .collect();
        let rig = match rigs.as_slice() {
            [] => continue,
            [rig] => *rig,
            _ => return Err(format!("asset {label} has {} rigs: a part has one", rigs.len())),
        };
        let skeleton = w
            .get::<Skeleton>(rig)
            .ok_or_else(|| format!("asset {label}: its rig holds no core.skeleton"))?
            .clone();
        let rests =
            model_transforms(&skeleton, &locals(&skeleton, None)).map_err(|e| format!("{label}: {}", e.0))?;
        parts.push(Part {
            root,
            label,
            rig,
            skeleton,
            rests,
            members: asset
                .members
                .iter()
                .filter(|(_, r)| *r != Role::Rig)
                .copied()
                .collect(),
        });
    }
    if parts.len() < 2 {
        return Err(format!(
            "{} asset(s) with a rig: assembling takes at least two",
            parts.len()
        ));
    }
    Ok(parts)
}

/// Which assembled joint each joint of each part is: `of[part][joint index]`.
fn by_name(parts: &[Part]) -> Result<Vec<Vec<usize>>, String> {
    let mut holders: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    let mut repeated = BTreeSet::new();
    for (p, part) in parts.iter().enumerate() {
        let mut seen = BTreeSet::new();
        for j in &part.skeleton.joints {
            holders.entry(&j.name).or_default().insert(p);
            if !seen.insert(j.name.as_str()) {
                repeated.insert((p, j.name.as_str()));
            }
        }
    }
    if let Some((p, name)) = repeated.iter().find(|(_, n)| holders[n].len() > 1) {
        return Err(format!(
            "joint name {name} appears twice in {} and is held by another part: by-name cannot say which joint \
             corresponds (correspondence=table states it)",
            parts[*p].label
        ));
    }
    let mut next = 0;
    let mut named: BTreeMap<&str, usize> = BTreeMap::new();
    let mut of = Vec::new();
    for (p, part) in parts.iter().enumerate() {
        let mut row = Vec::new();
        for j in &part.skeleton.joints {
            let class = if repeated.contains(&(p, j.name.as_str())) {
                next += 1;
                next - 1
            } else {
                *named.entry(&j.name).or_insert_with(|| {
                    next += 1;
                    next - 1
                })
            };
            row.push(class);
        }
        of.push(row);
    }
    Ok(of)
}

/// The same, from the world's joint maps between the parts' skeletons.
fn by_table(parts: &[Part], w: &World) -> Result<Vec<Vec<usize>>, String> {
    let offsets: Vec<usize> = parts
        .iter()
        .scan(0, |at, p| {
            let o = *at;
            *at += p.skeleton.joints.len();
            Some(o)
        })
        .collect();
    let total: usize = parts.iter().map(|p| p.skeleton.joints.len()).sum();
    let part_of: BTreeMap<Entity, usize> = parts.iter().enumerate().map(|(p, x)| (x.rig, p)).collect();
    let mut parent: Vec<usize> = (0..total).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut used = 0;
    for (_, m) in w.iter::<JointMap>() {
        let (Some(&a), Some(&b)) = (part_of.get(&m.from), part_of.get(&m.to)) else {
            continue;
        };
        used += 1;
        for &(x, y) in &m.pairs {
            let (Some(i), Some(j)) = (parts[a].skeleton.index_of(x), parts[b].skeleton.index_of(y)) else {
                return Err("a joint map names a joint its skeleton does not hold".into());
            };
            let (ra, rb) = (
                find(&mut parent, offsets[a] + i),
                find(&mut parent, offsets[b] + j),
            );
            // the lower flat index stays the representative, so classes keep the order of their first joint
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    if used == 0 {
        return Err(
            "correspondence=table: the world holds no core.joint_map between the parts' skeletons".into(),
        );
    }
    let mut class_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut of = Vec::new();
    for (p, part) in parts.iter().enumerate() {
        let mut row = Vec::new();
        for i in 0..part.skeleton.joints.len() {
            let root = find(&mut parent, offsets[p] + i);
            let next = class_of_root.len();
            row.push(*class_of_root.entry(root).or_insert(next));
        }
        of.push(row);
    }
    Ok(of)
}

/// The assembled joints: who is in each, their names, parents and the part whose rest each takes.
struct Assembled {
    members: Vec<Vec<(usize, usize)>>,
    names: Vec<String>,
    parents: Vec<Option<usize>>,
    /// The (part, joint index) each joint's rest is taken from.
    source: Vec<(usize, usize)>,
}

impl Assembled {
    fn resolve(
        parts: &[Part],
        of: &[Vec<usize>],
        weighted: &[BTreeSet<JointId>],
    ) -> Result<Assembled, String> {
        let count = of.iter().flatten().max().map_or(0, |m| m + 1);
        let mut members = vec![Vec::new(); count];
        for (p, row) in of.iter().enumerate() {
            for (j, &c) in row.iter().enumerate() {
                members[c].push((p, j));
            }
        }
        let names: Vec<String> = members
            .iter()
            .map(|m| parts[m[0].0].skeleton.joints[m[0].1].name.clone())
            .collect();
        let mut problems = Vec::new();
        let mut parents = vec![None; count];
        let mut source = Vec::with_capacity(count);
        for (c, m) in members.iter().enumerate() {
            let mut seen = BTreeSet::new();
            if let Some(&(p, _)) = m.iter().find(|(p, _)| !seen.insert(*p)) {
                problems.push(format!(
                    "joint {}: two joints of {} correspond to it",
                    names[c], parts[p].label
                ));
            }
            // the parents the parts state
            let stated: Vec<(usize, usize, usize)> = m
                .iter()
                .filter_map(|&(p, j)| {
                    let part = &parts[p];
                    let up = part.skeleton.joints[j]
                        .parent
                        .and_then(|id| part.skeleton.index_of(id))?;
                    Some((p, j, of[p][up]))
                })
                .collect();
            let classes: BTreeSet<usize> = stated.iter().map(|s| s.2).collect();
            if classes.len() > 1 {
                let detail: Vec<String> = stated
                    .iter()
                    .map(|&(p, _, up)| format!("{} in {}", names[up], parts[p].label))
                    .collect();
                problems.push(format!(
                    "joint {}: parents differ ({})",
                    names[c],
                    detail.join(", ")
                ));
            }
            parents[c] = stated.first().map(|s| s.2);
            // its rest from a part that states its parent (any part for a root), one that weights it first
            let candidates: Vec<(usize, usize)> = if stated.is_empty() {
                m.clone()
            } else {
                stated.iter().map(|s| (s.0, s.1)).collect()
            };
            source.push(
                candidates
                    .iter()
                    .copied()
                    .find(|&(p, j)| weighted[p].contains(&parts[p].skeleton.joints[j].id))
                    .unwrap_or(candidates[0]),
            );
        }
        if let Some(p) = (0..count).find(|&c| {
            let mut at = c;
            for _ in 0..=count {
                match parents[at] {
                    Some(up) => at = up,
                    None => return false,
                }
            }
            true
        }) {
            problems.push(format!("joint {}: its parents form a cycle", names[p]));
        }
        if problems.is_empty() {
            Ok(Assembled {
                members,
                names,
                parents,
                source,
            })
        } else {
            Err(problems.join("; "))
        }
    }

    fn skeleton(&self, parts: &[Part]) -> Skeleton {
        Skeleton {
            joints: (0..self.names.len())
                .map(|c| {
                    let (p, j) = self.source[c];
                    Joint {
                        id: JointId(c as u32),
                        name: self.names[c].clone(),
                        parent: self.parents[c].map(|up| JointId(up as u32)),
                        rest: parts[p].skeleton.joints[j].rest,
                    }
                })
                .collect(),
        }
    }
}

/// The joints of a part that carry weight in the skin of `proto` (a prototype or a form of one) bound to the part's rig.
fn weighted(w: &World, proto: Entity, rig: Entity) -> Result<BTreeSet<JointId>, String> {
    let mut out = BTreeSet::new();
    let (Some(s), Some(mesh)) = (w.get::<Skin>(proto), w.get::<Mesh>(proto)) else {
        return Ok(out);
    };
    for b in s.bindings.iter().filter(|b| b.skeleton == rig) {
        for (i, prim) in mesh.primitives().iter().enumerate() {
            let (Some(Some(how)), Some(Some(palette))) = (s.influences.get(i), b.palettes.get(i)) else {
                continue;
            };
            let inf =
                influences(prim, *how, s.weights).map_err(|e| format!("{proto:?} primitive {i}: {}", e.0))?;
            out.extend(
                inf.iter()
                    .flatten()
                    .filter(|(_, wt)| *wt > 0.0)
                    .map(|&(slot, _)| palette[slot].joint),
            );
        }
    }
    Ok(out)
}

/// What comparing the parts' rests found: each corresponded joint's difference as (joint, part, the joint's index in
/// the part, difference), and the detached roots left out as (part, joint index).
type Disagreements = (Vec<(usize, usize, usize, f64)>, Vec<(usize, usize)>);

impl AssembleCharacter {
    /// Each corresponded joint's rest in each part against the assembled one: (joint, part, index, difference),
    /// and the detached roots (a root of a part that another part parents) left out.
    fn disagreements(
        &self,
        parts: &[Part],
        of: &[Vec<usize>],
        asm: &Assembled,
        rests: &BTreeMap<JointId, Affine>,
    ) -> Result<Disagreements, String> {
        let id = |c: usize| JointId(c as u32);
        let (mut found, mut detached) = (Vec::new(), Vec::new());
        for (c, m) in asm.members.iter().enumerate().filter(|(_, m)| m.len() > 1) {
            for &(p, j) in m {
                let part = &parts[p];
                let t = part.top(j);
                let top_class = of[p][t];
                let own = part.rests[&part.skeleton.joints[j].id];
                let here = rests[&id(c)];
                let d = if asm.parents[top_class].is_some() {
                    if t == j {
                        detached.push((p, j));
                        continue;
                    }
                    let own_top = part.rests[&part.skeleton.joints[t].id];
                    let top = rests[&id(top_class)];
                    let (Some(a), Some(b)) = (inverse(&own_top), inverse(&top)) else {
                        return Err(format!("joint {}: a rest has no inverse", asm.names[top_class]));
                    };
                    gap(&compose(&a, &own), &compose(&b, &here))
                } else {
                    gap(&own, &here)
                };
                found.push((c, p, j, d));
            }
        }
        Ok((found, detached))
    }
}

impl Pass for AssembleCharacter {
    fn name(&self) -> &'static str {
        "assemble-character"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "correspondence".to_owned(),
                match self.correspondence {
                    Correspondence::ByName => "by-name",
                    Correspondence::Table => "table",
                }
                .to_owned(),
            ),
            (
                "reference-pose".to_owned(),
                match self.reference_pose {
                    ReferencePose::Own => "own",
                    ReferencePose::Assembled => "assembled",
                }
                .to_owned(),
            ),
            ("tolerance".to_owned(), self.tolerance.to_string()),
            (
                "unweighted-rests".to_owned(),
                match self.unweighted_rests {
                    UnweightedRests::Report => "report",
                    UnweightedRests::Refuse => "refuse",
                }
                .to_owned(),
            ),
            ("name".to_owned(), self.name.clone()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME, Skeleton::NAME, Skin::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Skeleton::NAME,
            JointMap::NAME,
            Asset::NAME,
            Name::NAME,
            Source::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Skin::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "parts whose rests for a joint differ within the tolerance related as one joint; with reference-pose=assembled, inverse binds moved to the assembled rest"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let parts = gather(w)?;
        // every entity whose skin binds to a part's rig (its prototypes and their level-of-detail forms), and the
        // joints of the part that carry weight in each
        let mut prototypes: Vec<Vec<(Entity, BTreeSet<JointId>)>> = Vec::new();
        for part in &parts {
            let mut row = Vec::new();
            for (e, s) in w.iter::<Skin>() {
                if s.bindings.iter().any(|b| b.skeleton == part.rig) {
                    row.push((e, weighted(w, e, part.rig)?));
                }
            }
            prototypes.push(row);
        }
        let carrying: Vec<BTreeSet<JointId>> = prototypes
            .iter()
            .map(|row| row.iter().flat_map(|(_, j)| j.iter().copied()).collect())
            .collect();
        let of = match self.correspondence {
            Correspondence::ByName => by_name(&parts)?,
            Correspondence::Table => by_table(&parts, w)?,
        };
        let asm = Assembled::resolve(&parts, &of, &carrying)?;
        let skeleton = asm.skeleton(&parts);
        let rests = model_transforms(&skeleton, &locals(&skeleton, None)).map_err(|e| e.0)?;
        let id = |c: usize| JointId(c as u32);
        let mut problems = Vec::new();

        let (found, detached) = self.disagreements(&parts, &of, &asm, &rests)?;
        let mut within = Vec::new();
        for &(c, p, j, d) in &found {
            let weighted_here = carrying[p].contains(&parts[p].skeleton.joints[j].id);
            let line = format!(
                "joint {}: the rest in {} differs from the assembled rest (taken from {}) by {d:.3e}",
                asm.names[c], parts[p].label, parts[asm.source[c].0].label
            );
            if d > self.tolerance && (weighted_here || self.unweighted_rests == UnweightedRests::Refuse) {
                problems.push(line);
            } else if d > REPORT_FLOOR {
                within.push(if weighted_here {
                    line
                } else {
                    format!("{line} (the part does not weight the joint)")
                });
            }
        }
        if self.reference_pose == ReferencePose::Assembled {
            for (p, part) in parts.iter().enumerate() {
                for &jid in &carrying[p] {
                    let j = part
                        .skeleton
                        .index_of(jid)
                        .ok_or("a weighted joint is not in its skeleton")?;
                    let d = gap(&part.rests[&jid], &rests[&id(of[p][j])]);
                    if d > self.tolerance {
                        problems.push(format!(
                            "joint {} carries weight in {} and its rest there differs from the assembled rest by \
                             {d:.3e}: rebinding would move the part",
                            part.skeleton.joints[j].name, part.label
                        ));
                    }
                }
            }
        }
        if !problems.is_empty() {
            return Err(format!(
                "{} joint rest disagreement(s) beyond tolerance {}: {}",
                problems.len(),
                self.tolerance,
                problems.join("; ")
            ));
        }

        // the reports that need the parts as they are
        if self.reference_pose == ReferencePose::Own {
            for (p, part) in parts.iter().enumerate() {
                let drive: BTreeMap<JointId, Affine> = part
                    .skeleton
                    .joints
                    .iter()
                    .enumerate()
                    .map(|(j, joint)| (joint.id, rests[&id(of[p][j])]))
                    .collect();
                let (mut most, mut moved) = (0.0f64, 0u64);
                for (proto, _) in &prototypes[p] {
                    let (Some(s), Some(mesh)) = (w.get::<Skin>(*proto), w.get::<Mesh>(*proto)) else {
                        continue;
                    };
                    for b in s.bindings.iter().filter(|b| b.skeleton == part.rig) {
                        for (i, prim) in mesh.primitives().iter().enumerate() {
                            let (Some(Some(how)), Some(Some(palette))) =
                                (s.influences.get(i), b.palettes.get(i))
                            else {
                                continue;
                            };
                            let v = Vertices::of(prim).map_err(|e| e.0)?;
                            let at = |pose: &BTreeMap<JointId, Affine>| {
                                skin(&v, prim, *how, s.weights, b.deformation, palette, pose).map_err(|e| e.0)
                            };
                            let (own, driven) = (at(&part.rests)?, at(&drive)?);
                            for (a, b) in own.positions.iter().zip(&driven.positions) {
                                let d = (0..3).map(|k| f64::from(a[k] - b[k]).powi(2)).sum::<f64>().sqrt();
                                most = most.max(d);
                                moved += u64::from(d > 1e-5);
                            }
                        }
                    }
                }
                if moved > 0 {
                    log.approximate(
                        &format!(
                            "vertices of {} sit up to {most:.3e} m from their rest when the assembled rest poses its joints directly",
                            part.label
                        ),
                        moved,
                    );
                }
            }
        }

        // the world changes from here
        let character = w.spawn();
        w.insert(character, Name(self.name.clone()));
        w.insert(character, Source(format!("assembled/{}", self.name)));
        let assembled = w.spawn();
        w.insert(assembled, Name(format!("{} skeleton", self.name)));
        w.insert(assembled, skeleton);
        let mut asset = Asset {
            members: vec![(assembled, Role::Rig)],
        };
        for (p, part) in parts.iter().enumerate() {
            let map = w.spawn();
            w.insert(
                map,
                JointMap {
                    from: part.rig,
                    to: assembled,
                    pairs: part
                        .skeleton
                        .joints
                        .iter()
                        .enumerate()
                        .map(|(j, joint)| (joint.id, id(of[p][j])))
                        .collect(),
                },
            );
            if self.reference_pose == ReferencePose::Own {
                asset.members.push((part.rig, Role::Rig));
            }
            asset.members.extend(part.members.iter().copied());
        }
        if self.reference_pose == ReferencePose::Assembled {
            for (p, part) in parts.iter().enumerate() {
                let (mut moved_unused, mut moved_weighted) = (0u64, BTreeMap::<String, f64>::new());
                for (proto, carrying) in &prototypes[p] {
                    let s = w.get_mut::<Skin>(*proto).ok_or("a skin is gone")?;
                    for b in s.bindings.iter_mut().filter(|b| b.skeleton == part.rig) {
                        b.skeleton = assembled;
                        for slot in b.palettes.iter_mut().flatten().flatten() {
                            let j = part
                                .skeleton
                                .index_of(slot.joint)
                                .ok_or("a slot names a joint its skeleton does not hold")?;
                            let c = of[p][j];
                            let new = inverse(&rests[&id(c)]).ok_or("an assembled rest has no inverse")?;
                            let d = gap(&affine(&slot.inverse_bind), &new);
                            if carrying.contains(&slot.joint) {
                                let e = moved_weighted.entry(asm.names[c].clone()).or_default();
                                *e = e.max(d);
                            } else {
                                moved_unused += 1;
                            }
                            slot.joint = id(c);
                            slot.inverse_bind = transform(&new);
                        }
                    }
                }
                for (joint, d) in moved_weighted.into_iter().filter(|(_, d)| *d > REPORT_FLOOR) {
                    log.approximate(
                        &format!("joint {joint}: the inverse bind of {} moved to the assembled rest by up to {d:.3e}", part.label),
                        1,
                    );
                }
                log.count(
                    &format!(
                        "unweighted palette slots of {} rebound to the assembled skeleton",
                        part.label
                    ),
                    moved_unused,
                );
            }
        }
        w.insert(character, asset);
        for part in &parts {
            w.despawn(part.root);
        }
        for line in within {
            log.approximate(&line, 1);
        }
        log.count("parts assembled", parts.len() as u64);
        log.count("assembled joints", asm.names.len() as u64);
        log.count(
            "assembled joints held by more than one part",
            asm.members.iter().filter(|m| m.len() > 1).count() as u64,
        );
        log.count(
            "roots of a part that another part parents, left out of the rest comparison",
            detached.len() as u64,
        );
        log.count(
            "assembled joints named unlike a joint that corresponds to them",
            asm.members
                .iter()
                .enumerate()
                .filter(|(c, m)| {
                    m.iter()
                        .any(|&(p, j)| parts[p].skeleton.joints[j].name != asm.names[*c])
                })
                .count() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{
        Binding, BindingId, Deformation, Influences, Primitive, Rest, Semantic, Slot, Stream, Topology,
        WeightRule,
    };

    fn at(t: [f32; 3]) -> Rest {
        Rest::Trs {
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    /// A joint of a fixture part: name, parent index, translation.
    type J = (&'static str, Option<u32>, [f32; 3]);

    /// A part: an asset root named `label`, its skeleton and a prototype of points, each on a joint (by index) and
    /// stored in the part's own model space, skinned wholly to that joint with the inverse of its model rest.
    fn part(w: &mut World, label: &str, joints: &[J], points: &[([f32; 3], u32)]) -> Entity {
        let skeleton = Skeleton {
            joints: joints
                .iter()
                .enumerate()
                .map(|(i, &(name, parent, t))| Joint {
                    id: JointId(i as u32),
                    name: name.to_owned(),
                    parent: parent.map(JointId),
                    rest: at(t),
                })
                .collect(),
        };
        let rests = model_transforms(&skeleton, &locals(&skeleton, None)).unwrap();
        let palette: Vec<Slot> = skeleton
            .joints
            .iter()
            .map(|j| Slot {
                joint: j.id,
                inverse_bind: transform(&inverse(&rests[&j.id]).unwrap()),
            })
            .collect();
        let rig = w.spawn();
        w.insert(rig, Name(format!("{label} skeleton")));
        w.insert(rig, skeleton);
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [
                    (
                        Semantic::Position,
                        Stream::F32x3(points.iter().map(|p| p.0).collect()),
                    ),
                    (
                        Semantic::Joints(0),
                        Stream::U8x4(points.iter().map(|p| [p.1 as u8, 0, 0, 0]).collect()),
                    ),
                    (
                        Semantic::Weights(0),
                        Stream::F32x4(points.iter().map(|_| [1.0, 0.0, 0.0, 0.0]).collect()),
                    ),
                ]
                .into(),
                indices: None,
                material: None,
            }]),
        );
        w.insert(
            proto,
            Skin {
                influences: vec![Some(Influences::Streams { sets: 1 })],
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton: rig,
                    palettes: vec![Some(palette)],
                    deformation: Deformation::Linear,
                }],
            },
        );
        let root = w.spawn();
        w.insert(root, Name(label.to_owned()));
        w.insert(
            root,
            Asset {
                members: vec![(rig, Role::Rig), (proto, Role::Part)],
            },
        );
        proto
    }

    const BODY: [J; 4] = [
        ("Master", None, [0.0; 3]),
        ("Spine", Some(0), [0.0, 1.0, 0.0]),
        ("Neck", Some(1), [0.0, 0.5, 0.0]),
        ("L_Arm", Some(1), [0.3, 0.0, 0.0]),
    ];

    /// The head part: its Spine is a root here when `spine` is `None` (its parent is in the body part), with a stored
    /// rest in a space the part does not hold; its Neck sits `neck` above it.
    fn head_joints(spine: Option<[f32; 3]>, neck: [f32; 3]) -> [J; 4] {
        [
            ("Master", None, [0.0; 3]),
            ("Spine", spine.map(|_| 0), spine.unwrap_or([9.0, 9.0, 9.0])),
            ("Neck", Some(1), neck),
            ("Head", Some(2), [0.0, 0.2, 0.0]),
        ]
    }

    /// Body and head in a world: the body's points on its Neck and arm, the head's on its Head, each at where the
    /// part's own rest puts its joint (so the stored positions are in each part's model space).
    fn world(spine: Option<[f32; 3]>, neck: [f32; 3]) -> (World, Entity, Entity) {
        let mut w = World::new();
        let body = part(
            &mut w,
            "body",
            &BODY,
            &[([0.0, 1.6, 0.0], 2), ([0.4, 1.0, 0.0], 3)],
        );
        let root = spine.unwrap_or([9.0, 9.0, 9.0]);
        let head_at = [root[0] + neck[0], root[1] + neck[1] + 0.3, root[2] + neck[2]];
        let head = part(&mut w, "head", &head_joints(spine, neck), &[(head_at, 3)]);
        assert_eq!(w.verify_components(), vec![]);
        (w, body, head)
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

    /// The one asset of the world, and its rig's skeleton.
    fn assembled(w: &World) -> (Entity, Skeleton) {
        assert_eq!(w.iter::<Asset>().count(), 1);
        let (_, asset) = w.iter::<Asset>().next().unwrap();
        let rig = asset.members.iter().find(|m| m.1 == Role::Rig).unwrap().0;
        (rig, w.get::<Skeleton>(rig).unwrap().clone())
    }

    const OWN: &str = "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,name=hero}";

    /// The parts join into five joints: Master, Spine, Neck and the arm and head once each; the head part's Spine is a
    /// detached root, so it takes its parent and rest from the body's and is not compared. Every part keeps its
    /// skeleton and skin, each with a joint map listing all its joints.
    #[test]
    fn parts_keep_their_bindings_and_relate_through_joint_maps() {
        let (mut w, body, head) = world(None, [0.0, 0.5, 0.0]);
        let before: Vec<Skin> = [body, head]
            .iter()
            .map(|p| w.get::<Skin>(*p).unwrap().clone())
            .collect();
        let report = run(&mut w, OWN).unwrap();
        let (rig, s) = assembled(&w);
        let names: Vec<(&str, Option<u32>)> = s
            .joints
            .iter()
            .map(|j| (j.name.as_str(), j.parent.map(|p| p.0)))
            .collect();
        assert_eq!(
            names,
            [
                ("Master", None),
                ("Spine", Some(0)),
                ("Neck", Some(1)),
                ("L_Arm", Some(1)),
                ("Head", Some(2))
            ]
        );
        assert_eq!(s.joints[1].rest, at([0.0, 1.0, 0.0]));
        for (p, was) in [body, head].iter().zip(&before) {
            assert_eq!(w.get::<Skin>(*p).unwrap(), was);
        }
        let asset = w.iter::<Asset>().next().unwrap().1;
        assert_eq!(asset.members.len(), 5);
        assert_eq!(asset.members[0], (rig, Role::Rig));
        assert_eq!(w.iter::<JointMap>().count(), 2);
        for (_, m) in w.iter::<JointMap>() {
            assert_eq!(m.to, rig);
            assert_eq!(m.pairs.len(), w.get::<Skeleton>(m.from).unwrap().joints.len());
        }
        assert_eq!(w.verify_components(), vec![]);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["assembled joints"], 5);
        assert_eq!(log.counts["assembled joints held by more than one part"], 3);
        assert_eq!(
            log.counts["roots of a part that another part parents, left out of the rest comparison"],
            1
        );
        // the head's vertex is 15.03 m from where the assembled rest would put it: its part stores a detached chain
        assert_eq!(
            log.approximations["vertices of head sit up to 1.503e1 m from their rest when the assembled rest poses its joints directly"],
            1
        );
        assert!(
            !log.approximations.keys().any(|k| k.contains("body")),
            "{:?}",
            log.approximations
        );
    }

    #[test]
    fn rests_that_disagree_are_refused_each_by_name_and_reported_within_the_tolerance() {
        // the head part's Neck sits 3 mm higher than the body part's
        let (mut w, ..) = world(None, [0.0, 0.503, 0.0]);
        let err = run(&mut w, OWN).unwrap_err();
        assert!(
            err.contains(
                "joint Neck: the rest in head differs from the assembled rest (taken from body) by 3.000e-3"
            ),
            "{err}"
        );
        assert!(
            err.contains("1 joint rest disagreement(s) beyond tolerance 0.001"),
            "{err}"
        );
        assert_eq!(w.iter::<Asset>().count(), 2, "a refusal changes nothing");
        let (mut w, ..) = world(None, [0.0, 0.503, 0.0]);
        let report = run(
            &mut w,
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.01,unweighted-rests=refuse,name=hero}",
        )
        .unwrap();
        assert_eq!(
            report.steps[0].log.approximations["joint Neck: the rest in head differs from the assembled rest (taken from body) by 3.000e-3 (the part does not weight the joint)"],
            1
        );
    }

    #[test]
    fn parents_that_differ_are_refused() {
        let mut w = World::new();
        part(&mut w, "body", &BODY, &[]);
        let mut other = head_joints(Some([0.0, 1.0, 0.0]), [0.0, 0.5, 0.0]);
        other[2].1 = Some(0);
        part(&mut w, "head", &other, &[]);
        let err = run(&mut w, OWN).unwrap_err();
        assert!(
            err.contains("joint Neck: parents differ (Spine in body, Master in head)"),
            "{err}"
        );
    }

    /// Rebinding moves the part onto the assembled skeleton with the inverse of the assembled rest: at that rest every
    /// vertex is where the part stores it. A weighted joint whose chain is detached (so its model rest in the part is
    /// not the assembled one) is refused instead.
    #[test]
    fn rebinding_keeps_vertices_at_the_assembled_rest_and_refuses_what_would_move_a_part() {
        let rebind = "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=refuse,name=hero}";
        let (mut w, body, head) = world(Some([0.0, 1.0, 0.0]), [0.0, 0.5, 0.0]);
        let stored: Vec<Vec<[f32; 3]>> = [body, head]
            .iter()
            .map(
                |p| match &w.get::<Mesh>(*p).unwrap().primitives()[0].streams[&Semantic::Position] {
                    Stream::F32x3(v) => v.clone(),
                    _ => unreachable!(),
                },
            )
            .collect();
        let report = run(&mut w, rebind).unwrap();
        let (rig, s) = assembled(&w);
        let rests = model_transforms(&s, &locals(&s, None)).unwrap();
        for (p, was) in [body, head].iter().zip(&stored) {
            let sk = w.get::<Skin>(*p).unwrap();
            assert_eq!(sk.bindings[0].skeleton, rig);
            let mesh = w.get::<Mesh>(*p).unwrap();
            let prim = &mesh.primitives()[0];
            let posed = skin(
                &Vertices::of(prim).unwrap(),
                prim,
                sk.influences[0].unwrap(),
                sk.weights,
                Deformation::Linear,
                sk.bindings[0].palettes[0].as_ref().unwrap(),
                &rests,
            )
            .unwrap();
            for (a, b) in posed.positions.iter().zip(was) {
                assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1e-6), "{a:?} vs {b:?}");
            }
        }
        assert_eq!(w.verify_components(), vec![]);
        let asset = w.iter::<Asset>().next().unwrap().1;
        assert!(!asset.members.iter().any(|m| m.0 != rig && m.1 == Role::Rig));
        assert_eq!(report.steps[0].log.counts["assembled joints"], 5);

        // a detached chain: the head's Head is at 9, 9.7, 9 in its part and at 1.7 in the assembled skeleton
        let (mut w, ..) = world(None, [0.0, 0.5, 0.0]);
        let err = run(&mut w, rebind).unwrap_err();
        assert!(
            err.contains(
                "joint Head carries weight in head and its rest there differs from the assembled rest by"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_table_corresponds_the_joints_it_states_and_no_others() {
        let mut w = World::new();
        part(&mut w, "body", &BODY, &[]);
        part(
            &mut w,
            "head",
            &[
                ("Master", None, [0.0; 3]),
                ("Spine", Some(0), [0.0, 1.0, 0.0]),
                ("Neck_h", Some(1), [0.0, 0.5, 0.0]),
                ("L_Arm", Some(1), [0.3, 0.0, 0.0]),
            ],
            &[],
        );
        let rigs: Vec<Entity> = w.iter::<Asset>().map(|(_, a)| a.members[0].0).collect();
        // no table, no correspondence
        let table = "assemble-character{correspondence=table,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,name=hero}";
        assert!(
            run(&mut w, table)
                .unwrap_err()
                .contains("holds no core.joint_map")
        );
        // the head's Master, Spine and Neck_h are the body's Master, Spine and Neck; its L_Arm is not named
        let map = w.spawn();
        w.insert(
            map,
            JointMap {
                from: rigs[1],
                to: rigs[0],
                pairs: vec![
                    (JointId(0), JointId(0)),
                    (JointId(1), JointId(1)),
                    (JointId(2), JointId(2)),
                ],
            },
        );
        let report = run(&mut w, table).unwrap();
        let (_, s) = assembled(&w);
        // the body's four and the head part's L_Arm, which only an equal name would relate to the body's
        assert_eq!(s.joints.len(), 5);
        assert_eq!(s.joints[2].name, "Neck");
        assert_eq!(
            (s.joints[4].name.as_str(), s.joints[4].parent),
            ("L_Arm", Some(JointId(1)))
        );
        assert_eq!(
            report.steps[0].log.counts["assembled joints named unlike a joint that corresponds to them"],
            1
        );
    }

    #[test]
    fn a_table_that_gives_a_joint_two_parents_is_refused() {
        let mut w = World::new();
        part(&mut w, "body", &BODY, &[]);
        part(
            &mut w,
            "head",
            &[("Root", None, [0.0; 3]), ("Spine", Some(0), [0.0, 1.0, 0.0])],
            &[],
        );
        let rigs: Vec<Entity> = w.iter::<Asset>().map(|(_, a)| a.members[0].0).collect();
        let map = w.spawn();
        w.insert(
            map,
            JointMap {
                from: rigs[1],
                to: rigs[0],
                pairs: vec![(JointId(1), JointId(1))],
            },
        );
        let err = run(
            &mut w,
            "assemble-character{correspondence=table,reference-pose=own,tolerance=1,unweighted-rests=refuse,name=hero}",
        )
        .unwrap_err();
        assert!(
            err.contains("joint Spine: parents differ (Master in body, Root in head)"),
            "{err}"
        );
    }

    #[test]
    fn a_name_repeated_in_a_part_is_refused_where_another_part_holds_it() {
        let mut w = World::new();
        part(&mut w, "body", &BODY, &[]);
        part(
            &mut w,
            "face",
            &[
                ("Master", None, [0.0; 3]),
                ("Ctrl", Some(0), [0.0; 3]),
                ("Ctrl", Some(0), [1.0, 0.0, 0.0]),
            ],
            &[],
        );
        // held by no other part: the two stay distinct joints
        run(&mut w, OWN).unwrap();
        assert_eq!(assembled(&w).1.joints.len(), 4 + 2);
        let mut w = World::new();
        part(
            &mut w,
            "body",
            &[("Master", None, [0.0; 3]), ("Ctrl", Some(0), [0.0; 3])],
            &[],
        );
        part(
            &mut w,
            "face",
            &[
                ("Master", None, [0.0; 3]),
                ("Ctrl", Some(0), [0.0; 3]),
                ("Ctrl", Some(0), [1.0, 0.0, 0.0]),
            ],
            &[],
        );
        let err = run(&mut w, OWN).unwrap_err();
        assert!(
            err.contains("joint name Ctrl appears twice in face and is held by another part"),
            "{err}"
        );
    }

    #[test]
    fn every_option_is_required_and_checked() {
        let (mut w, ..) = world(None, [0.0, 0.5, 0.0]);
        for (pipeline, said) in [
            ("assemble-character", "correspondence=by-name|table is required"),
            (
                "assemble-character{correspondence=by-name}",
                "reference-pose=own|assembled is required",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own}",
                "tolerance=<largest rest disagreement accepted> is required",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0}",
                "unweighted-rests=report|refuse is required",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=report}",
                "name=<the assembled character's name> is required",
            ),
            (
                "assemble-character{correspondence=by-nam,reference-pose=own,tolerance=0,unweighted-rests=report,name=a}",
                "correspondence=by-nam: by-name or table",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=-1,unweighted-rests=report,name=a}",
                "a finite number, zero or more",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=some,name=a}",
                "unweighted-rests=some: report or refuse",
            ),
        ] {
            let err = run(&mut w, pipeline).unwrap_err();
            assert!(err.contains(said), "{pipeline}: {err}");
        }
    }

    /// A joint the part weights is refused whatever `unweighted-rests` says; one it does not is reported or refused as
    /// that says; and the rest is taken from a part that weights the joint.
    #[test]
    fn only_rests_a_skin_depends_on_are_refused_unless_all_are() {
        // the head part (first) stores its Neck 3 mm high and weights nothing on it; the body weights the Neck
        let mut w = World::new();
        part(
            &mut w,
            "head",
            &head_joints(Some([0.0, 1.0, 0.0]), [0.0, 0.503, 0.0]),
            &[],
        );
        part(&mut w, "body", &BODY, &[([0.0, 1.6, 0.0], 2)]);
        let report = run(
            &mut w,
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,name=hero}",
        )
        .unwrap();
        // the body's rest wins although the head part comes first: it is the part whose skin weights the joint
        assert_eq!(
            assembled(&w)
                .1
                .joints
                .iter()
                .find(|j| j.name == "Neck")
                .unwrap()
                .rest,
            at([0.0, 0.5, 0.0])
        );
        assert_eq!(
            report.steps[0].log.approximations["joint Neck: the rest in head differs from the assembled rest (taken from body) by 3.000e-3 (the part does not weight the joint)"],
            1
        );
        let mut w = World::new();
        part(
            &mut w,
            "head",
            &head_joints(Some([0.0, 1.0, 0.0]), [0.0, 0.503, 0.0]),
            &[],
        );
        part(&mut w, "body", &BODY, &[([0.0, 1.6, 0.0], 2)]);
        let err = run(&mut w, OWN).unwrap_err();
        assert!(err.contains("joint Neck: the rest in head differs"), "{err}");
        // now the head weights its Neck: refused with `report` too
        let mut w = World::new();
        part(
            &mut w,
            "head",
            &head_joints(Some([0.0, 1.0, 0.0]), [0.0, 0.503, 0.0]),
            &[([0.0, 1.5, 0.0], 2)],
        );
        part(&mut w, "body", &BODY, &[([0.0, 1.6, 0.0], 2)]);
        let err = run(
            &mut w,
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,name=hero}",
        )
        .unwrap_err();
        assert!(err.contains("joint Neck: the rest in "), "{err}");
    }
}
