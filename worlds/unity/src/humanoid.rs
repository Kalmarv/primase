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
//!   own rotation.
//!
//! **What is not evaluated**: the body's centre of mass and orientation (a clip's `RootT`, `RootQ`) that place the hips,
//! the IK goals, and the fingers' muscles (no avatar read here has finger bones, so the formula for them, the same,
//! has no oracle). Each is counted by the lowering.

use primase_core::{Channel, ClipTarget, Entity, Interpolation, JointId, Property, Rest};

use crate::animator::{AvatarAxes, AvatarHuman, UnityAvatar};
use crate::components::UNITY;
use crate::read::clip::{ClipData, Curve, Track};

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

fn mul(a: Quat, b: Quat) -> Quat {
    let ([x1, y1, z1, w1], [x2, y2, z2, w2]) = (a, b);
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

fn inverse(q: Quat) -> Quat {
    let n = q.iter().map(|x| x * x).sum::<f64>();
    [-q[0] / n, -q[1] / n, -q[2] / n, q[3] / n]
}

fn f64q(q: [f32; 4]) -> Quat {
    q.map(f64::from)
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
    let a = degrees(axes, bone);
    let share = if TWIST_CHILD.iter().any(|(p, _)| *p == bone) {
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
        let rest = degrees(parent_axes, parent)[0] * (1.0 - twist_share(human, parent));
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

/// Spherical interpolation along the shorter arc.
fn slerp(a: Quat, mut b: Quat, t: f64) -> Quat {
    let mut dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    if dot < 0.0 {
        b = b.map(|x| -x);
        dot = -dot;
    }
    if dot > 0.999_999 {
        let q: Quat = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        return q.map(|x| x / n);
    }
    let theta = dot.acos();
    let (wa, wb) = (
        ((1.0 - t) * theta).sin() / theta.sin(),
        (t * theta).sin() / theta.sin(),
    );
    std::array::from_fn(|i| a[i] * wa + b[i] * wb)
}

/// How far a lowered bone channel may be from the muscles' rotation between its keys, in degrees, and the shortest gap
/// between keys (seconds): keys are added by bisection until slerp between them is within the tolerance.
const TOLERANCE_DEGREES: f64 = 0.01;
const SHORTEST_GAP: f64 = 1.0 / 480.0;

/// What lowering a humanoid clip did.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct HumanoidLowering {
    /// Bones given a rotation channel, and the keys of those channels.
    pub bones: usize,
    pub keys: usize,
    /// The largest angle, over the bones and the gaps between keys, between the muscles' rotation at a quarter, half
    /// and three quarters of a gap and the channel's slerp there (degrees).
    pub worst_between_keys_degrees: f64,
    /// Muscle curves the clip lacks (those muscles stay 0), RootT/RootQ curves, IK goal curves and finger muscle curves
    /// the clip has and nothing here applies.
    pub missing_muscles: usize,
    pub root_curves: usize,
    pub goal_curves: usize,
    pub finger_curves: usize,
    /// Human bones of the avatar whose joint the rig lacks, by name.
    pub unresolved_bones: Vec<&'static str>,
}

impl ClipData {
    /// The rotation channels of `avatar`'s humanoid bones for this clip's muscle curves, in the world's frame, each
    /// found among the rig's joints by the path hash of its human skeleton node (`resolve`), and what was counted.
    /// Empty for an avatar that is not a humanoid or a clip with no muscle curves.
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
                7..=13 => report.root_curves += 1,
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
        // every key time of the muscle curves, within the clip
        let mut times: Vec<f64> = curves
            .iter()
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
            // keys: the muscles' key times, bisected where slerp between them is off
            let mut keys: Vec<(f64, Quat)> = times.iter().map(|&t| (t, rotation(t))).collect();
            let mut out: Vec<(f64, Quat)> = Vec::with_capacity(keys.len());
            let mut worst = 0.0f64;
            let first = keys.remove(0);
            out.push(first);
            let mut last = first;
            for next in keys {
                let mut stack = vec![(last, next)];
                let mut pieces: Vec<(f64, Quat)> = Vec::new();
                while let Some((a, b)) = stack.pop() {
                    let gap = b.0 - a.0;
                    let off = |t: f64, f: f64| between(rotation(t), slerp(a.1, b.1, f));
                    let measured = [0.25, 0.5, 0.75]
                        .iter()
                        .map(|&f| off(a.0 + gap * f, f))
                        .fold(0.0, f64::max);
                    if measured > TOLERANCE_DEGREES && gap > SHORTEST_GAP {
                        let mid = a.0 + gap / 2.0;
                        let m = (mid, rotation(mid));
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
            report.worst_between_keys_degrees = report.worst_between_keys_degrees.max(worst);
            let mut times_out = Vec::with_capacity(out.len());
            let mut values = Vec::with_capacity(out.len() * 4);
            let mut previous: Option<Quat> = None;
            for (t, q) in out {
                // one hemisphere along the channel, so slerp takes the arc the keys sampled
                let q = match previous {
                    Some(p) if p.iter().zip(&q).map(|(a, b)| a * b).sum::<f64>() < 0.0 => q.map(|x| -x),
                    _ => q,
                };
                previous = Some(q);
                let converted = Rest::Trs {
                    translation: [0.0; 3],
                    rotation: q.map(|x| x as f32),
                    scale: [1.0; 3],
                }
                .converted(&UNITY);
                let Rest::Trs { rotation, .. } = converted else {
                    unreachable!("a TRS converts to a TRS");
                };
                times_out.push(t as f32);
                values.extend(rotation);
            }
            report.bones += 1;
            report.keys += times_out.len();
            channels.push(Channel {
                target: ClipTarget::Joint { skeleton, joint },
                property: Property::Rotation,
                interpolation: Interpolation::Linear,
                times: times_out,
                values,
            });
        }
        (channels, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animator::{AvatarSkeleton, Xform};

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
}
