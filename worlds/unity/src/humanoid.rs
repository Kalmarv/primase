//! Mecanim's humanoid muscles as bone rotations: what a clip's muscle curves (`Spine Front-Back`, ...) make of the
//! bones of an avatar built from a model's skeleton. Evaluated from the avatar's own compiled data, the pre- and
//! post-rotation, sign and limits of each human bone (`Axes`) and the twist shares of the limbs.
//!
//! **The model**, found by fitting Unity itself (6000.6.4f1 rebuilding the frog avatar of Sledding Game from its stored
//! `HumanDescription`, whose compiled axes it reproduces to 1e-7: `primase-oracles/unity/mecanim`) and equal to it to
//! 6e-5 degrees on every bone of 300 random muscle poses and on poses with other twist shares:
//!
//! - A human bone has up to three degrees of freedom, each driven by one muscle `v` (a value of -1 to 1, beyond it
//!   overshooting the range): its angle is `sgn * (v * max)` for `v >= 0` and `sgn * (-v * min)` below, `min` and `max`
//!   the bone's limits (radians, `min` negative) and `sgn` its sign, per axis. The axes are x (twist), y and z.
//! - The rotation inside the bone is `T(y, z) * Rx(x)`: a twist about x, then a swing `T` whose quaternion is
//!   `normalize(0, tan(y/2), tan(z/2), 1)` (a single axis is the plain rotation; two together are not the product of
//!   two rotations nor one about the combined vector).
//! - The bone's local rotation is `pre * inner * post^-1`, with `pre` and `post` the axes' rotations (all in the
//!   avatar's skeleton frame).
//! - The twist of a limb is shared with the bone below it (upper arm to forearm to hand, upper leg to lower leg to
//!   foot): a bone keeps the share its `HumanDescription` states (arm, forearm, upper leg and leg twist) of its own x
//!   angle, and passes the rest to the child, which applies it as `post_parent * Rx(rest) * post_parent^-1` before its
//!   own rotation. The twist is the limb's total angle wrapped into (-pi, pi] first (the pose is periodic in the twist
//!   muscle, 360 degrees of total twist apart; at exactly 180 degrees Unity's sign is float noise): an upper arm's
//!   muscle at 2 and -2 turns it by +-90 degrees, not +-180 (the frog's upper arm in Unity 6000.6.4f1, muscle values -3
//!   to 3 read back through `HumanPoseHandler`: continuous, period 4, the sign flipping at +-2). The same wrap is applied
//!   to the forearm and the legs, whose twist is shared the same way; only the upper arm's is measured.
//!
//! **The hips** are placed by the clip's `RootT` and `RootQ`, the body's centre of mass (in units of the avatar's human
//! scale) and orientation (relative to the avatar's rest orientation, `m_RootX.q`), which the muscles' pose moves the
//! hips to: the hips' rotation is the one that turns the pose's body frame to the stated orientation, and the hips'
//! position the one that puts the pose's centre of mass at the stated place. Found by the same oracle
//! (`HumanPoseHandler.GetHumanPose` reporting the body position and rotation of a pose, and how they respond when one
//! bone is moved or turned), and checked against the avatars' own stored data:
//!
//! - The pose is the human skeleton's (the translations of the avatar's human pose, the muscles' rotations), the hips at
//!   the origin, unrotated, their own scale applied to the bones below.
//! - The body frame has its up axis from the middle of the upper legs to the middle of the upper arms, its lateral axis
//!   the sum of the right-minus-left vectors of the upper arms and of the upper legs, made perpendicular to the up axis,
//!   and its forward axis their cross product. At the avatar's stored pose this frame is `m_RootX.q` (frog and yeti, to
//!   the digits the file stores), the up axis is exact (the editor keeps it to 6e-5 degrees over 1,932 poses) and the
//!   lateral axis follows a bone turned 10 degrees from the rest pose to 2e-4 degrees. It is not the whole of the
//!   editor's frame: against the editor's hips the rotation is off by 0.05 degrees rms (0.4 at most) over 1,251 poses of
//!   the spine's muscles, 0.25 rms (3.2 at most) over 681 poses of every muscle at random on the frog and 0.6 rms (9.6
//!   at most, a pose with every muscle near its limit) on the yeti; what the editor adds beyond these four points was
//!   not found.
//! - The centre of mass is the mean of the bones' masses at their segments' centres, the centre of a bone being the mean
//!   of its origin and its children's (the hips': their children's alone; the head, hands and feet: their own, the toes
//!   and eyes being no children; the left lower arm's own origin is the left upper arm's, which the editor does on both
//!   avatars). The stored pose's centre of mass is `m_RootX.t` and its height is the human scale `m_Scale` (frog and
//!   yeti, to float precision), and the rule is exact to 1e-6 over the editor's poses on both. The centre of mass is
//!   `RootT` times the human scale.
//!
//! The skeleton of an avatar lacking optional bones (a chest, shoulders) is placed by the same rules over the bones it
//! has; no avatar read here lacks one, so that is unchecked.
//!
//! **What is not evaluated**: the IK goals and the fingers' muscles (no avatar read here has finger bones, so the
//! formula for them, the same, has no oracle). Each is counted by the lowering.

use primase_animation::quat::{self, from_columns, inverse, mul, normalize, rotate, slerp};
use primase_core::vector::{V3 as Vec3, add, cross, dot, scale as scaled, sub, unit_beyond};
use primase_core::{Channel, ClipTarget, Entity, Interpolation, JointId, Property, Rest};

use crate::animator::{AvatarAxes, AvatarHuman, UnityAvatar, Xform};
use crate::clipdata::{ClipData, Curve, Track};
use crate::components::UNITY;

/// How many muscles Unity has (`HumanTrait.MuscleCount`).
pub const MUSCLES: usize = 95;

/// The attribute of muscle 0 among a clip's Animator bindings: attributes 7 to 41 are the root (`RootT`, `RootQ`) and the
/// IK goals (feet and hands, translation and rotation); muscle `m` is `MUSCLE_ATTRIBUTE_BASE + m`. Unit-length rotations
/// confirm the layout on Sledding Game's clips.
pub const MUSCLE_ATTRIBUTE_BASE: u32 = 42;

/// The first muscles are the body's (to 54), then the left hand's 20 and the right hand's 20.
const BODY_MUSCLES: usize = 55;

/// The human bones of an avatar in the order its `m_HumanBoneIndex` lists them (`m_HumanBoneIndex[b]` is the node of
/// the human skeleton that bone is, `-1` for a bone the model lacks).
pub const BONES: usize = 25;

/// Which muscle drives each degree of freedom (x, y, z) of each of the 25 bones, `NONE` for none.
const NONE: u8 = u8::MAX;
#[rustfmt::skip]
const BONE_MUSCLES: [[u8; 3]; BONES] = [
    [NONE, NONE, NONE], // Hips
    [23, 22, 21], [31, 30, 29],             // left, right upper leg
    [25, NONE, 24], [33, NONE, 32],         // lower leg
    [NONE, 27, 26], [NONE, 35, 34],         // foot
    [2, 1, 0], [5, 4, 3], [8, 7, 6],        // spine, chest, upper chest
    [11, 10, 9], [14, 13, 12],              // neck, head
    [NONE, 38, 37], [NONE, 47, 46],         // shoulder
    [41, 40, 39], [50, 49, 48],             // upper arm
    [43, NONE, 42], [52, NONE, 51],         // lower arm
    [NONE, 45, 44], [NONE, 54, 53],         // hand
    [NONE, NONE, 28], [NONE, NONE, 36],     // toes
    [NONE, 16, 15], [NONE, 18, 17],         // left, right eye
    [NONE, 20, 19],                         // jaw
];

/// The bone below `bone` that shares its twist: upper arm to lower arm to hand, upper leg to lower leg to foot.
const TWIST_CHILD: [(usize, usize); 8] = [
    (14, 16),
    (15, 17),
    (16, 18),
    (17, 19),
    (1, 3),
    (2, 4),
    (3, 5),
    (4, 6),
];

/// The bone number of the body's hips and the names of the bones, for reports.
pub const BONE_NAMES: [&str; BONES] = [
    "Hips",
    "LeftUpperLeg",
    "RightUpperLeg",
    "LeftLowerLeg",
    "RightLowerLeg",
    "LeftFoot",
    "RightFoot",
    "Spine",
    "Chest",
    "UpperChest",
    "Neck",
    "Head",
    "LeftShoulder",
    "RightShoulder",
    "LeftUpperArm",
    "RightUpperArm",
    "LeftLowerArm",
    "RightLowerArm",
    "LeftHand",
    "RightHand",
    "LeftToes",
    "RightToes",
    "LeftEye",
    "RightEye",
    "Jaw",
];

type Quat = [f64; 4];

/// A stored rotation as a unit quaternion (the reader refuses one of no length).
fn f64q(q: [f32; 4]) -> Quat {
    quat::normalize(q.map(f64::from)).expect("the reader refuses a rotation of no length")
}

/// `angle` wrapped into (-pi, pi]: the shortest arc to the same rotation about an axis.
fn wrap_pi(angle: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    angle - TAU * ((angle - PI) / TAU).ceil()
}

fn rotation_x(angle: f64) -> Quat {
    [(angle / 2.0).sin(), 0.0, 0.0, (angle / 2.0).cos()]
}

/// The swing of a bone: `normalize(0, tan(y/2), tan(z/2), 1)`.
fn swing(y: f64, z: f64) -> Quat {
    let v = [0.0, (y / 2.0).tan(), (z / 2.0).tan(), 1.0];
    let n = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    v.map(|x| x / n)
}

/// The angle of degree of freedom `d` of a bone with `axes` at muscle value `v`.
fn angle(axes: &AvatarAxes, d: usize, v: f64) -> f64 {
    let (min, max) = (f64::from(axes.min[d]), f64::from(axes.max[d]));
    f64::from(axes.sgn[d]) * if v >= 0.0 { v * max } else { -v * min }
}

/// The axes of human bone `bone` in `human`, `None` for a bone the model lacks or one without axes.
fn axes_of(human: &AvatarHuman, bone: usize) -> Option<&AvatarAxes> {
    let node = usize::try_from(*human.bone_index.get(bone)?).ok()?;
    let axes = usize::try_from(human.skeleton.nodes.get(node)?.1).ok()?;
    human.skeleton.axes.get(axes)
}

/// The share of its own twist a bone keeps for the child below it, from the avatar's description: arm, forearm, upper
/// leg and leg twist; `1` for a bone that shares none.
fn twist_share(human: &AvatarHuman, bone: usize) -> f64 {
    f64::from(match bone {
        14 | 15 => human.arm_twist,
        16 | 17 => human.fore_arm_twist,
        1 | 2 => human.upper_leg_twist,
        3 | 4 => human.leg_twist,
        _ => return 1.0,
    })
}

/// The local rotation (in the avatar's skeleton frame, `[x, y, z, w]`) of human bone `bone` at `muscles`, `None` for a
/// bone the model lacks or one with no axes.
pub fn bone_rotation(human: &AvatarHuman, bone: usize, muscles: &[f64; MUSCLES]) -> Option<[f64; 4]> {
    let axes = axes_of(human, bone)?;
    let degrees = |axes: &AvatarAxes, bone: usize| -> [f64; 3] {
        std::array::from_fn(|d| match BONE_MUSCLES[bone][d] {
            NONE => 0.0,
            m => angle(axes, d, muscles[usize::from(m)]),
        })
    };
    let mut a = degrees(axes, bone);
    let shares_twist = TWIST_CHILD.iter().any(|(p, _)| *p == bone);
    let share = if shares_twist {
        a[0] = wrap_pi(a[0]);
        twist_share(human, bone)
    } else {
        1.0
    };
    let inner = mul(swing(a[1], a[2]), rotation_x(a[0] * share));
    let mut q = mul(mul(f64q(axes.pre_q), inner), inverse(f64q(axes.post_q)));
    // the rest of the parent bone's twist
    if let Some(&(parent, _)) = TWIST_CHILD.iter().find(|(_, c)| *c == bone)
        && let Some(parent_axes) = axes_of(human, parent)
    {
        let rest = wrap_pi(degrees(parent_axes, parent)[0]) * (1.0 - twist_share(human, parent));
        let post = f64q(parent_axes.post_q);
        q = mul(mul(mul(post, rotation_x(rest)), inverse(post)), q);
    }
    Some(q)
}

/// The angle in degrees between two rotations.
fn between(a: Quat, b: Quat) -> f64 {
    let d = mul(inverse(a), b);
    let s = d[..3].iter().map(|x| x * x).sum::<f64>().sqrt();
    (2.0 * s.atan2(d[3].abs())).to_degrees()
}

fn each(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// `a` of unit length where it is longer than 1e-12 (shorter, its direction is noise).
fn unit(a: Vec3) -> Option<Vec3> {
    unit_beyond(a, 1e-12)
}

/// A node of the human skeleton placed in the avatar's space: its rotation, its scale (per axis, as the xform stores it)
/// and its origin.
#[derive(Clone, Copy, Debug)]
struct Placed {
    rotation: Quat,
    scale: Vec3,
    origin: Vec3,
}

impl Placed {
    const ROOT: Placed = Placed {
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
        origin: [0.0; 3],
    };

    /// A child with local translation `t`, rotation `q` and scale `s`.
    fn child(&self, t: Vec3, q: Quat, s: Vec3) -> Placed {
        Placed {
            rotation: mul(self.rotation, q),
            scale: each(self.scale, s),
            origin: add(self.origin, rotate(self.rotation, each(self.scale, t))),
        }
    }
}

fn xform(x: &Xform) -> (Vec3, Quat, Vec3) {
    (x.t.map(f64::from), f64q(x.q), x.s.map(f64::from))
}

/// Every human bone's origin in a posed skeleton, `None` for a bone the model lacks.
type Origins = [Option<Vec3>; BONES];

/// The human bone below which a human bone sits in Mecanim's fixed hierarchy.
#[rustfmt::skip]
const PARENT_BONE: [Option<usize>; BONES] = [
    None,                                   // Hips
    Some(0), Some(0), Some(1), Some(2),     // upper legs, lower legs
    Some(3), Some(4),                       // feet
    Some(0), Some(7), Some(8), Some(9), Some(10), // spine, chest, upper chest, neck, head
    Some(9), Some(9),                       // shoulders
    Some(12), Some(13), Some(14), Some(15), // upper arms, lower arms
    Some(16), Some(17),                     // hands
    Some(5), Some(6),                       // toes
    Some(11), Some(11), Some(11),           // eyes, jaw
];

/// The bones that are no child of their parent for the centre of mass: the toes, eyes and jaw.
const EXTREMITIES: std::ops::RangeInclusive<usize> = 20..=24;

const LEFT_UPPER_ARM: usize = 14;
const LEFT_LOWER_ARM: usize = 16;

/// The human skeleton node of human bone `bone`.
fn node_of(human: &AvatarHuman, bone: usize) -> Option<usize> {
    usize::try_from(*human.bone_index.get(bone)?).ok()
}

/// The human skeleton posed at `muscles` in the hips' own frame: the hips at the origin, unrotated and with the scale
/// of their place in the skeleton, the human bones below them rotated by their muscles, every other node as the avatar's
/// pose has it. `None` for an avatar without a hips node or a pose for each node.
fn posed_origins(human: &AvatarHuman, muscles: &[f64; MUSCLES]) -> Option<Origins> {
    let nodes = &human.skeleton.nodes;
    if human.pose.len() != nodes.len() {
        return None;
    }
    let hips = node_of(human, 0).filter(|n| *n < nodes.len())?;
    // the hips' scale: their own composed with their ancestors'
    let mut scale = [1.0; 3];
    let mut at = Some(hips);
    while let Some(n) = at {
        scale = each(scale, human.pose[n].s.map(f64::from));
        at = usize::try_from(nodes[n].0)
            .ok()
            .filter(|p| *p < nodes.len() && *p != n);
    }
    let mut bone_of: Vec<Option<usize>> = vec![None; nodes.len()];
    for bone in 1..BONES {
        if let Some(n) = node_of(human, bone).filter(|n| *n < nodes.len()) {
            bone_of[n] = Some(bone);
        }
    }
    let mut placed: Vec<Option<Placed>> = vec![None; nodes.len()];
    placed[hips] = Some(Placed {
        rotation: Placed::ROOT.rotation,
        scale,
        origin: [0.0; 3],
    });
    loop {
        let mut changed = false;
        for n in 0..nodes.len() {
            let Some(parent) = usize::try_from(nodes[n].0).ok().filter(|p| *p < nodes.len()) else {
                continue;
            };
            if placed[n].is_some() {
                continue;
            }
            let Some(parent) = placed[parent] else {
                continue;
            };
            let (t, q, s) = xform(&human.pose[n]);
            let q = bone_of[n]
                .and_then(|b| bone_rotation(human, b, muscles))
                .unwrap_or(q);
            placed[n] = Some(parent.child(t, q, s));
            changed = true;
        }
        if !changed {
            break;
        }
    }
    Some(std::array::from_fn(|bone| {
        let n = node_of(human, bone).filter(|n| *n < nodes.len())?;
        placed[n].map(|p| p.origin)
    }))
}

/// The body frame of a posed skeleton: its lateral (right), up and forward axes. The up axis runs from the middle of the
/// upper legs to the middle of the upper arms; the lateral axis is the sum of the right-minus-left vectors of the upper
/// arms and of the upper legs, made perpendicular to the up axis; the forward axis is their cross product.
fn body_axes(o: &Origins) -> Option<[Vec3; 3]> {
    let [Some(left_leg), Some(right_leg), Some(left_arm), Some(right_arm)] = [o[1], o[2], o[14], o[15]]
    else {
        return None;
    };
    let up = unit(sub(
        scaled(add(left_arm, right_arm), 0.5),
        scaled(add(left_leg, right_leg), 0.5),
    ))?;
    let right = add(sub(right_arm, left_arm), sub(right_leg, left_leg));
    let right = unit(sub(right, scaled(up, dot(right, up))))?;
    Some([right, up, cross(right, up)])
}

/// The nearest present bone above `bone` in Mecanim's hierarchy.
fn present_parent(o: &Origins, bone: usize) -> Option<usize> {
    let mut at = PARENT_BONE[bone];
    while let Some(p) = at {
        if o[p].is_some() {
            return Some(p);
        }
        at = PARENT_BONE[p];
    }
    None
}

/// The centre of mass of a posed skeleton: each bone's mass at the mean of its origin and its children's, the hips' at
/// the mean of their children's, over the masses the avatar stores. `None` where a bone with mass is missing.
fn centre_of_mass(human: &AvatarHuman, o: &Origins) -> Option<Vec3> {
    let (mut sum, mut total) = ([0.0; 3], 0.0);
    for bone in 0..BONES {
        let mass = f64::from(*human.bone_mass.get(bone)?);
        if mass == 0.0 {
            continue;
        }
        let own = o[bone]?;
        let mut points: Vec<Vec3> = (1..BONES)
            .filter(|c| !EXTREMITIES.contains(c) && o[*c].is_some() && present_parent(o, *c) == Some(bone))
            .filter_map(|c| o[c])
            .collect();
        if bone != 0 {
            // the editor takes the left lower arm's origin from the left upper arm
            points.push(if bone == LEFT_LOWER_ARM {
                o[LEFT_UPPER_ARM]?
            } else {
                own
            });
        }
        let centre = scaled(
            points.iter().fold([0.0; 3], |a, p| add(a, *p)),
            1.0 / points.len() as f64,
        );
        sum = add(sum, scaled(centre, mass));
        total += mass;
    }
    (total > 0.0).then(|| scaled(sum, 1.0 / total))
}

/// Where the hips go: what places them for a body position and orientation, from an avatar's data.
#[derive(Debug)]
pub struct Hips<'a> {
    human: &'a AvatarHuman,
    /// The avatar's rest body orientation (`m_RootX.q`).
    rest: Quat,
    /// The hips' parent in the avatar's skeleton, in the avatar's space.
    parent: Placed,
}

impl<'a> Hips<'a> {
    /// The hips' placement for `avatar`'s humanoid `human`; `None` where the avatar lacks what it needs (a hips node, the
    /// pose of its human skeleton, the avatar skeleton node of the hips).
    pub fn new(avatar: &UnityAvatar, human: &'a AvatarHuman) -> Option<Hips<'a>> {
        let hips = node_of(human, 0)?;
        let node = usize::try_from(*avatar.human_skeleton_index.get(hips)?).ok()?;
        let nodes = &avatar.skeleton.nodes;
        if avatar.skeleton_pose.len() != nodes.len() {
            return None;
        }
        // the chain above the hips in the avatar's skeleton, from its root down
        let mut chain = Vec::new();
        let mut at = usize::try_from(nodes.get(node)?.0).ok();
        while let Some(n) = at {
            if chain.len() > nodes.len() || n >= nodes.len() {
                return None;
            }
            chain.push(n);
            at = usize::try_from(nodes[n].0).ok();
        }
        let parent = chain.iter().rev().fold(Placed::ROOT, |p, &n| {
            let (t, q, s) = xform(&avatar.skeleton_pose[n]);
            p.child(t, q, s)
        });
        Some(Hips {
            human,
            rest: f64q(human.root.q),
            parent,
        })
    }

    /// The body position (in units of the human scale) of the avatar's rest pose, `m_RootX.t` over the scale.
    pub fn rest_position(&self) -> Vec3 {
        scaled(
            self.human.root.t.map(f64::from),
            1.0 / f64::from(self.human.scale),
        )
    }

    /// The hips' local rotation and translation (in their parent's frame, Unity's) that give the skeleton posed at
    /// `muscles` the body position `body_position` and rotation `body_rotation` (a clip's `RootT` and `RootQ`).
    pub fn place(
        &self,
        muscles: &[f64; MUSCLES],
        body_position: Vec3,
        body_rotation: Quat,
    ) -> Option<(Quat, Vec3)> {
        let human = self.human;
        let origins = posed_origins(human, muscles)?;
        let local = from_columns(body_axes(&origins)?);
        // the body frame the pose must have: the stated rotation of the rest one
        let wanted = mul(normalize(body_rotation)?, self.rest);
        let rotation = mul(wanted, inverse(local));
        let centre = centre_of_mass(human, &origins)?;
        let origin = sub(
            scaled(body_position, f64::from(human.scale)),
            rotate(rotation, centre),
        );
        // into the parent's frame
        let back = inverse(self.parent.rotation);
        let translation = rotate(back, sub(origin, self.parent.origin));
        let translation = [0, 1, 2].map(|i| translation[i] / self.parent.scale[i]);
        Some((mul(back, rotation), translation))
    }
}

/// How far a lowered bone channel may be from the muscles' rotation between its keys, in degrees, and the shortest gap
/// between keys (seconds): keys are added by bisection until slerp between them is within the tolerance. The hips'
/// translation may be off by this fraction of the avatar's human scale.
const TOLERANCE_DEGREES: f64 = 0.01;
const TOLERANCE_TRANSLATION: f64 = 1e-4;
const SHORTEST_GAP: f64 = 1.0 / 480.0;

/// What lowering a humanoid clip did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct HumanoidLowering {
    /// Bones given a rotation channel (the hips included), and the keys of those channels (the hips' translation too).
    pub bones: usize,
    pub keys: usize,
    /// The largest angle, over the bones and the gaps between keys, between the muscles' rotation at a quarter, half
    /// and three quarters of a gap and the channel's slerp there (degrees); and the largest distance the hips'
    /// translation is off between its keys, as a fraction of the avatar's human scale.
    pub worst_between_keys_degrees: f64,
    pub worst_hips_between_keys: f64,
    /// Muscle curves the clip lacks (those muscles stay 0), RootT/RootQ curves the clip has, IK goal curves and finger
    /// muscle curves the clip has and nothing here applies.
    pub missing_muscles: usize,
    pub root_curves: usize,
    pub goal_curves: usize,
    pub finger_curves: usize,
    /// Whether the hips were placed (by the clip's `RootT` and `RootQ`), and how many of those seven curves the clip lacks
    /// (the body then keeps the avatar's rest position, or no rotation, in them).
    pub hips: bool,
    pub missing_root_curves: usize,
    /// Human bones of the avatar whose joint the rig lacks, by name.
    pub unresolved_bones: Vec<&'static str>,
}

/// `times` sampled by `evaluate`, with keys added by bisection between neighbours wherever `blend` of their values is
/// off `evaluate` by more than 1 in `miss` at a quarter, half or three quarters of the gap (a gap shorter than
/// [`SHORTEST_GAP`] is kept), and the largest miss left in the gaps kept.
fn bisect_keys<T: Copy>(
    times: &[f64],
    evaluate: &dyn Fn(f64) -> T,
    blend: &dyn Fn(&T, &T, f64) -> T,
    miss: &dyn Fn(&T, &T) -> f64,
) -> (Vec<(f64, T)>, f64) {
    let mut keys: Vec<(f64, T)> = times.iter().map(|&t| (t, evaluate(t))).collect();
    let mut out: Vec<(f64, T)> = Vec::with_capacity(keys.len());
    let mut worst = 0.0f64;
    if keys.is_empty() {
        return (out, worst);
    }
    let first = keys.remove(0);
    out.push(first);
    let mut last = first;
    for next in keys {
        let mut stack = vec![(last, next)];
        let mut pieces: Vec<(f64, T)> = Vec::new();
        while let Some((a, b)) = stack.pop() {
            let gap = b.0 - a.0;
            let measured = [0.25, 0.5, 0.75]
                .iter()
                .map(|&f| miss(&evaluate(a.0 + gap * f), &blend(&a.1, &b.1, f)))
                .fold(0.0, f64::max);
            if measured > 1.0 && gap > SHORTEST_GAP {
                let mid = a.0 + gap / 2.0;
                let m = (mid, evaluate(mid));
                stack.push((m, b));
                stack.push((a, m));
            } else {
                worst = worst.max(measured);
                pieces.push(b);
            }
        }
        out.extend(pieces);
        last = next;
    }
    (out, worst)
}

/// Keys of a rotation sampled by `evaluate`, bisected to [`TOLERANCE_DEGREES`], and the worst miss left (degrees).
fn rotation_keys(times: &[f64], evaluate: &dyn Fn(f64) -> Quat) -> (Vec<(f64, Quat)>, f64) {
    let (keys, worst) = bisect_keys(times, evaluate, &|a, b, f| slerp(*a, *b, f), &|a, b| {
        between(*a, *b) / TOLERANCE_DEGREES
    });
    (keys, worst * TOLERANCE_DEGREES)
}

/// A rotation and a translation in the world's frame.
fn world_frame(rotation: Quat, translation: Vec3) -> ([f32; 4], [f32; 3]) {
    let converted = Rest::Trs {
        translation: translation.map(|x| x as f32),
        rotation: rotation.map(|x| x as f32),
        scale: [1.0; 3],
    }
    .converted(&UNITY);
    let Rest::Trs {
        translation,
        rotation,
        ..
    } = converted
    else {
        unreachable!("a TRS converts to a TRS");
    };
    (rotation, translation)
}

/// The rotation channel of `joint` from keys, their quaternions on one hemisphere along the channel so that slerp takes
/// the arc the keys sampled.
fn rotation_channel(skeleton: Entity, joint: JointId, keys: Vec<(f64, Quat)>) -> Channel {
    let mut times = Vec::with_capacity(keys.len());
    let mut values = Vec::with_capacity(keys.len() * 4);
    let mut previous: Option<Quat> = None;
    for (t, q) in keys {
        let q = match previous {
            Some(p) if p.iter().zip(&q).map(|(a, b)| a * b).sum::<f64>() < 0.0 => q.map(|x| -x),
            _ => q,
        };
        previous = Some(q);
        times.push(t as f32);
        values.extend(world_frame(q, [0.0; 3]).0);
    }
    Channel {
        target: ClipTarget::Joint { skeleton, joint },
        property: Property::Rotation,
        interpolation: Interpolation::Linear,
        times,
        values,
    }
}

/// The translation channel of `joint` from keys.
fn translation_channel(skeleton: Entity, joint: JointId, keys: Vec<(f64, Vec3)>) -> Channel {
    let mut times = Vec::with_capacity(keys.len());
    let mut values = Vec::with_capacity(keys.len() * 3);
    for (t, v) in keys {
        times.push(t as f32);
        values.extend(world_frame([0.0, 0.0, 0.0, 1.0], v).1);
    }
    Channel {
        target: ClipTarget::Joint { skeleton, joint },
        property: Property::Translation,
        interpolation: Interpolation::Linear,
        times,
        values,
    }
}

impl ClipData {
    /// The rotation channels of `avatar`'s humanoid bones for this clip's muscle curves, in the world's frame, each
    /// found among the rig's joints by the path hash of its human skeleton node (`resolve`), and what was counted. The
    /// hips get a rotation and a translation channel, placed by the clip's `RootT` and `RootQ` ([`Hips`]). Empty for an
    /// avatar that is not a humanoid or a clip with no muscle curves.
    pub fn humanoid_channels(
        &self,
        avatar: &UnityAvatar,
        resolve: &dyn Fn(u32) -> Option<(Entity, JointId)>,
    ) -> (Vec<Channel>, HumanoidLowering) {
        let mut report = HumanoidLowering::default();
        let Some(human) = &avatar.human else {
            return (Vec::new(), report);
        };
        let mut curves: [Option<&Curve>; MUSCLES] = [None; MUSCLES];
        // RootT.xyz, then RootQ.xyzw
        let mut root: [Option<&Curve>; 7] = [None; 7];
        let mut any = false;
        for Track {
            binding,
            curves: tracks,
        } in &self.tracks
        {
            if binding.type_id != 95 {
                continue;
            }
            match binding.attribute {
                a @ 7..=13 => {
                    report.root_curves += 1;
                    root[(a - 7) as usize] = tracks.first();
                }
                14..=41 => report.goal_curves += 1,
                a if (MUSCLE_ATTRIBUTE_BASE..MUSCLE_ATTRIBUTE_BASE + MUSCLES as u32).contains(&a) => {
                    let m = (a - MUSCLE_ATTRIBUTE_BASE) as usize;
                    if m >= BODY_MUSCLES {
                        report.finger_curves += 1;
                    } else if let Some(c) = tracks.first() {
                        curves[m] = Some(c);
                        any = true;
                    }
                }
                _ => {}
            }
        }
        if !any {
            return (Vec::new(), report);
        }
        report.missing_muscles = curves[..BODY_MUSCLES].iter().filter(|c| c.is_none()).count();
        // every key time of the muscle and root curves, within the clip
        let mut times: Vec<f64> = curves
            .iter()
            .chain(&root)
            .flatten()
            .flat_map(|c| c.keys.iter().map(|k| f64::from(k.time.min(self.duration))))
            .collect();
        times.sort_by(f64::total_cmp);
        times.dedup();
        let muscles_at = |t: f64| -> [f64; MUSCLES] {
            std::array::from_fn(|m| curves[m].map_or(0.0, |c| f64::from(c.sample(t as f32).0)))
        };
        let mut channels = Vec::new();
        for (bone, &name) in BONE_NAMES.iter().enumerate() {
            if bone == 0 || axes_of(human, bone).is_none() {
                continue;
            }
            let Some(&node) = human.bone_index.get(bone).filter(|n| **n >= 0) else {
                continue;
            };
            let Some(path) = usize::try_from(node).ok().and_then(|n| human.skeleton.ids.get(n)) else {
                continue;
            };
            let Some((skeleton, joint)) = resolve(*path) else {
                report.unresolved_bones.push(name);
                continue;
            };
            let rotation = |t: f64| bone_rotation(human, bone, &muscles_at(t)).expect("the bone has axes");
            let (keys, worst) = rotation_keys(&times, &rotation);
            report.worst_between_keys_degrees = report.worst_between_keys_degrees.max(worst);
            report.bones += 1;
            report.keys += keys.len();
            channels.push(rotation_channel(skeleton, joint, keys));
        }
        // the hips: placed by the body's position and orientation
        if let Some(hips) = Hips::new(avatar, human)
            && let Some(path) = node_of(human, 0).and_then(|n| human.skeleton.ids.get(n))
        {
            match resolve(*path) {
                None => report.unresolved_bones.push(BONE_NAMES[0]),
                Some((skeleton, joint)) => {
                    let at_rest = hips.rest_position();
                    let placed = |t: f64| -> Option<(Quat, Vec3)> {
                        let value = |i: usize, default: f64| {
                            root[i].map_or(default, |c| f64::from(c.sample(t as f32).0))
                        };
                        let position = [value(0, at_rest[0]), value(1, at_rest[1]), value(2, at_rest[2])];
                        let rotation =
                            normalize([value(3, 0.0), value(4, 0.0), value(5, 0.0), value(6, 1.0)])
                                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                        hips.place(&muscles_at(t), position, rotation)
                    };
                    if placed(times[0]).is_some() {
                        report.hips = true;
                        report.missing_root_curves = root.iter().filter(|c| c.is_none()).count();
                        let (keys, worst) = rotation_keys(&times, &|t| placed(t).expect("placed").0);
                        report.worst_between_keys_degrees = report.worst_between_keys_degrees.max(worst);
                        report.bones += 1;
                        report.keys += keys.len();
                        channels.push(rotation_channel(skeleton, joint, keys));
                        let limit = TOLERANCE_TRANSLATION * f64::from(human.scale);
                        let (keys, worst) = bisect_keys(
                            &times,
                            &|t| placed(t).expect("placed").1,
                            &|a: &Vec3, b: &Vec3, f| add(*a, scaled(sub(*b, *a), f)),
                            &|a: &Vec3, b: &Vec3| dot(sub(*a, *b), sub(*a, *b)).sqrt() / limit,
                        );
                        report.worst_hips_between_keys =
                            report.worst_hips_between_keys.max(worst * TOLERANCE_TRANSLATION);
                        report.keys += keys.len();
                        channels.push(translation_channel(skeleton, joint, keys));
                    }
                }
            }
        }
        (channels, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animator::AvatarSkeleton;

    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// A one-bone humanoid (the left shoulder, node 1) with identity pre and post rotation, unit signs and limits of 1
    /// radian either way.
    fn human(min: f32, max: f32, sgn: [f32; 3]) -> AvatarHuman {
        let axes = AvatarAxes {
            pre_q: IDENTITY,
            post_q: IDENTITY,
            sgn,
            min: [min; 3],
            max: [max; 3],
            length: 1.0,
            kind: 1,
        };
        let mut bone_index = vec![-1; BONES];
        bone_index[12] = 1;
        AvatarHuman {
            root: Xform {
                t: [0.0; 3],
                q: IDENTITY,
                s: [1.0; 3],
            },
            skeleton: AvatarSkeleton {
                nodes: vec![(-1, -1), (0, 0)],
                ids: vec![0, 7],
                axes: vec![axes],
            },
            pose: vec![],
            left_hand: vec![],
            right_hand: vec![],
            bone_index,
            bone_mass: vec![],
            scale: 1.0,
            arm_twist: 0.5,
            fore_arm_twist: 0.5,
            upper_leg_twist: 0.5,
            leg_twist: 0.5,
            arm_stretch: 0.05,
            leg_stretch: 0.05,
            feet_spacing: 0.0,
            has_left_hand: false,
            has_right_hand: false,
            has_translation_dof: false,
        }
    }

    fn muscles(set: &[(usize, f64)]) -> [f64; MUSCLES] {
        let mut m = [0.0; MUSCLES];
        for &(i, v) in set {
            m[i] = v;
        }
        m
    }

    /// A single muscle is a plain rotation by its limit: positive values use the maximum, negative the minimum
    /// (negative), the axis' sign flips it; muscles beyond 1 overshoot.
    #[test]
    fn a_single_muscle_rotates_by_its_limit() {
        let h = human(-1.0, 0.5, [1.0, 1.0, -1.0]);
        // left shoulder: y is muscle 38, z muscle 37
        let q = bone_rotation(&h, 12, &muscles(&[(38, 1.0)])).unwrap();
        assert!(
            (q[1] - 0.25f64.sin()).abs() < 1e-12 && (q[3] - 0.25f64.cos()).abs() < 1e-12,
            "{q:?}"
        );
        // negative values use the minimum: -1 gives -1 rad, i.e. sgn(1) * (-v * min) = -1
        let q = bone_rotation(&h, 12, &muscles(&[(38, -1.0)])).unwrap();
        assert!((q[1] + 0.5f64.sin()).abs() < 1e-12, "{q:?}");
        // the z axis' sign is -1: +1 gives -0.5 rad
        let q = bone_rotation(&h, 12, &muscles(&[(37, 1.0)])).unwrap();
        assert!((q[2] + 0.25f64.sin()).abs() < 1e-12, "{q:?}");
        // an overshoot of 2 doubles the angle
        let q = bone_rotation(&h, 12, &muscles(&[(38, 2.0)])).unwrap();
        assert!((q[1] - 0.5f64.sin()).abs() < 1e-12, "{q:?}");
        // a bone the model lacks, and one with no axes
        assert!(bone_rotation(&h, 11, &muscles(&[])).is_none());
    }

    /// Two axes together are the swing `normalize(0, tan(y/2), tan(z/2), 1)`: not the product of the two rotations.
    #[test]
    fn two_axes_swing_as_a_tangent_quaternion() {
        let h = human(-1.0, 1.0, [1.0; 3]);
        let q = bone_rotation(&h, 12, &muscles(&[(38, 0.8), (37, 0.6)])).unwrap();
        let (ty, tz) = ((0.8f64 / 2.0).tan(), (0.6f64 / 2.0).tan());
        let n = (ty * ty + tz * tz + 1.0).sqrt();
        let want = [0.0, ty / n, tz / n, 1.0 / n];
        for i in 0..4 {
            assert!((q[i] - want[i]).abs() < 1e-12, "{q:?} vs {want:?}");
        }
        // it is unit length and differs from the exponential map of the vector (0, 0.8, 0.6)
        assert!((q.iter().map(|x| x * x).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((q[1] - (0.5f64).sin() * 0.8).abs() > 1e-3);
    }

    /// The twist of a limb is its total angle wrapped into (-pi, pi]: the upper arm and the forearm after it turn the same
    /// at a muscle value 4 apart (360 degrees of total twist), and a total of 270 degrees is -90.
    #[test]
    fn the_total_twist_of_a_limb_is_wrapped_before_it_is_shared() {
        use std::f64::consts::PI;
        assert_eq!(wrap_pi(0.0), 0.0);
        assert!((wrap_pi(1.5 * PI) + 0.5 * PI).abs() < 1e-12);
        assert!((wrap_pi(-1.5 * PI) - 0.5 * PI).abs() < 1e-12);
        assert!((wrap_pi(PI) - PI).abs() < 1e-12 && (wrap_pi(-PI) - PI).abs() < 1e-12);
        assert!((wrap_pi(5.0 * PI + 0.25) - (-PI + 0.25)).abs() < 1e-9);
        // the left upper arm (bone 14) and its forearm (16) on the one-bone fixture's axes: limit pi/2 either way, so a
        // muscle value of 4 is a total of 2 pi
        // (limits are stored as f32, so the periods agree to ~1e-5 degrees, not to f64)
        let mut h = human(-(PI as f32) / 2.0, PI as f32 / 2.0, [1.0; 3]);
        h.bone_index[14] = 1;
        h.bone_index[16] = 1;
        for (bone, muscle) in [(14, 41), (16, 43)] {
            let at = |v: f64| bone_rotation(&h, bone, &muscles(&[(muscle, v)])).unwrap();
            assert!(between(at(1.0), at(5.0)) < 1e-4, "{bone}: v and v + 4");
            assert!(between(at(-1.5), at(2.5)) < 1e-4, "{bone}: -1.5 and 2.5");
            // 3 (a total of 270 degrees) is -90 degrees; the share keeps half
            assert!(between(at(3.0), at(-1.0)) < 1e-4, "{bone}: 3 and -1");
        }
        // the share of the forearm below: the upper arm's rest rotates it, wrapped the same
        let forearm = |v: f64| bone_rotation(&h, 16, &muscles(&[(41, v)])).unwrap();
        assert!(between(forearm(1.0), forearm(5.0)) < 1e-4);
        // a bone with no shared twist is not wrapped: the shoulder's swing at 4 is not its swing at 0
        let shoulder = |v: f64| bone_rotation(&h, 12, &muscles(&[(38, v)])).unwrap();
        assert!(between(shoulder(0.0), shoulder(1.0)) > 1e-3);
    }

    /// A slerp at the ends and the angle between rotations.
    #[test]
    fn slerp_and_angle_between_rotations() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = rotation_x(1.0);
        assert!(between(slerp(a, b, 0.0), a) < 1e-9 && between(slerp(a, b, 1.0), b) < 1e-9);
        assert!((between(a, b).to_radians() - 1.0).abs() < 1e-12);
        assert!((between(slerp(a, b, 0.5), rotation_x(0.5))) < 1e-9);
        // the opposite sign is the same rotation
        assert!(between(a, [0.0, 0.0, 0.0, -1.0]) < 1e-9);
    }

    /// The rotation of a set of axes is the rotation that has them as images of x, y and z, and `rotate` applies it.
    #[test]
    fn axes_and_rotations_round_trip() {
        for q in [
            [0.0, 0.0, 0.0, 1.0],
            rotation_x(1.0),
            rotation_x(3.0),
            normalize([0.3, -0.5, 0.2, 0.7]).unwrap(),
            normalize([0.9, 0.1, -0.3, 0.05]).unwrap(),
            normalize([-0.1, 0.8, 0.2, 0.0]).unwrap(),
        ] {
            let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|v| rotate(q, v));
            assert!(between(from_columns(axes), q) < 1e-9, "{q:?}");
        }
        // x turned a quarter about y is -z
        let r = rotate([0.0, 0.5f64.sqrt(), 0.0, 0.5f64.sqrt()], [1.0, 0.0, 0.0]);
        assert!(
            r[0].abs() < 1e-12 && r[1].abs() < 1e-12 && (r[2] + 1.0).abs() < 1e-12,
            "{r:?}"
        );
    }

    /// A trunk with the bones the body frame and the centre of mass need: hips (node 1), upper legs (2, 3), spine (4) and
    /// upper arms (5, 6), every node a child of the hips but the hips', with the masses 0.3, 0.1 + 0.1, 0.2 and 0.15 + 0.15.
    fn trunk() -> AvatarHuman {
        let mut h = human(-1.0, 1.0, [1.0; 3]);
        let at = |t: [f32; 3]| Xform {
            t,
            q: IDENTITY,
            s: [1.0; 3],
        };
        h.skeleton = AvatarSkeleton {
            nodes: vec![(-1, -1), (0, -1), (1, -1), (1, -1), (1, -1), (1, -1), (1, -1)],
            ids: vec![0; 7],
            axes: vec![],
        };
        h.pose = vec![
            at([0.0; 3]),
            at([0.0, 1.0, 0.0]),
            at([-0.1, -0.1, 0.0]),
            at([0.1, -0.1, 0.0]),
            at([0.0, 0.2, 0.0]),
            at([-0.4, 0.5, 0.0]),
            at([0.4, 0.5, 0.0]),
        ];
        h.bone_index = vec![-1; BONES];
        for (bone, node) in [(0, 1), (1, 2), (2, 3), (7, 4), (14, 5), (15, 6)] {
            h.bone_index[bone] = node;
        }
        h.bone_mass = vec![0.0; BONES];
        for (bone, mass) in [(0, 0.3), (1, 0.1), (2, 0.1), (7, 0.2), (14, 0.15), (15, 0.15)] {
            h.bone_mass[bone] = mass;
        }
        h.scale = 2.0;
        h
    }

    /// The origins of the trunk in the hips' frame, the frame of a symmetric body is the axes, and the centre of mass is
    /// the masses' mean of the segments' centres: the hips' at the mean of their children's origins, the spine's at the
    /// mean of its own and the arms' (the nearest present bones below it), the others' at their own.
    #[test]
    fn the_trunks_frame_and_centre_of_mass() {
        let h = trunk();
        let o = posed_origins(&h, &[0.0; MUSCLES]).unwrap();
        // the pose's translations are f32 (-0.4 is stored as -0.4000000059604645)
        assert_eq!(o[0], Some([0.0; 3]));
        let arm = o[14].unwrap();
        assert!(
            (0..3).all(|i| (arm[i] - [-0.4, 0.5, 0.0][i]).abs() < 1e-6),
            "{arm:?}"
        );
        let axes = body_axes(&o).unwrap();
        for (axis, want) in axes
            .iter()
            .zip([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
        {
            assert!((0..3).all(|i| (axis[i] - want[i]).abs() < 1e-6), "{axes:?}");
        }
        let mean = |points: &[Vec3]| {
            scaled(
                points.iter().fold([0.0; 3], |a, p| add(a, *p)),
                1.0 / points.len() as f64,
            )
        };
        let (spine, left_leg, right_leg) = ([0.0, 0.2, 0.0], [-0.1, -0.1, 0.0], [0.1, -0.1, 0.0]);
        let (left_arm, right_arm) = ([-0.4, 0.5, 0.0], [0.4, 0.5, 0.0]);
        let want = [
            (0.3, mean(&[spine, left_leg, right_leg])),
            (0.1, left_leg),
            (0.1, right_leg),
            (0.2, mean(&[left_arm, right_arm, spine])),
            (0.15, left_arm),
            (0.15, right_arm),
        ]
        .iter()
        .fold([0.0; 3], |a, (m, c)| add(a, scaled(*c, *m)));
        let got = centre_of_mass(&h, &o).unwrap();
        assert!(
            (0..3).all(|i| (got[i] - want[i]).abs() < 1e-6),
            "{got:?} vs {want:?}"
        );
    }

    /// The hips are placed so that the pose's body frame is the stated rotation of the rest frame and its centre of mass
    /// is the body position times the human scale, whatever the hips' parent is.
    #[test]
    fn the_hips_are_placed_by_the_body_position_and_rotation() {
        let h = trunk();
        let rest = normalize([0.03, 0.0, 0.0, 1.0]).unwrap();
        let parent = Placed {
            rotation: normalize([0.5, -0.5, 0.5, 0.5]).unwrap(),
            scale: [1.0, 2.0, 1.0],
            origin: [0.3, -0.2, 0.1],
        };
        let hips = Hips {
            human: &h,
            rest,
            parent,
        };
        let (body, turn) = ([0.1, 1.2, -0.3], normalize([0.2, -0.4, 0.1, 0.8]).unwrap());
        let (local_rotation, local_translation) = hips.place(&[0.0; MUSCLES], body, turn).unwrap();
        // back to the avatar's space
        let at = parent.child(local_translation, local_rotation, [1.0; 3]);
        let o = posed_origins(&h, &[0.0; MUSCLES]).unwrap();
        let frame = from_columns(body_axes(&o).unwrap());
        assert!(between(mul(at.rotation, frame), mul(turn, rest)) < 1e-7);
        let centre = add(at.origin, rotate(at.rotation, centre_of_mass(&h, &o).unwrap()));
        let want = scaled(body, 2.0);
        assert!(
            (0..3).all(|i| (centre[i] - want[i]).abs() < 1e-9),
            "{centre:?} vs {want:?}"
        );
    }
}
