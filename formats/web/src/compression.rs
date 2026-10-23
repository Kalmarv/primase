use crate::{Error, Result, require_extension as extension};
use primase_glb::Document;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const EXT: &str = "EXT_meshopt_compression";
fn err(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}
/// A non-negative integer of the document, or an error naming what it was read as.
fn number(v: &Value) -> Result<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| {
        err(format!(
            "{v} is not a non-negative integer where the writer's document has one"
        ))
    })
}
fn align(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}
fn stride(a: &Value) -> Result<usize> {
    let channels = match a["type"].as_str() {
        Some("SCALAR") => 1,
        Some("VEC2") => 2,
        Some("VEC3") => 3,
        Some("VEC4") => 4,
        other => {
            return Err(err(format!(
                "accessor type {other:?} is not one the writer writes"
            )));
        }
    };
    let component = match number(&a["componentType"])? {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        c => {
            return Err(err(format!(
                "accessor component type {c} is not one the writer writes"
            )));
        }
    };
    Ok(channels * component)
}
fn data(doc: &Value, bin: &[u8], index: usize) -> Result<Vec<u8>> {
    let a = &doc["accessors"][index];
    let view = &doc["bufferViews"][number(&a["bufferView"])?];
    let offset =
        view["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let width = stride(a)?;
    let step = view["byteStride"].as_u64().map_or(width, |v| v as usize);
    let count = number(&a["count"])?;
    let mut out = Vec::with_capacity(count * width);
    for i in 0..count {
        let at = offset + i * step;
        out.extend_from_slice(
            bin.get(at..at + width)
                .ok_or_else(|| err(format!("accessor {index} element {i} runs past the buffer")))?,
        );
    }
    Ok(out)
}
fn floats(doc: &Value, bin: &[u8], index: usize) -> Result<Vec<f32>> {
    Ok(data(doc, bin, index)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect())
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

/// How far (metres, in the world: through each placement) a quantized position may be from where it was. A mesh
/// whose 16-bit positions would move further keeps f32 positions.
pub const POSITION_BUDGET: f64 = 0.000_25;

/// A mesh's quantized positions decode as `centre + step * q` (`q` the signed 16-bit components): the frame each of
/// its placements composes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    centre: [f32; 3],
    step: f32,
}

/// `world = basis * local + translation`, basis by columns, as a placement's stored numbers state it.
#[derive(Clone, Copy, Debug)]
struct Affine {
    basis: [[f64; 3]; 3],
    translation: [f64; 3],
}

impl Affine {
    fn point(&self, p: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|r| self.translation[r] + (0..3).map(|c| self.basis[c][r] * p[c]).sum::<f64>())
    }
    /// A glTF node matrix (column-major).
    fn matrix(m: &[f64; 16]) -> Self {
        Affine {
            basis: std::array::from_fn(|c| std::array::from_fn(|r| m[c * 4 + r])),
            translation: [m[12], m[13], m[14]],
        }
    }
    /// glTF's T * R * S, the rotation a quaternion `[x, y, z, w]` taken as stored (as consumers compose it).
    fn trs(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> Self {
        let [x, y, z, w] = q;
        // rotation columns
        let r = [
            [
                1. - 2. * (y * y + z * z),
                2. * (x * y + z * w),
                2. * (x * z - y * w),
            ],
            [
                2. * (x * y - z * w),
                1. - 2. * (x * x + z * z),
                2. * (y * z + x * w),
            ],
            [
                2. * (x * z + y * w),
                2. * (y * z - x * w),
                1. - 2. * (x * x + y * y),
            ],
        ];
        Affine {
            basis: std::array::from_fn(|c| r[c].map(|v| v * s[c])),
            translation: t,
        }
    }
}

/// Where a mesh is placed, and how its transform is stored there.
#[derive(Clone, Copy, Debug)]
enum Site {
    /// A node's `matrix`.
    Matrix { node: usize },
    /// A node's `translation`, `rotation` and `scale` (each optional).
    Trs { node: usize },
    /// Instance `index` of a node's `EXT_mesh_gpu_instancing` TRANSLATION, ROTATION and SCALE accessors.
    Instance { index: usize, streams: [usize; 3] },
}

/// A placement's stored numbers, as f64.
#[derive(Clone, Copy, Debug)]
enum Stored {
    Matrix([f64; 16]),
    Trs { t: [f64; 3], r: [f64; 4], s: [f64; 3] },
}

impl Stored {
    fn affine(&self) -> Affine {
        match *self {
            Stored::Matrix(m) => Affine::matrix(&m),
            Stored::Trs { t, r, s } => Affine::trs(t, r, s),
        }
    }
    /// The placement composed with `f` (`self * (centre + step * q)`), as it is stored: every number rounded to f32,
    /// as consumers hold transforms (instance streams are f32; node JSON gets the f32's exact value, so a consumer
    /// reading it as f64 holds the same number).
    fn compose(&self, f: Frame) -> Stored {
        let centre = self.affine().point(f.centre.map(f64::from));
        let round = |v: f64| f64::from(v as f32);
        let step = f64::from(f.step);
        match *self {
            Stored::Matrix(m) => Stored::Matrix(std::array::from_fn(|k| match k {
                12..=14 => round(centre[k - 12]),
                _ if k % 4 == 3 => m[k],
                _ => round(m[k] * step),
            })),
            Stored::Trs { r, s, .. } => Stored::Trs {
                t: centre.map(round),
                r,
                s: s.map(|v| round(v * step)),
            },
        }
    }
}

/// A node's transform numbers as the f32 values they are: the glTF writer writes each f32 as its shortest decimal,
/// which read as f64 is not that f32 (up to half an f32 step away: 0.24 mm at 6 km).
fn numbers<const N: usize>(v: &Value, default: [f64; N]) -> Result<[f64; N]> {
    let Some(values) = v.as_array() else {
        return Ok(default);
    };
    if values.len() != N {
        return Err(err("node transform has the wrong number of values"));
    }
    let mut out = default;
    for (o, v) in out.iter_mut().zip(values) {
        let v = v
            .as_f64()
            .ok_or_else(|| err("node transform value is not a number"))?;
        *o = f64::from(v as f32);
    }
    Ok(out)
}

/// The numbers stored at `site`.
fn stored(doc: &Value, bin: &[u8], site: Site) -> Result<Stored> {
    Ok(match site {
        Site::Matrix { node } => Stored::Matrix(numbers(&doc["nodes"][node]["matrix"], [0.; 16])?),
        Site::Trs { node } => {
            let n = &doc["nodes"][node];
            Stored::Trs {
                t: numbers(&n["translation"], [0.; 3])?,
                r: numbers(&n["rotation"], [0., 0., 0., 1.])?,
                s: numbers(&n["scale"], [1.; 3])?,
            }
        }
        Site::Instance { index, streams } => Stored::Trs {
            // each stream holds as many components as its attribute has: 3 translations, 4 rotation, 3 scales
            t: instance_components::<3>(doc, bin, streams[0], index)?,
            r: instance_components::<4>(doc, bin, streams[1], index)?,
            s: instance_components::<3>(doc, bin, streams[2], index)?,
        },
    })
}

/// The `N` components of element `index` of an f32 instance stream, which must hold exactly `N` per element.
fn instance_components<const N: usize>(
    doc: &Value,
    bin: &[u8],
    accessor: usize,
    index: usize,
) -> Result<[f64; N]> {
    let mut out = [0.0; N];
    for (k, x) in out.iter_mut().enumerate() {
        *x = f64::from(instance_value(doc, bin, accessor, index, k)?);
    }
    Ok(out)
}

/// Byte offset of component `k` of element `index` of a tightly packed f32 accessor.
fn element(doc: &Value, accessor: usize, index: usize, k: usize) -> Result<usize> {
    let a = &doc["accessors"][accessor];
    let view = &doc["bufferViews"][number(&a["bufferView"])?];
    let width = stride(a)?;
    Ok(view["byteOffset"].as_u64().unwrap_or(0) as usize
        + a["byteOffset"].as_u64().unwrap_or(0) as usize
        + index * width
        + k * 4)
}

fn instance_value(doc: &Value, bin: &[u8], accessor: usize, index: usize, k: usize) -> Result<f32> {
    let at = element(doc, accessor, index, k)?;
    let bytes = bin.get(at..at + 4).ok_or_else(|| {
        err(format!(
            "instance stream {accessor} element {index} runs past the buffer"
        ))
    })?;
    Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Write `value` at `site` (f32 values, see [`Stored::compose`]): the node's JSON, or the instance streams in `bin`.
fn store(doc: &mut Value, bin: &mut [u8], site: Site, value: &Stored) -> Result<()> {
    match (site, value) {
        (Site::Matrix { node }, Stored::Matrix(m)) => doc["nodes"][node]["matrix"] = json!(m),
        (Site::Trs { node }, Stored::Trs { t, s, .. }) => {
            doc["nodes"][node]["translation"] = json!(t);
            doc["nodes"][node]["scale"] = json!(s);
        }
        (Site::Instance { index, streams }, Stored::Trs { t, s, .. }) => {
            for (accessor, values) in [(streams[0], t), (streams[2], s)] {
                for (k, v) in values.iter().enumerate() {
                    let at = element(doc, accessor, index, k)?;
                    bin.get_mut(at..at + 4)
                        .ok_or_else(|| {
                            err(format!(
                                "instance stream {accessor} element {index} runs past the buffer"
                            ))
                        })?
                        .copy_from_slice(&(*v as f32).to_le_bytes());
                }
            }
        }
        _ => return Err(err("a site's stored numbers are not of its kind")),
    }
    Ok(())
}

/// The frame of `values`: centred on their bounding box, with one uniform step (uniform decoding keeps normals)
/// that reaches the farthest coordinate within ±32767.
fn frame(values: &[[f32; 3]]) -> Frame {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in values {
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(v[k]));
            hi[k] = hi[k].max(f64::from(v[k]));
        }
    }
    if values.is_empty() {
        return Frame {
            centre: [0.; 3],
            step: 1.,
        };
    }
    let centre: [f32; 3] = std::array::from_fn(|k| ((lo[k] + hi[k]) / 2.) as f32);
    let reach = (0..3)
        .map(|k| (hi[k] - f64::from(centre[k])).max(f64::from(centre[k]) - lo[k]))
        .fold(0f64, f64::max);
    let mut step = (reach / 32767.) as f32;
    while f64::from(step) * 32767. < reach {
        step = step.next_up();
    }
    if step == 0. {
        step = 1.;
    }
    Frame { centre, step }
}

fn quantize(f: Frame, v: [f32; 3]) -> [i16; 3] {
    std::array::from_fn(|k| {
        ((f64::from(v[k]) - f64::from(f.centre[k])) / f64::from(f.step))
            .round()
            .clamp(-32767., 32767.) as i16
    })
}

fn decode(f: Frame, q: [i16; 3]) -> [f64; 3] {
    std::array::from_fn(|k| f64::from(f.centre[k]) + f64::from(f.step) * f64::from(q[k]))
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

/// What quantizing a tile's positions did.
#[derive(Default)]
struct Positions {
    /// Per quantized POSITION accessor, its values.
    quantized: BTreeMap<usize, Vec<[i16; 3]>>,
    max_local_error: f64,
    max_world_error: f64,
    max_step: f32,
    /// Meshes keeping f32 positions: quantizing would move a vertex further than [`POSITION_BUDGET`].
    meshes_float: u64,
}

fn root(group: &mut [usize], mut m: usize) -> usize {
    while group[m] != m {
        group[m] = group[group[m]];
        m = group[m];
    }
    m
}

/// Quantize each mesh's positions in a frame centred on their bounds and compose the frame into every placement of
/// the mesh, where the world error (measured from the stored numbers, through every placement, at every vertex)
/// stays within [`POSITION_BUDGET`]; otherwise the mesh keeps f32 positions. Meshes sharing a position accessor share
/// a frame.
fn positions(doc: &mut Value, bin: &mut [u8]) -> Result<Positions> {
    let meshes = doc["meshes"].as_array().map_or(0, Vec::len);
    let nodes = doc["nodes"].as_array().cloned().unwrap_or_default();
    if nodes.iter().any(|n| {
        n.get("children")
            .and_then(Value::as_array)
            .is_some_and(|c| !c.is_empty())
    }) {
        return Err(err(
            "tile nodes form a hierarchy: a mesh's frame would move its children",
        ));
    }
    // meshes joined by a shared position accessor
    let mut group: Vec<usize> = (0..meshes).collect();
    let mut accessors_of = vec![Vec::new(); meshes];
    let mut first_mesh = BTreeMap::<usize, usize>::new();
    for (m, accessors) in accessors_of.iter_mut().enumerate() {
        for p in doc["meshes"][m]["primitives"]
            .as_array()
            .ok_or_else(|| err(format!("meshes[{m}] has no primitives array")))?
        {
            let a = number(&p["attributes"]["POSITION"])?;
            if !accessors.contains(&a) {
                accessors.push(a);
            }
            let other = *first_mesh.entry(a).or_insert(m);
            let (x, y) = (root(&mut group, m), root(&mut group, other));
            group[x.max(y)] = x.min(y);
        }
    }
    let mut sites_of = vec![Vec::new(); meshes];
    for (node, n) in nodes.iter().enumerate() {
        let Some(m) = n["mesh"].as_u64().map(|m| m as usize) else {
            continue;
        };
        if m >= meshes {
            return Err(err("node names a mesh the tile lacks"));
        }
        let instancing = &n["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
        if instancing.is_object() {
            let stream = |name: &str| {
                instancing[name]
                    .as_u64()
                    .map(|a| a as usize)
                    .ok_or_else(|| err(format!("instanced node lacks its {name} stream")))
            };
            let streams = [stream("TRANSLATION")?, stream("ROTATION")?, stream("SCALE")?];
            for index in 0..number(&doc["accessors"][streams[0]]["count"])? {
                sites_of[m].push(Site::Instance { index, streams });
            }
        } else if n.get("matrix").is_some() {
            sites_of[m].push(Site::Matrix { node });
        } else {
            sites_of[m].push(Site::Trs { node });
        }
    }
    let mut groups = BTreeMap::<usize, Vec<usize>>::new();
    for m in 0..meshes {
        groups.entry(root(&mut group, m)).or_default().push(m);
    }
    let mut out = Positions::default();
    for members in groups.values() {
        let mut accessors: Vec<usize> = members
            .iter()
            .flat_map(|&m| accessors_of[m].iter().copied())
            .collect();
        accessors.sort_unstable();
        accessors.dedup();
        let values: BTreeMap<usize, Vec<[f32; 3]>> = accessors
            .iter()
            .map(|&a| Ok((a, floats(doc, bin, a)?.as_chunks::<3>().0.to_vec())))
            .collect::<Result<_>>()?;
        let all: Vec<[f32; 3]> = values.values().flatten().copied().collect();
        let f = frame(&all);
        let q: BTreeMap<usize, Vec<[i16; 3]>> = values
            .iter()
            .map(|(&a, v)| (a, v.iter().map(|&v| quantize(f, v)).collect()))
            .collect();
        let mut local = 0f64;
        for (a, v) in &values {
            for (v, q) in v.iter().zip(&q[a]) {
                local = local.max(distance(decode(f, *q), v.map(f64::from)));
            }
        }
        // through every placement: the stored transform after against before, at every vertex of the mesh
        let mut world = 0f64;
        let mut composed = Vec::new();
        for &m in members {
            for &site in &sites_of[m] {
                let before = stored(doc, bin, site)?;
                let after = before.compose(f);
                let (was, now) = (before.affine(), after.affine());
                for accessor in &accessors_of[m] {
                    for (v, q) in values[accessor].iter().zip(&q[accessor]) {
                        world = world.max(distance(now.point(q.map(f64::from)), was.point(v.map(f64::from))));
                    }
                }
                composed.push((site, after));
            }
        }
        if composed.is_empty() {
            world = local;
        }
        if world.is_nan() || world > POSITION_BUDGET {
            out.meshes_float += members.len() as u64;
            continue;
        }
        for (site, after) in &composed {
            store(doc, bin, *site, after)?;
        }
        out.max_local_error = out.max_local_error.max(local);
        out.max_world_error = out.max_world_error.max(world);
        out.max_step = out.max_step.max(f.step);
        out.quantized.extend(q);
    }
    Ok(out)
}

/// The vertex streams a tile's compression stored in 16 bits: an approximation of their f32 values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Quantized {
    /// POSITION streams, each vertex within [`POSITION_BUDGET`] of where it was in the world.
    pub(super) positions: u64,
    /// NORMAL and TANGENT streams (signed normalized) and texture coordinates within 0-1 (unsigned normalized).
    pub(super) directions_and_texcoords: u64,
}

/// Bounds-centred signed-16 positions within [`POSITION_BUDGET`] (see [`positions`]); normal/tangent signed
/// normalized 16-bit; unit UVs unsigned normalized 16-bit. Returns the tile and the streams it quantized.
pub(super) fn apply(input: Document) -> Result<(Document, Quantized)> {
    let Document {
        json: mut doc,
        bin: mut source,
    } = input;
    let mut kinds = BTreeMap::<usize, String>::new();
    for mesh in doc["meshes"]
        .as_array()
        .ok_or_else(|| err("the document has no meshes array"))?
    {
        for p in mesh["primitives"]
            .as_array()
            .ok_or_else(|| err("a mesh has no primitives array"))?
        {
            for (name, a) in p["attributes"]
                .as_object()
                .ok_or_else(|| err("a primitive has no attributes object"))?
            {
                kinds.insert(number(a)?, name.clone());
            }
        }
    }
    let placed = positions(&mut doc, &mut source)?;
    let old = doc.clone();
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut fallback = 0usize;
    let mut quantized = Quantized::default();
    for a in 0..old["accessors"]
        .as_array()
        .ok_or_else(|| err("the document has no accessors array"))?
        .len()
    {
        let original = &old["accessors"][a];
        let count = number(&original["count"])?;
        let old_view = &old["bufferViews"][number(&original["bufferView"])?];
        let is_vertex = old_view["target"] == 34962;
        let name = kinds.get(&a).map(String::as_str).unwrap_or("");
        let mut raw = data(&old, &source, a)?;
        let mut step = stride(original)?;
        let mut accessor = original.clone();
        accessor
            .as_object_mut()
            .ok_or_else(|| err(format!("accessors[{a}] is not an object")))?
            .remove("byteOffset");
        if original["componentType"] == 5126
            && ((name == "POSITION" && placed.quantized.contains_key(&a))
                || name == "NORMAL"
                || name == "TANGENT"
                || name.starts_with("TEXCOORD_"))
        {
            let values = floats(&old, &source, a)?;
            let uv = name.starts_with("TEXCOORD_");
            if !uv || values.iter().all(|v| (0. ..=1.).contains(v)) {
                let components = step / 4;
                step = (components * 2).div_ceil(4) * 4;
                raw.clear();
                let mut min = vec![i32::MAX; components];
                let mut max = vec![i32::MIN; components];
                for (vertex_index, vertex) in values.chunks_exact(components).enumerate() {
                    for (k, &v) in vertex.iter().enumerate() {
                        let q = if name == "POSITION" {
                            i32::from(placed.quantized[&a][vertex_index][k])
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
                    quantized.positions += 1;
                } else {
                    quantized.directions_and_texcoords += 1;
                }
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
            let view = &old["bufferViews"][number(&image["bufferView"])?];
            let start = number(&view["byteOffset"])?;
            let length = number(&view["byteLength"])?;
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
    if quantized != Quantized::default() {
        extension(&mut doc, "KHR_mesh_quantization");
    }
    doc["asset"]["extras"]["primase.quantization"] = json!({
        "position_centre": "bounds",
        "position_budget": POSITION_BUDGET,
        "position_max_world_error": placed.max_world_error,
        "position_max_local_error": placed.max_local_error,
        "position_max_local_step": placed.max_step,
        "meshes_float_positions": placed.meshes_float,
        "position_bits": 16,
        "normal_bits": 16,
        "uv_bits": 16,
    });
    Ok((Document::new(doc, bin), quantized))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message<T>(r: Result<T>) -> String {
        match r {
            Ok(_) => String::from("ok"),
            Err(e) => e.to_string(),
        }
    }

    /// A document the writer did not make is an error naming the field, not a panic of an `expect`.
    #[test]
    fn a_document_that_is_not_the_writers_is_an_error() {
        assert_eq!(number(&json!(7)).unwrap(), 7);
        assert!(message(number(&json!("7"))).contains("not a non-negative integer"));
        assert!(message(number(&json!(-1))).contains("not a non-negative integer"));
        assert_eq!(
            stride(&json!({"type": "VEC3", "componentType": 5126})).unwrap(),
            12
        );
        assert!(message(stride(&json!({"type": "VEC9", "componentType": 5126}))).contains("accessor type"));
        assert!(
            message(stride(&json!({"type": "VEC3", "componentType": 5124}))).contains("component type 5124")
        );
        assert!(message(stride(&json!({"componentType": 5126}))).contains("accessor type None"));
    }

    /// The last element of an instance's streams reads: translation and scale are three components, rotation four, so the
    /// last element of a tightly packed VEC3 stream is not read for a fourth (that runs past the buffer).
    #[test]
    fn the_last_instance_of_a_tightly_packed_set_of_streams_reads() {
        let floats = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let mut bin = floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]); // two translations
        bin.extend(floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0])); // two rotations
        bin.extend(floats(&[1.0, 1.0, 1.0, 7.0, 8.0, 9.0])); // two scales
        let accessor = |ty: &str, view: usize| json!({"bufferView": view, "componentType": 5126, "count": 2, "type": ty});
        let doc = json!({
            "accessors": [accessor("VEC3", 0), accessor("VEC4", 1), accessor("VEC3", 2)],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 24},
                {"buffer": 0, "byteOffset": 24, "byteLength": 32},
                {"buffer": 0, "byteOffset": 56, "byteLength": 24},
            ],
        });
        let site = Site::Instance {
            index: 1,
            streams: [0, 1, 2],
        };
        let Stored::Trs { t, r, s } = stored(&doc, &bin, site).unwrap() else {
            panic!("an instance stores a translation, rotation and scale");
        };
        assert_eq!(
            (t, r, s),
            ([4.0, 5.0, 6.0], [0.0, 0.0, 1.0, 0.0], [7.0, 8.0, 9.0])
        );
    }

    /// An accessor whose elements run past the buffer, and an instance stream that does.
    #[test]
    fn an_accessor_past_the_buffer_is_an_error() {
        let doc = json!({
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
        });
        assert_eq!(data(&doc, &[0; 36], 0).unwrap().len(), 36);
        assert!(message(data(&doc, &[0; 30], 0)).contains("runs past the buffer"));
        assert!(message(instance_value(&doc, &[0; 8], 0, 1, 0)).contains("runs past the buffer"));
        let mut bin = [0u8; 8];
        let site = Site::Instance {
            index: 0,
            streams: [0, 0, 0],
        };
        let stored = Stored::Trs {
            t: [0.0; 3],
            r: [0.0, 0.0, 0.0, 1.0],
            s: [1.0; 3],
        };
        assert!(message(store(&mut doc.clone(), &mut bin, site, &stored)).contains("runs past the buffer"));
    }
}
