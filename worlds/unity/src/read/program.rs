//! A Shader's compiled programs, as a player stores them: per pass, each stage's program (vertex, fragment, geometry,
//! hull, domain: the stages the pass has) with its variants (each a keyword
//! set, a GPU program type and an entry of the compressed blob) and their parameters (constant buffers with each
//! field's name and byte offset, texture and sampler bind slots), read from `m_ParsedForm`; and a variant's code from
//! the platform's LZ4-compressed blob. What the bindings mean for a material is the caller's.
//!
//! Players store the variants in one of two layouts, told apart by their fields: from 2021.2, `m_PlayerSubPrograms`
//! (lists of variants, keywords indexing the form's `m_KeywordNames`) with `m_CommonParameters` shared by them;
//! before it, `m_SubPrograms` (one list, each variant with its hardware tier, its keywords as global and local indices
//! into the pass's name table, and its own parameters). Struct fields (instancing's per-draw arrays) are read as
//! [`StructField`]s.
//!
//! The blob of one platform is split into segments (`offsets`, `compressedLengths`, `decompressedLengths`, one
//! entry per segment). Segment 0 starts with the entry table: a count, then per entry its offset, length and segment.
//! An entry of a program holds Unity's program record: its version, GPU program type, then (from 2016.2) a header the
//! reader skips, keyword strings (empty in current players: the parsed form names them), and the code as a byte array.

use primase_compress::{Decompressor, Lz4};

use crate::components::{ConstantBuffer, Field, Parameters, Shape, StructField, TextureParam};
use crate::read::player::{ObjRef, Player};
use crate::read::value::{Array, Value};
use crate::{Result, invalid};

/// One compiled variant: the blob entry holding its code, its keywords, its GPU program type (Unity's
/// `ShaderGpuProgramType`: 17 a D3D11 pixel program before 2021.2, 18 an SM 5.0 one after) and its shader
/// requirements mask; `group` is the index of the player sub-program list it is in (from 2021.2) or its hardware tier
/// (before); `parameters` its own, where the player stores them per variant (before 2021.2), else `None`: the pass's.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Variant {
    pub group: usize,
    pub blob_index: u32,
    pub keywords: Vec<String>,
    pub gpu_program_type: i64,
    pub requirements: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Parameters>,
}

/// One pass's program of one stage: its variants and the parameters common to them (empty where each variant has its
/// own).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PassProgram {
    pub variants: Vec<Variant>,
    pub parameters: Parameters,
}

impl PassProgram {
    /// The parameters variant `v` (of this pass) is compiled with.
    pub fn parameters_of<'a>(&'a self, v: &'a Variant) -> &'a Parameters {
        v.parameters.as_ref().unwrap_or(&self.parameters)
    }
}

/// The stages Unity stores a pass's programs for (`progVertex`, `progFragment`, `progGeometry`, `progHull`,
/// `progDomain`), in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum Stage {
    Vertex,
    Fragment,
    Geometry,
    Hull,
    Domain,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Vertex,
        Stage::Fragment,
        Stage::Geometry,
        Stage::Hull,
        Stage::Domain,
    ];

    /// The pass field Unity stores the stage's program under.
    fn field(self) -> &'static str {
        match self {
            Stage::Vertex => "progVertex",
            Stage::Fragment => "progFragment",
            Stage::Geometry => "progGeometry",
            Stage::Hull => "progHull",
            Stage::Domain => "progDomain",
        }
    }
}

/// One pass's programs, a [`PassProgram`] per stage (empty where the pass stores none for it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassStages {
    pub vertex: PassProgram,
    pub fragment: PassProgram,
    pub geometry: PassProgram,
    pub hull: PassProgram,
    pub domain: PassProgram,
}

impl PassStages {
    pub fn stage(&self, stage: Stage) -> &PassProgram {
        match stage {
            Stage::Vertex => &self.vertex,
            Stage::Fragment => &self.fragment,
            Stage::Geometry => &self.geometry,
            Stage::Hull => &self.hull,
            Stage::Domain => &self.domain,
        }
    }
}

/// A Shader's compiled programs: `passes[subshader][pass]`, and its per-platform blobs.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderPrograms {
    pub name: String,
    pub passes: Vec<Vec<PassStages>>,
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

/// The name an entry's `m_NameIndex` gives in the pass's name table.
fn name_of(e: &Value, names: &std::collections::BTreeMap<i64, String>) -> Result<String> {
    let i = int(e, "m_NameIndex")?;
    names
        .get(&i)
        .cloned()
        .ok_or_else(|| invalid(format!("shader program: name index {i} past the pass's names")))
}

/// The vector (`vectors`) and matrix (`matrices`) fields of a constant buffer or struct, by offset.
fn fields(
    v: &Value,
    vectors: &str,
    matrices: &str,
    names: &std::collections::BTreeMap<i64, String>,
) -> Result<Vec<Field>> {
    let mut fields = Vec::new();
    for (key, matrix) in [(vectors, false), (matrices, true)] {
        for f in items(v, key)? {
            fields.push(Field {
                name: name_of(f, names)?,
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
    fields.sort_by_key(|f| f.offset);
    Ok(fields)
}

/// The parameters as stored, their names resolved through the pass's name table.
fn parameters(v: &Value, names: &std::collections::BTreeMap<i64, String>) -> Result<Parameters> {
    let name = |e: &Value| name_of(e, names);
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
        let mut structs = items(cb, "m_StructParams")?
            .iter()
            .map(|s| {
                Ok(StructField {
                    name: name(s)?,
                    offset: u32_of(int(s, "m_Index")?, "struct offset")?,
                    array_size: u32_of(int(s, "m_ArraySize")?, "array size")?,
                    size: u32_of(int(s, "m_StructSize")?, "struct size")?,
                    members: fields(s, "m_VectorMembers", "m_MatrixMembers", names)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        structs.sort_by_key(|s| s.offset);
        constant_buffers.push(ConstantBuffer {
            name: name(cb)?,
            slot: bindings.get(&int(cb, "m_NameIndex")?).copied(),
            size: u32_of(int(cb, "m_Size")?, "buffer size")?,
            fields: fields(cb, "m_VectorParams", "m_MatrixParams", names)?,
            structs,
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

/// One stage's program of a pass as stored (`program` is its `prog*` value): its variants and common parameters, names
/// resolved through the pass's name table `names` and the form's `keyword_names`; `shader` names the shader in errors.
fn stage_program(
    program: &Value,
    names: &std::collections::BTreeMap<i64, String>,
    keyword_names: &[String],
    name: &str,
) -> Result<PassProgram> {
    let mut variants = Vec::new();
    // before 2021.2: one list, each variant with its tier, keywords by name index and own parameters
    for v in items(program, "m_SubPrograms")? {
        let mut keywords = Vec::new();
        for k in ["m_GlobalKeywordIndices", "m_LocalKeywordIndices"] {
            if let Some(k) = v.get(k) {
                for i in ints(k)? {
                    keywords.push(names.get(&i).cloned().ok_or_else(|| {
                        invalid(format!("shader {name}: keyword index {i} past the pass's names"))
                    })?);
                }
            }
        }
        variants.push(Variant {
            group: usize::try_from(int(v, "m_ShaderHardwareTier")?)
                .map_err(|_| invalid("shader program: a negative hardware tier"))?,
            blob_index: u32_of(int(v, "m_BlobIndex")?, "blob index")?,
            keywords,
            gpu_program_type: int(v, "m_GpuProgramType")?,
            requirements: v.get("m_ShaderRequirements").and_then(Value::as_i64).unwrap_or(0),
            parameters: Some(parameters(v, names)?),
        });
    }
    for (group, list) in items(program, "m_PlayerSubPrograms")?.iter().enumerate() {
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
                parameters: None,
            });
        }
    }
    let parameters = match program.get("m_CommonParameters") {
        Some(c) => parameters(c, names)?,
        None => Parameters::default(),
    };
    Ok(PassProgram { variants, parameters })
}

/// Blob segments of a [`ShaderPrograms`] already decompressed ([`ShaderPrograms::code_with`]).
#[derive(Debug, Default)]
pub struct Segments(std::collections::BTreeMap<(usize, usize), std::sync::Arc<Vec<u8>>>);

impl Segments {
    fn get(
        &mut self,
        programs: &ShaderPrograms,
        platform: usize,
        k: usize,
    ) -> Result<std::sync::Arc<Vec<u8>>> {
        if let Some(s) = self.0.get(&(platform, k)) {
            return Ok(s.clone());
        }
        let s = std::sync::Arc::new(programs.segment(platform, k)?);
        self.0.insert((platform, k), s.clone());
        Ok(s)
    }
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
                let stage = |stage: Stage| match p.get(stage.field()) {
                    Some(program) => stage_program(program, &names, &keyword_names, &name),
                    None => Ok(PassProgram::default()),
                };
                out.push(PassStages {
                    vertex: stage(Stage::Vertex)?,
                    fragment: stage(Stage::Fragment)?,
                    geometry: stage(Stage::Geometry)?,
                    hull: stage(Stage::Hull)?,
                    domain: stage(Stage::Domain)?,
                });
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
        self.code_with(&mut Segments::default(), platform, index)
    }

    /// [`ShaderPrograms::code`] keeping the segments it decompresses in `cache`, which reads many entries of one
    /// shader without decompressing a segment for each.
    pub fn code_with(&self, cache: &mut Segments, platform: i64, index: u32) -> Result<Vec<u8>> {
        let p = self
            .platforms
            .iter()
            .position(|&x| x == platform)
            .ok_or_else(|| invalid(format!("shader {}: no blob for platform {platform}", self.name)))?;
        let table = cache.get(self, p, 0)?;
        let mut r = super::Reader::new(&table, false);
        let count = usize::try_from(r.i32()?).map_err(|_| invalid("shader blob: a negative entry count"))?;
        if index as usize >= count {
            return Err(invalid(format!(
                "shader {}: blob entry {index} of {count}",
                self.name
            )));
        }
        // the table's count is its own word: the entry has to be in the bytes there are
        let at = usize::try_from(index)
            .ok()
            .and_then(|i| i.checked_mul(12))
            .and_then(|o| o.checked_add(4))
            .filter(|at| at.saturating_add(12) <= table.len())
            .ok_or_else(|| {
                invalid(format!(
                    "shader {}: blob entry {index} of {count} in a table of {} bytes",
                    self.name,
                    table.len()
                ))
            })?;
        r.pos = at;
        let (offset, length, segment) = (r.i32()?, r.i32()?, r.i32()?);
        let span = |n: i32| usize::try_from(n).map_err(|_| invalid("shader blob: a negative entry span"));
        let (offset, length, segment) = (span(offset)?, span(length)?, span(segment)?);
        let data = if segment == 0 {
            table
        } else {
            cache.get(self, p, segment)?
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
        let mut r = super::Reader::new(entry, false);
        let version = r.i32()?;
        let _program_type = r.i32()?;
        r.take(12)?;
        if version >= 201_608_170 {
            r.take(4)?;
        }
        let strings = |r: &mut super::Reader<'_>| -> Result<()> {
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

/// Unity's `ShaderGpuProgramType` of the D3D11 programs: vertex SM 4.0 and 5.0 (15, 16), pixel (17, 18), geometry (19,
/// 20), hull (21), domain (22).
pub const D3D11_PROGRAM_TYPES: std::ops::RangeInclusive<i64> = 15..=22;

/// The DXBC container in a D3D11 program record's code: Unity stores a short header before it. `None` when the code
/// holds no container (the first `DXBC` within its first 64 bytes, whole as its own header sizes it).
pub fn dxbc(code: &[u8]) -> Option<&[u8]> {
    (0..code.len().min(64)).find_map(|at| {
        let c = code.get(at..)?;
        if c.get(..4)? != b"DXBC" {
            return None;
        }
        let size = u32::from_le_bytes(c.get(24..28)?.try_into().ok()?) as usize;
        c.get(..size)
    })
}

/// The default each texture property of shader `r` declares (`m_ParsedForm.m_PropInfo.m_Props`, type 4:
/// `m_DefTexture.m_DefaultName`), by property.
fn texture_defaults(player: &Player, r: &ObjRef) -> Result<Vec<(String, String)>> {
    let s = player.decode(r)?;
    let props = s
        .get("m_ParsedForm")
        .and_then(|f| f.get("m_PropInfo"))
        .map(|p| items(p, "m_Props"))
        .transpose()?
        .unwrap_or(&[]);
    let mut out = Vec::new();
    for p in props {
        if p.get("m_Type").and_then(Value::as_i64) != Some(4) {
            continue;
        }
        let name = p.get("m_Name").and_then(Value::as_str);
        let default = p
            .get("m_DefTexture")
            .and_then(|d| d.get("m_DefaultName"))
            .and_then(Value::as_str);
        if let (Some(name), Some(default)) = (name, default) {
            out.push((name.to_owned(), default.to_owned()));
        }
    }
    Ok(out)
}

/// What [`read_programs`] attached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramsReport {
    /// Materials given their program.
    pub attached: usize,
    /// Materials of the shaders without one, by reason.
    pub left_out: std::collections::BTreeMap<String, usize>,
}

/// For every `unity.material` of `w` whose shader is one of `shaders`, the programs of its draw pass under
/// `pipeline` ([`UnityMaterial::draw_pass`]) as stored, each stage's (vertex, fragment, geometry, hull, domain): every
/// variant with its D3D11 code, and the shader's declared texture defaults, as `unity.programs` on the material's
/// entity (choosing a variant is the caller's, [`UnityPrograms::variant`]). A material without a shader object, draw
/// pass or D3D11 code is left out, counted by reason; so are variants whose code holds no DXBC container.
pub fn read_programs(
    player: &Player,
    w: &mut primase_core::World,
    shaders: &[&str],
    pipeline: &crate::components::RenderPipeline,
) -> Result<ProgramsReport> {
    use crate::components::{UnityMaterial, UnityProgramVariant, UnityPrograms};
    let mut report = ProgramsReport::default();
    let mut read: std::collections::BTreeMap<(String, i64), (ShaderPrograms, Segments)> =
        std::collections::BTreeMap::new();
    // each (shader, subshader, pass)'s variants of every stage and the shader's texture defaults, made once
    type Built = (
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<UnityProgramVariant>,
        Vec<(String, String)>,
    );
    let mut built: std::collections::BTreeMap<((String, i64), usize, usize), Built> =
        std::collections::BTreeMap::new();
    let materials: Vec<(primase_core::Entity, UnityMaterial)> = w
        .iter::<UnityMaterial>()
        .filter(|(_, m)| m.shader.as_deref().is_some_and(|s| shaders.contains(&s)))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    for (e, m) in materials {
        let mut leave = |why: &str| *report.left_out.entry(why.to_owned()).or_default() += 1;
        let data = crate::material::UnityMaterialData::read(
            player,
            &ObjRef {
                file: m.material.file.clone(),
                path_id: m.material.path_id,
            },
        )?;
        let Some(shader) = data.shader else {
            leave("no shader object");
            continue;
        };
        // the draw pass's place in the shader's parsed form, which `shader_state` mirrors
        let place = m.subshader(pipeline).and_then(|sub| {
            let i = m.shader_state.iter().position(|s| std::ptr::eq(s, sub))?;
            let pass = m.draw_pass(pipeline)?;
            let j = sub.passes.iter().position(|p| std::ptr::eq(p, pass))?;
            Some((i, j))
        });
        let Some((i, j)) = place else {
            leave("no draw pass");
            continue;
        };
        let key = (shader.file.clone(), shader.path_id);
        if !read.contains_key(&key) {
            read.insert(
                key.clone(),
                (ShaderPrograms::read(player, &shader)?, Segments::default()),
            );
        }
        let (programs, segments) = read.get_mut(&key).expect("read above");
        let programs = &*programs;
        let Some(pass) = programs.passes.get(i).and_then(|sub| sub.get(j)) else {
            leave("no draw pass");
            continue;
        };
        if !programs.platforms.contains(&4) {
            leave("no D3D11 code");
            continue;
        }
        let built_key = (key.clone(), i, j);
        if !built.contains_key(&built_key) {
            let mut stored = |stage: Stage, program: &PassProgram| -> Result<Vec<UnityProgramVariant>> {
                let mut variants = Vec::new();
                for v in &program.variants {
                    // a pass stores the variants of every graphics API its shader was compiled for; this blob is D3D11's
                    if !D3D11_PROGRAM_TYPES.contains(&v.gpu_program_type) {
                        *report
                            .left_out
                            .entry("variants of another graphics API than D3D11".to_owned())
                            .or_default() += 1;
                        continue;
                    }
                    let code = programs.code_with(segments, 4, v.blob_index)?;
                    let Some(code) = dxbc(&code) else {
                        *report
                            .left_out
                            .entry(match stage {
                                Stage::Fragment => {
                                    "variants whose D3D11 code holds no DXBC container".to_owned()
                                }
                                _ => format!(
                                    "{} variants whose D3D11 code holds no DXBC container",
                                    format!("{stage:?}").to_lowercase()
                                ),
                            })
                            .or_default() += 1;
                        continue;
                    };
                    variants.push(UnityProgramVariant {
                        tier: v.group,
                        gpu_program_type: v.gpu_program_type,
                        keywords: v.keywords.clone(),
                        parameters: program.parameters_of(v).clone(),
                        code: std::sync::Arc::from(code),
                    });
                }
                Ok(variants)
            };
            let variants = stored(Stage::Fragment, &pass.fragment)?;
            let vertex = stored(Stage::Vertex, &pass.vertex)?;
            let geometry = stored(Stage::Geometry, &pass.geometry)?;
            let hull = stored(Stage::Hull, &pass.hull)?;
            let domain = stored(Stage::Domain, &pass.domain)?;
            let texture_defaults = texture_defaults(player, &shader)?;
            built.insert(
                built_key.clone(),
                (variants, vertex, geometry, hull, domain, texture_defaults),
            );
        }
        let (variants, vertex, geometry, hull, domain, texture_defaults) = built[&built_key].clone();
        w.insert(
            e,
            UnityPrograms {
                shader: programs.name.clone(),
                subshader: i,
                pass: j,
                texture_defaults,
                variants,
                vertex,
                geometry,
                hull,
                domain,
            },
        );
        report.attached += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blob table that states more entries than it holds: the entry is refused, not indexed past the table.
    #[test]
    fn a_blob_entry_past_the_table_is_refused() {
        // one LZ4 literal run of the four bytes of a count of 1000
        let table = vec![0x40u8, 0xE8, 0x03, 0, 0];
        let programs = ShaderPrograms {
            name: "s".into(),
            passes: vec![],
            platforms: vec![4],
            segments: vec![vec![(0, table.len(), 4)]],
            blob: table,
        };
        let e = programs.code(4, 999).unwrap_err().to_string();
        assert!(e.contains("blob entry 999 of 1000 in a table of 4 bytes"), "{e}");
    }
    use std::sync::Arc;

    #[test]
    fn the_dxbc_container_is_found_past_unitys_header() {
        let mut container = b"DXBC".to_vec();
        container.extend([0; 20]);
        container.extend(32u32.to_le_bytes());
        container.extend([0; 4]);
        let mut code = vec![7, 0, 0, 0, 1, 2];
        code.extend(&container);
        code.extend([9, 9]);
        assert_eq!(dxbc(&code), Some(&container[..]));
        assert_eq!(dxbc(&code[..20]), None);
    }

    /// The caller states the renderer's keywords and tier; the material adds the keywords the pass's variants use
    /// (a stale one no variant has is not compiled in); the keywords must match exactly, in the stated tier.
    #[test]
    fn a_variant_is_the_one_the_caller_states() {
        use crate::components::{UnityProgramVariant, UnityPrograms};
        let variant = |tier: usize, keywords: &[&str]| UnityProgramVariant {
            tier,
            gpu_program_type: 18,
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            parameters: Parameters::default(),
            code: std::sync::Arc::from(vec![tier as u8, keywords.len() as u8]),
        };
        let programs = UnityPrograms {
            shader: "Test".into(),
            subshader: 0,
            pass: 0,
            texture_defaults: Vec::new(),
            vertex: Vec::new(),
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
            variants: vec![
                variant(1, &["DIRECTIONAL"]),
                variant(0, &["DIRECTIONAL"]),
                variant(0, &["DIRECTIONAL", "SHADOWS_SCREEN"]),
                variant(0, &["_NORMALMAP", "DIRECTIONAL"]),
            ],
        };
        let s = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let chosen = |material: &[&str], global: &[&str], tier| {
            programs
                .variant(&s(material), &s(global), tier)
                .map(|v| v.code.to_vec())
        };
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 0), Some(vec![0, 1]));
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 1), Some(vec![1, 1]));
        assert_eq!(
            chosen(&["_NORMALMAP", "STALE"], &["DIRECTIONAL"], 0),
            Some(vec![0, 2])
        );
        assert_eq!(
            chosen(&[], &["DIRECTIONAL", "SHADOWS_SCREEN"], 0),
            Some(vec![0, 2])
        );
        assert_eq!(chosen(&[], &["DIRECTIONAL", "LIGHTMAP_ON"], 0), None);
        assert_eq!(chosen(&[], &["DIRECTIONAL"], 2), None);
    }

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
    fn parameters_resolve_names_slots_offsets_and_structs_and_refuse_what_they_cannot() {
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
        // a struct field: an array of members (instancing's per-draw arrays)
        let instanced = obj(vec![
            ("m_NameIndex", Value::Int(1)),
            ("m_Index", Value::Int(0)),
            ("m_ArraySize", Value::Int(2)),
            ("m_StructSize", Value::Int(32)),
            ("m_VectorMembers", list(vec![vector(2, 16, 4)])),
            ("m_MatrixMembers", list(Vec::new())),
        ]);
        let p = parameters(&params(vec![cbuffer(Vec::new(), vec![instanced])]), &names).unwrap();
        let s = &p.constant_buffers[0].structs[0];
        assert_eq!(
            (s.name.as_str(), s.offset, s.array_size, s.size),
            ("_BaseColor", 0, 2, 32)
        );
        assert_eq!(
            s.members
                .iter()
                .map(|f| (f.name.as_str(), f.offset))
                .collect::<Vec<_>>(),
            vec![("_Smoothness", 16)]
        );
        // a name index past the table and a negative offset are refused
        assert!(parameters(&params(vec![cbuffer(vec![vector(9, 0, 4)], Vec::new())]), &names).is_err());
        assert!(parameters(&params(vec![cbuffer(vec![vector(1, -4, 4)], Vec::new())]), &names).is_err());
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
