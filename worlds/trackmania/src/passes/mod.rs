//! TrackMania's lowerings: the materials ([`materials`]), the spawn locations as markers ([`spawns`]) and the collision
//! surfaces' classes ([`surfaces`]).
pub mod materials;
pub mod spawns;
pub mod surfaces;

pub use materials::{LOWER_TRACKMANIA_MATERIALS, LowerTrackmaniaMaterials};
pub use spawns::{LOWER_TRACKMANIA_SPAWNS, LowerTrackmaniaSpawns, OtherSpawns};
pub use surfaces::{LOWER_TRACKMANIA_SURFACES, LowerTrackmaniaSurfaces};
