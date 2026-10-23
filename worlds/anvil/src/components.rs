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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
/// as stored (`vertex_records`, `stride` bytes each); and its attributes as the engine's vertex declarations decode
/// them (`attributes`, [`VertexAttributes`]: normals, tangents, texture coordinates, colour, in the source's frame). On a
/// mesh with bones, `skin` holds its bone list and skin stream as stored. `entry` and `index_id` name the forge entry it
/// came from.
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
    pub attributes: VertexAttributes,
    /// The bone list and skin stream, on a mesh with bones.
    pub skin: Option<AnvilSkin>,
}

/// A Mesh's vertex attributes as the engine's vertex declarations decode them ([`crate::read::vertex`]; each empty where
/// its layout declares none), in the source's frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VertexAttributes {
    /// Unnormalised, as the programs read them.
    pub normals: Vec<[f32; 3]>,
    /// xyz unnormalised, w the bitangent's sign (+1 or -1).
    pub tangents: Vec<[f32; 4]>,
    /// The texture coordinates of `TEXCOORD0` (or the split layouts' first set), then the second set.
    pub uvs: [Vec<[f32; 2]>; 2],
    /// RGBA, UNORM8.
    pub colour: Vec<[u8; 4]>,
    /// static16's two spherical vectors: `P` (bytes 10, 11) and `Q` (bytes 8, 9).
    pub spherical: Vec<[[f32; 3]; 2]>,
}

impl Component for AnvilMesh {
    const NAME: &'static str = "anvil.mesh";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.material_entities.iter().flatten());
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.positions.len();
        if let Some(s) = &self.skin
            && s.skin.vertices().count() != n
        {
            return Err(format!(
                "{n} positions, {} skinned vertices",
                s.skin.vertices().count()
            ));
        }
        let stride = usize::try_from(self.stride).unwrap_or(0);
        if self.vertex_records.len() != n.saturating_mul(stride) {
            return Err(format!(
                "{n} positions of stride {}, {} record bytes",
                self.stride,
                self.vertex_records.len()
            ));
        }
        let a = &self.attributes;
        let lengths = [
            ("normals", a.normals.len()),
            ("tangents", a.tangents.len()),
            ("texture coordinates 0", a.uvs[0].len()),
            ("texture coordinates 1", a.uvs[1].len()),
            ("vertex colours", a.colour.len()),
            ("spherical vectors", a.spherical.len()),
        ];
        if let Some((what, len)) = lengths.iter().find(|(_, len)| *len != 0 && *len != n) {
            return Err(format!("{n} positions, {len} {what}"));
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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

/// A collision placement's RigidBody filter (`anvil.collision_filter`; the reader's `CollisionFilter` reads it): its
/// `QueryFlags` and two `CollisionLayerBits` as bit masks by the registry's slots
/// ([`query`], [`layer`]), and its `IsGround` as stored (what it marks
/// isn't established: fences, lights and steel sheets among its 33,960, docs/placement.md §19.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilCollisionFilter {
    pub query: u64,
    pub layers: [u32; 2],
    pub ground: bool,
}

impl Component for AnvilCollisionFilter {
    const NAME: &'static str = "anvil.collision_filter";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

impl AnvilCollisionFilter {
    /// Whether `QueryFlags` slot `slot` ([`query`]) is set.
    pub fn queries(&self, slot: u32) -> bool {
        self.query.checked_shr(slot).is_some_and(|q| q & 1 == 1)
    }

    /// A water surface's: `WaterLevel` set and `Static` clear (on Riders Republic the river surfaces, docs/placement.md
    /// §19.2).
    pub fn is_water_surface(&self) -> bool {
        self.queries(query::WATER_LEVEL) && !self.queries(query::STATIC)
    }
}

/// `QueryLayerBits` slots by the registry's names (the others are unnamed or unset in the data).
pub mod query {
    pub const STATIC: u32 = 0;
    pub const DYNAMIC: u32 = 2;
    pub const CAMERA_COLLISION: u32 = 3;
    pub const CAMERA_OCCLUSION: u32 = 5;
    pub const CHARACTER: u32 = 9;
    pub const WATER_LEVEL: u32 = 10;
    pub const PARTICLE: u32 = 11;
    pub const RAYCAST: u32 = 19;
    pub const OCCLUSION_HEIGHT_TEST: u32 = 20;
    pub const TARGETING: u32 = 21;
    /// `TerrainLod0`; `TerrainLod1` to `4` follow.
    pub const TERRAIN_LOD0: u32 = 25;
}

/// `CollisionLayerBits` slots by the registry's names.
pub mod layer {
    pub const STATIC: u32 = 0;
    pub const DYNAMIC: u32 = 2;
    pub const DEBRIS: u32 = 3;
    pub const DETECTION_ZONE: u32 = 4;
    pub const MAIN_CHARACTER: u32 = 5;
    pub const CHARACTER: u32 = 6;
    pub const CHARACTER_NO_INTERACTIVE: u32 = 7;
    pub const WATER_LEVEL: u32 = 13;
    pub const TARGETABLE: u32 = 14;
    /// `TerrainLod0`; `TerrainLod1` to `4` follow.
    pub const TERRAIN_LOD0: u32 = 16;
}

/// The owning Entity's `IsHidden` and `IsPhantom` as stored (`anvil.entity_state`; the Entity bit-field run's bytes
/// 3 and 4, docs/placement.md §19.1), on every placement made for an Entity or EntityGroup that sets either: on Riders
/// Republic UI, HUD and editor helpers. `select-anvil-entities` keeps or drops them, as the program states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilEntityState {
    pub hidden: bool,
    pub phantom: bool,
}

impl Component for AnvilEntityState {
    const NAME: &'static str = "anvil.entity_state";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// The owning Entity's `BoundingVolume` as a box in core's frame (`anvil.bounds`): the stored volume's two corners
/// (axis-aligned, relative to the owner's translation in the source frame: docs/placement.md) placed at the owner's
/// translation and carried through the source frame's axes. On every visual placement and every collision-only
/// placement ([`AnvilCollisionOnly`]); `select-anvil-entities` reads it for `collision-only=overlap` and removes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnvilBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Component for AnvilBounds {
    const NAME: &'static str = "anvil.bounds";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// A collision placement whose owner (an Entity or EntityGroup) places no visual (`anvil.collision_only`): its
/// RigidBody has a shape and the owner has no Visual with something to show. On Riders Republic the `MAD_COL_*` blockers
/// (a RigidBodyComponent and nothing else). `select-anvil-entities` keeps, drops or tests it against the visuals'
/// bounds (`collision-only=`), and removes the mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilCollisionOnly;

impl Component for AnvilCollisionOnly {
    const NAME: &'static str = "anvil.collision_only";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// A placement whose MeshInstanceData is a fake (`anvil.fake`: its `IsFake` set; docs/placement.md §19.4): its
/// `FakeMeshIndex` and `IsNearFake`. On Riders Republic exactly the far terrain's cells (`MAD_USNAT_FarFake_<n>_Cell`,
/// the index `n`), which stand in for the near ground at a distance: a program leaves them out where it draws the
/// ground itself, or keeps them as a horizon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilFake {
    pub mesh_index: u32,
    pub near: bool,
}

impl Component for AnvilFake {
    const NAME: &'static str = "anvil.fake";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// One grid cell that lists an object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedBy {
    /// The grid's GridPartition entry name.
    pub grid: String,
    /// The cell's number (its position in the partition).
    pub cell: u32,
    /// The cell's level: 0 the finest (level 0's cells are the grid's smallest squares).
    pub level: u32,
}

/// Where a section read found an Entity or EntityGroup (`anvil.listed`): on every placement (and collision, lake and
/// decal entity) made for it. `cells` are the grid cells that list it itself; `reached_from` is the finest level of
/// any cell that reaches it, directly or as a member of a listed EntityGroup (members are placed with their group;
/// a member of a group a level-0 cell lists is reached from level 0 whatever lists it itself). An object larger than
/// a cell is listed only at a coarser level, so a program that keeps what stands on its section tells them apart
/// by `reached_from`: a stated choice (`select-anvil-listed`), never the reader's. Removed by the pass that makes it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilListed {
    /// The owner's ClassID (an Entity's or EntityGroup's).
    pub owner: u64,
    /// The frame the owner's placements were converted through (a placement's numbers are core's; the grid's units and
    /// the program's rectangles are the source's).
    pub frame: primase_core::frame::SourceFrame,
    pub cells: Vec<ListedBy>,
    pub reached_from: u32,
}

impl Component for AnvilListed {
    const NAME: &'static str = "anvil.listed";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// A prototype's level distances as its object stores them (`anvil.lod_distances`; registry-named-fields §2, §4,
/// docs/placement.md §19.5): each mesh level's `SwitchDistance` (its LODDescriptor's: the level's far limit, rising
/// level to level on all 686 selectors of two areas), finest first, as `(level, metres)`; and the last mesh level's
/// Mesh's `MaxCullingDistance` (0: none). A Mesh placed directly has no levels. The game's draw distance is
/// [`AnvilLodDistances::draw_distance`].
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilLodDistances {
    pub levels: Vec<(u8, f32)>,
    pub culling: f32,
}

impl AnvilLodDistances {
    /// The last level's switch distance, capped by its mesh's culling distance where that is set (above 0); `None`
    /// where neither is.
    pub fn draw_distance(&self) -> Option<f32> {
        let culling = (self.culling > 0.0).then_some(self.culling);
        match (self.levels.last().map(|&(_, d)| d), culling) {
            (Some(d), Some(c)) => Some(d.min(c)),
            (d, c) => d.or(c),
        }
    }
}

impl Component for AnvilLodDistances {
    const NAME: &'static str = "anvil.lod_distances";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        let pairs: usize = self.levels.iter().map(|(_, p)| p.len()).sum();
        u64::try_from(pairs * 16).unwrap_or(u64::MAX)
    }
}

/// A DDecalModel on its own entity (`anvil.decal_model`, named as its entry): its Diffuse, Normal, Height and Mask1
/// maps' textures (`None` where its reference is null or unread), its opacity `m` (the f32 at 72), its byte at 1,
/// flags and parameters as stored (docs/entity-grammar.md §9.3, material-spec §23.8).
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilDecalModel {
    pub textures: [Option<Entity>; 4],
    pub opacity: f32,
    pub byte: u8,
    pub flags: [u8; 20],
    pub parameters: Vec<u8>,
}

impl Component for AnvilDecalModel {
    const NAME: &'static str = "anvil.decal_model";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures.iter().flatten());
    }
}

/// One deferred decal: a projection box in the world (its two half-axes, its centre and its bounds' minimum and
/// maximum, in the source's frame; the owner's matrix not applied), its window of its texture's length (offset and
/// width, U units), its fade (its opacity `1 - fade` of its model's) and its 100-byte record as stored
/// ([`crate::read::ddecal`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeferredDecal {
    pub axis_x: [f32; 3],
    pub axis_y: [f32; 3],
    pub centre: [f32; 3],
    pub bounds: [[f32; 3]; 2],
    pub window: [f32; 2],
    pub fade: f32,
    pub record: [u8; 100],
}

/// One DDecalStaticInstancers item's decals (`anvil.deferred_decals`, on an entity of its own): the frame the program
/// states for their points, their DDecalModel's entity (`None` where it isn't read), the decals.
/// `lower-anvil-decals` makes quads of them.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilDeferredDecals {
    pub frame: primase_core::frame::SourceFrame,
    pub model: Option<Entity>,
    pub decals: Vec<DeferredDecal>,
}

impl Component for AnvilDeferredDecals {
    const NAME: &'static str = "anvil.deferred_decals";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        u64::try_from(self.decals.len() * std::mem::size_of::<DeferredDecal>()).unwrap_or(u64::MAX)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.model);
    }
}

/// A placement of one grass model (`anvil.grass_model`): the placement of a GrassVisual's Mesh at its Entity's matrix,
/// marked with the model's number (`GrassModelID`: the position in the setup's ManagedEntities and the library's
/// models, which the library's rules name) and the u16 array its GrassVisualInstance stores. The grass instances are
/// not stored: the engine creates them at run time from the library's rules and the terrain's group raster
/// (docs/grass.md), so this placement is the model's one copy, a prototype for a pass that scatters it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilGrassModel {
    pub model: u8,
    pub ranges: Vec<u16>,
}

impl Component for AnvilGrassModel {
    const NAME: &'static str = "anvil.grass_model";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// A GrassModel's settings, as the registry's class stores them (docs/grass.md §2.2): the six f32, the bytes and the
/// enum, the imposter shader levels, and the 23 bytes after them that the class list doesn't name.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassModelSettings {
    pub z_offset: f32,
    pub height_scale_for_occlusion: f32,
    pub terrain_normal_follower_ratio: f32,
    pub root_height: f32,
    pub softness: f32,
    pub normal_bend_factor: f32,
    pub visible_virtual_lod_count: u8,
    pub texture_atlas_element_count: u8,
    /// `RaycastTranslucency` (the enum `GrassModel::RaycastTranslucency`).
    pub raycast_translucency: u32,
    /// `ImposterStartShaderModelLodIndex` and the two after it: three bytes.
    pub imposter_shader_start_model_lod_index: [u8; 3],
    pub imposter_lookat_ratio: f32,
    /// The 23 bytes that follow, as stored: a LOD table of 5 bytes (`00 01 02 03 03`), four f32 and 2 bytes.
    pub unnamed: [u8; 23],
    /// The four f32 of `unnamed`: the model's LOD switch distances in metres by the notes' reading by value: (0, 10, 20,
    /// 40) on 26 models, (0, 5, 10, 20) on 16 and (0, 20, 40, 80) on 4 (counted over the 46 models).
    pub switch_distances: [f32; 4],
}

/// One model of the library: its settings and its bounding volume.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassModel {
    pub id: u64,
    pub settings: GrassModelSettings,
    /// The BoundingVolume object's id, its six f32 (min and max) and the 24 bytes after them as stored.
    pub bounds_id: u64,
    pub bounds: [f32; 6],
    pub bounds_rest: [u8; 24],
}

/// A rule: one model that may grow where its group is picked (a GrassPatternElement).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassRule {
    pub id: u64,
    /// `GrassModelIndex`: the position in the setup's ManagedEntities (and the library's models). 0 to 50 occur;
    /// 46 to 50 name no model.
    pub model: u16,
    pub tiling_log_count: u8,
    pub density: f32,
    pub tilt_factor: f32,
    pub scaling_enabled: bool,
    pub scale_a: f32,
    pub scale_b: f32,
    pub height_scaling_enabled: bool,
    pub height_scale_a: f32,
    pub height_scale_b: f32,
    /// The field `63D19E01` (5.0 on 145 rules, 0.0 on 66, 2.5 on others): not named.
    pub unnamed_f32: f32,
    /// The 49 bytes after it as stored (a byte 100, a u16, ten f32 of 1.0 and six bytes).
    pub unnamed: [u8; 49],
}

/// A group: the rules that apply where a ground sample picks it (a GrassPattern).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassGroup {
    pub id: u64,
    pub rules: Vec<StoredGrassRule>,
}

/// A GrassPatternLibrary body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGrassLibrary {
    pub models: Vec<StoredGrassModel>,
    pub groups: Vec<StoredGrassGroup>,
    /// The 256-byte table that follows the groups: the group of a material (`FF`: none; all but entry 255 on Riders
    /// Republic's, whose entry 255 is 184).
    pub table: [u8; 256],
    /// The 187 bytes after the table, as stored (zeros).
    pub rest: Vec<u8>,
}

/// The GrassComponent of `SETUP_GrassUS_NAT` as stored, less what only its reading needs: its ids, the inline
/// GrassSettings' body as stored, the ManagedEntities and the library it names.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassSystemSetup {
    /// The GrassComponent's own object id (its instance's).
    pub id: u64,
    /// The ClassID of the GrassPatternLibrary it names.
    pub library: Option<u64>,
    /// The inline GrassSettings object's id.
    pub settings_id: u64,
    /// Its `GrassVisibilityDistance` in metres (a u16 after its three u32 and its BoundingVolume: 250 on both copies).
    pub visibility_distance: Option<u16>,
    /// Its body as stored: WorldExtent, WorldMinZ and WorldMaxZ (u32), the WorldBoundingBox (a BoundingVolume), then
    /// `GrassVisibilityDistance` (a u16), 16 f32, four tables of 16 u16 (the LOD start distances by model class)
    /// and the run of floats and bytes the notes did not assign to field names (docs/grass.md §2.1).
    pub settings: Vec<u8>,
    /// ManagedEntities: the GrassVisual Entities' ids, in order (a model's number is its position).
    pub entities: Vec<u64>,
}

/// The grass system's setup (`anvil.grass`): the GrassComponent of `SETUP_GrassUS_NAT` ([`GrassSystemSetup`]) and the
/// GrassPatternLibrary it names (its models' settings, its groups of rules and its material table), on an entity of its
/// own.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilGrass {
    pub setup: GrassSystemSetup,
    pub library: StoredGrassLibrary,
}

impl Component for AnvilGrass {
    const NAME: &'static str = "anvil.grass";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// One terrain node's grass rasters: its row (along y) and column (along x) at its level, and its 132 x 132 samples
/// row by row of its slot 5 (the group of the library to use at each ground sample, 254: none) and slot 4 (the weight).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupTile {
    pub row: u32,
    pub column: u32,
    /// Block 5 of the node: a group's number in the library, or 254 where the node names none. By the engine's
    /// reading from the runtime image (docs/vegetation.md §3.4, docs/grass.md §3: slot 5 is the explicit raster of
    /// the grass builder, matched by its values and by 67 of 67 values naming a non-empty group).
    pub group: Vec<u8>,
    /// Block 4 of the node: the weight raster (96 on every node the notes decoded).
    pub weight: Vec<u8>,
}

/// The nodes of a terrain read ([`AnvilTerrain`]) with their grass rasters (`anvil.terrain_groups`), on the same entity:
/// the node level, the quadtree square and the frame as [`AnvilTerrain`] holds them.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilTerrainGroups {
    pub frame: primase_core::frame::SourceFrame,
    pub level: u32,
    pub origin: [f64; 2],
    pub side: f64,
    pub tiles: Vec<GroupTile>,
}

impl Component for AnvilTerrainGroups {
    const NAME: &'static str = "anvil.terrain_groups";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        let bytes: usize = self.tiles.iter().map(|t| t.group.len() + t.weight.len()).sum();
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

/// A bone as stored (see the module documentation for what each field is by the registry and by value).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredBone {
    /// The object's id: what a parent, the root and a mirror name.
    pub id: u64,
    /// The CRC-32 of the bone's name (the registry's two packed bit fields `41B24805` and `778FF80E`, 32 bits).
    pub name_hash: u32,
    /// The parent's id (none for a root).
    pub parent: Option<u64>,
    /// `230C4503`: the bone itself on a chain's bones, the opposite side's bone on a paired one, by value.
    pub mirror: Option<u64>,
    /// GlobalPosition: x, y, z and 0.
    pub global_position: [f32; 4],
    /// `02A2A3E3`, by value the global rotation: a unit quaternion x, y, z, w.
    pub global_rotation: [f32; 4],
    /// LocalPosition: in the parent's space, x, y, z and 0.
    pub local_position: [f32; 4],
    /// `8AE955A2`, by value the local rotation: a unit quaternion x, y, z, w.
    pub local_rotation: [f32; 4],
    /// `EF07E0C9`: 0 on all but one of the 11,113 bones read.
    pub flag: u8,
    /// `73196C8B`, an enum: 0 on 10,890 of 11,113 bones of the census, 1 to 5, 7, 8 and 14 on the others.
    pub kind: u32,
    /// `4007121E`, an array of u32 (a hash list like the skeleton's last array).
    pub hashes: Vec<u32>,
    /// `B58D6510`, an enum: 7 on every bone.
    pub kind2: u32,
    /// `8CE175E4`: 1.0 on every bone.
    pub unnamed: f32,
    /// `2DDF1266`: the low 16 bits the bone's position in the skeleton, the high 16 the bones below it, by value.
    pub key: u32,
}

/// One SkeletonPoseBone: its object's id and its 16 bytes as stored (no field list of the registry describes them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPoseBone {
    pub id: u64,
    pub bytes: [u8; 16],
}

/// One SkeletonPose: a f32 and its bones.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPose {
    pub id: u64,
    pub value: f32,
    pub bones: Vec<StoredPoseBone>,
}

/// One SkeletonPoseGroup: its poses.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPoseGroup {
    pub id: u64,
    pub poses: Vec<StoredPose>,
}

/// A Skeleton body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSkeleton {
    /// `9C78EB24`, an enum.
    pub kind: u32,
    pub bones: Vec<StoredBone>,
    /// `4E1F230B`: the root bone's id.
    pub root: Option<u64>,
    /// `F4D6F079`: the skeleton's own position (x, y, z, 0).
    pub position: [f32; 4],
    /// `9B868C7D`: its own rotation (a quaternion x, y, z, w by value).
    pub rotation: [f32; 4],
    /// The BodyPartMapping the skeleton names (its ClassID).
    pub body_part_mapping: Option<u64>,
    pub pose_groups: Vec<StoredPoseGroup>,
    /// The IKData object's id where the skeleton has one (the class stores no field).
    pub ik_data: Option<u64>,
    /// The LookAtDataSkeletonDefinition it names (its ClassID).
    pub look_at: Option<u64>,
    /// `F656FC33` and `5FC3FE25`.
    pub floats: [f32; 2],
    /// The Reflex3SkeletonConstraints object's id and its byte buffer (8 bytes on every skeleton that has one).
    pub reflex3: Option<(u64, Vec<u8>)>,
    /// `3708B98C`, an array of u32.
    pub hashes: Vec<u32>,
}

/// A Skeleton as stored (`anvil.skeleton`), on an entity of its own named for it: its bones, pose groups and the rest
/// ([`StoredSkeleton`]), and the frame the reader was given (the bones' positions and rotations are in the game's source
/// frame). `lower-anvil-skeleton` makes a `core.skeleton` of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilSkeleton {
    pub frame: primase_core::frame::SourceFrame,
    pub skeleton: StoredSkeleton,
}

impl Component for AnvilSkeleton {
    const NAME: &'static str = "anvil.skeleton";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// One WaterPatchDesc.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterPatch {
    /// Its object's u64 and type, as stored.
    pub id: u64,
    pub kind: u32,
    /// x and y in metres, z the water level.
    pub position: [f32; 3],
    /// The 4 bytes at 25, as stored (`00 00 01 02` on the notes' samples).
    pub unread: [u8; 4],
    /// u0, v0, u1, v1 in the atlases.
    pub rect: [f32; 4],
    pub level: u32,
}

impl WaterPatch {
    /// Its square's side in metres: `512 >> level` (`None` past 512's bits).
    pub fn side(&self) -> Option<f32> {
        512u32.checked_shr(self.level).map(|s| s as f32)
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
    pub patches: Vec<WaterPatch>,
    pub atlases: [Option<Entity>; 3],
}

impl Component for AnvilWater {
    const NAME: &'static str = "anvil.water";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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

impl TerrainTile {
    /// Where a node's samples begin in its 132-sample tile (every raster: heights, colour, material, block 8): its core
    /// sample (i, j), the 128 x 128 covering its square from its corner, is stored at (i + 2, j + 2); the two before and
    /// the two after are its neighbours'. The engine's reading (docs/terrain.md §3.6 correction, §3.7): its per-node
    /// constant block offsets every node 2 texels into its 132-texel block (`cb4[3]` = 2 mod 132 on 125 of 125 blocks in
    /// the world snapshot), and its physics terrain reads cell (r, c) at texel (r + 2, c + 2), its 129 x 129 vertices at
    /// texels 2 to 130. (A tile's last four equal its +x or +y neighbour's first four under either convention, so the
    /// rasters alone don't settle it.)
    pub const CORE_AT: usize = 2;
}

/// A terrain material's detail, as its TerrainMaterial and TextureSet give it: the f32 at 2 (`uv_scale`), the f32 at 6
/// (`f32_6`: the detail tile's size in metres, a lead) and the TextureSet's slot 0 map (`diffuse`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainDetail {
    pub uv_scale: f32,
    pub f32_6: f32,
    pub diffuse: Option<Entity>,
    /// Its TextureSet's slot 1 map: the detail normal (BC7, x and y; the notes, terrain.md §3.4).
    pub normal: Option<Entity>,
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.details.iter().flatten().filter_map(|d| d.diffuse));
        out.extend(self.details.iter().flatten().filter_map(|d| d.normal));
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
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

/// Where a texture binding's texture comes from, as the engine resolves it (Riders Republic docs/material-spec.md
/// §27.0, the notes' reading of the data: the Material's records override its TextureSet, which overrides the
/// template's defaults).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFrom {
    /// The Material's record for the binding (its property's key; a generic `Texture`'s also its slot and channel).
    Record,
    /// The Material's record with a null texture: whether the engine takes that as no texture or as absent (the
    /// fallback then) isn't read; the texture is none, the fallback kept.
    NullRecord { fallback: Option<Entity> },
    /// No record, channel 0: the Material's TextureSet's slot.
    Set,
    /// No record, channel 1 or 2: the template's default texture.
    Default,
    /// None of them (or the template's TextureParameter not found).
    Unset,
}

/// A texture binding of a Material's template resolved: its key, its shader slot `t(75 + shader_slot)`, the
/// TextureParameter's TextureSet slot and channel (none where the template's record isn't found), the texture and
/// where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundTexture {
    pub key: u32,
    pub shader_slot: u32,
    pub parameter: Option<(u32, u32)>,
    pub texture: Option<Entity>,
    pub from: TextureFrom,
}

/// A Material on its own entity (`anvil.material`): its template (ClassID and entry name), the template's bindings
/// and the Material's property values, each by the property's key, and each texture binding resolved (`textures`);
/// the frame the program states for the source's points (a template that maps its textures by the world position reads
/// it). `lower-anvil-material` makes PBR of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilMaterial {
    pub index_id: u64,
    pub template: Option<u64>,
    pub template_name: Option<String>,
    pub bindings: Vec<(u32, MaterialBinding)>,
    pub values: Vec<(u32, MaterialValue)>,
    /// Its TextureSet's 19 slots, each its TextureMap's texture entity (empty where it has no TextureSet).
    pub texture_set: Vec<Option<Entity>>,
    /// Its template's texture bindings, each resolved.
    pub textures: Vec<BoundTexture>,
    /// Its template's default value for each non-texture binding whose Operator holds one (a ConstantFloatValue's,
    /// ConstantVectorValue's or ConstantIntegerValue's `Value`, a UVTransformOperator's UVTransform as a Material's UV
    /// property's 8 floats): what the shader reads where the Material sets none (registry-named-fields §12).
    pub defaults: Vec<(MaterialBinding, MaterialValue)>,
    pub frame: primase_core::frame::SourceFrame,
    pub flags: MaterialFlags,
    /// Its template's shader permutations ([`AnvilShaders`]), where the reader reads them.
    pub shaders: Option<Entity>,
}

/// A lowered material whose shader maps its textures by the world position (`anvil.world_uv`; MAD_Outskirt): the two
/// rows taking a world point (core frame, `(x, y, z, 1)`) to its texture coordinates. `lower-anvil-world-uv` writes
/// them into `TexCoord(0)` per placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnvilWorldUv {
    pub rows: [[f64; 4]; 2],
}

impl Component for AnvilWorldUv {
    const NAME: &'static str = "anvil.world_uv";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// A Material's flags as stored: body 41 to 84 (of the 102 bytes after its references, 20 to 63), the fields the
/// engine's Material deserialiser reads there in its order, named by its class registry (Riders Republic
/// docs/shaders.md §6): `BlendMode` (a u32, of which the engine keeps 4 bits), `AlphaDisplayMode` (a u32, 2 bits),
/// `AlphaTestValue` and 35 booleans, a byte each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialFlags {
    pub blend_mode: u32,
    pub alpha_display_mode: u32,
    pub alpha_test_value: u8,
    /// Boolean j at body 50 + j.
    pub booleans: [u8; 35],
}

impl MaterialFlags {
    /// Booleans by their registry names (index j).
    pub const Z_WRITE_DISABLED_OPAQUE: usize = 0;
    pub const Z_WRITE_DISABLED_ALPHA: usize = 1;
    pub const Z_TEST_DISABLED: usize = 2;
    pub const LAYER_MATERIAL: usize = 3;
    pub const PRE_TRANSPARENT: usize = 10;
    pub const FLIP_NORMAL_ON_BACK_FACE: usize = 11;
    pub const RENDER_IN_LOW_RESOLUTION: usize = 15;
    pub const ALPHA_TEST_ENABLED: usize = 24;
    pub const TWO_SIDED: usize = 25;
    /// The boolean whose name's CRC-32 is `42452A12` (the name not known).
    pub const BOOLEAN_42452A12: usize = 34;
}

/// A MaterialTemplate's shader permutations (`anvil.shaders`, on an entity of its own, named as the template, that
/// its Materials name): each stage's ShaderKeys and their programs' DXBC containers, as stored
/// (`crate::read::template::permutations`). A key names a permutation within its stage (the engine's ShaderKey, bits
/// 22 to 29 its pass); the same key may be in both stages, each with its own program.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilShaders {
    /// (ShaderKey, index into `programs`) of the pixel stage, sorted by key: the engine looks a key up exactly.
    pub pixel: Vec<(u32, usize)>,
    /// The same for the vertex stage.
    pub vertex: Vec<(u32, usize)>,
    /// The containers, each once: permutations that share a program share its index.
    pub programs: Vec<Vec<u8>>,
    /// The body of the template's shader-state node as stored ([`crate::read::template::feature_node`]): the object's
    /// byte, then the template's feature flags.
    pub feature_node: Vec<u8>,
}

impl AnvilShaders {
    fn program(&self, stage: &[(u32, usize)], key: u32) -> Option<&[u8]> {
        let at = stage.binary_search_by_key(&key, |&(k, _)| k).ok()?;
        let &(_, program) = stage.get(at)?;
        self.programs.get(program).map(Vec::as_slice)
    }

    /// The stored byte of feature `n` (bit `n` of the engine's feature word `T`, which gates bits of the ShaderKey:
    /// docs/shaders.md §6.2): byte `n + 1` of the template's shader-state node, `None` where the node has no such byte.
    /// It is 0 or 1 for every feature but 15, which is also 2 on 82 of the 432 templates (see `tests/template_features.rs`).
    pub fn feature_byte(&self, n: usize) -> Option<u8> {
        self.feature_node.get(n.checked_add(1)?).copied()
    }

    /// Whether the template has feature `n`: its byte is not 0. The census (`tests/template_features.rs`) holds this to
    /// the key bit the feature gates on all 432 templates, for the byte 2 of feature 15 as for the byte 1 of the rest.
    pub fn feature(&self, n: usize) -> Option<bool> {
        self.feature_byte(n).map(|b| b != 0)
    }

    /// The program the pixel key `key` names.
    pub fn pixel(&self, key: u32) -> Option<&[u8]> {
        self.program(&self.pixel, key)
    }

    /// The program the vertex key `key` names.
    pub fn vertex(&self, key: u32) -> Option<&[u8]> {
        self.program(&self.vertex, key)
    }
}

impl Component for AnvilShaders {
    const NAME: &'static str = "anvil.shaders";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn payload_bytes(&self) -> u64 {
        let bytes: usize = self.programs.iter().map(Vec::len).sum();
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }
}

impl Component for AnvilMaterial {
    const NAME: &'static str = "anvil.material";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.values.iter().filter_map(|(_, v)| match v {
            MaterialValue::Texture { texture, .. } => *texture,
            _ => None,
        }));
        out.extend(self.texture_set.iter().flatten());
        out.extend(self.textures.iter().filter_map(|t| t.texture));
        out.extend(self.textures.iter().filter_map(|t| match t.from {
            TextureFrom::NullRecord { fallback } => fallback,
            _ => None,
        }));
        out.extend(self.shaders);
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
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// One `MeshBone` of a Mesh's bone list: a bone the mesh's skin stream may name, as the 81-byte record stores it
/// (an object id of 0, the class `MeshBone`, then the registry's three fields, `BoneID`, `InitialMatrixInverse` and
/// `IsUsedBySubMeshes`; docs/registry-named-fields.md §4).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMeshBone {
    /// `BoneID`: by value the CRC-32 of the bone's name, the key of a skeleton's [`StoredBone::name_hash`].
    pub bone_id: u32,
    /// `InitialMatrixInverse`: a 4x4 as 16 f32 in four rows of x, y, z and w, the translation in the last row.
    pub initial_matrix_inverse: [[f32; 4]; 4],
    /// `IsUsedBySubMeshes`.
    pub used_by_submeshes: bool,
}

/// A mesh's skin as its skin stream stores it (the split layouts'): each vertex a record of `influences` bone slots
/// (a byte each: positions in the mesh's bone list, [`crate::read::mesh::StoredMesh::bones`]) and then as many weights (a byte each,
/// summing to 255). Established on every one of Riders Republic's 29,174 meshes with bones (29,116 with 4 influences
/// and 58 with 8; 39,795,423 vertices: tests/skin_binding.rs): the weights sum to 255 on every vertex, and every slot
/// with a weight is below the bone count. A slot with no weight is kept as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSkin {
    /// The slots and weights of a vertex each: 4 or 8.
    pub influences: usize,
    /// Each vertex's slots, in vertex order, `influences` a vertex.
    pub slots: Vec<u8>,
    /// Each vertex's weights, in the same order.
    pub weights: Vec<u8>,
}

/// One bone of a rig.
#[derive(Clone, Debug, PartialEq)]
pub struct RigBone {
    /// The CRC-32 of its name.
    pub name_hash: u32,
    /// The name of its parent bone: none for a root.
    pub parent: Option<u32>,
    /// LocalPosition as stored (x, y, z, 0), in its parent's space.
    pub local_position: [f32; 4],
    /// The local rotation as stored: a unit quaternion x, y, z, w.
    pub local_rotation: [f32; 4],
    /// The position in the list given to [`StoredRig::assemble`] of the Skeleton the bone is from.
    pub skeleton: usize,
}

/// A rig (see the module documentation): each bone once, the main Skeleton's first, then each addon's new bones in the
/// order it stores them.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRig {
    pub bones: Vec<RigBone>,
}

/// A Mesh's skin as the reader keeps it, on the mesh's prototype with its [`AnvilMesh`]: the bone list (the slots the
/// skin stream names are positions in it) and the per-vertex slots and weights, both as stored. `lower-anvil-mesh` makes
/// joint and weight streams of it and leaves the bone list as [`AnvilPalette`].
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilSkin {
    pub bones: Vec<StoredMeshBone>,
    pub skin: StoredSkin,
}

/// What a lowered skinned mesh keeps for its binding (`anvil.palette`, left by `lower-anvil-mesh` on the entity whose
/// `AnvilMesh` had an [`AnvilSkin`]): the bone list with its inverse binds as stored, in the source's frame (`frame`),
/// how many four-influence sets the skin streams hold, and those streams, per primitive of the core mesh (`Joints(k)`
/// and `Weights(k)`, U8x4): a core mesh may not hold them before it has a `core.skin`. `lower-anvil-skin` binds it to a
/// rig and puts them in the mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilPalette {
    pub frame: primase_core::frame::SourceFrame,
    pub bones: Vec<StoredMeshBone>,
    pub sets: u8,
    pub streams: Vec<std::collections::BTreeMap<primase_core::Semantic, primase_core::Stream>>,
}

impl Component for AnvilPalette {
    const NAME: &'static str = "anvil.palette";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// The rig of a SkeletonComponent (`anvil.rig`), on an entity of its own: its main Skeleton's and addon Skeletons'
/// bones joined by name ([`StoredRig::assemble`]), and the main Skeleton's own position and rotation (the bones'
/// stored transforms leave it out), in the game's source frame (`frame`, as the reader was given it).
/// `lower-anvil-rig` makes a `core.skeleton` of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilRig {
    pub frame: primase_core::frame::SourceFrame,
    pub position: [f32; 4],
    pub rotation: [f32; 4],
    pub rig: StoredRig,
}

impl Component for AnvilRig {
    const NAME: &'static str = "anvil.rig";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// One placed SkeletonComponent (`anvil.rig_occurrence`), on an entity at its owner's transform: the rig it poses.
/// `lower-anvil-skin` makes a `core.rig_instance` of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilRigOccurrence {
    pub rig: Entity,
}

impl Component for AnvilRigOccurrence {
    const NAME: &'static str = "anvil.rig_occurrence";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.rig);
    }
}

/// A Visual's placement that follows its owner's SkeletonComponent (`anvil.skinned`): the owner's
/// [`AnvilRigOccurrence`]. `lower-anvil-skin` binds the placement's meshes to the rig and names the binding on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilSkinned {
    pub occurrence: Entity,
}

impl Component for AnvilSkinned {
    const NAME: &'static str = "anvil.skinned";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.occurrence);
    }
}

/// How a track's key times are stored ([`crate::read::animation`]: the tag's remainder over 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Times {
    /// A u8 for each key after the first (the first is at 0).
    U8(Vec<u8>),
    /// A u16 for each key after the first.
    U16(Vec<u16>),
    /// A bit mask of the frames that hold a key, with its marker byte's top two bits (`mode`: 1, 2 or 3).
    Mask { mode: u8, mask: Vec<u8> },
}

/// One track of an Animation as stored: its block's tag (`3 * kind + w`), its size field, the key count, the times and
/// the values (`keys` of the kind's width, as bytes). The bone it drives is the clip's mapping at its position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTrack {
    pub tag: u8,
    /// The block's `a` field, its in-memory size (within 8 of the stored payload's length on every clip).
    pub size: u32,
    pub keys: u32,
    pub times: Times,
    pub values: Vec<u8>,
}

/// An Animation's `AnimTrackEvent` array as stored: the count and the elements' bytes (not read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvents {
    pub count: u32,
    pub bytes: Vec<u8>,
}

/// An Animation's `TTECLAnimation` object, its four arrays as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEcl {
    pub id: u64,
    pub animation: Vec<u8>,
    pub words: Vec<u32>,
    pub halves: Vec<u16>,
    pub bytes: Vec<u8>,
}

/// An Animation resource's body as stored ([`crate::read::animation`] for what each field is).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredAnimation {
    /// `Length` (f32).
    pub length: f32,
    /// `F4F597FF`: varies by clip.
    pub id_hash: u32,
    /// `322B96B8`: 0 on every clip.
    pub f2: u32,
    /// `556E9166` and `251905D9`: empty on every clip.
    pub arrays: [Vec<u16>; 2],
    /// The four booleans (`1F4DA15F`, `93847F6A`, `ABC2D4B8`, `0D7D1D50`).
    pub flags: [u8; 4],
    pub events: StoredEvents,
    /// The `AnimTrackData` object's id (0 on every clip) and its `567530C5`.
    pub track_data_id: u64,
    pub track_data_hash: u32,
    /// Each track's bone: the CRC-32 of its name (0 for the root's), the mappings in the tracks' order.
    pub mappings: Vec<u32>,
    /// `66A6DC91`.
    pub track_data_tail: u16,
    pub ecl: Option<StoredEcl>,
    /// `B832D26F`, a byte buffer (empty on every clip).
    pub buffer: Vec<u8>,
    /// `CB4E5F18`.
    pub cb4e: u32,
    pub tracks: Vec<StoredTrack>,
}
