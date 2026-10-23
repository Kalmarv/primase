//! The token-level decoder of Shader Model 4 and 5 programs: any stage's version token, declarations as typed
//! records, and instructions as an opcode, its control bits, its extended tokens and its operands. It decides nothing
//! about what a consumer supports: [`crate::Program`] (the bounded evaluator) and [`crate::lift`] each refuse, by name,
//! what they cannot use. A token it cannot read at all (an operand register type it has no name for, a truncated
//! instruction) is an error naming the word.

use crate::opcodes::NAMES;
use crate::{Result, error};

/// The kind of register an operand names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Register {
    Temp,
    Input,
    Output,
    /// An indexable temporary (`x<n>[i]`).
    IndexableTemp,
    Constant,
    Immediate,
    Texture,
    Sampler,
    Null,
    /// An unordered-access view (`u<n>`).
    Uav,
    /// The immediate constant buffer (`icb[n]`), its rows the program's own.
    ImmediateBuffer,
    /// `vPrim`, the primitive id input.
    PrimitiveId,
    /// `vCoverage`, the input coverage mask.
    InputCoverage,
    /// `oDepth`.
    OutputDepth,
    OutputDepthGreaterEqual,
    OutputDepthLessEqual,
    /// `oMask`, the coverage output.
    OutputCoverage,
    OutputStencilRef,
}
impl Register {
    /// How many index dimensions an operand of this register has in Shader Model 5.0.
    fn dimensions(self) -> u32 {
        match self {
            Register::Immediate
            | Register::Null
            | Register::PrimitiveId
            | Register::InputCoverage
            | Register::OutputDepth
            | Register::OutputDepthGreaterEqual
            | Register::OutputDepthLessEqual
            | Register::OutputCoverage
            | Register::OutputStencilRef => 0,
            Register::Constant | Register::IndexableTemp => 2,
            _ => 1,
        }
    }
    fn from_type(n: u32) -> Option<Self> {
        Some(match n {
            0 => Register::Temp,
            1 => Register::Input,
            2 => Register::Output,
            3 => Register::IndexableTemp,
            4 => Register::Immediate,
            6 => Register::Sampler,
            7 => Register::Texture,
            8 => Register::Constant,
            9 => Register::ImmediateBuffer,
            11 => Register::PrimitiveId,
            12 => Register::OutputDepth,
            13 => Register::Null,
            15 => Register::OutputCoverage,
            30 => Register::Uav,
            35 => Register::InputCoverage,
            38 => Register::OutputDepthGreaterEqual,
            39 => Register::OutputDepthLessEqual,
            41 => Register::OutputStencilRef,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Mask(u8),
    Swizzle([u8; 4]),
    Scalar(u8),
    None,
}

#[derive(Clone, Debug)]
pub struct Operand {
    pub register: Register,
    pub selection: Selection,
    pub indices: Vec<Index>,
    pub immediate: [u32; 4],
    /// 0 none, 1 negate, 2 absolute, 3 negated absolute (float sources); 4 two's-complement negate (an integer
    /// instruction's negate, set by the consumer that knows the instruction reads integers).
    pub modifier: u8,
    /// The minimum-precision annotation of the operand's extended token (0 none; 1 float16, 2 float2_8, 4 sint16,
    /// 5 uint16).
    pub precision: u8,
    /// The operand's extended token marks it as a non-uniform resource index.
    pub non_uniform: bool,
}

#[derive(Clone, Debug)]
pub struct Index {
    pub base: u32,
    pub relative: Option<Box<Operand>>,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    /// The instruction's first word in the token stream.
    pub word: usize,
    pub opcode: u16,
    /// The opcode token's bits 11 to 23: saturate and the opcode-specific controls (the test of a conditional, a
    /// `resinfo` return type, the interpolation of a declaration).
    pub control: u32,
    /// The opcode's extended tokens as stored (sample offsets, resource dimension, return type).
    pub extensions: Vec<u32>,
    pub saturate: bool,
    /// A conditional (`if`, `breakc`, `continuec`) taken when its operand is nonzero (`_nz`), else when zero (`_z`).
    pub nonzero: bool,
    pub operands: Vec<Operand>,
}
impl Instruction {
    pub fn name(&self) -> &'static str {
        opcode_name(self.opcode)
    }
    /// The sample offsets of the extended token of kind 1 (`_aoffimmi`), if the instruction has one.
    pub fn sample_offset(&self) -> Option<[i32; 3]> {
        self.extension(1).map(|e| {
            let field = |shift: u32| (((e >> shift) & 15) as i32) << 28 >> 28;
            [field(9), field(13), field(17)]
        })
    }
    /// The resource dimension and structured stride of the extended token of kind 2.
    pub fn resource_dimension(&self) -> Option<(u32, u32)> {
        self.extension(2).map(|e| ((e >> 6) & 31, (e >> 11) & 0xf_ffff))
    }
    /// The return-type token of the extended token of kind 3 (four nibbles).
    pub fn return_type(&self) -> Option<u32> {
        self.extension(3).map(|e| (e >> 6) & 0xffff)
    }
    fn extension(&self, kind: u32) -> Option<u32> {
        self.extensions.iter().copied().find(|e| e & 63 == kind)
    }
}

pub fn opcode_name(opcode: u16) -> &'static str {
    NAMES.get(usize::from(opcode)).copied().unwrap_or("unknown opcode")
}

/// The type of a program's four return lanes in a resource declaration, as D3D_RETURN_TYPE codes (1 unorm, 2 snorm,
/// 3 sint, 4 uint, 5 float, 6 mixed, 7 double, 8 continued, 9 unused), one nibble per lane.
pub type ReturnCodes = [u8; 4];

/// A declaration, typed. `word` is where it starts.
#[derive(Clone, Debug)]
pub struct Declaration {
    pub word: usize,
    pub opcode: u16,
    /// The opcode token's bits 11 to 23 as stored.
    pub control: u32,
    pub kind: Declared,
}

#[derive(Clone, Debug)]
pub enum Declared {
    /// `dcl_temps`.
    Temps(u32),
    /// `dcl_indexable_temp x<register>[size], components`.
    IndexableTemp { register: u32, size: u32, components: u32 },
    /// `dcl_globalFlags`: the control bits as stored (bit 0 refactoring allowed, 1 double precision, 2 forced early
    /// depth-stencil, 3 raw and structured buffers, 4 skip optimization, 5 min precision, 6 double extensions, 7
    /// shader extensions).
    GlobalFlags(u32),
    /// `dcl_constantbuffer cb<slot>[rows]`; `dynamic` when it is dynamically indexed.
    ConstantBuffer { slot: u32, rows: u32, dynamic: bool, dimensions: usize },
    /// `dcl_sampler s<slot>`, its mode: 0 default, 1 comparison, 2 mono.
    Sampler { slot: u32, mode: u32 },
    /// `dcl_resource_<dimension> t<slot>`: the D3D resource dimension code, the sample count of a multisampled one,
    /// and the four return types.
    Resource { slot: u32, dimension: u32, samples: u32, returns: ReturnCodes, token: u32 },
    ResourceRaw { slot: u32 },
    ResourceStructured { slot: u32, stride: u32 },
    /// `dcl_uav_typed` and its control bits (dimension in bits 0 to 4, globally coherent bit 5).
    UavTyped { slot: u32, dimension: u32, returns: ReturnCodes, token: u32, control: u32 },
    UavRaw { slot: u32, control: u32 },
    UavStructured { slot: u32, stride: u32, control: u32 },
    /// An input register or special input: `dcl_input`, `_sgv`, `_siv`, `_ps`, `_ps_sgv`, `_ps_siv`. `interpolation` is
    /// a pixel input's `D3D_INTERPOLATION_MODE`; `name` a system value's `D3D_NAME`.
    Input { operand: Operand, interpolation: Option<u32>, name: Option<u32> },
    /// An output register or special output: `dcl_output`, `_sgv`, `_siv`.
    Output { operand: Operand, name: Option<u32> },
    /// `dcl_immediateConstantBuffer`.
    ImmediateBuffer(Vec<[u32; 4]>),
    /// Custom data that is not an immediate constant buffer (a comment, debug information, ...) by its class.
    CustomData { class: u32 },
    /// A declaration this decoder does not type: stage-specific (geometry, tessellation, compute), interface tables.
    Other { opcode: u16, control: u32, words: Vec<u32> },
}

/// The kind of program a version token names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramType {
    Pixel,
    Vertex,
    Geometry,
    Hull,
    Domain,
    Compute,
}
impl ProgramType {
    pub fn name(self) -> &'static str {
        match self {
            ProgramType::Pixel => "ps",
            ProgramType::Vertex => "vs",
            ProgramType::Geometry => "gs",
            ProgramType::Hull => "hs",
            ProgramType::Domain => "ds",
            ProgramType::Compute => "cs",
        }
    }
}

/// A decoded token stream: its target, declarations and instructions, each in program order.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub kind: ProgramType,
    pub major: u32,
    pub minor: u32,
    pub declarations: Vec<Declaration>,
    pub instructions: Vec<Instruction>,
}
impl Decoded {
    /// The target's name (`ps_5_0`).
    pub fn target(&self) -> String {
        format!("{}_{}_{}", self.kind.name(), self.major, self.minor)
    }
}

struct Cursor<'a> {
    words: &'a [u32],
    at: usize,
    origin: usize,
}
impl Cursor<'_> {
    fn take(&mut self) -> Result<u32> {
        let value = *self
            .words
            .get(self.at)
            .ok_or_else(|| error(self.origin + self.at, "truncated instruction"))?;
        self.at += 1;
        Ok(value)
    }
    fn rest(&mut self) -> Vec<u32> {
        let rest = self.words[self.at..].to_vec();
        self.at = self.words.len();
        rest
    }
    fn operand(&mut self, depth: usize) -> Result<Operand> {
        let pos = self.origin + self.at;
        if depth > 2 {
            return Err(error(pos, "nested relative operand exceeds bound"));
        }
        let t = self.take()?;
        let components = t & 3;
        let selection = match components {
            0 => Selection::None,
            1 => Selection::Scalar(0),
            2 => match (t >> 2) & 3 {
                0 => Selection::Mask(((t >> 4) & 15) as u8),
                1 => Selection::Swizzle(std::array::from_fn(|i| ((t >> (4 + 2 * i)) & 3) as u8)),
                2 => Selection::Scalar(((t >> 4) & 3) as u8),
                _ => return Err(error(pos, "reserved component selection")),
            },
            _ => return Err(error(pos, "unsupported N-component operand")),
        };
        let kind = (t >> 12) & 255;
        let register = Register::from_type(kind)
            .ok_or_else(|| error(pos, format!("unsupported operand register type {kind}")))?;
        let (mut modifier, mut precision, mut non_uniform) = (0, 0, false);
        if t >> 31 != 0 {
            let mut ext = self.take()?;
            loop {
                // only the modifier extension (kind 1) exists in Shader Model 4 and 5
                if ext & 63 != 1 {
                    return Err(error(pos, "unsupported extended operand (precision/modifier)"));
                }
                modifier = ((ext >> 6) & 255) as u8;
                precision = ((ext >> 14) & 7) as u8;
                non_uniform = ext & (1 << 17) != 0;
                if ext >> 31 == 0 {
                    break;
                }
                ext = self.take()?;
            }
            if modifier > 3 {
                return Err(error(pos, "unsupported extended operand (precision/modifier)"));
            }
        }
        let dimensions = (t >> 20) & 3;
        if dimensions != register.dimensions() {
            return Err(error(pos, "unexpected operand index dimensions"));
        }
        let mut indices = Vec::new();
        for d in 0..dimensions {
            let representation = (t >> (22 + 3 * d)) & 7;
            if !matches!(representation, 0 | 2 | 3) {
                return Err(error(pos, "unsupported 64-bit/reserved index"));
            }
            let base = if representation != 2 { self.take()? } else { 0 };
            let relative = if representation != 0 {
                Some(Box::new(self.operand(depth + 1)?))
            } else {
                None
            };
            indices.push(Index { base, relative });
        }
        let mut immediate = [0; 4];
        if register == Register::Immediate {
            match components {
                1 => immediate = [self.take()?; 4],
                2 => {
                    for x in &mut immediate {
                        *x = self.take()?;
                    }
                }
                _ => return Err(error(pos, "immediate requires one or four components")),
            }
        }
        Ok(Operand {
            register,
            selection,
            indices,
            immediate,
            modifier,
            precision,
            non_uniform,
        })
    }
}

fn return_codes(token: u32) -> ReturnCodes {
    std::array::from_fn(|i| ((token >> (4 * i)) & 15) as u8)
}

/// The opcodes that are declarations (and the phase markers of hull programs, which carry none).
fn is_declaration(opcode: u16) -> bool {
    matches!(opcode, 88..=106 | 113..=116 | 143..=162)
}

/// Decode a complete token stream: version token, length, then declarations and instructions to its end.
pub fn decode(words: &[u32]) -> Result<Decoded> {
    let (Some(&version), Some(&length)) = (words.first(), words.get(1)) else {
        return Err(error(0, "token stream shorter than its header"));
    };
    if length as usize != words.len() {
        return Err(error(1, "declared length differs from the token stream"));
    }
    let kind = match version >> 16 {
        0 => ProgramType::Pixel,
        1 => ProgramType::Vertex,
        2 => ProgramType::Geometry,
        3 => ProgramType::Hull,
        4 => ProgramType::Domain,
        5 => ProgramType::Compute,
        other => return Err(error(0, format!("program type {other} is not a Shader Model 4 or 5 stage"))),
    };
    let mut result = Decoded {
        kind,
        major: (version >> 4) & 15,
        minor: version & 15,
        declarations: Vec::new(),
        instructions: Vec::new(),
    };
    let mut at = 2;
    while at < words.len() {
        let t = words[at];
        let opcode = (t & 0x7ff) as u16;
        // customdata: its class in the opcode token's upper bits, its length (these two words included) the next word
        if opcode == 53 {
            let len = words.get(at + 1).copied().unwrap_or(0) as usize;
            if len < 2 || at + len > words.len() {
                return Err(error(at, "invalid customdata length"));
            }
            let class = t >> 11;
            let kind = if class == 3 {
                if !(len - 2).is_multiple_of(4) {
                    return Err(error(at, "an immediate constant buffer of partial rows"));
                }
                Declared::ImmediateBuffer(words[at + 2..at + len].as_chunks::<4>().0.to_vec())
            } else {
                Declared::CustomData { class }
            };
            result.declarations.push(Declaration {
                word: at,
                opcode,
                control: class,
                kind,
            });
            at += len;
            continue;
        }
        let len = ((t >> 24) & 127) as usize;
        if len == 0 || at + len > words.len() {
            return Err(error(at, "invalid instruction length"));
        }
        let mut c = Cursor {
            words: &words[at + 1..at + len],
            at: 0,
            origin: at + 1,
        };
        let mut extensions = Vec::new();
        let mut extended = t >> 31 != 0;
        while extended {
            let ext = c.take()?;
            extensions.push(ext);
            extended = ext >> 31 != 0;
        }
        let control = (t >> 11) & 0x1fff;
        if is_declaration(opcode) {
            let kind = declaration(opcode, control, &mut c)?;
            result.declarations.push(Declaration {
                word: at,
                opcode,
                control,
                kind,
            });
        } else {
            let mut operands = Vec::new();
            while c.at < c.words.len() {
                operands.push(c.operand(0)?);
            }
            result.instructions.push(Instruction {
                word: at,
                opcode,
                control,
                extensions,
                saturate: t & 0x2000 != 0,
                nonzero: t & 0x4_0000 != 0,
                operands,
            });
        }
        at += len;
    }
    Ok(result)
}

fn declaration(opcode: u16, control: u32, c: &mut Cursor<'_>) -> Result<Declared> {
    let pos = c.origin - 1;
    let slot = |o: &Operand| -> Result<u32> {
        o.indices
            .first()
            .map(|i| i.base)
            .ok_or_else(|| error(pos, "missing declaration index"))
    };
    Ok(match opcode {
        104 => Declared::Temps(c.take()?),
        105 => Declared::IndexableTemp {
            register: c.take()?,
            size: c.take()?,
            components: c.take()?,
        },
        106 => Declared::GlobalFlags(control),
        88 => {
            let o = c.operand(0)?;
            let token = c.take()?;
            Declared::Resource {
                slot: slot(&o)?,
                dimension: control & 31,
                samples: (control >> 5) & 127,
                returns: return_codes(token),
                token,
            }
        }
        89 => {
            let o = c.operand(0)?;
            Declared::ConstantBuffer {
                slot: slot(&o)?,
                rows: o.indices.get(1).map_or(0, |i| i.base),
                dynamic: control & 1 != 0,
                dimensions: o.indices.len(),
            }
        }
        90 => {
            let o = c.operand(0)?;
            Declared::Sampler {
                slot: slot(&o)?,
                mode: control & 15,
            }
        }
        95..=100 => {
            let operand = c.operand(0)?;
            let pixel = matches!(opcode, 98..=100);
            let interpolation = pixel.then_some(control & 15);
            let name = matches!(opcode, 96 | 97 | 99 | 100).then(|| c.take()).transpose()?;
            Declared::Input {
                operand,
                interpolation,
                name,
            }
        }
        101..=103 => {
            let operand = c.operand(0)?;
            let name = (opcode != 101).then(|| c.take()).transpose()?;
            Declared::Output { operand, name }
        }
        156 => {
            let o = c.operand(0)?;
            let token = c.take()?;
            Declared::UavTyped {
                slot: slot(&o)?,
                dimension: control & 31,
                returns: return_codes(token),
                token,
                control,
            }
        }
        157 => {
            let o = c.operand(0)?;
            Declared::UavRaw {
                slot: slot(&o)?,
                control,
            }
        }
        158 => {
            let o = c.operand(0)?;
            Declared::UavStructured {
                slot: slot(&o)?,
                stride: c.take()?,
                control,
            }
        }
        161 => {
            let o = c.operand(0)?;
            Declared::ResourceRaw { slot: slot(&o)? }
        }
        162 => {
            let o = c.operand(0)?;
            Declared::ResourceStructured {
                slot: slot(&o)?,
                stride: c.take()?,
            }
        }
        _ => Declared::Other {
            opcode,
            control,
            words: c.rest(),
        },
    })
}
