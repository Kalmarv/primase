//! Anvil engine containers: the forge archive's index ([`read::forge`]: a `.forge` file of version 27 opened and its
//! entries listed, offset, size and both ids as stored, with each entry's name record) and its entries' data blocks
//! ([`read::data`]: compression version 2, algorithm 5 (zstd), checksum checked, the resource's header parsed, its
//! body kept as stored).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

pub use passes::register;
