//! `lower-anvil-skeleton{own-transform=compose|refuse}`: a Skeleton ([`AnvilSkeleton`]) becomes a `core.skeleton`.
//!
//! - **Joints** (read): one per bone, listed in the skeleton's order, its [`JointId`] the bone's position there, its
//!   parent the bone whose object id the bone's parent pointer names (a bone the skeleton doesn't hold is an error,
//!   naming it), its rest the bone's LocalPosition and local rotation (the quaternion x, y, z, w by value) in its
//!   parent's space as a translation-rotation-scale with scale 1 (the bone stores none), converted from the frame the
//!   reader was given into the world's.
//! - **Name** (read): the bone stores its name's CRC-32 and no string, so a joint is named by that hash in eight
//!   hexadecimal digits (`07c159a2` is "Head"): provenance, as the core says a name is, not identity.
//! - **The skeleton's own transform** (stored, its place in the rig not established): `own-transform=compose` puts it in
//!   front of each root bone's rest (a root's parent space being the skeleton's), `refuse` is an error where it isn't
//!   the identity. 16 of the 361 skeletons the Riders Republic census read through this field have one that isn't.
//!
//! `lower-anvil-rig{own-transform=compose|refuse}`: the same for a SkeletonComponent's rig ([`AnvilRig`]: its main
//! Skeleton's and addons' bones joined by name), joints in the rig's order, the main Skeleton's own transform the rig's.
//!
//! Left out, counted: the pose groups (SkeletonPoses of SkeletonPoseBones, kept as bytes), the IKData, the Reflex
//! constraints, the global transforms (derivable from the local ones), each bone's mirror, flags, kinds and hash lists.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::skeleton::{Joint, JointId, Rest, Skeleton};
use primase_core::{Component, World};

use crate::components::{AnvilRig, AnvilSkeleton};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilSkeleton {
    /// Whether a skeleton's own position and rotation are composed into its roots' rests.
    pub compose_own: bool,
}

impl LowerAnvilSkeleton {
    /// From its options: `own-transform=compose|refuse`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilSkeleton, String> {
        Ok(LowerAnvilSkeleton {
            compose_own: own_transform_option(o)?,
        })
    }
}

type Quat = [f64; 4];

/// `a` then `b`: the product `a b` of quaternions x, y, z, w.
fn mul(a: Quat, b: Quat) -> Quat {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `q` rotating `v`.
fn rotate(q: Quat, v: [f64; 3]) -> [f64; 3] {
    let p = mul(mul(q, [v[0], v[1], v[2], 0.0]), [-q[0], -q[1], -q[2], q[3]]);
    [p[0], p[1], p[2]]
}

/// One joint as the source states it: its name (a hash), its parent's index and its local translation and rotation.
struct RawJoint {
    name_hash: u32,
    parent: Option<usize>,
    translation: [f32; 3],
    rotation: [f32; 4],
}

/// The joints of `raw` in its order, the own transform (`position` and `rotation`) composed in front of each root's rest
/// where `compose_own`, refused where it isn't the identity and not composed.
fn joints_of(
    raw: &[RawJoint],
    position: [f32; 4],
    rotation: [f32; 4],
    compose_own: bool,
) -> Result<Vec<Joint>, String> {
    let own_position = [position[0], position[1], position[2]];
    let own_rotation = rotation.map(f64::from);
    let identity = own_position.iter().all(|&v| v.abs() < 1e-6)
        && own_rotation[..3].iter().all(|&v| v.abs() < 1e-6)
        && (own_rotation[3] - 1.0).abs() < 1e-4;
    if !identity && !compose_own {
        return Err(format!(
            "a skeleton's own position {own_position:?} and rotation {rotation:?} aren't the identity and \
             own-transform=refuse"
        ));
    }
    raw.iter()
        .enumerate()
        .map(|(i, r)| {
            let id = JointId(u32::try_from(i).map_err(|_| "too many bones")?);
            let parent = r
                .parent
                .map(|p| u32::try_from(p).map(JointId).map_err(|_| "too many bones"))
                .transpose()?;
            let mut translation = r.translation;
            let mut rot = r.rotation;
            if parent.is_none() && compose_own && !identity {
                let moved = rotate(own_rotation, translation.map(f64::from));
                translation = std::array::from_fn(|k| (f64::from(own_position[k]) + moved[k]) as f32);
                rot = mul(own_rotation, rot.map(f64::from)).map(|v| v as f32);
            }
            Ok(Joint {
                id,
                name: format!("{:08x}", r.name_hash),
                parent,
                rest: Rest::Trs {
                    translation,
                    rotation: rot,
                    scale: [1.0; 3],
                },
            })
        })
        .collect()
}

/// The `own-transform` option, required.
fn own_transform_option(o: &Options) -> Result<bool, String> {
    if let Some(k) = o.keys().find(|k| k.as_str() != "own-transform") {
        return Err(format!("unknown option {k}"));
    }
    match o.get("own-transform").map(String::as_str) {
        Some("compose") => Ok(true),
        Some("refuse") => Ok(false),
        Some(v) => Err(format!("own-transform={v} is not compose or refuse")),
        None => Err("own-transform is required: own-transform=compose|refuse".to_owned()),
    }
}

impl Pass for LowerAnvilSkeleton {
    fn name(&self) -> &'static str {
        "lower-anvil-skeleton"
    }
    fn options(&self) -> Options {
        Options::from([(
            "own-transform".to_owned(),
            if self.compose_own { "compose" } else { "refuse" }.to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilSkeleton::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Skeleton::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilSkeleton::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let skeletons: Vec<_> = w.iter::<AnvilSkeleton>().map(|(e, _)| e).collect();
        for e in skeletons {
            let Some(AnvilSkeleton { frame, skeleton }) = w.remove::<AnvilSkeleton>(e) else {
                continue;
            };
            let index = |id: u64| skeleton.bones.iter().position(|b| b.id == id);
            let raw = skeleton
                .bones
                .iter()
                .map(|bone| {
                    let parent = match bone.parent {
                        None => None,
                        Some(id) => Some(index(id).ok_or_else(|| {
                            format!(
                                "bone {:08x} names a parent object {id:#x} the skeleton doesn't hold",
                                bone.name_hash
                            )
                        })?),
                    };
                    Ok(RawJoint {
                        name_hash: bone.name_hash,
                        parent,
                        translation: [
                            bone.local_position[0],
                            bone.local_position[1],
                            bone.local_position[2],
                        ],
                        rotation: bone.local_rotation,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let joints = joints_of(&raw, skeleton.position, skeleton.rotation, self.compose_own)?;
            log.count("bones lowered to joints", joints.len() as u64);
            let poses: usize = skeleton.pose_groups.iter().map(|g| g.poses.len()).sum();
            if poses > 0 {
                log.omit(
                    "skeleton poses (pose groups of 16-byte pose bones, kept as bytes only)",
                    poses as u64,
                );
            }
            if skeleton.ik_data.is_some() {
                log.omit("skeleton IKData", 1);
            }
            if skeleton.reflex3.is_some() {
                log.omit("skeleton Reflex3SkeletonConstraints", 1);
            }
            log.omit(
                "bones' global transforms, mirrors, flags, kinds and hash lists, the skeleton's root pointer and body-part mapping",
                skeleton.bones.len() as u64,
            );
            let skeleton = Skeleton { joints }.converted(&frame);
            w.insert(e, skeleton);
        }
        Ok(())
    }
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilRig {
    /// Whether the main Skeleton's own position and rotation are composed into the roots' rests.
    pub compose_own: bool,
}

impl LowerAnvilRig {
    /// From its options: `own-transform=compose|refuse`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilRig, String> {
        Ok(LowerAnvilRig {
            compose_own: own_transform_option(o)?,
        })
    }
}

impl Pass for LowerAnvilRig {
    fn name(&self) -> &'static str {
        "lower-anvil-rig"
    }
    fn options(&self) -> Options {
        Options::from([(
            "own-transform".to_owned(),
            if self.compose_own { "compose" } else { "refuse" }.to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilRig::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Skeleton::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilRig::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let rigs: Vec<_> = w.iter::<AnvilRig>().map(|(e, _)| e).collect();
        for e in rigs {
            let Some(AnvilRig {
                frame,
                position,
                rotation,
                rig,
            }) = w.remove::<AnvilRig>(e)
            else {
                continue;
            };
            let raw = rig
                .bones
                .iter()
                .map(|b| {
                    let parent = match b.parent {
                        None => None,
                        Some(name) => Some(rig.position(name).ok_or_else(|| {
                            format!(
                                "bone {:08x} names a parent {name:08x} the rig doesn't hold",
                                b.name_hash
                            )
                        })?),
                    };
                    Ok(RawJoint {
                        name_hash: b.name_hash,
                        parent,
                        translation: [b.local_position[0], b.local_position[1], b.local_position[2]],
                        rotation: b.local_rotation,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let joints = joints_of(&raw, position, rotation, self.compose_own)?;
            log.count("rigs lowered", 1);
            log.count("bones lowered to joints", joints.len() as u64);
            log.omit(
                "addon Skeletons' own transforms (the main Skeleton's is the rig's), pose groups, IKData and constraints",
                1,
            );
            w.insert(e, Skeleton { joints }.converted(&frame));
        }
        Ok(())
    }
}
