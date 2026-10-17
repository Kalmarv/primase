//! primase core: the world store, the core components, the frame, the pass manager and gates.
//!
//! See `docs/DESIGN.md` §3-4 and CONTRIBUTING.md for the conventions every crate follows.

pub mod components;
pub mod frame;
pub mod gate;
pub mod json;
pub mod layered;
mod lod;
pub use lod::{LodChoice, LodForm, Lods, SelectLod, select_lod};
pub mod pass;
pub mod pbr;
mod priority;
pub use priority::{DropOptional, Priority, drop_optional};
pub mod state;
pub mod texformat;
pub mod world;

pub use components::*;
pub use json::JsonError;
pub use layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
pub use texformat::TexFormat;
pub use world::{
    Column, ColumnMut, Component, ComponentJson, ComponentJsonError, Entity, Opaque, OpaqueData, Resource,
    World,
};
