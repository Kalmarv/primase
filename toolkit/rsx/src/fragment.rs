//! RSX fragment instructions. Words in the big-endian microcode are rotated by
//! 16 bits before field extraction. Inline constants occupy one further slot.
//! This reads instructions; it neither executes nor lowers them.
use crate::cgbinary::Error;
use primase_bytes::{Endian::Big, View};
use std::fmt::Write;

macro_rules! opcodes {
    ($($name:ident = $code:literal, $arity:literal;)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Opcode { $($name = $code,)+ }
        impl Opcode {
            fn read(code: u8) -> Option<Self> { match code { $($code => Some(Self::$name),)+ _ => None } }
            pub fn mnemonic(self) -> &'static str { match self { $(Self::$name => stringify!($name),)+ } }
            pub fn source_count(self) -> usize { match self { $(Self::$name => $arity,)+ } }
        }
    }
}

// The arithmetic/texture vocabulary follows NV_fragment_program[2]. The RSX
// encodings are described by the public RSX instruction declarations.
opcodes! {
    NOP=0x00,0; MOV=0x01,1; MUL=0x02,2; ADD=0x03,2; MAD=0x04,3;
    DP3=0x05,2; DP4=0x06,2; DST=0x07,2; MIN=0x08,2; MAX=0x09,2;
    SLT=0x0a,2; SGE=0x0b,2; SLE=0x0c,2; SGT=0x0d,2; SNE=0x0e,2; SEQ=0x0f,2;
    FRC=0x10,1; FLR=0x11,1; KIL=0x12,0; PK4=0x13,1; UP4=0x14,1;
    DDX=0x15,1; DDY=0x16,1; TEX=0x17,1; TXP=0x18,1; TXD=0x19,3;
    RCP=0x1a,1; RSQ=0x1b,1; EX2=0x1c,1; LG2=0x1d,1; LIT=0x1e,1; LRP=0x1f,3;
    STR=0x20,0; SFL=0x21,0; COS=0x22,1; SIN=0x23,1; PK2=0x24,1; UP2=0x25,1;
    POW=0x26,2; PKB=0x27,1; UPB=0x28,1; PK16=0x29,1; UP16=0x2a,1;
    PKG=0x2c,1; UPG=0x2d,1; DP2A=0x2e,3; TXL=0x2f,2; TXB=0x31,2;
    REFL=0x36,2; DP2=0x38,2; NRM=0x39,1; DIV=0x3a,2; DIVSQ=0x3b,2; LIF=0x3c,1;
    FENCT=0x3d,0; FENCB=0x3e,0; BRK=0x40,0; CAL=0x41,0;
    IFE=0x42,0; LOOP=0x43,0; REP=0x44,0; RET=0x45,0;
}

impl Opcode {
    pub fn samples_texture(self) -> bool {
        matches!(self, Self::TEX | Self::TXP | Self::TXD | Self::TXL | Self::TXB)
    }
    pub fn writes_destination(self) -> bool {
        !matches!(
            self,
            Self::NOP
                | Self::KIL
                | Self::FENCT
                | Self::FENCB
                | Self::BRK
                | Self::CAL
                | Self::IFE
                | Self::LOOP
                | Self::REP
                | Self::RET
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Register {
    pub index: u8,
    pub half: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Register(Register),
    Input(u8),
    Constant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub kind: SourceKind,
    pub swizzle: [u8; 4],
    pub negate: bool,
    pub absolute: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Predicate {
    /// Bit 0 less, bit 1 equal, bit 2 greater. Zero never executes; seven always executes.
    pub test: u8,
    pub swizzle: [u8; 4],
    pub register: u8,
}

impl Predicate {
    /// NV condition-code test, with all three outcomes enabled meaning true.
    pub fn condition(self) -> &'static str {
        match self.test {
            0 => "FL",
            1 => "LT",
            2 => "EQ",
            3 => "LE",
            4 => "GT",
            5 => "NE",
            6 => "GE",
            7 => "TR",
            _ => "invalid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    If {
        else_offset: usize,
        end_offset: usize,
    },
    Loop {
        count: u8,
        initial: u8,
        increment: u8,
        end_offset: usize,
    },
    /// No call-target interpretation or execution is claimed by this reader.
    Call {
        encoded: [u32; 3],
    },
    Break,
    Return,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub offset: usize,
    pub size: usize,
    pub opcode: Opcode,
    /// Rotated words, including fields not interpreted here.
    pub words: [u32; 4],
    pub end: bool,
    pub destination: Register,
    pub write_mask: u8,
    pub no_destination: bool,
    pub update_condition: bool,
    pub condition_destination: u8,
    pub predicate: Predicate,
    pub saturate: bool,
    pub precision: u8,
    pub expand_texture: bool,
    pub texture_unit: Option<u8>,
    pub sources: Vec<Source>,
    pub constant: Option<[u32; 4]>,
    /// Encoded modifiers for each source, and the result scale, retained without floating-point evaluation.
    pub source_precision: [u8; 3],
    pub scale: u8,
    pub indexed_input: bool,
    pub input_address: u16,
    pub perspective_input: bool,
    pub control: Option<Control>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub instructions: Vec<Instruction>,
    pub byte_len: usize,
}

fn error(offset: usize, detail: impl Into<String>) -> Error {
    Error {
        offset,
        detail: detail.into(),
    }
}
fn bits(n: u32, shift: u32, width: u32) -> u32 {
    (n >> shift) & ((1 << width) - 1)
}
fn swizzle(n: u32, shift: u32) -> [u8; 4] {
    std::array::from_fn(|i| bits(n, shift + i as u32 * 2, 2) as u8)
}

/// Decode exactly one big-endian fragment program. Requires END at the exact
/// end, validates branch boundaries and refuses reserved/unimplemented opcodes.
/// Inline constants are retained as IEEE bits, never decoded as instructions.
pub fn decode(bytes: &[u8]) -> Result<Program, Error> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(16) {
        return Err(error(0, "fragment microcode must contain complete 16-byte slots"));
    }
    let view = View::new(bytes);
    let slot = |at| -> Result<[u32; 4], Error> {
        let data = view.slice(at, 16).map_err(|e| error(at, e.to_string()))?;
        let v = View::new(data);
        Ok([
            v.u32(0, Big)
                .map_err(|e| error(at, e.to_string()))?
                .rotate_left(16),
            v.u32(4, Big)
                .map_err(|e| error(at, e.to_string()))?
                .rotate_left(16),
            v.u32(8, Big)
                .map_err(|e| error(at, e.to_string()))?
                .rotate_left(16),
            v.u32(12, Big)
                .map_err(|e| error(at, e.to_string()))?
                .rotate_left(16),
        ])
    };
    let mut at = 0;
    let mut instructions = Vec::new();
    loop {
        let words = slot(at)?;
        let [d, a, b, c] = words;
        let code = (bits(d, 24, 6) | (bits(b, 31, 1) << 6)) as u8;
        let opcode =
            Opcode::read(code).ok_or_else(|| error(at, format!("unsupported fragment opcode {code:#x}")))?;
        let mut sources = Vec::new();
        for (i, raw) in [a, b, c].into_iter().take(opcode.source_count()).enumerate() {
            let kind = match bits(raw, 0, 2) {
                0 => SourceKind::Register(Register {
                    index: bits(raw, 2, 6) as u8,
                    half: bits(raw, 8, 1) != 0,
                }),
                1 => SourceKind::Input(bits(d, 13, 4) as u8),
                2 => SourceKind::Constant,
                _ => return Err(error(at + 4 + i * 4, "reserved source register type")),
            };
            sources.push(Source {
                kind,
                swizzle: swizzle(raw, 9),
                negate: bits(raw, 17, 1) != 0,
                absolute: bits(raw, if i == 0 { 29 } else { 18 }, 1) != 0,
            });
        }
        let constant = if sources.iter().any(|s| s.kind == SourceKind::Constant) {
            Some(slot(at + 16)?)
        } else {
            None
        };
        let size = if constant.is_some() { 32 } else { 16 };
        let control = match opcode {
            Opcode::IFE => Some(Control::If {
                else_offset: usize::try_from(b & 0x7fffffff)
                    .map_err(|_| error(at, "branch address overflow"))?
                    .checked_mul(4)
                    .ok_or_else(|| error(at, "branch address overflow"))?,
                end_offset: usize::try_from(c)
                    .map_err(|_| error(at, "branch address overflow"))?
                    .checked_mul(4)
                    .ok_or_else(|| error(at, "branch address overflow"))?,
            }),
            Opcode::LOOP | Opcode::REP => Some(Control::Loop {
                count: bits(b, 2, 8) as u8,
                initial: bits(b, 10, 8) as u8,
                increment: bits(b, 19, 8) as u8,
                end_offset: usize::try_from(c)
                    .map_err(|_| error(at, "loop address overflow"))?
                    .checked_mul(4)
                    .ok_or_else(|| error(at, "loop address overflow"))?,
            }),
            Opcode::CAL => Some(Control::Call { encoded: [a, b, c] }),
            Opcode::BRK => Some(Control::Break),
            Opcode::RET => Some(Control::Return),
            _ => None,
        };
        let end = d & 1 != 0;
        instructions.push(Instruction {
            offset: at,
            size,
            opcode,
            words,
            end,
            destination: Register {
                index: bits(d, 1, 6) as u8,
                half: bits(d, 7, 1) != 0,
            },
            write_mask: bits(d, 9, 4) as u8,
            no_destination: bits(d, 30, 1) != 0,
            update_condition: bits(d, 8, 1) != 0,
            condition_destination: bits(a, 30, 1) as u8,
            predicate: Predicate {
                test: bits(a, 18, 3) as u8,
                swizzle: swizzle(a, 21),
                register: bits(a, 31, 1) as u8,
            },
            saturate: bits(d, 31, 1) != 0,
            precision: bits(d, 22, 2) as u8,
            expand_texture: bits(d, 21, 1) != 0,
            texture_unit: opcode.samples_texture().then_some(bits(d, 17, 4) as u8),
            sources,
            constant,
            source_precision: [bits(b, 19, 3) as u8, bits(b, 22, 3) as u8, bits(b, 25, 3) as u8],
            scale: bits(b, 28, 3) as u8,
            indexed_input: bits(c, 30, 1) != 0,
            input_address: bits(c, 19, 11) as u16,
            perspective_input: bits(c, 31, 1) != 0,
            control,
        });
        at += size;
        if end {
            if at != bytes.len() {
                return Err(error(at, "bytes after END instruction"));
            }
            break;
        }
        if at == bytes.len() {
            return Err(error(at, "fragment program has no END instruction"));
        }
    }
    let boundary = |offset| offset == bytes.len() || instructions.iter().any(|i| i.offset == offset);
    for i in &instructions {
        match i.control {
            Some(Control::If {
                else_offset,
                end_offset,
            }) if !boundary(else_offset)
                || !boundary(end_offset)
                || else_offset < i.offset + i.size
                || end_offset < else_offset =>
            {
                return Err(error(i.offset, "invalid IF boundaries"));
            }
            Some(Control::Loop { end_offset, .. })
                if !boundary(end_offset) || end_offset < i.offset + i.size =>
            {
                return Err(error(i.offset, "invalid loop boundary"));
            }
            _ => {}
        }
    }
    Ok(Program {
        instructions,
        byte_len: bytes.len(),
    })
}

fn register(r: Register) -> String {
    format!("{}{}", if r.half { "h" } else { "r" }, r.index)
}
fn channels(s: [u8; 4]) -> String {
    s.into_iter()
        .map(|n| char::from(b"xyzw"[usize::from(n)]))
        .collect()
}

impl Program {
    /// Stable inspection text. Full predicates and modifiers remain in the typed
    /// instruction even where an external assembler dialect omits them.
    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        for i in &self.instructions {
            let _ = write!(out, "{:04x}: {}", i.offset, i.opcode.mnemonic());
            if i.opcode.writes_destination() {
                let mask: String = (0..4)
                    .filter(|n| i.write_mask & (1 << n) != 0)
                    .map(|n| char::from(b"xyzw"[n]))
                    .collect();
                let _ = write!(
                    out,
                    " {}.{mask}",
                    if i.no_destination || i.destination.index == 63 {
                        format!("cc{}", i.condition_destination)
                    } else {
                        register(i.destination)
                    }
                );
            }
            for s in &i.sources {
                let source = match s.kind {
                    SourceKind::Register(r) => register(r),
                    SourceKind::Input(n) => format!("input[{n}]"),
                    SourceKind::Constant => format!("{:08x?}", i.constant),
                };
                let _ = write!(
                    out,
                    ", {}{}{}.{}",
                    if s.negate { "-" } else { "" },
                    if s.absolute { "abs " } else { "" },
                    source,
                    channels(s.swizzle)
                );
            }
            if let Some(unit) = i.texture_unit {
                let _ = write!(out, ", tex{unit}");
            }
            if let Some(control) = &i.control {
                let _ = write!(out, " {control:?}");
            }
            let _ = writeln!(
                out,
                " ; pred={} cc{}.{}, sat={}, precision={}, scale={}, src_precision={:?}, end={}",
                i.predicate.test,
                i.predicate.register,
                channels(i.predicate.swizzle),
                i.saturate,
                i.precision,
                i.scale,
                i.source_precision,
                i.end
            );
        }
        out
    }
}

pub mod flow;
