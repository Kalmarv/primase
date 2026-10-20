//! STUNTBOOST's reader: a level from the install ([`level::read_level`]), the game's own files derived from it
//! ([`derived`]); node names are parsed by [`crate::components::names`].
pub mod derived;
pub mod level;

pub use level::{LevelReport, read_level};
