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

use crate::read::reference::{self, Reference};

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
    #[error("skeleton body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("skeleton {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("skeleton reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("skeleton body: {0} bytes after its end")]
    Trailing(usize),
}

/// Little-endian fields read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&[u8], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(n).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s = self.b.get(self.at..end).ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let at = self.at;
        self.take(N, what)?.try_into().map_err(|_| Error::Short {
            what,
            at,
            have: self.b.len(),
        })
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        let [b] = self.array::<1>(what)?;
        Ok(b)
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }
    fn f32x4(&mut self, what: &'static str) -> Result<[f32; 4], Error> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }
    fn expect(&mut self, what: &'static str, want: u64, width: usize) -> Result<(), Error> {
        let at = self.at;
        let found = match width {
            1 => u64::from(self.u8(what)?),
            4 => u64::from(self.u32(what)?),
            _ => self.u64(what)?,
        };
        if found == want {
            Ok(())
        } else {
            Err(Error::Value {
                what,
                at,
                found,
                read: "the grammar's value",
            })
        }
    }
    /// An embedded object's header: a u64 id, whose type must be `kind`.
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        self.expect(what, u64::from(kind), 4)?;
        Ok(id)
    }
    /// A count of items of `each` bytes the rest of the body can hold.
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(each.max(1)).is_none_or(|bytes| bytes > left) {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
    }
    /// A pointer to a bone read before: `03` (none) or `02` and its id.
    fn bone_pointer(&mut self, what: &'static str) -> Result<Option<u64>, Error> {
        let at = self.at;
        match self.u8(what)? {
            3 => Ok(None),
            2 => Ok(Some(self.u64(what)?)),
            found => Err(Error::Value {
                what,
                at,
                found: u64::from(found),
                read: "3 (none) or 2 (a reference)",
            }),
        }
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        reference::parse(self.array::<{ reference::LENGTH }>("reference")?)
            .map_err(|source| Error::Reference { at, source })
    }
    /// An object pointer whose class stores no field or a byte buffer: `03`, or a tag (`00` or `04`) and the header.
    fn optional_object(&mut self, what: &'static str, kind: u32) -> Result<Option<u64>, Error> {
        let at = self.at;
        match self.u8(what)? {
            3 => Ok(None),
            0 | 4 => Ok(Some(self.object(what, kind)?)),
            found => Err(Error::Value {
                what,
                at,
                found: u64::from(found),
                read: "3 (none) or an object",
            }),
        }
    }
}

/// A bone as stored (see the module documentation for what each field is by the registry and by value).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredBone {
    /// The object's id: what a parent, the root and a mirror name.
    pub id: u64,
    /// The CRC-32 of the bone's name (the registry's two packed bit fields `41B24805` and `778FF80E`, 32 bits).
    pub name_hash: u32,
    /// The parent's id (none for a root).
    pub parent: Option<u64>,
    /// `230C4503`: the bone itself on a chain's bones, the opposite side's bone on a paired one, by value.
    pub mirror: Option<u64>,
    /// GlobalPosition: x, y, z and 0.
    pub global_position: [f32; 4],
    /// `02A2A3E3`, by value the global rotation: a unit quaternion x, y, z, w.
    pub global_rotation: [f32; 4],
    /// LocalPosition: in the parent's space, x, y, z and 0.
    pub local_position: [f32; 4],
    /// `8AE955A2`, by value the local rotation: a unit quaternion x, y, z, w.
    pub local_rotation: [f32; 4],
    /// `EF07E0C9`: 0 on all but one of the 11,113 bones read.
    pub flag: u8,
    /// `73196C8B`, an enum: 0 on 10,890 of 11,113 bones of the census, 1 to 5, 7, 8 and 14 on the others.
    pub kind: u32,
    /// `4007121E`, an array of u32 (a hash list like the skeleton's last array).
    pub hashes: Vec<u32>,
    /// `B58D6510`, an enum: 7 on every bone.
    pub kind2: u32,
    /// `8CE175E4`: 1.0 on every bone.
    pub unnamed: f32,
    /// `2DDF1266`: the low 16 bits the bone's position in the skeleton, the high 16 the bones below it, by value.
    pub key: u32,
}

/// One SkeletonPoseBone: its object's id and its 16 bytes as stored (no field list of the registry describes them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPoseBone {
    pub id: u64,
    pub bytes: [u8; 16],
}

/// One SkeletonPose: a f32 and its bones.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPose {
    pub id: u64,
    pub value: f32,
    pub bones: Vec<StoredPoseBone>,
}

/// One SkeletonPoseGroup: its poses.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPoseGroup {
    pub id: u64,
    pub poses: Vec<StoredPose>,
}

/// A Skeleton body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSkeleton {
    /// `9C78EB24`, an enum.
    pub kind: u32,
    pub bones: Vec<StoredBone>,
    /// `4E1F230B`: the root bone's id.
    pub root: Option<u64>,
    /// `F4D6F079`: the skeleton's own position (x, y, z, 0).
    pub position: [f32; 4],
    /// `9B868C7D`: its own rotation (a quaternion x, y, z, w by value).
    pub rotation: [f32; 4],
    pub body_part_mapping: Option<Reference>,
    pub pose_groups: Vec<StoredPoseGroup>,
    /// The IKData object's id where the skeleton has one (the class stores no field).
    pub ik_data: Option<u64>,
    pub look_at: Option<Reference>,
    /// `F656FC33` and `5FC3FE25`.
    pub floats: [f32; 2],
    /// The Reflex3SkeletonConstraints object's id and its byte buffer (8 bytes on every skeleton that has one).
    pub reflex3: Option<(u64, Vec<u8>)>,
    /// `3708B98C`, an array of u32.
    pub hashes: Vec<u32>,
}

impl StoredSkeleton {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredSkeleton, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.expect("first byte", 1, 1)?;
        let kind = c.u32("kind")?;
        let n = c.count("bone count", 13)?;
        let mut bones = Vec::with_capacity(n);
        for _ in 0..n {
            let at = c.at;
            match c.u8("bone tag")? {
                0 | 4 => {}
                found => {
                    return Err(Error::Value {
                        what: "bone tag",
                        at,
                        found: u64::from(found),
                        read: "0 or 4",
                    });
                }
            }
            let id = c.object("Bone", BONE)?;
            let name_hash = c.u32("bone name hash")?;
            let parent = c.bone_pointer("bone parent")?;
            let mirror = c.bone_pointer("bone mirror")?;
            c.expect("BoneDisplayData pointer", 3, 1)?;
            let global_position = c.f32x4("GlobalPosition")?;
            let global_rotation = c.f32x4("global rotation")?;
            let local_position = c.f32x4("LocalPosition")?;
            let local_rotation = c.f32x4("local rotation")?;
            let flag = c.u8("bone flag")?;
            let bone_kind = c.u32("bone kind")?;
            c.expect("BoneModifier count", 0, 4)?;
            let h = c.count("bone hash count", 4)?;
            let hashes = (0..h).map(|_| c.u32("bone hash")).collect::<Result<_, _>>()?;
            let kind2 = c.u32("bone kind 2")?;
            let unnamed = c.f32("bone float")?;
            let key = c.u32("bone key")?;
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
        let position = c.f32x4("skeleton position")?;
        let rotation = c.f32x4("skeleton rotation")?;
        let body_part_mapping = c.reference()?;
        let groups = c.count("pose group count", 12)?;
        let mut pose_groups = Vec::with_capacity(groups);
        for _ in 0..groups {
            let id = c.object("SkeletonPoseGroup", SKELETON_POSE_GROUP)?;
            let poses_n = c.count("pose count", 12)?;
            let mut poses = Vec::with_capacity(poses_n);
            for _ in 0..poses_n {
                let id = c.object("SkeletonPose", SKELETON_POSE)?;
                let value = c.f32("pose value")?;
                let bones_n = c.count("pose bone count", 28)?;
                let mut pose_bones = Vec::with_capacity(bones_n);
                for _ in 0..bones_n {
                    let id = c.object("SkeletonPoseBone", SKELETON_POSE_BONE)?;
                    pose_bones.push(StoredPoseBone {
                        id,
                        bytes: c.array::<16>("SkeletonPoseBone")?,
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
        let look_at = c.reference()?;
        c.expect("ReflexSystem pointer", 3, 1)?;
        let floats = [c.f32("skeleton float")?, c.f32("skeleton float")?];
        let reflex3 = match c.optional_object("Reflex3SkeletonConstraints", REFLEX3_CONSTRAINTS)? {
            None => None,
            Some(id) => {
                let len = c.count("Reflex3SkeletonConstraints bytes", 1)?;
                Some((id, c.take(len, "Reflex3SkeletonConstraints bytes")?.to_vec()))
            }
        };
        let h = c.count("skeleton hash count", 4)?;
        let hashes = (0..h).map(|_| c.u32("skeleton hash")).collect::<Result<_, _>>()?;
        match c.b.len().saturating_sub(c.at) {
            0 => {}
            left => return Err(Error::Trailing(left)),
        }
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
