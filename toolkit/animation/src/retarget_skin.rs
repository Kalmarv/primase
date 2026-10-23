//! `retarget-skin`: a skinned mesh moved onto another skeleton (a costume onto the skeleton a game poses it with), an
//! approximation that states its choices and reports them.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Asset, Binding, Component, Entity, Influences, JointId, JointMap, JointOffset, JointPlacement, Mesh,
    Morphs, Role, Semantic, Skeleton, Skin, SkinInstance, Slot, Stream, World,
};

use crate::deform::{Vertices, influences, skin};
use crate::fit::{Fit, FitReport, Measure, Rig, fitted, joint_report};
use crate::placement::{Frames, frames, place};
use crate::pose::{Affine, affine, compose, inverse, locals, model_transforms, transform};

/// Where the weight of a source joint the map does not list goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnmappedJoints {
    /// Refuse the mesh, naming the first such joint that carries weight.
    Refuse,
    /// To its nearest mapped ancestor; refused for a joint with none.
    Ancestor,
    /// To the mapped joint nearest it at rest (in the source's model space; of equally near ones, the first in the
    /// source skeleton's joint list).
    Nearest,
}

/// `retarget-skin{fit=positions|lengths,ik=none|root:mid:end;...,scale=1,unmapped=refuse|ancestor|nearest,placement=automatic|stated}`
/// (`fit`, `unmapped` and `placement` required; `ik` required with `fit=lengths` and refused with `fit=positions`;
/// `scale`, default 1, only with `fit=lengths`): every skinned prototype bound to the `from` skeleton of a
/// `core.joint_map` is moved onto its `to` skeleton, in three steps:
///
/// 1. **Fit.** The source skeleton is posed onto the target's proportions, by the rule `fit` names ([`Fit`]):
///    `positions` places each mapped joint at its target joint's rest position and turns it (the least rotation) so
///    its bone points the way its target's bone does (the bone to its one mapped joint below; a joint with none or
///    several keeps its parent's turn), so each segment takes the target's length and the mesh stretches between the
///    joints; `lengths` keeps each segment's source length and points it the way the target's points, a pelvis,
///    shoulders' joint or hand moves as one rigid body turned to its children's target directions (a hand's roll), and
///    each `ik` limb (source joint names, `root:mid:end`, several joined by `;`) is solved by two-bone IK so its end
///    reaches the target's end joint as far as the lengths allow (`scale` first sizes the whole source by one factor,
///    no segment more than another). Joints the map does not list keep their rest relative to their parent. Bone
///    directions, not whole rotations, are matched: two rigs' bone axes need not agree.
///    With `placement=stated` the map's `core.joint_placement` then moves where the target
///    joints stand inside the mesh from where this fit put them ([`crate::placement`]): the mesh weighted to a mapped
///    joint follows its moved target joint, and the joints the map does not list under it follow it; a joint with the
///    identity offset keeps the fit's own. `placement=automatic` is the fit alone and refuses a map that has a
///    placement (it would be ignored); `stated` refuses a map that has none.
/// 2. **Deform.** The mesh is skinned into that pose by its own weights (positions, normals, tangents): it now stands
///    in the target's rest pose, with the target's proportions under `positions` and the source's segment lengths under
///    `lengths`.
/// 3. **Rebind.** Each influence moves to its joint's target joint, the weight of joints the map does not list as
///    `unmapped` says; weights on one target joint are summed (each vertex's sum is unchanged). The new binding names the
///    target skeleton, one palette of the target joints used (in joint order), each inverse bind the inverse of its
///    joint's rest in the target's model space; influences as `Streams` of as many sets of four as the most influences
///    a vertex has, weights as floats (`quantize-skin-weights` and `limit-skin-influences` make them a destination's).
///
/// Both skeletons must share a model space (metres, the same up and facing): the program turns one first. The mesh must
/// be in the source skeleton's model space: every joint that carries weight has an inverse bind that is the inverse of
/// its rest there (to within [`BIND_TOLERANCE`]), else the prototype is refused naming the joint. Two joint maps from
/// one skeleton are refused (which is meant is not stated). A prototype already placed with a skin instance is refused (retarget, then place), as is one with morph targets (their deltas
/// are not carried yet) or more than one binding. An asset listing the source skeleton as the prototype's rig lists the
/// target instead.
///
/// Approximations, reported: the fit (a body of other proportions), and each unmapped joint's weight moved to another
/// joint, by name, with the vertices it touched. The fit's residuals are reported by name as well ([`FitReport`], also
/// returned by [`RetargetSkin::report`] without changing anything), with any stated placement applied: every mapped
/// segment's source, target and fitted length with the scale the fit applied, every joint farther than a millimetre
/// from its target joint, the frame of each joint with several mapped children (the angle and the roll about the
/// incoming bone), each IK limb's reach, each vertex group's largest displacement (a group is the vertices whose
/// heaviest influence is a source joint), and the largest growth of a mesh edge and the widest split of vertices that
/// stood together.
#[derive(Clone, Debug, PartialEq)]
pub struct RetargetSkin {
    pub unmapped: UnmappedJoints,
    pub placement: Placement,
    pub fit: Fit,
}

/// Where the target joints stand inside the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// As the fit puts them; a map with a `core.joint_placement` is refused.
    Automatic,
    /// As the map's `core.joint_placement` says; a map without one is refused.
    Stated,
}

impl Choice for UnmappedJoints {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("refuse", UnmappedJoints::Refuse),
        ("ancestor", UnmappedJoints::Ancestor),
        ("nearest", UnmappedJoints::Nearest),
    ];
}

impl Choice for Placement {
    const VALUES: &'static [(&'static str, Self)] =
        &[("automatic", Placement::Automatic), ("stated", Placement::Stated)];
}

/// Which [`Fit`] (option `fit`), its parameters apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FitKind {
    Positions,
    Lengths,
}

impl Choice for FitKind {
    const VALUES: &'static [(&'static str, Self)] =
        &[("positions", FitKind::Positions), ("lengths", FitKind::Lengths)];
}

/// `retarget-skin`'s declaration ([`primase_core::pass::PassRegistry::add`]).
pub const RETARGET_SKIN: PassSpec = PassSpec {
    name: "retarget-skin",
    options: &[
        OptionSpec::choice::<FitKind>(
            "fit",
            "how the target's joints are fitted: to the source's positions, or keeping the target's bone lengths",
        ),
        OptionSpec::optional(
            "ik",
            "none",
            "with fit=lengths, required: none, or the limbs solved by IK as root:mid:end joint names, `;` between",
        ),
        OptionSpec::optional(
            "scale",
            "1",
            "with fit=lengths, above 0: the target's size over the source's; without it 1",
        ),
        OptionSpec::choice::<UnmappedJoints>(
            "unmapped",
            "a weighted source joint the map leaves out: refused, given to its nearest mapped ancestor, or to the \
             nearest mapped joint at rest",
        ),
        OptionSpec::choice::<Placement>(
            "placement",
            "where the target's joints stand: as the fit puts them, or as the map's core.joint_placement says",
        ),
    ],
    make: retarget_skin,
};

fn retarget_skin(g: &Given) -> Result<Box<dyn Pass>, String> {
    let ik = match g.optional("ik") {
        None => None,
        Some("none") => Some(Vec::new()),
        Some(v) => Some(
            v.split(';')
                .map(|limb| {
                    let names: Vec<&str> = limb.split(':').collect();
                    match names.as_slice() {
                        [a, b, c] if names.iter().all(|n| !n.is_empty()) => {
                            Ok([(*a).to_owned(), (*b).to_owned(), (*c).to_owned()])
                        }
                        _ => Err(format!("ik={v}: none, or root:mid:end joined by ;")),
                    }
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
    };
    let size = g
        .optional("scale")
        .map(|v| {
            v.parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .ok_or(format!("scale={v}: a number above 0"))
        })
        .transpose()?;
    let fit = match (g.choice::<FitKind>("fit"), ik) {
        (FitKind::Positions, Some(_)) => return Err("ik applies to fit=lengths".into()),
        (FitKind::Positions, None) if size.is_some() => return Err("scale applies to fit=lengths".into()),
        (FitKind::Positions, None) => Fit::Positions,
        (FitKind::Lengths, Some(ik)) => Fit::Lengths {
            ik,
            scale: size.unwrap_or(1.0),
        },
        (FitKind::Lengths, None) => {
            return Err("fit=lengths needs ik=none|root:mid:end;... (the limbs to solve by IK)".into());
        }
    };
    Ok(Box::new(RetargetSkin {
        unmapped: g.choice::<UnmappedJoints>("unmapped"),
        placement: g.choice::<Placement>("placement"),
        fit,
    }))
}

/// A joint map's skeletons, rests and fit (the stated placement applied), ready to measure or apply.
struct Plan {
    source: Skeleton,
    target: Skeleton,
    pairs: BTreeMap<JointId, JointId>,
    source_rest: BTreeMap<JointId, Affine>,
    target_rest: BTreeMap<JointId, Affine>,
    fitted: BTreeMap<JointId, Affine>,
    limbs: Vec<[JointId; 3]>,
    /// The source joints a stated placement moved.
    placed: BTreeSet<JointId>,
}

impl Plan {
    fn new(
        w: &World,
        map: &JointMap,
        fit: &Fit,
        offsets: &BTreeMap<JointId, JointOffset>,
    ) -> Result<Plan, String> {
        let source = w
            .get::<Skeleton>(map.from)
            .ok_or("the map's source is no skeleton")?
            .clone();
        let target = w
            .get::<Skeleton>(map.to)
            .ok_or("the map's target is no skeleton")?
            .clone();
        let pairs: BTreeMap<JointId, JointId> = map.pairs.iter().copied().collect();
        let source_rest = model_transforms(&source, &locals(&source, None)).map_err(|e| e.0)?;
        let target_rest = model_transforms(&target, &locals(&target, None)).map_err(|e| e.0)?;
        let (automatic, limbs) = fitted(
            fit,
            &Rig::new(&source, &pairs, source_locals(&source)),
            &source_rest,
            &target_rest,
        )?;
        let (fitted, placed) = place(&source, &source_rest, &automatic, &target_rest, &pairs, offsets)?;
        Ok(Plan {
            source,
            target,
            pairs,
            source_rest,
            target_rest,
            fitted,
            limbs,
            placed,
        })
    }

    /// The joint-level report (the vertices' parts are added by the caller).
    fn joint_report(&self) -> FitReport {
        joint_report(
            &Rig::new(&self.source, &self.pairs, source_locals(&self.source)),
            &self.source_rest,
            &self.target_rest,
            &self.target,
            &self.fitted,
            &self.limbs,
        )
    }
}

fn source_locals(source: &Skeleton) -> BTreeMap<JointId, Affine> {
    locals(source, None)
        .into_iter()
        .map(|(j, r)| (j, affine(&r.matrix())))
        .collect()
}

/// The most an entry of a weighted joint's rest times its inverse bind may differ from the identity (the mesh is in the
/// skeleton's model space when it is zero): single-precision noise of values stored as f32.
const BIND_TOLERANCE: f64 = 1e-4;

/// The joints carrying weight on each vertex of each primitive of `p`'s mesh (`None` for a primitive with no skin), by
/// source joint.
fn named_influences(
    w: &World,
    p: Entity,
    skin_c: &Skin,
    binding: &Binding,
) -> Result<Vec<Option<VertexJoints>>, String> {
    let mesh = w.get::<Mesh>(p).ok_or("a skinned prototype has no mesh")?;
    let mut per_prim = Vec::new();
    for (i, prim) in mesh.primitives().iter().enumerate() {
        let (Some(Some(how)), Some(Some(palette))) = (skin_c.influences.get(i), binding.palettes.get(i))
        else {
            per_prim.push(None);
            continue;
        };
        let inf =
            influences(prim, *how, skin_c.weights).map_err(|e| format!("{p:?} primitive {i}: {}", e.0))?;
        per_prim.push(Some(
            inf.iter()
                .map(|v| {
                    v.iter()
                        .filter(|(_, wt)| *wt > 0.0)
                        .map(|&(s, wt)| (palette[s].joint, wt))
                        .collect()
                })
                .collect(),
        ));
    }
    Ok(per_prim)
}

/// The vertex measures of every skinned prototype bound to `map`'s source, under `plan`'s fit (prototypes that `run`
/// would refuse are measured all the same), and the report with them.
fn measured(w: &World, map: &JointMap, plan: &Plan) -> Result<FitReport, String> {
    let mut measure = Measure::default();
    for (p, skin_c) in w.iter::<Skin>() {
        let Some(binding) = skin_c.bindings.iter().find(|b| b.skeleton == map.from) else {
            continue;
        };
        let mesh = w.get::<Mesh>(p).ok_or("a skinned prototype has no mesh")?;
        let named = named_influences(w, p, skin_c, binding)?;
        for (i, prim) in mesh.primitives().iter().enumerate() {
            let (Some(named), Some(how), Some(Some(palette))) =
                (&named[i], skin_c.influences[i], binding.palettes.get(i))
            else {
                continue;
            };
            let posed = skin(
                &Vertices::of(prim).map_err(|e| e.0)?,
                prim,
                how,
                skin_c.weights,
                binding.deformation,
                palette,
                &plan.fitted,
            )
            .map_err(|e| format!("{p:?} primitive {i}: {}", e.0))?;
            measure.observe(prim, prim.positions().unwrap_or(&[]), &posed.positions, named);
        }
    }
    Ok(measure.finish(&plan.source, plan.joint_report()))
}

/// The report as the pass log's approximations (each line one item, with the vertices it concerns).
fn log_report(report: &FitReport, log: &mut PassLog) {
    for (line, n) in report.lines() {
        log.approximate(&line, n);
    }
}

/// Joint to joint.
type JointMapping = BTreeMap<JointId, JointId>;

/// Per vertex, its joints and weights.
type VertexJoints = Vec<Vec<(JointId, f64)>>;

/// Each source joint's target joint, the unmapped ones by `policy` (those that carry weight only: `used`); for each
/// moved joint, the joint it went to.
fn resolve(
    policy: UnmappedJoints,
    source: &Skeleton,
    source_rest: &BTreeMap<JointId, Affine>,
    pairs: &BTreeMap<JointId, JointId>,
    used: &BTreeSet<JointId>,
) -> Result<(JointMapping, JointMapping), String> {
    let parent: BTreeMap<JointId, Option<JointId>> = source.joints.iter().map(|j| (j.id, j.parent)).collect();
    let name = |j: JointId| {
        source
            .joints
            .iter()
            .find(|x| x.id == j)
            .map_or_else(|| format!("{j:?}"), |x| x.name.clone())
    };
    let mut to = pairs.clone();
    let mut moved = BTreeMap::new();
    for &j in used.iter().filter(|j| !pairs.contains_key(j)) {
        let via = match policy {
            UnmappedJoints::Refuse => {
                return Err(format!(
                    "joint {} carries weight and the map does not list it",
                    name(j)
                ));
            }
            UnmappedJoints::Ancestor => {
                let mut at = parent[&j];
                loop {
                    match at {
                        Some(p) if pairs.contains_key(&p) => break p,
                        Some(p) => at = parent[&p],
                        None => {
                            return Err(format!(
                                "joint {} carries weight and has no mapped ancestor (unmapped=nearest places it)",
                                name(j)
                            ));
                        }
                    }
                }
            }
            UnmappedJoints::Nearest => {
                let p = source_rest[&j][3];
                // in the source skeleton's joint order, so that equally near joints go to the first listed
                source
                    .joints
                    .iter()
                    .map(|k| k.id)
                    .filter(|k| pairs.contains_key(k))
                    .min_by(|a, b| {
                        let d =
                            |k: &JointId| (0..3).map(|i| (source_rest[k][3][i] - p[i]).powi(2)).sum::<f64>();
                        d(a).total_cmp(&d(b))
                    })
                    .ok_or("the map lists no joint")?
            }
        };
        to.insert(j, pairs[&via]);
        moved.insert(j, via);
    }
    Ok((to, moved))
}

impl RetargetSkin {
    /// The offsets stated for a map: the map's `core.joint_placement` under `placement=stated`, none under `automatic`
    /// (which refuses a map that has one: it would be ignored).
    pub fn offsets(&self, w: &World, map: Entity) -> Result<BTreeMap<JointId, JointOffset>, String> {
        let placements: Vec<&JointPlacement> = w
            .iter::<JointPlacement>()
            .filter(|(_, p)| p.map == map)
            .map(|(_, p)| p)
            .collect();
        match (self.placement, placements.as_slice()) {
            (Placement::Automatic, []) => Ok(BTreeMap::new()),
            (Placement::Automatic, _) => Err(format!(
                "map {map:?} has a core.joint_placement: placement=stated uses it, placement=automatic would ignore it"
            )),
            (Placement::Stated, [p]) => Ok(p.offsets.iter().copied().collect()),
            (Placement::Stated, _) => Err(format!(
                "placement=stated: map {map:?} has no core.joint_placement"
            )),
        }
    }

    /// The frames the map's target joints stand at in the source's model space under this fit, before and after
    /// `offsets` (by target joint): what an editor draws the target skeleton at ([`crate::placement::frames`]). The
    /// offsets are the caller's, whatever `placement` says; none are read from the world.
    pub fn frames(
        &self,
        w: &World,
        map: Entity,
        offsets: &BTreeMap<JointId, JointOffset>,
    ) -> Result<BTreeMap<JointId, Frames>, String> {
        let joint_map = w.get::<JointMap>(map).ok_or("the entity holds no joint map")?;
        let plan = Plan::new(w, joint_map, &self.fit, &BTreeMap::new())?;
        frames(
            &plan.source,
            &plan.source_rest,
            &plan.fitted,
            &plan.target_rest,
            &plan.pairs,
            offsets,
        )
    }

    /// What the fit would do, per joint map that a skin is bound to, without changing anything ([`FitReport`]): the
    /// rest-pose measures of the fitted skeleton (with the stated placement applied) against the target, and the vertex
    /// measures of every skinned prototype bound to the map's source (prototypes that `run` would refuse are measured
    /// all the same).
    pub fn report(&self, w: &World) -> Result<Vec<(Entity, FitReport)>, String> {
        let mut reports = Vec::new();
        for (map_entity, map) in w.iter::<JointMap>() {
            if !w
                .iter::<Skin>()
                .any(|(_, s)| s.bindings.iter().any(|b| b.skeleton == map.from))
            {
                continue;
            }
            let plan = Plan::new(w, map, &self.fit, &self.offsets(w, map_entity)?)?;
            reports.push((map_entity, measured(w, map, &plan)?));
        }
        Ok(reports)
    }
}

impl Pass for RetargetSkin {
    fn name(&self) -> &'static str {
        "retarget-skin"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("unmapped".to_owned(), self.unmapped.text().to_owned()),
            ("placement".to_owned(), self.placement.text().to_owned()),
        ]);
        match &self.fit {
            Fit::Positions => o.insert("fit".to_owned(), FitKind::Positions.text().to_owned()),
            Fit::Lengths { ik, scale } => {
                o.insert("fit".to_owned(), FitKind::Lengths.text().to_owned());
                o.insert("scale".to_owned(), scale.to_string());
                let limbs: Vec<String> = ik.iter().map(|l| l.join(":")).collect();
                o.insert(
                    "ik".to_owned(),
                    if limbs.is_empty() {
                        "none".to_owned()
                    } else {
                        limbs.join(";")
                    },
                )
            }
        };
        o
    }
    fn requires(&self) -> &[&'static str] {
        match self.placement {
            Placement::Automatic => &[Skin::NAME, JointMap::NAME],
            Placement::Stated => &[Skin::NAME, JointMap::NAME, JointPlacement::NAME],
        }
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME, Asset::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a skinned mesh fitted onto a skeleton of other proportions; weight of joints the map does not list moved"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let maps: Vec<(Entity, JointMap)> = w.iter::<JointMap>().map(|(e, m)| (e, m.clone())).collect();
        let mut from = BTreeSet::new();
        for (_, m) in &maps {
            if !from.insert(m.from) {
                return Err(format!(
                    "two joint maps from skeleton {:?}: which is meant is not stated",
                    m.from
                ));
            }
        }
        let drawn: BTreeSet<Entity> = w
            .iter::<SkinInstance>()
            .filter_map(|(e, _)| w.get::<primase_core::Instance>(e).map(|i| i.prototype))
            .collect();
        for (map_entity, map) in maps {
            let prototypes: Vec<Entity> = w
                .iter::<Skin>()
                .filter(|(_, s)| s.bindings.iter().any(|b| b.skeleton == map.from))
                .map(|(e, _)| e)
                .collect();
            // a map no skin binds to (the correspondence of a clip's rig, say) is not this pass's: it has no mesh to place
            if prototypes.is_empty() {
                continue;
            }
            let offsets = self.offsets(w, map_entity)?;
            let plan = Plan::new(w, &map, &self.fit, &offsets)?;
            // measured before anything moves: the report is of the fit against the meshes as read
            log_report(&measured(w, &map, &plan)?, log);
            let Plan {
                source,
                target,
                pairs,
                source_rest,
                target_rest,
                fitted,
                placed,
                ..
            } = plan;
            if !placed.is_empty() {
                log.count("source joints moved by a stated placement", placed.len() as u64);
            }
            for p in prototypes {
                if drawn.contains(&p) {
                    return Err(format!(
                        "prototype {p:?} is placed with a skin instance: retarget it, then place it"
                    ));
                }
                if w.has::<Morphs>(p) {
                    return Err(format!(
                        "prototype {p:?} has morph targets: their deltas are not carried yet"
                    ));
                }
                let skin_c = w.get::<Skin>(p).ok_or("a skin is gone")?.clone();
                let [binding] = skin_c.bindings.as_slice() else {
                    return Err(format!(
                        "prototype {p:?} has {} bindings: one is retargeted",
                        skin_c.bindings.len()
                    ));
                };
                let mesh = w.get::<Mesh>(p).ok_or("a skinned prototype has no mesh")?.clone();
                // the joints carrying weight, and where each goes
                let per_prim = named_influences(w, p, &skin_c, binding)?;
                let used: BTreeSet<JointId> = per_prim
                    .iter()
                    .flatten()
                    .flatten()
                    .flatten()
                    .map(|(j, _)| *j)
                    .collect();
                // the mesh is in the skeleton's model space when each weighted joint's rest times its inverse bind
                // is the identity: the fit and the new binding both take that for granted
                let identity = affine(&primase_core::Transform::IDENTITY);
                for s in binding.palettes.iter().flatten().flatten() {
                    let Some(rest) = source_rest.get(&s.joint).filter(|_| used.contains(&s.joint)) else {
                        continue;
                    };
                    let off = compose(rest, &affine(&s.inverse_bind))
                        .iter()
                        .zip(&identity)
                        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
                        .fold(0.0, f64::max);
                    if off > BIND_TOLERANCE {
                        let name = source
                            .joints
                            .iter()
                            .find(|x| x.id == s.joint)
                            .map_or_else(|| format!("{:?}", s.joint), |x| x.name.clone());
                        return Err(format!(
                            "prototype {p:?}: joint {name}'s inverse bind is not the inverse of its rest in the skeleton's \
                             model space (off by {off:.3e}): the mesh is not in that space, and fitting it would move it"
                        ));
                    }
                }
                let (to, moved) = resolve(self.unmapped, &source, &source_rest, &pairs, &used)?;
                // the target joints used, as one palette in joint order
                let palette_joints: BTreeSet<JointId> = used.iter().map(|j| to[j]).collect();
                let palette: Vec<Slot> = palette_joints
                    .iter()
                    .map(|&j| {
                        let ib = inverse(&target_rest[&j]).ok_or("a target joint's rest has no inverse")?;
                        Ok(Slot {
                            joint: j,
                            inverse_bind: transform(&ib),
                        })
                    })
                    .collect::<Result<_, String>>()?;
                let slot_of: BTreeMap<JointId, usize> =
                    palette_joints.iter().enumerate().map(|(k, j)| (*j, k)).collect();
                let mut new_mesh = mesh.clone();
                let mut new_influences = Vec::with_capacity(per_prim.len());
                let mut moved_vertices: BTreeMap<JointId, u64> = BTreeMap::new();
                let mut fitted_vertices = 0u64;
                for (i, mut prim) in new_mesh.primitives_mut().enumerate() {
                    let Some(named) = &per_prim[i] else {
                        new_influences.push(None);
                        continue;
                    };
                    let src = mesh.primitives()[i].clone();
                    let how = skin_c.influences[i].ok_or("influences")?;
                    let posed = skin(
                        &Vertices::of(&src).map_err(|e| e.0)?,
                        &src,
                        how,
                        skin_c.weights,
                        binding.deformation,
                        binding.palettes[i].as_ref().ok_or("palette")?,
                        &fitted,
                    )
                    .map_err(|e| format!("{p:?} primitive {i}: {}", e.0))?;
                    fitted_vertices += posed.positions.len() as u64;
                    // the new influences, merged per target joint, heaviest first (ties by slot)
                    let merged: Vec<Vec<(usize, f32)>> = named
                        .iter()
                        .map(|v| {
                            let mut acc: BTreeMap<usize, f64> = BTreeMap::new();
                            for &(j, wt) in v {
                                if moved.contains_key(&j) {
                                    *moved_vertices.entry(j).or_default() += 1;
                                }
                                *acc.entry(slot_of[&to[&j]]).or_default() += wt;
                            }
                            let mut out: Vec<(usize, f32)> =
                                acc.into_iter().map(|(s, wt)| (s, wt as f32)).collect();
                            out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                            out
                        })
                        .collect();
                    let most = merged.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let sets = most.div_ceil(4);
                    let wide = palette.len() > 256;
                    let streams = prim.streams_mut();
                    streams.retain(|s, _| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_)));
                    for k in 0..sets {
                        let (mut js, mut ws): (Vec<[u32; 4]>, Vec<[f32; 4]>) = (Vec::new(), Vec::new());
                        for v in &merged {
                            let pad = v.first().map_or(0, |x| x.0) as u32;
                            let mut jj = [pad; 4];
                            let mut ww = [0.0f32; 4];
                            for c in 0..4 {
                                if let Some(&(s, wt)) = v.get(k * 4 + c) {
                                    jj[c] = s as u32;
                                    ww[c] = wt;
                                }
                            }
                            js.push(jj);
                            ws.push(ww);
                        }
                        streams.insert(
                            Semantic::Joints(k as u8),
                            if wide {
                                Stream::U16x4(js.iter().map(|j| j.map(|x| x as u16)).collect())
                            } else {
                                Stream::U8x4(js.iter().map(|j| j.map(|x| x as u8)).collect())
                            },
                        );
                        streams.insert(Semantic::Weights(k as u8), Stream::F32x4(ws));
                    }
                    streams.insert(Semantic::Position, Stream::F32x3(posed.positions));
                    if let Some(n) = posed.normals {
                        streams.insert(Semantic::Normal, Stream::F32x3(n));
                    }
                    if let Some(t) = posed.tangents {
                        streams.insert(Semantic::Tangent, Stream::F32x4(t));
                    }
                    new_influences.push(Some(Influences::Streams { sets: sets as u8 }));
                }
                let palettes = new_influences
                    .iter()
                    .map(|i| i.map(|_| palette.clone()))
                    .collect();
                w.insert(p, new_mesh);
                w.insert(
                    p,
                    Skin {
                        influences: new_influences,
                        weights: skin_c.weights,
                        bindings: vec![Binding {
                            id: binding.id,
                            skeleton: map.to,
                            palettes,
                            deformation: binding.deformation,
                        }],
                    },
                );
                // the assets drawing it name the target as their rig
                let assets: Vec<Entity> = w
                    .iter::<Asset>()
                    .filter(|(_, a)| a.members.iter().any(|(m, r)| *m == p && *r == Role::Part))
                    .map(|(e, _)| e)
                    .collect();
                for a in assets {
                    let asset = w.get_mut::<Asset>(a).ok_or("an asset is gone")?;
                    asset
                        .members
                        .retain(|(m, r)| !(*m == map.from && *r == Role::Rig));
                    if !asset.members.iter().any(|(m, _)| *m == map.to) {
                        asset.members.push((map.to, Role::Rig));
                    }
                }
                log.approximate(
                    "skinned vertices fitted onto the target skeleton's proportions",
                    fitted_vertices,
                );
                for (j, n) in moved_vertices {
                    let name = |s: &Skeleton, j: JointId| {
                        s.joints
                            .iter()
                            .find(|x| x.id == j)
                            .map_or_else(|| format!("{j:?}"), |x| x.name.clone())
                    };
                    log.approximate(
                        &format!(
                            "vertex weight of unmapped joint {} moved to {} ({})",
                            name(&source, j),
                            name(&target, to[&j]),
                            match self.unmapped {
                                UnmappedJoints::Ancestor => "its mapped ancestor",
                                _ => "the mapped joint nearest it",
                            }
                        ),
                        n,
                    );
                }
                log.count("skinned prototypes retargeted", 1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{BindingId, Deformation, Joint, Primitive, Rest, Topology, WeightRule};

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

    /// Source: root 1 at y=1, arm 2 at (0.2, 1.5, 0) pointing +x, hand 3 at (0.7, 1.5, 0), and a cloth joint 4 under the
    /// arm the map does not list. Target, taller: root 10 at y=2, arm 20 at (0.3, 2.6, 0) turned 45 degrees down, hand
    /// 30 0.6 along it. A mesh of three points: one on the hand (weighted to it), one halfway along the arm (half arm,
    /// half hand), one on the cloth joint: (world, prototype, target skeleton, map entity).
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
                    joint(4, Some(2), trs([0.4, -0.1, 0.0], id)),
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
        // inverse binds: the inverse of each source joint's rest in the model's space (translations only here)
        let slot = |j: u32, t: [f32; 3]| Slot {
            joint: JointId(j),
            inverse_bind: {
                let mut m = primase_core::Transform::IDENTITY;
                m.cols[3] = t.map(|x| -x);
                m
            },
        };
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0.7, 1.5, 0.0], [0.45, 1.5, 0.0], [0.6, 1.4, 0.0]]),
                    ),
                    (
                        Semantic::Joints(0),
                        Stream::U8x4(vec![[2, 0, 0, 0], [1, 2, 0, 0], [3, 0, 0, 0]]),
                    ),
                    (
                        Semantic::Weights(0),
                        Stream::F32x4(vec![
                            [1.0, 0.0, 0.0, 0.0],
                            [0.5, 0.5, 0.0, 0.0],
                            [1.0, 0.0, 0.0, 0.0],
                        ]),
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
                    skeleton: source,
                    palettes: vec![Some(vec![
                        slot(1, [0.0, 1.0, 0.0]),
                        slot(2, [0.2, 1.5, 0.0]),
                        slot(3, [0.7, 1.5, 0.0]),
                        slot(4, [0.6, 1.4, 0.0]),
                    ])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        (w, proto, target)
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

    fn near(a: [f32; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (f64::from(*x) - y).abs() < 1e-5)
    }

    /// Fitted: the hand point lands on the target's hand, the mid-arm point halfway along the target's arm; the cloth
    /// point rides its ancestor (the arm), reported by name; the mesh is bound to the target, its weights summing as
    /// before, and at the target's rest it is where it stands.
    #[test]
    fn a_skin_moves_onto_the_target_skeleton() {
        let (mut w, proto, target) = world();
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap();
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let arm = [0.3, 2.6, 0.0];
        let hand = [0.3 + 0.6 * h, 2.6 - 0.6 * h, 0.0];
        let mesh = w.get::<Mesh>(proto).unwrap();
        let Stream::F32x3(p) = &mesh.primitives()[0].streams[&Semantic::Position] else {
            panic!("positions")
        };
        assert!(near(p[0], hand), "{:?} vs {hand:?}", p[0]);
        let mid: [f64; 3] = std::array::from_fn(|k| (arm[k] + hand[k]) / 2.0);
        assert!(near(p[1], mid), "{:?} vs {mid:?}", p[1]);
        let sk = w.get::<Skin>(proto).unwrap();
        assert_eq!(sk.bindings[0].skeleton, target);
        let joints: Vec<JointId> = sk.bindings[0].palettes[0]
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| s.joint)
            .collect();
        assert_eq!(joints, [JointId(20), JointId(30)]);
        assert_eq!(w.verify_components(), vec![]);
        let log = &report.steps[0].log;
        assert_eq!(
            log.approximations["vertex weight of unmapped joint j4 moved to j20 (its mapped ancestor)"],
            1
        );
        assert_eq!(
            log.approximations["skinned vertices fitted onto the target skeleton's proportions"],
            3
        );
        // at the target's rest the skin leaves the mesh where it stands
        let target_rest = model_transforms(
            w.get::<Skeleton>(target).unwrap(),
            &locals(w.get::<Skeleton>(target).unwrap(), None),
        )
        .unwrap();
        let prim = &mesh.primitives()[0];
        let posed = skin(
            &Vertices::of(prim).unwrap(),
            prim,
            sk.influences[0].unwrap(),
            sk.weights,
            Deformation::Linear,
            sk.bindings[0].palettes[0].as_ref().unwrap(),
            &target_rest,
        )
        .unwrap();
        for (a, b) in posed.positions.iter().zip(p) {
            assert!(near(*a, b.map(f64::from)));
        }
    }

    /// A mesh bound with an inverse bind that is not the inverse of the joint's rest (its bind pose is another pose,
    /// or the mesh sits in its own space) is refused naming the joint, not fitted as if it stood in the skeleton's
    /// space. Here joint 3's inverse bind is moved 0.5 along x.
    #[test]
    fn a_mesh_outside_the_skeletons_model_space_is_refused() {
        let (mut w, proto, _) = world();
        let mut skin_c = w.get::<Skin>(proto).unwrap().clone();
        skin_c.bindings[0].palettes[0].as_mut().unwrap()[2]
            .inverse_bind
            .cols[3][0] -= 0.5;
        w.insert(proto, skin_c);
        let err = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap_err();
        assert!(
            err.contains("joint j3's inverse bind") && err.contains("not in that space"),
            "{err}"
        );
    }

    /// Two maps from one skeleton are refused (the second would find the first's result already moved).
    #[test]
    fn a_second_map_from_one_skeleton_is_refused() {
        let (mut w, ..) = world();
        let first = w.iter::<JointMap>().next().map(|(_, m)| m.clone()).unwrap();
        let again = w.spawn();
        w.insert(again, first);
        let err = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap_err();
        assert!(err.contains("two joint maps from skeleton"), "{err}");
    }

    /// Joints equally near go to the first in the source skeleton's joint list, whatever their ids. Hand-worked: a cloth
    /// joint 9 at (0, 1, 0) is sqrt 2 from both mapped joints, 7 at (1, 0, 0) and 5 at (-1, 0, 0), exactly (the mapped root,
    /// which would be nearer, stands 10 below them); the list
    /// has 7 first (id order would pick 5), so its weight goes to 7's target, 70.
    #[test]
    fn nearest_takes_the_first_of_equals_in_joint_order() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let mut w = World::new();
        let source = w.spawn();
        w.insert(
            source,
            Skeleton {
                joints: vec![
                    joint(1, None, trs([0.0, -10.0, 0.0], id)),
                    joint(7, Some(1), trs([1.0, 10.0, 0.0], id)),
                    joint(5, Some(1), trs([-1.0, 10.0, 0.0], id)),
                    joint(9, Some(1), trs([0.0, 11.0, 0.0], id)),
                ],
            },
        );
        let target = w.spawn();
        w.insert(
            target,
            Skeleton {
                joints: vec![
                    joint(10, None, trs([0.0; 3], id)),
                    joint(70, Some(10), trs([1.0, 0.0, 0.0], id)),
                    joint(50, Some(10), trs([-1.0, 0.0, 0.0], id)),
                ],
            },
        );
        let map = w.spawn();
        w.insert(
            map,
            JointMap {
                from: source,
                to: target,
                pairs: vec![
                    (JointId(1), JointId(10)),
                    (JointId(5), JointId(50)),
                    (JointId(7), JointId(70)),
                ],
            },
        );
        let slot = |j: u32, t: [f32; 3]| Slot {
            joint: JointId(j),
            inverse_bind: {
                let mut m = primase_core::Transform::IDENTITY;
                m.cols[3] = t.map(|x| -x);
                m
            },
        };
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [
                    (Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]])),
                    (Semantic::Joints(0), Stream::U8x4(vec![[3, 0, 0, 0]])),
                    (Semantic::Weights(0), Stream::F32x4(vec![[1.0, 0.0, 0.0, 0.0]])),
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
                    skeleton: source,
                    palettes: vec![Some(vec![
                        slot(1, [0.0; 3]),
                        slot(7, [1.0, 0.0, 0.0]),
                        slot(5, [-1.0, 0.0, 0.0]),
                        slot(9, [0.0, 1.0, 0.0]),
                    ])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=nearest,placement=automatic}",
        )
        .unwrap();
        let moved: Vec<&String> = report.steps[0]
            .log
            .approximations
            .keys()
            .filter(|k| k.starts_with("vertex weight of unmapped joint"))
            .collect();
        assert_eq!(
            moved,
            ["vertex weight of unmapped joint j9 moved to j70 (the mapped joint nearest it)"]
        );
    }

    #[test]
    fn retarget_skin_refuses_what_it_was_not_told() {
        let (mut w, ..) = world();
        let err = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=refuse,placement=automatic}",
        )
        .unwrap_err();
        assert!(
            err.contains("joint j4 carries weight and the map does not list it"),
            "{err}"
        );
        let (mut w, ..) = world();
        let err = run(&mut w, "retarget-skin").unwrap_err();
        assert!(err.contains("fit is required: "), "{err}");
        let err = run(&mut w, "retarget-skin{fit=positions}").unwrap_err();
        assert!(err.contains("unmapped is required: "), "{err}");
        let err = run(&mut w, "retarget-skin{fit=positions,unmapped=ancestor}").unwrap_err();
        assert!(err.contains("placement is required: "), "{err}");
        // nearest: the cloth point goes to the mapped joint nearest the cloth joint at rest (the hand, 0.14 away; the arm is 0.41)
        let (mut w, proto, _) = world();
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=nearest,placement=automatic}",
        )
        .unwrap();
        assert_eq!(
            report.steps[0].log.approximations["vertex weight of unmapped joint j4 moved to j30 (the mapped joint nearest it)"],
            1
        );
        assert_eq!(
            w.get::<Skin>(proto).unwrap().bindings[0].palettes[0]
                .as_ref()
                .unwrap()
                .len(),
            2
        );
    }

    fn positions(w: &World, proto: Entity) -> Vec<[f32; 3]> {
        let mesh = w.get::<Mesh>(proto).unwrap();
        let Stream::F32x3(p) = &mesh.primitives()[0].streams[&Semantic::Position] else {
            panic!("positions")
        };
        p.clone()
    }

    fn length(a: [f32; 3], b: [f32; 3]) -> f64 {
        (0..3).map(|k| f64::from(a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
    }

    /// A placement of the world's map, stating `offsets` (by target joint id).
    fn state(w: &mut World, offsets: &[(u32, JointOffset)]) {
        let map = w.iter::<JointMap>().next().map(|(e, _)| e).unwrap();
        let p = w.spawn();
        w.insert(
            p,
            JointPlacement {
                map,
                offsets: offsets.iter().map(|&(j, o)| (JointId(j), o)).collect(),
            },
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    fn up(y: f32) -> JointOffset {
        JointOffset {
            translation: [0.0, y, 0.0],
            ..JointOffset::IDENTITY
        }
    }

    /// Stating only identity offsets (a joint listed or not) is the automatic fit, bit for bit: the same mesh, the same
    /// skin.
    #[test]
    fn zero_offsets_are_the_automatic_fit_exactly() {
        let (mut auto, p, _) = world();
        run(
            &mut auto,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap();
        let (mut stated, q, _) = world();
        state(
            &mut stated,
            &[(20, JointOffset::IDENTITY), (30, JointOffset::IDENTITY)],
        );
        let report = run(
            &mut stated,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap();
        assert_eq!(auto.get::<Mesh>(p), stated.get::<Mesh>(q));
        assert_eq!(auto.get::<Skin>(p), stated.get::<Skin>(q));
        assert!(
            !report.steps[0]
                .log
                .counts
                .contains_key("source joints moved by a stated placement")
        );
    }

    /// An offset on the hand's target joint moves the vertex weighted only to the hand by the offset, the one half
    /// weighted to it by half, and nothing else; an offset on the arm's moves what the arm carries, and the cloth
    /// joint the map does not list that follows it, and leaves the hand's vertex where it was.
    #[test]
    fn a_stated_offset_moves_exactly_the_vertices_its_joint_weights() {
        let (mut w, p, _) = world();
        run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap();
        let fit = positions(&w, p);

        let (mut w, p, _) = world();
        state(&mut w, &[(30, up(0.1))]);
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap();
        let hand = positions(&w, p);
        // vertices: 0 on the hand, 1 half arm half hand, 2 on the cloth joint (which follows the arm)
        assert!(
            (length(hand[0], fit[0]) - 0.1).abs() < 1e-6,
            "{:?} {:?}",
            hand[0],
            fit[0]
        );
        assert!((length(hand[1], fit[1]) - 0.05).abs() < 1e-6);
        assert_eq!(hand[2], fit[2]);
        assert_eq!(
            report.steps[0].log.counts["source joints moved by a stated placement"],
            1
        );

        let (mut w, p, _) = world();
        state(&mut w, &[(20, up(0.1))]);
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap();
        let arm = positions(&w, p);
        assert_eq!(arm[0], fit[0]);
        assert!((length(arm[1], fit[1]) - 0.05).abs() < 1e-6);
        assert!((length(arm[2], fit[2]) - 0.1).abs() < 1e-6);
        // the arm and the cloth joint that follows it
        assert_eq!(
            report.steps[0].log.counts["source joints moved by a stated placement"],
            2
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    /// The report measures the fit with the stated placement applied: the largest displacement of any vertex group is the
    /// largest the pass moves a vertex (the hand's group is displaced otherwise than under the automatic fit), and the
    /// pass logs what the report holds.
    #[test]
    fn the_report_measures_the_fit_with_the_stated_placement() {
        let largest = |r: &FitReport| {
            r.groups
                .iter()
                .map(|g| g.largest_displacement)
                .fold(0.0, f64::max)
        };
        let pass = |placement| RetargetSkin {
            unmapped: UnmappedJoints::Ancestor,
            placement,
            fit: Fit::Positions,
        };
        let (w, ..) = world();
        let automatic = pass(Placement::Automatic).report(&w).unwrap();
        assert_eq!(automatic.len(), 1);
        let (mut w, p, _) = world();
        state(&mut w, &[(30, up(0.1))]);
        let stated = pass(Placement::Stated).report(&w).unwrap();
        let before = positions(&w, p);
        let run_report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap();
        let moved = positions(&w, p)
            .iter()
            .zip(&before)
            .map(|(a, b)| length(*a, *b))
            .fold(0.0, f64::max);
        assert!((largest(&stated[0].1) - moved).abs() < 1e-6, "{stated:?} {moved}");
        assert_ne!(stated[0].1.groups, automatic[0].1.groups);
        let logged = &run_report.steps[0].log.approximations;
        assert!(
            logged
                .keys()
                .any(|k| k.starts_with("fit: vertex group j3 moved at most"))
        );
        assert!(logged.keys().any(|k| k.starts_with("fit: segment j2 to j3 of")));
    }

    /// The frames an editor draws: the automatic one of a target joint sits at its source joint (the positions fit), the
    /// placed one moves by the stated translation, and a joint with no offset keeps its automatic frame.
    #[test]
    fn the_frames_of_the_target_joints_follow_the_stated_offsets() {
        let (w, ..) = world();
        let map = w.iter::<JointMap>().next().map(|(e, _)| e).unwrap();
        let pass = RetargetSkin {
            unmapped: UnmappedJoints::Ancestor,
            placement: Placement::Automatic,
            fit: Fit::Positions,
        };
        let offsets = BTreeMap::from([(JointId(30), up(0.1))]);
        let f = pass.frames(&w, map, &offsets).unwrap();
        assert_eq!(f.len(), 3);
        let hand = f[&JointId(30)];
        for (got, want) in hand.automatic[3].iter().zip([0.7, 1.5, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{:?}", hand.automatic);
        }
        for (got, want) in hand.placed[3].iter().zip([0.7, 1.6, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{:?}", hand.placed);
        }
        assert_eq!(f[&JointId(20)].automatic, f[&JointId(20)].placed);
        // an offset of a joint no pair names is refused
        let err = pass
            .frames(&w, map, &BTreeMap::from([(JointId(99), up(0.1))]))
            .unwrap_err();
        assert!(err.contains("no pair names"), "{err}");
    }

    /// `fit=lengths` runs through the whole pass and leaves a world the checks accept.
    #[test]
    fn the_lengths_fit_runs_through_the_pass() {
        let (mut w, ..) = world();
        let report = run(
            &mut w,
            "retarget-skin{fit=lengths,ik=none,unmapped=ancestor,placement=automatic}",
        )
        .unwrap();
        assert!(
            report.steps[0]
                .log
                .approximations
                .keys()
                .any(|k| k.contains("fit: segment j2 to j3 of 500.0 mm") && k.contains("scale 1.000"))
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    /// `automatic` refuses a map that has a placement, and `stated` a world that has none.
    #[test]
    fn the_placement_choice_is_checked() {
        let (mut w, ..) = world();
        state(&mut w, &[(30, up(0.1))]);
        let err = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=automatic}",
        )
        .unwrap_err();
        assert!(err.contains("placement=automatic would ignore it"), "{err}");
        let (mut w, ..) = world();
        let err = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap_err();
        // the pass's own requirement refuses a world with no placement at all
        assert!(err.contains("requires `core.joint_placement`"), "{err}");
    }

    /// A map no skin binds to (a clip rig's correspondence to the same target, say) is not this pass's: `stated` asks no
    /// placement of it and `automatic` finds none to refuse, whatever the world's other maps say.
    #[test]
    fn a_map_with_no_skin_to_retarget_is_left_out_of_the_placement_choice() {
        let (mut w, _, target) = world();
        state(&mut w, &[(30, up(0.1))]);
        let rig = w.spawn();
        w.insert(
            rig,
            Skeleton {
                joints: vec![joint(100, None, trs([0.0; 3], [0.0, 0.0, 0.0, 1.0]))],
            },
        );
        let other = w.spawn();
        w.insert(
            other,
            JointMap {
                from: rig,
                to: target,
                pairs: vec![(JointId(100), JointId(10))],
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        let report = run(
            &mut w,
            "retarget-skin{fit=positions,unmapped=ancestor,placement=stated}",
        )
        .unwrap();
        assert_eq!(
            report.steps[0].log.counts["source joints moved by a stated placement"], 1,
            "the hand alone: nothing the map leaves out hangs under it"
        );
        assert_eq!(report.steps[0].log.counts["skinned prototypes retargeted"], 1);
    }
}
