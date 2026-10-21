//! A terrain's nodes of one level read into a world: those of a TerrainNodeLayer whose squares meet a rectangle, each
//! TNLChunk found through an [`Archive`], its height block decoded ([`crate::read::tnl_code::heights`]) and its colour
//! tile's blocks interleaved as stored ([`crate::read::tnl_code::tile`]).
//!
//! Where the quadtree's square lies is the program's stated choice, as nothing in the layer or its chunks stores it: the
//! Riders Republic notes place it on the world's GridLayout square ((-8192, -8192), side 16,384) by the colour tiles
//! against the game's world map (correlation 0.891, the other orientations 0.468 or less) and by 101 placed props
//! standing on the decoded heights there (to 0.1 m, docs/terrain.md §3.1 and §3.3).

use primase_core::Entity;
use primase_core::World;
use primase_core::frame::SourceFrame;

use crate::components::{AnvilTerrain, TerrainTile};
use crate::read::archive::{Archive, At};
use crate::read::tnl::{StoredNodeLayer, StoredTnlChunk, TERRAIN_NODE_LAYER, TNL_CHUNK, node};
use crate::read::tnl_code;

/// Why terrain can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] crate::read::archive::Error),
    #[error("{at:?}: type {found:#010x}, not {what}")]
    Type { at: At, found: u32, what: &'static str },
    #[error("{at:?}: {source}")]
    Layout {
        at: At,
        #[source]
        source: crate::read::tnl::Error,
    },
    #[error("node {position} ({at:?}): {source}")]
    Code {
        position: u32,
        at: At,
        #[source]
        source: tnl_code::Error,
    },
    #[error("node {position}: chunk {chunk:#x} in no forge")]
    Missing { position: u32, chunk: u64 },
    #[error("node {position}: a height outside 32 bits")]
    Range { position: u32 },
}

/// Where the program states the quadtree lies, and what of it to read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainRead {
    /// The square's corner with the least x and y, and its side.
    pub origin: [f64; 2],
    pub side: f64,
    /// The level whose nodes are read (8, the leaves, at 0.5 m in Riders Republic).
    pub level: u32,
    /// x0, y0, x1, y1: the nodes whose squares meet it are read.
    pub rect: [f64; 4],
    pub frame: SourceFrame,
}

/// The nodes of the layer at `layer` that `what` names, as an entity holding their [`AnvilTerrain`].
pub fn read_terrain(
    w: &mut World,
    archive: &mut Archive,
    layer: At,
    what: TerrainRead,
) -> Result<Entity, Error> {
    let r = archive.resource(layer)?;
    if r.type_id != TERRAIN_NODE_LAYER {
        return Err(Error::Type {
            at: layer,
            found: r.type_id,
            what: "a TerrainNodeLayer",
        });
    }
    let items = StoredNodeLayer::parse(&r.body)
        .map_err(|source| Error::Layout { at: layer, source })?
        .items;
    let node_side = what.side / f64::from(1u32 << what.level.min(30));
    let [x0, y0, x1, y1] = what.rect;
    let [ox, oy] = what.origin;
    let mut tiles = Vec::new();
    for item in items {
        let Some((level, row, column)) = node(item.position) else {
            continue;
        };
        let (nx, ny) = (
            ox + node_side * f64::from(column),
            oy + node_side * f64::from(row),
        );
        if level != what.level || nx > x1 || nx + node_side < x0 || ny > y1 || ny + node_side < y0 {
            continue;
        }
        let position = item.position;
        let at = archive.find(item.chunk).ok_or(Error::Missing {
            position,
            chunk: item.chunk,
        })?;
        let c = archive.resource(at)?;
        if c.type_id != TNL_CHUNK {
            return Err(Error::Type {
                at,
                found: c.type_id,
                what: "a TNLChunk",
            });
        }
        let chunk = StoredTnlChunk::parse(&c.body).map_err(|source| Error::Layout { at, source })?;
        let h = tnl_code::heights(&chunk.blocks[0]).map_err(|source| Error::Code { position, at, source })?;
        let heights = h
            .values
            .into_iter()
            .map(|v| i32::try_from(v).map_err(|_| Error::Range { position }))
            .collect::<Result<_, _>>()?;
        let colour =
            tnl_code::tile(&chunk.blocks[1], false).map_err(|source| Error::Code { position, at, source })?;
        tiles.push(TerrainTile {
            row,
            column,
            heights,
            colour,
        });
    }
    let e = w.spawn();
    w.insert(
        e,
        AnvilTerrain {
            frame: what.frame,
            level: what.level,
            origin: what.origin,
            side: what.side,
            tiles,
        },
    );
    Ok(e)
}
