//! The Anvil dialect: what the reader puts in a world that has no core meaning yet, as stored.

use anvil_formats::animation::StoredAnimation;
use anvil_formats::entity::query;
use anvil_formats::floor::FloorBlock;
use anvil_formats::grass::StoredGrassLibrary;
use anvil_formats::material::MaterialFlags;
use anvil_formats::mesh::{StoredMeshBone, StoredSkin};
use anvil_formats::rig::StoredRig;
use anvil_formats::skeleton::StoredSkeleton;
use anvil_formats::template::ShaderTables;
use anvil_formats::vertex::VertexAttributes;
use anvil_formats::water::WaterPatch;
use primase_core::{Component, Entity, World};

pub mod component_types;
mod coverage;
mod impostor;
pub use coverage::{
    AnvilGiDyn, AnvilGridMetadata, AnvilLookPreset, AnvilSourceComponents, AnvilSourceOwner, SourceComponent,
};
pub use impostor::{
    AnvilFarClass, AnvilFarSelected, AnvilFarTerrain, AnvilImpostor, AnvilRenderStatus, AnvilSky,
    AnvilSkyControllers, AnvilSpotLight, AnvilSurfaceBake,
};

/// A record of `anvil_formats` held as a component: the record as stored, its component name, checks and size given
/// by its `impl Component for Stored<..>` here. It derefs to the record and has the record's JSON (none).
#[derive(Clone, Debug, PartialEq)]
pub struct Stored<T>(pub T);

impl<T> std::ops::Deref for Stored<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Stored<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// A MeshShape's triangle geometry ([`anvil_formats::mesh_shape::Geometry`]) on its prototype: positions converted into
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

/// A Mesh's compiled buffers ([`anvil_formats::mesh::StoredMesh`]) on its prototype: positions decoded
/// ([`anvil_formats::mesh::StoredVertices::positions`]) and converted into core's frame through the frame the reader was
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
/// `FakeMeshIndex` and `IsNearFake`. On Riders Republic these identify `MAD_USNAT_FarFake_<n>_Cell` merged object
/// proxies (the index `n`), not the ground. Coarser terrain supplies far ground. A program must resolve their overlap
/// with real objects separately; `select-anvil-far-view` omits a merged proxy cell overlapping its near rectangle.
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
/// ([`anvil_formats::ddecal`]).
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
    /// The grid as the reader puts it in a world, its points in `frame` (the program's stated choice).
    pub fn new(
        stored: anvil_formats::floor::StoredFloorHeight,
        frame: primase_core::frame::SourceFrame,
    ) -> Self {
        let [x, y, _] = stored.trailer.origin;
        AnvilFloorHeight {
            frame,
            cell: stored.cell,
            scale: stored.trailer.scale,
            origin: [x, y],
            cells: stored.trailer.cells,
            blocks: stored.blocks,
        }
    }

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
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        Ok(Some(
            serde_json::json!({"cell_side": self.cell, "height_word_scale": self.scale,
            "origin_xy": self.origin, "cells": self.cells, "blocks": self.blocks.len(),
            "height_origin": "not inferred from the trailer; lowering requires caller's stated vertical policy"}),
        ))
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

/// What a MaterialTemplate binding binds (see [`anvil_formats::template::BindingKind`]).
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

/// A MaterialTemplate's shader permutations (`anvil.shaders`, on an entity of its own, named as the template, that
/// its Materials name): the template's [`ShaderTables`].
pub type AnvilShaders = Stored<ShaderTables>;

impl Component for Stored<ShaderTables> {
    const NAME: &'static str = "anvil.shaders";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// [`ShaderTables::check`].
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        self.0.check().map_err(|e| e.to_string())
    }
    fn payload_bytes(&self) -> u64 {
        let bytes: usize = self.0.programs.iter().map(Vec::len).sum();
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

/// The vertex layout number of each primitive of a lowered mesh (`anvil.layout`, left by `lower-anvil-mesh` beside the
/// core mesh it made): the compiled mesh's `kind` byte ([`AnvilMesh::kind`], the same for every primitive of one mesh),
/// which picks the vertex program a Material's binding holds for it (bits 17 to 21 of the vertex key). It is the key a
/// binding's vertex stages carry ([`crate::passes::MESH_LAYOUT_FIELD`], `anvil.layout.kind`): a consumer reads
/// `kind[primitive]` of the entity that owns the mesh. It describes the mesh's primitives, so it follows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnvilLayout {
    pub kind: Vec<u8>,
}

impl Component for AnvilLayout {
    const NAME: &'static str = "anvil.layout";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.kind, from).map(|kind| AnvilLayout { kind }))
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        match w.get::<primase_core::Mesh>(e) {
            Some(m) if m.primitives().len() != self.kind.len() => Err(format!(
                "{} layout numbers for {} primitives",
                self.kind.len(),
                m.primitives().len()
            )),
            _ => Ok(()),
        }
    }
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

/// An Animation read for a rig (`anvil.clip`), on an entity named for it: the clip as stored and the rig it drives (the
/// entity holding its `core.skeleton`), in the source's frame (`frame`, as the reader was given it). Each track names
/// its bone by the CRC-32 of the bone's name; the rig's joints are named by that hash. `lower-anvil-clips` makes a
/// `core.clip` of it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilClip {
    pub frame: primase_core::frame::SourceFrame,
    /// The Animation's resource id.
    pub id: u64,
    pub animation: StoredAnimation,
    pub rig: Entity,
}

impl Component for AnvilClip {
    const NAME: &'static str = "anvil.clip";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.rig);
    }
}

/// An Animation of the forges read once for the whole library (shared by every rig it fits, [`AnvilClipCandidates`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryClip {
    /// The Animation's resource id and its entry's name.
    pub id: u64,
    pub name: String,
    pub animation: StoredAnimation,
    /// The bone hash of each track that drives a joint (a translation or rotation track of a bone, not the root's
    /// motion, hash 0), in the tracks' order.
    pub joint_tracks: Vec<u32>,
}

/// One clip of the library that fits a rig: `covered` of its `clip.joint_tracks` name a joint of the rig.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipCandidate {
    pub clip: std::sync::Arc<LibraryClip>,
    pub covered: u32,
}

/// The library's clips that fit a rig (`anvil.clip_candidates`, on the rig's entity): every Animation with at least
/// one joint-driving track on one of the rig's joints, with how many of its tracks that is. Which of them play is not
/// read: `select-anvil-clips` chooses by this coverage.
#[derive(Clone, Debug, PartialEq)]
pub struct AnvilClipCandidates {
    pub frame: primase_core::frame::SourceFrame,
    pub candidates: Vec<ClipCandidate>,
}

impl Component for AnvilClipCandidates {
    const NAME: &'static str = "anvil.clip_candidates";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
}

/// An Animation an Entity plays, bound to the Entity's placed rig (`anvil.clip_binding`, on an entity of its own): the
/// clip ([`AnvilClip`]) and the rig occurrence ([`AnvilRigOccurrence`]) that plays it. `lower-anvil-clips` makes a
/// `core.clip_binding` of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnvilClipBinding {
    pub clip: Entity,
    pub occurrence: Entity,
}

impl Component for AnvilClipBinding {
    const NAME: &'static str = "anvil.clip_binding";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.clip);
        out.push(self.occurrence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_formats::floor::{FloorBlock, StoredFloorHeight, Trailer};

    #[test]
    fn a_floor_grid_takes_its_corner_and_its_cells_take_their_indices_in_order() {
        // one 4x4-cell block, lo 100, hi 400; cell (r, c) index (4 r + c) mod 4
        let bits: u32 = (0..16).map(|k| (k % 4) << (2 * k)).sum();
        let stored = StoredFloorHeight {
            cell: 2.0,
            blocks: vec![FloorBlock {
                lo: 100,
                hi: 400,
                indices: bits,
            }],
            trailer: Trailer {
                scale: 0.061_879_4,
                unknown: 338.86,
                origin: [-4.0, -4.0, -3584.0],
                cells: [4, 4],
                rest: [0; 15],
            },
        };
        let g = AnvilFloorHeight::new(stored, primase_core::frame::SourceFrame::WORLD);
        assert_eq!((g.origin, g.cells), ([-4.0, -4.0], [4, 4]));
        assert_eq!(g.cell(3, 0), Some((100, 400, 3)));
        assert_eq!(g.cell(1, 2).map(|c| c.2), Some(1));
        assert_eq!(g.cell(4, 0), None);
    }
}
