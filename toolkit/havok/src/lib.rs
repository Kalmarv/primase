//! Havok data made primase's: convex cores as triangles ([`ConvexCore`]), skeletons and spline compressed animations
//! as world data with the pass that lowers them to clips ([`world`]), and a model's cloth as world data for a program
//! that runs it ([`cloth`]). The files are read by `havok-formats`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cloth;
mod convex;
pub use convex::{ConvexCore, MAX_PLANES, MAX_VERTICES};
pub mod world;
/// Register this crate's passes (`lower-havok-clips`).
pub use world::{LOWER_HAVOK_CLIPS, register};

fn invalid(offset: usize, detail: impl Into<String>) -> havok_formats::Error {
    havok_formats::Error::new(offset, detail)
}
