use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const EXT: &str = "EXT_meshopt_compression";
fn err(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}
fn number(v: &Value) -> usize {
    v.as_u64().expect("writer integer") as usize
}
fn align(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}
fn stride(a: &Value) -> usize {
    let channels = match a["type"].as_str().expect("writer accessor type") {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => unreachable!("writer accessor type"),
    };
    channels
        * match number(&a["componentType"]) {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => unreachable!("writer component"),
        }
}
fn data(doc: &Value, bin: &[u8], index: usize) -> Vec<u8> {
    let a = &doc["accessors"][index];
    let view = &doc["bufferViews"][number(&a["bufferView"])];
    let offset =
        view["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let width = stride(a);
    let step = view["byteStride"].as_u64().map_or(width, |v| v as usize);
    (0..number(&a["count"]))
        .flat_map(|i| bin[offset + i * step..offset + i * step + width].iter().copied())
        .collect()
}
fn floats(doc: &Value, bin: &[u8], index: usize) -> Vec<f32> {
    data(doc, bin, index)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect()
}
fn extension(doc: &mut Value, name: &str) {
    for key in ["extensionsUsed", "extensionsRequired"] {
        if !doc[key].is_array() {
            doc[key] = json!([]);
        }
        if !doc[key].as_array().expect("array").iter().any(|v| v == name) {
            doc[key].as_array_mut().expect("array").push(json!(name));
        }
    }
}
fn encode<const N: usize>(bytes: &[u8]) -> Result<Vec<u8>>
where
    [u8; N]: Default,
{
    let vertices: Vec<[u8; N]> = bytes.as_chunks::<N>().0.to_vec();
    let encoded = meshopt::encode_vertex_buffer(&vertices).map_err(err)?;
    let decoded = meshopt::decode_vertex_buffer::<[u8; N]>(&encoded, vertices.len()).map_err(err)?;
    if decoded != vertices {
        return Err(err("meshopt decoded stream differs from quantized input"));
    }
    Ok(encoded)
}

/// Uniform, origin-centred signed-16 positions; normal/tangent signed normalized
/// 16-bit; unit UVs unsigned normalized 16-bit. Uniform decoding preserves normals.
pub(super) fn apply(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let chunks = primase_gltf::glb::parse(&bytes)?;
    let mut doc: Value = serde_json::from_slice(chunks.json)?;
    let mut source = chunks.bin.unwrap_or(&[]).to_vec();
    let meshes = doc["meshes"].as_array().expect("writer meshes").clone();
    let mut position_scale = BTreeMap::<usize, f32>::new();
    let mut kinds = BTreeMap::<usize, String>::new();
    let mut mesh_scales = Vec::new();
    for mesh in &meshes {
        let mut max = 0f32;
        for p in mesh["primitives"].as_array().expect("primitives") {
            for (name, a) in p["attributes"].as_object().expect("attributes") {
                kinds.insert(number(a), name.clone());
            }
            for v in floats(&doc, &source, number(&p["attributes"]["POSITION"])) {
                max = max.max(v.abs());
            }
        }
        // Use one positive uniform scale for every primitive in this mesh.
        let scale = if max > 0. {
            (max as f64 / 32767.) as f32
        } else {
            1.
        };
        if !scale.is_finite() || scale == 0. {
            return Err(err("position quantization scale outside f32"));
        }
        for p in mesh["primitives"].as_array().expect("primitives") {
            let a = number(&p["attributes"]["POSITION"]);
            if position_scale.insert(a, scale).is_some_and(|old| old != scale) {
                return Err(err("shared position accessor has incompatible mesh bounds"));
            }
        }
        mesh_scales.push(scale);
    }
    // Node * instance * uniformScale. For instancing the scale is multiplied into
    // the instance SCALE stream; plain nodes compose it into their local basis.
    let mut scaled_accessors = BTreeMap::new();
    for node in doc["nodes"].as_array_mut().expect("nodes") {
        let Some(mesh) = node["mesh"].as_u64() else {
            continue;
        };
        let scale = mesh_scales[mesh as usize];
        if let Some(a) = node["extensions"]["EXT_mesh_gpu_instancing"]["attributes"]["SCALE"].as_u64() {
            scaled_accessors.insert(a as usize, scale);
        } else if let Some(matrix) = node.get_mut("matrix").and_then(Value::as_array_mut) {
            for column in 0..3 {
                for row in 0..3 {
                    let k = column * 4 + row;
                    matrix[k] = json!(matrix[k].as_f64().expect("matrix") * scale as f64);
                }
            }
        } else {
            let old = node["scale"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![json!(1.); 3]);
            node["scale"] = json!(
                old.iter()
                    .map(|v| v.as_f64().expect("scale") * scale as f64)
                    .collect::<Vec<_>>()
            );
        }
    }
    for (a, scale) in scaled_accessors {
        let view = &doc["bufferViews"][number(&doc["accessors"][a]["bufferView"])];
        let offset = number(&view["byteOffset"]);
        for (i, v) in floats(&doc, &source, a).into_iter().enumerate() {
            let value = v * scale;
            if !value.is_finite() {
                return Err(err("instance quantization scale overflow"));
            }
            source[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    let old = doc.clone();
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut fallback = 0usize;
    let mut quantized = false;
    let mut max_error = 0f64;
    for a in 0..old["accessors"].as_array().expect("accessors").len() {
        let original = &old["accessors"][a];
        let count = number(&original["count"]);
        let old_view = &old["bufferViews"][number(&original["bufferView"])];
        let is_vertex = old_view["target"] == 34962;
        let name = kinds.get(&a).map(String::as_str).unwrap_or("");
        let mut raw = data(&old, &source, a);
        let mut step = stride(original);
        let mut accessor = original.clone();
        accessor.as_object_mut().expect("accessor").remove("byteOffset");
        if original["componentType"] == 5126
            && (name == "POSITION" || name == "NORMAL" || name == "TANGENT" || name.starts_with("TEXCOORD_"))
        {
            let values = floats(&old, &source, a);
            let uv = name.starts_with("TEXCOORD_");
            if !uv || values.iter().all(|v| (0. ..=1.).contains(v)) {
                let components = step / 4;
                step = (components * 2).div_ceil(4) * 4;
                raw.clear();
                let mut min = vec![i32::MAX; components];
                let mut max = vec![i32::MIN; components];
                for vertex in values.chunks_exact(components) {
                    for (k, &v) in vertex.iter().enumerate() {
                        let q = if name == "POSITION" {
                            let scale = position_scale[&a];
                            let q = (v as f64 / scale as f64).round().clamp(-32767., 32767.) as i32;
                            max_error = max_error.max((q as f64 * scale as f64 - v as f64).abs());
                            q
                        } else if uv {
                            (v as f64 * 65535.).round() as i32
                        } else {
                            (v.clamp(-1., 1.) as f64 * 32767.).round() as i32
                        };
                        min[k] = min[k].min(q);
                        max[k] = max[k].max(q);
                        raw.extend_from_slice(&(q as u16).to_le_bytes());
                    }
                    raw.resize(raw.len() + step - components * 2, 0);
                }
                accessor["componentType"] = json!(if uv { 5123 } else { 5122 });
                accessor["normalized"] = json!(name != "POSITION");
                if name == "POSITION" {
                    accessor["min"] = json!(min);
                    accessor["max"] = json!(max);
                }
                quantized = true;
            }
        }
        align(&mut bin);
        let offset = bin.len();
        let mut view;
        if is_vertex {
            let encoded = match step {
                4 => encode::<4>(&raw),
                8 => encode::<8>(&raw),
                12 => encode::<12>(&raw),
                16 => encode::<16>(&raw),
                _ => return Err(err("unsupported vertex stride")),
            }?;
            bin.extend_from_slice(&encoded);
            view = json!({"buffer":1,"byteOffset":fallback,"byteLength":raw.len(),"byteStride":step,"target":34962,"extensions":{EXT:{"buffer":0,"byteOffset":offset,"byteLength":encoded.len(),"byteStride":step,"count":count,"mode":"ATTRIBUTES"}}});
            fallback += raw.len();
        } else {
            bin.extend_from_slice(&raw);
            view = json!({"buffer":0,"byteOffset":offset,"byteLength":raw.len()});
            if let Some(target) = old_view.get("target") {
                view["target"] = target.clone();
            }
        }
        accessor["bufferView"] = json!(views.len());
        doc["accessors"][a] = accessor;
        views.push(view);
    }
    if let Some(images) = doc.get_mut("images").and_then(Value::as_array_mut) {
        for image in images {
            let view = &old["bufferViews"][number(&image["bufferView"])];
            let start = number(&view["byteOffset"]);
            let length = number(&view["byteLength"]);
            align(&mut bin);
            image["bufferView"] = json!(views.len());
            views.push(json!({"buffer":0,"byteOffset":bin.len(),"byteLength":length}));
            bin.extend_from_slice(&source[start..start + length]);
        }
    }
    doc["bufferViews"] = json!(views);
    doc["buffers"] =
        json!([{ "byteLength":bin.len() }, {"byteLength":fallback,"extensions":{EXT:{"fallback":true}}}]);
    extension(&mut doc, EXT);
    if quantized {
        extension(&mut doc, "KHR_mesh_quantization");
    }
    let max_step = mesh_scales.iter().copied().fold(0f32, f32::max);
    doc["asset"]["extras"]["primase.quantization"] = json!({"position_max_local_error":max_error,"position_max_local_step":max_step,"position_bits":16,"normal_bits":16,"uv_bits":16});
    Ok(primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?)
}
