//! Havok data made primase's: convex cores as triangles ([`ConvexCore`]), and skeletons and spline compressed
//! animations as world data with the pass that lowers them to clips ([`world`]). The files are read by
//! `havok-formats`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod convex;
pub use convex::{ConvexCore, MAX_PLANES, MAX_VERTICES};
pub mod world;
/// Register this crate's passes (`lower-havok-clips`).
pub use world::{LOWER_HAVOK_CLIPS, register};

fn invalid(offset: usize, detail: impl Into<String>) -> havok_formats::Error {
    havok_formats::Error::new(offset, detail)
}
