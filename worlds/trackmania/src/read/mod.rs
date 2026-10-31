//! A map and the game's files read into a world: the game's file system ([`files`]), its GameBox documents and the
//! references between them ([`library`]), solids ([`solid`]), materials and textures ([`material`]) and the map's
//! blocks placed from their models ([`map`]).
pub mod files;
pub mod library;
pub mod map;
pub mod material;
pub mod solid;

pub use files::GameFiles;
pub use map::{MapReport, TrackmaniaReader};
