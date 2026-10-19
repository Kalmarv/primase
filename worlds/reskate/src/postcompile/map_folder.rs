//! Reading a map folder's vertex-blend input for the post-compile stage ([`crate::postcompile::vbrecord`]): the
//! writer's record (`vertex_blend.json`), the weights of the primitives it names (from the map's GLBs) and the map's
//! objects (`map.json`), into memory, so the stage itself reads no file.
//!
//! The record: `{"format": 1, "materials": {name: [[mesh, primitive], ...]}}`, each (GLB relative to the map folder,
//! primitive of its one mesh) drawing the material. A recorded GLB path must stay inside the map folder and go through
//! no link, as the scene reader requires of its files.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the vertex-blend record, map.json and the GLBs of the map folder the caller names"
)]

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};

use crate::postcompile::{Error, Result, bad};

const W: &str = "vertex-blend record";

/// The record's file name in the map folder.
pub const RECORD: &str = "vertex_blend.json";

/// `vertex_blend.json`: material name -> the (GLB relative to the map folder, primitive) pairs drawing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub materials: BTreeMap<String, Vec<(String, usize)>>,
}

impl Record {
    /// The record from the bytes of `vertex_blend.json`.
    pub fn parse(bytes: &[u8]) -> Result<Record> {
        let v: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| bad(W, 0, format!("{RECORD}: {e}")))?;
        if v["format"] != 1 {
            return Err(bad(W, 0, format!("{RECORD}: format {} is not 1", v["format"])));
        }
        let materials = v["materials"]
            .as_object()
            .ok_or_else(|| bad(W, 0, format!("{RECORD}: no materials object")))?
            .iter()
            .map(|(name, pairs)| {
                let pairs = pairs
                    .as_array()
                    .ok_or_else(|| bad(W, 0, format!("{RECORD}: {name} is not a list")))?
                    .iter()
                    .map(|p| match (p[0].as_str(), p[1].as_u64()) {
                        (Some(mesh), Some(prim)) => Ok((mesh.to_owned(), prim as usize)),
                        _ => Err(bad(
                            W,
                            0,
                            format!("{RECORD}: {name}: {p} is not [mesh, primitive]"),
                        )),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((name.clone(), pairs))
            })
            .collect::<Result<_>>()?;
        Ok(Record { materials })
    }
}

/// An object of `map.json` with a mesh: its name, the GLB it draws (relative to the map folder) and its transform's
/// numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct MapObject {
    pub name: String,
    pub mesh: String,
    pub transform: Vec<f64>,
}

/// A map folder's vertex-blend input: its record, the weights of every recorded primitive and the map's objects.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendMap {
    pub record: Record,
    /// (recorded GLB, primitive) -> the `_RESKATE_BLEND` weights of the vertices the primitive draws, in vertex order.
    pub weights: BTreeMap<(String, usize), Vec<f32>>,
    /// `map.json`'s objects with a mesh, in its order.
    pub objects: Vec<MapObject>,
}

/// Reads map folder `map`'s vertex-blend input; `None` when the map has no vertex-blend material (no record).
pub fn read(map: &Path) -> Result<Option<BlendMap>> {
    let path = map.join(RECORD);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::Io { path, source }),
    };
    let record = Record::parse(&bytes)?;
    let mut glbs: BTreeMap<&str, MapGlb> = BTreeMap::new();
    let mut weights = BTreeMap::new();
    for (mesh, primitive) in record.materials.values().flatten() {
        if !glbs.contains_key(mesh.as_str()) {
            glbs.insert(mesh, load_glb(&map_file(map, mesh)?)?);
        }
        if let Entry::Vacant(e) = weights.entry((mesh.clone(), *primitive)) {
            e.insert(glb_weights(&glbs[mesh.as_str()], *primitive)?);
        }
    }
    let path = map.join("map.json");
    let bytes = std::fs::read(&path).map_err(|source| Error::Io { path, source })?;
    let objects = objects(&bytes)?;
    Ok(Some(BlendMap {
        record,
        weights,
        objects,
    }))
}

/// The objects with a name and a mesh of `map.json`'s bytes.
fn objects(bytes: &[u8]) -> Result<Vec<MapObject>> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| bad(W, 0, format!("map.json: {e}")))?;
    let mut out = Vec::new();
    for o in v["objects"].as_array().into_iter().flatten() {
        let (Some(name), Some(mesh)) = (o["name"].as_str(), o["mesh"].as_str()) else {
            continue;
        };
        let transform = o["transform"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                x.as_f64().ok_or_else(|| {
                    bad(
                        W,
                        0,
                        format!("map.json: object {name}: transform {x} is not a number"),
                    )
                })
            })
            .collect::<Result<_>>()?;
        out.push(MapObject {
            name: name.to_owned(),
            mesh: mesh.to_owned(),
            transform,
        });
    }
    Ok(out)
}

/// A recorded file of map folder `map`: `rel` must be a relative path with forward slashes that stays inside the
/// folder and goes through no link (as the scene reader requires of its files).
fn map_file(map: &Path, rel: &str) -> Result<PathBuf> {
    let refuse = |why: &str| bad(W, 0, format!("{RECORD}: '{rel}' {why}"));
    if rel.is_empty() || rel.contains('\\') || rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') {
        return Err(refuse("is not a relative path with forward slashes"));
    }
    let mut parts: Vec<&str> = Vec::new();
    for c in Path::new(rel).components() {
        match c {
            std::path::Component::Normal(x) => parts.push(x.to_str().unwrap_or_default()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(refuse("is outside the map folder"));
                }
            }
            _ => return Err(refuse("is not a relative path with forward slashes")),
        }
    }
    let mut full = map.to_path_buf();
    for p in &parts {
        full.push(p);
        if std::fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(refuse("goes through a link: the map folder must hold its files"));
        }
    }
    Ok(full)
}

/// A map GLB, read and parsed once.
struct MapGlb {
    path: PathBuf,
    gltf: primase_gltf::Gltf,
}

/// Reads and parses the map GLB at `path`, which must hold the writer's one mesh.
fn load_glb(path: &Path) -> Result<MapGlb> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let fail = |d: String| bad(W, 0, format!("{}: {d}", path.display()));
    let gltf = primase_gltf::Gltf::from_glb(&bytes).map_err(|e| fail(e.to_string()))?;
    // the writer writes one mesh per GLB, and the record's primitive indices are that mesh's
    let meshes = gltf.json()["meshes"].as_array().map_or(0, Vec::len);
    if meshes != 1 {
        return Err(fail(format!("{meshes} meshes, not the writer's one")));
    }
    Ok(MapGlb {
        path: path.to_path_buf(),
        gltf,
    })
}

/// The `_RESKATE_BLEND` weights of the vertices `primitive` of a map GLB draws (all of them without indices), in
/// vertex order.
fn glb_weights(glb: &MapGlb, primitive: usize) -> Result<Vec<f32>> {
    let (g, path) = (&glb.gltf, &glb.path);
    let fail = |d: String| bad(W, 0, format!("{}: {d}", path.display()));
    let j = g.json();
    let attr = &j["meshes"][0]["primitives"][primitive]["attributes"]["_RESKATE_BLEND"];
    let a = &j["accessors"][attr
        .as_u64()
        .ok_or_else(|| fail(format!("primitive {primitive} has no _RESKATE_BLEND")))?
        as usize];
    if a["componentType"] != 5126 || a["type"] != "SCALAR" {
        return Err(fail(format!(
            "primitive {primitive}: _RESKATE_BLEND is not a float scalar"
        )));
    }
    let view = a["bufferView"]
        .as_u64()
        .ok_or_else(|| fail("_RESKATE_BLEND has no buffer view".into()))? as usize;
    if j["bufferViews"][view].get("byteStride").is_some_and(|s| s != 4) {
        return Err(fail("_RESKATE_BLEND is interleaved".into()));
    }
    let data = g.buffer_view(view).map_err(|e| fail(e.to_string()))?;
    let past = |e: String| fail(format!("_RESKATE_BLEND runs past its buffer view: {e}"));
    let int = |k: &str| usize::try_from(a[k].as_u64().unwrap_or(0)).map_err(|e| past(e.to_string()));
    let span = primase_bytes::checked_span(int("byteOffset")?, int("count")?, 4, data.len())
        .map_err(|e| past(e.to_string()))?;
    let weights: Vec<f32> = data[span]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    // Studio compiles only the vertices the primitive's triangles use; a lowered overlay shares its source
    // primitive's vertices, and the unused ones may carry weights of other triangles
    let Some(ix) = j["meshes"][0]["primitives"][primitive]["indices"].as_u64() else {
        return Ok(weights);
    };
    let ia = &j["accessors"][ix as usize];
    let size = match ia["componentType"].as_u64() {
        Some(5121) => 1,
        Some(5123) => 2,
        Some(5125) => 4,
        _ => {
            return Err(fail(format!(
                "primitive {primitive}: indices are not unsigned integers"
            )));
        }
    };
    let iview = ia["bufferView"]
        .as_u64()
        .ok_or_else(|| fail("indices have no buffer view".into()))? as usize;
    if j["bufferViews"][iview].get("byteStride").is_some() {
        return Err(fail("indices are interleaved".into()));
    }
    let idata = g.buffer_view(iview).map_err(|e| fail(e.to_string()))?;
    let iint = |k: &str| usize::try_from(ia[k].as_u64().unwrap_or(0)).map_err(|e| past(e.to_string()));
    let ispan = primase_bytes::checked_span(iint("byteOffset")?, iint("count")?, size, idata.len())
        .map_err(|e| fail(format!("indices run past their buffer view: {e}")))?;
    let mut used = vec![false; weights.len()];
    for c in idata[ispan].chunks_exact(size) {
        let i = match size {
            1 => usize::from(c[0]),
            2 => usize::from(u16::from_le_bytes([c[0], c[1]])),
            _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize,
        };
        *used.get_mut(i).ok_or_else(|| {
            fail(format!(
                "primitive {primitive}: index {i} past its {} vertices",
                weights.len()
            ))
        })? = true;
    }
    Ok(weights
        .into_iter()
        .zip(used)
        .filter(|(_, u)| *u)
        .map(|(w, _)| w)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GLB with `meshes` copies of one primitive whose `_RESKATE_BLEND` accessor has `count` floats, over a buffer
    /// of 0.25 and 0.5.
    fn glb_file(name: &str, meshes: usize, count: u64) -> PathBuf {
        let mesh = serde_json::json!({"primitives": [{"attributes": {"_RESKATE_BLEND": 0}}]});
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 8}],
            "bufferViews": [{"buffer": 0, "byteLength": 8}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": count, "type": "SCALAR"}],
            "meshes": vec![mesh; meshes],
        });
        let bin: Vec<u8> = [0.25f32, 0.5].iter().flat_map(|w| w.to_le_bytes()).collect();
        let bytes = primase_gltf::glb::assemble(json.to_string().as_bytes(), &bin).unwrap();
        let path = std::env::temp_dir().join(format!("vbrecord-{name}-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn weights_at(path: &Path, primitive: usize) -> Result<Vec<f32>> {
        glb_weights(&load_glb(path)?, primitive)
    }

    #[test]
    fn record_paths_stay_in_the_map_folder() {
        let map = std::env::temp_dir();
        assert_eq!(
            map_file(&map, "meshes/a.glb").unwrap(),
            map.join("meshes").join("a.glb")
        );
        assert_eq!(map_file(&map, "meshes/../b.glb").unwrap(), map.join("b.glb"));
        for (rel, why) in [
            ("../a.glb", "outside the map folder"),
            ("meshes/../../a.glb", "outside the map folder"),
            ("/a.glb", "not a relative path"),
            ("C:/a.glb", "not a relative path"),
            ("meshes\\a.glb", "not a relative path"),
            ("", "not a relative path"),
        ] {
            let e = map_file(&map, rel).unwrap_err().to_string();
            assert!(e.contains(why), "{rel}: {e}");
        }
    }

    #[test]
    fn glb_weights_are_read_strictly() {
        let ok = glb_file("ok", 1, 2);
        assert_eq!(weights_at(&ok, 0).unwrap(), vec![0.25, 0.5]);
        let two = glb_file("two", 2, 2);
        let e = weights_at(&two, 0).unwrap_err().to_string();
        assert!(e.contains("2 meshes"), "{e}");
        // a count whose byte length overflows is an error, not a panic
        let huge = glb_file("huge", 1, u64::MAX / 2);
        let e = weights_at(&huge, 0).unwrap_err().to_string();
        assert!(e.contains("runs past its buffer view"), "{e}");
        for p in [ok, two, huge] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// A GLB whose primitive has four weights and the u16 indices `idx`.
    fn indexed_glb(name: &str, idx: &[u16]) -> PathBuf {
        let mesh = serde_json::json!({"primitives": [{"attributes": {"_RESKATE_BLEND": 0}, "indices": 1}]});
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 16 + 2 * idx.len()}],
            "bufferViews": [{"buffer": 0, "byteLength": 16}, {"buffer": 0, "byteOffset": 16, "byteLength": 2 * idx.len()}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "SCALAR"},
                {"bufferView": 1, "componentType": 5123, "count": idx.len(), "type": "SCALAR"},
            ],
            "meshes": [mesh],
        });
        let mut bin: Vec<u8> = [0.1f32, 0.2, 0.3, 0.4]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        bin.extend(idx.iter().flat_map(|i| i.to_le_bytes()));
        let bytes = primase_gltf::glb::assemble(json.to_string().as_bytes(), &bin).unwrap();
        let path = std::env::temp_dir().join(format!("vbrecord-{name}-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn only_the_vertices_a_primitive_draws_give_its_weights() {
        // vertex 2 is not drawn (an overlay sharing its source primitive's vertices): its weight is not the
        // primitive's, since Studio compiles only drawn vertices
        let p = indexed_glb("idx", &[0, 1, 3]);
        assert_eq!(weights_at(&p, 0).unwrap(), vec![0.1, 0.2, 0.4]);
        let bad = indexed_glb("idx-bad", &[0, 1, 7]);
        let e = weights_at(&bad, 0).unwrap_err().to_string();
        assert!(e.contains("index 7 past its 4 vertices"), "{e}");
        for p in [p, bad] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// A map folder: its record, GLB weights and objects read into memory; no record, no input; a transform that is
    /// not numbers refused.
    #[test]
    fn a_map_folder_reads_into_memory() {
        let map = std::env::temp_dir().join(format!("vbrecord-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&map);
        std::fs::create_dir_all(map.join("meshes")).unwrap();
        assert_eq!(read(&map).unwrap(), None);
        std::fs::rename(glb_file("map", 1, 2), map.join("meshes/t_0.glb")).unwrap();
        std::fs::write(
            map.join(RECORD),
            r#"{"format": 1, "materials": {"decal": [["meshes/t_0.glb", 0]]}}"#,
        )
        .unwrap();
        let t: Vec<f64> = (0..12).map(f64::from).collect();
        let objects = serde_json::json!({"objects": [
            {"name": "t_0", "mesh": "meshes/t_0.glb", "transform": t},
            {"name": "spawn"},
        ]});
        std::fs::write(map.join("map.json"), objects.to_string()).unwrap();
        let input = read(&map).unwrap().unwrap();
        assert_eq!(
            input.record.materials["decal"],
            vec![("meshes/t_0.glb".to_owned(), 0)]
        );
        assert_eq!(input.weights[&("meshes/t_0.glb".to_owned(), 0)], vec![0.25, 0.5]);
        assert_eq!(
            input.objects,
            vec![MapObject {
                name: "t_0".into(),
                mesh: "meshes/t_0.glb".into(),
                transform: t
            }]
        );
        let bad =
            serde_json::json!({"objects": [{"name": "t_0", "mesh": "meshes/t_0.glb", "transform": ["x"]}]});
        std::fs::write(map.join("map.json"), bad.to_string()).unwrap();
        let e = read(&map).unwrap_err().to_string();
        assert!(e.contains("is not a number"), "{e}");
        let _ = std::fs::remove_dir_all(&map);
    }
}
