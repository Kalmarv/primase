//! The reflection (`RDEF`) and signature (`ISGN`, `OSGN`, `ISG1`, `OSG1`, `OSG5`, `PCSG`) parts of a Shader Model 4
//! or 5 container, as the records they store: resource bindings, constant buffers with their variables and type
//! trees, and signature elements, each with D3D's own numbers (`D3D_SHADER_INPUT_TYPE`, `D3D_NAME`, ...) and the word
//! it starts at. What the numbers mean for a graphics API is the caller's to decide.
//!
//! Every table is bounded before it is read, every string is checked to end inside its part and be UTF-8, and a type
//! tree is refused where it nests deeper than 32 or refers to itself.
use crate::decode::ProgramType;
use crate::{Result, chunk, error, parts};

struct View<'a>(&'a [u8]);
impl View<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8]> {
        let end = at
            .checked_add(len)
            .ok_or_else(|| error(at / 4, "reflection range overflow"))?;
        self.0
            .get(at..end)
            .ok_or_else(|| error(at / 4, "reflection range exceeds chunk"))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(at, 4)?.try_into().expect("four bytes"),
        ))
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(at, 2)?.try_into().expect("two bytes"),
        ))
    }
    fn string(&self, at: usize) -> Result<String> {
        let tail = self
            .0
            .get(at..)
            .ok_or_else(|| error(at / 4, "invalid string offset"))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error(at / 4, "unterminated reflection string"))?;
        if end > 4096 {
            return Err(error(at / 4, "reflection string exceeds bound"));
        }
        Ok(std::str::from_utf8(&tail[..end])
            .map_err(|_| error(at / 4, "non-UTF8 reflection string"))?
            .to_owned())
    }
    fn table(&self, at: usize, count: usize, stride: usize) -> Result<std::ops::Range<usize>> {
        if count > 65536 {
            return Err(error(at / 4, "reflection table exceeds count bound"));
        }
        self.bytes(
            at,
            count
                .checked_mul(stride)
                .ok_or_else(|| error(at / 4, "table overflow"))?,
        )?;
        Ok(0..count)
    }
    /// The type record at byte `at` and its members' types, depth first.
    fn ty(&self, at: usize, stack: &mut Vec<usize>, budget: &mut usize) -> Result<RdefType> {
        if stack.len() >= 32 || stack.contains(&at) || *budget == 0 {
            return Err(error(at / 4, "cyclic or excessive reflection types"));
        }
        *budget -= 1;
        self.bytes(at, 36)?;
        stack.push(at);
        let count = self.u16(at + 10)? as usize;
        let offset = self.u32(at + 12)? as usize;
        let mut members = Vec::new();
        for i in self.table(offset, count, 12)? {
            let p = offset + i * 12;
            members.push(RdefMember {
                name: self.string(self.u32(p)? as usize)?,
                offset: self.u32(p + 8)?,
                ty: self.ty(self.u32(p + 4)? as usize, stack, budget)?,
            });
        }
        stack.pop();
        Ok(RdefType {
            word: at / 4,
            class: self.u16(at)?,
            scalar: self.u16(at + 2)?,
            rows: self.u16(at + 4)?,
            columns: self.u16(at + 6)?,
            elements: self.u16(at + 8)?,
            members,
        })
    }
}

/// One element of a signature as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureElement {
    pub name: String,
    pub index: u32,
    /// `D3D_NAME`.
    pub system_value: u32,
    /// `D3D_REGISTER_COMPONENT_TYPE`.
    pub component_type: u32,
    pub register: u32,
    pub mask: u8,
    pub read_write_mask: u8,
    /// Where the record starts, in words of the chunk.
    pub at: usize,
}

/// A signature part's records. Layouts: `ISGN`/`OSGN`/`PCSG` (name, index, system value, component type, register,
/// masks), `ISG1`/`OSG1` (a stream and a minimum precision besides) and `OSG5` (a stream besides). `kind` is the
/// part's tag, which says which layout `bytes` has.
pub fn signature(bytes: &[u8], kind: &[u8; 4]) -> Result<Vec<SignatureElement>> {
    let v = View(bytes);
    let count = v.u32(0)? as usize;
    if v.u32(4)? != 8 {
        return Err(error(1, "unsupported signature directory offset"));
    }
    // bytes before the name offset, and the record size
    let (skip, record) = match kind {
        b"ISG1" | b"OSG1" => (4, 32),
        b"OSG5" => (4, 28),
        _ => (0, 24),
    };
    let mut result = Vec::new();
    for i in v.table(8, count, record)? {
        let p = 8 + i * record;
        let q = p + skip;
        let lanes = v.bytes(q + 20, 4)?;
        result.push(SignatureElement {
            name: v.string(v.u32(q)? as usize)?,
            index: v.u32(q + 4)?,
            system_value: v.u32(q + 8)?,
            component_type: v.u32(q + 12)?,
            register: v.u32(q + 16)?,
            mask: lanes[0],
            read_write_mask: lanes[1],
            at: p / 4,
        });
    }
    Ok(result)
}

/// A hull or domain program's patch-constant signature elements as stored (the `PCSG` part).
pub fn patch_signature_elements(bytes: &[u8]) -> Result<Vec<SignatureElement>> {
    let directory = parts(bytes)?;
    signature(chunk(&directory, b"PCSG")?, b"PCSG")
}

/// The part among `tags` the container has (the first tag where it has none, so that is the one reported missing).
fn signature_part<'a>(
    directory: &[dxcontainer::Chunk<'a>],
    tags: &[&[u8; 4]],
) -> Result<(&'a [u8], [u8; 4])> {
    let tag = tags
        .iter()
        .find(|t| directory.iter().any(|c| &&c.kind == *t))
        .unwrap_or(&tags[0]);
    Ok((chunk(directory, tag)?, **tag))
}

/// A container's program type and its input and output signature elements as stored.
pub fn signature_elements(
    bytes: &[u8],
) -> Result<(ProgramType, Vec<SignatureElement>, Vec<SignatureElement>)> {
    let directory = parts(bytes)?;
    let kind = program_type(&directory)?;
    let read = |tags: &[&[u8; 4]]| -> Result<Vec<SignatureElement>> {
        let (data, tag) = signature_part(&directory, tags)?;
        signature(data, &tag)
    };
    Ok((
        kind,
        read(&[b"ISGN", b"ISG1"])?,
        read(&[b"OSGN", b"OSG1", b"OSG5"])?,
    ))
}

/// The type of a container's program: its code part's version token names it in the high 16 bits.
pub fn program_type(directory: &[dxcontainer::Chunk<'_>]) -> Result<ProgramType> {
    let code = directory
        .iter()
        .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
        .ok_or_else(|| error(0, "missing shader code chunk"))?;
    let token = View(code.data).u32(0)?;
    match token >> 16 {
        0 => Ok(ProgramType::Pixel),
        1 => Ok(ProgramType::Vertex),
        2 => Ok(ProgramType::Geometry),
        3 => Ok(ProgramType::Hull),
        4 => Ok(ProgramType::Domain),
        5 => Ok(ProgramType::Compute),
        kind => Err(error(0, format!("program type {kind} (unknown) is not mapped"))),
    }
}

/// A container's `RDEF` part as stored: what it says the program binds and the layouts of its buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rdef {
    /// The part's own version token (`0xffff0500`: an SM5.0 pixel program, `0xfffe0500`: vertex, ...).
    pub target: u32,
    pub bindings: Vec<RdefBinding>,
    pub buffers: Vec<RdefBuffer>,
}

/// One resource binding record as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdefBinding {
    /// Where the record starts, in words of the part.
    pub word: usize,
    pub name: String,
    /// `D3D_SHADER_INPUT_TYPE`.
    pub kind: u32,
    /// `D3D_RESOURCE_RETURN_TYPE`.
    pub returns: u32,
    /// `D3D_SRV_DIMENSION`.
    pub dimension: u32,
    /// A multisampled texture's sample count; a structured buffer's stride.
    pub samples: u32,
    pub register: u32,
    pub count: u32,
    /// `D3D_SHADER_INPUT_FLAGS`.
    pub flags: u32,
}

/// One buffer (`D3D11_SHADER_BUFFER_DESC`) as stored, with its variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdefBuffer {
    pub word: usize,
    pub name: String,
    /// `D3D_CBUFFER_TYPE`: 0 a cbuffer, 1 a tbuffer, 2 interface pointers, 3 a structured buffer's element.
    pub kind: u32,
    /// In bytes.
    pub size: u32,
    pub variables: Vec<RdefVariable>,
}

/// One variable of a buffer as stored: its byte range in the buffer, its type and its default value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdefVariable {
    pub word: usize,
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub ty: RdefType,
    /// The stored default value's `size` bytes, where the variable has one.
    pub default: Option<Vec<u8>>,
}

/// A variable's type record (`D3D11_SHADER_TYPE_DESC`) as stored, and its members' types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdefType {
    pub word: usize,
    /// `D3D_SHADER_VARIABLE_CLASS`: 0 scalar, 1 vector, 2 row-major matrix, 3 column-major matrix, 4 object, 5 struct.
    pub class: u16,
    /// `D3D_SHADER_VARIABLE_TYPE`.
    pub scalar: u16,
    pub rows: u16,
    pub columns: u16,
    /// The array length; 0 where it isn't an array.
    pub elements: u16,
    pub members: Vec<RdefMember>,
}

/// A struct member: its name, its byte offset in the struct and its type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdefMember {
    pub name: String,
    pub offset: u32,
    pub ty: RdefType,
}

/// The version token of a container's first `RDEF` part (`None` where it has none), read without the rest of the part:
/// a caller can tell which program model's layout it holds before it asks [`rdef`] for the records.
pub fn rdef_target(bytes: &[u8]) -> Result<Option<u32>> {
    let directory = parts(bytes)?;
    directory
        .iter()
        .find(|c| &c.kind == b"RDEF")
        .map(|rdef| View(rdef.data).u32(16))
        .transpose()
}

/// A container's `RDEF` part (`None` where it has none), for the SM5 layout (the `RD11` record-size directory).
pub fn rdef(bytes: &[u8]) -> Result<Option<Rdef>> {
    let directory = parts(bytes)?;
    if !directory.iter().any(|c| &c.kind == b"RDEF") {
        return Ok(None);
    }
    let v = View(chunk(&directory, b"RDEF")?);
    let target = v.u32(16)?;
    if v.bytes(28, 4)? != b"RD11" {
        return Err(error(4, "expected SM5.0 pixel or vertex RDEF"));
    }
    for (i, size) in [60, 24, 32, 40, 36, 12].into_iter().enumerate() {
        if v.u32(32 + i * 4)? != size {
            return Err(error(8 + i, "unsupported RDEF record size"));
        }
    }
    let mut bindings = Vec::new();
    let offset = v.u32(12)? as usize;
    for i in v.table(offset, v.u32(8)? as usize, 32)? {
        let p = offset + i * 32;
        bindings.push(RdefBinding {
            word: p / 4,
            name: v.string(v.u32(p)? as usize)?,
            kind: v.u32(p + 4)?,
            returns: v.u32(p + 8)?,
            dimension: v.u32(p + 12)?,
            samples: v.u32(p + 16)?,
            register: v.u32(p + 20)?,
            count: v.u32(p + 24)?,
            flags: v.u32(p + 28)?,
        });
    }
    let mut buffers = Vec::new();
    let offset = v.u32(4)? as usize;
    let mut budget = 65536;
    for i in v.table(offset, v.u32(0)? as usize, 24)? {
        let p = offset + i * 24;
        let name = v.string(v.u32(p)? as usize)?;
        let kind = v.u32(p + 20)?;
        let size = v.u32(p + 12)?;
        if size > 65536 {
            return Err(error(p / 4, "constant-buffer size exceeds bound"));
        }
        let mut variables = Vec::new();
        let entries = v.u32(p + 8)? as usize;
        for j in v.table(entries, v.u32(p + 4)? as usize, 40)? {
            let q = entries + j * 40;
            let start = v.u32(q + 4)?;
            let len = v.u32(q + 8)?;
            if start.checked_add(len).is_none_or(|end| end > size) {
                return Err(error(q / 4, "variable exceeds constant buffer"));
            }
            let default = v.u32(q + 20)? as usize;
            let ty = v.ty(v.u32(q + 16)? as usize, &mut Vec::new(), &mut budget)?;
            variables.push(RdefVariable {
                word: q / 4,
                name: v.string(v.u32(q)? as usize)?,
                offset: start,
                size: len,
                ty,
                default: if default == 0 {
                    None
                } else {
                    Some(v.bytes(default, len as usize)?.to_vec())
                },
            });
        }
        buffers.push(RdefBuffer {
            word: p / 4,
            name,
            kind,
            size,
            variables,
        });
    }
    Ok(Some(Rdef {
        target,
        bindings,
        buffers,
    }))
}
