//! Forza Horizon 6 (ForzaTech) over `forzatech-formats`: a track's placements in an x/z box and one season, their
//! models, materials and textures, read as stored and converted to the world's frame.
//!
//! The game's frame is Y-up and left-handed in metres, +X east and +Z north (the pause map's row 0 is its largest z):
//! the world's (`x, y, -z`), [`FRAME`], which mirrors, so a mesh's triangles are reversed on the way in.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use primase_core::frame::{Axis, SourceFrame};

pub mod components;
pub mod passes;
pub mod read;

/// Register Forza Horizon 6's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.add(&passes::COLLIDE_FORZATECH_DRAWN);
    reg.add(&passes::LOWER_FORZATECH_MATERIALS);
    reg.add(&passes::LOWER_FORZATECH_PLANAR_UV);
    reg.add(&passes::APPLY_FORZATECH_EMISSION_VARIATION);
    reg.add(&passes::BAKE_FORZATECH_EMISSION_PRODUCT);
    reg.add(&passes::BAKE_FORZATECH_SWATCH_TINT);
    reg.add(&passes::SELECT_FORZATECH_DRAWN);
    reg.add(&passes::SELECT_FORZATECH_MAIN_VIEW);
}

/// The game's frame in the world's terms: `world = (x, y, -z)`, metres.
pub const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(String),
    #[error(transparent)]
    Format(#[from] forzatech_formats::Error),
    #[error("{0}")]
    Track(String),
    #[error("{0}")]
    Model(String),
    #[error("sky: {0}")]
    Sky(String),
}

pub type Result<T> = std::result::Result<T, Error>;
