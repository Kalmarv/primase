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
pub mod names;
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
pub mod morphs;
pub mod pass;
pub mod pbr;
pub mod print;
pub mod provenance;
pub mod rig;
pub mod skeleton;
pub mod skin;
pub use morphs::{Deltas, Frame as MorphFrame, MorphId, Morphs, Target as MorphTarget};
pub use provenance::{Origin, Provenance};
pub use rig::{Attachment, MorphWeights, RigInstance, RigPose, SkinInstance, Socket, SocketId, Sockets};
pub use skeleton::{Joint, JointId, Rest, Skeleton};
pub use skin::{Binding, BindingId, Deformation, Influences, Skin, Slot, WeightRule};
mod priority;
pub use priority::{DropOptional, Priority, drop_optional};
pub mod state;
pub mod summary;
pub mod texformat;
pub mod translate;
pub mod world;

pub use components::*;
pub use json::JsonError;
pub use layered::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
pub use texformat::TexFormat;
pub use world::{
    Column, ColumnMut, Component, ComponentJson, ComponentJsonError, Entity, Geometry, Linearity, Opaque,
    OpaqueData, Resource, World, remap_list,
};

/// Register core's generic passes by name (`select-lod`, `drop-optional`, `place-spawn`, `resolve-state`,
/// `translate-world`), as every crate registers its own: a program builds its registry from these.
pub fn register(reg: &mut pass::PassRegistry) {
    reg.register("select-lod", select_lod);
    reg.register("drop-optional", drop_optional);
    reg.register("place-spawn", place_spawn);
    reg.register("resolve-state", state::resolve_state);
    reg.register_requiring("translate-world", translate::translate_world, &[("by", "0,0,0")]);
}
