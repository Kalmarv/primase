//! ReSkate (skate.): the writer of ReSkate Studio normalized map folders from skate-scene worlds (`map.json`,
//! `meshes/*.glb`, `textures/*`, `ui/*`, `trainer.json`, `stats.json`, `pieces.json`, `textures_index.json`), gated
//! against the reference C# writer by what Studio reads: meshes, tables and texture payloads exactly, JSON as parsed
//! values, PNGs by pixels.

pub mod build;
pub mod collision;
pub mod glb;
pub mod grind;
pub mod map;
pub mod material;
pub mod model;
pub mod overlay;
pub mod pausemap;
pub mod records;
pub mod scene;
pub mod texture;
pub mod vec;

use std::path::PathBuf;

pub use build::{BuildOptions, BuildReport, build};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}")]
    Scene(String),
    #[error("texture: {0}")]
    Texture(String),
    #[error("not supported yet: {0}")]
    Unsupported(String),
    #[error("{file}: {source}")]
    Json {
        file: String,
        source: primase_core::JsonError,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The JSON text of one of the map's files (`primase_core::json::to_vec`: a non-finite number is an error).
pub(crate) fn json_bytes<T: serde::Serialize + ?Sized>(file: &str, v: &T, pretty: bool) -> Result<Vec<u8>> {
    primase_core::json::to_vec(v, pretty).map_err(|source| Error::Json {
        file: file.to_owned(),
        source,
    })
}
