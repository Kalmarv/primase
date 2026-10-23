//! RAGE's compiled effect files (`.fxc`, magic `rgxe`) as GTA V Legacy stores them (the Enhanced edition ships no
//! `.fxc`: its programs are one `SGD2` container, [`super::sgd2`]; a census of this reader needs a Legacy install): the effect's preset parameters, the
//! shader programs of its six stages (each a Direct3D container, DXBC on the Legacy edition and DXIL on Enhanced), the
//! constant buffers and variables they use, and the techniques that pick programs for a draw. No I/O: the reader
//! hands over bytes.
//!
//! Byte order is little endian. A string is one length byte (which counts a terminating zero) then that many bytes; a
//! length of zero is the empty string. The file is read to its end: bytes after the last technique are an error, not
//! skipped. What the format means beyond its layout (which variables are material parameters, what a preset is for)
//! is the binding rules', not this reader's, so every field is kept as stored.
//!
//! The layout is the format as CodeWalker.Core (`FxcFile`) reads it; the facts are the layout, no code is shared.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Result, invalid};

/// The magic: `rgxe`, as the first four bytes.
pub const MAGIC: u32 = 0x6578_6772;
/// A Direct3D container's magic: `DXBC`.
const CONTAINER_MAGIC: u32 = 0x4342_5844;

/// The stages of an effect, in the order the file lists their groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProgramStage {
    Vertex,
    Pixel,
    Compute,
    Domain,
    Geometry,
    Hull,
}
impl ProgramStage {
    pub const ALL: [ProgramStage; 6] = [
        ProgramStage::Vertex,
        ProgramStage::Pixel,
        ProgramStage::Compute,
        ProgramStage::Domain,
        ProgramStage::Geometry,
        ProgramStage::Hull,
    ];
}

/// A preset parameter: a named value the effect starts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetParam {
    pub name: String,
    pub unused: u8,
    pub value: u32,
}

/// One shader program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shader {
    pub name: String,
    /// A second name, where the first is empty (the file then carries both).
    pub second_name: Option<String>,
    pub variables: Vec<String>,
    /// The constant buffers it reads: name and register.
    pub buffers: Vec<(String, u16)>,
    /// A geometry shader's extra byte.
    pub geometry_byte: Option<u8>,
    /// The Direct3D container, as stored (empty when the file stores none).
    pub bytecode: Vec<u8>,
    /// The shader model's major and minor, stored after the program of the vertex, pixel and geometry stages.
    pub version: Option<(u8, u8)>,
}

/// The programs of one stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The count byte was zero and another byte followed it.
    pub extra_count_byte: bool,
    pub name: String,
    pub unknown_bytes: [u8; 2],
    pub unknown_word: u32,
    pub shaders: Vec<Shader>,
}

/// A constant buffer of the effect: its size, the register it has in each stage (one per [`ProgramStage`]) and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstantBuffer {
    pub size: u32,
    pub registers: [u16; 6],
    pub name: String,
}

/// A parameter of a variable.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    Int(i32),
    Float(f32),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariableParam {
    pub name: String,
    pub value: ParamValue,
}

/// A variable of the effect: a constant, a texture, a buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub kind: u8,
    pub count: u8,
    pub slot: u8,
    pub group: u8,
    pub name: String,
    pub name2: String,
    pub offset: u8,
    pub variant: u8,
    pub unused: [u8; 2],
    /// The hash of the constant buffer's name.
    pub constant_buffer: u32,
    pub params: Vec<VariableParam>,
    /// The default value's words as stored (integers and handles are integers, the rest floats: `kind` says).
    pub values: Vec<u32>,
}

/// One pass of a technique: the program of each stage (1-based into that stage's group, 0 none) and its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    pub programs: [u8; 6],
    pub params: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Technique {
    pub name: String,
    pub passes: Vec<Pass>,
}

/// A compiled effect.
#[derive(Clone, Debug, PartialEq)]
pub struct Fxc {
    pub vertex_type: u32,
    pub presets: Vec<PresetParam>,
    /// The six stages' groups, in [`ProgramStage::ALL`] order.
    pub groups: [Group; 6],
    pub constant_buffers: [Vec<ConstantBuffer>; 2],
    pub variables: [Vec<Variable>; 2],
    pub techniques: Vec<Technique>,
}

impl Fxc {
    /// The programs of a stage.
    pub fn shaders(&self, stage: ProgramStage) -> &[Shader] {
        self.groups
            .get(stage as usize)
            .map_or(&[], |g| g.shaders.as_slice())
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| invalid("fxc", self.at as u64, "length overflow"))?;
        let s = self.data.get(self.at..end).ok_or_else(|| {
            invalid(
                "fxc",
                self.at as u64,
                format!("{n} bytes past the {} there are", self.data.len()),
            )
        })?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| invalid("fxc", self.at as u64, "a byte"))
    }
    fn u16(&mut self) -> Result<u16> {
        let b: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| invalid("fxc", self.at as u64, "two bytes"))?;
        Ok(u16::from_le_bytes(b))
    }
    fn u32(&mut self) -> Result<u32> {
        let b: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| invalid("fxc", self.at as u64, "four bytes"))?;
        Ok(u32::from_le_bytes(b))
    }
    fn string(&mut self) -> Result<String> {
        let start = self.at;
        let length = usize::from(self.u8()?);
        if length == 0 {
            return Ok(String::new());
        }
        let bytes = self.take(length)?;
        let (text, last) = bytes.split_at(length.saturating_sub(1));
        if last != [0] {
            return Err(invalid("fxc", start as u64, "a string without its terminator"));
        }
        if !text.is_ascii() {
            return Err(invalid("fxc", start as u64, "a string that is not ASCII"));
        }
        Ok(String::from_utf8_lossy(text).into_owned())
    }
    fn strings(&mut self) -> Result<Vec<String>> {
        let n = self.u8()?;
        (0..n).map(|_| self.string()).collect()
    }
}

fn shader(c: &mut Cursor<'_>, stage: ProgramStage) -> Result<Shader> {
    let name = c.string()?;
    let second_name = if name.is_empty() { Some(c.string()?) } else { None };
    let variables = c.strings()?;
    let buffer_count = c.u8()?;
    let mut buffers = Vec::new();
    for _ in 0..buffer_count {
        let n = c.string()?;
        buffers.push((n, c.u16()?));
    }
    let geometry_byte = if stage == ProgramStage::Geometry {
        Some(c.u8()?)
    } else {
        None
    };
    let length = c.u32()? as usize;
    let at = c.at;
    let bytecode = if length == 0 {
        Vec::new()
    } else {
        // the container, whose own first word is its magic
        let bytes = c.take(length)?;
        let magic = bytes
            .get(..4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes);
        if magic != Some(CONTAINER_MAGIC) {
            return Err(invalid(
                "fxc",
                at as u64,
                "a program that is not a DXBC container",
            ));
        }
        bytes.to_vec()
    };
    let version = if matches!(
        stage,
        ProgramStage::Vertex | ProgramStage::Pixel | ProgramStage::Geometry
    ) {
        let major = c.u8()?;
        Some((major, c.u8()?))
    } else {
        None
    };
    Ok(Shader {
        name,
        second_name,
        variables,
        buffers,
        geometry_byte,
        bytecode,
        version,
    })
}

fn group(c: &mut Cursor<'_>, stage: ProgramStage) -> Result<Group> {
    let mut count = c.u8()?;
    let extra_count_byte = count == 0;
    if extra_count_byte {
        count = c.u8()?;
    }
    let name = c.string()?;
    let unknown_bytes = [c.u8()?, c.u8()?];
    let unknown_word = c.u32()?;
    // the stored count is one more than the number of programs
    let n = count.saturating_sub(1);
    let shaders = (0..n).map(|_| shader(c, stage)).collect::<Result<Vec<_>>>()?;
    Ok(Group {
        extra_count_byte,
        name,
        unknown_bytes,
        unknown_word,
        shaders,
    })
}

fn constant_buffers(c: &mut Cursor<'_>) -> Result<Vec<ConstantBuffer>> {
    let n = c.u8()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let size = c.u32()?;
        let mut registers = [0u16; 6];
        for r in &mut registers {
            *r = c.u16()?;
        }
        out.push(ConstantBuffer {
            size,
            registers,
            name: c.string()?,
        });
    }
    Ok(out)
}

/// Variable kinds whose values are integers: Int, Int4, Boolean, Texture, Buffer and the unordered-access ones are
/// stored as words; no reading of what a word means is made here, so the words are kept raw whatever the kind.
fn variables(c: &mut Cursor<'_>) -> Result<Vec<Variable>> {
    let n = c.u8()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let (kind, count, slot, group) = (c.u8()?, c.u8()?, c.u8()?, c.u8()?);
        let name = c.string()?;
        let name2 = c.string()?;
        let (offset, variant) = (c.u8()?, c.u8()?);
        let unused = [c.u8()?, c.u8()?];
        let constant_buffer = c.u32()?;
        let param_count = c.u8()?;
        let mut params = Vec::new();
        for _ in 0..param_count {
            let pname = c.string()?;
            let value = match c.u8()? {
                0 => ParamValue::Int(i32::from_le_bytes(c.u32()?.to_le_bytes())),
                1 => ParamValue::Float(f32::from_bits(c.u32()?)),
                2 => ParamValue::Text(c.string()?),
                other => {
                    return Err(invalid(
                        "fxc",
                        c.at as u64,
                        format!("variable parameter type {other}"),
                    ));
                }
            };
            params.push(VariableParam { name: pname, value });
        }
        let value_count = c.u8()?;
        let values = (0..value_count).map(|_| c.u32()).collect::<Result<Vec<_>>>()?;
        out.push(Variable {
            kind,
            count,
            slot,
            group,
            name,
            name2,
            offset,
            variant,
            unused,
            constant_buffer,
            params,
            values,
        });
    }
    Ok(out)
}

fn techniques(c: &mut Cursor<'_>) -> Result<Vec<Technique>> {
    let n = c.u8()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let name = c.string()?;
        let passes = c.u8()?;
        let mut list = Vec::new();
        for _ in 0..passes {
            let mut programs = [0u8; 6];
            for p in &mut programs {
                *p = c.u8()?;
            }
            let count = c.u8()?;
            let mut params = Vec::new();
            for _ in 0..count {
                let kind = c.u32()?;
                params.push((kind, c.u32()?));
            }
            list.push(Pass { programs, params });
        }
        out.push(Technique { name, passes: list });
    }
    Ok(out)
}

/// Read a compiled effect. The whole file is consumed: anything after the techniques is an error.
pub fn parse(data: &[u8]) -> Result<Fxc> {
    let mut c = Cursor { data, at: 0 };
    let magic = c.u32()?;
    if magic != MAGIC {
        return Err(invalid("fxc", 0, format!("magic {magic:#010x}, not rgxe")));
    }
    let vertex_type = c.u32()?;
    let preset_count = c.u8()?;
    let mut presets = Vec::new();
    for _ in 0..preset_count {
        let name = c.string()?;
        let unused = c.u8()?;
        presets.push(PresetParam {
            name,
            unused,
            value: c.u32()?,
        });
    }
    let mut groups = Vec::new();
    for stage in ProgramStage::ALL {
        groups.push(group(&mut c, stage)?);
    }
    let groups: [Group; 6] = groups
        .try_into()
        .map_err(|_| invalid("fxc", c.at as u64, "six stage groups"))?;
    let cbuffers1 = constant_buffers(&mut c)?;
    let variables1 = variables(&mut c)?;
    let cbuffers2 = constant_buffers(&mut c)?;
    let variables2 = variables(&mut c)?;
    let techniques = techniques(&mut c)?;
    if c.at != data.len() {
        return Err(invalid(
            "fxc",
            c.at as u64,
            format!(
                "{} bytes after the last technique",
                data.len().saturating_sub(c.at)
            ),
        ));
    }
    Ok(Fxc {
        vertex_type,
        presets,
        groups,
        constant_buffers: [cbuffers1, cbuffers2],
        variables: [variables1, variables2],
        techniques,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, s: &str) {
        if s.is_empty() {
            out.push(0);
        } else {
            out.push(u8::try_from(s.len() + 1).unwrap());
            out.extend(s.as_bytes());
            out.push(0);
        }
    }
    fn container() -> Vec<u8> {
        let mut c = b"DXBC".to_vec();
        c.extend([0u8; 28]);
        c
    }

    /// An effect written by hand following the layout: one vertex and one pixel program, two constant buffers, two
    /// variables, a technique of one pass.
    fn effect() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"rgxe");
        b.extend(7u32.to_le_bytes());
        // one preset parameter
        b.push(1);
        string(&mut b, "bool_a");
        b.push(0);
        b.extend(1u32.to_le_bytes());
        for stage in ProgramStage::ALL {
            let programs = match stage {
                ProgramStage::Vertex | ProgramStage::Pixel => 1u8,
                _ => 0,
            };
            // the stored count is one more than the programs
            b.push(programs + 1);
            string(&mut b, &format!("{stage:?}Group"));
            b.extend([3, 4]);
            b.extend(9u32.to_le_bytes());
            for k in 0..programs {
                string(&mut b, &format!("{stage:?}Shader{k}"));
                // variables
                b.push(2);
                string(&mut b, "world");
                string(&mut b, "tint");
                // buffer references
                b.push(1);
                string(&mut b, "rage_matrices");
                b.extend(1u16.to_le_bytes());
                let code = container();
                b.extend(u32::try_from(code.len()).unwrap().to_le_bytes());
                b.extend(&code);
                b.extend([5, 0]);
            }
        }
        // constant buffers (twice, with variables between)
        for _ in 0..2 {
            b.push(2);
            for (n, size) in [("rage_matrices", 256u32), ("misc", 16)] {
                b.extend(size.to_le_bytes());
                for r in 0..6u16 {
                    b.extend(r.to_le_bytes());
                }
                string(&mut b, n);
            }
            b.push(2);
            for (name, kind) in [("world", 11u8), ("tint", 8)] {
                b.extend([kind, 1, 0, 0]);
                string(&mut b, name);
                string(&mut b, "");
                b.extend([16, 0, 0, 0]);
                b.extend(0x1234_5678u32.to_le_bytes());
                // one parameter of each type
                b.push(3);
                string(&mut b, "i");
                b.push(0);
                b.extend((-5i32).to_le_bytes());
                string(&mut b, "f");
                b.push(1);
                b.extend(0.5f32.to_bits().to_le_bytes());
                string(&mut b, "s");
                b.push(2);
                string(&mut b, "text");
                // two default words
                b.push(2);
                b.extend(1.0f32.to_bits().to_le_bytes());
                b.extend(2.0f32.to_bits().to_le_bytes());
            }
        }
        // one technique, one pass: vertex program 1, pixel program 1
        b.push(1);
        string(&mut b, "draw");
        b.push(1);
        b.extend([1, 1, 0, 0, 0, 0]);
        b.push(1);
        b.extend(7u32.to_le_bytes());
        b.extend(8u32.to_le_bytes());
        b
    }

    #[test]
    fn an_effect_written_by_hand_reads_back_field_by_field() {
        let fx = parse(&effect()).unwrap();
        assert_eq!(fx.vertex_type, 7);
        assert_eq!(fx.presets.len(), 1);
        assert_eq!((fx.presets[0].name.as_str(), fx.presets[0].value), ("bool_a", 1));
        assert_eq!(fx.shaders(ProgramStage::Vertex).len(), 1);
        assert_eq!(fx.shaders(ProgramStage::Pixel).len(), 1);
        assert!(fx.shaders(ProgramStage::Compute).is_empty());
        let vs = &fx.shaders(ProgramStage::Vertex)[0];
        assert_eq!(vs.name, "VertexShader0");
        assert_eq!(vs.variables, ["world", "tint"]);
        assert_eq!(vs.buffers, [("rage_matrices".to_owned(), 1)]);
        assert_eq!(vs.version, Some((5, 0)));
        assert!(vs.bytecode.starts_with(b"DXBC"));
        assert_eq!(fx.groups[4].shaders.len(), 0);
        assert_eq!(fx.constant_buffers[0][0].size, 256);
        assert_eq!(fx.constant_buffers[1][1].registers, [0, 1, 2, 3, 4, 5]);
        let v = &fx.variables[0][0];
        assert_eq!((v.name.as_str(), v.kind, v.offset), ("world", 11, 16));
        assert_eq!(v.constant_buffer, 0x1234_5678);
        assert_eq!(v.params[0].value, ParamValue::Int(-5));
        assert_eq!(v.params[1].value, ParamValue::Float(0.5));
        assert_eq!(v.params[2].value, ParamValue::Text("text".into()));
        assert_eq!(v.values, [1.0f32.to_bits(), 2.0f32.to_bits()]);
        assert_eq!(fx.techniques[0].name, "draw");
        assert_eq!(fx.techniques[0].passes[0].programs, [1, 1, 0, 0, 0, 0]);
        assert_eq!(fx.techniques[0].passes[0].params, [(7, 8)]);
    }

    #[test]
    fn what_the_layout_does_not_allow_is_an_error_naming_the_offset() {
        let good = effect();
        // every truncation fails
        for n in 0..good.len() {
            assert!(parse(&good[..n]).is_err(), "{n}");
        }
        // a byte after the last technique
        let mut more = good.clone();
        more.push(0);
        assert!(
            parse(&more)
                .unwrap_err()
                .to_string()
                .contains("after the last technique")
        );
        // another magic
        let mut wrong = good.clone();
        wrong[0] = b'x';
        assert!(parse(&wrong).unwrap_err().to_string().contains("not rgxe"));
        // a program that is not a container
        let at = good.windows(4).position(|w| w == b"DXBC").unwrap();
        let mut broken = good;
        broken[at] = b'Y';
        assert!(
            parse(&broken)
                .unwrap_err()
                .to_string()
                .contains("not a DXBC container")
        );
    }
}
