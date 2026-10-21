//! The Anvil dialect: what the reader puts in a world that has no core meaning yet, as stored.

use primase_core::{Component, Entity, World};

/// A MeshShape's triangle geometry ([`crate::read::mesh_shape::Geometry`]) on its prototype: positions converted into
/// core's frame through the frame the reader was given (stated by the program, as the shape's units and frame are not
/// established), faces as position-index triangles (reversed when that frame mirrors, so their fronts stay), and each
/// face's edge ids and each edge's face ids as stored, and the CollisionMaterials its tail names with each face's index
/// into them. `entry` and `index_id` name the forge entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMeshShape {
    pub entry: u32,
    pub index_id: u64,
    pub positions: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
    pub face_edges: Vec<[u32; 3]>,
    pub edge_faces: Vec<[u16; 2]>,
    /// The CollisionMaterials its tail names, in stored order (`None`: a null reference).
    pub materials: Vec<Option<CollisionMaterialRef>>,
    /// Each face's index into `materials`, as stored (empty: every face the first).
    pub face_materials: Vec<u8>,
}

/// A CollisionMaterial a collision shape names: its ClassID, and its entry's name where a forge read holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionMaterialRef {
    pub id: u64,
    pub name: Option<String>,
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
            None if !self.face_materials.is_empty() && self.face_materials.len() != self.faces.len() => {
                Err(format!(
                    "{} face materials for {} faces",
                    self.face_materials.len(),
                    self.faces.len()
                ))
            }
            None => match self
                .face_materials
                .iter()
                .find(|&&k| usize::from(k) >= self.materials.len())
            {
                Some(k) => Err(format!("a face names material {k} of {}", self.materials.len())),
                None => Ok(()),
            },
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
/// as stored (`vertex_records`, `stride` bytes each), their meaning beyond the position not established; and each
/// vertex's two little-endian u16 where its layout keeps its texture coordinates (`uv_words`, from `uv_at`; empty
/// where that isn't known); and for kind 2 (stride 24) each vertex's bytes 16..20 as stored (`colour`, empty
/// otherwise): its vertex colour COLOR0, four UNORM8 (the Riders Republic notes, docs/vertex.md: the vertex programs pass
/// COLOR0 on and the pixel programs multiply it in, the bytes 255 on unpainted meshes, MAD_Tex2's alpha varying where its
/// shader blends by it; the channel order R, G, B, A likely, one asymmetry). `entry` and `index_id` name the forge
/// entry it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMesh {
    pub entry: u32,
    pub index_id: u64,
    pub frame: primase_core::frame::SourceFrame,
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub ranges: Vec<u32>,
    pub materials: Vec<u64>,
    /// Each range's Material entity, where the reader read the Materials (empty otherwise).
    pub material_entities: Vec<Option<Entity>>,
    pub kind: u8,
    pub stride: u32,
    pub vertex_records: Vec<u8>,
    pub uv_at: UvWordsAt,
    pub uv_words: Vec<[u16; 2]>,
    pub colour: Vec<[u8; 4]>,
}

/// Where in each vertex's primary record a Mesh's texture-coordinate words are read, by its layout (the Riders
/// Republic notes, docs/vertex.md: their classes as stated there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvWordsAt {
    /// Not read: the layout's place isn't known.
    None,
    /// Bytes 20..24 of kind 2 (stride 24) and kind 3 (stride 28): the tangent the words' gradient gives meets the
    /// stored tangent (median 0.52 degrees on 40 stride-24 meshes, 0.56 on 53 stride-28; bytes 24..27 of stride 28 at
    /// 89.8, not these).
    Bytes20,
    /// Bytes 12..16 of kind 11 (stride 16, the far terrain cells): RRAssetStudio's reading; on all 1,031 the words
    /// lie in 0..2045 and change smoothly across the triangles, and no stored tangent witnesses them.
    Bytes12,
    /// Bytes 4..8 of the split layouts' primary record (kind 0, stride 12; kind 1, stride 24): RRAssetStudio's
    /// reading; the smoothest word pair of the record on 80 sampled meshes, and the gradient meets the tool-decoded
    /// tangent at 3.1 and 5.7 degrees median (that tangent's decode the tool's).
    Primary4,
}

impl UvWordsAt {
    /// Where a layout of `kind` and primary `stride` keeps them.
    pub fn of(kind: u8, stride: usize) -> UvWordsAt {
        match (kind, stride) {
            (2, 24) | (3, 28) => UvWordsAt::Bytes20,
            (11, 16) => UvWordsAt::Bytes12,
            (0, 12) | (1, 24) => UvWordsAt::Primary4,
            _ => UvWordsAt::None,
        }
    }

    /// Their offset in the record.
    pub fn offset(self) -> Option<usize> {
        match self {
            UvWordsAt::None => None,
            UvWordsAt::Bytes20 => Some(20),
            UvWordsAt::Bytes12 => Some(12),
            UvWordsAt::Primary4 => Some(4),
        }
    }
}

impl Component for AnvilMesh {
    const NAME: &'static str = "anvil.mesh";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.material_entities.iter().flatten());
    }
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
        if !self.colour.is_empty() && self.colour.len() != n {
            return Err(format!("{n} positions, {} vertex colours", self.colour.len()));
        }
        if !self.uv_words.is_empty() && self.uv_words.len() != n {
            return Err(format!(
                "{n} positions, {} texture-coordinate words",
                self.uv_words.len()
            ));
        }
        let starts_in_order = self.ranges.first().is_none_or(|&s| s == 0)
            && self.ranges.is_sorted()
            && self
                .ranges
                .last()
                .is_none_or(|&s| usize::try_from(s).is_ok_and(|s| s <= self.triangles.len()));
        if !self.material_entities.is_empty() && self.material_entities.len() != self.ranges.len() {
            return Err(format!(
                "{} material entities for {} ranges",
                self.material_entities.len(),
                self.ranges.len()
            ));
        }
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

/// A lake's surface as its WaterComponent stores it (`anvil.water`, on an entity of its own): its patches (each a
/// square of side `512 >> level` metres from its position, at the water level; in world coordinates, the owner's
/// matrix not applied: every one of the 2,062 lies in its owner's 1 km cell, which its matrix's translation
/// centres) and its three atlases (Flow, Params, LocalProfile; `None` where the lake has none or one can't be read).
/// `lower-anvil-water` makes surfaces of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilWater {
    pub frame: primase_core::frame::SourceFrame,
    pub patches: Vec<crate::read::water::WaterPatch>,
    pub atlases: [Option<Entity>; 3],
}

impl Component for AnvilWater {
    const NAME: &'static str = "anvil.water";
    fn payload_bytes(&self) -> u64 {
        u64::try_from(self.patches.len() * 49).unwrap_or(u64::MAX)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.atlases.iter().flatten());
    }
}

/// A placement's DataLayerActions as its Entity stores them (`anvil.data_layers`), in stored order.
/// `lower-anvil-data-layers` makes world state of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilDataLayers {
    pub actions: Vec<DataLayerAction>,
}

/// One DataLayerAction: its DataLayer's ClassID and entry name (where a forge read names it), and the u32 after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataLayerAction {
    pub layer: u64,
    pub name: Option<String>,
    pub word: u32,
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
    /// Its colour tile as stored: 33 x 33 BC1 blocks row by row (8 bytes each, two RGB565 words then sixteen 2-bit
    /// indices), 132 x 132 texels, texel (i, j) at sample (i, j).
    pub colour: Vec<u8>,
    /// Its material raster (block 2), 132 x 132 values row by row, each an index into the terrain's material bank
    /// (empty where no bank was read).
    pub material: Vec<u8>,
    /// Its block 8 as stored: 33 x 33 BC4 blocks row by row (two endpoints, then sixteen 3-bit indices), one channel
    /// of 132 x 132 texels (the terrain shader's occlusion-like factor `0.5 + v`: the notes' lead, docs/terrain.md
    /// §3.4).
    pub block_8: Vec<u8>,
}

/// A terrain material's detail, as its TerrainMaterial and TextureSet give it: the f32 at 2 (`uv_scale`), the f32 at 6
/// (`f32_6`: the detail tile's size in metres, a lead) and the TextureSet's slot 0 map (`diffuse`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainDetail {
    pub uv_scale: f32,
    pub f32_6: f32,
    pub diffuse: Option<Entity>,
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
    /// Each material of the terrain's bank, in the order its rasters index them: its TerrainMaterial's
    /// CollisionMaterial (empty where no bank was read).
    pub materials: Vec<Option<CollisionMaterialRef>>,
    /// Each material's detail, in the same order (empty where no bank was read).
    pub details: Vec<Option<TerrainDetail>>,
    /// The splatting noise texture (its TextureSet's slot 0), where the program names the set.
    pub noise: Option<Entity>,
}

impl Component for AnvilTerrain {
    const NAME: &'static str = "anvil.terrain";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.details.iter().flatten().filter_map(|d| d.diffuse));
        out.extend(self.noise);
    }
    fn payload_bytes(&self) -> u64 {
        let bytes: usize = self
            .tiles
            .iter()
            .map(|t| t.heights.len() * 4 + t.colour.len())
            .sum();
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

/// A collision primitive's geometry in its owner's local frame, converted through the frame the reader was given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeGeometry {
    Sphere {
        centre: [f32; 3],
        radius: f32,
    },
    /// The segment `a`-`b` swept by `radius`.
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    /// Centred on its placement's origin, along its axes (a BoxShape's own transform is in its placement's).
    Box {
        half_extents: [f32; 3],
    },
}

/// A SphereShape, CapsuleShape or BoxShape on its prototype (`anvil.collision_shape`): its geometry and its
/// CollisionMaterial. `entry` and `index_id` name the forge entry it came from.
/// `lower-anvil-shapes` makes core colliders of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilCollisionShape {
    pub entry: u32,
    pub index_id: u64,
    pub geometry: ShapeGeometry,
    pub material: Option<CollisionMaterialRef>,
}

impl Component for AnvilCollisionShape {
    const NAME: &'static str = "anvil.collision_shape";
}

/// What a MaterialTemplate binding binds (see [`crate::read::template::BindingKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialBinding {
    Texture { slot: u32 },
    Scalar { vec4: u32 },
    Vector { vec4: u32 },
    UvTransform { vec4: u32 },
    Other,
}

/// A Material property's value, as its record stores it.
#[derive(Clone, Debug, PartialEq)]
pub enum MaterialValue {
    Int(i32),
    Float(f32),
    Float4([f32; 4]),
    /// A texture: its TextureMap's entity (`None`: the record refers to none, or it couldn't be read), and the
    /// record's TextureSet slot and channel.
    Texture {
        texture: Option<Entity>,
        slot: u32,
        channel: u32,
    },
    /// Eight or six f32 (two payloads that differ only in their count), as stored.
    Floats(Vec<f32>),
    /// Another payload, not read here.
    Other,
}

/// A Material on its own entity (`anvil.material`): its template (ClassID and entry name), the template's bindings
/// and the Material's property values, each by the property's key. `lower-anvil-material` makes PBR of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMaterial {
    pub index_id: u64,
    pub template: Option<u64>,
    pub template_name: Option<String>,
    pub bindings: Vec<(u32, MaterialBinding)>,
    pub values: Vec<(u32, MaterialValue)>,
    /// Its TextureSet's 19 slots, each its TextureMap's texture entity (empty where it has no TextureSet).
    pub texture_set: Vec<Option<Entity>>,
    /// The u32 at 20 and the bytes at 28 and 53 of the 102 bytes after its two references (body 41, 49 and 74), as
    /// stored ([`crate::read::material::StoredMaterial::prelude_20_28_53`]).
    pub prelude_20: u32,
    pub prelude_28: u8,
    pub prelude_53: u8,
}

impl Component for AnvilMaterial {
    const NAME: &'static str = "anvil.material";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.values.iter().filter_map(|(_, v)| match v {
            MaterialValue::Texture { texture, .. } => *texture,
            _ => None,
        }));
        out.extend(self.texture_set.iter().flatten());
    }
}

/// What the reader keeps of a TextureMap on its texture's entity (`anvil.texture_map`): its ClassID, engine format
/// code and the u32 at 25 (1 on maps the game samples as sRGB, by the Riders Republic notes' size-bucket join; a
/// stated choice for the lowering).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilTextureMap {
    pub index_id: u64,
    pub code: u32,
    pub field_25: u32,
}

impl Component for AnvilTextureMap {
    const NAME: &'static str = "anvil.texture_map";
}
