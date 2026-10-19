//! The cell spool: a map in parts gathers each 256 m collision cell from every part before it is laid out
//! ([`primase_core::convert::convert_parts_gathered`]). In the gather traversal each part shares what it lays out in
//! each cell (`share-reskate-collision`); the spool keeps the shares on disk, a file per cell ([`CellSpool`], in the
//! codec of [`codec`]), and gives each cell back alone, every part's shares of it in a world of its own, for the group
//! passes (`prepare-reskate-collision`) and the map sink. What it holds in memory at once is one part's shares while it
//! takes them, or one cell's while it gives it.

pub mod cells;
pub mod codec;

pub use cells::{CellSpool, SpoolReport};
