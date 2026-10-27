//! Frostbite in a world, as skate. (ReSkate) uses it: what the container readers of `frostbite-formats` find (entries,
//! EBX documents, textures, shader block depots, program databases, permutations, MeshSets, skeletons) put into
//! components as stored, and the passes that give the dialect core meanings (`resolve-frostbite-shaders`).
//!
//! The formats themselves (superbundle TOCs, bundles, CAS archives, `.fbmod`, EBX, MeshSet, physics, ...) are read and
//! written by `frostbite-formats`; this crate only builds worlds from them.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bytecode;
pub mod components;
pub mod ebx_json;
pub mod geometry;
pub mod passes;
pub mod read;
pub mod render_state;
