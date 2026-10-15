//! The strict GLB reader: meshes into [`primase_core::Mesh`], plus checked access to the rest of the document for
//! round-trip tests.
//!
//! What it reads (a superset of skate-scene v1's prototype GLB rules): one buffer held in
//! the BIN chunk; primitives of mode points, lines or triangles; `indices` unsigned byte, short or int (or none:
//! sequential); `POSITION` float VEC3, `NORMAL` float VEC3, `TANGENT` float VEC4, `TEXCOORD_n` float or normalized
//! unsigned byte/short VEC2, `COLOR_n` float or normalized unsigned byte/short VEC3/VEC4. Everything else that could
//! change the geometry is an error naming the item: sparse accessors, morph targets, skin attributes, external or
//! extra buffers, required extensions, any extension on what is read, unknown properties.

use std::collections::BTreeMap;

use primase_core::{Mesh, Primitive, Semantic, Stream, Topology};
use serde_json::Value;

use crate::json::{Obj, invalid};
use crate::{Error, Result, glb};

const BYTE: u64 = 5120;
const UNSIGNED_BYTE: u64 = 5121;
const SHORT: u64 = 5122;
const UNSIGNED_SHORT: u64 = 5123;
const UNSIGNED_INT: u64 = 5125;
const FLOAT: u64 = 5126;

/// A parsed GLB: its JSON document and its binary buffer.
#[derive(Debug)]
pub struct Gltf {
    json: Value,
    /// The buffer, `buffers[0].byteLength` bytes (without the BIN chunk's padding); empty when there is none.
    bin: Vec<u8>,
}

/// One primitive as read: the core primitive (its `material` entity left `None`) and the glTF material it named.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadPrimitive {
    pub primitive: Primitive,
    /// Index into the document's `materials`.
    pub material: Option<usize>,
    /// That material's `name`.
    pub material_name: Option<String>,
}

/// One glTF mesh as read.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadMesh {
    pub name: Option<String>,
    pub extras: Option<Value>,
    pub primitives: Vec<ReadPrimitive>,
}

impl ReadMesh {
    /// The core mesh, every primitive's material `None`.
    pub fn to_mesh(&self) -> Mesh {
        Mesh {
            primitives: self.primitives.iter().map(|p| p.primitive.clone()).collect(),
        }
    }
}

impl Gltf {
    /// Parse a GLB: the container, the JSON, the asset version, required extensions and the buffer.
    pub fn from_glb(bytes: &[u8]) -> Result<Gltf> {
        let chunks = glb::parse(bytes)?;
        let json: Value = serde_json::from_slice(chunks.json)?;
        let root = Obj::new(&json, "")?;
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
        let buffers = match root.get("buffers") {
            None => &[][..],
            Some(Value::Array(a)) => a.as_slice(),
            Some(_) => return Err(invalid("buffers", "not an array")),
        };
        let bin = match (buffers, chunks.bin) {
            ([], None) => Vec::new(),
            ([], Some(_)) => return Err(invalid("buffers", "a BIN chunk but no buffer")),
            ([b], bin) => {
                let b = Obj::new(b, "buffers[0]")?;
                b.only(&["byteLength", "name"])?;
                let len = b.usize("byteLength")?;
                let bin =
                    bin.ok_or_else(|| invalid("buffers[0]", "no BIN chunk (external buffers are not read)"))?;
                // The BIN chunk is the buffer padded to 4 bytes, nothing more.
                if bin.len() != len.next_multiple_of(4) {
                    return Err(invalid(
                        "buffers[0]",
                        &format!("byteLength {len} does not match the {}-byte BIN chunk", bin.len()),
                    ));
                }
                bin[..len].to_vec()
            }
            _ => return Err(invalid("buffers", "more than one buffer")),
        };
        Ok(Gltf { json, bin })
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
        if v.u64("buffer")? != 0 {
            return Err(invalid(&path, "buffer is not 0"));
        }
        let off = v.usize_opt("byteOffset")?.unwrap_or(0);
        let len = v.usize("byteLength")?;
        let stride = v.usize_opt("byteStride")?;
        if let Some(s) = stride
            && !(4..=252).contains(&s)
        {
            return Err(invalid(&path, &format!("byteStride {s} outside 4-252")));
        }
        let end = off
            .checked_add(len)
            .filter(|&e| e <= self.bin.len())
            .ok_or_else(|| invalid(&path, "runs past the buffer"))?;
        Ok((&self.bin[off..end], stride))
    }

    /// Read mesh `index` into core primitives.
    pub fn read_mesh(&self, index: usize) -> Result<ReadMesh> {
        let path = format!("meshes[{index}]");
        let m = self
            .array("meshes")
            .get(index)
            .ok_or_else(|| invalid(&path, "does not exist"))?;
        let m = Obj::new(m, &path)?;
        m.only(&["primitives", "name"])?;
        let prims = match m.get("primitives") {
            Some(Value::Array(a)) if !a.is_empty() => a,
            _ => return Err(invalid(&path, "primitives missing or empty")),
        };
        let primitives = prims
            .iter()
            .enumerate()
            .map(|(i, p)| self.read_primitive(p, &format!("{path}.primitives[{i}]")))
            .collect::<Result<_>>()?;
        Ok(ReadMesh {
            name: m.str_opt("name")?.map(str::to_owned),
            extras: m.get("extras").cloned(),
            primitives,
        })
    }

    fn read_primitive(&self, p: &Value, path: &str) -> Result<ReadPrimitive> {
        let p = Obj::new(p, path)?;
        p.only(&["attributes", "indices", "material", "mode"])?;
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
        for (name, acc) in attrs.map {
            let acc = acc
                .as_u64()
                .and_then(|a| usize::try_from(a).ok())
                .ok_or_else(|| invalid(&apath, &format!("{name} is not an accessor index")))?;
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
            "name",
        ])?;
        if a.get("sparse").is_some() {
            return Err(invalid(&path, "sparse"));
        }
        let view = a
            .usize_opt("bufferView")?
            .ok_or_else(|| invalid(&path, "no bufferView (implicit zeros are not read)"))?;
        let component = a.u64("componentType")?;
        let csize = match component {
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
            other => return Err(invalid(&path, &format!("type {other:?}"))),
        };
        let n = match kind {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            _ => 4,
        };
        let count = a.usize("count")?;
        let offset = a.usize_opt("byteOffset")?.unwrap_or(0);
        let (bytes, declared_stride) = self.view(view).map_err(|e| e.context(&path))?;
        let elem = csize * n;
        let stride = declared_stride.unwrap_or(elem);
        if stride < elem || stride % csize != 0 || offset % csize != 0 {
            return Err(invalid(&path, "misaligned offset or stride"));
        }
        let need = if count == 0 {
            0
        } else {
            (count - 1)
                .checked_mul(stride)
                .and_then(|x| x.checked_add(offset + elem))
                .ok_or_else(|| invalid(&path, "size overflows"))?
        };
        if need > bytes.len() {
            return Err(invalid(&path, "runs past its buffer view"));
        }
        let normalized = a.bool_opt("normalized")?.unwrap_or(false);
        let (min, max) = (a.f32s_opt("min")?, a.f32s_opt("max")?);
        Ok(Accessor {
            path,
            component,
            normalized,
            kind,
            count,
            bytes: &bytes[offset.min(bytes.len())..],
            stride,
            declared_stride,
            min,
            max,
        })
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
            .or_else(|| set("COLOR_").map(Semantic::Color)),
    }
}

struct Accessor<'a> {
    path: String,
    component: u64,
    normalized: bool,
    kind: &'static str,
    count: usize,
    /// From the accessor's first element to the end of its view.
    bytes: &'a [u8],
    stride: usize,
    declared_stride: Option<usize>,
    min: Option<Vec<f32>>,
    max: Option<Vec<f32>>,
}

/// A little-endian scalar the accessors hold.
trait Scalar: Copy + Default {
    const SIZE: usize;
    fn from_le(b: &[u8]) -> Self;
}
impl Scalar for u8 {
    const SIZE: usize = 1;
    fn from_le(b: &[u8]) -> Self {
        b[0]
    }
}
impl Scalar for u16 {
    const SIZE: usize = 2;
    fn from_le(b: &[u8]) -> Self {
        u16::from_le_bytes([b[0], b[1]])
    }
}
impl Scalar for u32 {
    const SIZE: usize = 4;
    fn from_le(b: &[u8]) -> Self {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
}
impl Scalar for f32 {
    const SIZE: usize = 4;
    fn from_le(b: &[u8]) -> Self {
        f32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
}

impl Accessor<'_> {
    /// Elements of `N` scalars of `T`; `T` must match the component type (callers check it).
    fn elements<T: Scalar, const N: usize>(&self) -> Vec<[T; N]> {
        (0..self.count)
            .map(|k| {
                let at = k * self.stride;
                std::array::from_fn(|c| T::from_le(&self.bytes[at + c * T::SIZE..]))
            })
            .collect()
    }

    fn floats<const N: usize>(&self) -> Result<Vec<[f32; N]>> {
        let v = self.elements::<f32, N>();
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
        Ok(self.elements::<T, N>())
    }

    /// Normalized unsigned integers dequantized to f32 (`c / max`, glTF §3.11), rounded once from f64.
    fn unorm_as_f32<const N: usize>(&self) -> Result<Vec<[f32; N]>> {
        Ok(match self.component {
            UNSIGNED_BYTE => self
                .elements::<u8, N>()
                .into_iter()
                .map(|e| e.map(|c| (f64::from(c) / 255.0) as f32))
                .collect(),
            _ => self
                .elements::<u16, N>()
                .into_iter()
                .map(|e| e.map(|c| (f64::from(c) / 65535.0) as f32))
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
pub(crate) fn bounds(p: &[[f32; 3]]) -> Option<([f32; 3], [f32; 3])> {
    let first = *p.first()?;
    Some(p.iter().fold((first, first), |(lo, hi), v| {
        (
            std::array::from_fn(|i| lo[i].min(v[i])),
            std::array::from_fn(|i| hi[i].max(v[i])),
        )
    }))
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
