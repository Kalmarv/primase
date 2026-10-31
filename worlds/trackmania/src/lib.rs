//! TrackMania Nations Forever over the GameBox reader (`gbx-formats`): a map (`CGameCtnChallenge`) read as the game
//! builds it, each block placed from the game's own block model (`CGameCtnBlockInfo*` in its packs and `GameData`)
//! with the meshes, materials, textures and collision of the solids its mobils show, and the terrain the map leaves to
//! its collection's default zone ([`read::TrackmaniaReader`]).
//!
//! The reader states what the game stores: its own `trackmania.*` components carry the map, the blocks, the materials
//! with their shaders and texture slots, the solids and the spawn locations beside the core data made of them. What a
//! material looks like in a destination is a lowering's choice.
//!
//! # Frame
//!
//! The game's frame is metres, +Y up, its block grid's +X and +Z the map's columns and rows, and right-handed: the
//! world's own, so [`FRAME`] is the identity. Its texts are the evidence: read with Z negated, the start's banner and
//! the START lettering on its barriers show mirrored, and as stored they read the right way round. A block's coordinate
//! is in squares of the collection's size (Stadium: 32 m by 8 m high), and its direction turns it about +Y, each step
//! taking +X to +Z.
//!
//! # Texture coordinates
//!
//! The game's texture coordinates have their origin at the image's bottom left (v up), as its map thumbnails store
//! their rows bottom first: with v as stored, every text draws upside down. The reader stores `(u, 1 - v)`, the world's
//! top-left origin; the images are kept as their files store them, top row first.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

use primase_core::frame::{Axis, SourceFrame};

pub use read::{MapReport, TrackmaniaReader};

/// The game's frame in world terms ([Frame](crate#frame)).
pub const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::PosZ], 1.0);

/// Registers TrackMania's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.add(&passes::LOWER_TRACKMANIA_MATERIALS);
    reg.add(&passes::LOWER_TRACKMANIA_SPAWNS);
    reg.add(&passes::LOWER_TRACKMANIA_SURFACES);
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Gbx(#[from] gbx_formats::Error),
    #[error(transparent)]
    Dds(#[from] primase_dds::Error),
    /// The game's data is not what the reader needs to go on (no block data, no collection to place it with).
    #[error("{0}")]
    Format(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
