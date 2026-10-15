//! Web tile sets in the core frame (metres, +Y up). Tiles use east/north ground coordinates.
use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, Name, Primitive, Stream, Topology, Transform, TransformKind,
    World,
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
    pub bytes: u64,
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
#[derive(Clone, Debug)]
struct Placement {
    entity: Entity,
    prototype: Entity,
    variant: u32,
    transform: Transform,
    mesh: Option<Mesh>,
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
fn subset(p: &Primitive, idx: &[u32]) -> Primitive {
    let mut map = BTreeMap::new();
    let mut vertices = Vec::new();
    let indices = idx
        .iter()
        .map(|&i| {
            *map.entry(i).or_insert_with(|| {
                let n = vertices.len() as u32;
                vertices.push(i as usize);
                n
            })
        })
        .collect();
    let streams = p
        .streams
        .iter()
        .map(|(sem, stream)| {
            let stream = match stream {
                Stream::F32x2(v) => Stream::F32x2(vertices.iter().map(|&i| v[i]).collect()),
                Stream::F32x3(v) => Stream::F32x3(vertices.iter().map(|&i| v[i]).collect()),
                Stream::F32x4(v) => Stream::F32x4(vertices.iter().map(|&i| v[i]).collect()),
                Stream::U8x4(v) => Stream::U8x4(vertices.iter().map(|&i| v[i]).collect()),
                Stream::U16x4(v) => Stream::U16x4(vertices.iter().map(|&i| v[i]).collect()),
            };
            (*sem, stream)
        })
        .collect();
    Primitive {
        topology: p.topology,
        streams,
        indices: Some(indices),
        material: p.material,
    }
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
/// Produce one deterministic GLB per occupied tile and an index. Triangle-centre
/// partitioning assigns complete triangles (no clipping), so bounds may exceed the
/// nominal cell. Shared prototypes retain sharing for pivot-filed placements.
/// Degenerate placements are counted and omitted, as in the glTF writer.
pub fn write(world: &World, options: &Options) -> Result<TileSet> {
    #[cfg(feature = "ktx2")]
    let mut texture_encoder = ktx2::TextureEncoder::default();
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
                mesh: None,
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
                    mesh: None,
                },
            );
        }
    }
    for e in &options.split_by_tile {
        if !placements.contains_key(e) {
            return Err(Error::Invalid(format!("split marker {e:?} is not a placement")));
        }
    }
    let mut tiles = BTreeMap::<(i32, i32), Vec<Placement>>::new();
    let mut skipped = 0;
    let mut without_mesh = 0;
    for p in placements.values() {
        if p.transform.classify() == TransformKind::Degenerate {
            skipped += 1;
            continue;
        }
        let Some(mesh) = world.get::<Mesh>(p.prototype) else {
            if world.has::<primase_core::MeshFile>(p.prototype) {
                return Err(Error::Invalid(format!(
                    "prototype {:?} has no loaded mesh",
                    p.prototype
                )));
            }
            without_mesh += 1;
            continue;
        };
        validate_mesh(mesh)?;
        if !options.split_by_tile.contains(&p.entity) {
            let key = coordinate(p.transform.cols[3].map(f64::from), options.tile_size)?;
            tiles.entry(key).or_default().push(p.clone());
            continue;
        }
        let variants = world.get::<MaterialVariants>(p.prototype);
        if variants.is_some_and(|v| v.0.len() != mesh.primitives.len()) {
            return Err(Error::Invalid(
                "material variant count differs from primitive count".into(),
            ));
        }
        let mut chunks = BTreeMap::<(i32, i32), Mesh>::new();
        for (part, prim) in mesh.primitives.iter().enumerate() {
            if prim.topology != Topology::Triangles {
                return Err(Error::Invalid("split-by-tile requires triangle lists".into()));
            }
            let pos = prim
                .positions()
                .ok_or_else(|| Error::Invalid("missing positions".into()))?;
            let idx = indices(prim);
            let mut pieces = BTreeMap::<(i32, i32), Vec<u32>>::new();
            for tri in idx.as_chunks::<3>().0 {
                let points = tri.map(|i| point(p.transform, pos[i as usize]));
                let centre = std::array::from_fn(|k| (points[0][k] + points[1][k] + points[2][k]) / 3.);
                pieces
                    .entry(coordinate(centre, options.tile_size)?)
                    .or_default()
                    .extend_from_slice(tri);
            }
            for (key, idx) in pieces {
                let mut sub = subset(prim, &idx);
                sub.material = variants
                    .and_then(|v| v.0[part].get(&p.variant).copied())
                    .or(prim.material);
                chunks.entry(key).or_default().primitives.push(sub);
            }
        }
        for (key, mesh) in chunks {
            let mut p = p.clone();
            p.mesh = Some(mesh);
            tiles.entry(key).or_default().push(p);
        }
    }
    let mut set = TileSet {
        collision_files: BTreeMap::new(),
        index: Index {
            empty_tile_policy: "omit_without_geometry_or_viewer_records; retain_metadata_without_mesh_payload",
            version: 1,
            tile_size: options.tile_size,
            tiles: Vec::new(),
            skipped_degenerate: skipped,
            placements_without_mesh: without_mesh,
        },
        files: BTreeMap::new(),
    };
    for ((col, row), group) in tiles {
        let mut w = world.clone();
        for &e in placements.keys() {
            w.remove::<Instance>(e);
            w.remove::<Transform>(e);
        }
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let mut triangles = 0;
        for p in &group {
            let mesh = p
                .mesh
                .as_ref()
                .or_else(|| world.get::<Mesh>(p.prototype))
                .ok_or_else(|| Error::Invalid("missing mesh".into()))?;
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
            let proto = if let Some(mesh) = &p.mesh {
                let e = w.spawn();
                w.insert(e, mesh.clone());
                if let Some(n) = world.get::<Name>(p.prototype) {
                    w.insert(e, n.clone());
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
        if triangles == 0 {
            continue;
        }
        if !min.iter().chain(&max).all(|v| v.is_finite()) {
            return Err(Error::Invalid("tile has no finite geometry".into()));
        }
        let (bytes, report) = primase_gltf::write(
            &w,
            &primase_gltf::WriteOptions {
                group_by: primase_gltf::GroupBy::None,
                base_dir: options.base_dir.clone(),
                ..Default::default()
            },
        )?;
        if !report.textures_skipped.is_empty() {
            return Err(Error::Invalid(format!(
                "load textures before web export: {:?}",
                report.textures_skipped
            )));
        }
        let transforms = group.iter().map(|p| (p.entity.index(), p.transform)).collect();
        let (bytes, instanced_placements) = instancing::apply(bytes, &transforms, options.gpu_instancing)?;
        #[cfg(feature = "ktx2")]
        let bytes = if options.ktx2 {
            texture_encoder.apply(bytes)?
        } else {
            bytes
        };
        let bytes = if options.compress {
            compression::apply(bytes)?
        } else {
            bytes
        };
        let file = format!("tile_{col}_{row}.glb");
        set.index.tiles.push(TileInfo {
            collision: None,
            content: TileContent::Geometry,
            col,
            row,
            file: file.clone(),
            min,
            max,
            triangles,
            placements: group.len() as u64,
            instanced_placements,
            viewer_items: 0,
            bytes: bytes.len() as u64,
        });
        set.files.insert(file, bytes);
    }
    append_viewer_items(&mut set, options)?;
    let mut named = BTreeMap::new();
    for tile in &mut set.index.tiles {
        let bytes = set.files.remove(&tile.file).expect("generated tile");
        tile.file = asset_name("tile", tile.col, tile.row, &bytes);
        named.insert(tile.file.clone(), bytes);
    }
    set.files = named;
    Ok(set)
}
fn asset_name(kind: &str, col: i32, row: i32, bytes: &[u8]) -> String {
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    format!("{kind}_{col}_{row}.{hash}.glb")
}
fn append_viewer_items(set: &mut TileSet, options: &Options) -> Result<()> {
    use serde_json::{Value, json};
    let mut groups = BTreeMap::<(i32, i32), Vec<(usize, &ViewerItem)>>::new();
    for (i, item) in options.viewer_items.iter().enumerate() {
        if item.anchor.iter().any(|v| !v.is_finite()) || item.kind.is_empty() {
            return Err(Error::Invalid(
                "viewer item needs a finite anchor and nonempty kind".into(),
            ));
        }
        groups
            .entry(coordinate(item.anchor, options.tile_size)?)
            .or_default()
            .push((i, item));
    }
    for ((col, row), items) in groups {
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
            let descriptor = CollisionFile {
                file: collision_name.clone(),
                bytes: bytes.len() as u64,
                records: collisions.len(),
            };
            set.collision_files.insert(collision_name.clone(), bytes);
            Some(descriptor)
        };
        let file = format!("tile_{col}_{row}.glb");
        let (mut doc, bin) = if let Some(bytes) = set.files.get(&file) {
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
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?;
        if let Some(tile) = set.index.tiles.iter_mut().find(|t| t.col == col && t.row == row) {
            tile.collision = collision;
            tile.viewer_items = items.len() as u64;
            tile.bytes = bytes.len() as u64;
            for k in 0..3 {
                tile.min[k] = tile.min[k].min(min[k]);
                tile.max[k] = tile.max[k].max(max[k]);
            }
        } else {
            set.index.tiles.push(TileInfo {
                collision,
                content: TileContent::MetadataOnly,
                col,
                row,
                file: file.clone(),
                min,
                max,
                triangles: 0,
                placements: 0,
                instanced_placements: 0,
                viewer_items: items.len() as u64,
                bytes: bytes.len() as u64,
            });
        }
        set.files.insert(file, bytes);
    }
    set.index.tiles.sort_by_key(|t| (t.col, t.row));
    Ok(())
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
