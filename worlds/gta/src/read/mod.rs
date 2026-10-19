//! The reader: an install's archives, the game's mount and data files, and what a region of the map holds, read into
//! the world.
pub mod archive;
pub mod assets;
pub mod convert;
pub mod gtxd;
pub mod mount;
pub(crate) mod rbf;
pub mod region;
pub mod source;
pub(crate) mod xml;
