//! Shared images (format 2.0): a tile GLB's embedded images move out into files named by the SHA256 of their bytes,
//! referenced by URI, and the GLB's binary chunk is compacted to what remains (geometry and its views).
use crate::{Error, Result};
use primase_gltf::glb::Document;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Image files by name (`images/<sha256>.<ext>`), with their bytes.
pub type ImageFiles = Vec<(String, Vec<u8>)>;

/// The folder, beside `tiles.json`, holding a tile set's image files.
pub const IMAGE_DIR: &str = "images";

const MESHOPT: &str = "EXT_meshopt_compression";

fn invalid(m: &str) -> Error {
    Error::Invalid(format!("image externalization: {m}"))
}

fn number(v: &Value) -> Result<usize> {
    v.as_u64()
        .map(|n| n as usize)
        .ok_or_else(|| invalid("expected a number"))
}

/// `images/<sha256>.<ext>` for an image's bytes, by its MIME type.
pub fn image_name(mime: &str, bytes: &[u8]) -> Result<String> {
    let ext = match mime {
        "image/png" => "png",
        "image/ktx2" => "ktx2",
        "image/jpeg" => "jpg",
        other => return Err(invalid(&format!("unsupported image type {other}"))),
    };
    let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{IMAGE_DIR}/{hash}.{ext}"))
}

/// Move every embedded image of `glb` out: each becomes `uri: images/<sha256>.<ext>` (its MIME type kept), its
/// bufferView is dropped and the binary chunk holds only the remaining views, in their order, 4-byte aligned. Views
/// keep their meaning: a meshopt view's compressed range (its extension's) and a plain view's range are both moved.
/// Returns the GLB and the images it references, in image order, each once.
pub fn externalize(input: Document) -> Result<(Document, ImageFiles)> {
    let Some(images) = input.json.get("images").and_then(Value::as_array).cloned() else {
        return Ok((input, Vec::new()));
    };
    let Document { json: mut doc, bin } = input;
    let bin = &bin[..];
    let views = doc["bufferViews"].as_array().cloned().unwrap_or_default();
    let mut image_views = BTreeMap::new();
    let mut files: ImageFiles = Vec::new();
    let mut new_images = Vec::with_capacity(images.len());
    for image in images {
        let Some(v) = image.get("bufferView") else {
            new_images.push(image);
            continue;
        };
        let v = number(v)?;
        let view = views.get(v).ok_or_else(|| invalid("image view out of range"))?;
        if view["buffer"] != 0 || view.get("extensions").is_some() {
            return Err(invalid("an image view outside the binary chunk"));
        }
        let start = number(view.get("byteOffset").unwrap_or(&json!(0)))?;
        let length = number(&view["byteLength"])?;
        let bytes = bin
            .get(start..start + length)
            .ok_or_else(|| invalid("image bytes outside the binary chunk"))?
            .to_vec();
        let mime = image["mimeType"]
            .as_str()
            .ok_or_else(|| invalid("embedded image without a MIME type"))?;
        let name = image_name(mime, &bytes)?;
        image_views.insert(v, ());
        let mut image = image.clone();
        let o = image.as_object_mut().expect("image object");
        o.remove("bufferView");
        o.insert("uri".into(), json!(name));
        new_images.push(image);
        if !files.iter().any(|(n, _)| *n == name) {
            files.push((name, bytes));
        }
    }
    // the remaining views, renumbered, their buffer-0 ranges copied in order
    let mut remap = BTreeMap::new();
    let mut kept = Vec::new();
    let mut out = Vec::new();
    for (i, view) in views.into_iter().enumerate() {
        if image_views.contains_key(&i) {
            continue;
        }
        let mut view = view;
        let compressed = view["extensions"][MESHOPT].is_object();
        let (range_owner_is_ext, buffer) = if compressed {
            (true, view["extensions"][MESHOPT]["buffer"].clone())
        } else {
            (false, view["buffer"].clone())
        };
        if buffer == 0 {
            let at = if range_owner_is_ext {
                &view["extensions"][MESHOPT]
            } else {
                &view
            };
            let start = number(at.get("byteOffset").unwrap_or(&json!(0)))?;
            let length = number(&at["byteLength"])?;
            let bytes = bin
                .get(start..start + length)
                .ok_or_else(|| invalid("view bytes outside the binary chunk"))?;
            while out.len() % 4 != 0 {
                out.push(0);
            }
            let offset = out.len();
            out.extend_from_slice(bytes);
            let at = if range_owner_is_ext {
                &mut view["extensions"][MESHOPT]
            } else {
                &mut view
            };
            at["byteOffset"] = json!(offset);
        }
        remap.insert(i, kept.len());
        kept.push(view);
    }
    for accessor in doc["accessors"].as_array_mut().into_iter().flatten() {
        if let Some(v) = accessor.get("bufferView") {
            let v = number(v)?;
            accessor["bufferView"] = json!(
                remap
                    .get(&v)
                    .ok_or_else(|| invalid("an accessor on an image view"))?
            );
        }
    }
    doc["images"] = json!(new_images);
    doc["bufferViews"] = json!(kept);
    if let Some(buffer) = doc["buffers"].get_mut(0) {
        buffer["byteLength"] = json!(out.len());
    }
    Ok((Document::new(doc, out), files))
}
