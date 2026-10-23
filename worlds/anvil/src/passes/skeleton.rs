//! `lower-anvil-skeleton{own-transform=replace|compose|refuse|ignore}`: a Skeleton ([`AnvilSkeleton`]) becomes a `core.skeleton`.
//!
//! - **Joints** (read): one per bone, listed in the skeleton's order, its [`JointId`] the bone's position there, its
//!   parent the bone whose object id the bone's parent pointer names (a bone the skeleton doesn't hold is an error,
//!   naming it), its rest the bone's LocalPosition and local rotation (the quaternion x, y, z, w by value) in its
//!   parent's space as a translation-rotation-scale with scale 1 (the bone stores none), converted from the frame the
//!   reader was given into the world's.
//! - **Name** (read): the bone stores its name's CRC-32 and no string, so a joint is named by that hash in eight
//!   hexadecimal digits (`07c159a2` is "Head"): provenance, as the core says a name is, not identity.
//! - **The skeleton's own transform**: a Skeleton stores a position and a rotation of its own. `own-transform=replace`
//!   makes the root bone's rest the own position and rotation, **as the engine's runtime pose does**; `compose` puts
//!   it in front of each root bone's rest, `refuse` is an error where it isn't the identity, `ignore` leaves it out.
//!   What the data says (Riders Republic, build 25394987): of the 1,120 Skeleton root rows the root bone's local
//!   position and rotation equal the own ones on 1,033 (so composing counts them twice, and replacing and ignoring
//!   agree); the exceptions are the human rig `MAD_CHR_Skel_U` (root rotation the identity, own 0.964 up and a quarter
//!   turn about z) and the bike `MAD_CHR_Skel_Bike_U` (root rotation a quarter turn about z backwards, own the quarter
//!   turn forwards). The skeleton's bones' stored global transforms and the meshes' bind poses
//!   (`InitialMatrixInverse` equals the inverse of the bone's stored rest world matrix on every bone of 22 of the 118
//!   mesh and rig pairs whose rig has a non-identity own transform, and of none when it is composed) do not hold it.
//!   The engine's own pose arrays in the memory of a running game (the world snapshot: records of position, rotation
//!   and scale per bone) hold, for the human rig, a root at (0, 0, 0.964) turned a quarter turn about z with every
//!   other bone at its stored local, and for the bike a root turned a quarter turn about z **forwards** (its stored
//!   root rotation is backwards, so neither composing, which gives the identity, nor ignoring gives it) at height
//!   0.012 (a value no Skeleton holds; its source isn't established). Each Skeleton has exactly one bone without a
//!   parent, its root, the first bone (1,138 of 1,138). So the engine replaces the root's local transform with the
//!   own one: the human faces along x in the bones' space and along y after it, the bike's front along -y before and
//!   along y after, both facing the game's forward.
//!
//! `lower-anvil-rig{own-transform=replace|compose|refuse|ignore}`: the same for a SkeletonComponent's rig ([`AnvilRig`]: its main
//! Skeleton's and addons' bones joined by name), joints in the rig's order, the main Skeleton's own transform the rig's.
//!
//! Left out, counted: the pose groups (SkeletonPoses of SkeletonPoseBones, kept as bytes), the IKData, the Reflex
//! constraints, the global transforms (derivable from the local ones), each bone's mirror, flags, kinds and hash lists.

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::skeleton::{Joint, JointId, Rest, Skeleton};
use primase_core::{Component, World};

use crate::components::{AnvilRig, AnvilSkeleton};

/// What a pass does with a skeleton's own position and rotation (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnTransform {
    /// The root's rest is the own position and rotation (the engine's runtime pose).
    Replace,
    /// Composed in front of each root's rest.
    Compose,
    /// An error where it isn't the identity.
    Refuse,
    /// Left out.
    Ignore,
}

impl Choice for OwnTransform {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("replace", OwnTransform::Replace),
        ("compose", OwnTransform::Compose),
        ("refuse", OwnTransform::Refuse),
        ("ignore", OwnTransform::Ignore),
    ];
}

/// `own-transform`, as `lower-anvil-skeleton` and `lower-anvil-rig` take it.
const OWN_TRANSFORM: [OptionSpec; 1] = [OptionSpec::choice::<OwnTransform>(
    "own-transform",
    "a skeleton's own position and rotation: the root's rest, composed in front of it, refused unless the \
     identity, or left out",
)];

/// `lower-anvil-skeleton`'s declaration ([`primase_core::pass::PassRegistry::add`]): what becomes of a skeleton's own
/// transform is the program's.
pub const LOWER_ANVIL_SKELETON: PassSpec = PassSpec {
    name: "lower-anvil-skeleton",
    options: &OWN_TRANSFORM,
    make: |g| {
        Ok(Box::new(LowerAnvilSkeleton {
            own_transform: g.choice("own-transform"),
        }))
    },
};

/// `lower-anvil-rig`'s declaration ([`primase_core::pass::PassRegistry::add`]): as `lower-anvil-skeleton`'s.
pub const LOWER_ANVIL_RIG: PassSpec = PassSpec {
    name: "lower-anvil-rig",
    options: &OWN_TRANSFORM,
    make: |g| {
        Ok(Box::new(LowerAnvilRig {
            own_transform: g.choice("own-transform"),
        }))
    },
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilSkeleton {
    /// What is done with a skeleton's own position and rotation.
    pub own_transform: OwnTransform,
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

/// The joints of `raw` in its order, the own transform (`position` and `rotation`) the root's rest, composed in front
/// of it or left out, or refused where it isn't the identity, as `own` says.
fn joints_of(
    raw: &[RawJoint],
    position: [f32; 4],
    rotation: [f32; 4],
    own: OwnTransform,
) -> Result<Vec<Joint>, String> {
    let own_position = [position[0], position[1], position[2]];
    let own_rotation = rotation.map(f64::from);
    let identity = own_position.iter().all(|&v| v.abs() < 1e-6)
        && own_rotation[..3].iter().all(|&v| v.abs() < 1e-6)
        && (own_rotation[3] - 1.0).abs() < 1e-4;
    if !identity && own == OwnTransform::Refuse {
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
            if parent.is_none() && own == OwnTransform::Replace {
                translation = own_position;
                rot = rotation;
            }
            if parent.is_none() && own == OwnTransform::Compose && !identity {
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

impl Pass for LowerAnvilSkeleton {
    fn name(&self) -> &'static str {
        "lower-anvil-skeleton"
    }
    fn options(&self) -> Options {
        Options::from([("own-transform".to_owned(), self.own_transform.text().to_owned())])
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
            let joints = joints_of(&raw, skeleton.position, skeleton.rotation, self.own_transform)?;
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
    /// What is done with the main Skeleton's own position and rotation.
    pub own_transform: OwnTransform,
}

impl Pass for LowerAnvilRig {
    fn name(&self) -> &'static str {
        "lower-anvil-rig"
    }
    fn options(&self) -> Options {
        Options::from([("own-transform".to_owned(), self.own_transform.text().to_owned())])
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
            let joints = joints_of(&raw, position, rotation, self.own_transform)?;
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
