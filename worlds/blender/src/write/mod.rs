//! The writer: a world to a folder the save stage builds a `.blend` from.
//!
//! - `scene.glb`: the glTF writer's output for the world, unchanged ([`primase_gltf::write_file`]): its geometry,
//!   placements, rigs and lights reach Blender through Blender's own glTF importer. Its materials are glTF's form of
//!   the world's; Blender draws the graphs here instead.
//! - `materials.json` ([`crate::MATERIALS_SCHEMA`]): one node graph per material drawn ([`crate::graph`]), and for
//!   each glTF mesh, by index, each primitive's graph (or none). A material is drawn with one graph per form its
//!   primitives give it: with or without `COLOR_0` (a primitive the glTF writer wrote with it, for a material that
//!   reads it), and with its blend or its host alone.
//! - `textures/<sha256>.png`: each texture the graphs sample, its largest level as RGBA8 PNG, named by its bytes'
//!   hash (one file for identical images).
//!
//! What it draws: a material entity's `pbr.material` as its graph, and a `core.layered` on it as its blend, the
//! `pbr.material` its host ([`crate::graph::layered`]). A blend is drawn on a primitive that carries all it reads; on
//! one that does not, its host alone, counted by the reason ([`crate::graph::layered::Blocker`]). A `core.layered`
//! without a host `pbr.material` is not drawn, its primitives drawn with no material, counted (core's rule: no
//! default a writer picks). A material that also states a live form this writer does not draw yet (`pbr.detail`) is
//! drawn from its host `pbr.material`, counted by the form. A material entity with neither is not drawn, counted.
//!
//! The folder is the writer's: one that holds anything is refused, never emptied here.

#![allow(
    clippy::disallowed_methods,
    reason = "writer I/O: writes the scene folder (scene.glb, materials.json, textures/)"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use primase_core::detail::DetailedMaterial;
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, LayeredMaterial, Mesh, Name, Semantic, Texture, VertexColor, World};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::graph::texture::{Counts, Reading, Sampler};
use crate::graph::{Graph, PASS_ALPHA_THRESHOLD, Settings, aov, layered, pbr};
use crate::{Error, MATERIALS_SCHEMA, Result};

/// Writer options.
#[derive(Clone, Debug, Default)]
pub struct WriteOptions {
    /// The glTF writer's, for `scene.glb`.
    pub gltf: primase_gltf::WriteOptions,
}

/// What the writer did, with counts of everything it did not draw as the world states it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlenderReport {
    /// `scene.glb`'s report, whole. Its material counts are glTF's form of the world's materials; what Blender draws
    /// of them is counted here.
    pub gltf: primase_gltf::WriteReport,
    /// Blender materials written: one per material entity drawn and whether it draws `COLOR_0`.
    pub materials: u64,
    pub nodes: u64,
    pub links: u64,
    /// glTF primitives drawn with a graph.
    pub primitives_drawn: u64,
    /// glTF primitives with no material in the world: Blender draws them with its default surface.
    pub primitives_without_material: u64,
    /// Material entities not drawn, by cause: their primitives have no material.
    pub materials_not_drawn: BTreeMap<String, u64>,
    /// Blender materials drawing a `core.layered` blend.
    pub blends: u64,
    /// `core.layered` materials drawn as their host alone on some primitive, by the reason (once per material and
    /// reason).
    pub blends_drawn_from_host: BTreeMap<String, u64>,
    /// Primitives drawn with a blend whose mesh has `COLOR_0` and a layer takes it as its colour, where `scene.glb`
    /// leaves it out (the host does not read it, and the glTF writer writes it for the host).
    pub blend_layers_without_color0: u64,
    /// Material entities drawn from their host `pbr.material` that also state a live form this writer does not draw
    /// yet (`pbr.detail`), by the form's component.
    pub live_forms_drawn_from_host: BTreeMap<String, u64>,
    /// Images the graphs sample (a texture read as colour and as data is two).
    pub images: u64,
    /// PNG files written, and their bytes.
    pub image_files: u64,
    pub image_bytes: u64,
    /// What the graphs do not draw as core states it ([`Counts`]).
    pub counts: Counts,
}

impl fmt::Display for BlenderReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.gltf.to_string().trim_end())?;
        writeln!(
            f,
            "blender: {} materials ({} blends; {} nodes, {} links) drawing {} primitives; {} primitives without a \
             material; {} images in {} files ({} bytes); primitives whose blend's layers lose COLOR_0 {}",
            self.materials,
            self.blends,
            self.nodes,
            self.links,
            self.primitives_drawn,
            self.primitives_without_material,
            self.images,
            self.image_files,
            self.image_bytes,
            self.blend_layers_without_color0
        )?;
        let maps = [
            ("materials not drawn", &self.materials_not_drawn),
            ("blends drawn as their host alone", &self.blends_drawn_from_host),
            (
                "drawn from their host pbr.material, a live form not drawn yet",
                &self.live_forms_drawn_from_host,
            ),
            ("texture slots not sampled", &self.counts.slots_unsampled),
        ];
        for (what, map) in maps {
            if !map.is_empty() {
                let items: Vec<String> = map.iter().map(|(k, n)| format!("{k} {n}")).collect();
                writeln!(f, "{what}: {}", items.join(", "))?;
            }
        }
        let c = &self.counts;
        writeln!(
            f,
            "not drawn as stated: minification or mip filters {}, colour slots filtered before decoding {}, \
             occlusion maps {}, normal maps in Blender's tangents {}, specular f90 {}",
            c.minification_unexpressed,
            c.srgb_filtered_encoded,
            c.occlusion_unwritten,
            c.normal_maps_mikktspace,
            c.specular_f90_unexpressed
        )
    }
}

/// `materials.json`.
#[derive(Serialize)]
struct MaterialsFile<'a> {
    schema: &'static str,
    /// The view layer's pass alpha threshold the graphs' coverage is written at.
    pass_alpha_threshold: f32,
    /// The passes every material writes: name and Blender's AOV type.
    aovs: [(&'static str, &'static str); 3],
    images: Vec<ImageEntry>,
    materials: &'a [MaterialEntry],
    /// For each glTF mesh, by index, each primitive's material (its index in `materials`), or none.
    meshes: Vec<Vec<Option<usize>>>,
}

#[derive(Serialize)]
struct ImageEntry {
    /// Relative to the folder, `/`-separated.
    file: String,
    reading: Reading,
    name: String,
}

#[derive(Serialize)]
struct MaterialEntry {
    name: String,
    settings: Settings,
    graph: Graph,
}

/// Write `world` into the folder `dir` (absent or empty; see the module docs).
pub fn write_file(world: &World, options: &WriteOptions, dir: &Path) -> Result<BlenderReport> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    if dir.exists() && std::fs::read_dir(dir).map_err(io(dir))?.next().is_some() {
        return Err(Error::Stage(format!(
            "{}: the folder is not empty (empty it first if it is to be replaced)",
            dir.display()
        )));
    }
    let textures = dir.join("textures");
    std::fs::create_dir_all(&textures).map_err(io(&textures))?;
    let gltf = primase_gltf::write_file(world, &options.gltf, &dir.join("scene.glb"))?;
    let mut report = BlenderReport::default();
    let mut sampler = Sampler::new(world);
    let mut materials: Vec<MaterialEntry> = Vec::new();
    let mut index: BTreeMap<(Entity, Form), Option<usize>> = BTreeMap::new();
    let mut blocked: BTreeSet<(Entity, String)> = BTreeSet::new();
    let mut meshes = Vec::new();
    for mesh in &gltf.layout.meshes {
        let mut prims = Vec::new();
        for (i, p) in mesh.primitives.iter().enumerate() {
            let Some(m) = p.material else {
                report.primitives_without_material += 1;
                prims.push(None);
                continue;
            };
            let carries = p.attributes.contains("COLOR_0");
            let blend = match world.get::<LayeredMaterial>(m) {
                Some(l) if world.has::<PbrMaterial>(m) => {
                    match layered::blocker(world, &sampler, l, &p.attributes)? {
                        None => {
                            if !carries
                                && layers_take_color0(world, l)
                                && mesh_has_color0(world, mesh.prototype, i)
                            {
                                report.blend_layers_without_color0 += 1;
                            }
                            true
                        }
                        Some(why) => {
                            let why = why.to_string();
                            if blocked.insert((m, why.clone())) {
                                *report.blends_drawn_from_host.entry(why).or_default() += 1;
                            }
                            false
                        }
                    }
                }
                _ => false,
            };
            let form = Form {
                // a blend's layers each take COLOR_0 or not; a material alone as it states
                color0: carries && (blend || !world.has::<VertexColor>(m)),
                blend,
            };
            let k = match index.get(&(m, form)) {
                Some(&k) => k,
                None => {
                    let first = !index.keys().any(|(e, _)| *e == m);
                    let k = material(world, &mut sampler, &mut report, (m, form, first), &mut materials)?;
                    index.insert((m, form), k);
                    k
                }
            };
            if k.is_some() {
                report.primitives_drawn += 1;
            }
            prims.push(k);
        }
        meshes.push(prims);
    }
    // a material drawn in more than one form names each graph by what sets it apart
    let mut forms: BTreeMap<Entity, Vec<Form>> = BTreeMap::new();
    for ((e, form), k) in &index {
        if k.is_some() {
            forms.entry(*e).or_default().push(*form);
        }
    }
    for ((e, form), k) in &index {
        let Some(k) = k else { continue };
        let all = &forms[e];
        if form.color0 && all.iter().any(|f| !f.color0) {
            materials[*k].name.push_str(" (COLOR_0)");
        }
        if !form.blend && all.iter().any(|f| f.blend) {
            materials[*k].name.push_str(" (host)");
        }
    }
    let mut files: BTreeMap<Entity, String> = BTreeMap::new();
    let mut hashes: BTreeSet<String> = BTreeSet::new();
    let mut images = Vec::new();
    for u in &sampler.images.list {
        let file = match files.get(&u.texture) {
            Some(f) => f.clone(),
            None => {
                let t = world.get::<Texture>(u.texture).ok_or_else(|| Error::World {
                    entity: u.texture,
                    detail: "sampled texture without core.texture".into(),
                })?;
                let png = primase_image::texture_level_png(t, 0).map_err(|e| Error::Texture {
                    entity: u.texture,
                    detail: e.to_string(),
                })?;
                let hash: String = Sha256::digest(&png).iter().map(|b| format!("{b:02x}")).collect();
                let file = format!("textures/{hash}.png");
                if hashes.insert(hash) {
                    let path = dir.join(&file);
                    std::fs::write(&path, &png).map_err(io(&path))?;
                    report.image_files += 1;
                    report.image_bytes += png.len() as u64;
                }
                files.insert(u.texture, file.clone());
                file
            }
        };
        images.push(ImageEntry {
            file,
            reading: u.reading,
            name: u.name.clone(),
        });
    }
    report.images = images.len() as u64;
    report.materials = materials.len() as u64;
    for m in &materials {
        report.nodes += m.graph.nodes.len() as u64;
        report.links += m.graph.links.len() as u64;
        finite(&m.graph).map_err(|detail| Error::Stage(format!("material {}: {detail}", m.name)))?;
    }
    let file = MaterialsFile {
        schema: MATERIALS_SCHEMA,
        pass_alpha_threshold: PASS_ALPHA_THRESHOLD,
        aovs: [
            (aov::BASE_COLOR, "COLOR"),
            (aov::EMISSION, "COLOR"),
            (aov::COVERAGE, "VALUE"),
        ],
        images,
        materials: &materials,
        meshes,
    };
    let path = dir.join("materials.json");
    let text = serde_json::to_vec(&file).map_err(|e| Error::Stage(format!("materials.json: {e}")))?;
    std::fs::write(&path, text).map_err(io(&path))?;
    report.counts = sampler.counts;
    report.gltf = gltf;
    Ok(report)
}

/// How a material entity is drawn on a primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Form {
    /// The primitive carries `COLOR_0` and the material may read it as a colour (a blend's layers each by its own).
    color0: bool,
    /// Its `core.layered` blend is drawn (else its `pbr.material` alone).
    blend: bool,
}

/// Whether some layer of `l` takes `COLOR_0` as its colour ([`layered`]'s rule).
fn layers_take_color0(world: &World, l: &LayeredMaterial) -> bool {
    !l.reads(Semantic::Color(0))
        && l.layers
            .iter()
            .flatten()
            .any(|layer| !world.has::<VertexColor>(layer.material))
}

/// Whether primitive `i` of `prototype`'s mesh has a `COLOR_0` stream.
fn mesh_has_color0(world: &World, prototype: Entity, i: usize) -> bool {
    world
        .get::<Mesh>(prototype)
        .and_then(|m| m.primitives().get(i))
        .is_some_and(|p| p.streams.contains_key(&Semantic::Color(0)))
}

/// The graph drawing material entity `m` in `form`, pushed to `out`; `None` where it is not drawn. What it does not
/// draw is counted once per entity, its `first` graph.
fn material(
    world: &World,
    sampler: &mut Sampler,
    report: &mut BlenderReport,
    (m, form, first): (Entity, Form, bool),
    out: &mut Vec<MaterialEntry>,
) -> Result<Option<usize>> {
    let Some(pbr) = world.get::<PbrMaterial>(m) else {
        let cause = if world.has::<LayeredMaterial>(m) {
            "core.layered without a host pbr.material"
        } else {
            "without pbr.material"
        };
        if first {
            *report.materials_not_drawn.entry(cause.to_owned()).or_default() += 1;
        }
        return Ok(None);
    };
    if first && world.has::<DetailedMaterial>(m) {
        *report
            .live_forms_drawn_from_host
            .entry(DetailedMaterial::NAME.to_owned())
            .or_default() += 1;
    }
    let (graph, settings) = match world.get::<LayeredMaterial>(m) {
        Some(l) if form.blend => {
            report.blends += 1;
            layered::graph(sampler, world, m, l, pbr, form.color0)?
        }
        _ => pbr::graph(sampler, pbr, form.color0)?,
    };
    let name = world
        .get::<Name>(m)
        .map_or_else(|| format!("material {}", m.index()), |n| n.0.clone());
    out.push(MaterialEntry {
        name,
        settings,
        graph,
    });
    Ok(Some(out.len() - 1))
}

/// Every value the graph states is a number JSON holds (a non-finite one would be written as null).
fn finite(g: &Graph) -> std::result::Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::Null => false,
            Value::Array(a) => a.iter().all(walk),
            Value::Object(o) => o.values().all(walk),
            _ => true,
        }
    }
    for (i, n) in g.nodes.iter().enumerate() {
        for (id, v) in n.props.iter().chain(&n.inputs) {
            if !walk(v) {
                return Err(format!("node {i} ({}) {id} is not finite", n.kind));
            }
        }
    }
    Ok(())
}
