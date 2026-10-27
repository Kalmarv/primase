//! Meshes as accessors: [`Buffers`], the vertex and index encoding every mesh writer shares, and [`encode_mesh`], one
//! mesh as a GLB of its own.

use primase_core::json::{JsonError, f32s_value};
use primase_core::{Component, Mesh, Primitive, Semantic, Stream, Topology};
use serde_json::{Map, Value, json};

use crate::read::{ElementType, bounds};
use crate::{Document, Error, Result};

/// A number JSON cannot hold, as the encoder's refusal of a primitive.
fn number(e: JsonError) -> String {
    e.to_string()
}

#[cfg(not(target_endian = "little"))]
compile_error!("the encoder casts vertex data to bytes and assumes little-endian");

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// A document's binary buffer as it is built: the bytes, their buffer views and the accessors over them, each list
/// in the order things were added, so the same additions give the same bytes.
#[derive(Clone, Debug, Default)]
pub struct Buffers {
    /// Buffer 0's bytes (each view starts 4-byte aligned).
    pub bin: Vec<u8>,
    /// The document's `bufferViews`.
    pub views: Vec<Value>,
    /// The document's `accessors`.
    pub accessors: Vec<Value>,
}

/// The accessors of one encoded primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveAccessors {
    /// glTF attribute name and accessor index, in the primitive's stream order.
    pub attributes: Vec<(String, usize)>,
    pub indices: Option<usize>,
    /// The primitive's `mode`, `None` for triangles (glTF's default).
    pub mode: Option<u32>,
    /// Streams not written, by glTF attribute name: skin attributes (`JOINTS_n`, `WEIGHTS_n`), which need a skin.
    pub skipped: Vec<String>,
}

impl Buffers {
    /// A buffer view over `bytes`, appended 4-byte aligned; `target` is its `target` (vertex or index data), if any.
    pub fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
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

    /// Every stream and the indices of a primitive as accessors, after checking it is one glTF can hold: a float
    /// VEC3 `POSITION` with at least one vertex, streams as long as it, finite floats, whole points, lines or
    /// triangles and indices within the vertices. `color0`: whether `COLOR_0` is written (something drawing the
    /// primitive reads it). The error is the reason, for the caller to place.
    pub fn primitive(
        &mut self,
        prim: &Primitive,
        color0: bool,
    ) -> std::result::Result<PrimitiveAccessors, String> {
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
        let mut skipped = Vec::new();
        for (semantic, stream) in &prim.streams {
            if *semantic == Semantic::Color(0) && !color0 {
                continue;
            }
            let (name, index) = match (semantic, stream) {
                (Semantic::Position, Stream::F32x3(p)) => {
                    ("POSITION".to_owned(), self.positions(p).map_err(number)?)
                }
                (Semantic::Normal, Stream::F32x3(v)) => (
                    "NORMAL".to_owned(),
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec3, None)
                        .map_err(number)?,
                ),
                (Semantic::Tangent(n), Stream::F32x4(v)) => (
                    // Primase's own attribute for the further tangent sets: `_TANGENT<n>` belongs to whatever
                    // application wrote it, and the reader would otherwise read a foreign one as our tangent set.
                    if *n == 0 {
                        "TANGENT".to_owned()
                    } else {
                        format!("_PRIMASE_TANGENT{n}")
                    },
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec4, None)
                        .map_err(number)?,
                ),
                (Semantic::TexCoord(n), Stream::F32x2(v)) => (
                    format!("TEXCOORD_{n}"),
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec2, None)
                        .map_err(number)?,
                ),
                (Semantic::Color(n), Stream::F32x3(v)) => (
                    format!("COLOR_{n}"),
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec3, None)
                        .map_err(number)?,
                ),
                (Semantic::Color(n), Stream::F32x4(v)) => (
                    format!("COLOR_{n}"),
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec4, None)
                        .map_err(number)?,
                ),
                (Semantic::Color(n), Stream::U8x4(v)) => (
                    format!("COLOR_{n}"),
                    self.unorms(bytemuck::cast_slice(v), count, 5121),
                ),
                (Semantic::Color(n), Stream::U16x4(v)) => (
                    format!("COLOR_{n}"),
                    self.unorms(bytemuck::cast_slice(v), count, 5123),
                ),
                (Semantic::Joints(n), _) => {
                    skipped.push(format!("JOINTS_{n}"));
                    continue;
                }
                (Semantic::Weights(n), _) => {
                    skipped.push(format!("WEIGHTS_{n}"));
                    continue;
                }
                (s, v) => return Err(format!("{s:?} stream of {} is not writable", stream_kind(v))),
            };
            attributes.push((name, index));
        }
        let indices = prim.indices.as_ref().map(|idx| self.indices(idx, count));
        let mode = match prim.topology {
            Topology::Triangles => None,
            Topology::Lines => Some(1),
            Topology::Points => Some(0),
        };
        Ok(PrimitiveAccessors {
            attributes,
            indices,
            mode,
            skipped,
        })
    }

    /// An accessor over `values`, `element` of floats, its view targeting vertex data when `vertex` (attributes, morph
    /// targets) and nothing otherwise (inverse binds, animation keys); `min` and `max` written when given (glTF requires
    /// them of positions and animation inputs).
    pub fn float_accessor(
        &mut self,
        values: &[f32],
        element: ElementType,
        min_max: Option<(Vec<f32>, Vec<f32>)>,
        vertex: bool,
    ) -> crate::Result<usize> {
        let view = self.view(bytemuck::cast_slice(values), vertex.then_some(ARRAY_BUFFER));
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(5126));
        a.insert("count".into(), json!(values.len() / element.components()));
        a.insert("type".into(), json!(element.name()));
        if let Some((lo, hi)) = min_max {
            a.insert("min".into(), f32s_value(&lo)?);
            a.insert("max".into(), f32s_value(&hi)?);
        }
        self.accessors.push(Value::Object(a));
        Ok(self.accessors.len() - 1)
    }

    /// A `VEC4` vertex accessor over integer `bytes` (`count` elements) of `component` (5121 unsigned byte, 5123
    /// unsigned short), `normalized` for weights, not for joint indices.
    pub fn int_accessor(&mut self, bytes: &[u8], count: usize, component: u32, normalized: bool) -> usize {
        let view = self.view(bytes, Some(ARRAY_BUFFER));
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(component));
        if normalized {
            a.insert("normalized".into(), json!(true));
        }
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!("VEC4"));
        self.accessors.push(Value::Object(a));
        self.accessors.len() - 1
    }

    fn positions(&mut self, p: &[[f32; 3]]) -> std::result::Result<usize, JsonError> {
        let (lo, hi) = bounds(p).expect("primitive() rejects empty primitives");
        self.floats(
            bytemuck::cast_slice(p),
            p.len(),
            ElementType::Vec3,
            Some((&lo, &hi)),
        )
    }

    fn floats(
        &mut self,
        bytes: &[u8],
        count: usize,
        element: ElementType,
        minmax: Option<(&[f32], &[f32])>,
    ) -> std::result::Result<usize, JsonError> {
        let view = self.view(bytes, Some(ARRAY_BUFFER));
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(5126));
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!(element.name()));
        if let Some((lo, hi)) = minmax {
            a.insert("min".into(), f32s_value(lo)?);
            a.insert("max".into(), f32s_value(hi)?);
        }
        self.accessors.push(Value::Object(a));
        Ok(self.accessors.len() - 1)
    }

    fn unorms(&mut self, bytes: &[u8], count: usize, component: u32) -> usize {
        let view = self.view(bytes, Some(ARRAY_BUFFER));
        self.accessors.push(json!({
            "bufferView": view, "componentType": component, "normalized": true, "count": count, "type": "VEC4",
        }));
        self.accessors.len() - 1
    }

    /// Indices as unsigned short when every index is below 65535 (the primitive-restart value glTF forbids),
    /// else unsigned int.
    fn indices(&mut self, idx: &[u32], vertex_count: usize) -> usize {
        let (view, component) = if vertex_count <= 65535 {
            let short: Vec<u16> = idx.iter().map(|&i| i as u16).collect();
            (
                self.view(bytemuck::cast_slice(&short), Some(ELEMENT_ARRAY_BUFFER)),
                5123,
            )
        } else {
            (
                self.view(bytemuck::cast_slice(idx), Some(ELEMENT_ARRAY_BUFFER)),
                5125,
            )
        };
        self.accessors.push(json!({
            "bufferView": view, "componentType": component, "count": idx.len(), "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }
}

/// One mesh as a GLB of its own, named `name`: its primitives (materials ignored, every stream written, `COLOR_0`
/// included) as mesh 0, drawn by a node at the origin under a group node, as the glTF writer writes an unplaced
/// prototype whose primitives have no material: the node's extras show the mesh's `core.mesh` view (its summary,
/// materials null), as the writer shows the components of an entity placed as itself. Skin streams are left out
/// (glTF needs a skin for them). Refuses a mesh without primitives or with a primitive glTF can't hold
/// ([`Buffers::primitive`]), naming it.
pub fn encode_mesh(name: &str, mesh: &Mesh) -> Result<Vec<u8>> {
    if mesh.primitives().is_empty() {
        return Err(Error::Invalid {
            item: "meshes[0]".into(),
            detail: "a mesh without primitives".into(),
        });
    }
    let mut buffers = Buffers::default();
    let mut primitives = Vec::with_capacity(mesh.primitives().len());
    for (i, prim) in mesh.primitives().iter().enumerate() {
        let a = buffers.primitive(prim, true).map_err(|detail| Error::Invalid {
            item: format!("meshes[0].primitives[{i}]"),
            detail,
        })?;
        let mut p = Map::new();
        let attributes: Map<String, Value> = a.attributes.into_iter().map(|(n, i)| (n, json!(i))).collect();
        p.insert("attributes".into(), Value::Object(attributes));
        if let Some(i) = a.indices {
            p.insert("indices".into(), json!(i));
        }
        if let Some(mode) = a.mode {
            p.insert("mode".into(), json!(mode));
        }
        primitives.push(Value::Object(p));
    }
    let mut bare = mesh.clone();
    for mut p in bare.primitives_mut() {
        *p.material_mut() = None;
    }
    let view = bare.to_json().map_err(|e| Error::Invalid {
        item: "meshes[0]".into(),
        detail: e.to_string(),
    })?;
    let mut node = Map::new();
    node.insert("name".into(), json!(name));
    node.insert("mesh".into(), json!(0));
    if let Some(value) = view {
        node.insert(
            "extras".into(),
            json!({"primase": {Mesh::NAME: {"version": Mesh::VERSION, "value": value}}}),
        );
    }
    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({"version": "2.0", "generator": concat!("primase-glb ", env!("CARGO_PKG_VERSION"))}),
    );
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{"nodes": [1]}]));
    root.insert(
        "nodes".into(),
        json!([
            Value::Object(node),
            {"name": name, "children": [0], "extras": {"primase.collection": name, "primase.group": "prototype"}},
        ]),
    );
    root.insert("meshes".into(), json!([{"name": name, "primitives": primitives}]));
    root.insert("accessors".into(), Value::Array(buffers.accessors));
    root.insert("bufferViews".into(), Value::Array(buffers.views));
    root.insert("buffers".into(), json!([{"byteLength": buffers.bin.len()}]));
    Document::new(Value::Object(root), buffers.bin).to_glb()
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
