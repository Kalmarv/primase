//! The fit of `retarget-skin`: the source skeleton posed onto a target skeleton's proportions, by one of two rules, and
//! the report of what each rule did (what it changed in every segment, how far each joint stands from its target, what
//! the vertices suffered). The pass is [`crate::retarget_skin`]; this module is its arithmetic. `positions` is
//! [`crate::placement::fit`]; the stated placements ([`crate::placement::place`]) move the target joints from where
//! either rule put them, and the report measures the fit with them applied.
//!
//! A *mapped segment* is a mapped joint and one mapped joint below it with no mapped joint between (unmapped joints
//! in between ride the parent). Everything is measured in each skeleton's rest, in the model space the two share.

use std::collections::BTreeMap;

use primase_core::{JointId, Primitive, Skeleton, Topology};

use crate::pose::{Affine, compose};
use crate::quat;
use crate::retarget::{between, parents_first, rotate, rotation, with_rotation};

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}

fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}

fn scale(a: V3, s: f64) -> V3 {
    a.map(|x| x * s)
}

fn dot(a: V3, b: V3) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

fn dist(a: V3, b: V3) -> f64 {
    norm(sub(a, b))
}

/// `a` of unit length; `None` for one shorter than a nanometre.
fn unit(a: V3) -> Option<V3> {
    let l = norm(a);
    (l > 1e-9).then(|| scale(a, 1.0 / l))
}

const IDENTITY: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

/// How the source is posed onto the target's proportions.
#[derive(Clone, Debug, PartialEq)]
pub enum Fit {
    /// Every mapped joint is placed at its target joint's position and its bone turned (the least rotation) to point as
    /// the target's does, so each segment takes the target's length and the mesh stretches between the joints. A joint
    /// with several mapped joints below it keeps its parent's turn. This is [`crate::placement::fit`].
    Positions,
    /// Every segment keeps the source's length. The root-most mapped joint is placed at its target joint; from there
    /// the rule below fixes where the rest go, so the chains end up where the source's lengths let them, and the
    /// distance to the target joint is reported.
    ///
    /// - A mapped joint with one mapped joint below it points that segment the way the target's does (the least
    ///   rotation), at the source's length.
    /// - A mapped joint with several (a pelvis, the shoulders' joint, a hand) is one rigid body: its children keep
    ///   their source offsets, turned together by the one rotation that best takes their source directions onto the
    ///   target's (Horn's quaternion solution), so a hand's roll follows its fingers' directions in the target. A
    ///   joint whose children are all in one line has no roll to set: it gets the least rotation of its first child.
    /// - A listed limb (`root`, `mid`, `end`, source joint names) is solved by two-bone IK: `mid` and `end` are placed
    ///   at the source's lengths with `end` as near the target's `end` joint as the lengths allow (when the target is
    ///   farther than the limb reaches, or nearer than it folds, the limb is straight or folded along the line to it
    ///   and the shortfall is reported), and `mid` bent toward the target's `mid` joint.
    ///
    /// `scale` multiplies the whole source, every length and the mesh with it, by one factor (1 keeps the source's
    /// size): one size for a body of other height, not a stretch of any segment. The program chooses it (the ratio
    /// of the two skeletons' hip heights, say); the report shows what is left.
    Lengths { ik: Vec<[String; 3]>, scale: f64 },
}

/// A mapped joint's relations: its mapped parent and the mapped joints below it.
pub(crate) struct Rig<'a> {
    pub source: &'a Skeleton,
    pub pairs: &'a BTreeMap<JointId, JointId>,
    local: BTreeMap<JointId, Affine>,
    /// Each mapped joint's mapped joints below it with no mapped joint between, in joint order.
    pub below: BTreeMap<JointId, Vec<JointId>>,
    /// Each mapped joint's nearest mapped ancestor.
    pub above: BTreeMap<JointId, JointId>,
}

impl<'a> Rig<'a> {
    pub fn new(
        source: &'a Skeleton,
        pairs: &'a BTreeMap<JointId, JointId>,
        local: BTreeMap<JointId, Affine>,
    ) -> Rig<'a> {
        let parent: BTreeMap<JointId, Option<JointId>> =
            source.joints.iter().map(|j| (j.id, j.parent)).collect();
        let mut below: BTreeMap<JointId, Vec<JointId>> = BTreeMap::new();
        let mut above = BTreeMap::new();
        for j in source.joints.iter().filter(|j| pairs.contains_key(&j.id)) {
            let mut at = parent[&j.id];
            while let Some(p) = at {
                if pairs.contains_key(&p) {
                    below.entry(p).or_default().push(j.id);
                    above.insert(j.id, p);
                    break;
                }
                at = parent[&p];
            }
        }
        Rig {
            source,
            pairs,
            local,
            below,
            above,
        }
    }

    /// The joint named `name`, which must be one and mapped.
    pub fn mapped_named(&self, name: &str) -> Result<JointId, String> {
        let mut found = self.source.joints.iter().filter(|j| j.name == name);
        let (Some(j), None) = (found.next(), found.next()) else {
            return Err(format!(
                "ik: {name} is not exactly one joint of the source skeleton"
            ));
        };
        if !self.pairs.contains_key(&j.id) {
            return Err(format!("ik: joint {name} is not in the joint map"));
        }
        Ok(j.id)
    }

    /// A limb's joints, checked: `mid` is `root`'s only mapped child and `end` is `mid`'s only one.
    pub fn limb(&self, names: &[String; 3]) -> Result<[JointId; 3], String> {
        let [root, mid, end] = [0, 1, 2].map(|i| self.mapped_named(&names[i]));
        let (root, mid, end) = (root?, mid?, end?);
        let only = |p: JointId, c: JointId| self.below.get(&p).map(Vec::as_slice) == Some(&[c]);
        if !only(root, mid) || !only(mid, end) {
            return Err(format!(
                "ik: {}:{}:{} is not a chain of single mapped children ({} has {} mapped children, {} has {})",
                names[0],
                names[1],
                names[2],
                names[0],
                self.below.get(&root).map_or(0, Vec::len),
                names[1],
                self.below.get(&mid).map_or(0, Vec::len),
            ));
        }
        Ok([root, mid, end])
    }
}

/// A fit's every source joint's model transform, and the limbs it solved.
pub(crate) type Fitted = (BTreeMap<JointId, Affine>, Vec<[JointId; 3]>);

/// `fit` applied: every source joint's model transform (before any stated placement), and the limbs solved by IK.
pub(crate) fn fitted(
    fit: &Fit,
    rig: &Rig<'_>,
    source_rest: &BTreeMap<JointId, Affine>,
    target_rest: &BTreeMap<JointId, Affine>,
) -> Result<Fitted, String> {
    match fit {
        Fit::Positions => Ok((
            crate::placement::fit(rig.source, target_rest, rig.pairs)?,
            Vec::new(),
        )),
        Fit::Lengths { ik, scale } => {
            let limbs = ik
                .iter()
                .map(|names| rig.limb(names))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((fit_lengths(rig, source_rest, target_rest, &limbs, *scale)?, limbs))
        }
    }
}

/// Every source joint's model transform, each segment at the source's length (see [`Fit::Lengths`]); `limbs` are
/// checked ([`Rig::limb`]).
pub(crate) fn fit_lengths(
    rig: &Rig<'_>,
    source_rest: &BTreeMap<JointId, Affine>,
    target_rest: &BTreeMap<JointId, Affine>,
    limbs: &[[JointId; 3]],
    size: f64,
) -> Result<BTreeMap<JointId, Affine>, String> {
    let spos = |j: JointId| scale(source_rest[&j][3], size);
    let sized: Affine = [[size, 0.0, 0.0], [0.0, size, 0.0], [0.0, 0.0, size], [0.0; 3]];
    let tpos = |j: JointId| target_rest[&rig.pairs[&j]][3];
    let limb_at: BTreeMap<JointId, [JointId; 3]> = limbs.iter().map(|l| (l[0], *l)).collect();
    // each mapped joint's turn (the rotation that takes its source rest orientation to its fitted one, in the model's
    // space) and, once its parent is placed, its position
    let mut turn: BTreeMap<JointId, [f64; 4]> = BTreeMap::new();
    let mut place: BTreeMap<JointId, V3> = BTreeMap::new();
    let mut out: BTreeMap<JointId, Affine> = BTreeMap::new();
    for i in parents_first(rig.source)? {
        let j = &rig.source.joints[i];
        let m = match j.parent {
            Some(p) => compose(&out[&p], &rig.local[&j.id]),
            None => compose(&sized, &rig.local[&j.id]),
        };
        if !rig.pairs.contains_key(&j.id) {
            out.insert(j.id, m);
            continue;
        }
        let id = j.id;
        let t_in = rig.above.get(&id).map_or(IDENTITY, |p| turn[p]);
        let pos = *place.entry(id).or_insert_with(|| tpos(id));
        let kids = rig.below.get(&id).map_or(&[][..], Vec::as_slice);
        let offs: Vec<V3> = kids.iter().map(|&c| sub(spos(c), spos(id))).collect();
        if let Some(&[_, mid, end]) = limb_at.get(&id) {
            let (a, b) = (dist(spos(mid), spos(id)), dist(spos(end), spos(mid)));
            let bend = sub(spos(mid), spos(id));
            let (m1, e1) = two_bone(pos, a, b, tpos(end), tpos(mid), bend);
            place.insert(mid, m1);
            place.insert(end, e1);
        }
        let t = match kids {
            [] => t_in,
            [c] => {
                let want = unit(sub(tpos(*c), tpos(id)));
                let child = *place.entry(*c).or_insert_with(|| match want {
                    Some(d) => add(pos, scale(d, norm(offs[0]))),
                    None => add(pos, rotate(t_in, offs[0])),
                });
                quat::mul(between(rotate(t_in, offs[0]), sub(child, pos)), t_in)
            }
            _ => {
                let from: Vec<V3> = offs.clone();
                let to: Vec<V3> = kids.iter().map(|&c| sub(tpos(c), tpos(id))).collect();
                let t = best_turn(&from, &to).unwrap_or_else(|| {
                    // all children in one line: the least rotation of the first child that has a direction
                    let k = (0..kids.len()).find(|&k| norm(from[k]) > 1e-9 && norm(to[k]) > 1e-9);
                    k.map_or(t_in, |k| quat::mul(between(rotate(t_in, from[k]), to[k]), t_in))
                });
                for (&c, &o) in kids.iter().zip(&offs) {
                    place.insert(c, add(pos, rotate(t, o)));
                }
                t
            }
        };
        turn.insert(id, t);
        out.insert(
            id,
            with_rotation(pos, quat::mul(t, rotation(&source_rest[&id])), &m),
        );
    }
    Ok(out)
}

/// A two-bone chain from `root` with bones of length `a` then `b`, its end as near `effector` as it reaches, bent
/// toward `pole`: `(mid, end)`. The end's distance to the effector is clamped to `[|a - b|, a + b]` (straight or folded
/// along the line to the effector). Where `pole` lies on the line, the bend is toward `fallback` (a direction from
/// the root, the source's own bend); where that lies on it too, toward any direction at right angles.
pub(crate) fn two_bone(root: V3, a: f64, b: f64, effector: V3, pole: V3, fallback: V3) -> (V3, V3) {
    let d = sub(effector, root);
    let u = unit(d)
        .or_else(|| unit(sub(pole, root)))
        .unwrap_or([0.0, 1.0, 0.0]);
    let reach = norm(d).clamp((a - b).abs(), a + b);
    let x = if reach > 1e-9 {
        (a * a - b * b + reach * reach) / (2.0 * reach)
    } else {
        0.0
    };
    let h = (a * a - x * x).max(0.0).sqrt();
    let perp = |v: V3| unit(sub(v, scale(u, dot(v, u))));
    let n = perp(sub(pole, root))
        .or_else(|| perp(fallback))
        .or_else(|| {
            perp(if u[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            })
        })
        .unwrap_or([0.0, 0.0, 1.0]);
    (
        add(root, add(scale(u, x), scale(n, h))),
        add(root, scale(u, reach)),
    )
}

/// The rotation that best takes the directions `from` onto `to` in the least-squares sense on unit vectors (Horn's
/// quaternion solution: the eigenvector of the largest eigenvalue of a symmetric 4 x 4 matrix of their products);
/// pairs with a zero vector are left out. `None` when the directions left do not fix a rotation (fewer than two, or all
/// in one line on either side).
pub(crate) fn best_turn(from: &[V3], to: &[V3]) -> Option<[f64; 4]> {
    let pairs: Vec<(V3, V3)> = from
        .iter()
        .zip(to)
        .filter_map(|(&f, &t)| Some((unit(f)?, unit(t)?)))
        .collect();
    let spread = |side: &dyn Fn(&(V3, V3)) -> V3| {
        pairs
            .iter()
            .flat_map(|p| pairs.iter().map(move |q| norm(cross(side(p), side(q)))))
            .fold(0.0, f64::max)
    };
    if pairs.len() < 2 || spread(&|p| p.0) < 1e-4 || spread(&|p| p.1) < 1e-4 {
        return None;
    }
    let mut s = [[0.0f64; 3]; 3];
    for (f, t) in &pairs {
        for (a, row) in s.iter_mut().enumerate() {
            for (b, e) in row.iter_mut().enumerate() {
                *e += f[a] * t[b];
            }
        }
    }
    let n = [
        [
            s[0][0] + s[1][1] + s[2][2],
            s[1][2] - s[2][1],
            s[2][0] - s[0][2],
            s[0][1] - s[1][0],
        ],
        [
            s[1][2] - s[2][1],
            s[0][0] - s[1][1] - s[2][2],
            s[0][1] + s[1][0],
            s[2][0] + s[0][2],
        ],
        [
            s[2][0] - s[0][2],
            s[0][1] + s[1][0],
            -s[0][0] + s[1][1] - s[2][2],
            s[1][2] + s[2][1],
        ],
        [
            s[0][1] - s[1][0],
            s[2][0] + s[0][2],
            s[1][2] + s[2][1],
            -s[0][0] - s[1][1] + s[2][2],
        ],
    ];
    let v = largest_eigenvector(n);
    // (w, x, y, z) to [x, y, z, w], with w not negative
    let q = if v[0] < 0.0 { v.map(|x| -x) } else { v };
    quat::normalize([q[1], q[2], q[3], q[0]])
}

/// The unit eigenvector of the largest eigenvalue of a symmetric 4 x 4 matrix, by cyclic Jacobi rotations.
fn largest_eigenvector(mut a: [[f64; 4]; 4]) -> [f64; 4] {
    let mut v = [[0.0f64; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..64 {
        let off: f64 = (0..4)
            .flat_map(|p| (p + 1..4).map(move |q| (p, q)))
            .map(|(p, q)| a[p][q] * a[p][q])
            .sum();
        if off < 1e-30 {
            break;
        }
        for p in 0..3 {
            for q in p + 1..4 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in &mut a {
                    let (rp, rq) = (row[p], row[q]);
                    row[p] = c * rp - s * rq;
                    row[q] = s * rp + c * rq;
                }
                let (rp, rq) = (a[p], a[q]);
                for k in 0..4 {
                    a[p][k] = c * rp[k] - s * rq[k];
                    a[q][k] = s * rp[k] + c * rq[k];
                }
                for row in &mut v {
                    let (vp, vq) = (row[p], row[q]);
                    row[p] = c * vp - s * vq;
                    row[q] = s * vp + c * vq;
                }
            }
        }
    }
    let best = (0..4).max_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    std::array::from_fn(|k| v[k][best])
}

/// One segment: a mapped joint and one mapped joint below it.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentFit {
    /// The joint below (source name).
    pub to: String,
    /// The source's distance between the two joints at rest, in metres.
    pub source: f64,
    /// The target's distance between the two target joints at rest.
    pub target: f64,
    /// The distance the fit left between them.
    pub fitted: f64,
}

impl SegmentFit {
    /// What the fit multiplied the segment's length by.
    pub fn scale(&self) -> f64 {
        if self.source > 1e-9 {
            self.fitted / self.source
        } else {
            1.0
        }
    }
}

/// One mapped joint.
#[derive(Clone, Debug, PartialEq)]
pub struct JointFit {
    pub joint: String,
    pub target: String,
    /// How far the fitted joint stands from its target joint, in metres.
    pub residual: f64,
    pub segments: Vec<SegmentFit>,
}

/// A mapped joint with several mapped joints below it (a pelvis, a hand): how its fitted frame relates to the target's.
/// The frame's directions are the joint's turn applied to the source's child directions, so the relation holds even
/// where the children's positions were placed elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameFit {
    pub joint: String,
    pub children: usize,
    /// The angle of the rotation that would take the fitted frame's child directions onto the target's, in degrees
    /// (the worst child's angle when the directions fix no rotation).
    pub turn_deg: f64,
    /// Of that rotation, the twist about the target's bone into the joint from its mapped parent: a hand's palm roll
    /// against the forearm. `None` for a joint with no mapped parent or no rotation.
    pub roll_deg: Option<f64>,
    /// The largest angle between a child's fitted direction and the target's, in degrees.
    pub worst_child_deg: f64,
}

/// One IK limb of [`Fit::Lengths`].
#[derive(Clone, Debug, PartialEq)]
pub struct ReachFit {
    pub root: String,
    pub mid: String,
    pub end: String,
    /// The limb's length: its two bones.
    pub length: f64,
    /// The distance from the fitted root to the target's end joint.
    pub wanted: f64,
    /// How far the fitted end stands from the target's end joint (0 where the limb reaches it).
    pub end_residual: f64,
    /// How far the fitted mid joint stands from the target's.
    pub mid_residual: f64,
}

/// The vertices whose heaviest influence is one source joint (a vertex group).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupFit {
    pub joint: String,
    pub vertices: u64,
    /// The largest distance a vertex of the group moved, in metres.
    pub largest_displacement: f64,
    pub mean_displacement: f64,
}

/// The worst growth of a mesh edge, or the widest split of vertices that stood together.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    /// The two vertices' heaviest joints (source names); equal for a gap inside one group.
    pub between: (String, String),
    /// Metres: how much longer the edge is after the fit (or how far apart the coincident vertices are).
    pub metres: f64,
}

/// What a fit did, measured on the rest poses and the prototypes' vertices. See [`crate::RetargetSkin::report`].
#[derive(Clone, Debug, PartialEq)]
pub struct FitReport {
    pub joints: Vec<JointFit>,
    pub frames: Vec<FrameFit>,
    pub reaches: Vec<ReachFit>,
    pub groups: Vec<GroupFit>,
    /// The edge that grew most (triangle meshes only).
    pub largest_edge_gap: Option<Gap>,
    /// Vertices at one place in the source (to 10 micrometres) that the fit moved apart most: the seams of UV and
    /// material splits.
    pub largest_seam_gap: Option<Gap>,
}

impl FitReport {
    /// The report as lines, each with the number of vertices it concerns (1 for a joint-level line). The pass logs
    /// these as approximations; `Display` prints them.
    pub fn lines(&self) -> Vec<(String, u64)> {
        let mm = |m: f64| m * 1000.0;
        let mut out = Vec::new();
        for j in &self.joints {
            if j.residual > 0.001 {
                out.push((
                    format!(
                        "fit: joint {} stands {:.1} mm from its target joint {}",
                        j.joint,
                        mm(j.residual),
                        j.target
                    ),
                    1,
                ));
            }
            for s in &j.segments {
                out.push((
                    format!(
                        "fit: segment {} to {} of {:.1} mm (target {:.1} mm) fitted at {:.1} mm, scale {:.3}",
                        j.joint,
                        s.to,
                        mm(s.source),
                        mm(s.target),
                        mm(s.fitted),
                        s.scale()
                    ),
                    1,
                ));
            }
        }
        for f in &self.frames {
            let roll = f.roll_deg.map_or_else(
                || "no roll".to_owned(),
                |r| format!("roll about the incoming bone {r:.1} degrees"),
            );
            out.push((
                format!(
                    "fit: frame of {} ({} children) off the target's by {:.1} degrees, {roll}, worst child {:.1} degrees",
                    f.joint, f.children, f.turn_deg, f.worst_child_deg
                ),
                1,
            ));
        }
        for r in &self.reaches {
            out.push((
                format!(
                    "fit: limb {}:{}:{} of {:.1} mm to a target {:.1} mm away ends {:.1} mm short (mid joint {:.1} mm from its target)",
                    r.root,
                    r.mid,
                    r.end,
                    mm(r.length),
                    mm(r.wanted),
                    mm(r.end_residual),
                    mm(r.mid_residual)
                ),
                1,
            ));
        }
        for g in &self.groups {
            out.push((
                format!(
                    "fit: vertex group {} moved at most {:.1} mm (mean {:.1} mm)",
                    g.joint,
                    mm(g.largest_displacement),
                    mm(g.mean_displacement)
                ),
                g.vertices,
            ));
        }
        for (what, gap) in [
            ("an edge grew", &self.largest_edge_gap),
            ("vertices that stood together were parted", &self.largest_seam_gap),
        ] {
            if let Some(g) = gap {
                out.push((
                    format!(
                        "fit: largest gap, where {what} {:.1} mm, between groups {} and {}",
                        mm(g.metres),
                        g.between.0,
                        g.between.1
                    ),
                    1,
                ));
            }
        }
        out
    }
}

impl std::fmt::Display for FitReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (line, n) in self.lines() {
            if n > 1 {
                writeln!(f, "{line} ({n} vertices)")?;
            } else {
                writeln!(f, "{line}")?;
            }
        }
        Ok(())
    }
}

/// The angle in degrees of a unit quaternion, and its twist about `axis`.
fn angle_and_twist(q: [f64; 4], axis: Option<V3>) -> (f64, Option<f64>) {
    let angle = 2.0 * q[3].abs().min(1.0).acos().to_degrees();
    let twist = axis.map(|a| {
        let along = dot([q[0], q[1], q[2]], a);
        let t = 2.0 * along.atan2(q[3]).to_degrees();
        // into (-180, 180]
        if t > 180.0 {
            t - 360.0
        } else if t <= -180.0 {
            t + 360.0
        } else {
            t
        }
    });
    (angle, twist)
}

/// The joint-level report of `fitted` (the rest-pose measures; the vertices' are added by [`Measure`]).
pub(crate) fn joint_report(
    rig: &Rig<'_>,
    source_rest: &BTreeMap<JointId, Affine>,
    target_rest: &BTreeMap<JointId, Affine>,
    target: &Skeleton,
    fitted: &BTreeMap<JointId, Affine>,
    limbs: &[[JointId; 3]],
) -> FitReport {
    let sname = |j: JointId| name_of(rig.source, j);
    let tname = |j: JointId| name_of(target, j);
    let spos = |j: JointId| source_rest[&j][3];
    let tpos = |j: JointId| target_rest[&rig.pairs[&j]][3];
    let fpos = |j: JointId| fitted[&j][3];
    let mut joints = Vec::new();
    let mut frames = Vec::new();
    for j in rig.source.joints.iter().filter(|j| rig.pairs.contains_key(&j.id)) {
        let id = j.id;
        let kids = rig.below.get(&id).map_or(&[][..], Vec::as_slice);
        joints.push(JointFit {
            joint: sname(id),
            target: tname(rig.pairs[&id]),
            residual: dist(fpos(id), tpos(id)),
            segments: kids
                .iter()
                .map(|&c| SegmentFit {
                    to: sname(c),
                    source: dist(spos(c), spos(id)),
                    target: dist(tpos(c), tpos(id)),
                    fitted: dist(fpos(c), fpos(id)),
                })
                .collect(),
        });
        if kids.len() < 2 {
            continue;
        }
        // the frame's directions: the source's, turned by the joint's own turn
        let own = quat::mul(rotation(&fitted[&id]), quat::inverse(rotation(&source_rest[&id])));
        let seen: Vec<V3> = kids
            .iter()
            .map(|&c| rotate(own, sub(spos(c), spos(id))))
            .collect();
        let want: Vec<V3> = kids.iter().map(|&c| sub(tpos(c), tpos(id))).collect();
        let worst_child_deg = seen
            .iter()
            .zip(&want)
            .filter_map(|(&s, &w)| Some(dot(unit(s)?, unit(w)?).clamp(-1.0, 1.0).acos().to_degrees()))
            .fold(0.0, f64::max);
        let bone = rig.above.get(&id).and_then(|&p| unit(sub(tpos(id), tpos(p))));
        let (turn_deg, roll_deg) = match best_turn(&seen, &want) {
            Some(q) => {
                let (a, t) = angle_and_twist(q, bone);
                (a, t)
            }
            None => (worst_child_deg, None),
        };
        frames.push(FrameFit {
            joint: sname(id),
            children: kids.len(),
            turn_deg,
            roll_deg,
            worst_child_deg,
        });
    }
    let reaches = limbs
        .iter()
        .map(|&[root, mid, end]| ReachFit {
            root: sname(root),
            mid: sname(mid),
            end: sname(end),
            length: dist(fpos(mid), fpos(root)) + dist(fpos(end), fpos(mid)),
            wanted: dist(tpos(end), fpos(root)),
            end_residual: dist(fpos(end), tpos(end)),
            mid_residual: dist(fpos(mid), tpos(mid)),
        })
        .collect();
    FitReport {
        joints,
        frames,
        reaches,
        groups: Vec::new(),
        largest_edge_gap: None,
        largest_seam_gap: None,
    }
}

pub(crate) fn name_of(s: &Skeleton, j: JointId) -> String {
    s.joints
        .iter()
        .find(|x| x.id == j)
        .map_or_else(|| format!("{j:?}"), |x| x.name.clone())
}

/// The vertex-level measures of a fit, gathered over primitives.
#[derive(Default)]
pub(crate) struct Measure {
    groups: BTreeMap<JointId, (u64, f64, f64)>,
    edge: Option<(f64, JointId, JointId)>,
    seam: Option<(f64, JointId, JointId)>,
}

impl Measure {
    /// One primitive: its vertices `before` and `after` the fit, and each vertex's joints (source ids) and weights.
    pub fn observe(
        &mut self,
        p: &Primitive,
        before: &[[f32; 3]],
        after: &[[f32; 3]],
        named: &[Vec<(JointId, f64)>],
    ) {
        let heaviest: Vec<Option<JointId>> = named
            .iter()
            .map(|v| {
                v.iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
                    .map(|x| x.0)
            })
            .collect();
        let at = |v: &[[f32; 3]], i: usize| v[i].map(f64::from);
        for (i, h) in heaviest.iter().enumerate() {
            if let Some(h) = h {
                let d = dist(at(before, i), at(after, i));
                let g = self.groups.entry(*h).or_default();
                g.0 += 1;
                g.1 = g.1.max(d);
                g.2 += d;
            }
        }
        let worse = |slot: &mut Option<(f64, JointId, JointId)>, g: f64, a: usize, b: usize| {
            if let (Some(ja), Some(jb)) = (heaviest[a], heaviest[b])
                && slot.is_none_or(|s| g > s.0)
            {
                *slot = Some((g, ja, jb));
            }
        };
        if p.topology == Topology::Triangles {
            let idx: Vec<usize> = match &p.indices {
                Some(v) => v.iter().map(|&x| x as usize).collect(),
                None => (0..p.vertex_count()).collect(),
            };
            let mut edge = self.edge;
            for t in idx.as_chunks::<3>().0 {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    if a < before.len() && b < before.len() {
                        let growth = dist(at(after, a), at(after, b)) - dist(at(before, a), at(before, b));
                        worse(&mut edge, growth, a, b);
                    }
                }
            }
            self.edge = edge;
        }
        // vertices at one place before the fit, to 10 micrometres
        let mut seen: BTreeMap<[i64; 3], usize> = BTreeMap::new();
        let mut seam = self.seam;
        for i in 0..before.len() {
            let key = at(before, i).map(|x| (x * 1e5).round() as i64);
            match seen.get(&key) {
                Some(&first) => worse(&mut seam, dist(at(after, first), at(after, i)), first, i),
                None => {
                    seen.insert(key, i);
                }
            }
        }
        self.seam = seam;
    }

    /// The report with this measure's parts filled in.
    pub fn finish(self, skeleton: &Skeleton, mut report: FitReport) -> FitReport {
        report.groups = self
            .groups
            .into_iter()
            .map(|(j, (n, max, sum))| GroupFit {
                joint: name_of(skeleton, j),
                vertices: n,
                largest_displacement: max,
                mean_displacement: sum / n as f64,
            })
            .collect();
        let gap = |g: Option<(f64, JointId, JointId)>| {
            g.map(|(m, a, b)| Gap {
                between: (name_of(skeleton, a), name_of(skeleton, b)),
                metres: m,
            })
        };
        report.largest_edge_gap = gap(self.edge);
        report.largest_seam_gap = gap(self.seam);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::inverse;
    use crate::retarget_skin::{Placement, RetargetSkin, UnmappedJoints};
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{
        Binding, BindingId, Deformation, Entity, Influences, Joint, JointMap, Mesh, Rest, Semantic, Skin,
        Slot, Stream, World,
    };

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn near3(a: V3, b: V3, tol: f64) -> bool {
        dist(a, b) <= tol
    }

    /// A skeleton of joints given by id, parent and model position (no rotations).
    fn skeleton(joints: &[(u32, Option<u32>, V3)]) -> Skeleton {
        let at = |id: u32| joints.iter().find(|j| j.0 == id).map(|j| j.2);
        Skeleton {
            joints: joints
                .iter()
                .map(|&(id, parent, p)| {
                    let base = parent.and_then(at).unwrap_or([0.0; 3]);
                    Joint {
                        id: JointId(id),
                        name: format!("j{id}"),
                        parent: parent.map(JointId),
                        rest: Rest::Trs {
                            translation: [0, 1, 2].map(|k| (p[k] - base[k]) as f32),
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0; 3],
                        },
                    }
                })
                .collect(),
        }
    }

    /// A quarter turn about +x.
    fn quarter_x(v: V3) -> V3 {
        [v[0], -v[2], v[1]]
    }

    /// The arm fixture: joints 1 (root), 2 (upper), 3 (forearm), 4 (hand), and finger roots 5, 6, 7. The source arm is
    /// straight along +x (upper 0.3 long, forearm 0.25); the target has the given elbow and wrist, and its fingers a
    /// quarter turn about the arm when `roll`. The mesh is a ribbon of triangles along the arm, every station weighted
    /// wholly to one joint (the strictest case for a gap).
    fn arm(elbow: V3, wrist: V3, roll: bool) -> (World, Entity) {
        const FINGERS: [V3; 3] = [[0.03, 0.0, 0.03], [0.08, 0.0, 0.02], [0.07, 0.0, -0.03]];
        let root = [0.0, 1.4, 0.0];
        let up = add(root, [0.1, 0.0, 0.0]);
        let fore = add(up, [0.3, 0.0, 0.0]);
        let hand = add(up, [0.55, 0.0, 0.0]);
        let mut src = vec![
            (1, None, root),
            (2, Some(1), up),
            (3, Some(2), fore),
            (4, Some(3), hand),
        ];
        let mut tgt = vec![
            (11, None, root),
            (12, Some(11), up),
            (13, Some(12), elbow),
            (14, Some(13), wrist),
        ];
        for (k, f) in FINGERS.iter().enumerate() {
            src.push((5 + k as u32, Some(4), add(hand, *f)));
            tgt.push((
                15 + k as u32,
                Some(14),
                add(wrist, if roll { quarter_x(*f) } else { *f }),
            ));
        }
        let mut w = World::new();
        let (source, target) = (w.spawn(), w.spawn());
        w.insert(source, skeleton(&src));
        w.insert(target, skeleton(&tgt));
        let m = w.spawn();
        w.insert(
            m,
            JointMap {
                from: source,
                to: target,
                pairs: (0..7).map(|k| (JointId(1 + k), JointId(11 + k))).collect(),
            },
        );
        // stations along the arm: x and the palette slot (upper, forearm, hand)
        let stations: [(f32, u8); 9] = [
            (0.15, 0),
            (0.25, 0),
            (0.35, 0),
            (0.45, 1),
            (0.55, 1),
            (0.64, 1),
            (0.65, 2),
            (0.70, 2),
            (0.75, 2),
        ];
        let mut positions = Vec::new();
        let (mut joints, mut weights) = (Vec::new(), Vec::new());
        for (x, j) in stations {
            for z in [0.02f32, -0.02] {
                positions.push([x, 1.4, z]);
                joints.push([j, 0, 0, 0]);
                weights.push([1.0f32, 0.0, 0.0, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..stations.len() as u32 - 1 {
            let a = 2 * i;
            indices.extend([a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![primase_core::Primitive {
                topology: Topology::Triangles,
                streams: [
                    (Semantic::Position, Stream::F32x3(positions)),
                    (Semantic::Joints(0), Stream::U8x4(joints)),
                    (Semantic::Weights(0), Stream::F32x4(weights)),
                ]
                .into(),
                indices: Some(indices),
                material: None,
            }]),
        );
        let slot = |j: u32, p: V3| Slot {
            joint: JointId(j),
            inverse_bind: {
                let mut t = primase_core::Transform::IDENTITY;
                t.cols[3] = p.map(|x| -(x as f32));
                t
            },
        };
        w.insert(
            proto,
            Skin {
                influences: vec![Some(Influences::Streams { sets: 1 })],
                weights: primase_core::WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton: source,
                    palettes: vec![Some(vec![slot(2, up), slot(3, fore), slot(4, hand)])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        assert_eq!(w.verify_components(), vec![]);
        (w, proto)
    }

    /// A short target arm (the wrist within the source arm's reach: 0.348 from the upper joint) bent at the elbow.
    fn short_arm(roll: bool) -> (World, Entity) {
        arm([0.273, 1.3, 0.0], [0.414, 1.249, 0.0], roll)
    }

    /// A long target arm: its wrist is 0.64 from the upper joint, past the source arm's 0.55.
    fn long_arm(roll: bool) -> (World, Entity) {
        arm([0.429, 1.2803, 0.0], [0.729, 1.2803, 0.0], roll)
    }

    const LIMB: &str = "j2:j3:j4";

    fn pass(fit: &str) -> RetargetSkin {
        RetargetSkin {
            unmapped: UnmappedJoints::Refuse,
            placement: Placement::Automatic,
            fit: if fit == "lengths" {
                Fit::Lengths {
                    ik: vec![
                        LIMB.split(':')
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap(),
                    ],
                    scale: 1.0,
                }
            } else {
                Fit::Positions
            },
        }
    }

    /// The reports of a pass over a world, one per map.
    fn reports(p: RetargetSkin, w: &World) -> Vec<FitReport> {
        p.report(w).unwrap().into_iter().map(|(_, r)| r).collect()
    }

    fn run(w: &mut World, pipeline: &str) -> RunReport {
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        let m = PassManager::new(reg).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(w, &m.registry.parse(pipeline).unwrap(), &mut report)
            .unwrap();
        report
    }

    fn lengths_pipeline() -> String {
        format!("retarget-skin{{fit=lengths,ik={LIMB},placement=automatic,unmapped=refuse}}")
    }

    fn positions(w: &World, p: Entity) -> Vec<V3> {
        w.get::<Mesh>(p).unwrap().primitives()[0]
            .positions()
            .unwrap()
            .iter()
            .map(|v| v.map(f64::from))
            .collect()
    }

    /// The edges of the fixture's ribbon: each station's two vertices and the diagonals to the next.
    fn edge_lengths(v: &[V3]) -> Vec<f64> {
        let mut out = Vec::new();
        for i in 0..v.len() / 2 - 1 {
            let a = 2 * i;
            out.extend([
                dist(v[a], v[a + 1]),
                dist(v[a], v[a + 2]),
                dist(v[a + 1], v[a + 2]),
                dist(v[a + 1], v[a + 3]),
            ]);
        }
        out
    }

    fn segment(r: &FitReport, from: &str, to: &str) -> SegmentFit {
        r.joints
            .iter()
            .find(|j| j.joint == from)
            .and_then(|j| j.segments.iter().find(|s| s.to == to))
            .unwrap()
            .clone()
    }

    /// `fit=lengths` keeps every segment's length, for an arm shorter and an arm longer than the source's, and no mesh
    /// edge changes length: the wrist stays connected (the report's largest gap and the mesh itself agree). `positions`
    /// gives the forearm the target's length and opens the wrist by the difference.
    #[test]
    fn lengths_keep_every_segment_and_the_wrist_stays_connected() {
        for (name, build, forearm) in [
            (
                "short",
                short_arm as fn(bool) -> (World, Entity),
                (0.141f64.powi(2) + 0.051f64.powi(2)).sqrt(),
            ),
            ("long", long_arm, 0.3),
        ] {
            let (mut w, p) = build(false);
            let before = positions(&w, p);
            let positions_report = reports(pass("positions"), &w);
            let r = reports(pass("lengths"), &w);
            for j in &r[0].joints {
                for s in &j.segments {
                    assert!(
                        near(s.scale(), 1.0, 1e-6),
                        "{name}: {} to {} scaled {}",
                        j.joint,
                        s.to,
                        s.scale()
                    );
                }
            }
            // the wrist is bent but not opened: no edge grew by a millimetre
            let gap = r[0].largest_edge_gap.as_ref().unwrap().metres;
            assert!(gap < 1e-3, "{name}: lengths opened a gap of {gap}");
            // the report against the mesh itself: the same growth, measured on the pass's own output
            run(&mut w, &lengths_pipeline());
            let growth = edge_lengths(&positions(&w, p))
                .iter()
                .zip(edge_lengths(&before))
                .map(|(after, before)| after - before)
                .fold(f64::MIN, f64::max);
            assert!(
                near(growth, gap, 1e-6),
                "{name}: the report says {gap}, the mesh {growth}"
            );
            // positions: the forearm takes the target's length, the mesh does not follow
            let s = segment(&positions_report[0], "j3", "j4");
            assert!(near(s.fitted, forearm, 1e-3), "{name}: {s:?}");
            let opened = positions_report[0].largest_edge_gap.as_ref().unwrap().metres;
            assert!(
                opened > 0.01 || name == "short",
                "{name}: positions left a gap of only {opened}"
            );
        }
    }

    /// The IK limb reaches the target's wrist exactly when the source arm's lengths allow, bending toward the target's
    /// elbow; when the target is farther than the arm reaches it is left straight toward it and the shortfall is
    /// reported.
    #[test]
    fn two_bone_ik_reaches_or_reports_the_shortfall() {
        let (w, _) = short_arm(false);
        let r = reports(pass("lengths"), &w);
        let l = &r[0].reaches[0];
        assert!(l.end_residual < 1e-6 && near(l.length, 0.55, 1e-6), "{l:?}");
        // the target's elbow is below the line from shoulder to wrist and the fitted elbow bends the same way: 0.199 m
        // from it (the source's longer bones cannot meet it), where the mirrored bend would be 0.28 m away
        assert!(l.mid_residual > 0.19 && l.mid_residual < 0.21, "{l:?}");
        let (w, _) = long_arm(false);
        let r = reports(pass("lengths"), &w);
        let l = &r[0].reaches[0];
        let wanted = dist([0.729, 1.2803, 0.0], [0.1, 1.4, 0.0]);
        assert!(near(l.wanted, wanted, 1e-5), "{l:?}");
        assert!(near(l.end_residual, wanted - 0.55, 1e-5), "{l:?}");
        // the joint table: the wrist is on the line to the target, a full arm from the upper joint
        let j = r[0].joints.iter().find(|j| j.joint == "j4").unwrap();
        assert!(near(j.residual, wanted - 0.55, 1e-5), "{j:?}");
    }

    /// The hand is one rigid body whose roll follows its fingers' target directions: against a target whose fingers are
    /// a quarter turn about the arm, `positions` leaves the palm a quarter turn off (it only places the finger roots)
    /// and `lengths` has no turn left.
    #[test]
    fn the_palm_roll_follows_the_target_hand() {
        let (w, _) = short_arm(true);
        let hand = |fit: &str| {
            let r = reports(pass(fit), &w);
            r[0].frames.iter().find(|f| f.joint == "j4").unwrap().clone()
        };
        let l = hand("lengths");
        assert!(l.turn_deg < 1e-3 && l.worst_child_deg < 1e-3, "{l:?}");
        let p = hand("positions");
        // (the hand also inherits the forearm's bend, which is not a roll)
        assert!(p.turn_deg > 90.0 && p.turn_deg < 95.0, "{p:?}");
        assert!(near(p.roll_deg.unwrap().abs(), 90.0, 0.5), "{p:?}");
        // the fingers' roots keep their distance from the wrist in the mesh's fitted skeleton
        let r = reports(pass("lengths"), &w);
        for s in &r[0].joints.iter().find(|j| j.joint == "j4").unwrap().segments {
            assert!(near(s.scale(), 1.0, 1e-9), "{s:?}");
        }
    }

    /// Horn's solution recovers a known rotation from directions, and gives none where they fix none.
    #[test]
    fn the_best_turn_recovers_a_known_rotation() {
        let q = quat::normalize([0.3, -0.5, 0.2, 0.8]).unwrap();
        let from = [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.2],
            [0.3, -0.4, 1.0],
            [0.0, 0.0, 0.0],
        ];
        let to: Vec<V3> = from.iter().map(|&v| rotate(q, v)).collect();
        let got = best_turn(&from, &to).unwrap();
        for v in &from[..3] {
            assert!(near3(rotate(got, *v), rotate(q, *v), 1e-9));
        }
        assert!(
            best_turn(
                &[[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
                &[[0.0, 1.0, 0.0], [0.0, 3.0, 0.0]]
            )
            .is_none()
        );
        assert!(best_turn(&from[..1], &to[..1]).is_none());
    }

    /// The two-bone solution keeps both lengths, bends toward the pole, and clamps out of reach.
    #[test]
    fn two_bone_keeps_both_lengths() {
        let root = [0.0, 0.0, 0.0];
        let (mid, end) = two_bone(
            root,
            0.3,
            0.25,
            [0.4, 0.2, 0.0],
            [0.2, -1.0, 0.0],
            [0.0, 0.0, 1.0],
        );
        assert!(near(dist(root, mid), 0.3, 1e-12) && near(dist(mid, end), 0.25, 1e-12));
        assert!(near3(end, [0.4, 0.2, 0.0], 1e-12));
        assert!(mid[1] < 0.0 && mid[2].abs() < 1e-12, "{mid:?}");
        let (mid, end) = two_bone(root, 0.3, 0.25, [2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(near3(mid, [0.3, 0.0, 0.0], 1e-12) && near3(end, [0.55, 0.0, 0.0], 1e-12));
        // folded: the target nearer than |a - b|
        let (mid, end) = two_bone(root, 0.3, 0.1, [0.05, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(near(dist(root, mid), 0.3, 1e-12) && near(dist(mid, end), 0.1, 1e-12));
        assert!(near(dist(root, end), 0.2, 1e-12), "{end:?}");
    }

    /// The vertex groups: the hand's stations move rigidly with the hand joint, by the distance it moved.
    #[test]
    fn vertex_groups_report_their_largest_displacement() {
        let (w, _) = long_arm(false);
        let r = reports(pass("positions"), &w);
        let g = r[0].groups.iter().find(|g| g.joint == "j4").unwrap();
        let d = dist([0.65, 1.4, 0.0], [0.729, 1.2803, 0.0]);
        assert_eq!(g.vertices, 6);
        assert!(
            near(g.largest_displacement, d, 1e-5) && near(g.mean_displacement, d, 1e-5),
            "{g:?} {d}"
        );
    }

    /// Whatever the rests' rotations, a mapped joint with one mapped child has its bone along the segment the fit left:
    /// its own offset to the child, turned by its fitted rotation, points from its fitted position to the child's. (The
    /// source's rests here are rotated, so a fit that applied a joint's offset in the wrong space fails.)
    #[test]
    fn bones_point_along_their_segments_with_rotated_rests() {
        let q = |axis: usize, deg: f64| {
            let (s, c) = ((deg.to_radians() / 2.0).sin(), (deg.to_radians() / 2.0).cos());
            let mut v = [0.0f32, 0.0, 0.0, c as f32];
            v[axis] = s as f32;
            v
        };
        let joint = |id: u32, parent: Option<u32>, t: [f32; 3], rotation: [f32; 4]| Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest: Rest::Trs {
                translation: t,
                rotation,
                scale: [1.0; 3],
            },
        };
        let id = [0.0, 0.0, 0.0, 1.0];
        let source = Skeleton {
            joints: vec![
                joint(1, None, [0.0, 1.0, 0.0], id),
                joint(2, Some(1), [0.1, 0.0, 0.0], q(2, 90.0)),
                joint(3, Some(2), [0.3, 0.0, 0.0], q(1, 90.0)),
                joint(4, Some(3), [0.25, 0.0, 0.0], q(0, 30.0)),
            ],
        };
        let target = skeleton(&[
            (11, None, [0.0, 1.0, 0.0]),
            (12, Some(11), [0.1, 1.0, 0.0]),
            (13, Some(12), [0.3, 0.8, 0.0]),
            (14, Some(13), [0.5, 0.7, 0.1]),
        ]);
        let pairs: BTreeMap<JointId, JointId> = (1..=4).map(|k| (JointId(k), JointId(10 + k))).collect();
        let rests = |s: &Skeleton| crate::pose::model_transforms(s, &crate::pose::locals(s, None)).unwrap();
        let (srest, trest) = (rests(&source), rests(&target));
        let local = crate::pose::locals(&source, None)
            .into_iter()
            .map(|(j, r)| (j, crate::pose::affine(&r.matrix())))
            .collect();
        let rig = Rig::new(&source, &pairs, local);
        let limb = rig.limb(&["j2".into(), "j3".into(), "j4".into()]).unwrap();
        for (name, fitted) in [
            (
                "positions",
                crate::placement::fit(&source, &trest, &pairs).unwrap(),
            ),
            (
                "lengths",
                fit_lengths(&rig, &srest, &trest, &[limb], 1.0).unwrap(),
            ),
            (
                "lengths, scaled",
                fit_lengths(&rig, &srest, &trest, &[limb], 0.8).unwrap(),
            ),
        ] {
            for (a, b) in [(1, 2), (2, 3), (3, 4)] {
                let (a, b) = (JointId(a), JointId(b));
                let own = inverse(&srest[&a]).unwrap();
                let bone = rotate(rotation(&fitted[&a]), compose(&own, &srest[&b])[3]);
                let along = sub(fitted[&b][3], fitted[&a][3]);
                let cos = dot(unit(bone).unwrap(), unit(along).unwrap());
                assert!(
                    cos > 1.0 - 1e-9,
                    "{name}: bone {a:?} to {b:?} is off its segment (cos {cos})"
                );
            }
        }
    }

    /// `scale` sizes the whole source by one factor: every segment is that much of its length (the report says so), and
    /// the mesh follows.
    #[test]
    fn scale_sizes_the_whole_source() {
        let (mut w, p) = short_arm(false);
        let before = positions(&w, p);
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        let o =
            format!("retarget-skin{{fit=lengths,ik={LIMB},placement=automatic,scale=0.9,unmapped=refuse}}");
        let report = run(&mut w, &o);
        let log = &report.steps[0].log;
        assert!(
            log.approximations
                .keys()
                .any(|k| k.contains("fit: segment j3 to j4 of 250.0 mm")
                    && k.contains("fitted at 225.0 mm, scale 0.900"))
        );
        // the hand's stations moved with a hand 0.9 as long as the forearm: edges within a segment are 0.9 of theirs
        let after = positions(&w, p);
        let (a, b) = (edge_lengths(&before), edge_lengths(&after));
        assert!(
            near(b[4 * 6], 0.9 * a[4 * 6], 1e-6),
            "{} vs {}",
            b[4 * 6],
            a[4 * 6]
        );
    }

    /// The pass logs the report by name, and a run leaves the mesh as the report measured it.
    #[test]
    fn the_pass_logs_the_report() {
        let (mut w, _) = long_arm(false);
        let report = run(&mut w, &lengths_pipeline());
        let log = &report.steps[0].log;
        let has = |what: &str| log.approximations.keys().any(|k| k.contains(what));
        assert!(has(
            "fit: segment j3 to j4 of 250.0 mm (target 300.0 mm) fitted at 250.0 mm, scale 1.000"
        ));
        assert!(has("fit: limb j2:j3:j4 of 550.0 mm"));
        assert!(has(
            "fit: frame of j4 (3 children) off the target's by 0.0 degrees"
        ));
        assert!(has("fit: vertex group j4 moved at most"));
        assert!(has("fit: largest gap, where an edge grew 0.0 mm"));
    }

    /// The options: the fit is required, `ik` goes with `lengths` only and names a chain of single mapped children.
    #[test]
    fn the_options_refuse_what_they_were_not_told() {
        let (w, _) = short_arm(false);
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        let err = |p: &str| reg.parse(p).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err("retarget-skin{unmapped=refuse,placement=automatic}").contains("fit is required: "));
        assert!(err("retarget-skin{fit=lengths,unmapped=refuse,placement=automatic}").contains("needs ik="));
        assert!(
            err("retarget-skin{fit=positions,ik=none,unmapped=refuse,placement=automatic}")
                .contains("ik applies to fit=lengths")
        );
        assert!(
            err("retarget-skin{fit=lengths,ik=a:b,unmapped=refuse,placement=automatic}")
                .contains("root:mid:end")
        );
        assert!(
            err("retarget-skin{fit=stretch,unmapped=refuse,placement=automatic}")
                .contains("positions or lengths")
        );
        assert!(
            err("retarget-skin{fit=positions,scale=2,unmapped=refuse,placement=automatic}")
                .contains("scale applies to fit=lengths")
        );
        assert!(
            err("retarget-skin{fit=lengths,ik=none,scale=0,unmapped=refuse,placement=automatic}")
                .contains("a number above 0")
        );
        let bad = |ik: &str| {
            RetargetSkin {
                unmapped: UnmappedJoints::Refuse,
                placement: Placement::Automatic,
                fit: Fit::Lengths {
                    ik: vec![
                        ik.split(':')
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap(),
                    ],
                    scale: 1.0,
                },
            }
            .report(&w)
            .unwrap_err()
        };
        assert!(bad("j2:j3:nowhere").contains("nowhere is not exactly one joint"));
        assert!(
            bad("j3:j4:j5")
                .contains("is not a chain of single mapped children (j3 has 1 mapped children, j4 has 3)")
        );
        // the options read back
        let p = reg
            .parse("retarget-skin{fit=lengths,ik=j2:j3:j4;j1:j2:j3,placement=automatic,unmapped=nearest}")
            .unwrap();
        assert_eq!(p.passes[0].options()["ik"], "j2:j3:j4;j1:j2:j3");
        assert_eq!(p.passes[0].options()["fit"], "lengths");
        assert_eq!(p.passes[0].options()["scale"], "1");
        let p = reg
            .parse("retarget-skin{fit=lengths,ik=none,placement=automatic,unmapped=nearest}")
            .unwrap();
        assert_eq!(p.passes[0].options()["ik"], "none");
    }
}
