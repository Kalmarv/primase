//! Bounds-checked readers of Havok binary data: `TAG0` tagfiles and their `TCM0` type compendia ([`Tagfile`],
//! [`Compendium`]), the older 7.1.0 packfile container ([`packfile`]), and the objects read from them: hknp compressed
//! meshes ([`compressed_mesh`]), convex shapes ([`convex`]), cloth and ragdoll data ([`cloth`]), skeletons, bindings
//! and spline compressed animation ([`animation`], [`spline`]).
//!
//! Type indices are one-based; zero denotes a null type or object reference. Every read is checked: a file that is
//! shorter than it says, or refers outside itself, is an [`Error`] naming the offset, never a panic.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod animation;
pub mod cloth;
pub mod convex;
mod mesh;
pub mod packfile;
mod section;
pub mod spline;
mod spline_sample;

pub use mesh::{
    CompressedMesh, CustomKind, CustomPrimitive, MeshSection, ShapeInstance, Triangle, compressed_mesh,
    extern_shapes, mesh_sections,
};
mod tagfile;
mod types;
mod value;
pub use tagfile::{Item, SDK_VERSIONS, Tagfile};
pub use types::{Compendium, Field, Type};
pub use value::{Array, Value};

pub use section::{Section, sections};

/// A refusal: what is wrong, at the byte offset where it was found.
#[derive(Debug, thiserror::Error)]
#[error("Havok at {offset:#x}: {detail}")]
pub struct Error {
    /// The byte offset in the data read.
    pub offset: usize,
    /// What the data holds that the reader refuses, or lacks.
    pub detail: String,
}

impl Error {
    /// A refusal at `offset`.
    pub fn new(offset: usize, detail: impl Into<String>) -> Self {
        Error {
            offset,
            detail: detail.into(),
        }
    }
}

/// A read's result.
pub type Result<T> = std::result::Result<T, Error>;

/// A read past the animation data ([`primase_foundations::bytes::Cursor`]): the error at the read's offset.
impl From<primase_foundations::bytes::BoundsError> for Error {
    fn from(e: primase_foundations::bytes::BoundsError) -> Self {
        invalid(
            usize::try_from(e.absolute()).unwrap_or(usize::MAX),
            "the block runs past the animation data",
        )
    }
}

fn invalid(offset: usize, detail: impl Into<String>) -> Error {
    Error::new(offset, detail)
}
