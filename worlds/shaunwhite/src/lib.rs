//! Shaun White Skateboarding (PC): its levels read from version-26 Anvil forges (cells, entities, meshes,
//! materials, textures and collision) and lowered to core meanings.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;
pub mod shaders;

pub use passes::register;
