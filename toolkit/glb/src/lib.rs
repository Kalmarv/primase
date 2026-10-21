//! The GLB codec, shared by everything that reads or writes glTF binaries.
//!
//! - [`parse`] / [`assemble`] / [`Document`]: the container (glTF 2.0 §4.4).
//! - [`Gltf`]: the strict reader of a GLB, or of a `.gltf` whose buffers the caller supplies, meshes into
//!   [`primase_core::Mesh`].
//! - [`sampler_filters`] / [`material_shading`]: a sampler's filters and wrap, a material's shading, in core's terms.
//! - [`load_mesh_files`]: a reader's load step, `core.mesh_file` prototypes into `core.mesh`; [`mesh_file_box`]: a
//!   mesh file's box from its JSON alone.
//! - [`Buffers`]: a primitive's vertex streams and indices as accessors in one binary buffer, the encoding every
//!   writer of meshes shares; [`encode_mesh`]: one mesh, without materials, as a GLB of its own.
//!
//! Frames: glTF's (metres, right-handed, +Y up, -Z forward); nothing is converted.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod container;
mod encode;
mod json;
mod load;
mod material;
mod read;

use std::path::{Path, PathBuf};

use primase_core::Entity;

pub use container::{Chunks, Document, assemble, parse};
pub use encode::{Buffers, PrimitiveAccessors, encode_mesh};
pub use load::{LoadReport, load_mesh_files, mesh_file_box};
pub use material::{material_shading, sampler_filters};
pub use read::{AccessorData, ComponentType, ElementType, Gltf, ReadMesh, ReadPrimitive, bounds};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// The GLB container is malformed.
    #[error("GLB at byte {offset}: {detail}")]
    Glb { offset: usize, detail: String },
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The glTF document breaks a rule the reader enforces, or a mesh one the encoder does; `item` is its JSON path
    /// (`meshes[2].primitives()[0]`).
    #[error("{item}: {detail}")]
    Invalid { item: String, detail: String },
    /// An error inside one file.
    #[error("{path}: {source}")]
    InFile { path: PathBuf, source: Box<Error> },
    /// A file does not fit the world that references it (a `core.mesh_file` naming more materials than its mesh has
    /// primitives).
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
