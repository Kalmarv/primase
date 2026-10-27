//! Readers for the data files of Unity players.
//!
//! A built Unity game keeps its scenes and assets in *SerializedFiles* (`globalgamemanagers`, `*.assets`, `levelN`),
//! with the bulk data (textures, meshes, audio) in `.resS` stream files beside them. The modules:
//!
//! - [`serialized`]: the SerializedFile container: header, types, the object table, script references, external
//!   files. Strict and version-gated: only container versions verified against real data are read.
//! - [`tpk`]: a type tree package (the TPK format), the class layouts per Unity version that a player build strips
//!   from its files. The caller supplies the package; none is bundled.
//! - [`typetree`]: decoding an object's bytes by its class's type tree into a [`Value`] tree.
//! - [`value`]: the decoded value tree.
//! - [`version`]: Unity version strings.
//! - [`resource`]: streamed resources (`.resS`), the bulk data that objects point at.
//! - [`player`]: a player's data folder: its files, pointers resolved across them, its quality levels, its build's
//!   scene list and render pipeline.
//! - [`scene`]: a scene's hierarchy (GameObjects, Transforms composed to world matrices) and its renderers, lights,
//!   colliders and terrains, with the scene's ambient and fog settings.
//! - [`listing`]: a player's objects by class and name, read from the object tables.
//! - [`material`]: materials as stored (shader, saved properties, keywords).
//! - [`shader`]: a Shader's render state as stored (subshaders, passes, blend, depth and stencil state).
//! - [`terrain`]: terrains as stored (heightmap, holes, splat layers and alphamaps, trees).
//! - [`pipeline`]: the lighting and shadow settings of a Universal Render Pipeline asset.
//!
//! Every read is bounds-checked, and a file the reader does not understand is refused with an [`Error`] that names
//! what it found and where.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod listing;
pub mod material;
pub mod pipeline;
pub mod player;
pub mod reader;
pub mod resource;
pub mod scene;
pub mod serialized;
pub mod shader;
pub mod terrain;
pub mod tpk;
pub mod typetree;
pub mod value;
pub mod version;

pub use serialized::{ObjectInfo, SerializedFile};
pub use tpk::TypeTreePackage;
pub use typetree::TypeTree;
pub use value::Value;
pub use version::UnityVersion;

/// Why a read was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The bytes are not what the format allows (a length past the data, a bad tag, a broken reference).
    #[error("{0}")]
    Invalid(String),
    /// The bytes are well formed but use something the reader does not read (an unverified container version, a
    /// feature of a newer Unity).
    #[error("not supported: {0}")]
    Unsupported(String),
    /// A file could not be read from disk.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: std::path::PathBuf,
        /// What the operating system said.
        source: std::io::Error,
    },
}

/// A read's result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}
