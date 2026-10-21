//! A WaterComponent's lake data: the water patches a lake's surface is made of and the atlases its shader samples.
//! Little-endian; a reference is [`crate::read::reference`]'s.
//!
//! The body (docs/entity-grammar.md §8.11, docs/material-spec.md §24.3; its length rule in [`crate::read::entity`]):
//! 4 bytes; a reference (its WaterMesh); a WaterMeshInstance (`00`, an object `0xC15B678F`, a count k and k 64-byte
//! items); a bounding volume (40 bytes), 10 bytes, the 125-byte Visual tail; a RiverQuadTree (an object `0x92DB01AE`,
//! 33 bytes, a count n and n 61-byte nodes); a byte; a count m and m 49-byte WaterPatchDescs; three references (the
//! atlases). A WaterPatchDesc: `00`, a u64 and a u32 type; its position (3 f32: x and y in metres, z the water
//! level) at 13; 4 bytes at 25; its rectangle in the atlases (4 f32: u0, v0, u1, v1) at 29; its level (a u32) at 45:
//! a square of side `512 >> level` metres from its position (the notes: 2,062 patches of the 35 lakes, levels 3 to
//! 6). The three references are the Flow, Params and LocalProfile atlases, in that order (by their entries' names).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A component type: WaterComponent.
pub const WATER_COMPONENT: u32 = 0x996D_F94F;

/// Why a WaterComponent body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("WaterComponent body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("WaterComponent {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("WaterComponent reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("WaterComponent body: {0} bytes after its end")]
    Trailing(usize),
}

/// One WaterPatchDesc.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterPatch {
    /// Its object's u64 and type, as stored.
    pub id: u64,
    pub kind: u32,
    /// x and y in metres, z the water level.
    pub position: [f32; 3],
    /// The 4 bytes at 25, as stored (`00 00 01 02` on the notes' samples).
    pub unread: [u8; 4],
    /// u0, v0, u1, v1 in the atlases.
    pub rect: [f32; 4],
    pub level: u32,
}

impl WaterPatch {
    /// Its square's side in metres: `512 >> level` (`None` past 512's bits).
    pub fn side(&self) -> Option<f32> {
        512u32.checked_shr(self.level).map(|s| s as f32)
    }
}

/// A WaterComponent's body as stored, its lake data read.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredWater {
    /// Its WaterMesh.
    pub mesh: Option<Reference>,
    /// The byte before the patches (1 on the notes' lakes), as stored.
    pub byte: u8,
    pub patches: Vec<WaterPatch>,
    /// Flow, Params and LocalProfile.
    pub atlases: [Option<Reference>; 3],
}

/// Bytes read in order, each bounded.
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
        let s = self.take(N, what)?;
        s.try_into().map_err(|_| Error::Short {
            what,
            at,
            have: s.len(),
        })
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(u8::from_le_bytes(self.array(what)?))
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
    fn expect(&mut self, what: &'static str, want: u64, found: u64, at: usize) -> Result<(), Error> {
        if want == found {
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
    fn object(&mut self, what: &'static str, kind: u32) -> Result<u64, Error> {
        let id = self.u64(what)?;
        let at = self.at;
        let found = self.u32(what)?;
        self.expect(what, u64::from(kind), u64::from(found), at)?;
        Ok(id)
    }
    fn tag(&mut self, what: &'static str, want: u8) -> Result<(), Error> {
        let at = self.at;
        let found = self.u8(what)?;
        self.expect(what, u64::from(want), u64::from(found), at)
    }
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        if n.checked_mul(each.max(1))
            .is_none_or(|bytes| bytes > self.b.len())
        {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        let bytes = self.array::<{ reference::LENGTH }>("reference")?;
        reference::parse(bytes).map_err(|source| Error::Reference { at, source })
    }
}

impl StoredWater {
    /// The lake data of a WaterComponent's body (the bytes after its component header), read to its end.
    pub fn parse(body: &[u8]) -> Result<StoredWater, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.take(4, "head")?;
        let mesh = c.reference()?;
        c.tag("WaterMeshInstance", 0)?;
        c.object("WaterMeshInstance", 0xC15B_678F)?;
        let k = c.count("WaterMeshInstance count", 64)?;
        c.take(k.saturating_mul(64), "WaterMeshInstance")?;
        c.take(40 + 10 + 125, "bounds and Visual tail")?;
        c.object("RiverQuadTree", 0x92DB_01AE)?;
        c.take(33, "RiverQuadTree")?;
        let n = c.count("RiverQuadTree node count", 61)?;
        c.take(n.saturating_mul(61), "RiverQuadTreeNode")?;
        let byte = c.u8("byte before the patches")?;
        let m = c.count("WaterPatchDesc count", 49)?;
        let mut patches = Vec::with_capacity(m);
        for _ in 0..m {
            c.tag("WaterPatchDesc", 0)?;
            let id = c.u64("WaterPatchDesc")?;
            let kind = c.u32("WaterPatchDesc type")?;
            let position = [c.f32("position")?, c.f32("position")?, c.f32("position")?];
            let unread = c.array("WaterPatchDesc")?;
            let rect = [c.f32("rect")?, c.f32("rect")?, c.f32("rect")?, c.f32("rect")?];
            let level = c.u32("level")?;
            patches.push(WaterPatch {
                id,
                kind,
                position,
                unread,
                rect,
                level,
            });
        }
        let atlases = [c.reference()?, c.reference()?, c.reference()?];
        match body.len().saturating_sub(c.at) {
            0 => Ok(StoredWater {
                mesh,
                byte,
                patches,
                atlases,
            }),
            left => Err(Error::Trailing(left)),
        }
    }
}
