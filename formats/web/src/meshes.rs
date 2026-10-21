//! Shared mesh geometry (format 3.0, [`crate::Options::mesh_files`]): the geometry of each glTF mesh of a tile GLB
//! moves out into a buffer file of its own, named by the SHA256 of its bytes (`meshes/<sha256>.bin`) and referenced by
//! the buffer's `uri`, so the tiles placing the same prototype reference one file and a consumer fetches each mesh when
//! it draws it. Meshes that share an accessor (a prototype's material variants) share a file. The views keep their
//! meaning: a view's range (a meshopt view's compressed range, its extension's) moves to the file, its accessors
//! unchanged; what the tile itself holds (instance streams) stays in the GLB's binary chunk.
use crate::{Error, Result};
use primase_glb::Document;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Mesh buffer files by name (`meshes/<sha256>.bin`), with their bytes.
pub type MeshFiles = Vec<(String, Vec<u8>)>;

/// The folder, beside `tiles.json`, holding a tile set's mesh buffer files.
pub const MESH_DIR: &str = "meshes";

const MESHOPT: &str = "EXT_meshopt_compression";

fn invalid(m: &str) -> Error {
    Error::Invalid(format!("mesh externalization: {m}"))
}

fn number(v: &Value) -> Result<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .ok_or_else(|| invalid("expected a number"))
}

/// `meshes/<sha256>.bin` for a mesh buffer's bytes.
pub fn mesh_name(bytes: &[u8]) -> String {
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    format!("{MESH_DIR}/{hash}.bin")
}

/// The object holding view `view`'s range: its meshopt extension when compressed, else the view.
fn range_owner(view: &mut Value) -> &mut Value {
    if view["extensions"][MESHOPT].is_object() {
        &mut view["extensions"][MESHOPT]
    } else {
        view
    }
}

fn root(group: &mut [usize], mut m: usize) -> usize {
    while group[m] != m {
        group[m] = group[group[m]];
        m = group[m];
    }
    m
}

/// Move each mesh's geometry views out of `input`'s binary chunk into its mesh buffer file (see the module docs).
/// Returns the GLB and the mesh files it references, in buffer order, each once. A tile without meshes is returned as
/// it is.
pub fn externalize(input: Document) -> Result<(Document, MeshFiles)> {
    let meshes = input.json["meshes"].as_array().map_or(0, Vec::len);
    if meshes == 0 {
        return Ok((input, Vec::new()));
    }
    let Document { json: mut doc, bin } = input;
    let accessor_view = |doc: &Value, a: &Value| -> Result<Option<usize>> {
        let a = number(a)?;
        doc["accessors"]
            .get(a)
            .ok_or_else(|| invalid("accessor out of range"))?
            .get("bufferView")
            .map(number)
            .transpose()
    };
    // each mesh's views, and the meshes joined by a shared view
    let mut group: Vec<usize> = (0..meshes).collect();
    let mut first_mesh = BTreeMap::<usize, usize>::new();
    let mut views_of = vec![BTreeSet::new(); meshes];
    for (m, views) in views_of.iter_mut().enumerate() {
        for p in doc["meshes"][m]["primitives"].as_array().into_iter().flatten() {
            let attributes = p["attributes"]
                .as_object()
                .ok_or_else(|| invalid("primitive attributes"))?;
            for a in attributes.values().chain(p.get("indices")) {
                if let Some(v) = accessor_view(&doc, a)? {
                    views.insert(v);
                }
            }
        }
        for &v in views.iter() {
            let other = *first_mesh.entry(v).or_insert(m);
            let (x, y) = (root(&mut group, m), root(&mut group, other));
            group[x.max(y)] = x.min(y);
        }
    }
    let moved: BTreeSet<usize> = first_mesh.keys().copied().collect();
    // the tile's own accessors (instance streams) keep their views in the binary chunk
    let meshes_accessors: BTreeSet<usize> = doc["meshes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|m| m["primitives"].as_array().into_iter().flatten())
        .flat_map(|p| {
            p["attributes"]
                .as_object()
                .into_iter()
                .flat_map(|a| a.values())
                .chain(p.get("indices"))
                .filter_map(Value::as_u64)
                .map(|a| a as usize)
        })
        .collect();
    for (a, accessor) in doc["accessors"].as_array().into_iter().flatten().enumerate() {
        if !meshes_accessors.contains(&a)
            && let Some(v) = accessor.get("bufferView")
            && moved.contains(&number(v)?)
        {
            return Err(invalid(
                "a mesh's view is also read by an accessor outside the meshes",
            ));
        }
    }
    let mut groups = BTreeMap::<usize, BTreeSet<usize>>::new();
    for (m, views) in views_of.into_iter().enumerate() {
        groups.entry(root(&mut group, m)).or_default().extend(views);
    }
    let range = |view: &mut Value| -> Result<(usize, usize)> {
        let at = range_owner(view);
        if at["buffer"] != 0 {
            return Err(invalid("a mesh view outside the binary chunk"));
        }
        let start = number(at.get("byteOffset").unwrap_or(&json!(0)))?;
        Ok((start, number(&at["byteLength"])?))
    };
    let mut views = doc["bufferViews"].as_array().cloned().unwrap_or_default();
    // each group's file: its views' ranges in view order, 4-byte aligned
    let mut files: MeshFiles = Vec::new();
    let mut file_of_view = BTreeMap::<usize, (usize, usize)>::new();
    for members in groups.values() {
        let mut bytes = Vec::new();
        let mut at = Vec::with_capacity(members.len());
        for &v in members {
            let view = views.get_mut(v).ok_or_else(|| invalid("view out of range"))?;
            let (start, length) = range(view)?;
            let data = bin
                .get(start..start + length)
                .ok_or_else(|| invalid("view bytes outside the binary chunk"))?;
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            at.push((v, bytes.len()));
            bytes.extend_from_slice(data);
        }
        let name = mesh_name(&bytes);
        let file = match files.iter().position(|(n, _)| *n == name) {
            Some(i) => i,
            None => {
                files.push((name, bytes));
                files.len() - 1
            }
        };
        for (v, offset) in at {
            file_of_view.insert(v, (file, offset));
        }
    }
    // what stays in the binary chunk, compacted in view order
    let mut kept = Vec::new();
    for (i, view) in views.iter_mut().enumerate() {
        if file_of_view.contains_key(&i) {
            continue;
        }
        let owner = range_owner(view);
        if owner["buffer"] != 0 {
            continue; // a meshopt fallback range, or another buffer's
        }
        let start = number(owner.get("byteOffset").unwrap_or(&json!(0)))?;
        let length = number(&owner["byteLength"])?;
        let data = bin
            .get(start..start + length)
            .ok_or_else(|| invalid("view bytes outside the binary chunk"))?;
        kept.resize(kept.len().next_multiple_of(4), 0);
        owner["byteOffset"] = json!(kept.len());
        kept.extend_from_slice(data);
    }
    // the buffers: the binary chunk (when anything stays in it: buffer 0 without a uri is the GLB's chunk), the
    // others as they were (a meshopt fallback), then the mesh files
    let old = doc["buffers"].as_array().cloned().unwrap_or_default();
    let mut buffers = Vec::new();
    let mut index = BTreeMap::<usize, usize>::new();
    if !kept.is_empty() {
        index.insert(0, 0);
        buffers.push(json!({"byteLength": kept.len()}));
    }
    let mut others = Vec::new();
    for (i, b) in old.into_iter().enumerate().skip(1) {
        others.push((i, b));
    }
    let files_first = kept.is_empty();
    let first_file = if files_first { 0 } else { 1 + others.len() };
    let place_others = |buffers: &mut Vec<Value>, index: &mut BTreeMap<usize, usize>| {
        for (i, b) in &others {
            index.insert(*i, buffers.len());
            buffers.push(b.clone());
        }
    };
    if !files_first {
        place_others(&mut buffers, &mut index);
    }
    for (name, bytes) in &files {
        buffers.push(json!({"uri": name, "byteLength": bytes.len()}));
    }
    if files_first {
        place_others(&mut buffers, &mut index);
    }
    for (i, view) in views.iter_mut().enumerate() {
        if let Some(&(file, offset)) = file_of_view.get(&i) {
            let owner = range_owner(view);
            owner["buffer"] = json!(first_file + file);
            owner["byteOffset"] = json!(offset);
        }
        // every other buffer reference renumbered: a compressed view's own (its fallback) and a range that stayed
        let renumber = |b: &mut Value, moved_here: bool| -> Result<()> {
            if moved_here {
                return Ok(());
            }
            let old = number(b)?;
            *b = json!(
                index
                    .get(&old)
                    .ok_or_else(|| invalid("a view on a buffer that no longer holds it"))?
            );
            Ok(())
        };
        let compressed = view["extensions"][MESHOPT].is_object();
        let in_file = file_of_view.contains_key(&i);
        if compressed {
            renumber(&mut view["extensions"][MESHOPT]["buffer"], in_file)?;
            renumber(&mut view["buffer"], false)?;
        } else {
            renumber(&mut view["buffer"], in_file)?;
        }
    }
    doc["bufferViews"] = json!(views);
    doc["buffers"] = json!(buffers);
    Ok((Document::new(doc, kept), files))
}
