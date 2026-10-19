//! skate-scene interchange v1 (frozen): the format. A scene folder is a JSON manifest (`scene.json`), little-endian
//! binary tables (placements, lights, grind curves, collision soups: [`tables`]) and the files they reference
//! (prototype GLBs, textures), with the layered-material syntax ([`layered`]) and the collision soups' surface bytes
//! ([`soup`]). Its I/O is the crate's reader and writer: [`read`] attaches a folder to a [`primase_core::World`] (see
//! [`crate::components`] for the mapping), the manifest and tables, with the GLBs and textures as file references
//! (`core.mesh_file`, `core.texture_file`); [`load`] then reads those files into `core.mesh` and `core.texture`.
//! [`write`] writes the world as a folder: the files it names copied, the meshes and textures it holds only in memory
//! encoded. An unchanged world writes the same tables byte for byte, the same manifest as parsed values, and the
//! referenced files copied.

pub(crate) mod layered;
pub mod soup;
pub mod tables;

pub use crate::read::interchange::{LoadReport, ReadReport, load, read};
pub use crate::write::interchange::{WriteReport, write};
