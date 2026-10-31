//! The reader: what an install's archives (`rage_engine_formats::archive`), the game's mount and data files and a region
//! of the map hold, read into the world.
pub mod assets;
pub mod clips;
pub mod collision;
pub mod convert;
pub mod entry;
pub mod key_digests;
pub mod ped;
pub mod region;
pub mod regions;
pub mod shaders;
pub mod source;
pub mod texture_format;
pub mod water;

/// Archive member meanings supplied by the public format reader.
pub use rage_engine_formats::file_class;
