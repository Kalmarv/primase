//! One STUNTBOOST level from the install into the world ([`read_level`]), faithfully: every part, node, mesh,
//! material and baked atlas as stored, the node names parsed, in the core frame in metres at the scale the caller
//! states ([Frame](crate#frame)).
//!
//! Files (all uncompressed XNB v5 under `Content/Models`): `Export/Level_<Name>_.xnb` (a `StringReader` JSON sidecar:
//! `modelCount`), per part `Export/Level_<Name>_<N>.xnb` (`BytingPipeline.GLTFReader`: the Blender exporter's glTF
//! 2.0 JSON) with its buffer `Export/<buffer uri>.xnb` (`ArrayReader<Byte>`), and each image
//! `<image uri relative to Export, .png -> .xnb>` (`BytingPipeline.QoiReader`: an i32 byte count and a QOI file; or
//! XNA's `Texture2DReader`: raw `Color` pixels, as the board's own textures are). The derived `<N>Collision.xnb` and
//! `<N>Cuts.xnb` (the game's own builds from the same glTF) are not read.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the level's files from the install"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use primase_core::frame::{Axis, SourceFrame};
use primase_core::pbr::{Filters, Wrap};
use primase_core::{
    ColorSpace, Curve, Entity, Instance, Mesh, Name, Semantic, Source, Stream, TexFormat, TexRole, Texture,
    Transform, World,
};
use primase_glb::Gltf;
use primase_monogame::xnb::Xnb;
use serde_json::Value;

use crate::components::names;
use crate::components::{
    StuntboostAttribute, StuntboostAttributes, StuntboostCollision, StuntboostDirectLight,
    StuntboostGameCollision, StuntboostGameCollisionPart, StuntboostLedge, StuntboostLedgeCorner,
    StuntboostLedgeGraph, StuntboostLedgeKind, StuntboostMaterial, StuntboostNode, StuntboostPart,
    StuntboostRole, StuntboostShortcuts, StuntboostTextureRef,
};
use crate::read::derived;
use crate::{Error, Result};

/// What [`read_level`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelReport {
    /// Each part: its number and role.
    pub parts: Vec<(u32, StuntboostRole)>,
    pub nodes: usize,
    pub placements: usize,
    pub meshes: usize,
    pub materials: usize,
    /// Images decoded, and the image files a material names that the install does not ship (their bindings left
    /// unbound), by path.
    pub textures: usize,
    pub textures_missing: Vec<String>,
    /// Application-specific vertex attributes (`_COLOR_SHIFT`, ...) kept by primitive, by name.
    pub custom_attributes: BTreeMap<String, usize>,
    /// From the game's derived files: nodes with collision, their triangles, the grind ledges and their corners, the
    /// shortcut entries; and the derived files a part lacks, by path.
    pub game_collision_nodes: usize,
    pub game_collision_triangles: usize,
    pub ledges: usize,
    pub ledge_corners: usize,
    pub shortcut_entries: usize,
    pub derived_missing: Vec<String>,
}

/// A glTF texture: its image's entity and its sampler's filters and wrap.
type GltfTexture = (Entity, Filters, [Wrap; 2]);

/// A texture info's texture coordinate set: absent is set 0 (glTF's default); present it is an integer a `u8` holds.
fn texcoord_set(info: &Value) -> std::result::Result<u8, String> {
    match info.get("texCoord") {
        None => Ok(0),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| format!("texCoord {v} is not a texture coordinate set")),
    }
}

pub(super) fn format(file: &str, detail: impl std::fmt::Display) -> Error {
    Error::Format(format!("{file}: {detail}"))
}

/// A file's bytes.
pub(super) fn content(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The folder the levels' files are in.
pub(super) fn export_dir(install: &Path) -> PathBuf {
    install.join("Content").join("Models").join("Export")
}

/// The number of parts the level's sidecar (`Level_<Name>_.xnb`, a JSON object) states.
fn model_count(export: &Path, level: &str) -> Result<u32> {
    let sidecar = export.join(format!("Level_{level}_.xnb"));
    let file = sidecar.display().to_string();
    let side: Value = serde_json::from_str(&string_of(&sidecar, None)?).map_err(|e| format(&file, e))?;
    let count = side
        .get("modelCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| format(&file, "no modelCount"))?;
    u32::try_from(count).map_err(|_| format(&file, "modelCount"))
}

/// A level the install ships, as its sidecar states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelEntry {
    /// The name as the game stores it: the `<Name>` of `Export/Level_<Name>_.xnb`, which [`read_level`] takes.
    pub name: String,
    /// The number of parts (`modelCount`): `Level_<Name>_0.xnb` to `Level_<Name>_<modelCount - 1>.xnb`.
    pub model_count: u32,
}

/// Every level the install ships, by name (byte order), each as its sidecar states it. A level is a
/// `Level_<Name>_.xnb` sidecar in `Content/Models/Export`; its parts' files are not opened.
pub fn levels(install: &Path) -> Result<Vec<LevelEntry>> {
    let export = export_dir(install);
    let io = |source| Error::Io {
        path: export.clone(),
        source,
    };
    let mut names = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&export).map_err(io)? {
        let entry = entry.map_err(io)?;
        let file = entry.file_name();
        let name = file
            .to_str()
            .and_then(|f| f.strip_prefix("Level_")?.strip_suffix("_.xnb"))
            .filter(|n| !n.is_empty());
        if let Some(name) = name {
            names.insert(name.to_owned());
        }
    }
    names
        .into_iter()
        .map(|name| {
            Ok(LevelEntry {
                model_count: model_count(&export, &name)?,
                name,
            })
        })
        .collect()
}

/// The text of a file whose one object a string-shaped reader stores (7-bit length and UTF-8): XNA's
/// `StringReader` (`reader` `None`) or the game's reader of that type name.
pub(super) fn string_of(path: &Path, reader: Option<&str>) -> Result<String> {
    let bytes = content(path)?;
    let xnb = Xnb::parse(&bytes)?;
    let (r, mut c) = xnb.primary()?;
    let fits = match reader {
        None => r.is_string(),
        Some(name) => r.type_name() == name,
    };
    if !fits {
        let reader = reader.unwrap_or("StringReader");
        return Err(format(
            &path.display().to_string(),
            format!("reader {}, not {reader}", r.type_name()),
        ));
    }
    let s = c.string()?.to_owned();
    c.end()?;
    Ok(s)
}

/// The bytes of a file whose one object a byte-array-shaped reader stores (an i32 count and the bytes): XNA's
/// `ArrayReader<Byte>` (`reader` `None`) or the game's reader of that type name.
pub(super) fn bytes_of(path: &Path, reader: Option<&str>) -> Result<Vec<u8>> {
    let bytes = content(path)?;
    let xnb = Xnb::parse(&bytes)?;
    let (r, mut c) = xnb.primary()?;
    let fits = match reader {
        None => r.is_byte_array(),
        Some(name) => r.type_name() == name,
    };
    if !fits {
        let reader = reader.unwrap_or("ArrayReader<Byte>");
        return Err(format(
            &path.display().to_string(),
            format!("reader {}, not {reader}", r.type_name()),
        ));
    }
    let b = c.byte_array()?.to_vec();
    c.end()?;
    Ok(b)
}

/// An image file of the install's content as RGBA8 mip levels, largest first: a QOI image (`BytingPipeline.QoiReader`:
/// an i32 byte count and a QOI file) or XNA's `Texture2D` (raw `Color` pixels).
pub(super) fn image_levels(path: &Path) -> Result<(u32, u32, Vec<Vec<u8>>)> {
    let bytes = content(path)?;
    let xnb = Xnb::parse(&bytes)?;
    let (reader, mut c) = xnb.primary()?;
    let file = path.display().to_string();
    if reader.is_texture_2d() {
        let t = xnb.texture2d()?;
        return Ok((t.width, t.height, t.levels.iter().map(|l| l.to_vec()).collect()));
    }
    if reader.type_name() != "BytingPipeline.QoiReader" {
        return Err(format(
            &file,
            format!(
                "reader {}, neither BytingPipeline.QoiReader nor Texture2DReader",
                reader.type_name()
            ),
        ));
    }
    let qoi = primase_image::read_qoi(c.byte_array()?)?;
    c.end()?;
    let primase_image::Samples::U8(rgba) = qoi.image.samples else {
        return Err(format(&file, "a QOI image not 8-bit"));
    };
    Ok((qoi.image.width, qoi.image.height, vec![rgba]))
}

/// A glTF node's local matrix (column-major 4x4), from `matrix` or its TRS.
fn local(node: &Value) -> [f64; 16] {
    let nums = |k: &str| -> Option<Vec<f64>> { node.get(k)?.as_array()?.iter().map(Value::as_f64).collect() };
    if let Some(m) = nums("matrix").filter(|m| m.len() == 16) {
        let mut out = [0.0; 16];
        out.copy_from_slice(&m);
        return out;
    }
    let t = nums("translation")
        .filter(|v| v.len() == 3)
        .unwrap_or(vec![0.0; 3]);
    let r = nums("rotation")
        .filter(|v| v.len() == 4)
        .unwrap_or(vec![0.0, 0.0, 0.0, 1.0]);
    let s = nums("scale").filter(|v| v.len() == 3).unwrap_or(vec![1.0; 3]);
    let (x, y, z, w) = (r[0], r[1], r[2], r[3]);
    let rot = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * w),
            2.0 * (x * z - y * w),
        ],
        [
            2.0 * (x * y - z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + x * w),
        ],
        [
            2.0 * (x * z + y * w),
            2.0 * (y * z - x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut m = [0.0; 16];
    for c in 0..3 {
        for r in 0..3 {
            m[c * 4 + r] = rot[c][r] * s[c];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m[15] = 1.0;
    m
}

fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn transform(m: &[f64; 16]) -> Transform {
    Transform {
        cols: std::array::from_fn(|c| std::array::from_fn(|r| m[c * 4 + r] as f32)),
    }
}

/// Level `level` (`Skatepark` for `Level_Skatepark`) of the install at `install` into `w`, a STUNTBOOST unit taken as
/// `metres_per_unit` metres (finite and positive; no default: see [Frame](crate#frame)):
///
/// - each part an entity with [`StuntboostPart`] (its role: the part with a `Spawn` node is the level, one with a
///   `Skybox` node the sky, any other a room) and `core.name`;
/// - each node an entity with its world `core.transform` (glTF's axes are the core frame's; translations scaled),
///   `core.name` (the name as stored), `core.source` (`<file>#node <i>`) and [`StuntboostNode`] (its name parsed by
///   [`names`], its extras), and an `Instance` of its mesh's prototype when it has one;
/// - each mesh a prototype with `core.mesh` (positions scaled; its primitives' materials the material entities) and
///   `core.source`, and its application-specific vertex attributes as [`StuntboostAttributes`] (as stored, unscaled);
/// - from the game's own derived files, when shipped: each node's collision as the game derived it
///   ([`StuntboostGameCollision`]), each grind ledge an entity with its segment as `core.curve` (kind `ledge` for an
///   edge, `rail` for a rail) and [`StuntboostLedge`], the part's [`StuntboostLedgeGraph`] and
///   [`StuntboostShortcuts`] (positions and lengths in metres; a missing file counted);
/// - each material an entity with [`StuntboostMaterial`] and `core.name`, and a bake material's direct-light mask as
///   [`StuntboostDirectLight`]; each image a texture entity with
///   `core.texture` (the QOI decoded to RGBA8, sRGB; role other), `core.name` and `core.source`. An image the install
///   does not ship is counted and left unbound.
pub fn read_level(install: &Path, level: &str, metres_per_unit: f64, w: &mut World) -> Result<LevelReport> {
    let frame = SourceFrame::try_new([Axis::PosX, Axis::PosY, Axis::PosZ], metres_per_unit)
        .map_err(|e| Error::Format(format!("metres per unit: {e}")))?;
    let export = export_dir(install);
    let mut report = LevelReport::default();
    let count = model_count(&export, level)?;
    // decoded images by path, shared by every part
    let mut images: BTreeMap<PathBuf, Option<Entity>> = BTreeMap::new();
    for index in 0..count {
        let file = format!("Level_{level}_{index}.xnb");
        let path = export.join(&file);
        let text = string_of(&path, Some("BytingPipeline.GLTFReader"))?;
        let gltf = Gltf::from_gltf(text.as_bytes(), |_, uri| {
            bytes_of(&export.join(format!("{uri}.xnb")), None).map_err(|e| e.to_string())
        })?;
        read_part(
            w,
            &mut report,
            &export,
            level,
            index,
            &file,
            &gltf,
            &frame,
            &mut images,
        )?;
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn read_part(
    w: &mut World,
    report: &mut LevelReport,
    export: &Path,
    level: &str,
    index: u32,
    file: &str,
    gltf: &Gltf,
    frame: &SourceFrame,
    images: &mut BTreeMap<PathBuf, Option<Entity>>,
) -> Result<()> {
    let json = gltf.json();
    let array = |k: &str| json.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let nodes = array("nodes");
    let parsed: Vec<_> = (nodes.iter())
        .map(|n| names::parse(n.get("name").and_then(Value::as_str).unwrap_or("")))
        .collect();
    let role = if parsed.iter().any(|n| n.class_name() == "Spawn") {
        StuntboostRole::Level
    } else if parsed.iter().any(|n| n.class_name() == "Skybox") {
        StuntboostRole::Skybox
    } else {
        StuntboostRole::Room
    };
    report.parts.push((index, role));
    let part = w.spawn();
    w.insert(
        part,
        StuntboostPart {
            level: level.to_owned(),
            index,
            role,
            file: file.to_owned(),
        },
    );
    w.insert(part, Name(format!("Level_{level}_{index}")));

    // textures: glTF texture index -> (image entity, sampler filters)
    let samplers = array("samplers");
    let gltf_images = array("images");
    let mut texture_refs: Vec<Option<GltfTexture>> = Vec::new();
    for t in array("textures") {
        let image = t
            .get("source")
            .and_then(Value::as_u64)
            .and_then(|i| gltf_images.get(i as usize));
        let uri = image.and_then(|i| i.get("uri")).and_then(Value::as_str);
        let entity = match uri {
            Some(uri) => {
                let path = export.join(uri).with_extension("xnb");
                if let Some(e) = images.get(&path) {
                    *e
                } else {
                    let e = if path.is_file() {
                        let (width, height, levels) = image_levels(&path)?;
                        let e = w.spawn();
                        w.insert(
                            e,
                            Texture {
                                width,
                                height,
                                format: TexFormat::Rgba8Unorm,
                                levels,
                                color_space: ColorSpace::Srgb,
                                role: TexRole::Other,
                                normal: None,
                            },
                        );
                        let stem = Path::new(uri)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned());
                        w.insert(e, Name(stem.unwrap_or_default()));
                        w.insert(e, Source(path.display().to_string()));
                        report.textures += 1;
                        Some(e)
                    } else {
                        report.textures_missing.push(path.display().to_string());
                        None
                    };
                    images.insert(path, e);
                    e
                }
            }
            None => None,
        };
        let sampler = t
            .get("sampler")
            .and_then(Value::as_u64)
            .and_then(|i| samplers.get(i as usize));
        let (filters, wrap) = match sampler {
            Some(s) => primase_glb::sampler_filters(s)?,
            None => (Filters::default(), [Wrap::Repeat; 2]),
        };
        texture_refs.push(entity.map(|e| (e, filters, wrap)));
    }
    let texture_ref =
        |info: Option<&Value>, material: &str, slot: &str| -> Result<Option<StuntboostTextureRef>> {
            let Some(info) = info else { return Ok(None) };
            let texcoord = texcoord_set(info)
                .map_err(|detail| format(file, format!("material {material:?} {slot}: {detail}")))?;
            let bound = info
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|i| texture_refs.get(i as usize));
            Ok(bound
                .copied()
                .flatten()
                .map(|(texture, filters, wrap)| StuntboostTextureRef {
                    texture,
                    filters,
                    wrap,
                    texcoord,
                }))
        };

    // materials
    let mut materials = Vec::new();
    for m in array("materials") {
        let pbr = m.get("pbrMetallicRoughness");
        let floats = |v: Option<&Value>, n: usize, default: &[f32]| -> Vec<f32> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_f64)
                        .map(|f| f as f32)
                        .collect::<Vec<_>>()
                })
                .filter(|a| a.len() == n)
                .unwrap_or_else(|| default.to_vec())
        };
        let bc = floats(pbr.and_then(|p| p.get("baseColorFactor")), 4, &[1.0; 4]);
        let em = floats(m.get("emissiveFactor"), 3, &[0.0; 3]);
        let mut other = BTreeMap::new();
        for (k, v) in m.as_object().into_iter().flatten() {
            if ![
                "name",
                "pbrMetallicRoughness",
                "emissiveFactor",
                "doubleSided",
                "alphaMode",
                "alphaCutoff",
            ]
            .contains(&k.as_str())
            {
                other.insert(k.clone(), v.clone());
            }
        }
        let specular = m
            .get("extensions")
            .and_then(|e| e.get("KHR_materials_specular"))
            .and_then(|s| s.get("specularColorTexture"));
        let name = m.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        let shading = primase_glb::material_shading(&m)?;
        let base_color_texture = texture_ref(
            pbr.and_then(|p| p.get("baseColorTexture")),
            &name,
            "baseColorTexture",
        )?;
        let direct_light = texture_ref(specular, &name, "specularColorTexture")?;
        let material = StuntboostMaterial {
            shading,
            name: name.clone(),
            base_color: [bc[0], bc[1], bc[2], bc[3]],
            base_color_texture,
            metallic: pbr
                .and_then(|p| p.get("metallicFactor"))
                .and_then(Value::as_f64)
                .unwrap_or(1.0) as f32,
            roughness: pbr
                .and_then(|p| p.get("roughnessFactor"))
                .and_then(Value::as_f64)
                .unwrap_or(1.0) as f32,
            emissive: [em[0], em[1], em[2]],
            double_sided: m.get("doubleSided").and_then(Value::as_bool).unwrap_or(false),
            alpha_mode: m
                .get("alphaMode")
                .and_then(Value::as_str)
                .unwrap_or("OPAQUE")
                .to_owned(),
            alpha_cutoff: m.get("alphaCutoff").and_then(Value::as_f64).map(|f| f as f32),
            other,
        };
        let e = w.spawn();
        w.insert(e, material);
        if let Some(texture) = direct_light {
            w.insert(e, StuntboostDirectLight { texture });
        }
        w.insert(e, Name(name));
        materials.push(e);
        report.materials += 1;
    }

    // meshes: one prototype each
    let mut prototypes = Vec::with_capacity(gltf.mesh_count());
    for i in 0..gltf.mesh_count() {
        let read = gltf.read_mesh(i)?;
        let mut mesh_primitives = Vec::with_capacity(read.primitives.len());
        let mut attributes = Vec::with_capacity(read.primitives.len());
        for p in &read.primitives {
            let mut primitive = p.primitive.clone();
            primitive.material = p.material.and_then(|m| materials.get(m).copied());
            if let Some(Stream::F32x3(positions)) = primitive.streams.get_mut(&Semantic::Position) {
                positions.iter_mut().for_each(|v| *v = frame.point(*v));
            }
            mesh_primitives.push(primitive);
            let mut custom = BTreeMap::new();
            for (name, data) in &p.custom {
                *report.custom_attributes.entry(name.clone()).or_default() += 1;
                custom.insert(name.clone(), StuntboostAttribute::from(data));
            }
            attributes.push(custom);
        }
        let e = w.spawn();
        w.insert(e, Mesh::new(mesh_primitives));
        w.insert(e, Source(format!("{file}#mesh {i}")));
        if let Some(name) = &read.name {
            w.insert(e, Name(name.clone()));
        }
        if attributes.iter().any(|a| !a.is_empty()) {
            w.insert(e, StuntboostAttributes(attributes));
        }
        prototypes.push(e);
        report.meshes += 1;
    }

    // nodes, from the scene's roots down
    let scene = json.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
    let roots: Vec<usize> = (json.get("scenes").and_then(Value::as_array))
        .and_then(|s| s.get(scene))
        .and_then(|s| s.get("nodes"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).map(|i| i as usize).collect())
        .unwrap_or_default();
    let identity = {
        let mut m = [0.0; 16];
        for d in 0..4 {
            m[d * 5] = 1.0;
        }
        m
    };
    let mut stack: Vec<(usize, [f64; 16])> = roots.into_iter().rev().map(|r| (r, identity)).collect();
    let mut visited = vec![false; nodes.len()];
    let mut node_entities: Vec<Option<Entity>> = vec![None; nodes.len()];
    while let Some((i, parent)) = stack.pop() {
        let (Some(node), Some(name)) = (nodes.get(i), parsed.get(i)) else {
            return Err(format(
                file,
                format!("node {i} past the document's {}", nodes.len()),
            ));
        };
        if std::mem::replace(&mut visited[i], true) {
            return Err(format(file, format!("node {i} reached twice")));
        }
        let world = mul(&parent, &local(node));
        let e = w.spawn();
        if let Some(slot) = node_entities.get_mut(i) {
            *slot = Some(e);
        }
        w.insert(e, frame.transform(&transform(&world)));
        w.insert(e, Name(name.raw.clone()));
        w.insert(e, Source(format!("{file}#node {i}")));
        w.insert(
            e,
            StuntboostNode {
                part,
                node: i as u32,
                name: name.clone(),
                extras: node.get("extras").cloned(),
            },
        );
        if let Some(m) = node.get("mesh").and_then(Value::as_u64) {
            let prototype = *prototypes
                .get(m as usize)
                .ok_or_else(|| format(file, format!("node {i}: mesh {m} past the document's meshes")))?;
            w.insert(
                e,
                Instance {
                    prototype,
                    variant: 0,
                },
            );
            report.placements += 1;
        }
        report.nodes += 1;
        for c in (node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten())
        .rev()
        .filter_map(Value::as_u64)
        {
            stack.push((c as usize, world));
        }
    }
    read_derived(w, report, export, level, index, part, &node_entities, frame)
}

/// The part's derived files, when the install ships them: the game's collision model onto the node entities, its
/// ledges as entities, its ledge graph and shortcuts onto the part (see [`derived`]).
#[allow(clippy::too_many_arguments)]
fn read_derived(
    w: &mut World,
    report: &mut LevelReport,
    export: &Path,
    level: &str,
    index: u32,
    part: Entity,
    node_entities: &[Option<Entity>],
    frame: &SourceFrame,
) -> Result<()> {
    let length = |v: f32| (f64::from(v) * frame.metres_per_unit()) as f32;
    let collision_path = export.join(format!("Level_{level}_{index}Collision.xnb"));
    if collision_path.is_file() {
        let file = collision_path.display().to_string();
        let model = derived::collision_model(&content(&collision_path)?).map_err(|e| format(&file, e))?;
        if model.meshes.len() != node_entities.len() {
            return Err(format(
                &file,
                format!(
                    "{} collision meshes for {} nodes",
                    model.meshes.len(),
                    node_entities.len()
                ),
            ));
        }
        for (mesh, entity) in model.meshes.iter().zip(node_entities) {
            let (Some(mesh), Some(e)) = (mesh, entity) else {
                continue;
            };
            let parts: Vec<StuntboostGameCollisionPart> = (mesh.parts.iter())
                .map(|p| StuntboostGameCollisionPart {
                    triangles: p.triangles.iter().map(|t| t.map(|v| frame.point(v))).collect(),
                    grippable: p.grippable.clone(),
                    ramps: p.ramps.clone(),
                })
                .collect();
            report.game_collision_nodes += 1;
            report.game_collision_triangles += parts.iter().map(|p| p.triangles.len()).sum::<usize>();
            let aggregate = match mesh.aggregate {
                0 => StuntboostCollision::Solid,
                1 => StuntboostCollision::Trigger,
                _ => StuntboostCollision::None,
            };
            w.insert(*e, StuntboostGameCollision { aggregate, parts });
        }
        // the ledges, each its own entity with its segment as a core curve
        let ledges: Vec<Entity> = (model.ledges.iter().enumerate())
            .map(|(i, l)| {
                let at = |c: u32| {
                    model
                        .corners
                        .get(c as usize)
                        .map_or([0.0; 3], |c| frame.point(c.position))
                };
                let kind = match l.kind {
                    0 => StuntboostLedgeKind::Edge,
                    1 => StuntboostLedgeKind::RailNoBoost,
                    _ => StuntboostLedgeKind::RailBoost,
                };
                let e = w.spawn();
                w.insert(
                    e,
                    Curve {
                        kind: if kind == StuntboostLedgeKind::Edge {
                            "ledge"
                        } else {
                            "rail"
                        }
                        .into(),
                        points: vec![at(l.corners[0]), at(l.corners[1])],
                        closed: false,
                        tangents: None,
                        up: None,
                    },
                );
                w.insert(e, Name(format!("Level_{level}_{index} ledge {i}")));
                w.insert(e, Source(format!("{file}#ledge {i}")));
                w.insert(
                    e,
                    StuntboostLedge {
                        part,
                        kind,
                        corners: l.corners,
                        normal: frame.direction(l.normal),
                        tangent: frame.direction(l.tangent),
                        binormal: frame.direction(l.binormal),
                        min_angle: l.min_angle,
                        max_angle: l.max_angle,
                        length: length(l.length),
                        length_recursive: l.length_recursive.map(length),
                        group: l.group,
                        lerp_cut_off: l.lerp_cut_off,
                    },
                );
                e
            })
            .collect();
        report.ledges += ledges.len();
        report.ledge_corners += model.corners.len();
        let corners = (model.corners.iter())
            .map(|c| StuntboostLedgeCorner {
                position: frame.point(c.position),
                near: c.near.clone(),
                ledges: (c.ledges.iter())
                    .filter_map(|&(l, forward)| Some((*ledges.get(l as usize)?, forward)))
                    .collect(),
            })
            .collect();
        w.insert(
            part,
            StuntboostLedgeGraph {
                solids_cell: model.solids_cell.map(length),
                triggers_cell: model.triggers_cell.map(length),
                corners,
            },
        );
    } else {
        report.derived_missing.push(collision_path.display().to_string());
    }
    let cuts_path = export.join(format!("Level_{level}_{index}Cuts.xnb"));
    if cuts_path.is_file() {
        let file = cuts_path.display().to_string();
        let cuts = derived::shortcuts(&content(&cuts_path)?).map_err(|e| format(&file, e))?;
        report.shortcut_entries += cuts.len();
        w.insert(part, StuntboostShortcuts(cuts));
    } else {
        report.derived_missing.push(cuts_path.display().to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::StuntboostCollision;

    /// An uncompressed XNB v5 file for DesktopGL holding one object of `reader`.
    fn xnb(reader: &str, content: &[u8]) -> Vec<u8> {
        let seven = |mut n: usize, out: &mut Vec<u8>| loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        };
        let mut body = Vec::new();
        seven(1, &mut body);
        seven(reader.len(), &mut body);
        body.extend_from_slice(reader.as_bytes());
        body.extend_from_slice(&0i32.to_le_bytes());
        seven(0, &mut body);
        seven(1, &mut body);
        body.extend_from_slice(content);
        let mut out = b"XNBd\x05\x00".to_vec();
        out.extend_from_slice(&u32::try_from(body.len() + 10).unwrap().to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn string(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut n = s.len();
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// The listing holds the levels' sidecars only, by name in byte order, each with its `modelCount`: a part's
    /// file, a buffer, a derived file and other content are not levels.
    #[test]
    fn the_listing_is_the_sidecars_with_their_counts() {
        let dir = std::env::temp_dir().join(format!("primase-stuntboost-levels-{}", std::process::id()));
        let export = dir.join("Content").join("Models").join("Export");
        std::fs::create_dir_all(&export).unwrap();
        let string_reader = "Microsoft.Xna.Framework.Content.StringReader, MonoGame.Framework";
        for (name, count) in [("B_x", 1), ("A", 3)] {
            std::fs::write(
                export.join(format!("Level_{name}_.xnb")),
                xnb(string_reader, &string(&format!(r#"{{"modelCount":{count}}}"#))),
            )
            .unwrap();
        }
        for other in [
            "Level_A_0.xnb",
            "Level_A_0.bin.xnb",
            "Level_A_0Collision.xnb",
            "Level__.xnb",
            "Board.xnb",
        ] {
            std::fs::write(export.join(other), b"not a sidecar").unwrap();
        }
        assert_eq!(
            levels(&dir).unwrap(),
            [
                LevelEntry {
                    name: "A".into(),
                    model_count: 3
                },
                LevelEntry {
                    name: "B_x".into(),
                    model_count: 1
                },
            ]
        );
        assert!(levels(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A level `T` of one part as the game ships it, in a fresh directory named for `test`: the sidecar, the glTF (a
    /// floor mesh placed under a parent moved 2 units up, a spawn, `material` naming an image the install does not
    /// ship) and its buffer.
    fn ship_level(test: &str, material: serde_json::Value) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("primase-stuntboost-{test}-{}", std::process::id()));
        let export = dir.join("Content").join("Models").join("Export");
        std::fs::create_dir_all(&export).unwrap();
        let string_reader = "Microsoft.Xna.Framework.Content.StringReader, MonoGame.Framework";
        std::fs::write(
            export.join("Level_T_.xnb"),
            xnb(string_reader, &string(r#"{"modelCount":1}"#)),
        )
        .unwrap();
        // one triangle: positions (36 bytes) then u16 indices (6 bytes)
        let mut bin = Vec::new();
        for v in [[0f32, 0., 0.], [100., 0., 0.], [0., 0., 100.]] {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let gltf = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0, 2]}],
            "nodes": [
                {"name": "group", "translation": [0.0, 200.0, 0.0], "children": [1]},
                {"name": "floor_<.001", "mesh": 0},
                {"name": "=Spawn(start)", "translation": [100.0, 0.0, 0.0]}
            ],
            "meshes": [{"name": "floor", "primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
            "materials": [material],
            "textures": [{"source": 0}],
            "images": [{"uri": "../Resources/T.export/Bake0_Denoised.png"}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                 "min": [0.0, 0.0, 0.0], "max": [100.0, 0.0, 100.0]},
                {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 6}
            ],
            "buffers": [{"uri": "Level_T_0.bin", "byteLength": 42}]
        });
        std::fs::write(
            export.join("Level_T_0.xnb"),
            xnb(
                "BytingPipeline.GLTFReader, BytingPipeline",
                &string(&gltf.to_string()),
            ),
        )
        .unwrap();
        let mut array = u32::try_from(bin.len()).unwrap().to_le_bytes().to_vec();
        array.extend_from_slice(&bin);
        let byte_array =
            "Microsoft.Xna.Framework.Content.ArrayReader`1[[System.Byte, System.Private.CoreLib]]";
        std::fs::write(export.join("Level_T_0.bin.xnb"), xnb(byte_array, &array)).unwrap();
        dir
    }

    /// A level of one part as the game ships it: the sidecar, the glTF (a floor mesh placed under a parent moved 2
    /// units up, a spawn, a material naming an image the install does not ship) and its buffer; read into parts,
    /// nodes with world transforms and parsed names, a prototype, a material and the missing image counted.
    #[test]
    fn a_shipped_level_reads_into_parts_nodes_and_prototypes() {
        let material = serde_json::json!({"name": "C_Default_Wood", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}});
        let dir = ship_level("shipped", material);
        let mut w = World::new();
        // a unit as a centimetre
        let r = read_level(&dir, "T", 0.01, &mut w).unwrap();
        assert_eq!(r.parts, [(0, StuntboostRole::Level)]);
        assert_eq!(
            (r.nodes, r.placements, r.meshes, r.materials, r.textures),
            (3, 1, 1, 1, 0)
        );
        assert_eq!(r.textures_missing.len(), 1);
        let floor = w
            .iter::<StuntboostNode>()
            .find(|(_, n)| n.name.name == "floor")
            .map(|(e, n)| (e, n.name.collision))
            .unwrap();
        assert_eq!(floor.1, StuntboostCollision::Solid);
        // under its parent: 200 units up, 2 m
        assert_eq!(w.get::<Transform>(floor.0).unwrap().cols[3], [0.0, 2.0, 0.0]);
        // the spawn 100 units along x: 1 m
        let spawn = (w.iter::<StuntboostNode>())
            .find(|(_, n)| n.name.class_name() == "Spawn")
            .map(|(e, _)| e)
            .unwrap();
        assert_eq!(w.get::<Transform>(spawn).unwrap().cols[3], [1.0, 0.0, 0.0]);
        let prototype = w.get::<Instance>(floor.0).unwrap().prototype;
        let mesh = w.get::<Mesh>(prototype).unwrap();
        // the vertices in metres too
        assert_eq!(
            mesh.primitives()[0].positions().unwrap(),
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert!(read_level(&dir, "T", 0.0, &mut World::new()).is_err());
        let material = mesh.primitives()[0].material.unwrap();
        let m = w.get::<StuntboostMaterial>(material).unwrap();
        assert_eq!(m.name, "C_Default_Wood");
        assert!(m.base_color_texture.is_none(), "the image is not shipped");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A texture info's `texCoord` is absent (set 0) or an integer a `u8` holds; anything else is an error naming the
    /// material and the slot, not set 0.
    #[test]
    fn a_malformed_texcoord_is_an_error_naming_the_material_and_slot() {
        let set = |info: serde_json::Value| texcoord_set(&info);
        assert_eq!(set(serde_json::json!({"index": 0})), Ok(0));
        assert_eq!(set(serde_json::json!({"index": 0, "texCoord": 1})), Ok(1));
        assert_eq!(set(serde_json::json!({"texCoord": 255})), Ok(255));
        for bad in [
            serde_json::json!(256),
            serde_json::json!(4294967296u64),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("1"),
            serde_json::json!(null),
        ] {
            assert!(set(serde_json::json!({"texCoord": bad})).is_err(), "{bad}");
        }
        for (material, slot) in [
            (
                serde_json::json!({"name": "M_Bad", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0, "texCoord": 256}}}),
                "baseColorTexture",
            ),
            (
                serde_json::json!({"name": "M_Bad", "extensions": {"KHR_materials_specular": {"specularColorTexture": {"index": 0, "texCoord": 1.5}}}}),
                "specularColorTexture",
            ),
        ] {
            let dir = ship_level(slot, material);
            let error = read_level(&dir, "T", 0.01, &mut World::new())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("M_Bad") && error.contains(slot) && error.contains("texCoord"),
                "{error}"
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
