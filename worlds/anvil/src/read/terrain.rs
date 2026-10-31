//! A terrain's nodes of one level read into a world: those of a TerrainNodeLayer whose squares meet a rectangle, each
//! TNLChunk found through an [`Archive`], its height block decoded ([`anvil_formats::tnl_code::heights`]) and its colour
//! tile's blocks interleaved as stored ([`anvil_formats::tnl_code::tile`]).
//!
//! Where the quadtree's square lies is the program's stated choice, as nothing in the layer or its chunks stores it: the
//! Riders Republic notes place it on the world's GridLayout square ((-8192, -8192), side 16,384) by the colour tiles
//! against the game's world map (correlation 0.891, the other orientations 0.468 or less) and by 101 placed props
//! standing on the decoded heights there (to 0.1 m, docs/terrain.md §3.1 and §3.3).

use std::collections::{BTreeSet, HashMap};

use primase_core::Entity;

use primase_core::World;
use primase_core::frame::SourceFrame;

use crate::components::{
    AnvilTerrain, AnvilTerrainGroups, CollisionMaterialRef, GroupTile, TerrainDetail, TerrainTile,
};
use crate::read::textures::read_texture_set_slot;
use anvil_formats::archive::{Archive, At};
use anvil_formats::terrain_material::{
    StoredMaterialBank, StoredTerrainMaterial, TERRAIN_MATERIAL, TERRAIN_MATERIAL_BANK,
};
use anvil_formats::tnl::{StoredNodeLayer, StoredTnlChunk, TERRAIN_NODE_LAYER, TNL_CHUNK, node};
use anvil_formats::tnl_code;

/// Where a world's terrain quadtree lies and what its nodes name, as the program states them to the reader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSquare {
    /// The square's corner with the least x and y, and its side, in metres.
    pub origin: [f64; 2],
    pub side: f64,
    /// The level of its leaves.
    pub leaf_level: u32,
    /// The samples along a node's side.
    pub node_samples: u32,
    /// The TerrainMaterialBank its nodes' material rasters index, by entry name.
    pub bank: &'static str,
    /// The TextureSet of its splatting noise, by entry name.
    pub noise: &'static str,
}

impl TerrainSquare {
    /// The spacing of the leaves' samples: the side over the leaves along it and the samples along a leaf.
    pub fn leaf_spacing(&self) -> f64 {
        self.side / f64::from(1u32 << self.leaf_level.min(30)) / f64::from(self.node_samples)
    }
}

impl TerrainSquare {
    /// `rect` (x0, y0, x1, y1) grown by `margin` on each side and snapped outward to the leaves' sample spacing.
    pub fn cover(&self, rect: [f64; 4], margin: f64) -> [f64; 4] {
        let spacing = self.leaf_spacing();
        let down = |v: f64| (v / spacing).floor() * spacing;
        let up = |v: f64| (v / spacing).ceil() * spacing;
        [
            down(rect[0] - margin),
            down(rect[1] - margin),
            up(rect[2] + margin),
            up(rect[3] + margin),
        ]
    }
}

/// The ground the placements of `world` stand on, as a load scope for its terrain: `rect` widened to the box, in the
/// source's ground axes (x east, y north, through `frame`), of every placed `anvil.mesh`'s positions under its
/// placement's transform. Far-view proxies (`anvil.far_class`) are left out: they don't ask for near terrain under
/// them. Non-finite points are skipped.
pub fn placed_footprint(world: &World, frame: SourceFrame, rect: [f64; 4]) -> [f64; 4] {
    let mut extent = rect;
    for (entity, instance) in world.iter::<primase_core::Instance>() {
        if world.has::<crate::components::AnvilFarClass>(entity) {
            continue;
        }
        let Some(transform) = world.get::<primase_core::Transform>(entity) else {
            continue;
        };
        let Some(mesh) = world.get::<crate::components::AnvilMesh>(instance.prototype) else {
            continue;
        };
        for point in &mesh.positions {
            let [x, y, _] = frame.point_to_source(transform.apply(*point)).map(f64::from);
            if x.is_finite() && y.is_finite() {
                extent[0] = extent[0].min(x);
                extent[1] = extent[1].min(y);
                extent[2] = extent[2].max(x);
                extent[3] = extent[3].max(y);
            }
        }
    }
    extent
}

/// The open world's terrain. Observed, not read: the square is the world's GridLayout square ((-8192, -8192), side
/// 16,384) by the notes this module's documentation gives; its leaves are level 8, 128 samples a side (0.5 m); the
/// Terrain Entity names the bank `TMB_MAD` and the noise `TerrainSplattingNoise_Set`.
pub const USNAT_TERRAIN: TerrainSquare = TerrainSquare {
    origin: [-8192.0, -8192.0],
    side: 16384.0,
    leaf_level: 8,
    node_samples: 128,
    bank: "TMB_MAD",
    noise: "TerrainSplattingNoise_Set",
};

/// Why terrain can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] anvil_formats::archive::Error),
    #[error("{at:?}: type {found:#010x}, not {what}")]
    Type { at: At, found: u32, what: &'static str },
    #[error("{at:?}: {source}")]
    Layout {
        at: At,
        #[source]
        source: anvil_formats::tnl::Error,
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
    #[error("{at:?}: {source}")]
    Material {
        at: At,
        #[source]
        source: anvil_formats::terrain_material::Error,
    },
    #[error(transparent)]
    TextureMap(#[from] crate::read::textures::Error),
}

/// Each TerrainMaterial of the bank at `bank`, in its order: its CollisionMaterial, and its detail with its TextureSet's
/// id (the diffuse not yet read).
fn bank_materials(archive: &mut Archive, bank: At) -> Result<BankMaterials, Error> {
    let r = archive.resource(bank)?;
    if r.type_id != TERRAIN_MATERIAL_BANK {
        return Err(Error::Type {
            at: bank,
            found: r.type_id,
            what: "a TerrainMaterialBank",
        });
    }
    let b = StoredMaterialBank::parse(&r.body).map_err(|source| Error::Material { at: bank, source })?;
    let mut out = Vec::with_capacity(b.materials.len());
    let mut details = Vec::with_capacity(b.materials.len());
    for m in &b.materials {
        let Some(at) = archive.find(m.id) else {
            out.push(None);
            details.push(None);
            continue;
        };
        let r = archive.resource(at)?;
        if r.type_id != TERRAIN_MATERIAL {
            return Err(Error::Type {
                at,
                found: r.type_id,
                what: "a TerrainMaterial",
            });
        }
        let t = StoredTerrainMaterial::parse(&r.body).map_err(|source| Error::Material { at, source })?;
        details.push(Some((
            TerrainDetail {
                uv_scale: t.uv_scale,
                f32_6: t.f32_6,
                diffuse: None,
                normal: None,
            },
            t.texture_set,
        )));
        out.push(t.collision_material.map(|c| {
            let name = archive
                .find(c.id)
                .and_then(|at| archive.forge(at.forge)?.names.get(at.entry))
                .map(|n| n.name.clone());
            CollisionMaterialRef { id: c.id, name }
        }));
    }
    Ok((out, details))
}

/// A bank's materials: each one's CollisionMaterial, and its detail and TextureSet.
type BankMaterials = (
    Vec<Option<CollisionMaterialRef>>,
    Vec<Option<(TerrainDetail, Option<u64>)>>,
);

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
    /// The TerrainMaterialBank the nodes' material rasters index, where they are read with it (the terrain names
    /// none the layer reaches: the Riders Republic notes find it through the Terrain Entity, docs/terrain.md §4).
    pub bank: Option<At>,
    /// The TextureSet of the splatting noise (`TerrainSplattingNoise_Set`, which the Terrain Entity names), where it is
    /// read: its slot 0 map.
    pub noise: Option<At>,
    /// Whether the nodes' grass rasters (blocks 5 and 4: [`AnvilTerrainGroups`]) are read too.
    pub groups: bool,
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
    let (materials, details) = match what.bank {
        Some(bank) => bank_materials(archive, bank)?,
        None => (Vec::new(), Vec::new()),
    };
    let noise = match what.noise {
        Some(set) => {
            let id = archive
                .forge(set.forge)
                .and_then(|f| f.index.get(set.entry))
                .map(|i| i.id);
            match id {
                Some(id) => read_texture_set_slot(archive, w, id, 0)?,
                None => None,
            }
        }
        None => None,
    };
    let order = (what.bank.is_some() || what.groups).then(tnl_code::hilbert);
    let mut tiles = Vec::new();
    let mut group_tiles = Vec::new();
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
        let block_8 =
            tnl_code::tile(&chunk.blocks[8], true).map_err(|source| Error::Code { position, at, source })?;
        let material = match (&order, what.bank) {
            (Some(order), Some(_)) => {
                tnl_code::raster(&chunk.blocks[2], order)
                    .map_err(|source| Error::Code { position, at, source })?
                    .samples
            }
            _ => Vec::new(),
        };
        if let (Some(order), true) = (&order, what.groups) {
            let raster = |block: usize| {
                tnl_code::raster(&chunk.blocks[block], order)
                    .map(|r| r.samples)
                    .map_err(|source| Error::Code { position, at, source })
            };
            group_tiles.push(GroupTile {
                row,
                column,
                group: raster(5)?,
                weight: raster(4)?,
            });
        }
        tiles.push(TerrainTile {
            row,
            column,
            heights,
            colour,
            material,
            block_8,
        });
    }
    // the diffuse and normal of each material the tiles' rasters use, read once per TextureSet and slot
    let used: BTreeSet<u8> = tiles.iter().flat_map(|t| t.material.iter().copied()).collect();
    let mut sets: HashMap<(u64, usize), Option<Entity>> = HashMap::new();
    let mut read = Vec::with_capacity(details.len());
    for (index, d) in (0u8..=u8::MAX).zip(details) {
        read.push(match d {
            Some((mut detail, Some(set))) if used.contains(&index) => {
                let mut slot = |s: usize| -> Result<Option<Entity>, Error> {
                    Ok(match sets.get(&(set, s)) {
                        Some(&e) => e,
                        None => {
                            let e = read_texture_set_slot(archive, w, set, s)?;
                            sets.insert((set, s), e);
                            e
                        }
                    })
                };
                detail.diffuse = slot(0)?;
                detail.normal = slot(1)?;
                Some(detail)
            }
            Some((detail, _)) => Some(detail),
            None => None,
        });
    }
    let details = read;
    let e = w.spawn();
    if what.groups {
        w.insert(
            e,
            AnvilTerrainGroups {
                frame: what.frame,
                level: what.level,
                origin: what.origin,
                side: what.side,
                tiles: group_tiles,
            },
        );
    }
    w.insert(
        e,
        AnvilTerrain {
            frame: what.frame,
            level: what.level,
            origin: what.origin,
            side: what.side,
            tiles,
            materials,
            details,
            noise,
        },
    );
    Ok(e)
}
