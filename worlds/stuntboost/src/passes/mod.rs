//! STUNTBOOST's lowerings, each choice the program's: collision with surfaces ([`collision`]), spawns as markers
//! ([`spawns`]), the materials and the baked atlas ([`materials`]), and what the game never draws or a static map has
//! no use for, left out ([`hidden`]).
pub mod collision;
pub mod hidden;
pub mod materials;
pub mod spawns;
#[cfg(test)]
mod tests;

pub use collision::{KillSurface, LOWER_STUNTBOOST_COLLISION, LowerStuntboostCollision};
pub use hidden::{OMIT_STUNTBOOST_HIDDEN, OmitStuntboostHidden};
pub use materials::{Baked, LOWER_STUNTBOOST_MATERIALS, LowerStuntboostMaterials, Transparent};
pub use spawns::{LOWER_STUNTBOOST_SPAWNS, LowerStuntboostSpawns, OtherSpawns, Spawn, Start, spawns};

pub mod resolve_shaders;
pub use resolve_shaders::{RESOLVE_STUNTBOOST_SHADERS, ResolveStuntboostShaders};
