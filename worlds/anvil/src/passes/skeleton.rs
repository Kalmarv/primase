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
//! Left out, counted: the pose groups (SkeletonPoses of SkeletonPoseBones, kept as bytes), the IKData, the Reflex
//! constraints, the global transforms (derivable from the local ones), each bone's mirror, flags, kinds and hash lists.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::skeleton::{Joint, JointId, Rest, Skeleton};
use primase_core::{Component, World};

use crate::components::AnvilSkeleton;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilSkeleton {
    /// Whether a skeleton's own position and rotation are composed into its roots' rests.
    pub compose_own: bool,
}

impl LowerAnvilSkeleton {
    /// From its options: `own-transform=compose|refuse`, required.
    pub fn from_options(o: &Options) -> Result<LowerAnvilSkeleton, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "own-transform") {
            return Err(format!("unknown option {k}"));
        }
        let compose_own = match o.get("own-transform").map(String::as_str) {
            Some("compose") => true,
            Some("refuse") => false,
            Some(v) => return Err(format!("own-transform={v} is not compose or refuse")),
            None => return Err("own-transform is required: own-transform=compose|refuse".to_owned()),
        };
        Ok(LowerAnvilSkeleton { compose_own })
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
            let name = |i: usize| format!("{:08x}", skeleton.bones.get(i).map_or(0, |b| b.name_hash));
            let index = |id: u64| skeleton.bones.iter().position(|b| b.id == id);
            let own_position = [skeleton.position[0], skeleton.position[1], skeleton.position[2]];
            let own_rotation = skeleton.rotation.map(f64::from);
            let identity = own_position.iter().all(|&v| v.abs() < 1e-6)
                && own_rotation[..3].iter().all(|&v| v.abs() < 1e-6)
                && (own_rotation[3] - 1.0).abs() < 1e-4;
            if !identity && !self.compose_own {
                return Err(format!(
                    "a skeleton's own position {own_position:?} and rotation {:?} aren't the identity and \
                     own-transform=refuse",
                    skeleton.rotation
                ));
            }
            let mut joints = Vec::with_capacity(skeleton.bones.len());
            for (i, bone) in skeleton.bones.iter().enumerate() {
                let parent = match bone.parent {
                    None => None,
                    Some(id) => Some(JointId(
                        index(id).and_then(|p| u32::try_from(p).ok()).ok_or_else(|| {
                            format!(
                                "bone {} names a parent object {id:#x} the skeleton doesn't hold",
                                name(i)
                            )
                        })?,
                    )),
                };
                let id = JointId(u32::try_from(i).map_err(|_| "too many bones")?);
                let mut translation = [
                    bone.local_position[0],
                    bone.local_position[1],
                    bone.local_position[2],
                ];
                let mut rotation = bone.local_rotation;
                if parent.is_none() && self.compose_own && !identity {
                    let moved = rotate(own_rotation, translation.map(f64::from));
                    translation = std::array::from_fn(|k| (f64::from(own_position[k]) + moved[k]) as f32);
                    let q = mul(own_rotation, rotation.map(f64::from));
                    rotation = q.map(|v| v as f32);
                }
                joints.push(Joint {
                    id,
                    name: name(i),
                    parent,
                    rest: Rest::Trs {
                        translation,
                        rotation,
                        scale: [1.0; 3],
                    },
                });
            }
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
