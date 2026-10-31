//! TrackMania's lowerings: the materials ([`materials`]), the spawn locations as markers ([`spawns`]), the collision
//! surfaces' classes ([`surfaces`]) and the blocks' lights ([`lights`]).
pub mod lights;
pub mod materials;
pub mod spawns;
pub mod surfaces;

pub use lights::{LOWER_TRACKMANIA_LIGHTS, LowerTrackmaniaLights};
pub use materials::{LOWER_TRACKMANIA_MATERIALS, LowerTrackmaniaMaterials};
pub use spawns::{LOWER_TRACKMANIA_SPAWNS, LowerTrackmaniaSpawns, OtherSpawns};
pub use surfaces::{LOWER_TRACKMANIA_SURFACES, LowerTrackmaniaSurfaces};
