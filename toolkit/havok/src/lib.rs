//! Bounds-checked views of Havok binary tagfiles and their type compendia.
//! Type indices are one-based; zero denotes a null type or object reference.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod animation;
mod convex;
mod mesh;
pub use convex::{ConvexCore, MAX_PLANES, MAX_VERTICES};
mod section;
pub mod spline;
mod spline_sample;
pub mod world;

pub use mesh::{
    CompressedMesh, CustomKind, CustomPrimitive, MeshSection, ShapeInstance, Triangle, compressed_mesh,
    extern_shapes, mesh_sections,
};
/// Register this crate's passes (`lower-havok-clips`).
pub use world::{LOWER_HAVOK_CLIPS, register};
mod tagfile;
mod types;
mod value;
pub use tagfile::{Item, SDK_VERSIONS, Tagfile};
pub use types::{Compendium, Field, Type};
pub use value::{Array, Value};

pub use section::{Section, sections};

#[derive(Debug, thiserror::Error)]
#[error("Havok at {offset:#x}: {detail}")]
pub struct Error {
    pub offset: usize,
    pub detail: String,
}

pub type Result<T> = std::result::Result<T, Error>;

/// A read past the animation data ([`primase_bytes::Cursor`]): the error at the read's offset.
impl From<primase_bytes::BoundsError> for Error {
    fn from(e: primase_bytes::BoundsError) -> Self {
        invalid(
            usize::try_from(e.absolute()).unwrap_or(usize::MAX),
            "the block runs past the animation data",
        )
    }
}

fn invalid(offset: usize, detail: impl Into<String>) -> Error {
    Error {
        offset,
        detail: detail.into(),
    }
}
