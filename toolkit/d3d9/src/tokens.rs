use std::collections::BTreeMap;

use primase_shader::TextureDim;

use crate::{ConstantTable, Result, error};

/// A float register in the supported pixel-stage banks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Register {
    Temporary(u32),
    Input(u32),
    Constant(u32),
    Sampler(u32),
    Target(u32),
}

/// A source token after decoding its swizzle and modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub register: Register,
    pub swizzle: [u8; 4],
    pub absolute: bool,
    pub negate: bool,
}

/// A destination token; masks keep their original lane positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destination {
    pub register: Register,
    pub mask: u8,
    pub saturate: bool,
    /// A request that hardware may execute at partial precision. The lifter uses f32 and reports it.
    pub partial_precision: bool,
}

/// The operations this lifter reads. Declarations and immediate definitions are kept separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    Nop,
    Mov,
    Add,
    Sub,
    Mad,
    Mul,
    Rcp,
    Rsq,
    Dp3,
    Dp4,
    Min,
    Max,
    Exp,
    Log,
    Lrp,
    Frc,
    Pow,
    Abs,
    Nrm,
    TexKill,
    Tex,
    Cmp,
    Dp2Add,
}

impl Opcode {
    fn decode(op: u16, at: usize) -> Result<(Self, usize)> {
        use Opcode::*;
        Ok(match op {
            0 => (Nop, 0),
            1 => (Mov, 1),
            2 => (Add, 2),
            3 => (Sub, 2),
            4 => (Mad, 3),
            5 => (Mul, 2),
            6 => (Rcp, 1),
            7 => (Rsq, 1),
            8 => (Dp3, 2),
            9 => (Dp4, 2),
            10 => (Min, 2),
            11 => (Max, 2),
            14 => (Exp, 1),
            15 => (Log, 1),
            18 => (Lrp, 3),
            19 => (Frc, 1),
            32 => (Pow, 2),
            35 => (Abs, 1),
            36 => (Nrm, 1),
            65 => (TexKill, 0),
            66 => (Tex, 2),
            88 => (Cmp, 3),
            90 => (Dp2Add, 3),
            _ => {
                let name = match op {
                    25 => "CALL",
                    26 => "CALLNZ",
                    27 => "LOOP",
                    28 => "RET",
                    29 => "ENDLOOP",
                    30 => "LABEL",
                    33 => "CRS",
                    34 => "SGN",
                    37 => "SINCOS",
                    38 => "REP",
                    39 => "ENDREP",
                    40 => "IF",
                    41 => "IFC",
                    42 => "ELSE",
                    43 => "ENDIF",
                    44 => "BREAK",
                    45 => "BREAKC",
                    46 => "MOVA",
                    47 => "DEFB",
                    48 => "DEFI",
                    91 => "DSX",
                    92 => "DSY",
                    93 => "TEXLDD",
                    94 => "SETP",
                    95 => "TEXLDL",
                    96 => "BREAKP",
                    _ => "unrecognized",
                };
                return Err(error(at, format!("opcode {op} {name} outside the pixel subset")));
            }
        })
    }
}

/// An executable operation. `control` is TEX's projection/bias mode (0, 1, 2); zero for arithmetic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub word: usize,
    pub opcode: Opcode,
    pub destination: Option<Destination>,
    pub sources: Vec<Source>,
    pub control: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub register: u32,
    pub usage: u8,
    pub index: u8,
    pub mask: u8,
    pub centroid: bool,
    pub partial_precision: bool,
}

/// A COMMENT's words, excluding its token. Unknown FourCCs are retained, not interpreted as instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub word: usize,
    pub data: Vec<u32>,
}

/// Reflection and executable tokens of one complete `ps_3_0` stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shader {
    pub inputs: Vec<Input>,
    pub samplers: BTreeMap<u32, TextureDim>,
    /// `def` constants, as exact f32 bit patterns.
    pub definitions: BTreeMap<u32, [u32; 4]>,
    pub instructions: Vec<Instruction>,
    pub comments: Vec<Comment>,
    pub constants: Option<ConstantTable>,
}

fn register(t: u32, at: usize) -> Result<Register> {
    if t & 0x8000_0000 == 0 || t & 0xe000 != 0 {
        return Err(error(
            at,
            "parameter marker/reserved bits or relative addressing (not supported)",
        ));
    }
    let kind = ((t >> 28) & 7) | ((t >> 8) & 24);
    let n = t & 2047;
    Ok(match (kind, n) {
        (0, 0..32) => Register::Temporary(n),
        (1, 0..10) => Register::Input(n),
        (2, 0..224) => Register::Constant(n),
        (8, 0..4) => Register::Target(n),
        (10, 0..16) => Register::Sampler(n),
        _ => {
            return Err(error(
                at,
                format!("register type {kind}, index {n} outside the supported ps_3_0 banks"),
            ));
        }
    })
}

fn destination(t: u32, at: usize, declaration: bool) -> Result<Destination> {
    let register = register(t, at)?;
    let mask = ((t >> 16) & 15) as u8;
    let modifiers = (t >> 20) & 15;
    if mask == 0 || t & 0x0f00_0000 != 0 || modifiers & !if declaration { 6 } else { 3 } != 0 {
        return Err(error(at, format!("destination mask/shift/modifier {t:#010x}")));
    }
    Ok(Destination {
        register,
        mask,
        saturate: modifiers & 1 != 0,
        partial_precision: modifiers & 2 != 0,
    })
}

fn source(t: u32, at: usize) -> Result<Source> {
    let register = register(t, at)?;
    let modifier = (t >> 24) & 15;
    if !matches!(modifier, 0 | 1 | 11 | 12) {
        return Err(error(
            at,
            format!("source modifier {modifier} outside NONE/NEG/ABS/ABSNEG"),
        ));
    }
    Ok(Source {
        register,
        swizzle: std::array::from_fn(|n| ((t >> (16 + 2 * n)) & 3) as u8),
        absolute: matches!(modifier, 11 | 12),
        negate: matches!(modifier, 1 | 12),
    })
}

/// Read a complete raw pixel shader, including comment/CTAB payloads and END. Lengths, bank limits,
/// declaration order, duplicate definitions, reserved bits and every operand count are checked.
pub fn read(bytes: &[u8]) -> Result<Shader> {
    if !bytes.len().is_multiple_of(4) || bytes.len() > 4 * 1_048_576 {
        return Err(error(
            0,
            "token stream is not whole words or exceeds 1,048,576 words",
        ));
    }
    let words: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .collect();
    if words.first() != Some(&0xffff_0300) {
        return Err(error(0, "version: only raw ps_3_0 is supported"));
    }
    let mut shader = Shader {
        inputs: Vec::new(),
        samplers: BTreeMap::new(),
        definitions: BTreeMap::new(),
        instructions: Vec::new(),
        comments: Vec::new(),
        constants: None,
    };
    let mut at = 1;
    let mut executable = false;
    while let Some(&token) = words.get(at) {
        let word = at;
        at += 1;
        if token == 0xffff {
            if at != words.len() {
                return Err(error(at, "bytes after END"));
            }
            return Ok(shader);
        }
        let op = (token & 65535) as u16;
        let count = if op == 65534 {
            (token >> 16) & 32767
        } else {
            (token >> 24) & 15
        } as usize;
        let args = words
            .get(at..at + count)
            .ok_or_else(|| error(word, "instruction/comment extends past the stream"))?;
        at += count;
        if op == 65534 {
            if token & 0x8000_0000 != 0 {
                return Err(error(word, "COMMENT reserved bit"));
            }
            if args.first() == Some(&u32::from_le_bytes(*b"CTAB")) {
                if shader.constants.is_some() {
                    return Err(error(word, "duplicate CTAB"));
                }
                let data: Vec<_> = args[1..].iter().flat_map(|w| w.to_le_bytes()).collect();
                shader.constants = Some(crate::ctab::read(&data, word + 2)?);
            }
            shader.comments.push(Comment {
                word,
                data: args.to_vec(),
            });
            continue;
        }
        if token & 0xf0ff_0000 != 0 && !(op == 66 && token & 0xf0fc_0000 == 0) {
            return Err(error(
                word,
                format!("instruction controls/predication/coissue {token:#010x}"),
            ));
        }
        if op == 31 {
            if executable || count != 2 {
                return Err(error(word, "DCL after executable code or not two operands"));
            }
            let d = destination(args[1], word + 2, true)?;
            match d.register {
                Register::Input(register) => {
                    if args[0] & !0x800f_000f != 0 || args[0] & 0x8000_0000 == 0 {
                        return Err(error(word, "DCL semantic reserved bits"));
                    }
                    let usage = (args[0] & 15) as u8;
                    if usage > 13 {
                        return Err(error(word, format!("DCL usage {usage}")));
                    }
                    let index = ((args[0] >> 16) & 15) as u8;
                    if shader
                        .inputs
                        .iter()
                        .any(|i| i.register == register || (i.usage == usage && i.index == index))
                    {
                        return Err(error(word, "duplicate input register or semantic"));
                    }
                    shader.inputs.push(Input {
                        register,
                        usage,
                        index,
                        mask: d.mask,
                        centroid: args[1] & 0x0040_0000 != 0,
                        partial_precision: d.partial_precision,
                    });
                }
                Register::Sampler(n) => {
                    let dim = match args[0] {
                        0x9000_0000 => TextureDim::D2,
                        0x9800_0000 => TextureDim::Cube,
                        0xa000_0000 => TextureDim::D3,
                        _ => return Err(error(word, format!("DCL sampler texture type {:#x}", args[0]))),
                    };
                    if d.mask != 15
                        || d.partial_precision
                        || args[1] & 0x0040_0000 != 0
                        || shader.samplers.insert(n, dim).is_some()
                    {
                        return Err(error(word, "duplicate or modified sampler declaration"));
                    }
                }
                _ => return Err(error(word, "DCL destination is neither input nor sampler")),
            }
            continue;
        }
        if op == 81 {
            if executable || count != 5 {
                return Err(error(word, "DEF after executable code or not five operands"));
            }
            let d = destination(args[0], word + 1, false)?;
            let Register::Constant(n) = d.register else {
                return Err(error(word, "DEF destination is not cN"));
            };
            if d.mask != 15
                || d.saturate
                || d.partial_precision
                || shader
                    .definitions
                    .insert(n, [args[1], args[2], args[3], args[4]])
                    .is_some()
            {
                return Err(error(word, "duplicate or modified DEF"));
            }
            continue;
        }
        let (opcode, sources) = Opcode::decode(op, word)?;
        let expected = if opcode == Opcode::Nop { 0 } else { 1 + sources };
        if count != expected {
            return Err(error(
                word,
                format!("{opcode:?} has {count} operands, expected {expected}"),
            ));
        }
        let destination = if count == 0 {
            None
        } else {
            Some(destination(args[0], word + 1, false)?)
        };
        if let Some(d) = destination {
            let allowed = matches!(d.register, Register::Temporary(_) | Register::Target(_))
                || (opcode == Opcode::TexKill && matches!(d.register, Register::Input(_)));
            if !allowed {
                return Err(error(word, format!("{opcode:?} destination {:?}", d.register)));
            }
            if opcode == Opcode::TexKill && (d.saturate || d.partial_precision) {
                return Err(error(word, "modified TEXKILL destination"));
            }
        }
        let sources = args
            .iter()
            .skip(1)
            .enumerate()
            .map(|(i, &t)| source(t, word + 2 + i))
            .collect::<Result<_>>()?;
        shader.instructions.push(Instruction {
            word,
            opcode,
            destination,
            sources,
            control: ((token >> 16) & 3) as u8,
        });
        executable = true;
    }
    Err(error(at, "missing END"))
}
