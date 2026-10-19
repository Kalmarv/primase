//! skate-scene interchange v1 (frozen): reading a scene folder into the world, loading what it references, and writing
//! a world back as a folder.
//!
//! A scene folder is a JSON manifest (`scene.json`), little-endian binary tables (placements, lights, grind curves,
//! collision soups) and the files they reference (prototype GLBs, textures). [`read`] attaches a folder to a
//! [`primase_core::World`] (see [`crate::dialect`] for the mapping): the manifest and tables, with the GLBs and
//! textures as file references (`core.mesh_file`, `core.texture_file`); [`load`] then reads those files into
//! `core.mesh` and `core.texture`. [`prepare_assets`] writes a world's in-memory meshes and textures as the files a
//! scene references, and [`write`] writes the world as a folder. An unchanged world writes the same tables byte for
//! byte, the same manifest as parsed values, and the referenced files copied.

mod assets;
pub(crate) mod layered;
mod read;
pub mod tables;
mod write;

use std::path::Path;

pub use assets::prepare_assets;
pub use read::{ReadReport, read};
pub use write::{WriteReport, write};

use crate::Result;

/// What [`load`] read: the prototypes' meshes and the textures.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub meshes: primase_gltf::LoadReport,
    pub textures: primase_image::TextureLoadReport,
}

/// Read every file a scene read by [`read`] references into the world: each `core.mesh_file` as `core.mesh` (the glTF
/// crate's GLB reader) and each `core.texture_file` without pixels as `core.texture` (the image toolkit's PNG and DDS
/// readers), paths relative to the scene folder `dir`. The file references stay, as the payloads' origin.
pub fn load(dir: &Path, world: &mut primase_core::World) -> Result<LoadReport> {
    Ok(LoadReport {
        meshes: primase_gltf::load_mesh_files(world, dir)?,
        textures: primase_image::load_textures(world, dir)?,
    })
}
