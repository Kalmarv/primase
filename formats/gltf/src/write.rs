//! The GLB writer: a [`World`] to one GLB that Blender (and any glTF 2.0 reader) imports.
//!
//! What goes where:
//! - **Prototypes** (entities with a [`Mesh`]) become glTF meshes shared by every placement that uses them with the
//!   same resolved materials; a prototype placed with several [`MaterialVariants`] gets one mesh per distinct
//!   material list, all of them sharing the same accessors.
//! - **Placements** ([`Instance`] + [`Transform`]) become nodes. Regular and mirrored transforms are written as the
//!   node's matrix, exactly (glTF allows a negative determinant and flips the winding itself). A **sheared**
//!   transform has no TRS decomposition, which glTF requires of node matrices, so its linear part is **baked** into a
//!   mesh of its own (positions, normals by the inverse transpose, tangents; winding reversed when it also
//!   mirrors), computed in f64 and rounded once to f32; the node keeps only the translation. Degenerate transforms
//!   are skipped and counted. An entity with a `Mesh` and a `Transform` but no `Instance` is placed as itself.
//! - **Collections**: Blender 4.3's importer makes no collections (every object goes to the active collection;
//!   nodes without a mesh become empties, children are parented). So groups are empty nodes, one per prototype or
//!   per tile ([`GroupBy`]), with `"primase.collection": <name>` in their extras so a Blender-side script can turn
//!   them into real collections. Blender's outliner already folds each group's placements under its empty.
//! - **Materials** come from `pbr.material` ([`PbrMaterial`]); a material entity without one is written as
//!   [`fallback_material`] (light grey, dielectric, rough) and counted. A primitive with no material has none in
//!   glTF (the reader's default material) and is counted.
//! - **Textures**: a `core.texture_file` PNG is embedded as-is (bytes checked to be PNG, identical files embedded
//!   once). In-memory textures take priority and embed their largest level as deterministic RGBA8 PNG.
//!   Normal maps must already be green-up with stored Z; use the explicit normalization pass first.
//!   Unloaded DDS files are skipped and counted; load them with the texture-loading pass.
//! - **Extras**: every component with a JSON form (`core.source`, dialect and opaque JSON components) that the glTF
//!   itself does not express goes into the extras of the object made from its entity, as
//!   `extras.primase["<component name>"] = {"version": n, "value": ...}`: placement -> node, prototype -> mesh,
//!   material -> material, texture -> texture; world resources -> the scene. Blender imports node, mesh, material
//!   and scene extras as custom properties. Components with no JSON form are counted per name in the report, never
//!   dropped silently; a component whose view would lose a value (a non-finite float) is an error.
//!
//! The output is a function of the world: every list is built in entity order or first-use order over entities in
//! order, so the same world gives the same bytes (`tests::output_is_deterministic`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    Component, ComponentJson, Entity, Instance, Light, LightKind, MaterialVariants, Mesh, MeshFile, Name,
    Primitive, Semantic, Stream, Texture, TextureFile, Tile, Topology, Transform, TransformKind, World,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::read::bounds;
use crate::{Error, Result, glb};
use primase_core::json::f32s_value;

#[cfg(not(target_endian = "little"))]
compile_error!("the writer casts vertex data to bytes and assumes little-endian");

/// How placements are grouped under empty nodes (Blender: parent empties; see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GroupBy {
    /// Every placement at the scene root.
    None,
    /// One group per prototype, named like it: all placements of a wall under one empty.
    #[default]
    Prototype,
    /// One group per placement `core.tile` (`tile_<col>_<row>`), `untiled` for placements without one.
    Tile,
}

/// Writer options.
#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    pub group_by: GroupBy,
    /// Relative `core.texture_file` paths are taken from here.
    pub base_dir: PathBuf,
    /// Also write prototypes that nothing places, at the origin.
    pub include_unplaced: bool,
    /// The glTF scene's name.
    pub scene_name: Option<String>,
}

/// What the writer did, with counts of everything it skipped or approximated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteReport {
    pub nodes: u64,
    pub group_nodes: u64,
    /// Lights written (`KHR_lights_punctual`).
    pub lights: u64,
    pub meshes: u64,
    pub primitives: u64,
    pub triangles: u64,
    pub materials: u64,
    pub textures: u64,
    pub images: u64,
    pub image_bytes: u64,
    /// Placements written as nodes (including self-placed and unplaced-at-origin ones).
    pub placements: u64,
    pub placements_mirrored: u64,
    /// Sheared placements baked into their own mesh.
    pub placements_sheared_baked: u64,
    pub placements_degenerate_skipped: u64,
    /// Placements whose prototype has no `core.mesh` (nothing to draw).
    pub placements_without_mesh: u64,
    /// Prototypes nothing places (written at the origin only with `include_unplaced`).
    pub prototypes_unplaced: u64,
    /// Material entities without `pbr.material`, written as [`fallback_material`].
    pub materials_fallback: u64,
    pub primitives_without_material: u64,
    /// Primitives given zero texture coordinates for a set their material samples but they lack.
    pub primitives_texcoords_zero_filled: u64,
    /// Texture slots left empty, by reason (`dds`, `other file format`, ...), counted per texture entity.
    pub textures_skipped: BTreeMap<String, u64>,
    /// Material texture slots left empty because their texture was skipped.
    pub texture_slots_dropped: u64,
    /// Vertex streams not written, by glTF attribute name (skin attributes without a skeleton).
    pub streams_skipped: BTreeMap<String, u64>,
    /// Components with no JSON form, by name: not in the extras.
    pub extras_unencoded: BTreeMap<String, u64>,
    pub glb_bytes: u64,
}

impl fmt::Display for WriteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "glb {} bytes: {} nodes ({} groups), {} meshes, {} primitives, {} triangles, {} materials, {} textures, \
             {} images ({} bytes)",
            self.glb_bytes,
            self.nodes,
            self.group_nodes,
            self.meshes,
            self.primitives,
            self.triangles,
            self.materials,
            self.textures,
            self.images,
            self.image_bytes
        )?;
        writeln!(
            f,
            "placements {}: mirrored {}, sheared baked {}, degenerate skipped {}, without mesh {}; prototypes \
             unplaced {}",
            self.placements,
            self.placements_mirrored,
            self.placements_sheared_baked,
            self.placements_degenerate_skipped,
            self.placements_without_mesh,
            self.prototypes_unplaced
        )?;
        writeln!(
            f,
            "materials fallback {}, primitives without material {}, primitives with zero-filled texcoords {}, \
             texture slots dropped {}",
            self.materials_fallback,
            self.primitives_without_material,
            self.primitives_texcoords_zero_filled,
            self.texture_slots_dropped
        )?;
        writeln!(f, "textures skipped {:?}", self.textures_skipped)?;
        writeln!(f, "streams skipped {:?}", self.streams_skipped)?;
        write!(f, "extras unencoded {:?}", self.extras_unencoded)
    }
}

/// The material written for a material entity without `pbr.material`: light grey (0.8 linear), metallic 0,
/// roughness 0.85 (skate-scene's default), opaque, single-sided. Not glTF's default material, which is fully
/// metallic and renders near-black in Blender without an environment.
pub fn fallback_material() -> PbrMaterial {
    PbrMaterial {
        base_color: [0.8, 0.8, 0.8, 1.0],
        metallic: 0.0,
        roughness: 0.85,
        ..PbrMaterial::default()
    }
}

/// Components the glTF expresses itself, per kind of object; everything else with a JSON form goes to extras.
/// The extension a texture slot's [`UvTransform`] is written as.
const TEXTURE_TRANSFORM: &str = "KHR_texture_transform";
const EMISSIVE_STRENGTH: &str = "KHR_materials_emissive_strength";
/// The extension lights are written as.
const LIGHTS: &str = "KHR_lights_punctual";
const CONSUMED_LIGHT: &[&str] = &[Name::NAME, Transform::NAME, Light::NAME];
const CONSUMED_PLACEMENT: &[&str] = &[Name::NAME, Transform::NAME, Instance::NAME];
const CONSUMED_PROTOTYPE: &[&str] = &[Name::NAME, Mesh::NAME, MaterialVariants::NAME, MeshFile::NAME];
const CONSUMED_MATERIAL: &[&str] = &[Name::NAME, PbrMaterial::NAME];
const CONSUMED_TEXTURE: &[&str] = &[Name::NAME, TextureFile::NAME, Texture::NAME];

/// Write the world as GLB bytes.
pub fn write(world: &World, options: &WriteOptions) -> Result<(Vec<u8>, WriteReport)> {
    let mut w = Writer {
        world,
        options,
        report: WriteReport::default(),
        bin: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
        meshes: Vec::new(),
        materials: Vec::new(),
        textures: Vec::new(),
        samplers: Vec::new(),
        images: Vec::new(),
        proto_accessors: BTreeMap::new(),
        mesh_index: BTreeMap::new(),
        material_index: BTreeMap::new(),
        zero_texcoords: BTreeMap::new(),
        texture_index: BTreeMap::new(),
        image_of_entity: BTreeMap::new(),
        image_by_hash: BTreeMap::new(),
        sampler_index: BTreeMap::new(),
        texture_transform_used: false,
        emissive_strength_used: false,
    };
    let json = w.document()?;
    let text = serde_json::to_vec(&json)?;
    let bytes = glb::assemble(&text, &w.bin)?;
    w.report.glb_bytes = bytes.len() as u64;
    Ok((bytes, w.report))
}

/// Write the world to a GLB file.
pub fn write_file(world: &World, options: &WriteOptions, path: &Path) -> Result<WriteReport> {
    let (bytes, report) = write(world, options)?;
    std::fs::write(path, bytes).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(report)
}

/// A placement to write as one node.
struct Place {
    entity: Entity,
    prototype: Entity,
    variant: u32,
    transform: Transform,
}

/// Group a placement belongs to; the derived order is the order of the group nodes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Prototype(Entity),
    Tile(i32, i32),
    Untiled,
}

/// The accessors of one prototype primitive.
#[derive(Clone)]
struct PrimAccessors {
    attributes: Vec<(String, usize)>,
    indices: Option<usize>,
    mode: Option<u32>,
}

struct Writer<'w> {
    world: &'w World,
    options: &'w WriteOptions,
    report: WriteReport,
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    meshes: Vec<Value>,
    materials: Vec<Value>,
    textures: Vec<Value>,
    samplers: Vec<Value>,
    images: Vec<Value>,
    proto_accessors: BTreeMap<Entity, Vec<PrimAccessors>>,
    mesh_index: BTreeMap<(Entity, Vec<Option<Entity>>), usize>,
    /// Material entity -> glTF index and the texture coordinate sets its written texture slots sample.
    material_index: BTreeMap<Entity, (usize, BTreeSet<u8>)>,
    /// Vertex count -> a zero-filled VEC2 accessor (texture coordinates a primitive lacks).
    zero_texcoords: BTreeMap<usize, usize>,
    texture_index: BTreeMap<(Entity, [Wrap; 2]), usize>,
    image_of_entity: BTreeMap<Entity, Option<usize>>,
    image_by_hash: BTreeMap<[u8; 32], usize>,
    sampler_index: BTreeMap<[Wrap; 2], usize>,
    /// A written texture slot has a transform (`KHR_texture_transform` is used).
    texture_transform_used: bool,
    /// A written material has an emissive strength other than 1 (`KHR_materials_emissive_strength` is used).
    emissive_strength_used: bool,
}

impl Writer<'_> {
    fn document(&mut self) -> Result<Value> {
        let places = self.places()?;
        let mut nodes: Vec<Value> = Vec::new();
        let mut groups: BTreeMap<GroupKey, Vec<usize>> = BTreeMap::new();
        let mut roots: Vec<usize> = Vec::new();
        for place in &places {
            let Some(node) = self.placement_node(place)? else {
                continue;
            };
            nodes.push(node);
            let index = nodes.len() - 1;
            match self.group_key(place) {
                Some(key) => groups.entry(key).or_default().push(index),
                None => roots.push(index),
            }
        }
        // lights: root nodes placed by their transform, the light in KHR_lights_punctual
        let mut lights = Vec::new();
        let world = self.world;
        for (e, l) in world.iter::<Light>() {
            let mut node = Map::new();
            node.insert("name".into(), Value::String(self.name_of(e, "light")));
            if let Some(t) = world.get::<Transform>(e)
                && *t != Transform::IDENTITY
            {
                let c = t.cols;
                let m = [
                    c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1],
                    c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                ];
                if m.iter().any(|x| !x.is_finite()) {
                    return Err(Error::World {
                        entity: e,
                        detail: "non-finite transform".into(),
                    });
                }
                node.insert("matrix".into(), f32s_value(&m));
            }
            node.insert("extensions".into(), json!({ LIGHTS: {"light": lights.len()} }));
            if let Some(extras) = self.extras(e, CONSUMED_LIGHT)? {
                node.insert("extras".into(), extras);
            }
            lights.push(self.light(e, l)?);
            nodes.push(Value::Object(node));
            roots.push(nodes.len() - 1);
        }
        self.report.lights = lights.len() as u64;
        self.report.nodes = nodes.len() as u64;
        for (key, children) in groups {
            let name = match key {
                GroupKey::Prototype(p) => self.name_of(p, "prototype"),
                GroupKey::Tile(c, r) => format!("tile_{c}_{r}"),
                GroupKey::Untiled => "untiled".to_owned(),
            };
            let kind = match key {
                GroupKey::Prototype(_) => "prototype",
                _ => "tile",
            };
            nodes.push(json!({
                "name": name,
                "children": children,
                "extras": {"primase.collection": name, "primase.group": kind},
            }));
            roots.push(nodes.len() - 1);
            self.report.group_nodes += 1;
        }
        self.report.nodes = nodes.len() as u64;

        let mut scene = Map::new();
        if let Some(name) = &self.options.scene_name {
            scene.insert("name".into(), Value::String(name.clone()));
        }
        scene.insert("nodes".into(), json!(roots));
        let resources = self.world.resources_json().map_err(|e| Error::Invalid {
            item: format!("resource {}", e.component),
            detail: e.message,
        })?;
        if let Some(extras) = self.primase_extras(resources, &[]) {
            scene.insert("extras".into(), extras);
        }

        let mut root = Map::new();
        root.insert(
            "asset".into(),
            json!({"version": "2.0", "generator": concat!("primase-gltf ", env!("CARGO_PKG_VERSION"))}),
        );
        let mut used = Vec::new();
        if self.texture_transform_used {
            used.push(TEXTURE_TRANSFORM);
        }
        if self.emissive_strength_used {
            used.push(EMISSIVE_STRENGTH);
        }
        if !lights.is_empty() {
            used.push(LIGHTS);
            root.insert("extensions".into(), json!({ LIGHTS: {"lights": lights} }));
        }
        if !used.is_empty() {
            root.insert("extensionsUsed".into(), json!(used));
        }
        root.insert("scene".into(), json!(0));
        root.insert("scenes".into(), json!([Value::Object(scene)]));
        root.insert("nodes".into(), Value::Array(nodes));
        let lists = [
            ("meshes", std::mem::take(&mut self.meshes)),
            ("materials", std::mem::take(&mut self.materials)),
            ("textures", std::mem::take(&mut self.textures)),
            ("samplers", std::mem::take(&mut self.samplers)),
            ("images", std::mem::take(&mut self.images)),
            ("accessors", std::mem::take(&mut self.accessors)),
            ("bufferViews", std::mem::take(&mut self.views)),
        ];
        for (key, list) in lists {
            if !list.is_empty() {
                root.insert(key.into(), Value::Array(list));
            }
        }
        if !self.bin.is_empty() {
            root.insert("buffers".into(), json!([{"byteLength": self.bin.len()}]));
        }
        Ok(Value::Object(root))
    }

    /// Every placement to write, in entity order: instances, self-placed meshes, and (with `include_unplaced`)
    /// prototypes nothing places.
    fn places(&mut self) -> Result<Vec<Place>> {
        let world = self.world;
        let mut places = Vec::new();
        let mut placed = BTreeSet::new();
        for (e, inst) in world.iter::<Instance>() {
            let transform = *world.get::<Transform>(e).ok_or_else(|| Error::World {
                entity: e,
                detail: "placement without core.transform".into(),
            })?;
            placed.insert(inst.prototype);
            places.push(Place {
                entity: e,
                prototype: inst.prototype,
                variant: inst.variant,
                transform,
            });
        }
        for (e, _) in world.iter::<Mesh>() {
            if world.has::<Instance>(e) {
                continue;
            }
            if let Some(&transform) = world.get::<Transform>(e) {
                places.push(Place {
                    entity: e,
                    prototype: e,
                    variant: 0,
                    transform,
                });
            } else if !placed.contains(&e) {
                self.report.prototypes_unplaced += 1;
                if self.options.include_unplaced {
                    places.push(Place {
                        entity: e,
                        prototype: e,
                        variant: 0,
                        transform: Transform::IDENTITY,
                    });
                }
            }
        }
        places.sort_by_key(|p| p.entity);
        Ok(places)
    }

    fn group_key(&self, place: &Place) -> Option<GroupKey> {
        match self.options.group_by {
            GroupBy::None => None,
            GroupBy::Prototype => Some(GroupKey::Prototype(place.prototype)),
            GroupBy::Tile => Some(match self.world.get::<Tile>(place.entity) {
                Some(t) => GroupKey::Tile(t.col, t.row),
                None => GroupKey::Untiled,
            }),
        }
    }

    fn name_of(&self, e: Entity, what: &str) -> String {
        match self.world.get::<Name>(e) {
            Some(Name(n)) => n.clone(),
            None => format!("{what}_e{}", e.index()),
        }
    }

    fn placement_node(&mut self, place: &Place) -> Result<Option<Value>> {
        let world = self.world;
        let Some(mesh) = world.get::<Mesh>(place.prototype) else {
            self.report.placements_without_mesh += 1;
            return Ok(None);
        };
        let kind = place.transform.classify();
        let materials = self.resolve_materials(place, mesh)?;
        let mut node = Map::new();
        let name = match world.get::<Name>(place.entity) {
            Some(Name(n)) => n.clone(),
            None => self.name_of(place.prototype, "prototype"),
        };
        node.insert("name".into(), Value::String(name));
        let c = place.transform.cols;
        match kind {
            TransformKind::Degenerate => {
                self.report.placements_degenerate_skipped += 1;
                return Ok(None);
            }
            TransformKind::Sheared => {
                let mesh_index = self.baked_mesh(place, mesh, &materials, kind)?;
                node.insert("mesh".into(), json!(mesh_index));
                if c[3] != [0.0; 3] {
                    node.insert("translation".into(), f32s_value(&c[3]));
                }
                self.report.placements_sheared_baked += 1;
            }
            TransformKind::Regular | TransformKind::Mirrored => {
                let mesh_index = self.shared_mesh(place.prototype, mesh, materials, place.variant)?;
                node.insert("mesh".into(), json!(mesh_index));
                if kind == TransformKind::Mirrored {
                    self.report.placements_mirrored += 1;
                }
                if c[..3] == Transform::IDENTITY.cols[..3] {
                    if c[3] != [0.0; 3] {
                        node.insert("translation".into(), f32s_value(&c[3]));
                    }
                } else {
                    let m = [
                        c[0][0], c[0][1], c[0][2], 0.0, c[1][0], c[1][1], c[1][2], 0.0, c[2][0], c[2][1],
                        c[2][2], 0.0, c[3][0], c[3][1], c[3][2], 1.0,
                    ];
                    if m.iter().any(|x| !x.is_finite()) {
                        return Err(Error::World {
                            entity: place.entity,
                            detail: "non-finite transform".into(),
                        });
                    }
                    node.insert("matrix".into(), f32s_value(&m));
                }
            }
        }
        if let Some(extras) = self.extras(place.entity, CONSUMED_PLACEMENT)? {
            node.insert("extras".into(), extras);
        }
        self.report.placements += 1;
        Ok(Some(Value::Object(node)))
    }

    /// The material of each primitive for this placement's variant.
    fn resolve_materials(&self, place: &Place, mesh: &Mesh) -> Result<Vec<Option<Entity>>> {
        let variants = self.world.get::<MaterialVariants>(place.prototype);
        if let Some(v) = variants
            && v.0.len() != mesh.primitives.len()
        {
            return Err(Error::World {
                entity: place.prototype,
                detail: format!(
                    "core.material_variants has {} entries for {} primitives",
                    v.0.len(),
                    mesh.primitives.len()
                ),
            });
        }
        Ok(mesh
            .primitives
            .iter()
            .enumerate()
            .map(|(i, p)| {
                variants
                    .and_then(|v| v.0[i].get(&place.variant).copied())
                    .or(p.material)
            })
            .collect())
    }

    /// The glTF mesh of a prototype with these materials, made on first use.
    fn shared_mesh(
        &mut self,
        prototype: Entity,
        mesh: &Mesh,
        materials: Vec<Option<Entity>>,
        variant: u32,
    ) -> Result<usize> {
        let key = (prototype, materials);
        if let Some(&i) = self.mesh_index.get(&key) {
            return Ok(i);
        }
        let accessors = self.prototype_accessors(prototype, mesh)?;
        let base: Vec<Option<Entity>> = mesh.primitives.iter().map(|p| p.material).collect();
        let mut name = self.name_of(prototype, "prototype");
        if key.1 != base {
            name = format!("{name}.v{variant}");
        }
        let index = self.push_mesh(prototype, name, &accessors, &key.1, mesh)?;
        self.mesh_index.insert(key, index);
        Ok(index)
    }

    /// A mesh of the prototype with a sheared placement's linear part applied to its vertices.
    fn baked_mesh(
        &mut self,
        place: &Place,
        mesh: &Mesh,
        materials: &[Option<Entity>],
        kind: TransformKind,
    ) -> Result<usize> {
        debug_assert_eq!(kind, TransformKind::Sheared);
        let shared = self.prototype_accessors(place.prototype, mesh)?;
        let lin = Linear::new(&place.transform);
        let mut baked = Vec::with_capacity(mesh.primitives.len());
        for (prim, acc) in mesh.primitives.iter().zip(&shared) {
            let mut acc = acc.clone();
            for (name, index) in &mut acc.attributes {
                *index = match (name.as_str(), &prim.streams) {
                    ("POSITION", s) => match s.get(&Semantic::Position) {
                        Some(Stream::F32x3(p)) => {
                            let p: Vec<[f32; 3]> = p.iter().map(|&v| lin.point(v)).collect();
                            self.positions_accessor(&p)
                        }
                        _ => unreachable!("validated by prototype_accessors"),
                    },
                    ("NORMAL", s) => match s.get(&Semantic::Normal) {
                        Some(Stream::F32x3(n)) => {
                            let n: Vec<[f32; 3]> = n.iter().map(|&v| lin.normal(v)).collect();
                            self.float_accessor(bytemuck::cast_slice(&n), n.len(), "VEC3", None)
                        }
                        _ => unreachable!("validated by prototype_accessors"),
                    },
                    ("TANGENT", s) => match s.get(&Semantic::Tangent) {
                        Some(Stream::F32x4(t)) => {
                            let t: Vec<[f32; 4]> = t.iter().map(|&v| lin.tangent(v)).collect();
                            self.float_accessor(bytemuck::cast_slice(&t), t.len(), "VEC4", None)
                        }
                        _ => unreachable!("validated by prototype_accessors"),
                    },
                    _ => *index,
                };
            }
            if lin.det < 0.0 && prim.topology == Topology::Triangles {
                let mut idx: Vec<u32> = match &prim.indices {
                    Some(i) => i.clone(),
                    None => (0..prim.vertex_count() as u32).collect(),
                };
                for t in idx.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
                acc.indices = Some(self.indices_accessor(&idx, prim.vertex_count()));
            }
            baked.push(acc);
        }
        let name = format!(
            "{}.baked.e{}",
            self.name_of(place.prototype, "prototype"),
            place.entity.index()
        );
        self.push_mesh(place.prototype, name, &baked, materials, mesh)
    }

    fn push_mesh(
        &mut self,
        prototype: Entity,
        name: String,
        accessors: &[PrimAccessors],
        materials: &[Option<Entity>],
        mesh: &Mesh,
    ) -> Result<usize> {
        let mut prims = Vec::with_capacity(accessors.len());
        for ((acc, material), prim) in accessors.iter().zip(materials).zip(&mesh.primitives) {
            let mut p = Map::new();
            let mut attrs: Map<String, Value> = acc
                .attributes
                .iter()
                .map(|(n, i)| (n.clone(), json!(i)))
                .collect();
            let material_index = match material {
                Some(m) => {
                    let (index, sets) = self.material(*m)?;
                    // glTF requires every texture coordinate set a material samples; skate-scene's rule for an
                    // absent one is zeros, so that is what the primitive gets.
                    let mut filled = false;
                    for set in sets {
                        let key = format!("TEXCOORD_{set}");
                        if !attrs.contains_key(&key) {
                            let zeros = self.zero_texcoords(prim.vertex_count());
                            attrs.insert(key, json!(zeros));
                            filled = true;
                        }
                    }
                    self.report.primitives_texcoords_zero_filled += u64::from(filled);
                    Some(index)
                }
                None => {
                    self.report.primitives_without_material += 1;
                    None
                }
            };
            p.insert("attributes".into(), Value::Object(attrs));
            if let Some(i) = acc.indices {
                p.insert("indices".into(), json!(i));
            }
            if let Some(index) = material_index {
                p.insert("material".into(), json!(index));
            }
            if let Some(mode) = acc.mode {
                p.insert("mode".into(), json!(mode));
            }
            prims.push(Value::Object(p));
            self.report.primitives += 1;
            if prim.topology == Topology::Triangles {
                self.report.triangles +=
                    (prim.indices.as_ref().map_or(prim.vertex_count(), Vec::len) / 3) as u64;
            }
        }
        let mut m = Map::new();
        m.insert("name".into(), Value::String(name));
        m.insert("primitives".into(), Value::Array(prims));
        if let Some(extras) = self.extras(prototype, CONSUMED_PROTOTYPE)? {
            m.insert("extras".into(), extras);
        }
        self.meshes.push(Value::Object(m));
        self.report.meshes += 1;
        Ok(self.meshes.len() - 1)
    }

    /// Accessors of every primitive of a prototype, written once and shared by all its meshes.
    fn prototype_accessors(&mut self, prototype: Entity, mesh: &Mesh) -> Result<Vec<PrimAccessors>> {
        if let Some(a) = self.proto_accessors.get(&prototype) {
            return Ok(a.clone());
        }
        if mesh.primitives.is_empty() {
            return Err(Error::World {
                entity: prototype,
                detail: "core.mesh without primitives".into(),
            });
        }
        let mut out = Vec::with_capacity(mesh.primitives.len());
        for (i, prim) in mesh.primitives.iter().enumerate() {
            let a = self.primitive_accessors(prim).map_err(|detail| Error::World {
                entity: prototype,
                detail: format!("primitive {i}: {detail}"),
            })?;
            out.push(a);
        }
        self.proto_accessors.insert(prototype, out.clone());
        Ok(out)
    }

    fn primitive_accessors(&mut self, prim: &Primitive) -> std::result::Result<PrimAccessors, String> {
        let count = prim.vertex_count();
        if !matches!(prim.streams.get(&Semantic::Position), Some(Stream::F32x3(_))) {
            return Err("no f32x3 Position stream".into());
        }
        if count == 0 {
            return Err("no vertices".into());
        }
        for (s, v) in &prim.streams {
            if v.len() != count {
                return Err(format!("{s:?} has {} values, Position {count}", v.len()));
            }
            let finite = match v {
                Stream::F32x2(x) => x.iter().flatten().all(|f| f.is_finite()),
                Stream::F32x3(x) => x.iter().flatten().all(|f| f.is_finite()),
                Stream::F32x4(x) => x.iter().flatten().all(|f| f.is_finite()),
                Stream::U8x4(_) | Stream::U16x4(_) => true,
            };
            if !finite {
                return Err(format!("{s:?} has a non-finite value"));
            }
        }
        let n = prim.indices.as_ref().map_or(count, Vec::len);
        let per = match prim.topology {
            Topology::Triangles => 3,
            Topology::Lines => 2,
            Topology::Points => 1,
        };
        if !n.is_multiple_of(per) || n == 0 {
            return Err(format!("{n} vertices is not whole {:?}", prim.topology));
        }
        if let Some(idx) = &prim.indices
            && let Some(bad) = idx.iter().find(|&&i| i as usize >= count)
        {
            return Err(format!("index {bad} of {count} vertices"));
        }
        let mut attributes = Vec::new();
        for (semantic, stream) in &prim.streams {
            let (name, index) = match (semantic, stream) {
                (Semantic::Position, Stream::F32x3(p)) => ("POSITION".to_owned(), self.positions_accessor(p)),
                (Semantic::Normal, Stream::F32x3(v)) => (
                    "NORMAL".to_owned(),
                    self.float_accessor(bytemuck::cast_slice(v), count, "VEC3", None),
                ),
                (Semantic::Tangent, Stream::F32x4(v)) => (
                    "TANGENT".to_owned(),
                    self.float_accessor(bytemuck::cast_slice(v), count, "VEC4", None),
                ),
                (Semantic::TexCoord(n), Stream::F32x2(v)) => (
                    format!("TEXCOORD_{n}"),
                    self.float_accessor(bytemuck::cast_slice(v), count, "VEC2", None),
                ),
                (Semantic::Color(n), Stream::F32x3(v)) => (
                    format!("COLOR_{n}"),
                    self.float_accessor(bytemuck::cast_slice(v), count, "VEC3", None),
                ),
                (Semantic::Color(n), Stream::F32x4(v)) => (
                    format!("COLOR_{n}"),
                    self.float_accessor(bytemuck::cast_slice(v), count, "VEC4", None),
                ),
                (Semantic::Color(n), Stream::U8x4(v)) => (
                    format!("COLOR_{n}"),
                    self.unorm_accessor(bytemuck::cast_slice(v), count, 5121),
                ),
                (Semantic::Color(n), Stream::U16x4(v)) => (
                    format!("COLOR_{n}"),
                    self.unorm_accessor(bytemuck::cast_slice(v), count, 5123),
                ),
                (Semantic::Joints(n), _) | (Semantic::Weights(n), _) => {
                    let name = match semantic {
                        Semantic::Joints(_) => format!("JOINTS_{n}"),
                        _ => format!("WEIGHTS_{n}"),
                    };
                    *self.report.streams_skipped.entry(name).or_default() += 1;
                    continue;
                }
                (s, v) => return Err(format!("{s:?} stream of {} is not writable", stream_kind(v))),
            };
            attributes.push((name, index));
        }
        let indices = prim.indices.as_ref().map(|idx| self.indices_accessor(idx, count));
        let mode = match prim.topology {
            Topology::Triangles => None,
            Topology::Lines => Some(1),
            Topology::Points => Some(0),
        };
        Ok(PrimAccessors {
            attributes,
            indices,
            mode,
        })
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
        let mut v = Map::new();
        v.insert("buffer".into(), json!(0));
        v.insert("byteOffset".into(), json!(self.bin.len()));
        v.insert("byteLength".into(), json!(bytes.len()));
        if let Some(t) = target {
            v.insert("target".into(), json!(t));
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(Value::Object(v));
        self.views.len() - 1
    }

    fn positions_accessor(&mut self, p: &[[f32; 3]]) -> usize {
        let (lo, hi) = bounds(p).expect("callers reject empty primitives");
        self.float_accessor(bytemuck::cast_slice(p), p.len(), "VEC3", Some((&lo, &hi)))
    }

    fn float_accessor(
        &mut self,
        bytes: &[u8],
        count: usize,
        kind: &str,
        minmax: Option<(&[f32], &[f32])>,
    ) -> usize {
        let view = self.view(bytes, Some(34962));
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(5126));
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!(kind));
        if let Some((lo, hi)) = minmax {
            a.insert("min".into(), f32s_value(lo));
            a.insert("max".into(), f32s_value(hi));
        }
        self.accessors.push(Value::Object(a));
        self.accessors.len() - 1
    }

    fn unorm_accessor(&mut self, bytes: &[u8], count: usize, component: u32) -> usize {
        let view = self.view(bytes, Some(34962));
        self.accessors.push(json!({
            "bufferView": view, "componentType": component, "normalized": true, "count": count, "type": "VEC4",
        }));
        self.accessors.len() - 1
    }

    /// Indices as unsigned short when every index is below 65535 (the primitive-restart value glTF forbids),
    /// else unsigned int.
    fn indices_accessor(&mut self, idx: &[u32], vertex_count: usize) -> usize {
        let (view, component) = if vertex_count <= 65535 {
            let short: Vec<u16> = idx.iter().map(|&i| i as u16).collect();
            (self.view(bytemuck::cast_slice(&short), Some(34963)), 5123)
        } else {
            (self.view(bytemuck::cast_slice(idx), Some(34963)), 5125)
        };
        self.accessors.push(json!({
            "bufferView": view, "componentType": component, "count": idx.len(), "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }

    fn zero_texcoords(&mut self, count: usize) -> usize {
        if let Some(&a) = self.zero_texcoords.get(&count) {
            return a;
        }
        let a = self.float_accessor(bytemuck::cast_slice(&vec![[0f32; 2]; count]), count, "VEC2", None);
        self.zero_texcoords.insert(count, a);
        a
    }

    /// The glTF material of an entity, and the texture coordinate sets it samples.
    fn material(&mut self, e: Entity) -> Result<(usize, BTreeSet<u8>)> {
        if let Some(m) = self.material_index.get(&e) {
            return Ok(m.clone());
        }
        let mut sets = BTreeSet::new();
        let pbr = match self.world.get::<PbrMaterial>(e) {
            Some(m) => m.clone(),
            None => {
                self.report.materials_fallback += 1;
                fallback_material()
            }
        };
        let mut m = Map::new();
        m.insert("name".into(), Value::String(self.name_of(e, "material")));
        let mut pmr = Map::new();
        if pbr.base_color != [1.0; 4] {
            pmr.insert("baseColorFactor".into(), self.factors(e, &pbr.base_color)?);
        }
        if let Some(t) = self.texture_info(pbr.base_color_texture, &mut sets)? {
            pmr.insert("baseColorTexture".into(), t);
        }
        if pbr.metallic != 1.0 {
            pmr.insert(
                "metallicFactor".into(),
                self.factors(e, &[pbr.metallic])?[0].clone(),
            );
        }
        if pbr.roughness != 1.0 {
            pmr.insert(
                "roughnessFactor".into(),
                self.factors(e, &[pbr.roughness])?[0].clone(),
            );
        }
        if let Some(t) = self.texture_info(pbr.metallic_roughness_texture, &mut sets)? {
            pmr.insert("metallicRoughnessTexture".into(), t);
        }
        if !pmr.is_empty() {
            m.insert("pbrMetallicRoughness".into(), Value::Object(pmr));
        }
        if let Some(Value::Object(mut t)) = self.texture_info(pbr.normal_texture, &mut sets)? {
            if pbr.normal_scale != 1.0 {
                t.insert("scale".into(), self.factors(e, &[pbr.normal_scale])?[0].clone());
            }
            m.insert("normalTexture".into(), Value::Object(t));
        }
        if let Some(Value::Object(mut t)) = self.texture_info(pbr.occlusion_texture, &mut sets)? {
            if pbr.occlusion_strength != 1.0 {
                t.insert(
                    "strength".into(),
                    self.factors(e, &[pbr.occlusion_strength])?[0].clone(),
                );
            }
            m.insert("occlusionTexture".into(), Value::Object(t));
        }
        if let Some(t) = self.texture_info(pbr.emissive_texture, &mut sets)? {
            m.insert("emissiveTexture".into(), t);
        }
        if pbr.emissive != [0.0; 3] {
            m.insert("emissiveFactor".into(), self.factors(e, &pbr.emissive)?);
        }
        if pbr.emissive_strength != 1.0 {
            let strength = self.factors(e, &[pbr.emissive_strength])?[0].clone();
            m.insert(
                "extensions".into(),
                json!({ EMISSIVE_STRENGTH: {"emissiveStrength": strength} }),
            );
            self.emissive_strength_used = true;
        }
        match pbr.alpha_mode {
            AlphaMode::Opaque => {}
            AlphaMode::Mask { cutoff } => {
                m.insert("alphaMode".into(), json!("MASK"));
                if cutoff != 0.5 {
                    m.insert("alphaCutoff".into(), self.factors(e, &[cutoff])?[0].clone());
                }
            }
            AlphaMode::Blend => {
                m.insert("alphaMode".into(), json!("BLEND"));
            }
        }
        if pbr.double_sided {
            m.insert("doubleSided".into(), json!(true));
        }
        if let Some(extras) = self.extras(e, CONSUMED_MATERIAL)? {
            m.insert("extras".into(), extras);
        }
        self.materials.push(Value::Object(m));
        self.report.materials += 1;
        let i = self.materials.len() - 1;
        self.material_index.insert(e, (i, sets.clone()));
        Ok((i, sets))
    }

    /// A light as `KHR_lights_punctual` writes it, named.
    fn light(&self, e: Entity, l: &Light) -> Result<Value> {
        let mut o = Map::new();
        o.insert("name".into(), Value::String(self.name_of(e, "light")));
        let Value::Object(fields) = punctual_light(l).map_err(|detail| Error::World { entity: e, detail })?
        else {
            unreachable!("punctual_light returns an object")
        };
        o.extend(fields);
        Ok(Value::Object(o))
    }

    fn factors(&self, e: Entity, xs: &[f32]) -> Result<Value> {
        if xs.iter().any(|x| !x.is_finite()) {
            return Err(Error::World {
                entity: e,
                detail: format!("non-finite material factor {xs:?}"),
            });
        }
        Ok(f32s_value(xs))
    }

    /// `{"index", "texCoord"}` (and its transform as `KHR_texture_transform`) for a texture slot, or `None` when its
    /// texture was skipped; adds the slot's texture coordinate set to `sets`.
    fn texture_info(&mut self, r: Option<TextureRef>, sets: &mut BTreeSet<u8>) -> Result<Option<Value>> {
        let Some(r) = r else { return Ok(None) };
        let Some(index) = self.texture(r)? else {
            self.report.texture_slots_dropped += 1;
            return Ok(None);
        };
        sets.insert(r.texcoord);
        let mut t = Map::new();
        t.insert("index".into(), json!(index));
        if r.texcoord != 0 {
            t.insert("texCoord".into(), json!(r.texcoord));
        }
        if let Some(x) = r.transform.filter(|x| *x != UvTransform::IDENTITY) {
            let mut k = Map::new();
            if x.offset != [0.0; 2] {
                k.insert("offset".into(), self.factors(r.texture, &x.offset)?);
            }
            if x.rotation != 0.0 {
                k.insert(
                    "rotation".into(),
                    self.factors(r.texture, &[x.rotation])?[0].take(),
                );
            }
            if x.scale != [1.0; 2] {
                k.insert("scale".into(), self.factors(r.texture, &x.scale)?);
            }
            t.insert(
                "extensions".into(),
                json!({ TEXTURE_TRANSFORM: Value::Object(k) }),
            );
            self.texture_transform_used = true;
        }
        Ok(Some(Value::Object(t)))
    }

    fn texture(&mut self, r: TextureRef) -> Result<Option<usize>> {
        let key = (r.texture, r.wrap);
        if let Some(&i) = self.texture_index.get(&key) {
            return Ok(Some(i));
        }
        let Some(image) = self.image(r.texture)? else {
            return Ok(None);
        };
        let mut t = Map::new();
        if r.wrap != [Wrap::Repeat; 2] {
            let next = self.samplers.len();
            let sampler = *self.sampler_index.entry(r.wrap).or_insert(next);
            if sampler == next {
                self.samplers
                    .push(json!({"wrapS": wrap_code(r.wrap[0]), "wrapT": wrap_code(r.wrap[1])}));
            }
            t.insert("sampler".into(), json!(sampler));
        }
        t.insert("source".into(), json!(image));
        t.insert("name".into(), Value::String(self.name_of(r.texture, "texture")));
        if let Some(extras) = self.extras(r.texture, CONSUMED_TEXTURE)? {
            t.insert("extras".into(), extras);
        }
        self.textures.push(Value::Object(t));
        self.report.textures += 1;
        let i = self.textures.len() - 1;
        self.texture_index.insert(key, i);
        Ok(Some(i))
    }

    /// The image of a texture entity: canonical PNG for loaded pixels, otherwise a PNG file as-is.
    fn image(&mut self, e: Entity) -> Result<Option<usize>> {
        if let Some(&i) = self.image_of_entity.get(&e) {
            return Ok(i);
        }
        let skip = |w: &mut Self, reason: &str| {
            *w.report.textures_skipped.entry(reason.to_owned()).or_default() += 1;
            w.image_of_entity.insert(e, None);
            Ok(None)
        };
        let (bytes, image_name) = if let Some(texture) = self.world.get::<Texture>(e) {
            if texture.role == primase_core::TexRole::Normal
                && texture.normal != Some(primase_core::NormalConvention::GLTF)
            {
                return Err(Error::World {
                    entity: e,
                    detail:
                        "normal texture requires tangent/bitangent basis, X in red, green up and stored Z; run normalize-normals first"
                            .into(),
                });
            }
            let bytes = primase_image::texture_level_png(texture, 0).map_err(|error| Error::World {
                entity: e,
                detail: error.to_string(),
            })?;
            (bytes, self.name_of(e, "image"))
        } else {
            let file = self.world.get::<TextureFile>(e).ok_or_else(|| Error::World {
                entity: e,
                detail: "texture entity with neither core.texture_file nor core.texture".into(),
            })?;
            let ext = file
                .path
                .extension()
                .and_then(|x| x.to_str())
                .map(str::to_ascii_lowercase);
            match ext.as_deref() {
                Some("png") => {}
                Some("dds") => return skip(self, "dds"),
                _ => return skip(self, "other file format"),
            }
            let path = self.options.base_dir.join(&file.path);
            let bytes = std::fs::read(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            check_png(&bytes).map_err(|detail| Error::World {
                entity: e,
                detail: format!("{}: {detail}", path.display()),
            })?;
            (
                bytes,
                file.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image")
                    .to_owned(),
            )
        };
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let index = match self.image_by_hash.get(&hash) {
            Some(&i) => i,
            None => {
                let view = self.view(&bytes, None);
                self.images
                    .push(json!({"name": image_name, "mimeType": "image/png", "bufferView": view}));
                self.report.images += 1;
                self.report.image_bytes += bytes.len() as u64;
                let i = self.images.len() - 1;
                self.image_by_hash.insert(hash, i);
                i
            }
        };
        self.image_of_entity.insert(e, Some(index));
        Ok(Some(index))
    }

    /// The extras of an entity: its components with a JSON form, minus what the glTF object already expresses, as
    /// `{"primase": {"<component name>": {"version": n, "value": ...}}}`. Components without one are counted in
    /// the report; a view that would lose a value (a non-finite float) is an error.
    fn extras(&mut self, e: Entity, consumed: &[&str]) -> Result<Option<Value>> {
        let shown = self.world.components_json(e).map_err(|err| Error::World {
            entity: e,
            detail: err.to_string(),
        })?;
        Ok(self.primase_extras(shown, consumed))
    }

    fn primase_extras(&mut self, shown: Vec<ComponentJson<'_>>, consumed: &[&str]) -> Option<Value> {
        let mut map = Map::new();
        for c in shown {
            if consumed.contains(&c.name) {
                continue;
            }
            match c.value {
                Some(v) => {
                    map.insert(c.name.to_owned(), json!({"version": c.version, "value": v}));
                }
                None => *self.report.extras_unencoded.entry(c.name.to_owned()).or_default() += 1,
            }
        }
        (!map.is_empty()).then(|| json!({ "primase": map }))
    }
}

fn wrap_code(w: Wrap) -> u32 {
    match w {
        Wrap::Repeat => 10497,
        Wrap::Clamp => 33071,
        Wrap::Mirror => 33648,
    }
}

fn stream_kind(s: &Stream) -> &'static str {
    match s {
        Stream::F32x2(_) => "f32x2",
        Stream::F32x3(_) => "f32x3",
        Stream::F32x4(_) => "f32x4",
        Stream::U8x4(_) => "u8x4",
        Stream::U16x4(_) => "u16x4",
    }
}

/// The PNG signature and a well-formed IHDR chunk first (PNG §5.2-5.3, §11.2.2): enough to refuse a file that is
/// not a PNG, without decoding it.
fn check_png(b: &[u8]) -> std::result::Result<(), String> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if b.len() < 33 || b[..8] != SIG {
        return Err("not a PNG (signature)".into());
    }
    if b[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return Err("PNG does not start with a 13-byte IHDR".into());
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    if w == 0 || h == 0 {
        return Err(format!("PNG of {w}x{h}"));
    }
    Ok(())
}

/// A transform's linear part, for baking sheared placements. f64 throughout, rounded to f32 once per value.
struct Linear {
    c: [[f64; 3]; 3],
    /// Columns of the inverse transpose times the determinant: `[c1 x c2, c2 x c0, c0 x c1]`.
    cof: [[f64; 3]; 3],
    det: f64,
}

impl Linear {
    fn new(t: &Transform) -> Linear {
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| t.cols[i].map(f64::from));
        let cof = [cross(c[1], c[2]), cross(c[2], c[0]), cross(c[0], c[1])];
        let det = dot(c[0], cof[0]);
        Linear { c, cof, det }
    }

    fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
    }

    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        Self::apply(&self.c, p.map(f64::from)).map(|x| x as f32)
    }

    /// `M^-T n`, normalized (the determinant's sign kept so normals stay outward under a mirror).
    fn normal(&self, n: [f32; 3]) -> [f32; 3] {
        let v = Self::apply(&self.cof, n.map(f64::from)).map(|x| x * self.det.signum());
        normalized(v)
    }

    /// `M t` normalized; w (the bitangent sign) flips under a mirror.
    fn tangent(&self, t: [f32; 4]) -> [f32; 4] {
        let v = normalized(Self::apply(&self.c, [t[0], t[1], t[2]].map(f64::from)));
        let w = if self.det < 0.0 { -t[3] } else { t[3] };
        [v[0], v[1], v[2], w]
    }
}

fn normalized(v: [f64; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len == 0.0 {
        return [0.0; 3];
    }
    v.map(|x| (x / len) as f32)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A light's `KHR_lights_punctual` object without its name (defaults left out: white, intensity 1, no range; a
/// directional light has no range). An error for a non-finite value.
pub fn punctual_light(l: &Light) -> std::result::Result<Value, String> {
    let finite = |xs: &[f32]| {
        if xs.iter().any(|x| !x.is_finite()) {
            Err(format!("non-finite material factor {xs:?}"))
        } else {
            Ok(f32s_value(xs))
        }
    };
    let mut o = Map::new();
    let kind = match l.kind {
        LightKind::Point => "point",
        LightKind::Directional => "directional",
        LightKind::Spot {
            inner_cone,
            outer_cone,
        } => {
            let cones = finite(&[inner_cone, outer_cone])?;
            o.insert(
                "spot".into(),
                json!({"innerConeAngle": cones[0], "outerConeAngle": cones[1]}),
            );
            "spot"
        }
    };
    o.insert("type".into(), json!(kind));
    if l.color != [1.0; 3] {
        o.insert("color".into(), finite(&l.color)?);
    }
    if l.intensity != 1.0 {
        o.insert("intensity".into(), finite(&[l.intensity])?[0].clone());
    }
    if let Some(r) = l.range
        && l.kind != LightKind::Directional
    {
        o.insert("range".into(), finite(&[r])?[0].clone());
    }
    Ok(Value::Object(o))
}
