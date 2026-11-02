//! Blender (Cycles): a world written as a scene whose materials are live shader node graphs of core's material model,
//! saved as a `.blend` and rendered in Cycles. Nothing core states as live is baked: each material's textures are
//! sampled as its slots state them, with their UV sets, transforms, wrap and filters, its vertex colour and alpha
//! mode drawn by its nodes.
//!
//! Three stages, each a program's step:
//! - [`write`](mod@write): the world to a folder: `scene.glb` (the glTF writer's, unchanged: geometry, placements,
//!   lights), `materials.json` (each material's node graph, [`graph`], and which glTF primitive draws with which) and
//!   `textures/` (the images the graphs sample).
//! - [`save`](mod@save): Blender imports `scene.glb` with its own glTF importer and builds each graph into the
//!   primitives' materials, then saves the `.blend`.
//! - [`render`](mod@render): Cycles renders a `.blend` from a stated camera: the image and the passes every material
//!   writes ([`graph::aov`]).
//!
//! Frames: the world frame is glTF's (metres, right-handed, +Y up), which Blender's glTF importer converts to its
//! own (+Z up): world `(x, y, z)` is Blender `(x, -z, y)`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod graph;
pub mod render;
pub mod save;
pub mod write;

use std::path::PathBuf;

use primase_core::Entity;

pub use write::{BlenderReport, WriteOptions, write_file};

/// The schema `materials.json` is written in.
pub const MATERIALS_SCHEMA: &str = "primase-blender-materials/1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    /// The glTF writer's error, writing `scene.glb`.
    #[error(transparent)]
    Gltf(#[from] primase_gltf::Error),
    /// A number JSON cannot hold.
    #[error("{0}")]
    Number(#[from] primase_core::json::JsonError),
    /// The world holds something the writer cannot express.
    #[error("entity {entity:?}: {detail}")]
    World { entity: Entity, detail: String },
    /// A texture whose pixels could not be made.
    #[error("texture {entity:?}: {detail}")]
    Texture { entity: Entity, detail: String },
    /// A folder the writer or a stage refuses to write into, or a stage's run that failed, with where its log is.
    #[error("{0}")]
    Stage(String),
}

pub type Result<T> = std::result::Result<T, Error>;
