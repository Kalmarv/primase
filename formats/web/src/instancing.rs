use crate::{Error, Result, require_extension};
use primase_core::{Transform, TransformKind};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
const EXT: &str = "EXT_mesh_gpu_instancing";
#[derive(Clone, Copy, Debug)]
struct Trs {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}
fn decompose(t: Transform) -> Option<Trs> {
    if t.classify() != TransformKind::Regular {
        return None;
    }
    let scale: [f64; 3] = std::array::from_fn(|i| {
        t.cols[i]
            .iter()
            .map(|&v| (v as f64) * (v as f64))
            .sum::<f64>()
            .sqrt()
    });
    if scale.iter().any(|v| !(*v as f32).is_finite()) {
        return None;
    }
    let m: [[f64; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| t.cols[c][r] as f64 / scale[c]));
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
            s / 4.,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1. + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.;
        [
            s / 4.,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[2][1] - m[1][2]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1. + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.;
        [
            (m[0][1] + m[1][0]) / s,
            s / 4.,
            (m[1][2] + m[2][1]) / s,
            (m[0][2] - m[2][0]) / s,
        ]
    } else {
        let s = (1. + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.;
        [
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            s / 4.,
            (m[1][0] - m[0][1]) / s,
        ]
    };
    let len = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let rotation = q.map(|v| (v / len) as f32);
    let [x, y, z, w] = rotation.map(f64::from);
    let rebuilt = [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
        ],
    ];
    if (0..3).any(|r| (0..3).any(|c| (rebuilt[r][c] - m[r][c]).abs() > 1e-6)) {
        return None;
    }
    Some(Trs {
        translation: t.cols[3],
        rotation,
        scale: scale.map(|v| v as f32),
    })
}
fn accessor(
    doc: &mut Value,
    bin: &mut Vec<u8>,
    values: impl Iterator<Item = f32>,
    count: usize,
    components: usize,
) -> usize {
    while !bin.len().is_multiple_of(4) {
        bin.push(0)
    }
    let offset = bin.len();
    for v in values {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let views = doc["bufferViews"].as_array_mut().expect("writer views");
    let view = views.len();
    views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":bin.len()-offset,"target":34962}));
    let accessors = doc["accessors"].as_array_mut().expect("writer accessors");
    let index = accessors.len();
    accessors.push(json!({"bufferView":view,"componentType":5126,"count":count,"type":if components==3{"VEC3"}else{"VEC4"}}));
    index
}
/// Consume the private placement key emitted by the tile writer and instance only
/// positive, non-sheared TRS groups sharing the same resolved glTF mesh.
pub(super) fn apply(
    bytes: Vec<u8>,
    transforms: &BTreeMap<u32, Transform>,
    enabled: bool,
) -> Result<(Vec<u8>, u64)> {
    let chunks = primase_gltf::glb::parse(&bytes)?;
    let mut doc: Value = serde_json::from_slice(chunks.json)?;
    let mut bin = chunks.bin.unwrap_or(&[]).to_vec();
    let length = doc["buffers"][0]["byteLength"].as_u64().unwrap_or(0) as usize;
    bin.truncate(length);
    let mut nodes = doc["nodes"]
        .as_array()
        .ok_or_else(|| Error::Invalid("writer returned no nodes".into()))?
        .clone();
    let mut groups = BTreeMap::<u64, Vec<(usize, u32, Trs)>>::new();
    for (i, node) in nodes.iter_mut().enumerate() {
        let marker = node["extras"]["primase"]["web.placement_key"]["value"]
            .as_u64()
            .ok_or_else(|| Error::Invalid("missing web placement key".into()))? as u32;
        if let Some(extras) = node["extras"]["primase"].as_object_mut() {
            extras.remove("web.placement_key");
        }
        if enabled && let Some(trs) = transforms.get(&marker).copied().and_then(decompose) {
            let mesh = node["mesh"]
                .as_u64()
                .ok_or_else(|| Error::Invalid("mesh node lacks mesh index".into()))?;
            groups.entry(mesh).or_default().push((i, marker, trs));
        }
    }
    let mut removed = BTreeSet::new();
    let mut count = 0;
    for group in groups.values().filter(|g| g.len() > 1) {
        let first = group[0].0;
        let n = group.len();
        let translation = accessor(
            &mut doc,
            &mut bin,
            group.iter().flat_map(|g| g.2.translation),
            n,
            3,
        );
        let rotation = accessor(&mut doc, &mut bin, group.iter().flat_map(|g| g.2.rotation), n, 4);
        let scale = accessor(&mut doc, &mut bin, group.iter().flat_map(|g| g.2.scale), n, 3);
        let instances:Vec<_>=group.iter().map(|(i,key,_)|json!({"entity":key,"name":nodes[*i].get("name"),"extras":nodes[*i].get("extras")})).collect();
        let node = nodes[first].as_object_mut().expect("writer node");
        for key in ["matrix", "translation", "rotation", "scale"] {
            node.remove(key);
        }
        node.insert(
            "extensions".into(),
            json!({EXT:{"attributes":{"TRANSLATION":translation,"ROTATION":rotation,"SCALE":scale}}}),
        );
        node.insert(
            "extras".into(),
            json!({"primase":{"web.instances":{"version":1,"value":instances}}}),
        );
        for &(i, _, _) in &group[1..] {
            removed.insert(i);
        }
        count += n as u64;
    }
    if count > 0 {
        let mut remap = BTreeMap::new();
        let mut kept = Vec::new();
        for (i, node) in nodes.into_iter().enumerate() {
            if !removed.contains(&i) {
                remap.insert(i, kept.len());
                kept.push(node);
            }
        }
        for scene in doc["scenes"]
            .as_array_mut()
            .ok_or_else(|| Error::Invalid("writer returned no scenes".into()))?
        {
            let roots = scene["nodes"]
                .as_array()
                .ok_or_else(|| Error::Invalid("scene has no root list".into()))?;
            scene["nodes"] = Value::Array(
                roots
                    .iter()
                    .filter_map(|v| remap.get(&(v.as_u64()? as usize)).map(|i| json!(i)))
                    .collect(),
            );
        }
        doc["nodes"] = json!(kept);
        require_extension(&mut doc, EXT);
    } else {
        doc["nodes"] = json!(nodes);
    }
    if !bin.is_empty() {
        doc["buffers"][0]["byteLength"] = json!(bin.len());
    }
    Ok((
        primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?,
        count,
    ))
}
