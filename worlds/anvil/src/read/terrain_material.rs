//! A terrain's material bank and its materials: TerrainMaterialBank, the list a terrain node's material raster
//! (block 2) indexes, and TerrainMaterial, each one's CollisionMaterial. Little-endian; a reference is
//! [`crate::read::reference`]'s.
//!
//! - **TerrainMaterialBank:** `01`, a u32 count and that many references (TextureSets), a u32 count and that many
//!   references (TerrainMaterials), a u32 count and that many references (region objects), one reference (a
//!   TextureMap).
//! - **TerrainMaterial** (202 bytes): its bytes kept as stored; at 2 an f32, the detail's uv scale (the engine's
//!   per-material record holds it, the notes' read of the runtime array: docs/terrain.md §3.5 (a)); at 6 an f32 (1 to
//!   200: the detail tile's size in metres, a lead); at 17 a u64, its TextureSet's id (bytes 15 and 16 zero, not the
//!   reference grammar; the detail maps: slot 0 the diffuse on 103 of 105, by the notes); at 180 a reference to its
//!   CollisionMaterial, at 198 a u32 (its position in the bank, by the notes).
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

use crate::read::fields::{BodyError, LE, body_error, count, end, reference};
use crate::read::reference::{self, Reference};
use primase_foundations::bytes::{Cursor, Field, View};

/// A record type: the CRC-32 of `TerrainMaterialBank`.
pub const TERRAIN_MATERIAL_BANK: u32 = 0x4BE9_4285;
/// A record type: the CRC-32 of `TerrainMaterial`.
pub const TERRAIN_MATERIAL: u32 = 0x0172_4188;
/// A record type: the CRC-32 of `CollisionMaterial`.
pub const COLLISION_MATERIAL: u32 = 0x74F7_311D;

/// A TerrainMaterial's length.
const MATERIAL_BYTES: usize = 202;
/// Where its f32s and TextureSet reference start.
const UV_SCALE_AT: usize = 2;
const F32_6_AT: usize = 6;
const TEXTURE_SET_AT: usize = 17;
/// Where its CollisionMaterial reference starts.
const COLLISION_AT: usize = 180;
/// Where its u32 index starts.
const INDEX_AT: usize = 198;

/// Why a bank or material can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terrain material {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

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
        let mut c = Cursor::new(body);
        let [first] = c.array::<1>().field("first byte")?;
        if first != 1 {
            return Err(BodyError::Value {
                what: "first byte",
                at: 0,
                found: u64::from(first),
                read: "1",
            }
            .into());
        }
        let bank = StoredMaterialBank {
            details: references(&mut c, "detail references")?,
            materials: references(&mut c, "material references")?,
            regions: references(&mut c, "region references")?,
            last: reference(&mut c)?,
        };
        end(&c)?;
        Ok(bank)
    }
}

/// A TerrainMaterial as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredTerrainMaterial {
    pub bytes: Vec<u8>,
    /// The f32 at 2: the detail's uv scale.
    pub uv_scale: f32,
    /// The f32 at 6 (its meaning a lead: the detail tile's size in metres).
    pub f32_6: f32,
    /// The u64 at 17, its TextureSet's id (`None` where 0).
    pub texture_set: Option<u64>,
    pub collision_material: Option<Reference>,
    pub index: u32,
}

impl StoredTerrainMaterial {
    pub fn parse(body: &[u8]) -> Result<StoredTerrainMaterial, Error> {
        if body.len() != MATERIAL_BYTES {
            return Err(BodyError::Value {
                what: "length",
                at: 0,
                found: u64::try_from(body.len()).unwrap_or(u64::MAX),
                read: "202",
            }
            .into());
        }
        let v = View::new(body);
        let (uv_scale, f32_6) = (
            v.f32(UV_SCALE_AT, LE).field("f32")?,
            v.f32(F32_6_AT, LE).field("f32")?,
        );
        let texture_set = Some(v.u64(TEXTURE_SET_AT, LE).field("texture set id")?).filter(|&id| id != 0);
        let mut c = Cursor::new(body);
        c.seek(COLLISION_AT).field("collision material")?;
        let collision_material = reference(&mut c)?;
        let index = v.u32(INDEX_AT, LE).field("index")?;
        Ok(StoredTerrainMaterial {
            bytes: body.to_vec(),
            uv_scale,
            f32_6,
            texture_set,
            collision_material,
            index,
        })
    }
}

/// A u32 count of references the rest of the body holds, each one present (tag 1).
fn references(c: &mut Cursor, what: &'static str) -> Result<Vec<Reference>, BodyError> {
    let n = count(c, what, reference::LENGTH)?;
    (0..n)
        .map(|_| {
            let at = c.position();
            reference(c)?.ok_or(BodyError::Value {
                what,
                at,
                found: 3,
                read: "a reference (tag 1)",
            })
        })
        .collect()
}
