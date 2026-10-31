//! `assemble-character`: the assets of a world that each hold a rig (a character's parts, each read with the skeleton
//! and bindings its source stores for it) related into one skeleton for animation, by a stated rule.
//!
//! A part is an asset that holds a rig; an entity is a member of one part only (a member listed by two parts is
//! refused, not merged).
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
//!   no pair names stay distinct even where their names are equal. A map with one end on a part's skeleton and the other
//!   outside the parts is refused; maps between skeletons that are no part's are left alone.
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
//! - `weighted-rests=report`: a part whose skin weights the joint is not refused either. The game poses a part by the
//!   character's pose through the part's OWN inverse binds, whatever the part's rest is, so the disagreement costs
//!   exactly what that rule costs: the displacement of the part's vertices from where it stores them (measured on the
//!   Wolf's face: its Head and the face chain under it stand 4.7e-3 from the other parts' and from the player's Havok
//!   skeleton, a 0.27 degree turn, which displaces the face's vertices by at most 1.03 mm). Each such joint is reported
//!   with its amount and counted, and the part's vertex displacement is reported (see below). `weighted-rests=refuse`
//!   refuses it beyond `tolerance=`, as every disagreement was.
//!
//! Each refused joint is named with its disagreement, every one in the one refusal; every disagreement above 1e-6
//! that is not refused is reported as an approximation, with its amount.
//!
//! **Source hierarchy reference.** A part rig may hold `core.assembly_reference`, naming a skeleton definition that
//! supplies the parents of corresponding joints. Part-only joints retain their parents; without a reference,
//! conflicting parents are still refused. All participating references must name the same skeleton. A changed
//! parent requires the part's model-space rest to agree with the reference within `reference-tolerance=` (default
//! 0.0001), including a weighted root. The named exception is an unweighted detached root: its placeholder rest is
//! ignored and the reference's is used, counted separately. Same-parent joints keep the existing rest and binding
//! policy, with their differences from the reference reported as a count and maximum matrix-entry difference.
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
//!   weighted joint that differs by more than `tolerance=` is refused (`weighted-rests=refuse`), and one that differs
//!   at all is reported with the amount its inverse bind moved. With `weighted-rests=report` such a joint's slots keep
//!   the part's own inverse bind (only the joint they name changes), which is what the game draws: the part's vertices
//!   then sit off their stored rest by the disagreement, counted and reported with the largest distance. The parts'
//!   skeletons stay in the world, as the maps' sources.
//!
//! The result is one asset named `name=` (`core.source` `assembled/<name>`) holding the assembled skeleton as its rig
//! (and, with `reference-pose=own`, each part's skeleton too: its skins bind to them) and every member of the parts'
//! assets other than their rigs; the parts' own asset roots are removed.
//!
//! **Sockets.** The parts' sockets (`core.sockets`: a FromSoftware model's dummy polygons) are the character's: the
//! assembled skeleton holds each part's sockets, each on the assembled joint its own joint is, at the offset it has in
//! that joint, so what names a socket of the character (an effect a clip starts) finds it on the skeleton a clip poses.
//! A name two parts (or one part twice) give sockets is one socket: where they stand alike (the same joint, the offsets
//! within `tolerance=`) that is counted; where they do not, the first part's (in part order) is kept and the others are
//! reported by part with how many, never dropped silently.
//!
//! **Held objects** (`leave-out=`, optional): an asset whose `core.source` is one given, or under one
//! (`parts/wp_a_0300_l` takes `parts/wp_a_0300_l/wp_a_0300_1_l`), `+` between, is not a part: an object the character
//! holds (a weapon) keeps its own rig, and what attaches it to the character (`core.attachment`) is the program's. Each
//! one left out is counted.
//!
//! The six original options are required; `leave-out=` and `reference-tolerance=` are optional. Approximations, reported: joints whose rests differ within the
//! tolerance, the vertex displacement measure of `own`, the inverse binds `assembled` moves, and sockets of one name two
//! parts place differently.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    AssemblyReference, Asset, Component, Entity, Joint, JointId, JointMap, Mesh, Name, Role, Skeleton, Skin,
    Slot, Socket, SocketId, Sockets, Source, World,
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

/// What the pass does with a rest disagreement of a joint the part weights, beyond the tolerance (`weighted-rests=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightedRests {
    /// Refused.
    Refuse,
    /// Reported with its amount and counted, the part staying bound to its own rest and inverse binds
    /// (`reference-pose=own`; with `assembled` it is refused all the same).
    Report,
}

/// `assemble-character{correspondence=by-name|table, reference-pose=own|assembled, tolerance=<number>,
/// unweighted-rests=report|refuse, weighted-rests=refuse|report, name=<word>[, leave-out=<source>+...]}` (see the module
/// documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct AssembleCharacter {
    pub correspondence: Correspondence,
    pub reference_pose: ReferencePose,
    /// The largest rest disagreement accepted.
    pub tolerance: f64,
    /// Model-space rest tolerance for a parent replaced by the source's reference hierarchy.
    pub reference_tolerance: f64,
    pub unweighted_rests: UnweightedRests,
    pub weighted_rests: WeightedRests,
    /// The assembled asset's name.
    pub name: String,
    /// The `core.source` of each asset that is a held object, not a part (`leave-out=`).
    pub leave_out: Vec<String>,
}

impl Choice for Correspondence {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("by-name", Correspondence::ByName),
        ("table", Correspondence::Table),
    ];
}

impl Choice for ReferencePose {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("own", ReferencePose::Own),
        ("assembled", ReferencePose::Assembled),
    ];
}

impl Choice for UnweightedRests {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("report", UnweightedRests::Report),
        ("refuse", UnweightedRests::Refuse),
    ];
}

impl Choice for WeightedRests {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("refuse", WeightedRests::Refuse),
        ("report", WeightedRests::Report),
    ];
}

/// `assemble-character`'s declaration ([`primase_core::pass::PassRegistry::add`]): six required options and two optional.
pub const ASSEMBLE_CHARACTER: PassSpec = PassSpec {
    name: "assemble-character",
    options: &[
        OptionSpec::choice::<Correspondence>(
            "correspondence",
            "how the parts' joints pair up: equal names, or the world's core.joint_map tables",
        ),
        OptionSpec::choice::<ReferencePose>(
            "reference-pose",
            "whether each part stays bound to its own skeleton and rest, or is rebound to the assembled skeleton",
        ),
        OptionSpec::required(
            "tolerance",
            "0.001",
            "the largest rest disagreement accepted between paired joints, finite and not negative",
        ),
        OptionSpec::optional(
            "reference-tolerance",
            "0.0001",
            "largest model-space rest difference when a source reference changes a joint's parent",
            "0.0001; an unweighted detached root uses the reference rest without comparison",
        ),
        OptionSpec::choice::<UnweightedRests>(
            "unweighted-rests",
            "a disagreeing rest of a joint no vertex weights: reported, or refused beyond the tolerance",
        ),
        OptionSpec::choice::<WeightedRests>(
            "weighted-rests",
            "a disagreeing rest of a weighted joint: refused, or reported with the part kept on its own rest",
        ),
        OptionSpec::required("name", "character", "the assembled character's name, not empty"),
        OptionSpec::optional(
            "leave-out",
            "parts/wp_a_0300_l",
            "the core.source of each asset that is an object the character holds, not a part of it (with the assets \
             under it), `+` between",
            "every asset that holds a rig is a part",
        ),
    ],
    make: assemble_character,
};

fn assemble_character(g: &Given) -> Result<Box<dyn Pass>, String> {
    let v = g.required("tolerance");
    let tolerance: f64 = v.parse().map_err(|_| format!("tolerance={v} is not a number"))?;
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err(format!("tolerance={v}: a finite number, zero or more"));
    }
    let v = g.optional("reference-tolerance").unwrap_or("0.0001");
    let reference_tolerance: f64 = v
        .parse()
        .map_err(|_| format!("reference-tolerance={v} is not a number"))?;
    if !reference_tolerance.is_finite() || reference_tolerance < 0.0 {
        return Err(format!("reference-tolerance={v}: a finite number, zero or more"));
    }
    let name = g.required("name");
    if name.is_empty() {
        return Err("name= is empty".into());
    }
    Ok(Box::new(AssembleCharacter {
        correspondence: g.choice::<Correspondence>("correspondence"),
        reference_pose: g.choice::<ReferencePose>("reference-pose"),
        tolerance,
        reference_tolerance,
        unweighted_rests: g.choice::<UnweightedRests>("unweighted-rests"),
        weighted_rests: g.choice::<WeightedRests>("weighted-rests"),
        name: name.to_owned(),
        leave_out: match g.optional("leave-out") {
            Some(list) => {
                let sources: Vec<String> = list.split('+').map(str::to_owned).collect();
                if sources.iter().any(String::is_empty) {
                    return Err(format!("leave-out={list}: an empty source"));
                }
                sources
            }
            None => Vec::new(),
        },
    }))
}

/// Whether `source` is one of `held` or under one (`parts/x` holds `parts/x/y`).
fn is_held(source: &str, held: &[String]) -> bool {
    held.iter().any(|h| {
        source == h
            || source
                .strip_prefix(h.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// One asset that holds a rig.
struct Part {
    /// None for the non-drawable source hierarchy reference.
    root: Option<Entity>,
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

/// The assets that hold a rig and are parts (not held objects, [`is_held`]), and how many held objects were left out.
fn gather(w: &World, held: &[String]) -> Result<(Vec<Part>, u64), String> {
    let mut parts = Vec::new();
    let mut left_out = 0u64;
    // the part asset that holds each entity: one entity cannot belong to two parts (it would be held twice by the
    // assembled asset, and its skin rebound once per holder)
    let mut held_by: BTreeMap<Entity, String> = BTreeMap::new();
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
        if w.get::<Source>(root).is_some_and(|s| is_held(&s.0, held)) {
            left_out += 1;
            continue;
        }
        for (member, _) in &asset.members {
            if let Some(other) = held_by.insert(*member, label.clone()) {
                return Err(format!(
                    "{member:?} is a member of both asset {other} and asset {label}: a part's members are its own                      (merging them is not done)"
                ));
            }
        }
        let skeleton = w
            .get::<Skeleton>(rig)
            .ok_or_else(|| format!("asset {label}: its rig holds no core.skeleton"))?
            .clone();
        if let Some(j) = skeleton
            .joints
            .iter()
            .find(|j| j.parent.is_some_and(|p| skeleton.index_of(p).is_none()))
        {
            return Err(format!(
                "asset {label}: joint {} has a parent outside its skeleton",
                j.name
            ));
        }
        // a NaN rest would read as no difference anywhere (`f64::max` skips it): refused where it is read
        if let Some(j) = skeleton
            .joints
            .iter()
            .find(|j| j.rest.matrix().cols.iter().flatten().any(|x| !x.is_finite()))
        {
            return Err(format!(
                "asset {label}: joint {} has a rest that is not finite",
                j.name
            ));
        }
        let rests =
            model_transforms(&skeleton, &locals(&skeleton, None)).map_err(|e| format!("{label}: {}", e.0))?;
        parts.push(Part {
            root: Some(root),
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
    Ok((parts, left_out))
}

/// The reference is a skeleton definition, not another drawable part. Adding it to correspondence preserves all
/// its parent chains, including ancestors a part does not carry, while leaving part-only joints in the union.
fn add_reference(parts: &mut Vec<Part>, w: &World) -> Result<Option<usize>, String> {
    let references: BTreeSet<_> = parts
        .iter()
        .filter_map(|p| w.get::<AssemblyReference>(p.rig).map(|r| r.skeleton))
        .collect();
    let reference = match references.len() {
        0 => return Ok(None),
        1 => *references.first().unwrap(),
        _ => return Err("parts state different assembly reference skeletons".into()),
    };
    if parts.iter().any(|p| p.rig == reference) {
        return Err("assembly reference is also a drawable part's rig".into());
    }
    let skeleton = w
        .get::<Skeleton>(reference)
        .ok_or("assembly reference holds no core.skeleton")?
        .clone();
    skeleton.verify(reference, w)?;
    let rests = model_transforms(&skeleton, &locals(&skeleton, None)).map_err(|e| e.0)?;
    let index = parts.len();
    parts.push(Part {
        root: None,
        label: w
            .get::<Name>(reference)
            .map_or_else(|| format!("reference {reference:?}"), |n| n.0.clone()),
        rig: reference,
        skeleton,
        rests,
        members: Vec::new(),
    });
    Ok(Some(index))
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
    for (e, m) in w.iter::<JointMap>() {
        let (Some(&a), Some(&b)) = (part_of.get(&m.from), part_of.get(&m.to)) else {
            // a map of two skeletons that are no part's is not the table's; one end on a part is
            if let Some(end) = [m.from, m.to].into_iter().find(|s| !part_of.contains_key(s))
                && part_of.contains_key(&m.from) != part_of.contains_key(&m.to)
            {
                return Err(format!(
                    "joint map {e:?} has one end on a part's skeleton and the other ({end:?}) outside the parts: \
                     its pairs would be lost"
                ));
            }
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
        reference: Option<usize>,
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
            let authority = m.iter().copied().find(|(p, _)| Some(*p) == reference);
            if classes.len() > 1 && authority.is_none() {
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
            parents[c] = match authority {
                Some((p, j)) => parts[p].skeleton.joints[j]
                    .parent
                    .and_then(|id| parts[p].skeleton.index_of(id))
                    .map(|up| of[p][up]),
                None => stated.first().map(|s| s.2),
            };
            // its rest from a part that states its parent (any part for a root), one that weights it first
            let candidates: Vec<(usize, usize)> = if stated.is_empty() {
                m.clone()
            } else {
                stated.iter().map(|s| (s.0, s.1)).collect()
            };
            let selected = candidates
                .iter()
                .copied()
                .find(|&(p, j)| weighted[p].contains(&parts[p].skeleton.joints[j].id))
                .unwrap_or(candidates[0]);
            let (p, j) = selected;
            let joint = &parts[p].skeleton.joints[j];
            let parent = joint
                .parent
                .and_then(|id| parts[p].skeleton.index_of(id))
                .map(|up| of[p][up]);
            // Same-parent joints keep the selected part's local rest. A changed parent uses the reference rest
            // only after the model-space guard in reference_decisions has established compatibility.
            let use_reference =
                parent != parents[c] || (joint.parent.is_none() && !weighted[p].contains(&joint.id));
            source.push(authority.filter(|_| use_reference).unwrap_or(selected));
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
    /// A reference may replace a parent only when it preserves the part's model-space rest. The one named
    /// exception is an unweighted detached root: it carries no geometry and its placeholder rest is ignored.
    fn reference_decisions(
        &self,
        parts: &[Part],
        of: &[Vec<usize>],
        weighted: &[BTreeSet<JointId>],
        reference: Option<usize>,
        log: &mut PassLog,
    ) -> Result<(), String> {
        let Some(r) = reference else { return Ok(()) };
        let authority = &parts[r];
        let reference_joints: BTreeMap<usize, usize> =
            of[r].iter().enumerate().map(|(j, &c)| (c, j)).collect();
        let (mut detached, mut reparented, mut same_parent) = (0u64, 0u64, 0u64);
        let mut max_same_parent = 0.0f64;
        let mut problems = Vec::new();
        for (p, part) in parts.iter().enumerate().filter(|(p, _)| *p != r) {
            for (j, joint) in part.skeleton.joints.iter().enumerate() {
                let Some(&reference_joint) = reference_joints.get(&of[p][j]) else {
                    continue;
                };
                let target = &authority.skeleton.joints[reference_joint];
                let parent = joint
                    .parent
                    .and_then(|id| part.skeleton.index_of(id))
                    .map(|up| of[p][up]);
                let target_parent = target
                    .parent
                    .and_then(|id| authority.skeleton.index_of(id))
                    .map(|up| of[r][up]);
                if joint.parent.is_none() && !weighted[p].contains(&joint.id) {
                    detached += 1;
                    continue;
                }
                let d = gap(&part.rests[&joint.id], &authority.rests[&target.id]);
                if parent != target_parent {
                    if d > self.reference_tolerance {
                        problems.push(format!(
                            "joint {} in {}: changing its parent to assembly reference {} changes its model-space rest by {d:.3e}, beyond reference-tolerance {}",
                            joint.name, part.label, authority.label, self.reference_tolerance
                        ));
                    } else {
                        reparented += 1;
                        log.approximate(&format!(
                            "joint {} in {}: parent taken from assembly reference {}, model-space rest difference {d:.3e}",
                            joint.name, part.label, authority.label
                        ), 1);
                    }
                } else if d > REPORT_FLOOR {
                    same_parent += 1;
                    max_same_parent = max_same_parent.max(d);
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        log.count("source assembly reference skeletons", 1);
        log.count(
            "part joints reparented to the reference within reference-tolerance",
            reparented,
        );
        log.count("unweighted detached roots: rest from reference", detached);
        log.count(
            "same-parent part joints with rests differing from the reference",
            same_parent,
        );
        if same_parent > 0 {
            log.approximate(&format!(
                "same-parent rests differ from assembly reference by up to {max_same_parent:.3e}; part rests and inverse binds follow the existing reference-pose policy"
            ), same_parent);
        }
        Ok(())
    }

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
                if part.root.is_none() {
                    continue;
                }
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

    /// The parts' sockets on the assembled skeleton (see the module documentation): each on the assembled joint its own
    /// joint is, its offset as stored; one a name, the first part's where two place a name otherwise (reported by part).
    fn carry_sockets(
        &self,
        w: &World,
        parts: &[Part],
        of: &[Vec<usize>],
        log: &mut PassLog,
    ) -> Result<Vec<Socket>, String> {
        let mut sockets: Vec<Socket> = Vec::new();
        // each name's socket (an index into `sockets`) and the part it was taken from
        let mut named: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut alike = 0u64;
        // (the part kept, the part that places the name otherwise): the names
        let mut otherwise: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
        for (p, part) in parts.iter().enumerate() {
            let Some(own) = w.get::<Sockets>(part.rig) else {
                continue;
            };
            for s in &own.0 {
                let j = part.skeleton.index_of(s.joint).ok_or_else(|| {
                    format!(
                        "{}: socket {} is on a joint its skeleton does not hold",
                        part.label, s.name
                    )
                })?;
                let joint = JointId(of[p][j] as u32);
                match named.get(&s.name) {
                    Some(&(k, first)) => {
                        let kept = &sockets[k];
                        if kept.joint == joint
                            && gap(&affine(&kept.offset), &affine(&s.offset)) <= self.tolerance
                        {
                            alike += 1;
                        } else {
                            otherwise.entry((first, p)).or_default().push(s.name.clone());
                        }
                    }
                    None => {
                        named.insert(s.name.clone(), (sockets.len(), p));
                        sockets.push(Socket {
                            id: SocketId(sockets.len() as u32),
                            name: s.name.clone(),
                            joint,
                            offset: s.offset,
                        });
                    }
                }
            }
        }
        for ((first, other), names) in otherwise {
            const SHOWN: usize = 5;
            let more = names.len().saturating_sub(SHOWN);
            log.approximate(
                &format!(
                    "sockets of {} given the name of a socket of {} and placed otherwise, {}'s kept: {}{}",
                    parts[other].label,
                    parts[first].label,
                    parts[first].label,
                    names[..names.len().min(SHOWN)].join(", "),
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                ),
                names.len() as u64,
            );
        }
        log.count("sockets on the assembled skeleton", sockets.len() as u64);
        log.count("sockets two parts give one name and place alike", alike);
        Ok(sockets)
    }
}

impl Pass for AssembleCharacter {
    fn name(&self) -> &'static str {
        "assemble-character"
    }
    fn options(&self) -> Options {
        let mut options = Options::from([
            ("correspondence".to_owned(), self.correspondence.text().to_owned()),
            ("reference-pose".to_owned(), self.reference_pose.text().to_owned()),
            ("tolerance".to_owned(), self.tolerance.to_string()),
            (
                "reference-tolerance".to_owned(),
                self.reference_tolerance.to_string(),
            ),
            (
                "unweighted-rests".to_owned(),
                self.unweighted_rests.text().to_owned(),
            ),
            ("weighted-rests".to_owned(), self.weighted_rests.text().to_owned()),
            ("name".to_owned(), self.name.clone()),
        ]);
        if !self.leave_out.is_empty() {
            options.insert("leave-out".to_owned(), self.leave_out.join("+"));
        }
        options
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME, Skeleton::NAME, Skin::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Skeleton::NAME,
            Sockets::NAME,
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
        "parts whose rests for a joint differ within the tolerance related as one joint (with weighted-rests=report, beyond it too); with reference-pose=assembled, inverse binds moved to the assembled rest; of sockets two parts give one name and place otherwise, the first part's kept"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let (mut parts, left_out) = gather(w, &self.leave_out)?;
        let reference = add_reference(&mut parts, w)?;
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
        self.reference_decisions(&parts, &of, &carrying, reference, log)?;
        let asm = Assembled::resolve(&parts, &of, &carrying, reference)?;
        let skeleton = asm.skeleton(&parts);
        let rests = model_transforms(&skeleton, &locals(&skeleton, None)).map_err(|e| e.0)?;
        let id = |c: usize| JointId(c as u32);
        let mut problems = Vec::new();

        let (found, detached) = self.disagreements(&parts, &of, &asm, &rests)?;
        let mut within = Vec::new();
        let mut accepted_weighted = 0u64;
        let weighted_accepted = self.weighted_rests == WeightedRests::Report;
        for &(c, p, j, d) in &found {
            let weighted_here = carrying[p].contains(&parts[p].skeleton.joints[j].id);
            let line = format!(
                "joint {}: the rest in {} differs from the assembled rest (taken from {}) by {d:.3e}",
                asm.names[c], parts[p].label, parts[asm.source[c].0].label
            );
            let refused = d > self.tolerance
                && if weighted_here {
                    !weighted_accepted
                } else {
                    self.unweighted_rests == UnweightedRests::Refuse
                };
            if refused {
                problems.push(line);
            } else if d > REPORT_FLOOR {
                within.push(match (weighted_here, d > self.tolerance) {
                    (true, true) => {
                        accepted_weighted += 1;
                        format!(
                            "{line} (beyond tolerance {}; the part weights the joint and keeps its own inverse binds)",
                            self.tolerance
                        )
                    }
                    (true, false) => line,
                    (false, _) => format!("{line} (the part does not weight the joint)"),
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
                    if d > self.tolerance && self.weighted_rests == WeightedRests::Refuse {
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
        let sockets = self.carry_sockets(w, &parts, &of, log)?;
        if !sockets.is_empty() {
            w.insert(assembled, Sockets(sockets));
        }
        let mut asset = Asset {
            members: vec![(assembled, Role::Rig)],
        };
        for (p, part) in parts.iter().enumerate() {
            // The reference supplies a hierarchy, not an implicit clip correspondence. A caller may later build
            // that map with its own rest/parent policy; emitting one here would create two maps from that source.
            if part.root.is_none() {
                continue;
            }
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
                let mut kept_own = BTreeMap::<String, f64>::new();
                // the bindings that keep an inverse bind, as they were: (prototype, binding, palettes)
                let mut kept_in = Vec::new();
                for (proto, carrying) in &prototypes[p] {
                    let s = w.get_mut::<Skin>(*proto).ok_or("a skin is gone")?;
                    for (bi, b) in s
                        .bindings
                        .iter_mut()
                        .enumerate()
                        .filter(|(_, b)| b.skeleton == part.rig)
                    {
                        let before = b.palettes.clone();
                        let mut kept = false;
                        b.skeleton = assembled;
                        for slot in b.palettes.iter_mut().flatten().flatten() {
                            let j = part
                                .skeleton
                                .index_of(slot.joint)
                                .ok_or("a slot names a joint its skeleton does not hold")?;
                            let c = of[p][j];
                            // the game poses the part through its own inverse binds: a weighted joint whose rest
                            // disagrees beyond the tolerance keeps its own, when the disagreement is accepted
                            let rest_gap = gap(&part.rests[&slot.joint], &rests[&id(c)]);
                            if carrying.contains(&slot.joint)
                                && rest_gap > self.tolerance
                                && self.weighted_rests == WeightedRests::Report
                            {
                                let e = kept_own.entry(asm.names[c].clone()).or_default();
                                *e = e.max(rest_gap);
                                kept = true;
                                slot.joint = id(c);
                                continue;
                            }
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
                        if kept {
                            kept_in.push((*proto, bi, before));
                        }
                    }
                }
                // what keeping them costs: the part's vertices posed by the assembled rest through the palettes now,
                // against the part's own rest through the palettes as stored
                let (mut most, mut displaced) = (0.0f64, 0u64);
                for (proto, bi, before) in &kept_in {
                    let (Some(s), Some(mesh)) = (w.get::<Skin>(*proto), w.get::<Mesh>(*proto)) else {
                        continue;
                    };
                    let b = &s.bindings[*bi];
                    for (i, prim) in mesh.primitives().iter().enumerate() {
                        let (Some(Some(how)), Some(Some(now)), Some(Some(was))) =
                            (s.influences.get(i), b.palettes.get(i), before.get(i))
                        else {
                            continue;
                        };
                        let v = Vertices::of(prim).map_err(|e| e.0)?;
                        let at = |palette: &Vec<Slot>, pose: &BTreeMap<JointId, Affine>| {
                            skin(&v, prim, *how, s.weights, b.deformation, palette, pose).map_err(|e| e.0)
                        };
                        let (stored, drawn) = (at(was, &part.rests)?, at(now, &rests)?);
                        for (a, b) in stored.positions.iter().zip(&drawn.positions) {
                            let d = (0..3).map(|k| f64::from(a[k] - b[k]).powi(2)).sum::<f64>().sqrt();
                            most = most.max(d);
                            displaced += u64::from(d > 1e-5);
                        }
                    }
                }
                for (joint, d) in kept_own {
                    log.approximate(
                        &format!(
                            "joint {joint}: the inverse binds of {} kept as the part's own, its rest differs from the assembled by up to {d:.3e}",
                            part.label
                        ),
                        1,
                    );
                }
                if displaced > 0 {
                    log.approximate(
                        &format!(
                            "vertices of {} sit up to {most:.3e} m from their stored rest where the part's own inverse binds are kept",
                            part.label
                        ),
                        displaced,
                    );
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
            if let Some(root) = part.root {
                w.despawn(root);
            }
        }
        for line in within {
            log.approximate(&line, 1);
        }
        if accepted_weighted > 0 {
            log.count(
                "weighted joint rests beyond the tolerance, accepted by weighted-rests=report",
                accepted_weighted,
            );
        }
        log.count(
            "parts assembled",
            parts.iter().filter(|p| p.root.is_some()).count() as u64,
        );
        log.count("held objects left out (leave-out=)", left_out);
        log.count("assembled joints", asm.names.len() as u64);
        log.count(
            "assembled joints held by more than one part",
            asm.members
                .iter()
                .filter(|m| m.iter().filter(|(p, _)| parts[*p].root.is_some()).count() > 1)
                .count() as u64,
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

    const OWN: &str = "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,weighted-rests=refuse,name=hero}";

    /// Two independently skinned parts agree in model space while one stores the shared joint under a different
    /// parent. The reference contains no PartOnly joint, so that joint must survive the union unchanged.
    fn reference_world() -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let reference_joints: &[J] = &[
            ("Root", None, [0.0; 3]),
            ("Parent", Some(0), [0.0, 1.0, 0.0]),
            ("Joint", Some(1), [0.0, 0.5, 0.0]),
        ];
        let a = part(
            &mut w,
            "flattened",
            &[
                reference_joints[0],
                reference_joints[1],
                ("Joint", Some(0), [0.0, 1.5, 0.0]),
                ("PartOnly", Some(2), [0.1, 0.0, 0.0]),
            ],
            &[([0.0, 1.5, 0.0], 2), ([0.1, 1.5, 0.0], 3)],
        );
        let b = part(&mut w, "hierarchical", reference_joints, &[([0.0, 1.5, 0.0], 2)]);
        let ra = w.get::<Skin>(a).unwrap().bindings[0].skeleton;
        let rb = w.get::<Skin>(b).unwrap().bindings[0].skeleton;
        let reference = w.spawn();
        w.insert(reference, Name("source hierarchy".into()));
        w.insert(reference, w.get::<Skeleton>(rb).unwrap().clone());
        for rig in [ra, rb] {
            w.insert(rig, AssemblyReference { skeleton: reference });
        }
        assert!(w.verify_components().is_empty());
        (w, a, ra, reference)
    }

    #[test]
    fn a_reference_hierarchy_resolves_compatible_parents_and_retains_part_only_joints() {
        let (mut w, _, _, reference) = reference_world();
        let report = run(&mut w, OWN).unwrap();
        let (_, skeleton) = assembled(&w);
        let joint = skeleton.joints.iter().find(|j| j.name == "Joint").unwrap();
        let parent = skeleton.joints.iter().find(|j| j.name == "Parent").unwrap();
        let extra = skeleton.joints.iter().find(|j| j.name == "PartOnly").unwrap();
        assert_eq!(joint.parent, Some(parent.id));
        assert_eq!(extra.parent, Some(joint.id));
        assert_eq!(report.steps[0].log.counts["parts assembled"], 2);
        assert_eq!(
            report.steps[0].log.counts["part joints reparented to the reference within reference-tolerance"],
            1
        );
        assert!(
            w.has::<Skeleton>(reference),
            "the reference stays alive for the parts' references"
        );
        assert!(
            w.iter::<JointMap>().all(|(_, map)| map.from != reference),
            "clip correspondence is a separate caller choice"
        );
        assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
    }

    #[test]
    fn a_reference_reparenting_past_the_model_space_tolerance_is_refused() {
        let (mut w, _, rig, _) = reference_world();
        w.get_mut::<Skeleton>(rig).unwrap().joints[2].rest = at([0.0, 1.51, 0.0]);
        let error = run(
            &mut w,
            &OWN.replace("weighted-rests=refuse", "weighted-rests=report"),
        )
        .unwrap_err();
        assert!(
            error.contains("joint Joint in flattened") && error.contains("reference-tolerance"),
            "{error}"
        );
        assert_eq!(
            w.iter::<Asset>().count(),
            2,
            "a refusal must not change the world"
        );
    }

    #[test]
    fn a_weighted_detached_root_past_the_reference_tolerance_is_refused() {
        let (mut w, _, rig, _) = reference_world();
        let joint = &mut w.get_mut::<Skeleton>(rig).unwrap().joints[2];
        joint.parent = None;
        joint.rest = at([0.0, 9.0, 0.0]);
        let error = run(
            &mut w,
            &OWN.replace("weighted-rests=refuse", "weighted-rests=report"),
        )
        .unwrap_err();
        assert!(
            error.contains("joint Joint in flattened") && error.contains("reference-tolerance"),
            "{error}"
        );
    }

    #[test]
    fn same_parent_rest_differences_are_reported_under_the_existing_policy() {
        let (mut w, _, rig, _) = reference_world();
        let joint = &mut w.get_mut::<Skeleton>(rig).unwrap().joints[2];
        joint.parent = Some(JointId(1));
        joint.rest = at([0.0, 0.52, 0.0]);
        let report = run(
            &mut w,
            &OWN.replace("weighted-rests=refuse", "weighted-rests=report"),
        )
        .unwrap();
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["same-parent part joints with rests differing from the reference"],
            1
        );
        assert!(
            log.approximations
                .keys()
                .any(|line| line.contains("same-parent rests") && line.contains("2.000e-2")),
            "{:?}",
            log.approximations
        );
        let (_, skeleton) = assembled(&w);
        let joint = skeleton.joints.iter().find(|j| j.name == "Joint").unwrap();
        assert_eq!(joint.rest.matrix().cols[3], [0.0, 0.52, 0.0]);
    }

    #[test]
    fn an_unweighted_detached_root_uses_the_reference_rest_and_is_counted() {
        let (mut w, proto, rig, _) = reference_world();
        let joint = &mut w.get_mut::<Skeleton>(rig).unwrap().joints[2];
        joint.parent = None;
        joint.rest = at([0.0, 9.0, 0.0]);
        w.get_mut::<Mesh>(proto)
            .unwrap()
            .primitive_mut(0)
            .unwrap()
            .streams_mut()
            .insert(Semantic::Joints(0), Stream::U8x4(vec![[1, 0, 0, 0]; 2]));
        let report = run(
            &mut w,
            &OWN.replace("unweighted-rests=refuse", "unweighted-rests=report"),
        )
        .unwrap();
        assert!(report.steps[0].log.counts["unweighted detached roots: rest from reference"] >= 1);
        let (_, skeleton) = assembled(&w);
        let joint = skeleton.joints.iter().find(|j| j.name == "Joint").unwrap();
        let model = model_transforms(&skeleton, &locals(&skeleton, None)).unwrap();
        assert_eq!(model[&joint.id][3], [0.0, 1.5, 0.0]);
    }

    /// The pass reads the rests itself: a NaN in a part's rest would make every difference involving it compare as no
    /// difference (`f64::max` skips a NaN), so it is refused naming the joint and the part, before any comparison.
    #[test]
    fn a_rest_that_is_not_finite_is_refused() {
        let (mut w, body, _) = world(None, [0.0, 0.5, 0.0]);
        let rig = w.get::<Skin>(body).unwrap().bindings[0].skeleton;
        w.get_mut::<Skeleton>(rig).unwrap().joints[2].rest = at([f32::NAN, 0.5, 0.0]);
        let options: Options = [
            ("correspondence", "by-name"),
            ("reference-pose", "own"),
            ("tolerance", "0.001"),
            ("unweighted-rests", "refuse"),
            ("weighted-rests", "refuse"),
            ("name", "hero"),
        ]
        .iter()
        .map(|&(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let err = ASSEMBLE_CHARACTER
            .instance(&options)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(
            err.contains("joint Neck") && err.contains("body") && err.contains("not finite"),
            "{err}"
        );
    }

    /// A table whose joint map has one end on a part's skeleton and the other outside the parts is refused naming the
    /// map (it would be skipped, and the correspondence it states lost); a map between two outside skeletons is not
    /// the table's business.
    #[test]
    fn a_table_map_with_an_end_outside_the_parts_is_refused() {
        let (mut w, body, head) = world(None, [0.0, 0.5, 0.0]);
        let body_rig = w.get::<Skin>(body).unwrap().bindings[0].skeleton;
        let head_rig = w.get::<Skin>(head).unwrap().bindings[0].skeleton;
        let outside = w.spawn();
        w.insert(
            outside,
            Skeleton {
                joints: vec![Joint {
                    id: JointId(0),
                    name: "Master".into(),
                    parent: None,
                    rest: at([0.0; 3]),
                }],
            },
        );
        let between = |w: &mut World, a, b, pairs: Vec<(JointId, JointId)>| {
            let m = w.spawn();
            w.insert(
                m,
                JointMap {
                    from: a,
                    to: b,
                    pairs,
                },
            );
            m
        };
        between(&mut w, outside, outside, vec![]);
        let table = between(&mut w, body_rig, head_rig, vec![(JointId(0), JointId(0))]);
        let stray = between(&mut w, head_rig, outside, vec![(JointId(0), JointId(0))]);
        let err = run(
            &mut w,
            "assemble-character{correspondence=table,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,weighted-rests=refuse,name=hero}",
        )
        .unwrap_err();
        assert!(
            err.contains(&format!("{stray:?}")) && err.contains("outside the parts"),
            "{err}"
        );
        assert!(!err.contains(&format!("{table:?}")), "{err}");
    }

    /// A mesh listed by two part assets would be held twice by the assembled asset and rebound once per part: refused,
    /// naming the entity and both assets.
    #[test]
    fn a_member_shared_by_two_parts_is_refused() {
        let (mut w, body, _) = world(None, [0.0, 0.5, 0.0]);
        let head_root = w
            .iter::<Asset>()
            .find(|(e, _)| w.get::<Name>(*e).is_some_and(|n| n.0 == "head"))
            .map(|(e, _)| e)
            .unwrap();
        w.get_mut::<Asset>(head_root)
            .unwrap()
            .members
            .push((body, Role::Part));
        let err = run(&mut w, OWN).unwrap_err();
        assert!(
            err.contains(&format!("{body:?}")) && err.contains("asset body") && err.contains("asset head"),
            "{err}"
        );
    }

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
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.01,unweighted-rests=refuse,weighted-rests=refuse,name=hero}",
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
        let rebind = "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=refuse,weighted-rests=refuse,name=hero}";
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
        let table = "assemble-character{correspondence=table,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,weighted-rests=refuse,name=hero}";
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
            "assemble-character{correspondence=table,reference-pose=own,tolerance=1,unweighted-rests=refuse,weighted-rests=refuse,name=hero}",
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
            ("assemble-character", "correspondence is required: "),
            (
                "assemble-character{correspondence=by-name}",
                "reference-pose is required: ",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own}",
                "tolerance is required: ",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0}",
                "unweighted-rests is required: ",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=report}",
                "weighted-rests is required: ",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=report,weighted-rests=refuse}",
                "name is required: ",
            ),
            (
                "assemble-character{correspondence=by-nam,reference-pose=own,tolerance=0,unweighted-rests=report,weighted-rests=refuse,name=a}",
                "correspondence=by-nam is none of by-name or table",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=-1,unweighted-rests=report,weighted-rests=refuse,name=a}",
                "a finite number, zero or more",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=some,weighted-rests=refuse,name=a}",
                "unweighted-rests=some is none of report or refuse",
            ),
            (
                "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0,unweighted-rests=report,weighted-rests=some,name=a}",
                "weighted-rests=some is none of refuse or report",
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
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,weighted-rests=refuse,name=hero}",
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
            "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=report,weighted-rests=refuse,name=hero}",
        )
        .unwrap_err();
        assert!(err.contains("joint Neck: the rest in "), "{err}");
    }

    /// `weighted-rests=report` accepts a weighted disagreement beyond the tolerance with `reference-pose=own`, names it
    /// with its amount and counts it, leaves every part bound as stored, and reports the part's vertex displacement;
    /// `assembled` and `refuse` still refuse it.
    #[test]
    fn weighted_rests_beyond_the_tolerance_are_reported_when_asked_and_the_part_stays_as_stored() {
        // both parts weight the Neck; the head part's sits 3 mm higher than the body's; the assembled rest is the first
        // weighting part's (the head's), so it is the body whose rest differs from it
        let build = || {
            let mut w = World::new();
            part(
                &mut w,
                "head",
                &head_joints(Some([0.0, 1.0, 0.0]), [0.0, 0.503, 0.0]),
                &[([0.0, 1.5, 0.0], 2)],
            );
            part(&mut w, "body", &BODY, &[([0.0, 1.6, 0.0], 2)]);
            w
        };
        let report_own = "assemble-character{correspondence=by-name,reference-pose=own,tolerance=0.001,unweighted-rests=refuse,weighted-rests=report,name=hero}";
        let mut w = build();
        let skins: Vec<(Entity, Skin)> = w.iter::<Skin>().map(|(e, s)| (e, s.clone())).collect();
        let report = run(&mut w, report_own).unwrap();
        let log = &report.steps[0].log;
        let line = "joint Neck: the rest in body differs from the assembled rest (taken from head) by 3.000e-3 (beyond tolerance 0.001; the part weights the joint and keeps its own inverse binds)";
        assert_eq!(log.approximations.get(line), Some(&1), "{:?}", log.approximations);
        assert_eq!(
            log.counts["weighted joint rests beyond the tolerance, accepted by weighted-rests=report"],
            1
        );
        for (e, s) in &skins {
            assert_eq!(w.get::<Skin>(*e), Some(s), "a part stays bound as stored");
        }
        assert!(
            log.approximations
                .keys()
                .any(|k| k.starts_with("vertices of body sit up to ")),
            "{:?}",
            log.approximations
        );
        assert_eq!(w.verify_components(), vec![]);
        // refuse refuses the same world
        let mut refused = build();
        let err = run(&mut refused, OWN).unwrap_err();
        assert!(err.contains("joint Neck: the rest in body differs"), "{err}");
        // rebinding to the assembled skeleton with weighted-rests=report keeps the body's own inverse bind for the Neck
        // it weights (the assembled rest is the head's, the first weighting part's) (the game poses a part through its own inverse binds) and rebinds what is within the tolerance
        let mut w = build();
        let body_proto = w
            .iter::<Skin>()
            .find(|(_, s)| {
                w.get::<Skeleton>(s.bindings[0].skeleton)
                    .is_some_and(|k| k.joints.len() == 4 && k.joints[3].name == "L_Arm")
            })
            .map(|(e, _)| e)
            .unwrap();
        let own_neck = w.get::<Skin>(body_proto).unwrap().bindings[0].palettes[0]
            .as_ref()
            .unwrap()[2]
            .inverse_bind;
        let report = run(
            &mut w,
            "assemble-character{correspondence=by-name,reference-pose=assembled,tolerance=0.001,unweighted-rests=refuse,weighted-rests=report,name=hero}",
        )
        .unwrap();
        let log = &report.steps[0].log;
        assert!(
            log.approximations.keys().any(|k| k.starts_with(
                "joint Neck: the inverse binds of body kept as the part's own, its rest differs from the assembled by up to 3.000e-3"
            )),
            "{:?}",
            log.approximations
        );
        assert!(
            log.approximations
                .keys()
                .any(|k| k.starts_with("vertices of body sit up to ")
                    && k.ends_with("where the part's own inverse binds are kept")),
            "{:?}",
            log.approximations
        );
        let (_, rig) = assembled(&w);
        let slot = &w.get::<Skin>(body_proto).unwrap().bindings[0].palettes[0]
            .as_ref()
            .unwrap()[2];
        assert_eq!(
            slot.inverse_bind, own_neck,
            "the weighted Neck keeps the body's own inverse bind"
        );
        assert_eq!(
            rig.joints[slot.joint.0 as usize].name, "Neck",
            "and names the assembled joint"
        );
        assert_eq!(w.verify_components(), vec![]);
        // the head, whose Neck is the assembled one, is rebound as before: nothing of it is reported kept
        assert!(
            !log.approximations.keys().any(|k| k.contains("of head kept")),
            "{:?}",
            log.approximations
        );
    }

    /// The skeleton entity of the part asset named `label`.
    fn skeleton_of(w: &World, label: &str) -> Entity {
        let (_, asset) = w
            .iter::<Asset>()
            .find(|(root, _)| w.get::<Name>(*root).is_some_and(|n| n.0 == label))
            .unwrap();
        asset.members.iter().find(|m| m.1 == Role::Rig).unwrap().0
    }

    fn socket(id: u32, name: &str, joint: u32, offset: [f32; 3]) -> Socket {
        Socket {
            id: SocketId(id),
            name: name.to_owned(),
            joint: JointId(joint),
            offset: primase_core::Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], offset],
            },
        }
    }

    /// The parts' sockets are on the assembled skeleton, each on the joint its own corresponds to, at its offset: the
    /// body's "dummy 20" on its L_Arm, the head's "dummy 85" on its Head. The head names a "dummy 20" too, on its Neck: on
    /// another joint than the body's, so the body's (first) is kept and the head's reported by name and counted; its
    /// "dummy 7" stands as the body's does (the Neck, the same offset), counted as alike.
    #[test]
    fn the_parts_sockets_are_carried_onto_the_assembled_skeleton_and_disagreements_reported() {
        let (mut w, ..) = world(None, [0.0, 0.5, 0.0]);
        let body = skeleton_of(&w, "body");
        let head = skeleton_of(&w, "head");
        w.insert(
            body,
            Sockets(vec![
                socket(0, "dummy 20", 3, [0.1, 0.0, 0.0]),
                socket(1, "dummy 7", 2, [0.0, 0.1, 0.0]),
            ]),
        );
        w.insert(
            head,
            Sockets(vec![
                socket(0, "dummy 85", 3, [0.0, 0.0, 0.2]),
                socket(1, "dummy 20", 2, [0.1, 0.0, 0.0]),
                socket(2, "dummy 7", 2, [0.0, 0.1, 0.0]),
            ]),
        );
        let report = run(&mut w, OWN).unwrap();
        let (rig, s) = assembled(&w);
        let carried: Vec<(&str, &str, [f32; 3])> = w
            .get::<Sockets>(rig)
            .unwrap()
            .0
            .iter()
            .map(|k| {
                (
                    k.name.as_str(),
                    s.joints[k.joint.0 as usize].name.as_str(),
                    [k.offset.cols[3][0], k.offset.cols[3][1], k.offset.cols[3][2]],
                )
            })
            .collect();
        assert_eq!(
            carried,
            [
                ("dummy 20", "L_Arm", [0.1, 0.0, 0.0]),
                ("dummy 7", "Neck", [0.0, 0.1, 0.0]),
                ("dummy 85", "Head", [0.0, 0.0, 0.2]),
            ]
        );
        assert_eq!(w.verify_components(), vec![]);
        let log = &report.steps[0].log;
        assert_eq!(log.counts["sockets on the assembled skeleton"], 3);
        assert_eq!(log.counts["sockets two parts give one name and place alike"], 1);
        assert_eq!(
            log.approximations["sockets of head given the name of a socket of body and placed otherwise, body's kept: dummy 20"],
            1
        );
    }

    /// An asset whose source is one `leave-out=` names, or under one, is not a part: it keeps its own rig and members, and
    /// the parts are assembled without it (here the body and the head; the held sword's asset and its form stay as read).
    #[test]
    fn held_objects_are_left_out_of_the_assembly() {
        let (mut w, ..) = world(None, [0.0, 0.5, 0.0]);
        part(
            &mut w,
            "sword",
            &[("Blade", None, [0.0; 3])],
            &[([0.0, 0.5, 0.0], 0)],
        );
        part(
            &mut w,
            "scabbard",
            &[("Blade", None, [0.0; 3])],
            &[([0.0, 0.3, 0.0], 0)],
        );
        for (label, source) in [
            ("sword", "parts/wp_a_0300_l"),
            ("scabbard", "parts/wp_a_0300_l/wp_a_0300_1_l"),
        ] {
            let root = w
                .iter::<Asset>()
                .find(|(r, _)| w.get::<Name>(*r).is_some_and(|n| n.0 == label))
                .unwrap()
                .0;
            w.insert(root, Source(source.to_owned()));
        }
        let held = OWN.replace("name=hero}", "name=hero,leave-out=parts/wp_a_0300_l}");
        let report = run(&mut w, &held).unwrap();
        let log = &report.steps[0].log;
        assert_eq!(log.counts["held objects left out (leave-out=)"], 2);
        assert_eq!(log.counts["parts assembled"], 2);
        // the two held assets and the character
        assert_eq!(w.iter::<Asset>().count(), 3);
        let character = w
            .iter::<Asset>()
            .find(|(r, _)| w.get::<Source>(*r).is_some_and(|s| s.0 == "assembled/hero"))
            .unwrap()
            .1;
        let rig = character.members.iter().find(|m| m.1 == Role::Rig).unwrap().0;
        assert!(
            w.get::<Skeleton>(rig)
                .unwrap()
                .joints
                .iter()
                .all(|j| j.name != "Blade")
        );
        // a source that only begins like one given is not under it
        assert!(!is_held("parts/wp_a_0300_lx", &["parts/wp_a_0300_l".to_owned()]));
        assert!(is_held("parts/wp_a_0300_l/a", &["parts/wp_a_0300_l".to_owned()]));
        // the option prints back as given, and an empty source is refused
        let mut r = PassRegistry::new();
        crate::register(&mut r);
        let pipeline = r.parse(&held).unwrap();
        assert_eq!(pipeline.passes[0].options()["leave-out"], "parts/wp_a_0300_l");
        let empty = OWN.replace("name=hero}", "name=hero,leave-out=parts/a+}");
        assert!(
            r.parse(&empty)
                .unwrap_err()
                .to_string()
                .contains("an empty source")
        );
    }
}
