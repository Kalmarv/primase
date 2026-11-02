//! Meshes as accessors: [`Buffers`], the vertex and index encoding every mesh writer shares, and [`encode_mesh`], one
//! mesh as a GLB of its own.

use std::collections::BTreeMap;

use primase_core::json::{JsonError, f32s_value};
use primase_core::{Mesh, Primitive, Semantic, Stream, Topology};
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
/// in the order things were added, so the same additions give the same bytes. A view's bytes are held from when it is
/// added ([`Buffers::view`]), or stated by their length and written when the buffer is ([`Buffers::deferred_view`],
/// [`Buffers::write_bin`]), so a writer streaming a large buffer need not hold its largest parts (encoded images)
/// together.
#[derive(Clone, Debug, Default)]
pub struct Buffers {
    /// Buffer 0's bytes but the deferred views' (each view starts 4-byte aligned).
    bin: Vec<u8>,
    /// The deferred views, in the order they were added: where in `bin` each one's bytes go, and how many there are.
    deferred: Vec<Deferred>,
    /// The deferred views' bytes, together.
    deferred_len: usize,
    /// The document's `bufferViews`.
    pub views: Vec<Value>,
    /// The document's `accessors`.
    pub accessors: Vec<Value>,
}

/// A view whose bytes are written with the buffer: `len` of them, before `bin[at]`.
#[derive(Clone, Copy, Debug)]
struct Deferred {
    at: usize,
    len: usize,
}

/// The accessors of one encoded primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct PrimitiveAccessors {
    /// glTF attribute name and accessor index, in the primitive's stream order.
    pub attributes: Vec<(String, usize)>,
    pub indices: Option<usize>,
    /// The primitive's `mode`, `None` for triangles (glTF's default).
    pub mode: Option<u32>,
    /// Streams not written, by name: skin attributes (`JOINTS_n`, `WEIGHTS_n`), which need a skin, and the streams glTF
    /// has no attribute for: a tangent set past the first (`tangent set n`; glTF has one `TANGENT`) and a shader's raw
    /// per-vertex inputs (`shader input n`) and points in the mesh's space (`point n`, a billboard's pivot).
    pub skipped: Vec<String>,
}

impl Buffers {
    /// The buffer's length, the deferred views' bytes included.
    pub fn len(&self) -> usize {
        self.bin.len() + self.deferred_len
    }

    /// Whether the buffer holds no byte.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A buffer view over `bytes`, appended 4-byte aligned; `target` is its `target` (vertex or index data), if any.
    pub fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        let index = self.view_of(bytes.len(), target);
        self.bin.extend_from_slice(bytes);
        index
    }

    /// A buffer view of `len` bytes appended as [`Buffers::view`] appends one, whose bytes are not held: the `n`th
    /// deferred view (from 0, in the order they were added) is written by [`Buffers::write_bin`]'s `fill(n)`.
    pub fn deferred_view(&mut self, len: usize, target: Option<u32>) -> usize {
        let index = self.view_of(len, target);
        self.deferred.push(Deferred {
            at: self.bin.len(),
            len,
        });
        self.deferred_len += len;
        index
    }

    /// The view of the next `len` bytes, 4-byte aligned: its padding added to `bin`.
    fn view_of(&mut self, len: usize, target: Option<u32>) -> usize {
        let aligned = self.len().next_multiple_of(4);
        self.bin.resize(aligned - self.deferred_len, 0);
        let mut v = Map::new();
        v.insert("buffer".into(), json!(0));
        v.insert("byteOffset".into(), json!(aligned));
        v.insert("byteLength".into(), json!(len));
        if let Some(t) = target {
            v.insert("target".into(), json!(t));
        }
        self.views.push(Value::Object(v));
        self.views.len() - 1
    }

    /// The buffer's bytes, where no view is deferred (an [`Error::Invalid`] otherwise: they are written, not held).
    pub fn bytes(&self) -> Result<&[u8]> {
        match self.deferred.len() {
            0 => Ok(&self.bin),
            n => Err(Error::Invalid {
                item: "buffers[0]".into(),
                detail: format!("{n} views are written with the buffer (write_bin), not held"),
            }),
        }
    }

    /// [`Buffers::bytes`], taken.
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        self.bytes()?;
        Ok(self.bin)
    }

    /// Write the buffer's bytes to `out` in order, `fill(n, out)` writing the `n`th deferred view's bytes where they
    /// go: exactly the length it was added with (an [`Error::Invalid`] naming the view otherwise).
    pub fn write_bin<E: From<Error>>(
        &self,
        out: &mut dyn std::io::Write,
        mut fill: impl FnMut(usize, &mut dyn std::io::Write) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E> {
        let io = |source: std::io::Error| Error::Stream { source };
        let mut from = 0;
        for (n, d) in self.deferred.iter().enumerate() {
            out.write_all(&self.bin[from..d.at]).map_err(io)?;
            let mut counted = crate::container::Counted::new(&mut *out);
            fill(n, &mut counted)?;
            if counted.written() != d.len as u64 {
                return Err(Error::Invalid {
                    item: format!("buffers[0] deferred view {n}"),
                    detail: format!("{} bytes written, {} stated", counted.written(), d.len),
                }
                .into());
            }
            from = d.at;
        }
        out.write_all(&self.bin[from..]).map_err(io)?;
        Ok(())
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
                (Semantic::Tangent(0), Stream::F32x4(v)) => (
                    "TANGENT".to_owned(),
                    self.floats(bytemuck::cast_slice(v), count, ElementType::Vec4, None)
                        .map_err(number)?,
                ),
                // glTF has one tangent set and no attribute for a shader's own inputs
                (Semantic::Tangent(n), Stream::F32x4(_)) => {
                    skipped.push(format!("tangent set {n}"));
                    continue;
                }
                (Semantic::ShaderInput(n), Stream::F32x4(_)) => {
                    skipped.push(format!("shader input {n}"));
                    continue;
                }
                (Semantic::Point(n), Stream::F32x3(_)) => {
                    skipped.push(format!("point {n}"));
                    continue;
                }
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

/// One mesh encoded as a GLB of its own ([`encode_mesh`]), and what the GLB leaves out of it.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedMesh {
    pub glb: Vec<u8>,
    /// The streams not written, by name ([`PrimitiveAccessors::skipped`]), each with the number of primitives it was
    /// left out of.
    pub skipped: BTreeMap<String, u64>,
}

/// One mesh as a GLB of its own, named `name`: its primitives (materials ignored, `COLOR_0` included) as mesh 0,
/// drawn by a node at the origin under a group node named like it, as the glTF writer writes an unplaced prototype
/// whose primitives have no material. Skin streams are left out (glTF needs a skin for them), and so are the streams
/// glTF has no attribute for; [`EncodedMesh::skipped`] names each, for the caller to report.
/// Refuses a mesh without primitives or with a primitive glTF can't hold ([`Buffers::primitive`]), naming it.
pub fn encode_mesh(name: &str, mesh: &Mesh) -> Result<EncodedMesh> {
    if mesh.primitives().is_empty() {
        return Err(Error::Invalid {
            item: "meshes[0]".into(),
            detail: "a mesh without primitives".into(),
        });
    }
    let mut buffers = Buffers::default();
    let mut primitives = Vec::with_capacity(mesh.primitives().len());
    let mut skipped = BTreeMap::new();
    for (i, prim) in mesh.primitives().iter().enumerate() {
        let a = buffers.primitive(prim, true).map_err(|detail| Error::Invalid {
            item: format!("meshes[0].primitives[{i}]"),
            detail,
        })?;
        for name in a.skipped {
            *skipped.entry(name).or_default() += 1;
        }
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
    let mut node = Map::new();
    node.insert("name".into(), json!(name));
    node.insert("mesh".into(), json!(0));
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
            {"name": name, "children": [0]},
        ]),
    );
    root.insert("meshes".into(), json!([{"name": name, "primitives": primitives}]));
    root.insert(
        "accessors".into(),
        Value::Array(std::mem::take(&mut buffers.accessors)),
    );
    root.insert(
        "bufferViews".into(),
        Value::Array(std::mem::take(&mut buffers.views)),
    );
    root.insert("buffers".into(), json!([{"byteLength": buffers.len()}]));
    let glb = Document::new(Value::Object(root), buffers.into_bytes()?).to_glb()?;
    Ok(EncodedMesh { glb, skipped })
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
