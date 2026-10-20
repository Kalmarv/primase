//! primase core: the world store, the core components, the frame, the pass manager and gates.
//!
//! See `docs/DESIGN.md` §3-4 and CONTRIBUTING.md for the conventions every crate follows.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod convert;
mod environment;
pub use environment::{Ambient, AmbientLight, Environment};
pub mod frame;
pub mod gate;
pub mod json;
pub mod layered;
pub mod ledger;
mod lod;
pub use lod::{LodChoice, LodForm, Lods, SelectLod, select_lod};
mod marker;
pub use marker::{Curve, Marker, kind_is_valid};
mod spawn;
pub use spawn::{PlaceSpawn, SpawnAt, place_spawn};
mod collider;
pub use collider::{Collider, Diagonal, Heightfield, Shape};
mod draw_order;
pub use draw_order::{DrawLayer, DrawOrder};
mod material_usage;
pub use material_usage::MaterialUsage;
mod vertex_color;
pub use vertex_color::{VertexColor, color0_drawn, uses_color0};
pub mod pass;
pub mod pbr;
pub mod print;
pub mod provenance;
pub use provenance::{Origin, Provenance};
mod priority;
pub use priority::{DropOptional, Priority, drop_optional};
pub mod state;
pub mod summary;
pub mod texformat;
pub mod world;

pub use components::*;
pub use json::JsonError;
pub use layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
pub use texformat::TexFormat;
pub use world::{
    Column, ColumnMut, Component, ComponentJson, ComponentJsonError, Entity, Linearity, Opaque, OpaqueData,
    Resource, World, remap_list,
};
