//! Web tile sets in the core frame (metres, +Y up). Tiles use east/north ground coordinates.
use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, Name, Primitive, Topology, Transform, TransformKind, World,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
mod collision;
mod compression;
mod instancing;
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
    /// Viewer-only records, already lowered from a source dialect into world space.
    pub viewer_items: Vec<ViewerItem>,
    /// Placement entities to partition by transformed triangle centre, rather than pivot.
    pub split_by_tile: BTreeSet<Entity>,
    pub base_dir: PathBuf,
}
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
            viewer_items: Vec::new(),
            split_by_tile: BTreeSet::new(),
            base_dir: PathBuf::new(),
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
    pub file: String,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub triangles: u64,
    pub placements: u64,
    pub instanced_placements: u64,
    pub viewer_items: u64,
    /// Scene lights in this tile (format 1.1; omitted when 0).
    #[serde(skip_serializing_if = "is_zero")]
    pub lights: u64,
    pub bytes: u64,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
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
    pub skipped_degenerate: u64,
    pub placements_without_mesh: u64,
}
#[derive(Clone, Debug)]
pub struct TileSet {
    pub collision_files: BTreeMap<String, Vec<u8>>,
    pub index: Index,
    pub files: BTreeMap<String, Vec<u8>>,
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
fn coordinate(p: [f64; 3], size: f64) -> Result<(i32, i32)> {
    let coord = |v: f64| {
        let q = (v / size).floor();
        if q.is_finite() && q >= i32::MIN as f64 && q <= i32::MAX as f64 {
            Ok(q as i32)
        } else {
            Err(Error::Invalid("tile coordinate outside i32".into()))
        }
    };
    // Project through the shared frame basis in f64: rounding transformed triangle
    // centres to f32 here would move faces across exact tile boundaries.
    let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].map(primase_core::frame::ground);
    let ground: [f64; 2] = std::array::from_fn(|k| (0..3).map(|i| p[i] * axes[i][k] as f64).sum());
    Ok((coord(ground[0])?, coord(ground[1])?))
}
fn indices(p: &Primitive) -> Vec<u32> {
    p.indices
        .clone()
        .unwrap_or_else(|| (0..p.vertex_count() as u32).collect())
}
fn validate_mesh(mesh: &Mesh) -> Result<()> {
    for p in &mesh.primitives {
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
}
impl WrittenTile {
    /// Write the tile's GLB and collision sidecar into `folder`.
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        std::fs::create_dir_all(folder).map_err(|source| Error::Io {
            path: folder.into(),
            source,
        })?;
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
    lights: Vec<PlannedLight>,
    placements: BTreeSet<Entity>,
    cells: BTreeMap<(i32, i32), Cell>,
    unsplit: Vec<Placement>,
    written: BTreeSet<(i32, i32)>,
    tiles: Vec<TileInfo>,
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
/// Plan `world`'s tiles. Geometry may still be in files; the world's collision and viewer records are read now.
pub fn plan(world: &World, options: &Options) -> Result<TilePlan> {
    if !options.tile_size.is_finite() || options.tile_size <= 0. {
        return Err(Error::Invalid("tile size must be finite and positive".into()));
    }
    if let Some((name, e, detail)) = world.verify_components().into_iter().next() {
        return Err(Error::Invalid(format!("{name} on {e:?}: {detail}")));
    }
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
        lights: Vec::new(),
        placements: placements.keys().copied().collect(),
        cells: BTreeMap::new(),
        unsplit: Vec::new(),
        written: BTreeSet::new(),
        tiles: Vec::new(),
        skipped: 0,
        without_mesh: 0,
        #[cfg(feature = "ktx2")]
        textures: ktx2::TextureEncoder::new(options.texture_caps, options.texture_caps_by_name.clone())?,
    };
    for p in placements.into_values() {
        if p.transform.classify() == TransformKind::Degenerate {
            plan.skipped += 1;
            continue;
        }
        if !world.has::<Mesh>(p.prototype) && !world.has::<primase_core::MeshFile>(p.prototype) {
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
    // the world's collision geometry, in world space, after the caller's viewer items
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
impl TilePlan {
    /// Placements filed by triangle centre that still wait for [`TilePlan::split`].
    pub fn unsplit(&self) -> Vec<Entity> {
        self.unsplit.iter().map(|p| p.entity).collect()
    }
    /// File the triangles of placement `e` (one of [`TilePlan::unsplit`]) by their transformed centres. `world`
    /// holds its prototype's mesh loaded.
    pub fn split(&mut self, world: &World, e: Entity) -> Result<()> {
        let i = self
            .unsplit
            .iter()
            .position(|p| p.entity == e)
            .ok_or_else(|| Error::Invalid(format!("{e:?} is not a placement waiting to be split")))?;
        let p = self.unsplit.remove(i);
        let mesh = loaded(world, p.prototype)?;
        validate_mesh(mesh)?;
        let mut parts = BTreeMap::<(i32, i32), Vec<(usize, Vec<u32>)>>::new();
        for (part, prim) in mesh.primitives.iter().enumerate() {
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
    /// materials lowered. Each tile is written from its own entities moved out of `world` ([`World::split_off`])
    /// and moved back, so nothing is copied; `world` is left as it was (on success) for another plan to write the
    /// same cell. Returns its tiles (none for a cell with nothing to draw or show).
    pub fn write_cell(&mut self, cell: (i32, i32), world: &mut World) -> Result<Vec<WrittenTile>> {
        if !self.unsplit.is_empty() {
            return Err(Error::Invalid(
                "split every placement filed by triangle centre before writing".into(),
            ));
        }
        if !self.written.insert(cell) {
            return Err(Error::Invalid(format!("cell {cell:?} written twice")));
        }
        let Some(content) = self.cells.get(&cell).cloned() else {
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
        let mut part = world.split_off(&closure);
        let tile = self.tile(cell, &mut part, &content);
        world.absorb(part);
        Ok(tile?.into_iter().collect())
    }
    /// One tile of `content` from `w`, the closure of its placements. `w` is left as it was.
    fn tile(&mut self, (col, row): (i32, i32), w: &mut World, content: &Cell) -> Result<Option<WrittenTile>> {
        let mut meshes = Vec::with_capacity(content.placements.len());
        for p in &content.placements {
            let mesh = loaded(w, p.prototype)?;
            validate_mesh(mesh)?;
            meshes.push(match &p.triangles {
                None => None,
                Some(parts) => {
                    let variants = w.get::<MaterialVariants>(p.prototype);
                    if variants.is_some_and(|v| v.0.len() != mesh.primitives.len()) {
                        return Err(Error::Invalid(
                            "material variant count differs from primitive count".into(),
                        ));
                    }
                    let mut chunk = Mesh::default();
                    for (part, tris) in parts {
                        let prim = &mesh.primitives[*part];
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
                        chunk.primitives.push(sub);
                    }
                    Some(chunk)
                }
            });
        }
        // only this tile's placements are placed while it is written; the others' are put back after
        let mut saved = Vec::new();
        for e in self
            .placements
            .iter()
            .copied()
            .filter(|&e| w.is_alive(e))
            .collect::<Vec<_>>()
        {
            saved.push((e, w.remove::<Instance>(e), w.remove::<Transform>(e)));
        }
        let mut spawned = Vec::new();
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut triangles = 0;
        for (p, chunk) in content.placements.iter().zip(meshes) {
            let mesh = chunk.as_ref().map_or_else(|| loaded(w, p.prototype), Ok)?;
            for prim in &mesh.primitives {
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
            let proto = if let Some(mesh) = chunk {
                let e = w.spawn();
                spawned.push(e);
                w.insert(e, mesh);
                if let Some(n) = w.get::<Name>(p.prototype).cloned() {
                    w.insert(e, n);
                }
                e
            } else {
                p.prototype
            };
            w.insert(
                p.entity,
                Instance {
                    prototype: proto,
                    variant: p.variant,
                },
            );
            w.insert(p.entity, p.transform);
            w.insert_opaque(
                p.entity,
                "web.placement_key",
                primase_core::Opaque {
                    version: 1,
                    data: primase_core::OpaqueData::Json(serde_json::json!(p.entity.index())),
                },
            );
        }
        let written: &World = w;
        let render = if triangles == 0 {
            Ok(None)
        } else {
            (|| -> Result<Option<(TileInfo, Vec<u8>)>> {
                if !min.iter().chain(&max).all(|v| v.is_finite()) {
                    return Err(Error::Invalid("tile has no finite geometry".into()));
                }
                let (bytes, report) = primase_gltf::write(
                    written,
                    &primase_gltf::WriteOptions {
                        group_by: primase_gltf::GroupBy::None,
                        base_dir: self.options.base_dir.clone(),
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
                let (bytes, instanced_placements) =
                    instancing::apply(bytes, &transforms, self.options.gpu_instancing)?;
                #[cfg(feature = "ktx2")]
                let bytes = if self.options.ktx2 {
                    self.textures.apply(bytes)?
                } else {
                    bytes
                };
                let bytes = if self.options.compress {
                    compression::apply(bytes)?
                } else {
                    bytes
                };
                Ok(Some(TileInfo {
                    collision: None,
                    content: TileContent::Geometry,
                    col,
                    row,
                    file: String::new(),
                    min,
                    max,
                    triangles,
                    placements: content.placements.len() as u64,
                    instanced_placements,
                    viewer_items: 0,
                    lights: 0,
                    bytes: bytes.len() as u64,
                })
                .map(|info| (info, bytes)))
            })()
        };
        for p in &content.placements {
            w.remove::<Instance>(p.entity);
            w.remove::<Transform>(p.entity);
            w.remove_opaque(p.entity, "web.placement_key");
        }
        for e in spawned {
            w.despawn(e);
        }
        for (e, instance, transform) in saved {
            if let Some(i) = instance {
                w.insert(e, i);
            }
            if let Some(t) = transform {
                w.insert(e, t);
            }
        }
        let Some(mut tile) = self.attach_items((col, row), render?, &content.items, &content.lights)? else {
            return Ok(None);
        };
        tile.info.file = asset_name("tile", col, row, &tile.bytes);
        self.tiles.push(tile.info.clone());
        Ok(Some(tile))
    }
    /// The tile with viewer records `items` added as identity nodes (and their collision as a sidecar) and `lights`
    /// as `KHR_lights_punctual` nodes carrying their world transforms: a metadata-only tile without `render`; `None`
    /// when there is nothing.
    fn attach_items(
        &self,
        (col, row): (i32, i32),
        render: Option<(TileInfo, Vec<u8>)>,
        items: &[usize],
        lights: &[usize],
    ) -> Result<Option<WrittenTile>> {
        use serde_json::{Value, json};
        if items.is_empty() && lights.is_empty() {
            return Ok(render.map(|(info, bytes)| WrittenTile {
                info,
                bytes,
                collision: None,
            }));
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
            collision_name = asset_name("collision", col, row, &bytes);
            Some((collision_name.clone(), bytes))
        };
        let (mut doc, bin) = if let Some((_, bytes)) = &render {
            let chunks = primase_gltf::glb::parse(bytes)?;
            (
                serde_json::from_slice::<Value>(chunks.json)?,
                chunks.bin.unwrap_or(&[]).to_vec(),
            )
        } else {
            (
                json!({"asset":{"version":"2.0","generator":"primase-web"},"scene":0,"scenes":[{"nodes":[]}],"nodes":[]}),
                Vec::new(),
            )
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
            doc["nodes"].as_array_mut().expect("nodes").push(json!({"name":format!("Viewer_{safe}_{i}"),"extras":{"primase":{"web.viewer":{"version":1,"value":value}}}}));
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
            Some((mut info, _)) => {
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
                file: String::new(),
                min,
                max,
                triangles: 0,
                placements: 0,
                instanced_placements: 0,
                viewer_items: items.len() as u64,
                lights: lights.len() as u64,
                bytes: bytes.len() as u64,
            },
        };
        Ok(Some(WrittenTile {
            info,
            bytes,
            collision,
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
        tiles.sort_by_key(|t| (t.col, t.row));
        Ok(Index {
            // 1.1 adds the optional scene lights; a tile set without them stays 1.0
            format_version: if tiles.iter().any(|t| t.lights > 0) {
                "1.1"
            } else {
                "1.0"
            },
            empty_tile_policy: "omit_without_geometry_or_viewer_records; retain_metadata_without_mesh_payload",
            version: 1,
            tile_size: self.options.tile_size,
            tiles,
            skipped_degenerate: self.skipped,
            placements_without_mesh: self.without_mesh,
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
            format_version: "1.0",
            empty_tile_policy: "",
            version: 1,
            tile_size: options.tile_size,
            tiles: Vec::new(),
            skipped_degenerate: 0,
            placements_without_mesh: 0,
        },
        files: BTreeMap::new(),
    };
    for cell in plan.cells() {
        let mut w = world.subset(&world.closure(plan.roots(cell)));
        for tile in plan.write_cell(cell, &mut w)? {
            set.files.insert(tile.info.file.clone(), tile.bytes);
            if let Some((name, bytes)) = tile.collision {
                set.collision_files.insert(name, bytes);
            }
        }
    }
    set.index = plan.finish()?;
    Ok(set)
}
fn asset_name(kind: &str, col: i32, row: i32, bytes: &[u8]) -> String {
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    format!("{kind}_{col}_{row}.{hash}.glb")
}
impl Index {
    /// Publish `tiles.json` in `folder`, after checking that every tile and collision file it names is there with
    /// its length (a driver writes each [`WrittenTile`] first).
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        for (file, bytes) in self.tiles.iter().flat_map(|t| {
            std::iter::once((&t.file, t.bytes)).chain(t.collision.iter().map(|c| (&c.file, c.bytes)))
        }) {
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
        let path = folder.join("tiles.json");
        std::fs::write(&path, serde_json::to_vec_pretty(self)?).map_err(|source| Error::Io { path, source })
    }
}
impl TileSet {
    /// Write only generated tile filenames, then publish tiles.json after every tile
    /// is present. Unrelated files in the destination are retained.
    pub fn write_to(&self, folder: &Path) -> Result<()> {
        let names: BTreeSet<_> = self.index.tiles.iter().map(|t| t.file.as_str()).collect();
        if names.len() != self.index.tiles.len()
            || names.len() != self.files.len()
            || self.index.tiles.iter().any(|t| {
                self.files.get(&t.file).is_none_or(|b| {
                    b.len() as u64 != t.bytes || t.file != asset_name("tile", t.col, t.row, b)
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
                    b.len() as u64 != c.bytes || c.file != asset_name("collision", t.col, t.row, b)
                })
            })
        {
            return Err(Error::Invalid(
                "collision index does not match generated files".into(),
            ));
        }
        std::fs::create_dir_all(folder).map_err(|source| Error::Io {
            path: folder.into(),
            source,
        })?;
        for (name, bytes) in self.files.iter().chain(&self.collision_files) {
            let path = folder.join(name);
            std::fs::write(&path, bytes).map_err(|source| Error::Io { path, source })?;
        }
        let path = folder.join("tiles.json");
        std::fs::write(&path, index_bytes).map_err(|source| Error::Io { path, source })?;
        Ok(())
    }
}
