//! Big-endian revision-6 CgBinaryProgram containers, with offsets relative to
//! their own header. See the crate README for the layout sources and scope.
use primase_bytes::{Endian::Big, View};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("CgBinaryProgram at byte {offset}: {detail}")]
pub struct Error {
    pub offset: usize,
    pub detail: String,
}

fn error(offset: usize, detail: impl Into<String>) -> Error {
    Error {
        offset,
        detail: detail.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub instruction_count: u32,
    pub instruction_slot: u32,
    pub register_count: u32,
    pub attribute_input_mask: u32,
    pub attribute_output_mask: u32,
    pub user_clip_mask: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    /// Number of 16-byte microcode slots, including embedded constants.
    pub instruction_count: u32,
    pub attribute_input_mask: u32,
    /// Two bits per texture unit, kept as encoded.
    pub partial_tex_type: u32,
    pub tex_coords_input_mask: u16,
    pub tex_coords_2d: u16,
    pub tex_coords_centroid: u16,
    pub register_count: u8,
    pub output_from_h0: bool,
    pub depth_replace: bool,
    pub pixel_kill: bool,
    pub padding: [u8; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex(Vertex),
    Fragment(Fragment),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter<'a> {
    /// Original CGtype, CGresource and CGenum values, including unknown values.
    pub ty: u32,
    pub resource: u32,
    pub variability: u32,
    pub resource_index: i32,
    /// NUL-terminated source strings, excluding their terminator; no encoding conversion.
    pub name: Option<&'a [u8]>,
    pub semantic: Option<&'a [u8]>,
    pub direction: u32,
    /// -1 denotes a global parameter.
    pub parameter_number: i32,
    pub referenced: bool,
    pub shared: bool,
    /// Big-endian float bits, preserving NaNs and signed zeros. Float scalar,
    /// vector and matrix types are supported; other types with defaults refuse.
    pub default_value: Option<Vec<u32>>,
    /// Byte offsets of 16-byte constants in this program's microcode.
    pub embedded_constants: Vec<u32>,
}

impl Parameter<'_> {
    /// CG_TEXUNIT0 through CG_TEXUNIT15. A resource index alone is not a texunit.
    pub fn texture_unit(&self) -> Option<u32> {
        self.resource.checked_sub(2048).filter(|&n| n < 16)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program<'a> {
    pub profile: u32,
    pub revision: u32,
    pub stage: Stage,
    pub parameters: Vec<Parameter<'a>>,
    /// Original byte order and inline constants retained verbatim.
    pub microcode: &'a [u8],
    /// Exactly the advertised container size, excluding any caller's padding.
    pub bytes: &'a [u8],
}

struct Reader<'a> {
    bytes: &'a [u8],
    fixed: Vec<Range<usize>>,
}

impl<'a> Reader<'a> {
    fn bytes(&self, at: usize, n: usize) -> Result<&'a [u8], Error> {
        View::new(self.bytes)
            .slice(at, n)
            .map_err(|e| error(at, e.to_string()))
    }
    fn word(&self, at: usize) -> Result<u32, Error> {
        View::new(self.bytes)
            .u32(at, Big)
            .map_err(|e| error(at, e.to_string()))
    }
    fn offset(&self, at: usize) -> Result<usize, Error> {
        usize::try_from(self.word(at)?).map_err(|_| error(at, "offset outside address space"))
    }
    fn boolean(&self, at: usize, byte: bool) -> Result<bool, Error> {
        let n = if byte {
            u32::from(self.bytes(at, 1)?[0])
        } else {
            self.word(at)?
        };
        match n {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(error(at, "boolean is neither zero nor one")),
        }
    }
    fn range(&self, at: usize, count: usize, stride: usize) -> Result<Range<usize>, Error> {
        let n = count
            .checked_mul(stride)
            .ok_or_else(|| error(at, "array size overflow"))?;
        self.bytes(at, n)?;
        Ok(at..at.checked_add(n).ok_or_else(|| error(at, "range overflow"))?)
    }
    fn reserve(&mut self, r: Range<usize>) -> Result<(), Error> {
        if !r.start.is_multiple_of(4) {
            return Err(error(r.start, "unaligned structure"));
        }
        self.metadata(r.clone())?;
        if !r.is_empty() {
            self.fixed.push(r);
        }
        Ok(())
    }
    fn metadata(&self, r: Range<usize>) -> Result<(), Error> {
        if !r.is_empty() && self.fixed.iter().any(|f| r.start < f.end && f.start < r.end) {
            return Err(error(
                r.start,
                "overlapping structures or metadata inside a fixed structure",
            ));
        }
        Ok(())
    }
    fn string(&self, field: usize) -> Result<Option<&'a [u8]>, Error> {
        let at = self.offset(field)?;
        if at == 0 {
            return Ok(None);
        }
        let tail = self
            .bytes
            .get(at..)
            .ok_or_else(|| error(field, "string offset outside container"))?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| error(at, "unterminated string"))?;
        let r = self.range(at, len + 1, 1)?;
        self.metadata(r)?;
        Ok(Some(self.bytes(at, len)?))
    }
    fn floats(&self, field: usize, ty: u32) -> Result<Option<Vec<u32>>, Error> {
        let at = self.offset(field)?;
        if at == 0 {
            return Ok(None);
        }
        let count = match ty {
            1045 | 1091 => 1,
            1046..=1048 => (ty - 1044) as usize,
            1049..=1064 => {
                let m = ty - 1049;
                ((m / 4 + 1) * (m % 4 + 1)) as usize
            }
            _ => {
                return Err(error(
                    field,
                    format!("default values for CGtype {ty} are unsupported"),
                ));
            }
        };
        if !at.is_multiple_of(4) {
            return Err(error(field, "unaligned default value"));
        }
        let r = self.range(at, count, 4)?;
        self.metadata(r)?;
        (0..count)
            .map(|i| self.word(at + i * 4))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
    fn embedded(&self, field: usize, ucode_size: usize) -> Result<Vec<u32>, Error> {
        let at = self.offset(field)?;
        if at == 0 {
            return Ok(Vec::new());
        }
        if !at.is_multiple_of(4) {
            return Err(error(field, "unaligned embedded constant table"));
        }
        let count = self.offset(at)?;
        let r = self.range(
            at,
            count
                .checked_add(1)
                .ok_or_else(|| error(at, "constant count overflow"))?,
            4,
        )?;
        self.metadata(r)?;
        let mut offsets = Vec::new();
        for i in 0..count {
            let field = at + 4 + i * 4;
            let offset = self.word(field)?;
            let n = usize::try_from(offset).map_err(|_| error(field, "constant outside address space"))?;
            if !n.is_multiple_of(16) || n.checked_add(16).is_none_or(|end| end > ucode_size) {
                return Err(error(
                    field,
                    "embedded constant is unaligned or outside microcode",
                ));
            }
            offsets.push(offset);
        }
        Ok(offsets)
    }
}

/// Read one container with no trailing bytes. Padding and concatenation belong
/// to the calling file format; use [`parse_prefix`] for those.
pub fn parse(bytes: &[u8]) -> Result<Program<'_>, Error> {
    let p = parse_prefix(bytes)?;
    if p.bytes.len() != bytes.len() {
        return Err(error(p.bytes.len(), "trailing bytes after container"));
    }
    Ok(p)
}

/// Read the advertised first container, keeping every offset relative to it.
/// Refuses unsupported profiles/revisions, out-of-bounds or overlapping fixed
/// structures, invalid booleans, unterminated strings, and invalid constants.
pub fn parse_prefix(bytes: &[u8]) -> Result<Program<'_>, Error> {
    let header = Reader {
        bytes,
        fixed: Vec::new(),
    };
    header.bytes(0, 32)?;
    let profile = header.word(0)?;
    if !matches!(profile, 7003 | 7004) {
        return Err(error(
            0,
            "unsupported profile (expected vertex 7003 or fragment 7004)",
        ));
    }
    let revision = header.word(4)?;
    if revision != 6 {
        return Err(error(4, "unsupported binary revision (expected 6)"));
    }
    let total = header.offset(8)?;
    let mut r = Reader {
        bytes: header.bytes(0, total)?,
        fixed: Vec::new(),
    };
    r.bytes(0, 32)?;
    r.fixed.push(0..32);
    let count = r.offset(12)?;
    let params = r.offset(16)?;
    let stage_at = r.offset(20)?;
    let ucode_size = r.offset(24)?;
    let ucode_at = r.offset(28)?;
    let param_range = r.range(params, count, 48)?;
    r.reserve(param_range)?;
    let stage_range = r.range(stage_at, 1, 24)?;
    r.reserve(stage_range)?;
    let ucode_range = r.range(ucode_at, ucode_size, 1)?;
    r.reserve(ucode_range)?;
    if !ucode_at.is_multiple_of(16) || !ucode_size.is_multiple_of(16) {
        return Err(error(28, "microcode offset or size is not 16-byte aligned"));
    }
    let stage_bytes = r.bytes(stage_at, 24)?;
    let s = Reader {
        bytes: stage_bytes,
        fixed: Vec::new(),
    };
    let instruction_count = s.word(0)?;
    if u64::from(instruction_count) * 16 != ucode_size as u64 {
        return Err(error(
            stage_at,
            "instruction slot count disagrees with microcode size",
        ));
    }
    let stage = if profile == 7003 {
        Stage::Vertex(Vertex {
            instruction_count,
            instruction_slot: s.word(4)?,
            register_count: s.word(8)?,
            attribute_input_mask: s.word(12)?,
            attribute_output_mask: s.word(16)?,
            user_clip_mask: s.word(20)?,
        })
    } else {
        let half = |at| {
            View::new(stage_bytes)
                .u16(at, Big)
                .map_err(|e| error(stage_at + at, e.to_string()))
        };
        Stage::Fragment(Fragment {
            instruction_count,
            attribute_input_mask: s.word(4)?,
            partial_tex_type: s.word(8)?,
            tex_coords_input_mask: half(12)?,
            tex_coords_2d: half(14)?,
            tex_coords_centroid: half(16)?,
            register_count: stage_bytes[18],
            output_from_h0: r.boolean(stage_at + 19, true)?,
            depth_replace: r.boolean(stage_at + 20, true)?,
            pixel_kill: r.boolean(stage_at + 21, true)?,
            padding: [stage_bytes[22], stage_bytes[23]],
        })
    };
    let mut parameters = Vec::new();
    for i in 0..count {
        let at = params + i * 48;
        let ty = r.word(at)?;
        // The source signed fields are two's-complement 32-bit values.
        parameters.push(Parameter {
            ty,
            resource: r.word(at + 4)?,
            variability: r.word(at + 8)?,
            resource_index: r.word(at + 12)? as i32,
            name: r.string(at + 16)?,
            default_value: r.floats(at + 20, ty)?,
            embedded_constants: r.embedded(at + 24, ucode_size)?,
            semantic: r.string(at + 28)?,
            direction: r.word(at + 32)?,
            parameter_number: r.word(at + 36)? as i32,
            referenced: r.boolean(at + 40, false)?,
            shared: r.boolean(at + 44, false)?,
        });
    }
    Ok(Program {
        profile,
        revision,
        stage,
        parameters,
        microcode: r.bytes(ucode_at, ucode_size)?,
        bytes: r.bytes,
    })
}
