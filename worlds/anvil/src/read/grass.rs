//! The grass library: a GrassPatternLibrary's models (one settings record each) and its groups of rules. What it
//! stores is read as stored; what the engine does with it (where plants grow and how many) is not stored in any forge
//! resource: the engine makes the instances at run time (docs/grass.md §0, §5).
//!
//! The body (docs/grass.md §2.2, docs/vegetation.md §3.1; consumed to the last byte on the one library of Riders
//! Republic, 30,247 bytes): `01`, a u32 model count n, `00`, then n times a GrassModel's settings object (a u64, 0xDDB820C0
//! and a 60-byte body) and a BoundingVolume object (a u64, 0x4AEC3476, six f32 and 24 bytes); a u32 0; the groups, each a
//! GrassPattern object (a u64, 0xBDD96F63, a u32 rule count r, `00`) and r GrassPatternElements (a u64, 0xA6FC073D and 82
//! bytes); then 443 bytes: a 256-byte table (the group of a material, `FF` for none) and 187 bytes.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::fields::{BodyError, LE, body_error, count, expect, object};
use primase_bytes::{Cursor, Field};

pub use crate::components::{
    GrassModelSettings, StoredGrassGroup, StoredGrassLibrary, StoredGrassModel, StoredGrassRule,
};

/// The four f32 after the LOD table in a model's 23 unnamed bytes (at 5, 9, 13 and 17): its LOD switch distances in
/// metres, by the notes' reading by value.
fn switch_distances(unnamed: &[u8; 23]) -> [f32; 4] {
    [5usize, 9, 13, 17].map(|at| {
        unnamed
            .get(at..at.saturating_add(4))
            .and_then(|w| <[u8; 4]>::try_from(w).ok())
            .map_or(0.0, f32::from_le_bytes)
    })
}

/// A record type: the CRC-32 of `GrassPatternLibrary`.
pub const GRASS_PATTERN_LIBRARY: u32 = 0x463C_C1B8;
/// A GrassModel's type, a GrassPattern's, a GrassPatternElement's and a BoundingVolume's.
const GRASS_MODEL: u32 = 0xDDB8_20C0;
const GRASS_PATTERN: u32 = 0xBDD9_6F63;
const GRASS_PATTERN_ELEMENT: u32 = 0xA6FC_073D;
const BOUNDING_VOLUME: u32 = 0x4AEC_3476;

/// Why a library body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("grass library {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

impl StoredGrassLibrary {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredGrassLibrary, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        let n = count(&mut c, "model count", 132)?;
        expect(&mut c, "model array", 0, 1)?;
        let mut models = Vec::with_capacity(n);
        for _ in 0..n {
            let id = object(&mut c, "GrassModel", GRASS_MODEL)?;
            let mut settings = {
                let f = |c: &mut Cursor<'_>| c.f32(LE).field("GrassModel");
                GrassModelSettings {
                    z_offset: f(&mut c)?,
                    height_scale_for_occlusion: f(&mut c)?,
                    terrain_normal_follower_ratio: f(&mut c)?,
                    root_height: f(&mut c)?,
                    softness: f(&mut c)?,
                    normal_bend_factor: f(&mut c)?,
                    visible_virtual_lod_count: c.u8().field("GrassModel")?,
                    texture_atlas_element_count: c.u8().field("GrassModel")?,
                    raycast_translucency: c.u32(LE).field("GrassModel")?,
                    imposter_shader_start_model_lod_index: c.array().field("GrassModel")?,
                    imposter_lookat_ratio: c.f32(LE).field("GrassModel")?,
                    unnamed: c.array().field("GrassModel")?,
                    switch_distances: [0.0; 4],
                }
            };
            settings.switch_distances = switch_distances(&settings.unnamed);
            let bounds_id = object(&mut c, "GrassModel bounds", BOUNDING_VOLUME)?;
            let mut bounds = [0.0f32; 6];
            for v in &mut bounds {
                *v = c.f32(LE).field("GrassModel bounds")?;
            }
            let bounds_rest = c.array().field("GrassModel bounds")?;
            models.push(StoredGrassModel {
                id,
                settings,
                bounds_id,
                bounds,
                bounds_rest,
            });
        }
        expect(&mut c, "word after the models", 0, 4)?;
        let mut groups = Vec::new();
        // the groups run on while an object of type GrassPattern follows; the 443 bytes after them begin with a table
        // of 256 bytes
        while c
            .bytes()
            .get(c.position().saturating_add(8)..c.position().saturating_add(12))
            == Some(&GRASS_PATTERN.to_le_bytes())
        {
            let id = object(&mut c, "GrassPattern", GRASS_PATTERN)?;
            let r = count(&mut c, "rule count", 94)?;
            expect(&mut c, "GrassPattern byte", 0, 1)?;
            let mut rules = Vec::with_capacity(r);
            for _ in 0..r {
                let id = object(&mut c, "GrassPatternElement", GRASS_PATTERN_ELEMENT)?;
                rules.push(StoredGrassRule {
                    id,
                    model: c.u16(LE).field("GrassPatternElement")?,
                    tiling_log_count: c.u8().field("GrassPatternElement")?,
                    density: c.f32(LE).field("GrassPatternElement")?,
                    tilt_factor: c.f32(LE).field("GrassPatternElement")?,
                    scaling_enabled: c.u8().field("GrassPatternElement")? != 0,
                    scale_a: c.f32(LE).field("GrassPatternElement")?,
                    scale_b: c.f32(LE).field("GrassPatternElement")?,
                    height_scaling_enabled: c.u8().field("GrassPatternElement")? != 0,
                    height_scale_a: c.f32(LE).field("GrassPatternElement")?,
                    height_scale_b: c.f32(LE).field("GrassPatternElement")?,
                    unnamed_f32: c.f32(LE).field("GrassPatternElement")?,
                    unnamed: c.array().field("GrassPatternElement")?,
                });
            }
            groups.push(StoredGrassGroup { id, rules });
        }
        let table = c.array::<256>().field("table")?;
        let rest = c.take(c.remaining()).field("the bytes after the table")?.to_vec();
        Ok(StoredGrassLibrary {
            models,
            groups,
            table,
            rest,
        })
    }
}
