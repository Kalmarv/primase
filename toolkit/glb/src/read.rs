//! The strict glTF reader: meshes into [`primase_core::Mesh`], plus checked access to the rest of the document for
//! round-trip tests and for readers that take materials, nodes and images from the JSON themselves.
//!
//! What it reads (a superset of skate-scene v1's prototype GLB rules): a GLB with one buffer held in the BIN chunk
//! ([`Gltf::from_glb`]), or a `.gltf` JSON document whose buffers, each named by a `uri`, the caller supplies
//! ([`Gltf::from_gltf`]: where the bytes come from, a file beside it or a game's container, is the caller's);
//! primitives of mode points, lines or triangles; `indices` unsigned byte, short or int (or none: sequential);
//! `POSITION` float VEC3, `NORMAL` float VEC3, `TANGENT` float VEC4, `TEXCOORD_n` float or normalized unsigned
//! byte/short VEC2, `COLOR_n` float or normalized unsigned byte/short VEC3/VEC4; skin attributes: `JOINTS_n`
//! unsigned byte/short VEC4 and `WEIGHTS_n` float or normalized unsigned byte/short VEC4, each kept as stored (a
//! reader reads faithfully: weights are not dequantized, their sums and the joint indices are not checked here),
//! every `JOINTS_n` with its `WEIGHTS_n`; morph targets (`targets`, with the mesh's default `weights`): per target
//! `POSITION`, `NORMAL` and `TANGENT` deltas as float VEC3, or normalized byte/short VEC3 dequantized by glTF
//! 3.11's rule (`max(c / 127, -1)`, `max(c / 32767, -1)`, in f64, rounded once to f32: core has no integer form for
//! a delta, so this is the stated conversion), every target as long as POSITION and every primitive of a mesh with
//! as many targets; sparse accessors and accessors without a `bufferView` (zeros), read into the same tightly
//! packed elements as any other; any accessor's elements as f32 for animation samplers ([`Gltf::floats`]);
//! application-specific attributes (named `_...`, glTF 3.7.2.1) as stored ([`AccessorData`]), for the caller to
//! state. Everything else that could change the geometry is an error naming the item: strips, fans and loops, a
//! GLB's external or extra buffers, required extensions, any extension on what is read, unknown properties,
//! application-specific, texture-coordinate or color morph target attributes. `extras` are kept where the reader has a
//! home for them (a mesh, primitive, node, skin or animation, and the default scene, [`Gltf::scene_extras`]) and an
//! error anywhere else (an accessor, buffer view, sampler...): never dropped without a word. Extensions a document only uses
//! (`extensionsUsed`, on materials or textures) are the caller's to read from [`Gltf::json`].
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::borrow::Cow;
use std::collections::BTreeMap;

use primase_core::{Mesh, Primitive, Semantic, Stream, Topology};
use serde_json::Value;

use crate::json::{Obj, invalid};
use crate::{Error, Result};

const BYTE: u64 = 5120;
const UNSIGNED_BYTE: u64 = 5121;
const SHORT: u64 = 5122;
const UNSIGNED_SHORT: u64 = 5123;
const UNSIGNED_INT: u64 = 5125;
const FLOAT: u64 = 5126;

/// The most elements the reader makes up for an accessor that has no `bufferView` (all zeros) or a sparse substitution:
/// the file's `count` is all that bounds them, so a few hundred bytes of JSON would otherwise ask for gigabytes. A dense
/// accessor is bounded by its view. 16,777,216 elements: a 16 M-vertex float VEC3 attribute is 192 MB.
pub const MAX_MATERIALISED_ELEMENTS: usize = 1 << 24;

/// A parsed glTF document: its JSON and its binary buffers.
#[derive(Debug)]
pub struct Gltf {
    json: Value,
    /// The buffers in the document's order, each its `byteLength` bytes (a GLB's without the BIN chunk's padding).
    buffers: Vec<Vec<u8>>,
}

/// One primitive as read: the core primitive (its `material` entity left `None`) and the glTF material it named.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadPrimitive {
    pub primitive: Primitive,
    /// Index into the document's `materials`.
    pub material: Option<usize>,
    /// That material's `name`.
    pub material_name: Option<String>,
    /// Application-specific attributes (named `_...`) by name, as stored: one element per vertex, like POSITION.
    pub custom: BTreeMap<String, AccessorData>,
    /// The morph targets in order; every primitive of a mesh has as many.
    pub targets: Vec<ReadTarget>,
    /// The primitive's `extras`, as stored.
    pub extras: Option<Value>,
}

/// One morph target: deltas added to the primitive's attributes, one element per vertex like POSITION. FLOAT as
/// stored; normalized BYTE/SHORT dequantized (glTF 3.11).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadTarget {
    pub position: Option<Vec<[f32; 3]>>,
    pub normal: Option<Vec<[f32; 3]>>,
    pub tangent: Option<Vec<[f32; 3]>>,
}

/// An accessor's component type (glTF 3.6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentType {
    I8,
    U8,
    I16,
    U16,
    U32,
    F32,
}

impl ComponentType {
    /// Bytes per component.
    pub fn size(self) -> usize {
        match self {
            ComponentType::I8 | ComponentType::U8 => 1,
            ComponentType::I16 | ComponentType::U16 => 2,
            ComponentType::U32 | ComponentType::F32 => 4,
        }
    }
}

/// An accessor's element type: how many components an element has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementType {
    Scalar,
    Vec2,
    Vec3,
    Vec4,
    /// A 4x4 matrix, column-major as stored (16 components).
    Mat4,
}

impl ElementType {
    /// The glTF `type` string.
    pub fn name(self) -> &'static str {
        match self {
            ElementType::Scalar => "SCALAR",
            ElementType::Vec2 => "VEC2",
            ElementType::Vec3 => "VEC3",
            ElementType::Vec4 => "VEC4",
            ElementType::Mat4 => "MAT4",
        }
    }

    /// Components per element.
    pub fn components(self) -> usize {
        match self {
            ElementType::Scalar => 1,
            ElementType::Vec2 => 2,
            ElementType::Vec3 => 3,
            ElementType::Vec4 => 4,
            ElementType::Mat4 => 16,
        }
    }
}

/// An accessor's elements as stored: `count` elements of `element` components of `component`, packed tight (the
/// view's stride removed), each component little-endian as the buffer holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessorData {
    pub component: ComponentType,
    /// The integers stand for `[0, 1]` (unsigned) or `[-1, 1]` (signed) values.
    pub normalized: bool,
    pub element: ElementType,
    pub count: usize,
    /// `count * element.components() * component.size()` bytes.
    pub bytes: Vec<u8>,
}

/// One glTF mesh as read.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadMesh {
    pub name: Option<String>,
    pub extras: Option<Value>,
    pub primitives: Vec<ReadPrimitive>,
    /// The mesh's default morph target weights; as many as the targets of each primitive.
    pub weights: Option<Vec<f32>>,
}

impl ReadMesh {
    /// The core mesh, every primitive's material `None`.
    pub fn to_mesh(&self) -> Mesh {
        Mesh::new(self.primitives.iter().map(|p| p.primitive.clone()).collect())
    }
}

impl Gltf {
    /// Parse a GLB: the container, the JSON, the asset version, required extensions and the buffer.
    pub fn from_glb(bytes: &[u8]) -> Result<Gltf> {
        let chunks = crate::parse(bytes)?;
        let json: Value = serde_json::from_slice(chunks.json)?;
        let buffers = match (document_buffers(&json)?, chunks.bin) {
            ([], None) => Vec::new(),
            ([], Some(_)) => return Err(invalid("buffers", "a BIN chunk but no buffer")),
            ([b], bin) => {
                let b = Obj::new(b, "buffers[0]")?;
                b.only(&["byteLength", "name"])?;
                let len = b.usize("byteLength")?;
                let bin =
                    bin.ok_or_else(|| invalid("buffers[0]", "no BIN chunk (external buffers are not read)"))?;
                // The BIN chunk is the buffer padded to 4 bytes, nothing more. The length is the file's word:
                // padding it may overflow, and slicing by it is checked.
                let mismatch = || {
                    invalid(
                        "buffers[0]",
                        &format!("byteLength {len} does not match the {}-byte BIN chunk", bin.len()),
                    )
                };
                if len.checked_next_multiple_of(4) != Some(bin.len()) {
                    return Err(mismatch());
                }
                vec![bin.get(..len).ok_or_else(mismatch)?.to_vec()]
            }
            _ => return Err(invalid("buffers", "more than one buffer")),
        };
        Ok(Gltf { json, buffers })
    }

    /// Parse a `.gltf` JSON document, its buffers supplied by the caller: `buffer(i, uri)` returns buffer `i`'s
    /// bytes for the `uri` the document names it by, as written (a relative path, a `data:` URI: resolving it is the
    /// caller's), or why it cannot. Each buffer needs a `uri` and must be exactly its `byteLength` bytes. The asset
    /// version and required extensions are checked as for a GLB.
    pub fn from_gltf(
        json: &[u8],
        mut buffer: impl FnMut(usize, &str) -> std::result::Result<Vec<u8>, String>,
    ) -> Result<Gltf> {
        let json: Value = serde_json::from_slice(json)?;
        let mut buffers = Vec::new();
        for (i, b) in document_buffers(&json)?.iter().enumerate() {
            let path = format!("buffers[{i}]");
            let b = Obj::new(b, &path)?;
            b.only(&["byteLength", "uri", "name"])?;
            let len = b.usize("byteLength")?;
            let uri = b
                .str_opt("uri")?
                .ok_or_else(|| invalid(&path, "no uri (only a GLB's BIN chunk buffer has none)"))?;
            let bytes = buffer(i, uri).map_err(|e| invalid(&path, &format!("{uri}: {e}")))?;
            if bytes.len() != len {
                return Err(invalid(
                    &path,
                    &format!(
                        "byteLength {len} does not match the {}-byte buffer {uri}",
                        bytes.len()
                    ),
                ));
            }
            buffers.push(bytes);
        }
        Ok(Gltf { json, buffers })
    }

    /// The whole JSON document.
    pub fn json(&self) -> &Value {
        &self.json
    }

    fn array(&self, key: &str) -> &[Value] {
        self.json
            .get(key)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    pub fn mesh_count(&self) -> usize {
        self.array("meshes").len()
    }

    /// Buffer `index`'s bytes.
    pub fn buffer(&self, index: usize) -> Result<&[u8]> {
        self.buffers
            .get(index)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid(&format!("buffers[{index}]"), "does not exist"))
    }

    /// The bytes of a buffer view, bounds-checked.
    pub fn buffer_view(&self, index: usize) -> Result<&[u8]> {
        Ok(self.view(index)?.0)
    }

    /// A buffer view's bytes and its stride, if it declares one.
    fn view(&self, index: usize) -> Result<(&[u8], Option<usize>)> {
        let path = format!("bufferViews[{index}]");
        let v = self
            .array("bufferViews")
            .get(index)
            .ok_or_else(|| invalid(&path, "does not exist"))?;
        let v = Obj::new(v, &path)?;
        v.only(&[
            "buffer",
            "byteOffset",
            "byteLength",
            "byteStride",
            "target",
            "name",
        ])?;
        let buffer = v.usize("buffer")?;
        let bin = self
            .buffers
            .get(buffer)
            .ok_or_else(|| invalid(&path, &format!("buffer {buffer} does not exist")))?;
        let off = v.usize_opt("byteOffset")?.unwrap_or(0);
        let len = v.usize("byteLength")?;
        let stride = v.usize_opt("byteStride")?;
        if let Some(s) = stride
            && !(4..=252).contains(&s)
        {
            return Err(invalid(&path, &format!("byteStride {s} outside 4-252")));
        }
        let bytes = off
            .checked_add(len)
            .and_then(|end| bin.get(off..end))
            .ok_or_else(|| invalid(&path, "runs past the buffer"))?;
        Ok((bytes, stride))
    }

    /// Read mesh `index` into core primitives.
    pub fn read_mesh(&self, index: usize) -> Result<ReadMesh> {
        let path = format!("meshes[{index}]");
        let m = self
            .array("meshes")
            .get(index)
            .ok_or_else(|| invalid(&path, "does not exist"))?;
        let m = Obj::new(m, &path)?;
        m.only(&["primitives", "weights", "name", "extras"])?;
        let prims = match m.get("primitives") {
            Some(Value::Array(a)) if !a.is_empty() => a,
            _ => return Err(invalid(&path, "primitives missing or empty")),
        };
        let primitives = prims
            .iter()
            .enumerate()
            .map(|(i, p)| self.read_primitive(p, &format!("{path}.primitives()[{i}]")))
            .collect::<Result<Vec<_>>>()?;
        let targets = primitives.first().map_or(0, |p| p.targets.len());
        if let Some((i, p)) = primitives
            .iter()
            .enumerate()
            .find(|(_, p)| p.targets.len() != targets)
        {
            return Err(invalid(
                &path,
                &format!(
                    "primitive {i} has {} morph targets, primitive 0 has {targets}",
                    p.targets.len()
                ),
            ));
        }
        let weights = m.f32s_opt("weights")?;
        if let Some(w) = &weights
            && w.len() != targets
        {
            return Err(invalid(
                &path,
                &format!("{} weights for {targets} morph targets", w.len()),
            ));
        }
        Ok(ReadMesh {
            name: m.str_opt("name")?.map(str::to_owned),
            extras: m.get("extras").cloned(),
            primitives,
            weights,
        })
    }

    fn read_primitive(&self, p: &Value, path: &str) -> Result<ReadPrimitive> {
        let p = Obj::new(p, path)?;
        p.only(&["attributes", "indices", "material", "mode", "targets", "extras"])?;
        let topology = match p.u64_opt("mode")?.unwrap_or(4) {
            0 => Topology::Points,
            1 => Topology::Lines,
            4 => Topology::Triangles,
            m => {
                return Err(invalid(
                    path,
                    &format!("mode {m} (strips, fans and loops are not read)"),
                ));
            }
        };
        let apath = format!("{path}.attributes");
        let attrs = Obj::new(
            p.get("attributes")
                .ok_or_else(|| invalid(path, "no attributes"))?,
            &apath,
        )?;
        let mut streams = BTreeMap::new();
        let mut custom = BTreeMap::new();
        for (name, acc) in attrs.map {
            let acc = acc
                .as_u64()
                .and_then(|a| usize::try_from(a).ok())
                .ok_or_else(|| invalid(&apath, &format!("{name} is not an accessor index")))?;
            if name.starts_with('_') {
                let data = self
                    .accessor_data(acc)
                    .map_err(|e| e.context(&format!("{apath}.{name}")))?;
                custom.insert(name.clone(), data);
                continue;
            }
            let semantic =
                semantic(name).ok_or_else(|| invalid(&apath, &format!("unsupported attribute {name}")))?;
            let stream = self
                .read_attribute(semantic, acc)
                .map_err(|e| e.context(&format!("{apath}.{name}")))?;
            streams.insert(semantic, stream);
        }
        let count = streams
            .get(&Semantic::Position)
            .ok_or_else(|| invalid(&apath, "no POSITION"))?
            .len();
        if let Some((s, _)) = streams.iter().find(|(_, v)| v.len() != count) {
            return Err(invalid(
                &apath,
                &format!("{s:?} count differs from POSITION's {count}"),
            ));
        }
        if let Some((name, _)) = custom.iter().find(|(_, d)| d.count != count) {
            return Err(invalid(
                &apath,
                &format!("{name} count differs from POSITION's {count}"),
            ));
        }
        for s in streams.keys() {
            let partner = match s {
                Semantic::Joints(n) => Semantic::Weights(*n),
                Semantic::Weights(n) => Semantic::Joints(*n),
                _ => continue,
            };
            if !streams.contains_key(&partner) {
                return Err(invalid(&apath, &format!("{s:?} without {partner:?}")));
            }
        }
        let targets = self.read_targets(p.get("targets"), count, &format!("{path}.targets"))?;
        let indices = p
            .usize_opt("indices")?
            .map(|a| {
                self.read_indices(a, count)
                    .map_err(|e| e.context(&format!("{path}.indices")))
            })
            .transpose()?;
        let n = indices.as_ref().map_or(count, Vec::len);
        let per = match topology {
            Topology::Triangles => 3,
            Topology::Lines => 2,
            Topology::Points => 1,
        };
        if !n.is_multiple_of(per) {
            return Err(invalid(path, &format!("{n} vertices is not whole {topology:?}")));
        }
        let material = p.usize_opt("material")?;
        let material_name = match material {
            None => None,
            Some(i) => {
                let mpath = format!("materials[{i}]");
                let mat = self
                    .array("materials")
                    .get(i)
                    .ok_or_else(|| invalid(&mpath, "does not exist"))?;
                Obj::new(mat, &mpath)?.str_opt("name")?.map(str::to_owned)
            }
        };
        Ok(ReadPrimitive {
            primitive: Primitive {
                topology,
                streams,
                indices,
                material: None,
            },
            material,
            material_name,
            custom,
            targets,
            extras: p.get("extras").cloned(),
        })
    }

    /// A primitive's morph targets: each a set of POSITION, NORMAL and TANGENT deltas of `count` elements.
    fn read_targets(&self, targets: Option<&Value>, count: usize, path: &str) -> Result<Vec<ReadTarget>> {
        let Some(targets) = targets else {
            return Ok(Vec::new());
        };
        let list = match targets {
            Value::Array(a) if !a.is_empty() => a,
            _ => return Err(invalid(path, "not a non-empty array")),
        };
        list.iter()
            .enumerate()
            .map(|(i, t)| {
                let tpath = format!("{path}[{i}]");
                let t = Obj::new(t, &tpath)?;
                if let Some(name) = t
                    .map
                    .keys()
                    .find(|k| !matches!(k.as_str(), "POSITION" | "NORMAL" | "TANGENT"))
                {
                    return Err(invalid(
                        &tpath,
                        &format!("unsupported morph target attribute {name}"),
                    ));
                }
                let delta = |name: &str| -> Result<Option<Vec<[f32; 3]>>> {
                    let Some(v) = t.get(name) else {
                        return Ok(None);
                    };
                    let acc = v
                        .as_u64()
                        .and_then(|a| usize::try_from(a).ok())
                        .ok_or_else(|| invalid(&tpath, &format!("{name} is not an accessor index")))?;
                    let data = self
                        .read_delta(acc)
                        .map_err(|e| e.context(&format!("{tpath}.{name}")))?;
                    if data.len() != count {
                        return Err(invalid(
                            &tpath,
                            &format!("{name} has {} elements, POSITION {count}", data.len()),
                        ));
                    }
                    Ok(Some(data))
                };
                Ok(ReadTarget {
                    position: delta("POSITION")?,
                    normal: delta("NORMAL")?,
                    tangent: delta("TANGENT")?,
                })
            })
            .collect()
    }

    /// A morph target attribute: VEC3, float or normalized byte/short.
    fn read_delta(&self, index: usize) -> Result<Vec<[f32; 3]>> {
        let acc = self.accessor(index)?;
        let bad = || {
            invalid(
                &acc.path,
                &format!(
                    "{} {} (normalized {}) is not allowed for a morph target delta",
                    acc.kind, acc.component, acc.normalized
                ),
            )
        };
        if acc.kind != "VEC3" {
            return Err(bad());
        }
        match (acc.component, acc.normalized) {
            (FLOAT, false) => acc.floats(),
            (BYTE, true) => Ok(acc
                .elements::<i8, 3>()?
                .into_iter()
                .map(|e| e.map(|c| round_f32((f64::from(c) / 127.0).max(-1.0))))
                .collect()),
            (SHORT, true) => Ok(acc
                .elements::<i16, 3>()?
                .into_iter()
                .map(|e| e.map(|c| round_f32((f64::from(c) / 32767.0).max(-1.0))))
                .collect()),
            _ => Err(bad()),
        }
    }

    /// Accessor `index`'s elements flattened to f32, for animation samplers: FLOAT as stored (finite), normalized
    /// integers dequantized by glTF 3.11 (`c / 255`, `c / 65535`, `max(c / 127, -1)`, `max(c / 32767, -1)`, in f64,
    /// rounded once). Unnormalized integers are refused; which types an animation path allows is the caller's.
    pub fn floats(&self, index: usize) -> Result<(ElementType, Vec<f32>)> {
        let acc = self.accessor(index)?;
        let component = acc.component_type()?;
        let element = acc.element_type();
        if element == ElementType::Mat4 && component != ComponentType::F32 {
            return Err(invalid(&acc.path, "a MAT4 is read as FLOAT only"));
        }
        let n = element.components();
        let v: Vec<f32> = match (component, acc.normalized) {
            (ComponentType::F32, false) => {
                let v: Vec<f32> = acc.scalars::<f32>(n)?;
                if let Some(k) = v.iter().position(|x| !x.is_finite()) {
                    return Err(invalid(&acc.path, &format!("component {k} is not finite")));
                }
                v
            }
            (ComponentType::U8, true) => acc
                .scalars::<u8>(n)?
                .into_iter()
                .map(|c| round_f32(f64::from(c) / 255.0))
                .collect(),
            (ComponentType::U16, true) => acc
                .scalars::<u16>(n)?
                .into_iter()
                .map(|c| round_f32(f64::from(c) / 65535.0))
                .collect(),
            (ComponentType::I8, true) => acc
                .scalars::<i8>(n)?
                .into_iter()
                .map(|c| round_f32((f64::from(c) / 127.0).max(-1.0)))
                .collect(),
            (ComponentType::I16, true) => acc
                .scalars::<i16>(n)?
                .into_iter()
                .map(|c| round_f32((f64::from(c) / 32767.0).max(-1.0)))
                .collect(),
            (c, normalized) => {
                return Err(invalid(
                    &acc.path,
                    &format!("{c:?} (normalized {normalized}) has no float form"),
                ));
            }
        };
        Ok((element, v))
    }

    /// Accessor `index`'s elements as stored.
    pub fn accessor_data(&self, index: usize) -> Result<AccessorData> {
        let acc = self.accessor(index)?;
        let component = acc.component_type()?;
        let element = acc.element_type();
        let mut bytes = Vec::new();
        for e in acc.element_bytes() {
            bytes.extend_from_slice(e?);
        }
        Ok(AccessorData {
            component,
            normalized: acc.normalized,
            element,
            count: acc.count,
            bytes,
        })
    }

    fn read_attribute(&self, semantic: Semantic, index: usize) -> Result<Stream> {
        let acc = self.accessor(index)?;
        let bad = || {
            invalid(
                &format!("accessors[{index}]"),
                &format!(
                    "{} {} (normalized {}) is not allowed for {semantic:?}",
                    acc.kind, acc.component, acc.normalized
                ),
            )
        };
        let float = acc.component == FLOAT && !acc.normalized;
        let unorm = acc.normalized && matches!(acc.component, UNSIGNED_BYTE | UNSIGNED_SHORT);
        let stream = match (semantic, acc.kind) {
            (Semantic::Position | Semantic::Normal, "VEC3") if float => Stream::F32x3(acc.floats()?),
            (Semantic::Tangent, "VEC4") if float => Stream::F32x4(acc.floats()?),
            (Semantic::TexCoord(_), "VEC2") if float => Stream::F32x2(acc.floats()?),
            (Semantic::TexCoord(_), "VEC2") if unorm => Stream::F32x2(acc.unorm_as_f32()?),
            (Semantic::Color(_), "VEC3") if float => Stream::F32x3(acc.floats()?),
            (Semantic::Color(_), "VEC4") if float => Stream::F32x4(acc.floats()?),
            (Semantic::Color(_), "VEC3") if unorm => Stream::F32x3(acc.unorm_as_f32()?),
            (Semantic::Color(_), "VEC4") if unorm && acc.component == UNSIGNED_BYTE => {
                Stream::U8x4(acc.ints()?)
            }
            (Semantic::Color(_), "VEC4") if unorm => Stream::U16x4(acc.ints()?),
            (Semantic::Joints(_), "VEC4") if !acc.normalized && acc.component == UNSIGNED_BYTE => {
                Stream::U8x4(acc.ints()?)
            }
            (Semantic::Joints(_), "VEC4") if !acc.normalized && acc.component == UNSIGNED_SHORT => {
                Stream::U16x4(acc.ints()?)
            }
            (Semantic::Weights(_), "VEC4") if float => Stream::F32x4(acc.floats()?),
            (Semantic::Weights(_), "VEC4") if unorm && acc.component == UNSIGNED_BYTE => {
                Stream::U8x4(acc.ints()?)
            }
            (Semantic::Weights(_), "VEC4") if unorm => Stream::U16x4(acc.ints()?),
            _ => return Err(bad()),
        };
        if semantic == Semantic::Position
            && let Stream::F32x3(p) = &stream
        {
            acc.check_bounds(p)?;
        }
        Ok(stream)
    }

    fn read_indices(&self, index: usize, vertex_count: usize) -> Result<Vec<u32>> {
        let acc = self.accessor(index)?;
        let path = format!("accessors[{index}]");
        if acc.kind != "SCALAR" || acc.normalized {
            return Err(invalid(&path, "indices are not an unnormalized SCALAR"));
        }
        if acc.declared_stride.is_some() {
            return Err(invalid(&path, "an index buffer view must not have a byteStride"));
        }
        let idx: Vec<u32> = match acc.component {
            UNSIGNED_BYTE => acc.ints::<u8, 1>()?.into_iter().map(|[i]| u32::from(i)).collect(),
            UNSIGNED_SHORT => acc
                .ints::<u16, 1>()?
                .into_iter()
                .map(|[i]| u32::from(i))
                .collect(),
            UNSIGNED_INT => acc.ints::<u32, 1>()?.into_iter().map(|[i]| i).collect(),
            c => return Err(invalid(&path, &format!("index component type {c}"))),
        };
        if let Some((k, i)) = idx.iter().enumerate().find(|(_, i)| **i as usize >= vertex_count) {
            return Err(invalid(
                &path,
                &format!("index {k} is {i}, {vertex_count} vertices"),
            ));
        }
        Ok(idx)
    }

    fn accessor(&self, index: usize) -> Result<Accessor<'_>> {
        let path = format!("accessors[{index}]");
        let a = self
            .array("accessors")
            .get(index)
            .ok_or_else(|| invalid(&path, "does not exist"))?;
        let a = Obj::new(a, &path)?;
        a.only(&[
            "bufferView",
            "byteOffset",
            "componentType",
            "normalized",
            "count",
            "type",
            "max",
            "min",
            "sparse",
            "name",
        ])?;
        let component = a.u64("componentType")?;
        let csize: usize = match component {
            BYTE | UNSIGNED_BYTE => 1,
            SHORT | UNSIGNED_SHORT => 2,
            UNSIGNED_INT | FLOAT => 4,
            c => return Err(invalid(&path, &format!("componentType {c}"))),
        };
        let kind: &'static str = match a.str_opt("type")? {
            Some("SCALAR") => "SCALAR",
            Some("VEC2") => "VEC2",
            Some("VEC3") => "VEC3",
            Some("VEC4") => "VEC4",
            Some("MAT4") => "MAT4",
            other => return Err(invalid(&path, &format!("type {other:?}"))),
        };
        let n: usize = match kind {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            _ => 16,
        };
        let count = a.usize("count")?;
        let offset = a.usize_opt("byteOffset")?.unwrap_or(0);
        let elem = csize
            .checked_mul(n)
            .ok_or_else(|| invalid(&path, "size overflows"))?;
        let normalized = a.bool_opt("normalized")?.unwrap_or(false);
        let (min, max) = (a.f32s_opt("min")?, a.f32s_opt("max")?);
        let sparse = a.get("sparse");
        // The dense part: the view's elements as laid out there, or none (glTF 3.6.2.1: zeros).
        let dense = match a.usize_opt("bufferView")? {
            None => {
                if a.get("byteOffset").is_some() {
                    return Err(invalid(&path, "byteOffset without a bufferView"));
                }
                None
            }
            Some(view) => {
                let (bytes, declared_stride) = self.view(view).map_err(|e| e.context(&path))?;
                let stride = declared_stride.unwrap_or(elem);
                if stride < elem || !stride.is_multiple_of(csize) || !offset.is_multiple_of(csize) {
                    return Err(invalid(&path, "misaligned offset or stride"));
                }
                let need = match count.checked_sub(1) {
                    None => 0,
                    Some(last) => last
                        .checked_mul(stride)
                        .and_then(|x| x.checked_add(offset.checked_add(elem)?))
                        .ok_or_else(|| invalid(&path, "size overflows"))?,
                };
                if need > bytes.len() {
                    return Err(invalid(&path, "runs past its buffer view"));
                }
                // no element (count 0) reads nothing, wherever the offset is
                Some((bytes.get(offset..).unwrap_or_default(), stride, declared_stride))
            }
        };
        let (bytes, stride, declared_stride) = match (dense, sparse) {
            (Some((bytes, stride, declared)), None) => (Cow::Borrowed(bytes), stride, declared),
            (dense, sparse) => {
                // Zeros or a sparse substitution: the elements are materialised tightly packed. The count is the
                // file's word and nothing else bounds it (a dense accessor is bounded by its view), so it is bounded here.
                if count > MAX_MATERIALISED_ELEMENTS {
                    return Err(invalid(
                        &path,
                        &format!(
                            "{count} elements without a bufferView or with a sparse substitution; the reader materialises \
                             at most {MAX_MATERIALISED_ELEMENTS}"
                        ),
                    ));
                }
                let total = count
                    .checked_mul(elem)
                    .ok_or_else(|| invalid(&path, "size overflows"))?;
                let mut out = Vec::new();
                out.try_reserve_exact(total)
                    .map_err(|_| invalid(&path, &format!("{total} bytes of elements cannot be allocated")))?;
                match dense {
                    Some((bytes, stride, _)) => {
                        for e in strided(bytes, stride, count, elem, &path) {
                            out.extend_from_slice(e?);
                        }
                    }
                    None => out.resize(total, 0),
                }
                if let Some(sparse) = sparse {
                    self.substitute(sparse, &path, count, csize, elem, &mut out)?;
                }
                (Cow::Owned(out), elem, dense.and_then(|(_, _, declared)| declared))
            }
        };
        Ok(Accessor {
            path,
            component,
            normalized,
            kind,
            count,
            bytes,
            elem,
            stride,
            declared_stride,
            min,
            max,
        })
    }

    /// Overwrite the elements `sparse` names (glTF 3.6.2.4) in `out`, the accessor's `count` tightly packed elements.
    fn substitute(
        &self,
        sparse: &Value,
        accessor: &str,
        count: usize,
        csize: usize,
        elem: usize,
        out: &mut [u8],
    ) -> Result<()> {
        let path = format!("{accessor}.sparse");
        let sparse = Obj::new(sparse, &path)?;
        sparse.only(&["count", "indices", "values"])?;
        let n = sparse.usize("count")?;
        if n == 0 || n > count {
            return Err(invalid(&path, &format!("count {n} outside 1-{count}")));
        }
        let ipath = format!("{path}.indices");
        let indices = Obj::new(
            sparse
                .get("indices")
                .ok_or_else(|| invalid(&path, "no indices"))?,
            &ipath,
        )?;
        indices.only(&["bufferView", "byteOffset", "componentType"])?;
        let vpath = format!("{path}.values");
        let values = Obj::new(
            sparse.get("values").ok_or_else(|| invalid(&path, "no values"))?,
            &vpath,
        )?;
        values.only(&["bufferView", "byteOffset"])?;

        // Both views hold tightly packed elements, so neither may declare a stride.
        let view = |o: &Obj, size: usize| -> Result<&[u8]> {
            let (bytes, stride) = self.view(o.usize("bufferView")?).map_err(|e| e.context(o.path))?;
            if stride.is_some() {
                return Err(invalid(o.path, "a sparse buffer view must not have a byteStride"));
            }
            let offset = o.usize_opt("byteOffset")?.unwrap_or(0);
            if !offset.is_multiple_of(size.min(4)) {
                return Err(invalid(
                    o.path,
                    &format!("byteOffset {offset} is not a multiple of {size}"),
                ));
            }
            n.checked_mul(size)
                .and_then(|x| x.checked_add(offset))
                .and_then(|end| bytes.get(offset..end))
                .ok_or_else(|| invalid(o.path, "runs past its buffer view"))
        };
        let isize = match indices.u64("componentType")? {
            UNSIGNED_BYTE => 1,
            UNSIGNED_SHORT => 2,
            UNSIGNED_INT => 4,
            c => return Err(invalid(&ipath, &format!("componentType {c}"))),
        };
        let idx = view(&indices, isize)?;
        let vals = view(&values, elem)?;
        if !values.usize_opt("byteOffset")?.unwrap_or(0).is_multiple_of(csize) {
            return Err(invalid(
                &vpath,
                &format!("byteOffset is not a multiple of {csize}"),
            ));
        }
        let mut previous = None;
        // n indices of isize bytes and n values of elem bytes, the views' lengths exactly
        for (k, (at, value)) in idx.chunks_exact(isize).zip(vals.chunks_exact(elem)).enumerate() {
            let i = match *at {
                [a] => usize::from(a),
                [a, b] => usize::from(u16::from_le_bytes([a, b])),
                [a, b, c, d] => usize::try_from(u32::from_le_bytes([a, b, c, d]))
                    .map_err(|_| invalid(&ipath, "index too large"))?,
                _ => return Err(invalid(&ipath, &format!("index {k} is not {isize} bytes"))),
            };
            if i >= count {
                return Err(invalid(&ipath, &format!("index {k} is {i}, {count} elements")));
            }
            if previous.is_some_and(|p| p >= i) {
                return Err(invalid(&ipath, &format!("index {k} ({i}) does not increase")));
            }
            previous = Some(i);
            // i < count, and `out` is count elements
            let slot = i
                .checked_mul(elem)
                .and_then(|start| out.get_mut(start..start.checked_add(elem)?))
                .ok_or_else(|| invalid(&ipath, &format!("index {k} ({i}) is past the elements")))?;
            slot.copy_from_slice(value);
        }
        Ok(())
    }
}

/// The document's checked header (asset version 2.0, no required extension) and its `buffers`.
fn document_buffers(json: &Value) -> Result<&[Value]> {
    let root = Obj::new(json, "")?;
    let asset = Obj::new(
        root.get("asset").ok_or_else(|| invalid("asset", "missing"))?,
        "asset",
    )?;
    if asset.str_opt("version")? != Some("2.0") {
        return Err(invalid("asset.version", "not \"2.0\""));
    }
    if let Some(req) = root.get("extensionsRequired")
        && req.as_array().is_none_or(|a| !a.is_empty())
    {
        return Err(invalid("extensionsRequired", &format!("unsupported: {req}")));
    }
    match root.get("buffers") {
        None => Ok(&[]),
        Some(Value::Array(a)) => Ok(a.as_slice()),
        Some(_) => Err(invalid("buffers", "not an array")),
    }
}

/// glTF attribute name -> semantic, for the attributes the reader takes.
fn semantic(name: &str) -> Option<Semantic> {
    // Only the canonical spelling: `TEXCOORD_01` would alias `TEXCOORD_1`.
    let set = |prefix: &str| {
        name.strip_prefix(prefix)
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| name == format!("{prefix}{n}"))
    };
    match name {
        "POSITION" => Some(Semantic::Position),
        "NORMAL" => Some(Semantic::Normal),
        "TANGENT" => Some(Semantic::Tangent),
        _ => set("TEXCOORD_")
            .map(Semantic::TexCoord)
            .or_else(|| set("COLOR_").map(Semantic::Color))
            .or_else(|| set("JOINTS_").map(Semantic::Joints))
            .or_else(|| set("WEIGHTS_").map(Semantic::Weights)),
    }
}

struct Accessor<'a> {
    path: String,
    component: u64,
    normalized: bool,
    kind: &'static str,
    count: usize,
    /// From the accessor's first element to the end of its view; or, sparse or without a view, its elements
    /// materialised tightly packed.
    bytes: Cow<'a, [u8]>,
    /// An element's size in bytes, at most `stride`.
    elem: usize,
    stride: usize,
    declared_stride: Option<usize>,
    min: Option<Vec<f32>>,
    max: Option<Vec<f32>>,
}

/// A little-endian scalar the accessors hold: read from the start of `b`, `None` when `b` is shorter.
trait Scalar: Copy + Default {
    const SIZE: usize;
    fn from_le(b: &[u8]) -> Option<Self>;
}
impl Scalar for u8 {
    const SIZE: usize = 1;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first().copied()
    }
}
impl Scalar for i8 {
    const SIZE: usize = 1;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first_chunk().map(|&a| i8::from_le_bytes(a))
    }
}
impl Scalar for i16 {
    const SIZE: usize = 2;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first_chunk().map(|&a| i16::from_le_bytes(a))
    }
}
impl Scalar for u16 {
    const SIZE: usize = 2;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first_chunk().map(|&a| u16::from_le_bytes(a))
    }
}
impl Scalar for u32 {
    const SIZE: usize = 4;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first_chunk().map(|&a| u32::from_le_bytes(a))
    }
}
impl Scalar for f32 {
    const SIZE: usize = 4;
    fn from_le(b: &[u8]) -> Option<Self> {
        b.first_chunk().map(|&a| f32::from_le_bytes(a))
    }
}

/// `count` elements of `size` bytes, `stride` apart from the start of `bytes` (`size` at most `stride`, which is not
/// 0): each element's bytes, or an error naming `path` for one that does not end inside `bytes`.
fn strided<'a>(
    bytes: &'a [u8],
    stride: usize,
    count: usize,
    size: usize,
    path: &'a str,
) -> impl Iterator<Item = Result<&'a [u8]>> + 'a {
    let mut at = bytes.chunks(stride);
    (0..count).map(move |k| {
        at.next()
            .and_then(|c| c.get(..size))
            .ok_or_else(|| invalid(path, &format!("element {k} runs past its bytes")))
    })
}

/// `v` rounded to the nearest f32: the one rounding of a value dequantized in f64 (glTF 3.11).
#[allow(
    clippy::cast_possible_truncation,
    reason = "rounding to f32 is the stated conversion"
)]
fn round_f32(v: f64) -> f32 {
    v as f32
}

impl Accessor<'_> {
    fn component_type(&self) -> Result<ComponentType> {
        let component = match self.component {
            BYTE => ComponentType::I8,
            UNSIGNED_BYTE => ComponentType::U8,
            SHORT => ComponentType::I16,
            UNSIGNED_SHORT => ComponentType::U16,
            UNSIGNED_INT => ComponentType::U32,
            FLOAT => ComponentType::F32,
            c => return Err(invalid(&self.path, &format!("componentType {c}"))),
        };
        // glTF 3.6.2.3: only 8- and 16-bit integers are normalized
        if self.normalized && matches!(component, ComponentType::U32 | ComponentType::F32) {
            return Err(invalid(&self.path, &format!("{component:?} is normalized")));
        }
        Ok(component)
    }

    fn element_type(&self) -> ElementType {
        match self.kind {
            "SCALAR" => ElementType::Scalar,
            "VEC2" => ElementType::Vec2,
            "VEC3" => ElementType::Vec3,
            "VEC4" => ElementType::Vec4,
            _ => ElementType::Mat4,
        }
    }

    /// Each element's bytes in order (the accessor checked that the last ends inside its bytes).
    fn element_bytes(&self) -> impl Iterator<Item = Result<&[u8]>> {
        strided(&self.bytes, self.stride, self.count, self.elem, &self.path)
    }

    /// An element holds fewer scalars than its type says: `T` is not the component type (callers check it).
    fn short(&self) -> Error {
        invalid(&self.path, "an element holds fewer components than its type")
    }

    /// Every component of every element in order, `n` per element; `T` must match the component type.
    fn scalars<T: Scalar>(&self, n: usize) -> Result<Vec<T>> {
        let mut out = Vec::with_capacity(self.count.saturating_mul(n));
        for e in self.element_bytes() {
            let mut components = e?.chunks_exact(T::SIZE).map(T::from_le);
            for _ in 0..n {
                out.push(components.next().flatten().ok_or_else(|| self.short())?);
            }
        }
        Ok(out)
    }

    /// Elements of `N` scalars of `T`; `T` must match the component type (callers check it).
    fn elements<T: Scalar, const N: usize>(&self) -> Result<Vec<[T; N]>> {
        self.element_bytes()
            .map(|e| {
                let mut components = e?.chunks_exact(T::SIZE).map(T::from_le);
                let mut v = [T::default(); N];
                for c in &mut v {
                    *c = components.next().flatten().ok_or_else(|| self.short())?;
                }
                Ok(v)
            })
            .collect()
    }

    fn floats<const N: usize>(&self) -> Result<Vec<[f32; N]>> {
        let v = self.elements::<f32, N>()?;
        if let Some(k) = v.iter().position(|e| e.iter().any(|x| !x.is_finite())) {
            return Err(invalid(&self.path, &format!("element {k} is not finite")));
        }
        Ok(v)
    }

    fn ints<T: Scalar, const N: usize>(&self) -> Result<Vec<[T; N]>> {
        let expect = match T::SIZE {
            1 => UNSIGNED_BYTE,
            2 => UNSIGNED_SHORT,
            _ => UNSIGNED_INT,
        };
        if self.component != expect {
            return Err(invalid(&self.path, &format!("componentType {}", self.component)));
        }
        self.elements::<T, N>()
    }

    /// Normalized unsigned integers dequantized to f32 (`c / max`, glTF §3.11), rounded once from f64.
    fn unorm_as_f32<const N: usize>(&self) -> Result<Vec<[f32; N]>> {
        Ok(match self.component {
            UNSIGNED_BYTE => self
                .elements::<u8, N>()?
                .into_iter()
                .map(|e| e.map(|c| round_f32(f64::from(c) / 255.0)))
                .collect(),
            _ => self
                .elements::<u16, N>()?
                .into_iter()
                .map(|e| e.map(|c| round_f32(f64::from(c) / 65535.0)))
                .collect(),
        })
    }

    /// POSITION's `min`/`max`, when given, must be the exact bounds of the data (glTF requires both; a mismatch
    /// means a broken writer).
    fn check_bounds(&self, p: &[[f32; 3]]) -> Result<()> {
        let (Some(min), Some(max)) = (&self.min, &self.max) else {
            return Ok(());
        };
        let Some((lo, hi)) = bounds(p) else {
            return Ok(());
        };
        if min.as_slice() != lo || max.as_slice() != hi {
            return Err(invalid(
                &self.path,
                &format!("min/max {min:?}/{max:?} are not the data's {lo:?}/{hi:?}"),
            ));
        }
        Ok(())
    }
}

/// Component-wise bounds of points; `None` for none.
pub fn bounds(p: &[[f32; 3]]) -> Option<([f32; 3], [f32; 3])> {
    let first = *p.first()?;
    Some(
        p.iter()
            .fold((first, first), |([l0, l1, l2], [h0, h1, h2]), &[x, y, z]| {
                (
                    [l0.min(x), l1.min(y), l2.min(z)],
                    [h0.max(x), h1.max(y), h2.max(z)],
                )
            }),
    )
}

impl Error {
    /// Wrap an error with the item that was being read when it happened.
    pub(crate) fn context(self, item: &str) -> Error {
        match self {
            Error::Invalid { item: inner, detail } if !inner.is_empty() => Error::Invalid {
                item: format!("{item} -> {inner}"),
                detail,
            },
            Error::Invalid { detail, .. } => Error::Invalid {
                item: item.to_owned(),
                detail,
            },
            other => other,
        }
    }
}
