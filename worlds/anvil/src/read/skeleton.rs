//! A Skeleton resource: the bone tree of a Riders Republic rig (the character kit's `MAD_CHR_Skel_*` and
//! `MAD_CHR_SkelAddon_*` rigs and the animals', props' and bikes'), read as stored. Little-endian.
//!
//! The body (the registry's stream order, from docs/types.md "Skeleton" and checked on all 1,138 Skeletons of the
//! game): `01`; a u32 (`9C78EB24`, an enum: 0 to 8 and 14 occur); the bones, a u32 count and each an object (`00`, a
//! u64 id, 0x95741049, the body below); the root (`4E1F230B`, a pointer to a bone: `03` none or `02` and a u64, a
//! reference to a bone read before); the skeleton's own position (a f32 x 4, `F4D6F079`) and rotation (a f32 x 4,
//! `9B868C7D`); a BodyPartMapping reference (10 bytes); the SkeletonPoseGroups (a u32 count, each an object of the
//! type 0xC8F8ABFF holding a u32 count of SkeletonPoses, each an object of 0x209CF072 with a f32 and a u32 count of
//! SkeletonPoseBones, each an object of 0x071CEE50 and 16 bytes); an IKData pointer (`03` or `00`, a u64 and 0x9A33101E:
//! the class stores no field); a LookAt reference (10 bytes); a ReflexSystem pointer (`03`: none on all 1,138); two f32;
//! a Reflex3SkeletonConstraints pointer (`03`, or `00`, a u64, 0x8E3FB47A and a u32 count of bytes); and a u32 count of
//! u32 (`3708B98C`).
//!
//! A bone: its name's CRC-32 (a u32 of the registry's two packed bit fields `41B24805` and `778FF80E`: "Head" hashes to
//! the first bone of `MAD_CHR_SkelAddon_HeadW_045_F`); its parent (`C46EF58D`, a pointer: `03` none or `02` and a u64
//! reference); `230C4503`, a pointer (`03`, or `02` and a u64: itself on a bone in a chain, the opposite side's bone
//! on a paired one, by value); `1263612A`, a BoneDisplayData pointer (`03`: none on all 54,925); GlobalPosition (`6DF2DFE7`),
//! `02A2A3E3`, LocalPosition (`E5B929A6`) and `8AE955A2`, each a f32 x 4 (the rotations unit quaternions x, y, z, w
//! by value: the fourth component of a position is 0); a byte (`EF07E0C9`); a u32 (`73196C8B`, an enum); a BoneModifier
//! array (`5A587CD0`: a u32 count, 0 on all 54,925); a u32 array (`4007121E`); two more u32 (`B58D6510`, 7 on all, and a
//! f32 `8CE175E4`, 1.0 on all) and a u32 `2DDF1266` whose low 16 bits are the bone's position in the array and whose
//! high 16 the number of bones below it, by value.
//!
//! What the bytes are called here rests on the registry's names where it has them and on values where it does not
//! (said at each field); the SkeletonPoseBones' 16 bytes are kept as stored (no field list describes them).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

pub use crate::components::{StoredBone, StoredPose, StoredPoseBone, StoredPoseGroup, StoredSkeleton};
use crate::read::fields::{BodyError, LE, body_error, count, end, expect, f32s, object, reference};
use primase_foundations::bytes::Field;

/// A record type: the CRC-32 of `Skeleton`.
pub const SKELETON: u32 = 0x24AE_CB7C;
/// A Bone's type, a SkeletonPoseGroup's, a SkeletonPose's, a SkeletonPoseBone's, an IKData's and a
/// Reflex3SkeletonConstraints'.
const BONE: u32 = 0x9574_1049;
const SKELETON_POSE_GROUP: u32 = 0xC8F8_ABFF;
const SKELETON_POSE: u32 = 0x209C_F072;
const SKELETON_POSE_BONE: u32 = 0x071C_EE50;
const IK_DATA: u32 = 0x9A33_101E;
const REFLEX3_CONSTRAINTS: u32 = 0x8E3F_B47A;

/// Why a Skeleton body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("skeleton {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

/// The body's cursor ([`primase_foundations::bytes::Cursor`]), with this record's grammar as methods.
struct Cursor<'a>(primase_foundations::bytes::Cursor<'a>);

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cursor(primase_foundations::bytes::Cursor::new(b))
    }
}

impl<'a> std::ops::Deref for Cursor<'a> {
    type Target = primase_foundations::bytes::Cursor<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Cursor<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Cursor<'_> {
    /// A pointer to a bone read before: `03` (none) or `02` and its id.
    fn bone_pointer(&mut self, what: &'static str) -> Result<Option<u64>, Error> {
        let at = self.position();
        match self.u8().field(what)? {
            3 => Ok(None),
            2 => Ok(Some(self.u64(LE).field(what)?)),
            found => Err(BodyError::Value {
                what,
                at,
                found: u64::from(found),
                read: "3 (none) or 2 (a reference)",
            }
            .into()),
        }
    }
    /// An object pointer whose class stores no field or a byte buffer: `03`, or a tag (`00` or `04`) and the header.
    fn optional_object(&mut self, what: &'static str, kind: u32) -> Result<Option<u64>, Error> {
        let at = self.position();
        match self.u8().field(what)? {
            3 => Ok(None),
            0 | 4 => Ok(Some(object(self, what, kind)?)),
            found => Err(BodyError::Value {
                what,
                at,
                found: u64::from(found),
                read: "3 (none) or an object",
            }
            .into()),
        }
    }
}

impl StoredSkeleton {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredSkeleton, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        let kind = c.u32(LE).field("kind")?;
        let n = count(&mut c, "bone count", 13)?;
        let mut bones = Vec::with_capacity(n);
        for _ in 0..n {
            let at = c.position();
            match c.u8().field("bone tag")? {
                0 | 4 => {}
                found => {
                    return Err(BodyError::Value {
                        what: "bone tag",
                        at,
                        found: u64::from(found),
                        read: "0 or 4",
                    }
                    .into());
                }
            }
            let id = object(&mut c, "Bone", BONE)?;
            let name_hash = c.u32(LE).field("bone name hash")?;
            let parent = c.bone_pointer("bone parent")?;
            let mirror = c.bone_pointer("bone mirror")?;
            expect(&mut c, "BoneDisplayData pointer", 3, 1)?;
            let global_position = f32s::<4>(&mut c, "GlobalPosition")?;
            let global_rotation = f32s::<4>(&mut c, "global rotation")?;
            let local_position = f32s::<4>(&mut c, "LocalPosition")?;
            let local_rotation = f32s::<4>(&mut c, "local rotation")?;
            let flag = c.u8().field("bone flag")?;
            let bone_kind = c.u32(LE).field("bone kind")?;
            expect(&mut c, "BoneModifier count", 0, 4)?;
            let h = count(&mut c, "bone hash count", 4)?;
            let hashes = (0..h)
                .map(|_| c.u32(LE).field("bone hash"))
                .collect::<Result<_, _>>()?;
            let kind2 = c.u32(LE).field("bone kind 2")?;
            let unnamed = c.f32(LE).field("bone float")?;
            let key = c.u32(LE).field("bone key")?;
            bones.push(StoredBone {
                id,
                name_hash,
                parent,
                mirror,
                global_position,
                global_rotation,
                local_position,
                local_rotation,
                flag,
                kind: bone_kind,
                hashes,
                kind2,
                unnamed,
                key,
            });
        }
        let root = c.bone_pointer("root")?;
        let position = f32s::<4>(&mut c, "skeleton position")?;
        let rotation = f32s::<4>(&mut c, "skeleton rotation")?;
        let body_part_mapping = reference(&mut c)?.map(|r| r.id);
        let groups = count(&mut c, "pose group count", 12)?;
        let mut pose_groups = Vec::with_capacity(groups);
        for _ in 0..groups {
            let id = object(&mut c, "SkeletonPoseGroup", SKELETON_POSE_GROUP)?;
            let poses_n = count(&mut c, "pose count", 12)?;
            let mut poses = Vec::with_capacity(poses_n);
            for _ in 0..poses_n {
                let id = object(&mut c, "SkeletonPose", SKELETON_POSE)?;
                let value = c.f32(LE).field("pose value")?;
                let bones_n = count(&mut c, "pose bone count", 28)?;
                let mut pose_bones = Vec::with_capacity(bones_n);
                for _ in 0..bones_n {
                    let id = object(&mut c, "SkeletonPoseBone", SKELETON_POSE_BONE)?;
                    pose_bones.push(StoredPoseBone {
                        id,
                        bytes: c.array::<16>().field("SkeletonPoseBone")?,
                    });
                }
                poses.push(StoredPose {
                    id,
                    value,
                    bones: pose_bones,
                });
            }
            pose_groups.push(StoredPoseGroup { id, poses });
        }
        let ik_data = c.optional_object("IKData", IK_DATA)?;
        let look_at = reference(&mut c)?.map(|r| r.id);
        expect(&mut c, "ReflexSystem pointer", 3, 1)?;
        let floats = [
            c.f32(LE).field("skeleton float")?,
            c.f32(LE).field("skeleton float")?,
        ];
        let reflex3 = match c.optional_object("Reflex3SkeletonConstraints", REFLEX3_CONSTRAINTS)? {
            None => None,
            Some(id) => {
                let len = count(&mut c, "Reflex3SkeletonConstraints bytes", 1)?;
                Some((
                    id,
                    c.take(len).field("Reflex3SkeletonConstraints bytes")?.to_vec(),
                ))
            }
        };
        let h = count(&mut c, "skeleton hash count", 4)?;
        let hashes = (0..h)
            .map(|_| c.u32(LE).field("skeleton hash"))
            .collect::<Result<_, _>>()?;
        end(&c)?;
        Ok(StoredSkeleton {
            kind,
            bones,
            root,
            position,
            rotation,
            body_part_mapping,
            pose_groups,
            ik_data,
            look_at,
            floats,
            reflex3,
            hashes,
        })
    }
}
