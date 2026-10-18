//! A Shader's compiled programs, as a player stores them: per pass, the fragment program's variants (each a keyword
//! set, a GPU program type and an entry of the compressed blob) and the parameters common to them (constant buffers
//! with each field's name and byte offset, texture and sampler bind slots), read from `m_ParsedForm`; and a variant's
//! code from the platform's LZ4-compressed blob. What the bindings mean for a material is the caller's.
//!
//! The blob of one platform is split into segments (`offsets`, `compressedLengths`, `decompressedLengths`, one
//! entry per segment). Segment 0 starts with the entry table: a count, then per entry its offset, length and segment.
//! An entry of a program holds Unity's program record: its version, GPU program type, then (from 2016.2) a header the
//! reader skips, keyword strings (empty in current players: the parsed form names them), and the code as a byte array.

use primase_compress::{Decompressor, Lz4};

use crate::player::{ObjRef, Player};
use crate::value::{Array, Value};
use crate::{Result, invalid};

/// A constant-buffer field: its name, byte offset in the buffer, element type (Unity's `ShaderParamType`: 0 float, 1
/// int, 2 bool, 3 half, 4 short, 5 uint), shape (a vector of `columns` lanes, or a matrix of `rows` rows) and array
/// size (0: not an array).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub kind: i64,
    pub shape: Shape,
    pub array_size: u32,
}

/// A field's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Shape {
    Vector { columns: u32 },
    Matrix { rows: u32 },
}

/// A constant buffer: its name, the bind slot it is bound to (`b<n>`; `None` when no binding names it), its size in
/// bytes and its fields.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConstantBuffer {
    pub name: String,
    pub slot: Option<u32>,
    pub size: u32,
    pub fields: Vec<Field>,
}

/// A texture parameter: its name, bind slot (`t<n>`), the sampler slot it samples with (`None`: none of its own) and
/// dimension (Unity's `TextureDimension`: 2 a 2D texture, 3 3D, 4 a cube).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TextureParam {
    pub name: String,
    pub slot: u32,
    pub sampler: Option<u32>,
    pub dim: i64,
}

/// The parameters the variants of a program share.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Parameters {
    pub constant_buffers: Vec<ConstantBuffer>,
    pub textures: Vec<TextureParam>,
    /// Sampler states: Unity's packed sampler value and its bind slot (`s<n>`).
    pub samplers: Vec<(u32, u32)>,
}

/// One compiled variant: the blob entry holding its code, its keywords, its GPU program type (Unity's
/// `ShaderGpuProgramType`: 18 a D3D11 SM 5.0 pixel program) and its shader requirements mask; `group` is the index
/// of the player sub-program list it is in.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Variant {
    pub group: usize,
    pub blob_index: u32,
    pub keywords: Vec<String>,
    pub gpu_program_type: i64,
    pub requirements: i64,
}

/// One pass's fragment program.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PassProgram {
    pub variants: Vec<Variant>,
    pub parameters: Parameters,
}

/// A Shader's compiled fragment programs: `passes[subshader][pass]`, and its per-platform blobs.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPrograms {
    pub name: String,
    pub passes: Vec<Vec<PassProgram>>,
    /// Unity's `ShaderCompilerPlatform` of each blob (4: D3D11).
    pub platforms: Vec<i64>,
    /// Per platform, per segment: offset, compressed and decompressed length.
    segments: Vec<Vec<(usize, usize, usize)>>,
    blob: Vec<u8>,
}

fn items<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match v.get(k) {
        Some(Value::Array(Array::Values(items))) => Ok(items),
        Some(Value::Array(a)) if a.is_empty() => Ok(&[]),
        None => Ok(&[]),
        _ => Err(invalid(format!("shader program: {k} is not a list"))),
    }
}

/// An array of integers, whatever element type the type tree gave it.
fn ints(v: &Value) -> Result<Vec<i64>> {
    let bad = || invalid("shader program: an integer array of another type");
    Ok(match v {
        Value::Array(a) => match a {
            Array::I8(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::U8(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::I16(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::U16(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::I32(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::U32(x) => x.iter().map(|&n| i64::from(n)).collect(),
            Array::I64(x) => x.clone(),
            Array::U64(x) => x
                .iter()
                .map(|&n| i64::try_from(n).map_err(|_| bad()))
                .collect::<Result<_>>()?,
            Array::Values(x) => x
                .iter()
                .map(|n| n.as_i64().ok_or_else(bad))
                .collect::<Result<_>>()?,
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    })
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(format!("shader program: no integer {k}")))
}

fn u32_of(n: i64, what: &str) -> Result<u32> {
    u32::try_from(n).map_err(|_| invalid(format!("shader program: {what} {n} out of range")))
}

/// The parameters as stored, their names resolved through the pass's name table.
fn parameters(v: &Value, names: &std::collections::BTreeMap<i64, String>) -> Result<Parameters> {
    let name = |e: &Value| -> Result<String> {
        let i = int(e, "m_NameIndex")?;
        names
            .get(&i)
            .cloned()
            .ok_or_else(|| invalid(format!("shader program: name index {i} past the pass's names")))
    };
    let bindings = items(v, "m_ConstantBufferBindings")?
        .iter()
        .map(|b| {
            Ok((
                int(b, "m_NameIndex")?,
                u32_of(int(b, "m_Index")?, "binding slot")?,
            ))
        })
        .collect::<Result<std::collections::BTreeMap<i64, u32>>>()?;
    let mut constant_buffers = Vec::new();
    for cb in items(v, "m_ConstantBuffers")? {
        let mut fields = Vec::new();
        for (key, matrix) in [("m_VectorParams", false), ("m_MatrixParams", true)] {
            for f in items(cb, key)? {
                fields.push(Field {
                    name: name(f)?,
                    offset: u32_of(int(f, "m_Index")?, "field offset")?,
                    kind: int(f, "m_Type")?,
                    shape: if matrix {
                        Shape::Matrix {
                            rows: u32_of(int(f, "m_RowCount")?, "matrix rows")?,
                        }
                    } else {
                        Shape::Vector {
                            columns: u32_of(int(f, "m_Dim")?, "vector size")?,
                        }
                    },
                    array_size: u32_of(int(f, "m_ArraySize")?, "array size")?,
                });
            }
        }
        if !items(cb, "m_StructParams")?.is_empty() {
            return Err(crate::Error::Unsupported(
                "constant buffers with struct fields".into(),
            ));
        }
        fields.sort_by_key(|f| f.offset);
        constant_buffers.push(ConstantBuffer {
            name: name(cb)?,
            slot: bindings.get(&int(cb, "m_NameIndex")?).copied(),
            size: u32_of(int(cb, "m_Size")?, "buffer size")?,
            fields,
        });
    }
    let textures = items(v, "m_TextureParams")?
        .iter()
        .map(|t| {
            let sampler = int(t, "m_SamplerIndex")?;
            Ok(TextureParam {
                name: name(t)?,
                slot: u32_of(int(t, "m_Index")?, "texture slot")?,
                sampler: (sampler >= 0)
                    .then(|| u32_of(sampler, "sampler slot"))
                    .transpose()?,
                dim: int(t, "m_Dim")?,
            })
        })
        .collect::<Result<_>>()?;
    let samplers = items(v, "m_Samplers")?
        .iter()
        .map(|s| {
            Ok((
                u32_of(int(s, "sampler")?, "sampler state")?,
                u32_of(int(s, "bindPoint")?, "sampler slot")?,
            ))
        })
        .collect::<Result<_>>()?;
    Ok(Parameters {
        constant_buffers,
        textures,
        samplers,
    })
}

impl ShaderPrograms {
    /// Read the Shader `r` of `player`.
    pub fn read(player: &Player, r: &ObjRef) -> Result<Self> {
        let s = player.decode(r)?;
        let form = s
            .get("m_ParsedForm")
            .ok_or_else(|| invalid(format!("shader {}/{}: no m_ParsedForm", r.file, r.path_id)))?;
        let name = form
            .get("m_Name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let keyword_names: Vec<String> = items(form, "m_KeywordNames")?
            .iter()
            .map(|k| {
                k.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("shader: a keyword name"))
            })
            .collect::<Result<_>>()?;
        let mut passes = Vec::new();
        for sub in items(form, "m_SubShaders")? {
            let mut out = Vec::new();
            for p in items(sub, "m_Passes")? {
                // the pass's name table: name -> index
                let mut names = std::collections::BTreeMap::new();
                for pair in items(p, "m_NameIndices")? {
                    let Value::Pair(kv) = pair else {
                        return Err(invalid("shader pass: m_NameIndices entry is not a pair"));
                    };
                    let key = kv.0.as_str().ok_or_else(|| invalid("shader pass: a name"))?;
                    let index =
                        kv.1.as_i64()
                            .ok_or_else(|| invalid("shader pass: a name index"))?;
                    names.insert(index, key.to_owned());
                }
                let Some(fragment) = p.get("progFragment") else {
                    out.push(PassProgram::default());
                    continue;
                };
                let mut variants = Vec::new();
                for (group, list) in items(fragment, "m_PlayerSubPrograms")?.iter().enumerate() {
                    let list: &[Value] = match list {
                        Value::Array(Array::Values(x)) => x,
                        Value::Array(a) if a.is_empty() => &[],
                        _ => return Err(invalid("shader pass: a player sub-program list")),
                    };
                    for v in list {
                        let keywords = match v.get("m_KeywordIndices") {
                            Some(k) => ints(k)?
                                .into_iter()
                                .map(|i| {
                                    usize::try_from(i)
                                        .ok()
                                        .and_then(|i| keyword_names.get(i))
                                        .cloned()
                                        .ok_or_else(|| invalid(format!("shader {name}: keyword index {i}")))
                                })
                                .collect::<Result<_>>()?,
                            None => Vec::new(),
                        };
                        variants.push(Variant {
                            group,
                            blob_index: u32_of(int(v, "m_BlobIndex")?, "blob index")?,
                            keywords,
                            gpu_program_type: int(v, "m_GpuProgramType")?,
                            requirements: v.get("m_ShaderRequirements").and_then(Value::as_i64).unwrap_or(0),
                        });
                    }
                }
                let parameters = match fragment.get("m_CommonParameters") {
                    Some(c) => parameters(c, &names)?,
                    None => Parameters::default(),
                };
                out.push(PassProgram { variants, parameters });
            }
            passes.push(out);
        }
        let platforms = s.get("platforms").map(ints).transpose()?.unwrap_or_default();
        let per = |k: &str| -> Result<Vec<Vec<i64>>> {
            items(&s, k)?
                .iter()
                .map(|v| match v {
                    Value::Array(_) => ints(v),
                    x => Ok(vec![
                        x.as_i64().ok_or_else(|| invalid(format!("shader: {k} entry")))?,
                    ]),
                })
                .collect()
        };
        let (offsets, packed, unpacked) = (
            per("offsets")?,
            per("compressedLengths")?,
            per("decompressedLengths")?,
        );
        if offsets.len() != platforms.len()
            || packed.len() != platforms.len()
            || unpacked.len() != platforms.len()
        {
            return Err(invalid(format!(
                "shader {name}: blob tables of another length than its platforms"
            )));
        }
        let segments = (0..platforms.len())
            .map(|i| {
                let (o, p, u) = (&offsets[i], &packed[i], &unpacked[i]);
                if o.len() != p.len() || o.len() != u.len() {
                    return Err(invalid(format!(
                        "shader {name}: segment tables of different lengths"
                    )));
                }
                (0..o.len())
                    .map(|k| {
                        let size =
                            |n: i64| usize::try_from(n).map_err(|_| invalid("shader: a negative blob size"));
                        Ok((size(o[k])?, size(p[k])?, size(u[k])?))
                    })
                    .collect()
            })
            .collect::<Result<_>>()?;
        let blob = s
            .get("compressedBlob")
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        Ok(ShaderPrograms {
            name,
            passes,
            platforms,
            segments,
            blob,
        })
    }

    /// Segment `k` of platform `platform`'s blob, decompressed.
    fn segment(&self, platform: usize, k: usize) -> Result<Vec<u8>> {
        let &(offset, packed, unpacked) = self
            .segments
            .get(platform)
            .and_then(|s| s.get(k))
            .ok_or_else(|| invalid(format!("shader {}: no blob segment {k}", self.name)))?;
        let bytes = offset
            .checked_add(packed)
            .and_then(|end| self.blob.get(offset..end))
            .ok_or_else(|| invalid(format!("shader {}: blob segment {k} past the blob", self.name)))?;
        Lz4.decompress(bytes, unpacked)
            .map_err(|e| invalid(format!("shader {}: blob segment {k}: {e}", self.name)))
    }

    /// The code of blob entry `index` of the blob for `platform` (Unity's `ShaderCompilerPlatform`): the program
    /// record's code bytes, as stored.
    pub fn code(&self, platform: i64, index: u32) -> Result<Vec<u8>> {
        let p = self
            .platforms
            .iter()
            .position(|&x| x == platform)
            .ok_or_else(|| invalid(format!("shader {}: no blob for platform {platform}", self.name)))?;
        let table = self.segment(p, 0)?;
        let mut r = crate::Reader::new(&table, false);
        let count = usize::try_from(r.i32()?).map_err(|_| invalid("shader blob: a negative entry count"))?;
        let at = 4 + usize::try_from(index).map_err(|_| invalid("shader blob: an entry index"))? * 12;
        if index as usize >= count {
            return Err(invalid(format!(
                "shader {}: blob entry {index} of {count}",
                self.name
            )));
        }
        r.pos = at;
        let (offset, length, segment) = (r.i32()?, r.i32()?, r.i32()?);
        let span = |n: i32| usize::try_from(n).map_err(|_| invalid("shader blob: a negative entry span"));
        let (offset, length, segment) = (span(offset)?, span(length)?, span(segment)?);
        let data = if segment == 0 {
            table
        } else {
            self.segment(p, segment)?
        };
        let entry = offset
            .checked_add(length)
            .and_then(|end| data.get(offset..end))
            .ok_or_else(|| {
                invalid(format!(
                    "shader {}: blob entry {index} past its segment",
                    self.name
                ))
            })?;
        // Unity's program record: version, program type, a 12-byte header, (2016.2+) 4 more, keywords, code
        let mut r = crate::Reader::new(entry, false);
        let version = r.i32()?;
        let _program_type = r.i32()?;
        r.take(12)?;
        if version >= 201_608_170 {
            r.take(4)?;
        }
        let strings = |r: &mut crate::Reader<'_>| -> Result<()> {
            let n =
                usize::try_from(r.i32()?).map_err(|_| invalid("shader blob: a negative keyword count"))?;
            for _ in 0..n {
                let len = usize::try_from(r.i32()?)
                    .map_err(|_| invalid("shader blob: a negative string length"))?;
                r.take(len)?;
                r.align(4)?;
            }
            Ok(())
        };
        strings(&mut r)?;
        if (201_806_140..202_012_090).contains(&version) {
            strings(&mut r)?;
        }
        let len = usize::try_from(r.i32()?).map_err(|_| invalid("shader blob: a negative code length"))?;
        Ok(r.take(len)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn obj(fields: Vec<(&str, Value)>) -> Value {
        Value::Struct(fields.into_iter().map(|(k, v)| (Arc::from(k), v)).collect())
    }

    fn list(items: Vec<Value>) -> Value {
        Value::Array(Array::Values(items))
    }

    fn vector(name: i64, offset: i64, dim: i64) -> Value {
        obj(vec![
            ("m_NameIndex", Value::Int(name)),
            ("m_Index", Value::Int(offset)),
            ("m_ArraySize", Value::Int(0)),
            ("m_Type", Value::Int(0)),
            ("m_Dim", Value::Int(dim)),
        ])
    }

    fn cbuffer(fields: Vec<Value>, structs: Vec<Value>) -> Value {
        obj(vec![
            ("m_NameIndex", Value::Int(0)),
            ("m_VectorParams", list(fields)),
            ("m_MatrixParams", list(Vec::new())),
            ("m_StructParams", list(structs)),
            ("m_Size", Value::Int(32)),
        ])
    }

    fn params(cbs: Vec<Value>) -> Value {
        obj(vec![
            ("m_ConstantBuffers", list(cbs)),
            (
                "m_ConstantBufferBindings",
                list(vec![obj(vec![
                    ("m_NameIndex", Value::Int(0)),
                    ("m_Index", Value::Int(3)),
                ])]),
            ),
        ])
    }

    #[test]
    fn parameters_resolve_names_slots_and_offsets_and_refuse_what_they_cannot() {
        let names: std::collections::BTreeMap<i64, String> = [
            (0, "UnityPerMaterial".to_owned()),
            (1, "_BaseColor".to_owned()),
            (2, "_Smoothness".to_owned()),
        ]
        .into_iter()
        .collect();
        // fields come out by offset, the buffer at the slot its binding names
        let p = parameters(
            &params(vec![cbuffer(vec![vector(2, 16, 1), vector(1, 0, 4)], Vec::new())]),
            &names,
        )
        .unwrap();
        let cb = &p.constant_buffers[0];
        assert_eq!(
            (cb.name.as_str(), cb.slot, cb.size),
            ("UnityPerMaterial", Some(3), 32)
        );
        assert_eq!(
            cb.fields
                .iter()
                .map(|f| (f.name.as_str(), f.offset, f.shape))
                .collect::<Vec<_>>(),
            vec![
                ("_BaseColor", 0, Shape::Vector { columns: 4 }),
                ("_Smoothness", 16, Shape::Vector { columns: 1 })
            ]
        );
        // a name index past the table, a negative offset and struct fields are refused
        assert!(parameters(&params(vec![cbuffer(vec![vector(9, 0, 4)], Vec::new())]), &names).is_err());
        assert!(parameters(&params(vec![cbuffer(vec![vector(1, -4, 4)], Vec::new())]), &names).is_err());
        assert!(matches!(
            parameters(&params(vec![cbuffer(Vec::new(), vec![obj(Vec::new())])]), &names),
            Err(crate::Error::Unsupported(_))
        ));
    }

    #[test]
    fn integer_arrays_read_whatever_their_element_type() {
        assert_eq!(ints(&Value::Array(Array::U16(vec![1, 2]))).unwrap(), vec![1, 2]);
        assert_eq!(
            ints(&list(vec![Value::Int(-1), Value::UInt(5)])).unwrap(),
            vec![-1, 5]
        );
        assert!(ints(&Value::Array(Array::F32(vec![1.0]))).is_err());
        assert!(ints(&Value::Int(1)).is_err());
    }
}
