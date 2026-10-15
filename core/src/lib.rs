//! primase core: the world store, the core components, the frame, the pass manager and gates.
//!
//! See `docs/DESIGN.md` §3-4 and CONTRIBUTING.md for the conventions every crate follows.

pub mod components;
pub mod frame;
pub mod gate;
pub mod json;
pub mod pass;
pub mod pbr;
pub mod texformat;
pub mod world;

pub use components::*;
pub use json::JsonError;
pub use texformat::TexFormat;
pub use world::{
    Column, ColumnMut, Component, ComponentJson, ComponentJsonError, Entity, Opaque, OpaqueData, Resource,
    World,
};
