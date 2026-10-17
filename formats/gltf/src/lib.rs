//! Our own glTF 2.0 / GLB writer (and reader for round trips) from the world.
//!
//! - [`write()`] / [`write_file`]: a [`primase_core::World`] to one GLB (prototypes as shared meshes, placements as
//!   nodes, `pbr.material`s, embedded PNGs, dialect components in extras); see [`write`](mod@write) for the rules.
//! - [`Gltf`]: the strict GLB reader, meshes into [`primase_core::Mesh`].
//! - [`load_mesh_files`]: the pass loading `core.mesh_file` prototypes into `core.mesh`; [`mesh_file_box`]: a mesh
//!   file's box from its JSON alone.
//!
//! Frames: the world frame is glTF's (metres, right-handed, +Y up, -Z forward), so nothing is converted.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod glb;
mod json;
mod load;
mod read;
pub mod write;

use std::path::{Path, PathBuf};

use primase_core::Entity;

pub use load::{LoadMeshFiles, LoadReport, load_mesh_files, mesh_file_box, register};
pub use read::{Gltf, ReadMesh, ReadPrimitive};
pub use write::{
    GroupBy, Selected, Selection, WriteOptions, WriteReport, fallback_material, write, write_file,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// The GLB container is malformed.
    #[error("GLB at byte {offset}: {detail}")]
    Glb { offset: usize, detail: String },
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The glTF document breaks a rule the reader enforces; `item` is its JSON path (`meshes[2].primitives[0]`).
    #[error("{item}: {detail}")]
    Invalid { item: String, detail: String },
    /// An error inside one file.
    #[error("{path}: {source}")]
    InFile { path: PathBuf, source: Box<Error> },
    /// The world holds something the writer cannot express (a malformed mesh, a placement without a transform).
    #[error("entity {entity:?}: {detail}")]
    World { entity: Entity, detail: String },
    /// Over glTF's 4 GiB GLB limit.
    #[error("GLB would be {bytes} bytes, over the 4 GiB limit")]
    TooLarge { bytes: u64 },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    fn in_file(self, path: &Path) -> Error {
        Error::InFile {
            path: path.to_owned(),
            source: Box::new(self),
        }
    }
}
