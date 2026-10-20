//! STUNTBOOST's reader: a level from the install ([`level::read_level`]) and its node names ([`names`]).
pub mod level;
pub mod names;

pub use level::{LevelReport, read_level};
