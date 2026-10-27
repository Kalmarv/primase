//! A WaterComponent's lake data: the water patches a lake's surface is made of and the atlases its shader samples.
//! Little-endian; a reference is [`crate::reference`]'s.
//!
//! The body (docs/entity-grammar.md §8.11, docs/material-spec.md §24.3; its length rule in [`crate::entity`]):
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

use crate::fields::{BodyError, LE, body_error, count, end, expect, object, reference};
use crate::reference::Reference;
use primase_foundations::bytes::{Cursor, Field};

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

/// A component type: WaterComponent.
pub const WATER_COMPONENT: u32 = 0x996D_F94F;

/// Why a WaterComponent body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("WaterComponent {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

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

impl StoredWater {
    /// The lake data of a WaterComponent's body (the bytes after its component header), read to its end.
    pub fn parse(body: &[u8]) -> Result<StoredWater, Error> {
        let mut c = Cursor::new(body);
        c.take(4).field("head")?;
        let mesh = reference(&mut c)?;
        expect(&mut c, "WaterMeshInstance", 0, 1)?;
        object(&mut c, "WaterMeshInstance", 0xC15B_678F)?;
        let k = count(&mut c, "WaterMeshInstance count", 64)?;
        c.take(k.saturating_mul(64)).field("WaterMeshInstance")?;
        c.take(40 + 10 + 125).field("bounds and Visual tail")?;
        object(&mut c, "RiverQuadTree", 0x92DB_01AE)?;
        c.take(33).field("RiverQuadTree")?;
        let n = count(&mut c, "RiverQuadTree node count", 61)?;
        c.take(n.saturating_mul(61)).field("RiverQuadTreeNode")?;
        let byte = c.u8().field("byte before the patches")?;
        let m = count(&mut c, "WaterPatchDesc count", 49)?;
        let mut patches = Vec::with_capacity(m);
        for _ in 0..m {
            expect(&mut c, "WaterPatchDesc", 0, 1)?;
            let id = c.u64(LE).field("WaterPatchDesc")?;
            let kind = c.u32(LE).field("WaterPatchDesc type")?;
            let position = [
                c.f32(LE).field("position")?,
                c.f32(LE).field("position")?,
                c.f32(LE).field("position")?,
            ];
            let unread = c.array().field("WaterPatchDesc")?;
            let rect = [
                c.f32(LE).field("rect")?,
                c.f32(LE).field("rect")?,
                c.f32(LE).field("rect")?,
                c.f32(LE).field("rect")?,
            ];
            let level = c.u32(LE).field("level")?;
            patches.push(WaterPatch {
                id,
                kind,
                position,
                unread,
                rect,
                level,
            });
        }
        let atlases = [reference(&mut c)?, reference(&mut c)?, reference(&mut c)?];
        end(&c)?;
        Ok(StoredWater {
            mesh,
            byte,
            patches,
            atlases,
        })
    }
}
