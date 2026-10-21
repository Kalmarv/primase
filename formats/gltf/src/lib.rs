//! Our own glTF 2.0 / GLB writer from the world, and the passes that prepare a world for it.
//!
//! - [`write()`] / [`write_file`]: a [`primase_core::World`] to one GLB (prototypes as shared meshes, placements as
//!   nodes, `pbr.material`s, embedded PNGs, dialect components in extras); see [`write`](mod@write) for the rules.
//! - [`prepare`]: `prepare-gltf-materials` and `fill-missing-texcoords`.
//!
//! The GLB container, the strict reader and the vertex encoding are `primase-glb`'s.
//!
//! Frames: the world frame is glTF's (metres, right-handed, +Y up, -Z forward), so nothing is converted.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod prepare;
mod rig;
pub mod write;

use std::path::PathBuf;

use primase_core::Entity;

pub use write::{
    GroupBy, Piece, Selected, Selection, WriteOptions, WriteReport, write, write_document, write_file,
};

/// Register this crate's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    prepare::register(reg);
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// The GLB could not be assembled (over glTF's 4 GiB limit), or the document not serialized.
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    /// A world resource whose JSON view loses a value.
    #[error("{item}: {detail}")]
    Invalid { item: String, detail: String },
    /// The world holds something the writer cannot express (a malformed mesh, a placement without a transform).
    #[error("entity {entity:?}: {detail}")]
    World { entity: Entity, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;
