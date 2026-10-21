//! A terrain's material bank and its materials: TerrainMaterialBank, the list a terrain node's material raster
//! (block 2) indexes, and TerrainMaterial, each one's CollisionMaterial. Little-endian; a reference is
//! [`crate::read::reference`]'s.
//!
//! - **TerrainMaterialBank:** `01`, a u32 count and that many references (TextureSets), a u32 count and that many
//!   references (TerrainMaterials), a u32 count and that many references (region objects), one reference (a
//!   TextureMap).
//! - **TerrainMaterial** (202 bytes): its bytes kept as stored; at 180 a reference to its CollisionMaterial, at 198 a
//!   u32 (its position in the bank, by the notes).
//!
//! The grammars are the Riders Republic notes' (docs/terrain.md §4 and §3.4: TMB_MAD consumed exactly; 202 bytes on
//! every TerrainMaterial), read on every one of the game's by the census (worlds/anvil/tests/census.rs). That a node's
//! raster value indexes the bank's TerrainMaterials in stored order is theirs too (each value's mean colour and slope
//! against its material's name, over 87 million samples). The rest of a TerrainMaterial isn't read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A record type: the CRC-32 of `TerrainMaterialBank`.
pub const TERRAIN_MATERIAL_BANK: u32 = 0x4BE9_4285;
/// A record type: the CRC-32 of `TerrainMaterial`.
pub const TERRAIN_MATERIAL: u32 = 0x0172_4188;
/// A record type: the CRC-32 of `CollisionMaterial`.
pub const COLLISION_MATERIAL: u32 = 0x74F7_311D;

/// A TerrainMaterial's length.
const MATERIAL_BYTES: usize = 202;
/// Where its CollisionMaterial reference starts.
const COLLISION_AT: usize = 180;
/// Where its u32 index starts.
const INDEX_AT: usize = 198;

/// Why a bank or material can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terrain material body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("terrain material {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("terrain material reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    #[error("terrain material body: {0} bytes after its end")]
    Trailing(usize),
}

/// Bytes read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(N).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s: [u8; N] = self
            .b
            .get(self.at..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn reference(&mut self) -> Result<Option<Reference>, Error> {
        let at = self.at;
        reference::parse(self.array::<{ reference::LENGTH }>("reference")?)
            .map_err(|source| Error::Reference { at, source })
    }
    /// A u32 count and that many references, none of them `03`.
    fn references(&mut self, what: &'static str) -> Result<Vec<Reference>, Error> {
        let at = self.at;
        let n = usize::try_from(u32::from_le_bytes(self.array(what)?)).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(reference::LENGTH).is_none_or(|bytes| bytes > left) {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        (0..n)
            .map(|_| {
                let at = self.at;
                self.reference()?.ok_or(Error::Value {
                    what,
                    at,
                    found: 3,
                    read: "a reference (tag 1)",
                })
            })
            .collect()
    }
}

/// A TerrainMaterialBank as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredMaterialBank {
    pub details: Vec<Reference>,
    /// The TerrainMaterials, in the order a material raster's values index them.
    pub materials: Vec<Reference>,
    pub regions: Vec<Reference>,
    pub last: Option<Reference>,
}

impl StoredMaterialBank {
    pub fn parse(body: &[u8]) -> Result<StoredMaterialBank, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let [first] = c.array::<1>("first byte")?;
        if first != 1 {
            return Err(Error::Value {
                what: "first byte",
                at: 0,
                found: u64::from(first),
                read: "1",
            });
        }
        let bank = StoredMaterialBank {
            details: c.references("detail references")?,
            materials: c.references("material references")?,
            regions: c.references("region references")?,
            last: c.reference()?,
        };
        match body.len().saturating_sub(c.at) {
            0 => Ok(bank),
            left => Err(Error::Trailing(left)),
        }
    }
}

/// A TerrainMaterial as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTerrainMaterial {
    pub bytes: Vec<u8>,
    pub collision_material: Option<Reference>,
    pub index: u32,
}

impl StoredTerrainMaterial {
    pub fn parse(body: &[u8]) -> Result<StoredTerrainMaterial, Error> {
        if body.len() != MATERIAL_BYTES {
            return Err(Error::Value {
                what: "length",
                at: 0,
                found: u64::try_from(body.len()).unwrap_or(u64::MAX),
                read: "202",
            });
        }
        let mut c = Cursor {
            b: body,
            at: COLLISION_AT,
        };
        let collision_material = c.reference()?;
        let mut c = Cursor {
            b: body,
            at: INDEX_AT,
        };
        let index = u32::from_le_bytes(c.array("index")?);
        Ok(StoredTerrainMaterial {
            bytes: body.to_vec(),
            collision_material,
            index,
        })
    }
}
