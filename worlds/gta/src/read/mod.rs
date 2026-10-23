//! The reader: an install's archives, the game's mount and data files, and what a region of the map holds, read into
//! the world.
pub mod archive;
pub mod assets;
pub mod clips;
pub mod collision;
pub mod convert;
pub mod entry;
pub mod gtxd;
pub mod mount;
pub mod ped;
pub(crate) mod rbf;
pub mod region;
pub mod shaders;
pub mod source;
pub mod water;
pub(crate) mod xml;
