//! Anvil worlds: forge archives read through `anvil_formats` into a world (placements, terrain, meshes, collision,
//! textures and the source records as components), and the passes that lower them to core meanings.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

pub use passes::register;
