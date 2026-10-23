//! Bounds-checked views of Havok binary tagfiles and their type compendia.
//! Type indices are one-based; zero denotes a null type or object reference.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod convex;
mod mesh;
pub use convex::ConvexCore;
mod section;
pub use mesh::{CompressedMesh, MeshSection, Triangle, compressed_mesh, mesh_sections};
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

fn invalid(offset: usize, detail: impl Into<String>) -> Error {
    Error {
        offset,
        detail: detail.into(),
    }
}
