//! Mesh entries of a forge read into a world: each chosen entry a prototype holding its compiled buffers
//! ([`AnvilMesh`]) and named by its entry, placed once at the origin. The meshes' units and frame are not established,
//! so the program states the frame their numbers are converted through (a [`SourceFrame`]); nothing defaults it.
//! `lower-anvil-mesh` makes a core mesh of the buffers.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the forge file the caller names"
)]

use std::fs::File;
use std::path::Path;

use primase_core::frame::SourceFrame;
use primase_core::{Entity, Instance, Name, Transform, World};

use crate::components::AnvilMesh;
use crate::read::data::{Resource, read_entry};
use crate::read::forge::Forge;
use crate::read::mesh::{MESH, StoredMesh};
use crate::read::shapes::MAX_RECORD;

/// Why the meshes can't be read.
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
    Mesh {
        entry: usize,
        #[source]
        source: crate::read::mesh::Error,
    },
    #[error(
        "entry {entry}: type {type_id:#010x}, not a Mesh ({MESH:#010x}); the type is read from the record"
    )]
    NotAMesh { entry: usize, type_id: u32 },
    #[error("entry {0}: past the forge's entries")]
    Entry(usize),
}

/// The Meshes at `entries` of the forge at `path` into `w`, their numbers converted through `frame`: the placements,
/// in `entries`' order.
pub fn read_meshes(
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
        if resource.type_id != MESH {
            return Err(Error::NotAMesh {
                entry,
                type_id: resource.type_id,
            });
        }
        let stored = StoredMesh::parse(&resource.body).map_err(|source| Error::Mesh { entry, source })?;
        let prototype = w.spawn();
        w.insert(prototype, Name(name.clone()));
        w.insert(prototype, anvil_mesh(&stored, entry, index.id, frame)?);
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

/// A Mesh body's buffers as an [`AnvilMesh`], its numbers converted through `frame`: positions decoded
/// ([`StoredMesh::positions`]), triangles counted from their ranges (reversed when `frame` mirrors), each range's first
/// triangle and Material, and the vertex records as stored.
pub fn anvil_mesh(
    stored: &StoredMesh,
    entry: usize,
    index_id: u64,
    frame: SourceFrame,
) -> Result<AnvilMesh, Error> {
    let mesh_err = |source| Error::Mesh { entry, source };
    let flip = frame.flips_winding();
    let triangles = stored
        .triangles()
        .map_err(mesh_err)?
        .into_iter()
        .map(|[a, b, c]| if flip { [a, c, b] } else { [a, b, c] })
        .collect();
    let ranges = stored
        .ranges()
        .map_err(mesh_err)?
        .into_iter()
        .map(|r| u32::try_from(r.indices.start / 3).map_err(|_| Error::Entry(entry)))
        .collect::<Result<_, _>>()?;
    let materials = stored.materials().map_err(mesh_err)?;
    let v = &stored.vertices;
    let positions = stored.positions().map_err(mesh_err)?;
    Ok(AnvilMesh {
        entry: u32::try_from(entry).map_err(|_| Error::Entry(entry))?,
        index_id,
        frame,
        positions: positions.into_iter().map(|p| frame.point(p)).collect(),
        triangles,
        ranges,
        materials,
        kind: stored.kind,
        stride: u32::try_from(v.stride).map_err(|_| Error::Entry(entry))?,
        vertex_records: v.bytes.clone(),
        words_20: if stored.kind == 2 {
            v.words_20()
        } else {
            Vec::new()
        },
    })
}
