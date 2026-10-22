//! STUNTBOOST's lowerings, each choice the program's: collision with surfaces ([`collision`]), spawns as markers
//! ([`spawns`]), the materials and the baked atlas ([`materials`]), and what the game never draws or a static map has
//! no use for, left out ([`hidden`]).
pub mod collision;
pub mod hidden;
pub mod materials;
pub mod spawns;
#[cfg(test)]
mod tests;

pub use collision::{KillSurface, LowerStuntboostCollision};
pub use hidden::OmitStuntboostHidden;
pub use materials::{Baked, LowerStuntboostMaterials, Transparent};
pub use spawns::{LowerStuntboostSpawns, OtherSpawns, Spawn, Start, spawns};
