//! skate-scene interchange v1 (frozen) read and write, and the `skate.*` dialect components it carries.
//!
//! A scene folder is a JSON manifest (`scene.json`), little-endian binary tables (placements, lights, grind curves,
//! collision soups) and the files they reference (prototype GLBs, textures). [`read`] attaches a folder to a
//! [`primase_core::World`] (see [`components`] for the mapping); [`write`] writes a world back as a folder. An unchanged
//! world writes the same tables byte for byte, the same manifest as parsed values, and the referenced files copied.

pub mod assets;
pub mod collision;
pub mod components;
mod layered;
mod layered_lower;
pub use layered_lower::LowerLayeredPbr;
pub mod lower;
pub mod raise;
pub mod read;
pub mod tables;
pub mod write;

use std::path::PathBuf;

pub use assets::prepare_scene_assets;
pub use collision::{SkateRock, scene_surfaces, soup};
pub use components::*;
pub use lower::{LowerSkateLights, LowerSkatePbr, MskError, lower_light, lower_material, msk_to_orm};
pub use raise::{RaiseOptions, raise_skate, studio_watts};
pub use read::{ReadReport, read};
pub use write::{WriteReport, write};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// The manifest or a table breaks a rule of the format; the message names the item.
    #[error("{0}")]
    Scene(String),
    #[error("{path}: {message}")]
    Table { path: String, message: String },
    #[error("write: {0}")]
    Write(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
