//! The Anvil dialect: what the reader puts in a world that has no core meaning yet, as stored.

use primase_core::{Component, Entity, World};

/// A MeshShape's triangle geometry ([`crate::read::mesh_shape::Geometry`]) on its prototype: positions converted into
/// core's frame through the frame the reader was given (stated by the program, as the shape's units and frame are not
/// established), faces as position-index triangles (reversed when that frame mirrors, so their fronts stay), and each
/// face's edge ids and each edge's face ids as stored. `entry` and `index_id` name the forge entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMeshShape {
    pub entry: u32,
    pub index_id: u64,
    pub positions: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
    pub face_edges: Vec<[u32; 3]>,
    pub edge_faces: Vec<[u16; 2]>,
}

impl Component for AnvilMeshShape {
    const NAME: &'static str = "anvil.mesh_shape";
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.positions.len();
        match self
            .faces
            .iter()
            .flatten()
            .find(|&&i| usize::try_from(i).map_or(true, |i| i >= n))
        {
            Some(i) => Err(format!("a face names position {i} of {n}")),
            None => Ok(()),
        }
    }
    fn payload_bytes(&self) -> u64 {
        let bytes = self.positions.len() * 12 + self.faces.len() * 24 + self.edge_faces.len() * 4;
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

/// A Mesh's compiled buffers ([`crate::read::mesh::StoredMesh`]) on its prototype: positions decoded
/// ([`crate::read::mesh::StoredVertices::positions`]) and converted into core's frame through the frame the reader was
/// given (`frame`, stated by the program); triangles as vertex indices (each stored range counted from its vertex base,
/// reversed when that frame mirrors); `ranges` each stored range's first triangle; `materials` descriptor *k*'s
/// Material ClassID for range *k* (in order; descriptor *k*'s vertex count equals range *k*'s low 16 bits; how the
/// engine binds them is not established); the compiled mesh's `kind` byte, `stride` and each vertex's primary record
/// as stored (`vertex_records`, `stride` bytes each), their meaning beyond the position not established; and for kind
/// 2 (single-stream) stride 24, bytes 20..24 as two little-endian u16 (`words_20`, empty otherwise). `entry` and
/// `index_id` name the forge entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMesh {
    pub entry: u32,
    pub index_id: u64,
    pub frame: primase_core::frame::SourceFrame,
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub ranges: Vec<u32>,
    pub materials: Vec<u64>,
    pub kind: u8,
    pub stride: u32,
    pub vertex_records: Vec<u8>,
    pub words_20: Vec<[u16; 2]>,
}

impl Component for AnvilMesh {
    const NAME: &'static str = "anvil.mesh";
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.positions.len();
        let stride = usize::try_from(self.stride).unwrap_or(0);
        if self.vertex_records.len() != n.saturating_mul(stride) {
            return Err(format!(
                "{n} positions of stride {}, {} record bytes",
                self.stride,
                self.vertex_records.len()
            ));
        }
        if !self.words_20.is_empty() && self.words_20.len() != n {
            return Err(format!(
                "{n} positions, {} texture-coordinate words",
                self.words_20.len()
            ));
        }
        let starts_in_order = self.ranges.first().is_none_or(|&s| s == 0)
            && self.ranges.is_sorted()
            && self
                .ranges
                .last()
                .is_none_or(|&s| usize::try_from(s).is_ok_and(|s| s <= self.triangles.len()));
        if self.materials.len() != self.ranges.len() {
            return Err(format!(
                "{} materials for {} ranges",
                self.materials.len(),
                self.ranges.len()
            ));
        }
        if !starts_in_order {
            return Err(format!(
                "range starts {:?}: not from 0 in order within {} triangles",
                self.ranges,
                self.triangles.len()
            ));
        }
        match self
            .triangles
            .iter()
            .flatten()
            .find(|&&i| usize::try_from(i).map_or(true, |i| i >= n))
        {
            Some(i) => Err(format!("a triangle names vertex {i} of {n}")),
            None => Ok(()),
        }
    }
    fn payload_bytes(&self) -> u64 {
        let bytes = self.positions.len() * 24 + self.triangles.len() * 12;
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

/// A LODSelector's mesh levels on the prototype placed for it (`anvil.lods`): the prototype's own [`AnvilMesh`] is the
/// level `detail` (the finest level with a mesh), `forms` the other levels with a mesh (each `(level, entity)`, the
/// entity holding its [`AnvilMesh`], not placed). Levels referring to an Impostor or to nothing aren't forms.
/// `lower-anvil-lods` makes `core.lods` of it once the meshes are core meshes; which level the game draws when isn't
/// read (it chooses by distance), so no default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilLods {
    pub detail: u8,
    pub forms: Vec<(u8, Entity)>,
}

impl Component for AnvilLods {
    const NAME: &'static str = "anvil.lods";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.forms.iter().map(|(_, e)| *e));
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        for (level, form) in &self.forms {
            if *form == e || !w.is_alive(*form) {
                return Err(format!("level {level}'s form isn't another alive entity"));
            }
        }
        Ok(())
    }
    fn payload_bytes(&self) -> u64 {
        u64::try_from(self.forms.len() * 16).unwrap_or(u64::MAX)
    }
}

/// A placement's material overrides as its MeshInstanceData store them (`anvil.material_overrides`): per LOD level
/// (its LODSelectorInstance slot), the pairs of a source Material and the Material that replaces it (an identity pair
/// where none is replaced), by ClassID, in stored order. That a level's mesh range draws with the replacement of its
/// own Material is the Riders Republic notes' reading (975 of 975 sampled sources one of their slot mesh's range
/// Materials), not applied here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilMaterialOverrides {
    pub levels: Vec<(u8, Vec<(u64, u64)>)>,
}

impl Component for AnvilMaterialOverrides {
    const NAME: &'static str = "anvil.material_overrides";
    fn payload_bytes(&self) -> u64 {
        let pairs: usize = self.levels.iter().map(|(_, p)| p.len()).sum();
        u64::try_from(pairs * 16).unwrap_or(u64::MAX)
    }
}

/// A placement's DataLayerActions as its Entity stores them (`anvil.data_layers`): each a DataLayer's ClassID and the
/// u32 after it, in stored order. Which layers a world has active, and what the u32 selects, aren't read (the Riders
/// Republic notes' lead: 0 present while the layer is active, 1 removed while it is; docs/world.md §9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilDataLayers {
    pub actions: Vec<(u64, u32)>,
}

impl Component for AnvilDataLayers {
    const NAME: &'static str = "anvil.data_layers";
    fn payload_bytes(&self) -> u64 {
        u64::try_from(self.actions.len() * 12).unwrap_or(u64::MAX)
    }
}

/// A FloorHeightData block: two words and sixteen 2-bit indices between them, cell (r, c) of the block (r along y)
/// at bits 2 (4 r + c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloorBlock {
    pub lo: u16,
    pub hi: u16,
    pub indices: u32,
}

/// A FloorHeightData grid (`anvil.floor_height`): its cell side, the metres per word step and corner its trailer
/// stores, its cells along x and y, its 4x4-cell blocks row by row (a row along x), and the frame the program states
/// for its points. `lower-anvil-floor` makes ground of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilFloorHeight {
    pub frame: primase_core::frame::SourceFrame,
    pub cell: f32,
    pub scale: f32,
    pub origin: [f32; 2],
    pub cells: [u32; 2],
    pub blocks: Vec<FloorBlock>,
}

impl AnvilFloorHeight {
    /// The cell at `column` (along x) and `row` (along y): its block's words and its index (0 to 3); `None` outside
    /// the grid.
    pub fn cell(&self, column: u32, row: u32) -> Option<(u16, u16, u8)> {
        let [dx, dy] = self.cells;
        if column >= dx || row >= dy {
            return None;
        }
        let at = usize::try_from(row / 4).ok()? * usize::try_from(dx / 4).ok()?
            + usize::try_from(column / 4).ok()?;
        let b = self.blocks.get(at)?;
        let shift = ((row % 4) * 4 + column % 4) * 2;
        Some((b.lo, b.hi, u8::try_from((b.indices >> shift) & 3).ok()?))
    }
}

impl Component for AnvilFloorHeight {
    const NAME: &'static str = "anvil.floor_height";
    fn payload_bytes(&self) -> u64 {
        u64::try_from(self.blocks.len() * 8).unwrap_or(u64::MAX)
    }
}

/// One terrain node's decoded heights: its row (along y) and column (along x) at its level, and its 132 x 132
/// samples row by row (its 128 x 128 and the 4 its +x and +y neighbours start with), as the fraction of the terrain's
/// range the node stores (units of 2^(level - 28)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainTile {
    pub row: u32,
    pub column: u32,
    pub heights: Vec<i32>,
}

/// Terrain nodes of one level (`anvil.terrain`): their tiles, where the program states the quadtree's square lies
/// (`origin`, its corner with the least x and y, and `side`), and the frame it states for their points.
/// `lower-anvil-terrain` makes ground of them.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilTerrain {
    pub frame: primase_core::frame::SourceFrame,
    pub level: u32,
    pub origin: [f64; 2],
    pub side: f64,
    pub tiles: Vec<TerrainTile>,
}

impl Component for AnvilTerrain {
    const NAME: &'static str = "anvil.terrain";
    fn payload_bytes(&self) -> u64 {
        let samples: usize = self.tiles.iter().map(|t| t.heights.len()).sum();
        u64::try_from(samples * 4).unwrap_or(u64::MAX)
    }
}
