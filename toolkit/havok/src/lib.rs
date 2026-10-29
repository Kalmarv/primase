//! Havok data made primase's: convex cores as triangles ([`ConvexCore`]), skeletons and spline compressed animations
//! as world data with the pass that lowers them to clips ([`world`]), a behaviour graph's layers as world data with the
//! pass that lowers them to a controller ([`layers`]), and a model's cloth as world data for a program
//! that runs it ([`cloth`]). The files are read by `havok-formats`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cloth;
mod convex;
pub use convex::{ConvexCore, MAX_PLANES, MAX_VERTICES};
pub mod layers;
pub mod world;
pub use layers::LOWER_HAVOK_LAYERS;
pub use world::LOWER_HAVOK_CLIPS;

/// Register this crate's passes (`lower-havok-clips`, `lower-havok-layers`).
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    world::register(reg);
    layers::register(reg);
}

fn invalid(offset: usize, detail: impl Into<String>) -> havok_formats::Error {
    havok_formats::Error::new(offset, detail)
}
