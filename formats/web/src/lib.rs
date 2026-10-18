//! Web tile sets in the core frame (metres, +Y up). Tiles use east/north ground coordinates.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

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
#[cfg(feature = "ktx2")]
pub mod ktx2;
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
    Json(#[from] serde_json::Error),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
pub struct Options {
    pub tile_size: f64,
    pub gpu_instancing: bool,
    /// Quantize vertex attributes and encode vertex streams with meshopt.
    pub compress: bool,
    /// Optional web texture lowering; PNG embedding remains the default.
    #[cfg(feature = "ktx2")]
    pub ktx2: bool,
    /// KTX2 lowering: largest texture edge per role.
    #[cfg(feature = "ktx2")]
    pub texture_caps: ktx2::TextureCaps,
    /// KTX2 lowering: caps for images whose glTF name starts with a prefix; first match wins.
    #[cfg(feature = "ktx2")]
    pub texture_caps_by_name: Vec<(String, ktx2::TextureCaps)>,
    /// KTX2 lowering: a disk cache of encoded images, kept across runs (`None`: encode every run).
    #[cfg(feature = "ktx2")]
    pub cache: Option<std::sync::Arc<primase_cache::Cache>>,
    /// KTX2 lowering: encode on every cache hit as well and fail when the stored value differs.
    #[cfg(feature = "ktx2")]
    pub cache_verify: bool,
    /// Viewer-only records, already lowered from a source dialect into world space.
    pub viewer_items: Vec<ViewerItem>,
    /// Placement entities to partition by transformed triangle centre, rather than pivot.
    pub split_by_tile: BTreeSet<Entity>,
    pub base_dir: PathBuf,
    /// The largest file a tile may carry, its GLB and its collision sidecar alike: a cell with a larger one is split
    /// into quadtree [`Subcell`]s, recursively, until each file fits. `None` keeps every cell whole.
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
/// The default [`Options::tile_byte_budget`], 96 MiB.
pub const DEFAULT_TILE_BYTE_BUDGET: u64 = 96 * 1024 * 1024;
/// The deepest [`Subcell::level`] a cell is split to.
pub const MAX_SUBCELL_LEVEL: u8 = 8;
impl Default for Options {
    fn default() -> Self {
        Self {
            tile_size: 200.,
            gpu_instancing: true,
            compress: true,
            #[cfg(feature = "ktx2")]
            ktx2: false,
            #[cfg(feature = "ktx2")]
            texture_caps: ktx2::TextureCaps::default(),
            #[cfg(feature = "ktx2")]
            texture_caps_by_name: Vec::new(),
            #[cfg(feature = "ktx2")]
            cache: None,
            #[cfg(feature = "ktx2")]
            cache_verify: false,
            viewer_items: Vec::new(),
            split_by_tile: BTreeSet::new(),
            base_dir: PathBuf::new(),
            tile_byte_budget: Some(DEFAULT_TILE_BYTE_BUDGET),
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
        let size = match p.topology {
            Topology::Triangles => 3,
            Topology::Lines => 2,
            Topology::Points => 1,
        };
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
/// One placement as a tile holds it: the whole prototype, or (filed by triangle centre) the triangles of each of its
/// primitives that fall in the tile, by index into that primitive's triangle list.
#[derive(Clone, Debug)]
struct Placement {
    entity: Entity,
    prototype: Entity,
    variant: u32,
    transform: Transform,
    triangles: Option<Vec<(usize, Vec<u32>)>>,
}
/// A planned cell: its placements and the indices of its viewer records and lights.
#[derive(Clone, Debug, Default)]
struct Cell {
    placements: Vec<Placement>,
    items: Vec<usize>,
    lights: Vec<usize>,
}
/// A scene light as planned: its name, the light and its world transform.
#[derive(Clone, Debug)]
struct PlannedLight {
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
/// What goes in which tile, decided before any tile is written: placements by pivot (or, for
/// [`Options::split_by_tile`], by transformed triangle centre), viewer records and the world's collision by anchor,
/// the world's `core.light`s by position (taken now, like viewer records: they are not in any cell's closure).
///
/// A driver prepares one cell at a time: the world's [`World::closure`] of the cell's [`TilePlan::roots`], as a
/// [`World::subset`], with its meshes and textures loaded and lowered to PBR, then [`TilePlan::write_cell`]; then
/// [`TilePlan::finish`]. The writer neither loads nor lowers: it fails, as before, on what is missing. Placements
/// filed by triangle centre need their prototype's geometry to be planned: [`TilePlan::split`] each of
/// [`TilePlan::unsplit`] first, from a world holding that prototype loaded.
pub struct TilePlan {
    options: Options,
    items: Vec<ViewerItem>,
    /// For a record from a world entity (a curve or a marker): the entity's other shown components, by name, as
    /// glTF extras show them, for the record's node.
    item_components: BTreeMap<usize, serde_json::Map<String, serde_json::Value>>,
    /// Entities whose `core.collision` the read world held: planned with it. A prototype's collision that arrives
    /// with a cell's world instead (a reader that loads it per part) is written with that cell ([`TilePlan::write_cell`]).
    planned_collision: BTreeSet<Entity>,
    /// Placements whose part-loaded collision has been written (a placement filed by triangle centre is in several
    /// cells: its collision goes with the first).
    part_collided: BTreeSet<Entity>,
    ground_markers: u64,
    lights: Vec<PlannedLight>,
    sun: Option<IndexSun>,
    cells: BTreeMap<(i32, i32), Cell>,
    unsplit: Vec<Placement>,
    written: BTreeSet<(i32, i32)>,
    tiles: Vec<TileInfo>,
    images: BTreeMap<String, u64>,
    over_budget: Vec<OverBudget>,
    skipped: u64,
    without_mesh: u64,
    #[cfg(feature = "ktx2")]
    textures: ktx2::TextureEncoder,
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
/// into geometry. A material entity without `pbr.material` is written as the glTF fallback material and counted.
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
/// Plan `world`'s tiles. Geometry may still be in files; the world's collision and viewer records are read now.
pub fn plan(world: &World, options: &Options) -> Result<TilePlan> {
    plan_with_geometry(world, options, &|_| false)
}

/// No mesh in the world or the cell world: neither a loaded one nor a file to load.
fn meshless(world: &World, prototype: Entity) -> bool {
    !world.has::<Mesh>(prototype) && !world.has::<primase_core::MeshFile>(prototype)
}

/// [`plan`] for a world whose geometry its cells will load (a reader that loads assets per cell): `geometry` says
/// which prototypes have geometry before it is loaded (they have a box, see [`spanning`]). A placement is planned
/// when its prototype has a mesh, a mesh file or `geometry`; one whose cell then loads no mesh counts as a placement
/// without mesh ([`TilePlan::write_cell`], [`TilePlan::split`]).
pub fn plan_with_geometry(
    world: &World,
    options: &Options,
    geometry: &dyn Fn(Entity) -> bool,
) -> Result<TilePlan> {
    if !options.tile_size.is_finite() || options.tile_size <= 0. {
        return Err(Error::Invalid("tile size must be finite and positive".into()));
    }
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
    let sun_entity = world
        .resource::<primase_core::Environment>()
        .and_then(|env| env.sun);
    let mut placements = BTreeMap::new();
    for (e, i) in world.iter::<Instance>() {
        let t = world
            .get::<Transform>(e)
            .ok_or_else(|| Error::Invalid(format!("placement {e:?} has no transform")))?;
        placements.insert(
            e,
            Placement {
                entity: e,
                prototype: i.prototype,
                variant: i.variant,
                transform: *t,
                triangles: None,
            },
        );
    }
    for (e, _) in world.iter::<Mesh>() {
        if !world.has::<Instance>(e)
            && let Some(&t) = world.get::<Transform>(e)
        {
            placements.insert(
                e,
                Placement {
                    entity: e,
                    prototype: e,
                    variant: 0,
                    transform: t,
                    triangles: None,
                },
            );
        }
    }
    for e in &options.split_by_tile {
        if !placements.contains_key(e) {
            return Err(Error::Invalid(format!("split marker {e:?} is not a placement")));
        }
    }
    let mut plan = TilePlan {
        options: Options {
            viewer_items: Vec::new(),
            ..options.clone()
        },
        items: options.viewer_items.clone(),
        item_components: BTreeMap::new(),
        planned_collision: world.iter::<primase_core::Collision>().map(|(e, _)| e).collect(),
        part_collided: BTreeSet::new(),
        ground_markers: 0,
        lights: Vec::new(),
        sun: sun_entity.map(|e| sun(world, e)).transpose()?,
        cells: BTreeMap::new(),
        unsplit: Vec::new(),
        written: BTreeSet::new(),
        images: BTreeMap::new(),
        tiles: Vec::new(),
        over_budget: Vec::new(),
        skipped: 0,
        without_mesh: 0,
        #[cfg(feature = "ktx2")]
        textures: ktx2::TextureEncoder::new(
            options.texture_caps,
            options.texture_caps_by_name.clone(),
            options.cache.clone(),
            options.cache_verify,
        )?,
    };
    for p in placements.into_values() {
        match p.transform.classify() {
            TransformKind::Degenerate => {
                plan.skipped += 1;
                continue;
            }
            TransformKind::Mirrored | TransformKind::Sheared => {
                return Err(Error::Invalid(format!(
                    "placement {:?}: a non-rigid transform; the writer takes rigid placements, run bake-nonrigid first",
                    p.entity
                )));
            }
            TransformKind::Regular => {}
        }
        if meshless(world, p.prototype) && !geometry(p.prototype) {
            plan.without_mesh += 1;
            continue;
        }
        if options.split_by_tile.contains(&p.entity) {
            plan.unsplit.push(p);
        } else {
            let key = coordinate(p.transform.cols[3].map(f64::from), options.tile_size)?;
            plan.cells.entry(key).or_default().placements.push(p);
        }
    }
    // after the caller's viewer items: the world's curves and markers, then its collision geometry, in world space
    plan.ground_markers = core_items(world, &mut plan.items, &mut plan.item_components)?;
    plan.items.extend(collision::world_items(world)?);
    for (i, item) in plan.items.iter().enumerate() {
        if item.anchor.iter().any(|v| !v.is_finite()) || item.kind.is_empty() {
            return Err(Error::Invalid(
                "viewer item needs a finite anchor and nonempty kind".into(),
            ));
        }
        let key = coordinate(item.anchor, options.tile_size)?;
        plan.cells.entry(key).or_default().items.push(i);
    }
    for (e, light) in world.iter::<primase_core::Light>() {
        if Some(e) == sun_entity {
            continue;
        }
        let transform = world.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
        let key = coordinate(transform.cols[3].map(f64::from), options.tile_size)?;
        plan.cells.entry(key).or_default().lights.push(plan.lights.len());
        plan.lights.push(PlannedLight {
            name: world
                .get::<Name>(e)
                .map_or_else(|| format!("light_{}", e.index()), |n| n.0.clone()),
            light: light.clone(),
            transform,
        });
    }
    Ok(plan)
}
/// Viewer records of the world's `core.curve`s (kind `grind_curve`) and then its `core.marker`s (`spawn`, or
/// `marker` for any other kind), each by entity, appended to `items`: named by their `core.name`, anchored at a
/// curve's first point or a marker's position, with the core component as their data and the entity's other shown
/// components in `components`. A marker with a ground position is anchored at height 0; their number is returned.
fn core_items(
    world: &World,
    items: &mut Vec<ViewerItem>,
    components: &mut BTreeMap<usize, serde_json::Map<String, serde_json::Value>>,
) -> Result<u64> {
    use primase_core::{Component, Curve, Marker};
    let json_error = |e: Entity, detail: String| Error::Invalid(format!("{e:?}: {detail}"));
    let mut record = |e: Entity, kind: &str, anchor: [f64; 3], core: &str| -> Result<()> {
        let mut data = None;
        let mut others = serde_json::Map::new();
        for c in world
            .components_json(e)
            .map_err(|err| json_error(e, err.to_string()))?
        {
            match (c.name, c.value) {
                (name, Some(v)) if name == core => data = Some(v),
                (Name::NAME | Transform::NAME, _) | (_, None) => {}
                (name, Some(v)) => {
                    others.insert(
                        name.to_owned(),
                        serde_json::json!({"version": c.version, "value": v}),
                    );
                }
            }
        }
        if !others.is_empty() {
            components.insert(items.len(), others);
        }
        items.push(ViewerItem {
            name: world
                .get::<Name>(e)
                .map_or_else(|| format!("Entity_{}", e.index()), |n| n.0.clone()),
            kind: kind.to_owned(),
            anchor,
            data: data.ok_or_else(|| json_error(e, format!("{core} has no JSON view")))?,
        });
        Ok(())
    };
    for (e, c) in world.iter::<Curve>() {
        if c.tangents.is_some() {
            return Err(json_error(
                e,
                "a Bezier curve: viewer records are polylines (flatten-curves first)".into(),
            ));
        }
        let first = c
            .points
            .first()
            .ok_or_else(|| json_error(e, "a curve without points".into()))?;
        record(e, "grind_curve", first.map(f64::from), Curve::NAME)?;
    }
    let mut ground = 0;
    for (e, m) in world.iter::<Marker>() {
        let anchor = match (world.get::<Transform>(e), m.ground) {
            (Some(t), _) => t.cols[3].map(f64::from),
            // east, north: x = east, z = -north
            (None, Some([east, north])) => {
                ground += 1;
                [f64::from(east), 0., -f64::from(north)]
            }
            (None, None) => return Err(json_error(e, "a marker without a position".into())),
        };
        let kind = if m.kind == "spawn" { "spawn" } else { "marker" };
        record(e, kind, anchor, Marker::NAME)?;
    }
    Ok(ground)
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
    /// Viewer records of markers with a ground position (no height: the source leaves it to its consumer), anchored
    /// at height 0.
    pub fn ground_markers(&self) -> u64 {
        self.ground_markers
    }
    /// Placements filed by triangle centre that still wait for [`TilePlan::split`].
    pub fn unsplit(&self) -> Vec<Entity> {
        self.unsplit.iter().map(|p| p.entity).collect()
    }
    /// File the triangles of placement `e` (one of [`TilePlan::unsplit`]) by their transformed centres. `world`
    /// holds its prototype's mesh loaded; a prototype that turned out to have no mesh (none loaded and no file) is
    /// counted as a placement without mesh, as at planning.
    pub fn split(&mut self, world: &World, e: Entity) -> Result<()> {
        let i = self
            .unsplit
            .iter()
            .position(|p| p.entity == e)
            .ok_or_else(|| Error::Invalid(format!("{e:?} is not a placement waiting to be split")))?;
        let p = self.unsplit.remove(i);
        if meshless(world, p.prototype) {
            self.without_mesh += 1;
            return Ok(());
        }
        let mesh = loaded(world, p.prototype)?;
        validate_mesh(mesh)?;
        let mut parts = BTreeMap::<(i32, i32), Vec<(usize, Vec<u32>)>>::new();
        for (part, prim) in mesh.primitives().iter().enumerate() {
            if prim.topology != Topology::Triangles {
                return Err(Error::Invalid("split-by-tile requires triangle lists".into()));
            }
            let pos = prim
                .positions()
                .ok_or_else(|| Error::Invalid("missing positions".into()))?;
            let idx = indices(prim);
            let mut pieces = BTreeMap::<(i32, i32), Vec<u32>>::new();
            for (t, tri) in idx.as_chunks::<3>().0.iter().enumerate() {
                let points = tri.map(|i| point(p.transform, pos[i as usize]));
                let centre = std::array::from_fn(|k| (points[0][k] + points[1][k] + points[2][k]) / 3.);
                pieces
                    .entry(coordinate(centre, self.options.tile_size)?)
                    .or_default()
                    .push(t as u32);
            }
            for (key, tris) in pieces {
                parts.entry(key).or_default().push((part, tris));
            }
        }
        for (key, triangles) in parts {
            self.cells.entry(key).or_default().placements.push(Placement {
                triangles: Some(triangles),
                ..p.clone()
            });
        }
        Ok(())
    }
    /// The planned cells, in order.
    pub fn cells(&self) -> Vec<(i32, i32)> {
        self.cells.keys().copied().collect()
    }
    /// The entities a cell's tiles are written from: its placements and their prototypes. A driver prepares the
    /// world's [`World::closure`] of them.
    pub fn roots(&self, cell: (i32, i32)) -> BTreeSet<Entity> {
        self.cells
            .get(&cell)
            .into_iter()
            .flat_map(|c| &c.placements)
            .flat_map(|p| [p.entity, p.prototype])
            .collect()
    }
    /// Write cell `cell` from `world`: exactly the closure of its [`TilePlan::roots`], meshes and textures loaded and
    /// materials lowered. The writer only reads it: each tile is a selection of the world's placements (and of the
    /// triangles of those filed by triangle centre), so the same world can be written by another plan (a plain set).
    /// Returns its tiles (none for a cell with nothing to draw or show).
    pub fn write_cell(&mut self, cell: (i32, i32), world: &World) -> Result<Vec<WrittenTile>> {
        if !self.unsplit.is_empty() {
            return Err(Error::Invalid(
                "split every placement filed by triangle centre before writing".into(),
            ));
        }
        if !self.written.insert(cell) {
            return Err(Error::Invalid(format!("cell {cell:?} written twice")));
        }
        let Some(mut content) = self.cells.get(&cell).cloned() else {
            return Err(Error::Invalid(format!("cell {cell:?} is not planned")));
        };
        let closure = world.closure(self.roots(cell));
        if closure.len() != world.entity_count() {
            return Err(Error::Invalid(format!(
                "cell {cell:?}: the world has {} entities, the closure of the cell's roots {}",
                world.entity_count(),
                closure.len()
            )));
        }
        if let Some((name, e, detail)) = world.verify_components().into_iter().next() {
            return Err(Error::Invalid(format!("{name} on {e:?}: {detail}")));
        }
        // a prototype whose collision came with the cell's world (not in the read world): this cell's placements of it
        // collide, each placement once
        let placed: BTreeSet<Entity> = content
            .placements
            .iter()
            .filter(|p| {
                !self.planned_collision.contains(&p.prototype) && !self.part_collided.contains(&p.entity)
            })
            .map(|p| p.entity)
            .collect();
        for item in collision::placement_items(world, &placed)? {
            if item.anchor.iter().any(|v| !v.is_finite()) {
                return Err(Error::Invalid("collision with a non-finite position".into()));
            }
            content.items.push(self.items.len());
            self.items.push(item);
        }
        self.part_collided.extend(placed);
        // a prototype whose cell loaded no mesh (planned from a world that loads per cell): counted as at planning
        let before = content.placements.len();
        content.placements.retain(|p| !meshless(world, p.prototype));
        self.without_mesh += (before - content.placements.len()) as u64;
        let mut out = Vec::new();
        self.emit(cell, None, world, content, false, &mut out)?;
        Ok(out)
    }
    /// Tiles left over the byte budget so far: a cell is split until each file fits, except where what remains is
    /// one placement or viewer record, which no cell boundary divides, or at [`MAX_SUBCELL_LEVEL`]. Such tiles are
    /// written as they are (they still load; only their caching suffers) and listed here.
    pub fn over_budget(&self) -> &[OverBudget] {
        &self.over_budget
    }
    /// Write `content` of `cell`'s subcell `sub` (`None`: the whole cell), splitting it while a file is over the
    /// budget. `over`: the content is already known to be too large (its parent's content, all in this subcell).
    fn emit(
        &mut self,
        cell: (i32, i32),
        sub: Option<Subcell>,
        world: &World,
        content: Cell,
        over: bool,
        out: &mut Vec<WrittenTile>,
    ) -> Result<()> {
        let level = sub.map_or(0, |s| s.level);
        if !over {
            let Some(tile) = self.tile(cell, sub, world, &content)? else {
                return Ok(());
            };
            let largest = tile
                .bytes
                .len()
                .max(tile.collision.as_ref().map_or(0, |(_, b)| b.len())) as u64;
            if self.options.tile_byte_budget.is_none_or(|b| largest <= b) {
                self.keep(tile, out);
                return Ok(());
            }
            let units = content.items.len()
                + content.lights.len()
                + content
                    .placements
                    .iter()
                    .map(|p| {
                        p.triangles
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
                self.keep(tile, out);
                return Ok(());
            }
        }
        let children = self.children(cell, sub, world, content)?;
        if children.len() == 1 && level + 1 < MAX_SUBCELL_LEVEL {
            // all of it in one child: the same content, as large; split further without writing it
            let (k, c) = children.into_iter().next().expect("one child");
            return self.emit(cell, Some(k), world, c, true, out);
        }
        for (k, c) in children {
            self.emit(cell, Some(k), world, c, false, out)?;
        }
        Ok(())
    }
    fn keep(&mut self, tile: WrittenTile, out: &mut Vec<WrittenTile>) {
        for (name, bytes) in &tile.images {
            self.images.insert(name.clone(), bytes.len() as u64);
        }
        self.tiles.push(tile.info.clone());
        out.push(tile);
    }
    /// `content` of `cell`'s subcell `sub`, divided among its four children by the cell rules: placements by pivot,
    /// placements filed by triangle centre by their triangles' centres (their prototype loaded in `world`), viewer
    /// records by anchor, scene lights by position. Each child keeps the parent's order.
    fn children(
        &self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        world: &World,
        content: Cell,
    ) -> Result<BTreeMap<Subcell, Cell>> {
        let size = self.options.tile_size;
        let mut out = BTreeMap::<Subcell, Cell>::new();
        for p in content.placements {
            let Some(parts) = &p.triangles else {
                let k = child(col, row, sub, ground(p.transform.cols[3].map(f64::from)), size);
                out.entry(k).or_default().placements.push(p);
                continue;
            };
            let mesh = loaded(world, p.prototype)?;
            let mut by = BTreeMap::<Subcell, Vec<(usize, Vec<u32>)>>::new();
            for (part, tris) in parts {
                let prim = &mesh.primitives()[*part];
                let pos = prim
                    .positions()
                    .ok_or_else(|| Error::Invalid("missing positions".into()))?;
                let idx = indices(prim);
                let mut pieces = BTreeMap::<Subcell, Vec<u32>>::new();
                for &t in tris {
                    let tri = &idx[3 * t as usize..3 * t as usize + 3];
                    let points: [[f64; 3]; 3] =
                        std::array::from_fn(|c| point(p.transform, pos[tri[c] as usize]));
                    let centre = std::array::from_fn(|k| (points[0][k] + points[1][k] + points[2][k]) / 3.);
                    pieces
                        .entry(child(col, row, sub, ground(centre), size))
                        .or_default()
                        .push(t);
                }
                for (k, v) in pieces {
                    by.entry(k).or_default().push((*part, v));
                }
            }
            for (k, triangles) in by {
                out.entry(k).or_default().placements.push(Placement {
                    triangles: Some(triangles),
                    ..p.clone()
                });
            }
        }
        for i in content.items {
            let k = child(col, row, sub, ground(self.items[i].anchor), size);
            out.entry(k).or_default().items.push(i);
        }
        for i in content.lights {
            let k = child(
                col,
                row,
                sub,
                ground(self.lights[i].transform.cols[3].map(f64::from)),
                size,
            );
            out.entry(k).or_default().lights.push(i);
        }
        Ok(out)
    }
    /// One tile of `content` from `w`: the tile's placements written as a [`primase_gltf::Selection`] of the world
    /// (a placement filed by triangle centre as a piece of its own: those of its triangles the tile holds), which
    /// leaves the world unchanged.
    fn tile(
        &mut self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        w: &World,
        content: &Cell,
    ) -> Result<Option<WrittenTile>> {
        let mut selection = primase_gltf::Selection {
            // the environment is the index's, scene-wide: a tile does not carry it
            omit_resources: [<primase_core::Environment as primase_core::Resource>::NAME.to_owned()].into(),
            ..Default::default()
        };
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut triangles = 0;
        for p in &content.placements {
            let mesh = loaded(w, p.prototype)?;
            validate_mesh(mesh)?;
            let piece = match &p.triangles {
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
                    for (part, tris) in parts {
                        let prim = &mesh.primitives()[*part];
                        let all = indices(prim);
                        let idx: Vec<u32> = tris
                            .iter()
                            .flat_map(|&t| all[3 * t as usize..3 * t as usize + 3].iter().copied())
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
        let render = if triangles == 0 {
            Ok(None)
        } else {
            (|| -> Result<Option<(TileInfo, primase_gltf::glb::Document, ImageFiles)>> {
                if !min.iter().chain(&max).all(|v| v.is_finite()) {
                    return Err(Error::Invalid("tile has no finite geometry".into()));
                }
                // one document through every stage, assembled once (attach_items)
                let (doc, report) = primase_gltf::write_document(
                    w,
                    &primase_gltf::WriteOptions {
                        group_by: primase_gltf::GroupBy::None,
                        base_dir: self.options.base_dir.clone(),
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
                let transforms = content
                    .placements
                    .iter()
                    .map(|p| (p.entity.index(), p.transform))
                    .collect();
                let (doc, instanced_placements) =
                    instancing::apply(doc, &transforms, self.options.gpu_instancing)?;
                let doc = provenance::apply(doc)?;
                #[cfg(feature = "ktx2")]
                let doc = if self.options.ktx2 {
                    self.textures.apply(doc)?
                } else {
                    doc
                };
                let doc = if self.options.compress {
                    compression::apply(doc)?
                } else {
                    doc
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
                .map(|info| (info, doc, files)))
            })()
        };
        let (render, files) = match render? {
            Some((info, bytes, files)) => (Some((info, bytes)), files),
            None => (None, Vec::new()),
        };
        let Some(mut tile) = self.attach_items((col, row), sub, render, &content.items, &content.lights)?
        else {
            return Ok(None);
        };
        tile.info.file = asset_name("tile", col, row, sub, &tile.bytes);
        tile.images = files;
        Ok(Some(tile))
    }
    /// The tile with viewer records `items` added as identity nodes (and their collision as a sidecar) and `lights`
    /// as `KHR_lights_punctual` nodes carrying their world transforms: a metadata-only tile without `render`; `None`
    /// when there is nothing.
    fn attach_items(
        &self,
        (col, row): (i32, i32),
        sub: Option<Subcell>,
        render: Option<(TileInfo, primase_gltf::glb::Document)>,
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
        let items: Vec<(usize, &ViewerItem)> = items.iter().map(|&i| (i, &self.items[i])).collect();
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
            extras.extend(self.item_components.get(i).cloned().unwrap_or_default());
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
                let PlannedLight {
                    name,
                    light,
                    transform,
                } = &self.lights[i];
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
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?;
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
        // every planned scene light reaches a tile
        let written: u64 = self.tiles.iter().map(|t| t.lights).sum();
        if written != self.lights.len() as u64 {
            return Err(Error::Invalid(format!(
                "{written} scene lights written of {} planned",
                self.lights.len()
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
            tile_size: self.options.tile_size,
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
/// [`Options::split_by_tile`] by triangle centre, assigning complete triangles (no clipping), so bounds may exceed
/// the nominal cell; prototypes keep their sharing within a tile. Degenerate placements are counted and omitted, as
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
            tile_size: options.tile_size,
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
    };
    for cell in plan.cells() {
        let w = world.subset(&world.closure(plan.roots(cell)));
        for tile in plan.write_cell(cell, &w)? {
            set.files.insert(tile.info.file.clone(), tile.bytes);
            set.images.extend(tile.images);
            if let Some((name, bytes)) = tile.collision {
                set.collision_files.insert(name, bytes);
            }
        }
    }
    set.over_budget = plan.over_budget().to_vec();
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
