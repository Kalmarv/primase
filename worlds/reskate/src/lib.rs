//! ReSkate (skate.): the writer of ReSkate Studio normalized map folders from skate-scene worlds, a port of the
//! reference C# writer gated byte for byte against it (`map.json`, `meshes/*.glb`, `textures/*`, `ui/*`,
//! `trainer.json`, `stats.json`, `pieces.json`, `textures_index.json`).
//!
//! The writer's numbers follow the reference runtime exactly: [`dotnet`] holds .NET's number text and JSON layout,
//! [`vec`] the two vector types' rounding.

pub mod build;
pub mod collision;
pub mod dotnet;
pub mod glb;
pub mod grind;
pub mod map;
pub mod material;
pub mod model;
pub mod overlay;
pub mod pausemap;
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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
