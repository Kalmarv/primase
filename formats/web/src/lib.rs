//! Web tile sets in the core frame (metres, +Y up). Tiles use east/north ground coordinates.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use primase_core::convert::{Notice, NoticeKind};
use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, Name, Primitive, Topology, Transform, TransformKind, World,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
mod collision;
mod compression;
pub mod convert;
mod images;
mod instancing;
mod provenance;
mod spanning;
pub use images::{IMAGE_DIR, ImageFiles};
pub use spanning::{LocalBox, PIVOT_MARGIN, Spans, spanning};
pub mod fixture;
#[cfg(feature = "ktx2")]
pub mod ktx2;
#[cfg(feature = "ktx2")]
pub mod prepare;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid web scene: {0}")]
    Invalid(String),
    #[error(transparent)]
    Gltf(#[from] primase_gltf::Error),
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
pub struct Options {
    /// The cells' side: how big a tile is, the program's choice.
    pub tile_size: TileSize,
    pub gpu_instancing: bool,
    /// Quantize vertex attributes and encode vertex streams with meshopt.
    pub compress: bool,
    /// Optional web texture lowering; PNG embedding remains the default.
    #[cfg(feature = "ktx2")]
    pub ktx2: bool,
    /// KTX2 lowering: a disk cache of encoded images, kept across runs (`None`: encode every run).
    #[cfg(feature = "ktx2")]
    pub cache: Option<std::sync::Arc<primase_cache::Cache>>,
    /// KTX2 lowering: encode on every cache hit as well and fail when the stored value differs.
    #[cfg(feature = "ktx2")]
    pub cache_verify: bool,
    /// KTX2 lowering: at most this many bytes of encodings held in memory between tiles, the least recently used
    /// dropped first ([`ktx2::DEFAULT_ENCODING_MEMORY`]); a dropped one comes back from the disk cache or is encoded
    /// again.
    #[cfg(feature = "ktx2")]
    pub encoding_memory: usize,
    /// Viewer-only records, already lowered from a source dialect into world space.
    pub viewer_items: Vec<ViewerItem>,
    /// Placement entities to partition by transformed element centre (a triangle's, a line segment's, a point),
    /// rather than pivot.
    pub split_by_tile: BTreeSet<Entity>,
    /// The largest file a tile may carry, its GLB and its collision sidecar alike: a cell with a larger one is split
    /// into quadtree [`Subcell`]s, recursively, until each file fits, each split reported. `None` (by default) keeps
    /// every cell whole: the budget is the program's choice.
    pub tile_byte_budget: Option<u64>,
    /// A starting view, written to the index as `camera`; `None`: a consumer frames the scene itself.
    pub camera: Option<Camera>,
}
/// A starting view in the index's world frame, metres: where the eye is and the point it looks at.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Camera {
    pub eye: [f64; 3],
    pub target: [f64; 3],
}
/// A cell's side in metres: finite and above 0.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct TileSize(f64);
impl TileSize {
    /// `metres`: an error unless finite and above 0.
    pub fn new(metres: f64) -> Result<TileSize> {
        if metres.is_finite() && metres > 0. {
            Ok(TileSize(metres))
        } else {
            Err(Error::Invalid(format!(
                "tile size {metres} is not finite and above 0 metres"
            )))
        }
    }
    pub fn get(self) -> f64 {
        self.0
    }
}
/// The deepest [`Subcell::level`] a cell is split to.
pub const MAX_SUBCELL_LEVEL: u8 = 8;
impl Options {
    /// Tiles of `tile_size`, GPU-instanced and compressed, images as PNG, no viewer records, no byte budget, no
    /// camera.
    pub fn new(tile_size: TileSize) -> Options {
        Self {
            tile_size,
            gpu_instancing: true,
            compress: true,
            #[cfg(feature = "ktx2")]
            ktx2: false,
            #[cfg(feature = "ktx2")]
            cache: None,
            #[cfg(feature = "ktx2")]
            cache_verify: false,
            #[cfg(feature = "ktx2")]
            encoding_memory: ktx2::DEFAULT_ENCODING_MEMORY,
            viewer_items: Vec::new(),
            split_by_tile: BTreeSet::new(),
            tile_byte_budget: None,
            camera: None,
        }
    }
}
/// A viewer component attached to an empty glTF node. Payload coordinates are in
/// world space; the node is identity, and the anchor chooses its streaming tile.
#[derive(Clone, Debug, Serialize)]
pub struct ViewerItem {
    pub name: String,
    pub kind: String,
    pub anchor: [f64; 3],
    pub data: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TileInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collision: Option<CollisionFile>,
    /// Metadata-only tiles contain viewer records and no mesh payload.
    pub content: TileContent,
    pub col: i32,
    pub row: i32,
    /// The part of the cell this tile covers, when the cell was split to fit the byte budget; absent for a whole cell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<Subcell>,
    pub file: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub triangles: u64,
    pub placements: u64,
    pub instanced_placements: u64,
    pub viewer_items: u64,
    /// Scene lights in this tile (omitted when 0).
    #[serde(skip_serializing_if = "is_zero")]
    pub lights: u64,
    /// `core.layered` materials in this tile's GLB, written with `PRIMASE_layered_material` (omitted when 0).
    #[serde(skip_serializing_if = "is_zero")]
    pub layered_materials: u64,
    pub bytes: u64,
    /// The image files the tile's GLB references (entries of [`Index::images`]), in its image order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}
/// An image file shared by the tiles that use it: `images/<sha256>.<ext>` and its length.
#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImageFile {
    pub file: String,
    pub bytes: u64,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
}
/// A quadtree subcell of a tile's ground cell: the cell halved `level` times (at least once) along both ground axes,
/// and the subcell `x` east and `y` north of the cell's minimum corner, each in `0..2^level`.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subcell {
    pub level: u8,
    pub x: u32,
    pub y: u32,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TileContent {
    Geometry,
    MetadataOnly,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CollisionFile {
    pub file: String,
    pub bytes: u64,
    pub records: usize,
}
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Index {
    /// Tile-set contract version, independent of component extras versions.
    pub format_version: &'static str,
    pub empty_tile_policy: &'static str,
    pub version: u32,
    pub tile_size: f64,
    pub tiles: Vec<TileInfo>,
    /// Every image file the tiles reference, by file name.
    pub images: Vec<ImageFile>,
    pub skipped_degenerate: u64,
    pub placements_without_mesh: u64,
    /// The scene's environment (`core.environment`), indexed scene-wide; absent when the scene names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<IndexEnvironment>,
    /// [`Options::camera`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera>,
}
/// The index's `environment`.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct IndexEnvironment {
    pub sun: IndexSun,
}
/// The scene's sun: a directional light, in the index's world frame.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct IndexSun {
    pub name: String,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Lux.
    pub intensity: f32,
    /// Unit vector, the way the light travels (its transform's local -Z).
    pub direction: [f32; 3],
    /// `"on"` or `"off"`; absent: unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cast: Option<&'static str>,
}
#[derive(Clone, Debug)]
pub struct TileSet {
    pub collision_files: BTreeMap<String, Vec<u8>>,
    pub index: Index,
    pub files: BTreeMap<String, Vec<u8>>,
    /// The image files, `images/<sha256>.<ext>`, each once.
    pub images: BTreeMap<String, Vec<u8>>,
    /// Tiles left over the byte budget (see [`TilePlan::over_budget`]).
    pub over_budget: Vec<OverBudget>,
    /// Cells and subcells split to fit the byte budget (see [`TilePlan::splits`]).
    pub splits: Vec<Split>,
    /// What the tiles left out, chose or drew approximately (see [`TilePlan::notices`]).
    pub notices: Vec<Notice>,
}
/// A cell or subcell split into its four children because a file of its tile was over [`Options::tile_byte_budget`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    /// `<col>_<row>`, with `_<level>-<x>-<y>` for a subcell.
    pub cell: String,
    /// The larger file's length, when its tile was written to measure it (not when the parent's content was already
    /// known to be too large).
    pub bytes: Option<u64>,
}
/// A tile with a file over [`Options::tile_byte_budget`], and why its cell was not split further.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverBudget {
    pub file: String,
    /// Its larger file's length: the GLB or the collision sidecar.
    pub bytes: u64,
    pub reason: &'static str,
}
/// Add `name` to a glTF document's `extensionsUsed` and `extensionsRequired`, keeping the extensions already there.
pub(crate) fn require_extension(doc: &mut serde_json::Value, name: &str) {
    for key in ["extensionsUsed", "extensionsRequired"] {
        if !doc[key].is_array() {
            doc[key] = serde_json::json!([]);
        }
        let list = doc[key].as_array_mut().expect("array");
        if !list.iter().any(|v| v == name) {
            list.push(serde_json::json!(name));
        }
    }
}

fn point(t: Transform, p: [f32; 3]) -> [f64; 3] {
    std::array::from_fn(|k| {
        t.cols[3][k] as f64
            + t.cols[0][k] as f64 * p[0] as f64
            + t.cols[1][k] as f64 * p[1] as f64
            + t.cols[2][k] as f64 * p[2] as f64
    })
}
/// A world-space point's ground coordinates (east, north), through the shared frame basis in f64: rounding
/// transformed triangle centres to f32 here would move faces across exact cell boundaries.
fn ground(p: [f64; 3]) -> [f64; 2] {
    let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].map(primase_core::frame::ground);
    std::array::from_fn(|k| (0..3).map(|i| p[i] * axes[i][k] as f64).sum())
}
/// The child of `parent` (`None`: the whole cell) of cell `(col, row)` that holds ground point `g`, clamped into the
/// parent's own four children, so a point rounded across the parent's edge stays with the parent's content.
fn child(col: i32, row: i32, parent: Option<Subcell>, g: [f64; 2], tile_size: f64) -> Subcell {
    let (level, px, py) = parent.map_or((1, 0, 0), |s| (s.level + 1, s.x, s.y));
    let n = f64::from(1u32 << level);
    let size = tile_size / n;
    let at = |v: f64, origin: i32, p: u32| -> u32 {
        let lo = 2 * p;
        let q = (v / size).floor() - f64::from(origin) * n;
        if q.is_nan() {
            lo
        } else {
            q.clamp(f64::from(lo), f64::from(lo + 1)) as u32
        }
    };
    Subcell {
        level,
        x: at(g[0], col, px),
        y: at(g[1], row, py),
    }
}
fn coordinate(p: [f64; 3], size: f64) -> Result<(i32, i32)> {
    let coord = |v: f64| {
        let q = (v / size).floor();
        if q.is_finite() && q >= i32::MIN as f64 && q <= i32::MAX as f64 {
            Ok(q as i32)
        } else {
            Err(Error::Invalid("tile coordinate outside i32".into()))
        }
    };
    let g = ground(p);
    Ok((coord(g[0])?, coord(g[1])?))
}
fn indices(p: &Primitive) -> Vec<u32> {
    p.indices
        .clone()
        .unwrap_or_else(|| (0..p.vertex_count() as u32).collect())
}
/// How many indices an element of a primitive takes: a point 1, a line segment 2, a triangle 3.
fn arity(t: Topology) -> usize {
    match t {
        Topology::Triangles => 3,
        Topology::Lines => 2,
        Topology::Points => 1,
    }
}
/// The world-space centre of an element (its indices `element` into `pos`) through `t`: the mean of its points.
fn element_centre(t: Transform, pos: &[[f32; 3]], element: &[u32]) -> [f64; 3] {
    let mut c = [0.; 3];
    for &i in element {
        let q = point(t, pos[i as usize]);
        for k in 0..3 {
            c[k] += q[k];
        }
    }
    c.map(|v| v / element.len() as f64)
}
fn validate_mesh(mesh: &Mesh) -> Result<()> {
    for p in mesh.primitives() {
        let positions = p
            .positions()
            .ok_or_else(|| Error::Invalid("missing float3 positions".into()))?;
        if positions.is_empty()
            || positions.len() > u32::MAX as usize
            || p.streams.values().any(|v| v.len() != positions.len())
        {
            return Err(Error::Invalid("empty or inconsistent vertex streams".into()));
        }
        let size = arity(p.topology);
        let count = p.indices.as_ref().map_or(positions.len(), Vec::len);
        if count == 0
            || !count.is_multiple_of(size)
            || p.indices
                .as_ref()
                .is_some_and(|v| v.iter().any(|&i| i as usize >= positions.len()))
        {
            return Err(Error::Invalid("invalid primitive indices".into()));
        }
    }
    Ok(())
}
/// The elements (triangles, line segments, points) of each primitive of a placement a cell or tile holds:
/// (primitive, element indices).
type Pieces = Vec<(usize, Vec<u32>)>;
/// A planned cell: what it owns, and the elements it holds of placements filed by element centre. Ownership only:
/// what the entities are and hold (placements, records, lights, collision; transforms) is read from the cell's world
/// when the cell is written.
#[derive(Clone, Debug, Default)]
struct Cell {
    /// The entities it owns, each filed by its first role when planned: a placement by its pivot, a curve, marker,
    /// light or collision holder by its position; and the placements filed by element centre it is the home of. Each
    /// is written in every role its world gives it, whatever its role was when planned.
    owned: Vec<Entity>,
    /// The placements filed by element centre it holds elements of, each with the elements of each of its primitives
    /// it holds, by index into that primitive's element list. Their home (one of these cells, owning them) writes the
    /// rest of them.
    pieces: Vec<(Entity, Pieces)>,
    /// The given viewer records ([`Options::viewer_items`]) filed here, by number.
    records: Vec<usize>,
}
/// One placement as a tile draws it, read from the cell's world: the whole prototype, or the elements of it the
/// tile holds.
#[derive(Clone, Debug)]
struct Placement {
    entity: Entity,
    prototype: Entity,
    variant: u32,
    transform: Transform,
    elements: Option<Pieces>,
}
/// What a tile holds: its placements, and its records and lights by their number and index in the [`CellWorld`].
#[derive(Clone, Debug, Default)]
struct Content {
    placements: Vec<Placement>,
    items: Vec<usize>,
    lights: Vec<usize>,
}
/// A scene light as written: its name, the light and its world transform, read from the cell's world.
#[derive(Clone, Debug)]
struct SceneLight {
    name: String,
    light: primase_core::Light,
    transform: Transform,
}
/// One written tile: its index entry, its GLB and its collision sidecar, for the caller to keep or store.
#[derive(Clone, Debug)]
pub struct WrittenTile {
    pub info: TileInfo,
    pub bytes: Vec<u8>,
    pub collision: Option<(String, Vec<u8>)>,
    /// The image files it references, with their bytes (shared with other tiles: write each once).
    pub images: ImageFiles,
}
impl WrittenTile {
    /// Write the tile's image files (those not there yet), then its GLB and collision sidecar, into `folder`.
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "writer I/O: writes the tile"
    )]
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        let dir = folder.join(IMAGE_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        for (name, bytes) in &self.images {
            let path = folder.join(name);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
                continue; // content-addressed: the same name holds the same bytes
            }
            std::fs::write(&path, bytes).map_err(|source| Error::Io { path, source })?;
        }
        for (name, bytes) in
            std::iter::once((&self.info.file, &self.bytes)).chain(self.collision.iter().map(|(n, b)| (n, b)))
        {
            let path = folder.join(name);
            std::fs::write(&path, bytes).map_err(|source| Error::Io { path, source })?;
        }
        Ok(())
    }
}
/// Which cell writes what, decided before any tile is written: each entity is owned by one cell, filed by its first
/// role: placements by pivot (or, for [`Options::split_by_tile`], by transformed element centre), then curves,
/// markers, lights and standalone collision by position; given viewer records by anchor. A placement's collision
/// goes with the placement.
///
/// The plan holds ownership, not content or roles: a cell is written from its world as prepared, which is
/// authoritative. What each entity it owns is (a placement, a curve, a marker, a light, a collision holder, or
/// several), its transform and its values are read from it, so what the cell's passes changed is what is written: a
/// placement a pass turned into a marker is written as the marker. An entity is written by the cell that owns it,
/// wherever a pass moved it (its tile's bounds follow it); what the cell's world holds that the planning world did
/// not (made by its load or its passes, and kept by the conversion as the part's output) is that cell's. What the
/// world holds of other cells' (reached through a reference) is theirs to write. A placement filed by element centre
/// is drawn piecewise by every cell holding its elements, and their passes must prepare its mesh alike (the same
/// primitives and element counts); it is owned by one of them, its home (the cell of its pivot when that holds some
/// of its elements, else the first), which writes the rest of it: its collision, and its other roles. The other cells
/// share it ([`TilePlan::shared`], a conversion's [`primase_core::convert::Part::shared`]): content their passes make
/// from it is its home's, which makes it too, so it is written once, the same whatever the cells.
///
/// A driver prepares one cell at a time: the world's [`World::closure`] of the cell's [`TilePlan::roots`] and
/// [`TilePlan::shared`] with what its passes made for what it owns (as a conversion keeps a part's output: not what
/// they made from what it shares), as a [`World::subset`], with its meshes and textures loaded and lowered to PBR, then
/// [`TilePlan::write_cell`]; then [`TilePlan::finish`]. The writer neither loads nor lowers: it fails, as before, on
/// what is missing. Placements filed by element centre need their prototype's geometry to be planned:
/// [`TilePlan::split`] each of [`TilePlan::unsplit`] first, from a world holding that prototype loaded.
pub struct TilePlan {
    options: Options,
    /// The given viewer records, by number.
    given: Vec<ViewerItem>,
    /// The number of each planned record of a world entity (a curve, a marker), by entity and component.
    numbers: BTreeMap<(Entity, &'static str), usize>,
    /// The number the next record made when a cell is written gets (records are numbered across the set).
    next_item: usize,
    /// The planning world's next entity index: an entity at or above it in a cell's world was made after planning.
    first_made: u32,
    /// The sun: indexed scene-wide, not written as a tile's light.
    sun_entity: Option<Entity>,
    /// Each placement filed by element centre: its primitives' triangle counts when it was split.
    shapes: BTreeMap<Entity, Vec<usize>>,
    ground_markers: u64,
    /// Whether the planning world held `core.environment`'s ambient: what is reported when no cell is written (a world
    /// with nothing to tile). The viewer has no ambient light, the index carries only the sun and the tiles leave the
    /// environment out, so an ambient is not written ([`TilePlan::notices`], once).
    ambient_planned: bool,
    /// Whether a written cell's world held it, as that cell's passes left it: once a cell is written, this alone is
    /// reported (a pass may have consumed the planning world's ambient, as `fill-ambient-lights` does, or lowered one
    /// after planning).
    ambient_written: bool,
    /// What the published tiles' documents and the records left out or drew approximately: by kind and reason, the
    /// count and a few examples ([`TilePlan::notices`]).
    omitted: BTreeMap<(NoticeKind, String), (u64, Vec<String>)>,
    sun: Option<IndexSun>,
    cells: BTreeMap<(i32, i32), Cell>,
    unsplit: Vec<Entity>,
    written: BTreeSet<(i32, i32)>,
    tiles: Vec<TileInfo>,
    images: BTreeMap<String, u64>,
    over_budget: Vec<OverBudget>,
    splits: Vec<Split>,
    skipped: u64,
    without_mesh: u64,
    #[cfg(feature = "ktx2")]
    textures: ktx2::TextureEncoder,
}
/// The world a cell is written from, with what the cell writes made from it: its records (by number), their
/// entities' other shown components, and its scene lights.
struct CellWorld<'w> {
    world: &'w World,
    records: BTreeMap<usize, ViewerItem>,
    /// For a record of a world entity (a curve or a marker): the entity's other shown components, by name, as glTF
    /// extras show them, for the record's node.
    components: BTreeMap<usize, serde_json::Map<String, serde_json::Value>>,
    lights: Vec<SceneLight>,
}
impl CellWorld<'_> {
    /// Hold record `item` (with its entity's other shown components) as number `n`, in `content`.
    fn put(
        &mut self,
        content: &mut Content,
        n: usize,
        item: ViewerItem,
        components: serde_json::Map<String, serde_json::Value>,
    ) {
        self.records.insert(n, item);
        if !components.is_empty() {
            self.components.insert(n, components);
        }
        content.items.push(n);
    }
}
impl std::fmt::Debug for TilePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TilePlan")
            .field("cells", &self.cells.len())
            .field("unsplit", &self.unsplit.len())
            .field("written", &self.written.len())
            .field("tiles", &self.tiles.len())
            .finish()
    }
}
/// What the writer accepts in the world it writes a cell from ([`TilePlan::write_cell`], after that cell's passes):
/// the core and PBR components it interprets are legal; a `core.mesh_file` or `core.texture_file` only on an entity
/// with the mesh or texture loaded; a `core.curve` only as a polyline (`flatten-curves` first); `core.lods` and
/// `core.collider` must be lowered first (`select-lod`, `tessellate-colliders`). Every other name is carried
/// (glTF extras). Transforms must be rigid or degenerate (skipped): `bake-nonrigid` bakes a mirror or shear
/// into geometry. As in glTF, a material entity without `pbr.material` is written as no material, and a primitive
/// lacking a texture coordinate set its material samples is refused (`prepare-gltf-materials`,
/// `fill-missing-texcoords` are a program's choices).
pub fn target() -> primase_core::pass::Target {
    use primase_core::{
        Collision, Component, Curve, LayeredMaterial, Light, Lods, Marker, MeshFile, Shadows, Texture,
        TextureFile, pbr::PbrMaterial,
    };
    primase_core::pass::Target::new("web")
        .legal(Instance::NAME)
        .dynamic_typed::<Transform>(|t| {
            matches!(t.classify(), TransformKind::Regular | TransformKind::Degenerate)
        })
        .legal(Name::NAME)
        .legal(Mesh::NAME)
        .legal(MaterialVariants::NAME)
        .legal(Shadows::NAME)
        .legal(Light::NAME)
        .legal(Collision::NAME)
        .legal(Marker::NAME)
        .legal(Curve::NAME)
        .legal(PbrMaterial::NAME)
        .legal(LayeredMaterial::NAME)
        .legal(Texture::NAME)
        .dynamic(MeshFile::NAME, |w, e| w.has::<Mesh>(e))
        .dynamic(TextureFile::NAME, |w, e| w.has::<Texture>(e))
        // viewer records draw polylines; collision is drawn from its triangles
        .dynamic_typed::<Curve>(|c| c.tangents.is_none())
        .illegal(Lods::NAME)
        .illegal(primase_core::Collider::NAME)
        .carry_unknown()
}
/// Plan `world`'s tiles. Geometry may still be in files: each cell's content is read from its world when it is
/// written.
pub fn plan(world: &World, options: &Options) -> Result<TilePlan> {
    plan_with_geometry(world, options, &|_| false)
}

/// No mesh in the world or the cell world: neither a loaded one nor a file to load.
fn meshless(world: &World, prototype: Entity) -> bool {
    !world.has::<Mesh>(prototype) && !world.has::<primase_core::MeshFile>(prototype)
}

/// Whether `e` has collision to write: a soup with triangles.
fn collides(world: &World, e: Entity) -> bool {
    world
        .get::<primase_core::Collision>(e)
        .is_some_and(|c| !c.triangles.is_empty())
}

/// `world`'s placements, each with its prototype: its instances, and its meshes placed as themselves (their own
/// prototype).
fn placements(world: &World) -> BTreeMap<Entity, Entity> {
    let mut out: BTreeMap<Entity, Entity> = world.iter::<Instance>().map(|(e, i)| (e, i.prototype)).collect();
    for (e, _) in world.iter::<Mesh>() {
        if !world.has::<Instance>(e) && world.has::<Transform>(e) {
            out.insert(e, e);
        }
    }
    out
}

/// Whether `e` holds collision of its own, placed by its own transform: a soup with triangles that is not a
/// placement and that none of `placements` places.
fn standalone_collision(world: &World, e: Entity, placements: &BTreeMap<Entity, Entity>) -> bool {
    collides(world, e) && !placements.contains_key(&e) && !placements.values().any(|&p| p == e)
}

/// A marker's anchor: its transform's position, or a ground position at height 0 (`true`: the source leaves the
/// height to its consumer).
fn marker_anchor(world: &World, e: Entity, m: &primase_core::Marker) -> Result<([f64; 3], bool)> {
    match (world.get::<Transform>(e), m.ground) {
        (Some(t), _) => Ok((t.cols[3].map(f64::from), false)),
        // east, north: x = east, z = -north
        (None, Some([east, north])) => Ok(([f64::from(east), 0., -f64::from(north)], true)),
        (None, None) => Err(Error::Invalid(format!("{e:?}: a marker without a position"))),
    }
}

/// A curve's anchor, its first point; a Bezier curve is refused (viewer records are polylines).
fn curve_anchor(e: Entity, c: &primase_core::Curve) -> Result<[f64; 3]> {
    if c.tangents.is_some() {
        return Err(Error::Invalid(format!(
            "{e:?}: a Bezier curve: viewer records are polylines (flatten-curves first)"
        )));
    }
    c.points
        .first()
        .map(|p| p.map(f64::from))
        .ok_or_else(|| Error::Invalid(format!("{e:?}: a curve without points")))
}

/// [`plan`] for a world whose geometry its cells will load (a reader that loads assets per cell): `geometry` says
/// which prototypes have geometry before it is loaded (they have a box, see [`spanning`]): a placement of one of
/// [`Options::split_by_tile`] is filed by element centre only when its prototype has a mesh, a mesh file or
/// `geometry`. Every placement is owned; one whose cell's world then holds no mesh for it counts as a placement
/// without mesh, one with a degenerate transform as skipped ([`TilePlan::write_cell`]).
pub fn plan_with_geometry(
    world: &World,
    options: &Options,
    geometry: &dyn Fn(Entity) -> bool,
) -> Result<TilePlan> {
    use primase_core::{Collision, Component, Curve, Light, Marker};
    if let Some(c) = options.camera
        && (!c.eye.iter().chain(&c.target).all(|v| v.is_finite()) || c.eye == c.target)
    {
        return Err(Error::Invalid(format!(
            "camera {c:?}: eye and target must be finite and apart"
        )));
    }
    if let Some((name, e, detail)) = world.verify_components().into_iter().next() {
        return Err(Error::Invalid(format!("{name} on {e:?}: {detail}")));
    }
    if let Some((name, detail)) = world.verify_resources().into_iter().next() {
        return Err(Error::Invalid(format!("{name}: {detail}")));
    }
    for (e, _) in world.iter::<Instance>() {
        if !world.has::<Transform>(e) {
            return Err(Error::Invalid(format!("placement {e:?} has no transform")));
        }
    }
    let placements = placements(world);
    for e in &options.split_by_tile {
        if !placements.contains_key(e) {
            return Err(Error::Invalid(format!("split marker {e:?} is not a placement")));
        }
    }
    let sun_entity = world
        .resource::<primase_core::Environment>()
        .and_then(|env| env.sun);
    let mut plan = TilePlan {
        options: Options {
            viewer_items: Vec::new(),
            ..options.clone()
        },
        given: options.viewer_items.clone(),
        numbers: BTreeMap::new(),
        next_item: options.viewer_items.len(),
        first_made: world.next_index(),
        sun_entity,
        shapes: BTreeMap::new(),
        ground_markers: 0,
        ambient_planned: world
            .resource::<primase_core::Environment>()
            .is_some_and(|env| env.ambient.is_some()),
        ambient_written: false,
        omitted: BTreeMap::new(),
        sun: sun_entity.map(|e| sun(world, e)).transpose()?,
        cells: BTreeMap::new(),
        unsplit: Vec::new(),
        written: BTreeSet::new(),
        images: BTreeMap::new(),
        tiles: Vec::new(),
        over_budget: Vec::new(),
        splits: Vec::new(),
        skipped: 0,
        without_mesh: 0,
        #[cfg(feature = "ktx2")]
        textures: ktx2::TextureEncoder::new(options.cache.clone(), options.cache_verify)
            .with_memory(options.encoding_memory),
    };
    // every placement owned by one cell, as a placement (its first role): what it draws or collides with, and whether
    // its transform is degenerate, is what its cell's world holds when the cell is written
    let mut filed = BTreeSet::new();
    for (&e, &prototype) in &placements {
        let t = *world.get::<Transform>(e).expect("a placement has a transform");
        let kind = t.classify();
        if matches!(kind, TransformKind::Mirrored | TransformKind::Sheared) {
            return Err(Error::Invalid(format!(
                "placement {e:?}: a non-rigid transform; the writer takes rigid placements, run bake-nonrigid first"
            )));
        }
        filed.insert(e);
        let draws = !meshless(world, prototype) || geometry(prototype);
        if draws && kind == TransformKind::Regular && options.split_by_tile.contains(&e) {
            plan.unsplit.push(e);
        } else {
            plan.own_whole(e, t)?;
        }
    }
    for (i, item) in plan.given.iter().enumerate() {
        if item.kind.is_empty() || item.anchor.iter().any(|v| !v.is_finite()) {
            return Err(Error::Invalid(
                "viewer item needs a finite anchor and nonempty kind".into(),
            ));
        }
        let key = coordinate(item.anchor, options.tile_size.get())?;
        plan.cells.entry(key).or_default().records.push(i);
    }
    // the other entities, each filed by its first role: curves (at their first point), markers, lights (the sun
    // aside: it is indexed scene-wide), then collision no placement places; their records numbered in that order,
    // after the given ones
    for (e, c) in world.iter::<Curve>() {
        let first = curve_anchor(e, c)?;
        plan.numbers.insert((e, Curve::NAME), plan.next_item);
        plan.next_item += 1;
        plan.file(&mut filed, e, first)?;
    }
    for (e, m) in world.iter::<Marker>() {
        let (anchor, _) = marker_anchor(world, e, m)?;
        plan.numbers.insert((e, Marker::NAME), plan.next_item);
        plan.next_item += 1;
        plan.file(&mut filed, e, anchor)?;
    }
    for (e, _) in world.iter::<Light>() {
        if Some(e) != sun_entity {
            let t = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
            plan.file(&mut filed, e, t.cols[3].map(f64::from))?;
        }
    }
    for (e, c) in world.iter::<Collision>() {
        if standalone_collision(world, e, &placements) {
            let t = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
            if matches!(t.classify(), TransformKind::Mirrored | TransformKind::Sheared) {
                return Err(Error::Invalid(format!(
                    "collision of {e:?}: a non-rigid transform; run bake-nonrigid first"
                )));
            }
            let anchor = collision::anchor(c, t)
                .ok_or_else(|| Error::Invalid("collision whose first triangle names no vertex".into()))?;
            plan.file(&mut filed, e, anchor)?;
        }
    }
    Ok(plan)
}
/// A viewer record of a world entity: the record, the entity's other shown components (by name, for the record's
/// node), and the names of its components without a JSON form (not shown).
struct Record {
    item: ViewerItem,
    components: serde_json::Map<String, serde_json::Value>,
    unencoded: Vec<String>,
}
/// The viewer record of `e`'s core component `core` (kind `kind`, anchored at `anchor`): named by its `core.name`,
/// with the component as its data; and the entity's other shown components.
fn core_record(world: &World, e: Entity, kind: &str, anchor: [f64; 3], core: &str) -> Result<Record> {
    use primase_core::Component;
    let json_error = |detail: String| Error::Invalid(format!("{e:?}: {detail}"));
    let mut data = None;
    let mut others = serde_json::Map::new();
    let mut unencoded = Vec::new();
    for c in world
        .components_json(e)
        .map_err(|err| json_error(err.to_string()))?
    {
        match (c.name, c.value) {
            (name, Some(v)) if name == core => data = Some(v),
            (Name::NAME | Transform::NAME, _) => {}
            (name, None) => unencoded.push(name.to_owned()),
            (name, Some(v)) => {
                others.insert(
                    name.to_owned(),
                    serde_json::json!({"version": c.version, "value": v}),
                );
            }
        }
    }
    let item = ViewerItem {
        name: world
            .get::<Name>(e)
            .map_or_else(|| format!("Entity_{}", e.index()), |n| n.0.clone()),
        kind: kind.to_owned(),
        anchor,
        data: data.ok_or_else(|| json_error(format!("{core} has no JSON view")))?,
    };
    Ok(Record {
        item,
        components: others,
        unencoded,
    })
}
/// The index's record of the sun `e` (a directional `core.light`, checked by the resource's verify).
fn sun(world: &World, e: Entity) -> Result<IndexSun> {
    let light = world
        .get::<primase_core::Light>(e)
        .ok_or_else(|| Error::Invalid(format!("sun {e:?} has no core.light")))?;
    let t = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
    let z = t.cols[2].map(f64::from);
    let len = (z[0] * z[0] + z[1] * z[1] + z[2] * z[2]).sqrt();
    if !(len.is_finite() && len > 0.0) {
        return Err(Error::Invalid(format!(
            "sun {e:?}: its transform has no direction"
        )));
    }
    Ok(IndexSun {
        name: world
            .get::<Name>(e)
            .map_or_else(|| format!("light_{}", e.index()), |n| n.0.clone()),
        color: light.color,
        intensity: light.intensity,
        direction: z.map(|v| (-v / len) as f32),
        cast: light.cast_shadows.map(|c| if c { "on" } else { "off" }),
    })
}
impl TilePlan {
    /// Viewer records written so far of markers with a ground position (no height: the source leaves it to its
    /// consumer), anchored at height 0.
    pub fn ground_markers(&self) -> u64 {
        self.ground_markers
    }
    /// Placements filed by element centre that still wait for [`TilePlan::split`].
    pub fn unsplit(&self) -> Vec<Entity> {
        self.unsplit.clone()
    }
    /// File `e` in the cell of `at` (ground position), once: an entity that is not a placement, by its first role.
    fn file(&mut self, filed: &mut BTreeSet<Entity>, e: Entity, at: [f64; 3]) -> Result<()> {
        if at.iter().any(|v| !v.is_finite()) {
            return Err(Error::Invalid(format!(
                "{e:?}: a viewer record or light without a finite position"
            )));
        }
        if filed.insert(e) {
            let key = coordinate(at, self.options.tile_size.get())?;
            self.cells.entry(key).or_default().owned.push(e);
        }
        Ok(())
    }
    /// Own placement `e` whole, in the cell of its pivot.
    fn own_whole(&mut self, e: Entity, t: Transform) -> Result<()> {
        let key = coordinate(t.cols[3].map(f64::from), self.options.tile_size.get())?;
        self.cells.entry(key).or_default().owned.push(e);
        Ok(())
    }
    /// File the elements (triangles, line segments, points) of placement `e` (one of [`TilePlan::unsplit`]) by
    /// their centres, through its transform in
    /// `world`, which holds its prototype's mesh loaded and prepared as each cell holding them will; it is owned by its
    /// home, the cell of its pivot when that holds some of its elements, else the first that does. A placement that
    /// turned out to have no mesh (none loaded and no file) or a degenerate transform is owned whole, by its pivot.
    pub fn split(&mut self, world: &World, e: Entity) -> Result<()> {
        let i = self
            .unsplit
            .iter()
            .position(|&p| p == e)
            .ok_or_else(|| Error::Invalid(format!("{e:?} is not a placement waiting to be split")))?;
        self.unsplit.remove(i);
        let prototype = world.get::<Instance>(e).map_or(e, |i| i.prototype);
        let transform = *world
            .get::<Transform>(e)
            .ok_or_else(|| Error::Invalid(format!("placement {e:?} has no transform")))?;
        match transform.classify() {
            TransformKind::Degenerate => return self.own_whole(e, transform),
            TransformKind::Mirrored | TransformKind::Sheared => {
                return Err(Error::Invalid(format!(
                    "placement {e:?}: a non-rigid transform; the writer takes rigid placements, run bake-nonrigid first"
                )));
            }
            TransformKind::Regular => {}
        }
        if meshless(world, prototype) {
            return self.own_whole(e, transform);
        }
        let mesh = loaded(world, prototype)?;
        validate_mesh(mesh)?;
        let mut parts = BTreeMap::<(i32, i32), Vec<(usize, Vec<u32>)>>::new();
        let mut shape = Vec::new();
        for (part, prim) in mesh.primitives().iter().enumerate() {
            let pos = prim
                .positions()
                .ok_or_else(|| Error::Invalid("missing positions".into()))?;
            let idx = indices(prim);
            let size = arity(prim.topology);
            shape.push(idx.len() / size);
            let mut pieces = BTreeMap::<(i32, i32), Vec<u32>>::new();
            for (t, element) in idx.chunks_exact(size).enumerate() {
                let centre = element_centre(transform, pos, element);
                pieces
                    .entry(coordinate(centre, self.options.tile_size.get())?)
                    .or_default()
                    .push(t as u32);
            }
            for (key, elements) in pieces {
                parts.entry(key).or_default().push((part, elements));
            }
        }
        let pivot = coordinate(transform.cols[3].map(f64::from), self.options.tile_size.get())?;
        let Some(&first) = parts.keys().next() else {
            // no elements to file: owned whole
            return self.own_whole(e, transform);
        };
        self.shapes.insert(e, shape);
        let home = if parts.contains_key(&pivot) { pivot } else { first };
        for (key, elements) in parts {
            let cell = self.cells.entry(key).or_default();
            if key == home {
                cell.owned.push(e);
            }
            cell.pieces.push((e, elements));
        }
        Ok(())
    }
    /// KTX2 encodings dropped from memory to stay within [`Options::encoding_memory`], so far (each made again or
    /// read from the disk cache when a later tile used it).
    #[cfg(feature = "ktx2")]
    pub fn encodings_dropped(&self) -> u64 {
        self.textures.dropped_from_memory()
    }
    /// The planned cells, in order.
    pub fn cells(&self) -> Vec<(i32, i32)> {
        self.cells.keys().copied().collect()
    }
    /// The entities a cell owns: its placements (those filed by element centre it is the home of included), lights,
    /// curves, markers and collision holders. A driver prepares the world's [`World::closure`] of them and of
    /// [`TilePlan::shared`] (their prototypes with them).
    pub fn roots(&self, cell: (i32, i32)) -> BTreeSet<Entity> {
        self.cells
            .get(&cell)
            .map(|c| c.owned.iter().copied().collect())
            .unwrap_or_default()
    }
    /// The placements filed by element centre a cell holds elements of and does not own (another cell is their home):
    /// it draws those elements, and what its passes make from them is their home's.
    pub fn shared(&self, cell: (i32, i32)) -> BTreeSet<Entity> {
        self.cells
            .get(&cell)
            .map(|c| {
                c.pieces
                    .iter()
                    .map(|(e, _)| *e)
                    .filter(|e| !c.owned.contains(e))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Write cell `cell` from `world`, as prepared: the closure of its [`TilePlan::roots`] and [`TilePlan::shared`] and
    /// what the cell's passes made for what it owns (meshes and textures loaded, materials lowered). The world is authoritative (see [`TilePlan`]): the
    /// entities the cell owns are written as the world holds them, with what the world holds that the planning world
    /// did not; what it holds of other cells' (reached through a reference) is theirs to write. The writer only reads
    /// it: each tile is a selection of the world's placements (and of the triangles of those filed by triangle
    /// centre), so the same world can be written by another plan (a plain set). Returns its tiles (none for a cell
    /// with nothing to draw or show).
    pub fn write_cell(&mut self, cell: (i32, i32), world: &World) -> Result<Vec<WrittenTile>> {
        use primase_core::{Collision, Component, Curve, Light, Marker};
        if !self.unsplit.is_empty() {
            return Err(Error::Invalid(
                "split every placement filed by element centre before writing".into(),
            ));
        }
        if !self.written.insert(cell) {
            return Err(Error::Invalid(format!("cell {cell:?} written twice")));
        }
        let Some(owned) = self.cells.get(&cell).cloned() else {
            return Err(Error::Invalid(format!("cell {cell:?} is not planned")));
        };
        if let Some((name, e, detail)) = world.verify_components().into_iter().next() {
            return Err(Error::Invalid(format!("{name} on {e:?}: {detail}")));
        }
        // the cell's world as its passes left it: the ambient may be lowered there after planning, or consumed
        if world
            .resource::<primase_core::Environment>()
            .is_some_and(|env| env.ambient.is_some())
        {
            self.ambient_written = true;
        }
        let mut cw = CellWorld {
            world,
            records: BTreeMap::new(),
            components: BTreeMap::new(),
            lights: Vec::new(),
        };
        let mut content = Content::default();
        for &i in &owned.records {
            cw.records.insert(i, self.given[i].clone());
            content.items.push(i);
        }
        // what the cell writes, each in every role its world gives it now: what it owns; the placements filed by
        // element centre it holds elements of, as those elements alone (their home, which owns them, writes the rest);
        // and what the world holds that the planning world did not (made by the cell's load or passes: the cell's)
        let first_made = self.first_made;
        let placements = placements(world);
        let homes: BTreeSet<Entity> = owned.owned.iter().copied().collect();
        let pieces: BTreeMap<Entity, Pieces> = owned.pieces.iter().cloned().collect();
        let held = owned
            .owned
            .iter()
            .map(|&e| (e, true))
            .chain(
                owned
                    .pieces
                    .iter()
                    .filter(|(e, _)| !homes.contains(e))
                    .map(|&(e, _)| (e, false)),
            )
            .chain(
                world
                    .entities()
                    .filter(|e| e.index() >= first_made)
                    .map(|e| (e, true)),
            )
            .filter(|&(e, _)| world.is_alive(e))
            .collect::<Vec<_>>();
        for (e, owner) in held {
            if owner {
                if let Some(c) = world.get::<Curve>(e) {
                    let r = core_record(world, e, "grind_curve", curve_anchor(e, c)?, Curve::NAME)?;
                    let n = self.number(e, Curve::NAME);
                    self.unencoded(&r);
                    cw.put(&mut content, n, r.item, r.components);
                }
                if let Some(m) = world.get::<Marker>(e) {
                    let (anchor, ground) = marker_anchor(world, e, m)?;
                    self.ground_markers += u64::from(ground);
                    let kind = if m.kind == "spawn" { "spawn" } else { "marker" };
                    let r = core_record(world, e, kind, anchor, Marker::NAME)?;
                    let n = self.number(e, Marker::NAME);
                    self.unencoded(&r);
                    cw.put(&mut content, n, r.item, r.components);
                }
                if let Some(light) = world.get::<Light>(e)
                    && Some(e) != self.sun_entity
                {
                    let transform = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
                    if transform.cols[3].iter().any(|v| !v.is_finite()) {
                        return Err(Error::Invalid(format!("light {e:?} without a finite position")));
                    }
                    content.lights.push(cw.lights.len());
                    cw.lights.push(SceneLight {
                        name: world
                            .get::<Name>(e)
                            .map_or_else(|| format!("light_{}", e.index()), |n| n.0.clone()),
                        light: light.clone(),
                        transform,
                    });
                }
                if standalone_collision(world, e, &placements) {
                    let c = world.get::<Collision>(e).expect("standalone collision");
                    let t = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
                    if let Some(item) = collision::record(world, e, c, t)? {
                        let n = self.number(e, Collision::NAME);
                        cw.put(&mut content, n, item, serde_json::Map::new());
                    }
                }
            }
            // as a placement (an instance, or a mesh with a transform): drawn, whole or the elements the cell holds,
            // and its owner writes its prototype's collision through its transform
            let Some(&prototype) = placements.get(&e) else {
                continue;
            };
            let elements = pieces.get(&e).cloned();
            let variant = world.get::<Instance>(e).map_or(0, |i| i.variant);
            let transform = *world.get::<Transform>(e).expect("a placement has a transform");
            match transform.classify() {
                TransformKind::Degenerate => {
                    self.skipped += u64::from(owner);
                    continue;
                }
                TransformKind::Mirrored | TransformKind::Sheared => {
                    return Err(Error::Invalid(format!(
                        "placement {e:?}: a non-rigid transform; the writer takes rigid placements, run bake-nonrigid first"
                    )));
                }
                TransformKind::Regular => {}
            }
            if owner
                && let Some(c) = world.get::<Collision>(prototype)
                && let Some(item) = collision::record(world, e, c, transform)?
            {
                let n = self.number(e, Collision::NAME);
                cw.put(&mut content, n, item, serde_json::Map::new());
            }
            if meshless(world, prototype) {
                // a prototype whose cell has no mesh (none loaded, or planned from a world that loads per cell)
                self.without_mesh += u64::from(owner);
                continue;
            }
            if elements.is_some() {
                let mesh = loaded(world, prototype)?;
                let shape: Vec<usize> = mesh
                    .primitives()
                    .iter()
                    .map(|p| indices(p).len() / arity(p.topology))
                    .collect();
                if self.shapes.get(&e) != Some(&shape) {
                    return Err(Error::Invalid(format!(
                        "placement {e:?}: its prepared mesh has element counts {shape:?} per primitive, its elements \
                         were filed from {:?}; a placement filed by element centre must be prepared alike in every \
                         cell holding it",
                        self.shapes.get(&e)
                    )));
                }
            }
            content.placements.push(Placement {
                entity: e,
                prototype,
                variant,
                transform,
                elements,
            });
        }
        let mut out = Vec::new();
        self.emit(cell, None, &cw, content, false, &mut out)?;
        Ok(out)
    }
    /// Count the components of record `r`'s entity that have no JSON form (not in the record's node).
    fn unencoded(&mut self, r: &Record) {
        for name in &r.unencoded {
            self.omit(
                NoticeKind::Unsupported,
                format!("{name} components without a JSON form, not in a viewer record's extras"),
                1,
                &r.item.name,
            );
        }
    }
    /// The number of `e`'s record of `core`: planned, or the next.
    fn number(&mut self, e: Entity, core: &'static str) -> usize {
        self.numbers.get(&(e, core)).copied().unwrap_or_else(|| {
            self.next_item += 1;
            self.next_item - 1
        })
    }
    /// Count `count` of `reason` (of `kind`), with `example` (a tile file or a record's name) among its examples.
    fn omit(&mut self, kind: NoticeKind, reason: String, count: u64, example: &str) {
        if count == 0 {
            return;
        }
        let (n, examples) = self.omitted.entry((kind, reason)).or_default();
        *n += count;
        if examples.len() < primase_core::convert::NOTICE_EXAMPLES && !examples.iter().any(|e| e == example) {
            examples.push(example.to_owned());
        }
    }
    /// What the tiles published so far left out, chose or drew approximately, as notices (stage `web`): what their
    /// glTF documents did not write (counted once per published tile, as each document counts it; a tile written to
    /// measure it and then split counts nothing), what the tile encoding approximated (instanced placements' f32
    /// translation, rotation and scale; 16-bit vertex streams, counted per tile; each KTX2 image once), viewer records'
    /// components without a JSON form, placements skipped
    /// for a degenerate transform, markers anchored at height 0, and the environment's ambient light (once, when a
    /// written cell's world held one, or the planning world when no cell is written: the viewer has no ambient, the
    /// index carries only the sun).
    pub fn notices(&self) -> Vec<Notice> {
        let mut out: Vec<Notice> = self
            .omitted
            .iter()
            .map(|((kind, reason), (count, examples))| {
                examples
                    .iter()
                    .fold(Notice::new("web", *kind, reason.clone(), *count), |n, e| {
                        n.example(e.clone())
                    })
            })
            .collect();
        if self.skipped > 0 {
            out.push(Notice::new(
                "web",
                NoticeKind::Unsupported,
                "placements with a degenerate transform, not written",
                self.skipped,
            ));
        }
        if self.ground_markers > 0 {
            out.push(Notice::new(
                "web",
                NoticeKind::Approximation,
                "markers with a ground position only, anchored at height 0",
                self.ground_markers,
            ));
        }
        // the written cells' worlds decide; the planning world's only when nothing was written
        if self.ambient_written || (self.written.is_empty() && self.ambient_planned) {
            out.push(Notice::new(
                "web",
                NoticeKind::Unsupported,
                "the environment's ambient light, not written: the viewer has no ambient light (the index carries \
                 only the sun; the tiles leave the environment out)",
                1,
            ));
        }
        out
    }
    /// Tiles left over the byte budget so far: a cell is split until each file fits, except where what remains is
    /// one placement or viewer record, which no cell boundary divides, or at [`MAX_SUBCELL_LEVEL`]. Such tiles are
    /// written as they are (they still load; only their caching suffers) and listed here.
    pub fn over_budget(&self) -> &[OverBudget] {
        &self.over_budget
    }
    /// Cells and subcells split to fit the byte budget so far, in the order they were split.
    pub fn splits(&self) -> &[Split] {
        &self.splits
    }
    /// Write `content` of `cell`'s subcell `sub` (`None`: the whole cell), splitting it while a file is over the
    /// budget. `over`: the content is already known to be too large (its parent's content, all in this subcell).
    fn emit(
        &mut self,
        cell: (i32, i32),
        sub: Option<Subcell>,
        cw: &CellWorld,
        content: Content,
        over: bool,
        out: &mut Vec<WrittenTile>,
    ) -> Result<()> {
        let level = sub.map_or(0, |s| s.level);
        let mut measured = None;
        if !over {
            let Some((tile, omitted)) = self.tile(cell, sub, cw, &content)? else {
                return Ok(());
            };
            let largest = tile
                .bytes
                .len()
                .max(tile.collision.as_ref().map_or(0, |(_, b)| b.len())) as u64;
            if self.options.tile_byte_budget.is_none_or(|b| largest <= b) {
                self.keep(tile, omitted, out);
                return Ok(());
            }
            measured = Some(largest);
            let units = content.items.len()
                + content.lights.len()
                + content
                    .placements
                    .iter()
                    .map(|p| {
                        p.elements
                            .as_ref()
                            .map_or(1, |t| t.iter().map(|(_, t)| t.len()).sum())
                    })
                    .sum::<usize>();
            if units <= 1 || level >= MAX_SUBCELL_LEVEL {
                self.over_budget.push(OverBudget {
                    file: tile.info.file.clone(),
                    bytes: largest,
                    reason: if units <= 1 {
                        "one placement or viewer record, which no cell boundary divides"
                    } else {
                        "the subdivision limit"
                    },
                });
                self.keep(tile, omitted, out);
                return Ok(());
            }
        }
        self.splits.push(Split {
            cell: match sub {
                None => format!("{}_{}", cell.0, cell.1),
                Some(s) => format!("{}_{}_{}-{}-{}", cell.0, cell.1, s.level, s.x, s.y),
            },
            bytes: measured,
        });
        let children = self.children(cell, sub, cw, content)?;
        if children.len() == 1 && level + 1 < MAX_SUBCELL_LEVEL {
            // all of it in one child: the same content, as large; split further without writing it
            let (k, c) = children.into_iter().next().expect("one child");
            return self.emit(cell, Some(k), cw, c, true, out);
        }
        for (k, c) in children {
            self.emit(cell, Some(k), cw, c, false, out)?;
        }
        Ok(())
    }
    /// Publish `tile`: its images and index entry kept, and what its document left out (`omitted`) counted; a tile
    /// written only to measure it, then split, counts nothing.
    fn keep(&mut self, tile: WrittenTile, omitted: Vec<Omitted>, out: &mut Vec<WrittenTile>) {
        for (kind, reason, count) in omitted {
            self.omit(kind, reason, count, &tile.info.file);
        }
        for (name, bytes) in &tile.images {
            if self.images.insert(name.clone(), bytes.len() as u64).is_none() && name.ends_with(".ktx2") {
                self.omit(
                    NoticeKind::Approximation,
                    "images encoded as KTX2 UASTC LDR 4x4 with Zstandard: lossy, each image's codec in its extras \
                     (primase.web_texture)"
                        .into(),
                    1,
                    name,
                );
            }
        }
        self.tiles.push(tile.info.clone());
        out.push(tile);
    }
    /// `content` of `cell`'s subcell `sub`, divided among its four children by the cell rules: placements by pivot,
    /// placements filed by element centre by their elements' centres (their prototype loaded in `world`), viewer
    /// records by anchor, scene lights by position. Each child keeps the parent's order.
    fn children(
        &self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        cw: &CellWorld,
        content: Content,
    ) -> Result<BTreeMap<Subcell, Content>> {
        let size = self.options.tile_size.get();
        let mut out = BTreeMap::<Subcell, Content>::new();
        for p in content.placements {
            let Some(parts) = &p.elements else {
                let k = child(col, row, sub, ground(p.transform.cols[3].map(f64::from)), size);
                out.entry(k).or_default().placements.push(p);
                continue;
            };
            let mesh = loaded(cw.world, p.prototype)?;
            let mut by = BTreeMap::<Subcell, Vec<(usize, Vec<u32>)>>::new();
            for (part, elements) in parts {
                let prim = &mesh.primitives()[*part];
                let pos = prim
                    .positions()
                    .ok_or_else(|| Error::Invalid("missing positions".into()))?;
                let idx = indices(prim);
                let n = arity(prim.topology);
                let mut pieces = BTreeMap::<Subcell, Vec<u32>>::new();
                for &t in elements {
                    let centre = element_centre(p.transform, pos, &idx[n * t as usize..][..n]);
                    pieces
                        .entry(child(col, row, sub, ground(centre), size))
                        .or_default()
                        .push(t);
                }
                for (k, v) in pieces {
                    by.entry(k).or_default().push((*part, v));
                }
            }
            for (k, elements) in by {
                out.entry(k).or_default().placements.push(Placement {
                    elements: Some(elements),
                    ..p.clone()
                });
            }
        }
        for i in content.items {
            let anchor = cw
                .records
                .get(&i)
                .ok_or_else(|| Error::Invalid(format!("record {i} not made")))?
                .anchor;
            let k = child(col, row, sub, ground(anchor), size);
            out.entry(k).or_default().items.push(i);
        }
        for i in content.lights {
            let k = child(
                col,
                row,
                sub,
                ground(cw.lights[i].transform.cols[3].map(f64::from)),
                size,
            );
            out.entry(k).or_default().lights.push(i);
        }
        Ok(out)
    }
    /// One tile of `content` from `w`, with what its glTF document left out, chose or drew approximately: the tile's placements written as a [`primase_gltf::Selection`] of the world
    /// (a placement filed by element centre as a piece of its own: those of its elements the tile holds), which
    /// leaves the world unchanged.
    fn tile(
        &mut self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        cw: &CellWorld,
        content: &Content,
    ) -> Result<Option<(WrittenTile, Vec<Omitted>)>> {
        let w = cw.world;
        let mut selection = primase_gltf::Selection {
            // the environment is the index's, scene-wide: a tile does not carry it
            omit_resources: [<primase_core::Environment as primase_core::Resource>::NAME.to_owned()].into(),
            ..Default::default()
        };
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut triangles = 0;
        let mut primitives = 0;
        for p in &content.placements {
            let mesh = loaded(w, p.prototype)?;
            validate_mesh(mesh)?;
            let piece = match &p.elements {
                None => None,
                Some(parts) => {
                    let variants = w.get::<MaterialVariants>(p.prototype);
                    if variants.is_some_and(|v| v.0.len() != mesh.primitives().len()) {
                        return Err(Error::Invalid(
                            "material variant count differs from primitive count".into(),
                        ));
                    }
                    let mut chunk = Vec::new();
                    let mut from = Vec::with_capacity(parts.len());
                    for (part, elements) in parts {
                        let prim = &mesh.primitives()[*part];
                        let all = indices(prim);
                        let size = arity(prim.topology);
                        let idx: Vec<u32> = elements
                            .iter()
                            .flat_map(|&t| all[size * t as usize..][..size].iter().copied())
                            .collect();
                        let mut sub = primase_meshops::subset_primitive(prim, &idx)
                            .map_err(|e| Error::Invalid(e.to_string()))?;
                        sub.material = variants
                            .and_then(|v| v.0[*part].get(&p.variant).copied())
                            .or(prim.material);
                        chunk.push(sub);
                        from.push(*part);
                    }
                    Some((Mesh::new(chunk), from))
                }
            };
            let drawn = piece.as_ref().map_or(mesh, |(chunk, _)| chunk);
            for prim in drawn.primitives() {
                let pos = prim
                    .positions()
                    .ok_or_else(|| Error::Invalid("missing positions".into()))?;
                let idx = indices(prim);
                primitives += 1;
                if prim.topology == Topology::Triangles {
                    triangles += idx.len() as u64 / 3;
                }
                for i in idx {
                    let q = point(p.transform, pos[i as usize]);
                    for k in 0..3 {
                        min[k] = min[k].min(q[k]);
                        max[k] = max[k].max(q[k]);
                    }
                }
            }
            let name = w
                .get::<Name>(p.prototype)
                .map_or_else(|| format!("prototype_e{}", p.prototype.index()), |n| n.0.clone());
            selection.placements.insert(
                p.entity,
                primase_gltf::Selected {
                    piece: piece.map(|(mesh, from)| primase_gltf::Piece { mesh, name, from }),
                    extras: vec![("web.placement_key".into(), 1, serde_json::json!(p.entity.index()))],
                },
            );
        }
        // a document when anything is drawn: triangles, line segments or points
        let render = if primitives == 0 {
            Ok(None)
        } else {
            (|| -> Result<Option<Rendered>> {
                if !min.iter().chain(&max).all(|v| v.is_finite()) {
                    return Err(Error::Invalid("tile has no finite geometry".into()));
                }
                // one document through every stage, assembled once (attach_items)
                let (doc, report) = primase_gltf::write_document(
                    w,
                    &primase_gltf::WriteOptions {
                        group_by: primase_gltf::GroupBy::None,
                        selection: Some(selection),
                        ..Default::default()
                    },
                )?;
                if !report.textures_skipped.is_empty() {
                    return Err(Error::Invalid(format!(
                        "load textures before web export: {:?}",
                        report.textures_skipped
                    )));
                }
                // instanced from the document's own node transforms: the world's, as the glTF writer wrote them
                let (doc, instanced_placements) = instancing::apply(doc, self.options.gpu_instancing)?;
                let doc = provenance::apply(doc)?;
                #[cfg(feature = "ktx2")]
                let doc = if self.options.ktx2 {
                    self.textures
                        .apply(doc, w.resource::<prepare::PreparedImages>())?
                } else {
                    doc
                };
                let (doc, quantized) = if self.options.compress {
                    compression::apply(doc)?
                } else {
                    (doc, compression::Quantized::default())
                };
                let (doc, files) = images::externalize(doc)?;
                Ok(Some(TileInfo {
                    collision: None,
                    content: TileContent::Geometry,
                    col,
                    row,
                    cell: sub,
                    file: String::new(),
                    min,
                    max,
                    triangles,
                    placements: content.placements.len() as u64,
                    instanced_placements,
                    viewer_items: 0,
                    lights: 0,
                    layered_materials: report.layered_materials,
                    // set when the GLB is assembled
                    bytes: 0,
                    images: files.iter().map(|(n, _)| n.clone()).collect(),
                })
                .map(|info| Rendered {
                    info,
                    doc,
                    files,
                    omitted: omitted(&report)
                        .into_iter()
                        .chain(approximated(instanced_placements, quantized))
                        .collect(),
                }))
            })()
        };
        let (render, files, omitted) = match render? {
            Some(r) => (Some((r.info, r.doc)), r.files, r.omitted),
            None => (None, Vec::new(), Vec::new()),
        };
        let Some(mut tile) =
            self.attach_items((col, row), sub, render, cw, &content.items, &content.lights)?
        else {
            return Ok(None);
        };
        tile.info.file = asset_name("tile", col, row, sub, &tile.bytes);
        tile.images = files;
        Ok(Some((tile, omitted)))
    }
    /// The tile with viewer records `items` added as identity nodes (and their collision as a sidecar) and `lights`
    /// as `KHR_lights_punctual` nodes carrying their world transforms: a metadata-only tile without `render`; `None`
    /// when there is nothing.
    fn attach_items(
        &self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        render: Option<(TileInfo, primase_glb::Document)>,
        cw: &CellWorld,
        items: &[usize],
        lights: &[usize],
    ) -> Result<Option<WrittenTile>> {
        use serde_json::{Value, json};
        if items.is_empty() && lights.is_empty() {
            return render
                .map(|(info, doc)| {
                    let bytes = doc.to_glb()?;
                    Ok(WrittenTile {
                        info: TileInfo {
                            bytes: bytes.len() as u64,
                            ..info
                        },
                        bytes,
                        collision: None,
                        images: Vec::new(),
                    })
                })
                .transpose();
        }
        let items: Vec<(usize, &ViewerItem)> = items
            .iter()
            .map(|&i| {
                cw.records
                    .get(&i)
                    .map(|v| (i, v))
                    .ok_or_else(|| Error::Invalid(format!("record {i} not made")))
            })
            .collect::<Result<_>>()?;
        let mut collision_name = String::new();
        let collisions: Vec<_> = items
            .iter()
            .filter(|(_, item)| item.kind == "collision")
            .map(|(_, item)| *item)
            .collect();
        let collision = if collisions.is_empty() {
            None
        } else {
            let bytes = collision::write(&collisions)?;
            collision_name = asset_name("collision", col, row, sub, &bytes);
            Some((collision_name.clone(), bytes))
        };
        let (render, (mut doc, bin)) = match render {
            Some((info, d)) => (Some(info), (d.json, d.bin)),
            None => (
                None,
                (
                    json!({"asset":{"version":"2.0","generator":"primase-web"},"scene":0,"scenes":[{"nodes":[]}],"nodes":[]}),
                    Vec::new(),
                ),
            ),
        };
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut collision_index = 0usize;
        for (i, item) in &items {
            for k in 0..3 {
                min[k] = min[k].min(item.anchor[k]);
                max[k] = max[k].max(item.anchor[k]);
            }
            let safe: String = item
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let index = doc["nodes"].as_array().expect("nodes").len();
            let value = if item.kind == "collision" {
                let record = collision_index;
                collision_index += 1;
                json!({"name":item.name,"kind":"collision_reference","anchor":item.anchor,"data":{"file":collision_name,"record":record}})
            } else {
                serde_json::to_value(item)?
            };
            // the record, then the source entity's components
            let mut extras = serde_json::Map::new();
            extras.insert("web.viewer".into(), json!({"version":1,"value":value}));
            extras.extend(cw.components.get(i).cloned().unwrap_or_default());
            doc["nodes"]
                .as_array_mut()
                .expect("nodes")
                .push(json!({"name":format!("Viewer_{safe}_{i}"),"extras":{"primase":extras}}));
            doc["scenes"][0]["nodes"]
                .as_array_mut()
                .expect("roots")
                .push(json!(index));
        }
        if !lights.is_empty() {
            const LIGHTS: &str = "KHR_lights_punctual";
            let mut objects = Vec::new();
            for &i in lights {
                let SceneLight {
                    name,
                    light,
                    transform,
                } = &cw.lights[i];
                let mut object = primase_gltf::write::punctual_light(light).map_err(Error::Invalid)?;
                object["name"] = json!(name);
                let c = transform.cols;
                let matrix = [
                    c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1],
                    c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                ];
                let index = doc["nodes"].as_array().expect("nodes").len();
                doc["nodes"].as_array_mut().expect("nodes").push(json!({
                    "name": name,
                    "matrix": primase_core::json::f32s_value(&matrix),
                    "extensions": {LIGHTS: {"light": objects.len()}},
                }));
                doc["scenes"][0]["nodes"]
                    .as_array_mut()
                    .expect("roots")
                    .push(json!(index));
                objects.push(object);
                for k in 0..3 {
                    min[k] = min[k].min(f64::from(c[3][k]));
                    max[k] = max[k].max(f64::from(c[3][k]));
                }
            }
            doc["extensions"][LIGHTS] = json!({"lights": objects});
            let mut used = doc["extensionsUsed"].as_array().cloned().unwrap_or_default();
            used.push(json!(LIGHTS));
            doc["extensionsUsed"] = Value::Array(used);
        }
        let bytes = primase_glb::assemble(&serde_json::to_vec(&doc)?, &bin)?;
        let descriptor = collision.as_ref().map(|(file, b)| CollisionFile {
            file: file.clone(),
            bytes: b.len() as u64,
            records: collisions.len(),
        });
        let info = match render {
            Some(mut info) => {
                for k in 0..3 {
                    info.min[k] = info.min[k].min(min[k]);
                    info.max[k] = info.max[k].max(max[k]);
                }
                TileInfo {
                    collision: descriptor,
                    viewer_items: items.len() as u64,
                    lights: lights.len() as u64,
                    bytes: bytes.len() as u64,
                    ..info
                }
            }
            None => TileInfo {
                collision: descriptor,
                content: TileContent::MetadataOnly,
                col,
                row,
                cell: sub,
                file: String::new(),
                min,
                max,
                triangles: 0,
                placements: 0,
                instanced_placements: 0,
                viewer_items: items.len() as u64,
                lights: lights.len() as u64,
                layered_materials: 0,
                bytes: bytes.len() as u64,
                images: Vec::new(),
            },
        };
        Ok(Some(WrittenTile {
            info,
            bytes,
            collision,
            images: Vec::new(),
        }))
    }
    /// The index of every tile written; fails unless every planned cell was.
    pub fn finish(self) -> Result<Index> {
        if let Some(cell) = self.cells.keys().find(|c| !self.written.contains(c)) {
            return Err(Error::Invalid(format!(
                "cell {cell:?} was planned but not written"
            )));
        }
        let mut tiles = self.tiles;
        tiles.sort_by_key(|t| (t.col, t.row, t.cell));
        Ok(Index {
            // the lowest minor that has every optional field the index holds
            format_version: match (self.options.camera, &self.sun) {
                (Some(_), _) => "2.2",
                (None, Some(_)) => "2.1",
                (None, None) => "2.0",
            },
            empty_tile_policy: "omit_without_geometry_or_viewer_records; retain_metadata_without_mesh_payload",
            version: 1,
            tile_size: self.options.tile_size.get(),
            tiles,
            images: self
                .images
                .into_iter()
                .map(|(file, bytes)| ImageFile { file, bytes })
                .collect(),
            skipped_degenerate: self.skipped,
            placements_without_mesh: self.without_mesh,
            environment: self.sun.map(|sun| IndexEnvironment { sun }),
            camera: self.options.camera,
        })
    }
}
/// One kind of thing a tile's document left out, chose or drew approximately: its kind, why, and how many.
type Omitted = (NoticeKind, String, u64);
/// A tile's render geometry as its glTF document: its index entry so far, the document, its image files and what it
/// left out.
struct Rendered {
    info: TileInfo,
    doc: primase_glb::Document,
    files: ImageFiles,
    omitted: Vec<Omitted>,
}
/// What the glTF writer's `report` of a tile's document says it left out, chose or drew approximately. Its textures
/// skipped are an error before this (the writer is given loaded textures); placements it skips for a degenerate
/// transform or a missing mesh are the tile writer's own (it never selects them).
fn omitted(report: &primase_gltf::WriteReport) -> Vec<Omitted> {
    let mut out: Vec<Omitted> = vec![
        (
            NoticeKind::Unsupported,
            "placements whose prototype has levels of detail (core.lods), not written: select-lod chooses a form"
                .into(),
            report.placements_with_lods,
        ),
        (
            NoticeKind::Unsupported,
            "materials without pbr.material or core.layered, not written: their primitives drawn with no material \
             (counted per tile)"
                .into(),
            report.materials_without_pbr,
        ),
        (
            NoticeKind::Unsupported,
            "layers of layered materials whose material has no pbr.material, written as empty slots (counted per tile)"
                .into(),
            report.layers_without_pbr,
        ),
        (
            NoticeKind::Approximation,
            "primitives drawn with a layered material but lacking its weight attribute: they render as its host \
             material"
                .into(),
            report.primitives_layered_without_weights,
        ),
        (
            NoticeKind::Selection,
            "primitives drawn without their COLOR_0 stream: their material ignores vertex colour (core.vertex_color)"
                .into(),
            report.primitives_color0_ignored,
        ),
        (
            NoticeKind::Unsupported,
            "material texture slots left empty: their texture was skipped".into(),
            report.texture_slots_dropped,
        ),
        (
            NoticeKind::Unsupported,
            "texture filters glTF cannot express (a minification filter stated without a mip filter, or the \
             reverse): no minFilter written, the renderer's default"
                .into(),
            report.filters_unexpressed,
        ),
    ];
    for (name, &n) in &report.streams_skipped {
        out.push((
            NoticeKind::Unsupported,
            format!("{name} vertex streams not written: skin attributes, and tiles carry no skin"),
            n,
        ));
    }
    for (name, &n) in &report.extras_unencoded {
        out.push((
            NoticeKind::Unsupported,
            format!("{name} components without a JSON form, not in the glTF extras (counted per tile)"),
            n,
        ));
    }
    for (name, &n) in &report.components_unwritten {
        out.push((
            NoticeKind::Unsupported,
            format!("{name} components on entities a tile's document does not write"),
            n,
        ));
    }
    out.retain(|o| o.2 > 0);
    out
}
/// What the tile writer's own encoding of a tile approximated: placements stored as instance streams, and vertex
/// streams quantized to 16 bits.
fn approximated(instanced: u64, quantized: compression::Quantized) -> Vec<Omitted> {
    let mut out: Vec<Omitted> = vec![
        (
            NoticeKind::Approximation,
            "placements instanced (EXT_mesh_gpu_instancing): each matrix stored as f32 translation, rotation and \
             scale, rebuilding its scale-free basis within 1e-6 per element"
                .into(),
            instanced,
        ),
        (
            NoticeKind::Approximation,
            format!(
                "position streams stored as 16-bit integers, every vertex within {:.2} mm of where it was in the world \
                 (counted per tile)",
                compression::POSITION_BUDGET * 1000.
            ),
            quantized.positions,
        ),
        (
            NoticeKind::Approximation,
            "normal, tangent and 0-1 texture coordinate streams stored as 16-bit normalized integers (counted per \
             tile)"
                .into(),
            quantized.directions_and_texcoords,
        ),
    ];
    out.retain(|o| o.2 > 0);
    out
}
/// `e`'s mesh, which must be loaded.
fn loaded(w: &World, e: Entity) -> Result<&Mesh> {
    w.get::<Mesh>(e).ok_or_else(|| {
        Error::Invalid(if w.has::<primase_core::MeshFile>(e) {
            format!("prototype {e:?} has no loaded mesh")
        } else {
            format!("prototype {e:?} has no mesh")
        })
    })
}
/// Every tile of an already prepared `world` (meshes and textures loaded, materials PBR): [`plan`], each cell
/// written from the world's closure of it, [`TilePlan::finish`]. Placements are partitioned whole by pivot, or with
/// [`Options::split_by_tile`] by element centre (a triangle's, a line segment's, a point), assigning complete
/// elements (no clipping), so bounds may exceed the nominal cell; prototypes keep their sharing within a tile. Degenerate placements are counted and omitted, as
/// in the glTF writer.
pub fn write(world: &World, options: &Options) -> Result<TileSet> {
    let mut plan = plan(world, options)?;
    for e in plan.unsplit() {
        plan.split(world, e)?;
    }
    let mut set = TileSet {
        collision_files: BTreeMap::new(),
        index: Index {
            format_version: "2.0",
            empty_tile_policy: "",
            version: 1,
            tile_size: options.tile_size.get(),
            tiles: Vec::new(),
            images: Vec::new(),
            skipped_degenerate: 0,
            placements_without_mesh: 0,
            environment: None,
            camera: None,
        },
        files: BTreeMap::new(),
        images: BTreeMap::new(),
        over_budget: Vec::new(),
        splits: Vec::new(),
        notices: Vec::new(),
    };
    for cell in plan.cells() {
        let w = world.subset(&world.closure(plan.roots(cell).into_iter().chain(plan.shared(cell))));
        for tile in plan.write_cell(cell, &w)? {
            set.files.insert(tile.info.file.clone(), tile.bytes);
            set.images.extend(tile.images);
            if let Some((name, bytes)) = tile.collision {
                set.collision_files.insert(name, bytes);
            }
        }
    }
    set.over_budget = plan.over_budget().to_vec();
    set.splits = plan.splits().to_vec();
    set.notices = plan.notices();
    set.index = plan.finish()?;
    Ok(set)
}
/// `<kind>_<col>_<row>.<sha256>.glb`, with `_<level>-<x>-<y>` after the row for a subcell.
fn asset_name(kind: &str, col: i32, row: i32, cell: Option<Subcell>, bytes: &[u8]) -> String {
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    match cell {
        None => format!("{kind}_{col}_{row}.{hash}.glb"),
        Some(c) => format!("{kind}_{col}_{row}_{}-{}-{}.{hash}.glb", c.level, c.x, c.y),
    }
}
impl Index {
    /// Publish `tiles.json` in `folder`, after checking that every image, tile and collision file it names is there
    /// with its length (a driver writes each [`WrittenTile`] first).
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "writer I/O: writes the index and its shared images"
    )]
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        for (file, bytes) in self
            .images
            .iter()
            .map(|i| (&i.file, i.bytes))
            .chain(self.tiles.iter().flat_map(|t| {
                std::iter::once((&t.file, t.bytes)).chain(t.collision.iter().map(|c| (&c.file, c.bytes)))
            }))
        {
            let path = folder.join(file);
            let len = std::fs::metadata(&path)
                .map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?
                .len();
            if len != bytes {
                return Err(Error::Invalid(format!(
                    "{file} is {len} bytes, the index says {bytes}"
                )));
            }
        }
        std::fs::create_dir_all(folder).map_err(|source| Error::Io {
            path: folder.to_owned(),
            source,
        })?;
        let path = folder.join("tiles.json");
        std::fs::write(&path, serde_json::to_vec_pretty(self)?).map_err(|source| Error::Io { path, source })
    }
}
impl TileSet {
    /// Write only generated tile filenames, then publish tiles.json after every tile
    /// is present. Unrelated files in the destination are retained.
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "writer I/O: writes the tile set"
    )]
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        let names: BTreeSet<_> = self.index.tiles.iter().map(|t| t.file.as_str()).collect();
        if names.len() != self.index.tiles.len()
            || names.len() != self.files.len()
            || self.index.tiles.iter().any(|t| {
                self.files.get(&t.file).is_none_or(|b| {
                    b.len() as u64 != t.bytes || t.file != asset_name("tile", t.col, t.row, t.cell, b)
                })
            })
        {
            return Err(Error::Invalid("tile index does not match generated files".into()));
        }
        let index_bytes = serde_json::to_vec_pretty(&self.index)?;
        let collision_entries: Vec<_> = self
            .index
            .tiles
            .iter()
            .filter_map(|t| t.collision.as_ref().map(|c| (t, c)))
            .collect();
        if collision_entries.len() != self.collision_files.len()
            || collision_entries.iter().any(|(t, c)| {
                self.collision_files.get(&c.file).is_none_or(|b| {
                    b.len() as u64 != c.bytes || c.file != asset_name("collision", t.col, t.row, t.cell, b)
                })
            })
        {
            return Err(Error::Invalid(
                "collision index does not match generated files".into(),
            ));
        }
        if self.index.images.len() != self.images.len()
            || self.index.images.iter().any(|i| {
                self.images.get(&i.file).is_none_or(|b| {
                    b.len() as u64 != i.bytes
                        || std::path::Path::new(&i.file)
                            .extension()
                            .and_then(|x| x.to_str())
                            .is_none_or(|x| {
                                images::image_name(
                                    match x {
                                        "png" => "image/png",
                                        "ktx2" => "image/ktx2",
                                        _ => "image/jpeg",
                                    },
                                    b,
                                )
                                .ok()
                                .as_deref()
                                    != Some(i.file.as_str())
                            })
                })
            })
        {
            return Err(Error::Invalid(
                "image index does not match generated files".into(),
            ));
        }
        let dir = folder.join(IMAGE_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io { path: dir, source })?;
        // images first, then the tiles that reference them, then the index
        for (name, bytes) in self.images.iter().chain(&self.files).chain(&self.collision_files) {
            let path = folder.join(name);
            std::fs::write(&path, bytes).map_err(|source| Error::Io { path, source })?;
        }
        let path = folder.join("tiles.json");
        std::fs::write(&path, index_bytes).map_err(|source| Error::Io { path, source })?;
        Ok(())
    }
}
