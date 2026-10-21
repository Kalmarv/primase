//! MeshShape entries of a forge read into a world: each chosen entry a prototype holding its geometry
//! ([`AnvilMeshShape`]) and named by its entry, placed once at the origin. The shapes' units and frame are not
//! established, so the program states the frame their numbers are converted through (a [`SourceFrame`]); nothing
//! defaults it. `lower-anvil-collision` makes core collision of the geometry.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the forge file the caller names"
)]

use std::fs::File;
use std::path::Path;

use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Name, Transform, World};

use crate::components::AnvilMeshShape;
use crate::read::data::{Resource, read_entry};
use crate::read::forge::Forge;
use crate::read::mesh_shape::{MESH_SHAPE, MeshShape};

/// Why the shapes can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Forge(#[from] crate::read::forge::Error),
    #[error("entry {entry}: {source}")]
    Data {
        entry: usize,
        #[source]
        source: crate::read::data::Error,
    },
    #[error("entry {entry}: {source}")]
    Shape {
        entry: usize,
        #[source]
        source: crate::read::mesh_shape::Error,
    },
    #[error("entry {entry}: type {type_id:#010x}, not a MeshShape ({MESH_SHAPE:#010x})")]
    NotAShape { entry: usize, type_id: u32 },
    #[error("entry {0}: past the forge's entries")]
    Entry(usize),
}

/// A decoded resource's largest size this reader accepts (a MeshShape's record; the forge's own sizes are checked).
pub const MAX_RECORD: usize = 64 << 20;

/// The MeshShapes at `entries` of the forge at `path` into `w`, their numbers converted through `frame`: the
/// placements, in `entries`' order.
pub fn read_mesh_shapes(
    path: impl AsRef<Path>,
    entries: &[usize],
    frame: SourceFrame,
    w: &mut World,
) -> Result<Vec<Entity>, Error> {
    let forge = Forge::open(&path)?;
    let mut file = File::open(&path)?;
    let mut placements = Vec::with_capacity(entries.len());
    for &entry in entries {
        let index = forge.index.get(entry).ok_or(Error::Entry(entry))?;
        let name = forge.names.get(entry).ok_or(Error::Entry(entry))?.name.clone();
        let payload = forge.payload(&mut file, entry)?;
        let data = |source| Error::Data { entry, source };
        let block = read_entry(&payload, MAX_RECORD).map_err(data)?;
        let resource = Resource::parse(&block.resource.data).map_err(data)?;
        if resource.type_id != MESH_SHAPE {
            return Err(Error::NotAShape {
                entry,
                type_id: resource.type_id,
            });
        }
        let shape = MeshShape::parse(&resource.body).map_err(|source| Error::Shape { entry, source })?;
        let prototype = w.spawn();
        w.insert(prototype, Name(name.clone()));
        w.insert(prototype, anvil_mesh_shape(&shape, entry, index.id, frame)?);
        let placement = w.spawn();
        w.insert(placement, Name(name));
        w.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        placements.push(placement);
    }
    Ok(placements)
}

/// The [`AnvilMeshShape`] of the MeshShape `shape` (forge entry `entry`, index id `index_id`), its positions converted
/// through `frame` and its faces reversed where `frame` mirrors.
pub fn anvil_mesh_shape(
    shape: &MeshShape,
    entry: usize,
    index_id: u64,
    frame: SourceFrame,
) -> Result<AnvilMeshShape, Error> {
    let g = shape
        .geometry()
        .map_err(|source| Error::Shape { entry, source })?;
    let flip = frame.flips_winding();
    let faces = g
        .faces
        .iter()
        .map(|f| {
            let [a, b, c] = f.map(u32::from);
            if flip { [a, c, b] } else { [a, b, c] }
        })
        .collect();
    Ok(AnvilMeshShape {
        entry: u32::try_from(entry).map_err(|_| Error::Entry(entry))?,
        index_id,
        positions: g.positions.iter().map(|&p| frame.point(p)).collect(),
        faces,
        face_edges: g.face_edges,
        edge_faces: g.edge_faces,
    })
}
