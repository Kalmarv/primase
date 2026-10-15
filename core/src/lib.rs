//! primase core: the world store, the core components, the frame, the pass manager and gates.
//!
//! See `docs/DESIGN.md` §3-4 and CONTRIBUTING.md for the conventions every crate follows.

pub mod components;
pub mod frame;
pub mod gate;
pub mod texformat;
pub mod world;

pub use components::*;
pub use texformat::TexFormat;
pub use world::{Column, ColumnMut, Component, Entity, Opaque, OpaqueData, Resource, World};
