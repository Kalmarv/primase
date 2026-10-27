//! Lua 5.1 precompiled chunks (`luac` output), read strictly in either byte order.
//!
//! A chunk is a 12-byte header (`ESC Lua`, version `0x51`, format 0, the byte-order flag, the sizes of `int`,
//! `size_t`, an instruction and a number, and the integral-number flag) and one function: its name, lines, upvalue and
//! parameter counts, code, constants, nested functions and (unless stripped) debug tables. [`Chunk::parse`] reads
//! exactly that. What `luac` never writes, and a loader for untrusted bytes must not take, is an error naming the
//! byte: a header of any other version or format, sizes other than 4 or 8 for `int` and `size_t`, an instruction other
//! than 4 bytes, a number other than an 8-byte float, a count the rest of the chunk cannot hold, a string without its
//! terminating NUL, a boolean other than 0 or 1, an unknown constant type, a chain of more than 200 functions inside one another, a byte after the
//! main function, and code the verifier ([`Proto`] checks a subset of `luaG_checkcode`) rejects: an opcode that does
//! not exist, a register past the function's stack, a constant, function or upvalue past its table, a jump out of the
//! code, a test not followed by a jump, a `CLOSURE` not followed by its upvalue instructions, and a function that does
//! not end in `RETURN`.
//!
//! [`calls`] lists the calls a function makes whose callee and arguments are constants, globals and fields of
//! them, which is how a script that only configures an engine reads.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

mod calls;
mod instruction;

pub use calls::{Call, Value, calls};
pub use instruction::{Instruction, Layout, Op, Operand};
use primase_foundations::bytes::{BoundsError, Cursor, Endian};

/// What is wrong with a chunk, and where (a byte offset in it).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Lua 5.1 chunk at byte {offset}: {detail}")]
pub struct Error {
    pub offset: usize,
    pub detail: String,
}

fn bounds(e: BoundsError) -> Error {
    Error {
        offset: usize::try_from(e.absolute()).unwrap_or(usize::MAX),
        detail: e.to_string(),
    }
}

/// The longest chain of functions inside one another, the main function included, that is read (a bound of this
/// reader, in the spirit of the parser's `LUAI_MAXCCALLS`).
pub const MAX_DEPTH: usize = 200;

/// The deepest stack a function may declare (`MAXSTACK`).
const MAX_STACK: u8 = 250;

/// The header's facts a reader needs: how numbers are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub endian: Endian,
    /// Bytes of `int` (4 or 8).
    pub int_size: u8,
    /// Bytes of `size_t` (4 or 8).
    pub size_t_size: u8,
}

/// A constant of a function.
#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Nil,
    Bool(bool),
    Number(f64),
    /// Bytes, as stored without the terminating NUL (a Lua string need not be text).
    Str(Vec<u8>),
}

/// A local variable's debug record: its name and the instructions it lives over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: Vec<u8>,
    pub start_pc: u32,
    pub end_pc: u32,
}

/// A function as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Proto {
    /// The source name (the main function of a stripped chunk has none).
    pub source: Option<Vec<u8>>,
    pub line_defined: u32,
    pub last_line_defined: u32,
    pub upvalues: u8,
    pub params: u8,
    /// `is_vararg`: bit 0 `HASARG`, bit 1 `ISVARARG`, bit 2 `NEEDSARG`.
    pub vararg: u8,
    pub max_stack: u8,
    pub code: Vec<Instruction>,
    pub constants: Vec<Constant>,
    pub protos: Vec<Proto>,
    /// Source line of each instruction; empty when stripped.
    pub lines: Vec<u32>,
    pub locals: Vec<Local>,
    pub upvalue_names: Vec<Vec<u8>>,
}

/// A whole chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub header: Header,
    pub main: Proto,
}

struct Reader<'a> {
    c: Cursor<'a>,
    header: Header,
}

impl Reader<'_> {
    fn err(&self, at: usize, detail: impl Into<String>) -> Error {
        Error {
            offset: at,
            detail: detail.into(),
        }
    }

    /// An unsigned number of `size` bytes (4 or 8, as the header was checked to say).
    fn unsigned(&mut self, size: u8) -> Result<u64, Error> {
        let e = self.header.endian;
        match size {
            4 => self.c.u32(e).map(u64::from).map_err(bounds),
            _ => self.c.u64(e).map_err(bounds),
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        self.c.u8().map_err(bounds)
    }

    /// An `int` that fits 32 bits (a line, a pc).
    fn int(&mut self) -> Result<u32, Error> {
        let at = self.c.position();
        let v = self.unsigned(self.header.int_size)?;
        u32::try_from(v).map_err(|_| self.err(at, format!("an int of {v} past 32 bits")))
    }

    /// A count of items of at least `each` bytes that the rest of the chunk can hold.
    fn count(&mut self, each: usize) -> Result<usize, Error> {
        let at = self.c.position();
        let v = self.unsigned(self.header.int_size)?;
        let n = usize::try_from(v).map_err(|_| self.err(at, format!("a count of {v}")))?;
        let need = n.checked_mul(each);
        match need {
            Some(need) if need <= self.c.remaining() => Ok(n),
            _ => Err(self.err(
                at,
                format!("a count of {n} items but {} bytes remain", self.c.remaining()),
            )),
        }
    }

    /// A string: its size with the NUL (0: none), then its bytes and that NUL.
    fn string(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let at = self.c.position();
        let v = self.unsigned(self.header.size_t_size)?;
        if v == 0 {
            return Ok(None);
        }
        let n = usize::try_from(v).map_err(|_| self.err(at, format!("a string of {v} bytes")))?;
        let bytes = self.c.take(n).map_err(bounds)?;
        match bytes.split_last() {
            Some((0, body)) => Ok(Some(body.to_vec())),
            _ => Err(self.err(at, "a string without its terminating NUL")),
        }
    }

    fn proto(&mut self, depth: usize) -> Result<Proto, Error> {
        let start = self.c.position();
        if depth >= MAX_DEPTH {
            return Err(self.err(start, format!("functions nested past {MAX_DEPTH}")));
        }
        let source = self.string()?;
        let line_defined = self.int()?;
        let last_line_defined = self.int()?;
        let upvalues = self.byte()?;
        let params = self.byte()?;
        let at = self.c.position();
        let vararg = self.byte()?;
        if vararg > 7 {
            return Err(self.err(
                at,
                format!("is_vararg {vararg} has bits past the three Lua 5.1 defines"),
            ));
        }
        let at = self.c.position();
        let max_stack = self.byte()?;
        if max_stack > MAX_STACK {
            return Err(self.err(
                at,
                format!("a stack of {max_stack} registers (at most {MAX_STACK})"),
            ));
        }

        let n = self.count(4)?;
        let code_at = self.c.position();
        let mut words = Vec::with_capacity(n);
        for _ in 0..n {
            words.push(self.c.u32(self.header.endian).map_err(bounds)?);
        }

        let n = self.count(1)?;
        let mut constants = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.c.position();
            constants.push(match self.byte()? {
                0 => Constant::Nil,
                1 => {
                    let at = self.c.position();
                    match self.byte()? {
                        0 => Constant::Bool(false),
                        1 => Constant::Bool(true),
                        b => return Err(self.err(at, format!("a boolean constant of {b}"))),
                    }
                }
                3 => Constant::Number(f64::from_bits(self.unsigned(8)?)),
                4 => {
                    let at = self.c.position();
                    Constant::Str(
                        self.string()?
                            .ok_or_else(|| self.err(at, "a string constant with no string"))?,
                    )
                }
                t => return Err(self.err(at, format!("a constant of type {t}"))),
            });
        }

        let n = self.count(1)?;
        let mut protos = Vec::with_capacity(n);
        for _ in 0..n {
            protos.push(self.proto(depth.saturating_add(1))?);
        }

        let at = self.c.position();
        let n = self.count(usize::from(self.header.int_size))?;
        if n != 0 && n != words.len() {
            return Err(self.err(at, format!("{n} line numbers for {} instructions", words.len())));
        }
        let mut lines = Vec::with_capacity(n);
        for _ in 0..n {
            lines.push(self.int()?);
        }

        let n = self.count(usize::from(self.header.size_t_size))?;
        let mut locals = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.c.position();
            let name = self
                .string()?
                .ok_or_else(|| self.err(at, "a local variable with no name"))?;
            let start_pc = self.int()?;
            let end_pc = self.int()?;
            if start_pc > end_pc || usize::try_from(end_pc).map_or(true, |e| e > words.len()) {
                return Err(self.err(
                    at,
                    format!(
                        "local {start_pc}..{end_pc} outside the {} instructions",
                        words.len()
                    ),
                ));
            }
            locals.push(Local {
                name,
                start_pc,
                end_pc,
            });
        }

        let at = self.c.position();
        let n = self.count(usize::from(self.header.size_t_size))?;
        if n != 0 && n != usize::from(upvalues) {
            return Err(self.err(at, format!("{n} upvalue names for {upvalues} upvalues")));
        }
        let mut upvalue_names = Vec::with_capacity(n);
        for _ in 0..n {
            let at = self.c.position();
            upvalue_names.push(
                self.string()?
                    .ok_or_else(|| self.err(at, "an upvalue with no name"))?,
            );
        }

        let mut code = Vec::with_capacity(words.len());
        for (i, &w) in words.iter().enumerate() {
            let at = code_at.saturating_add(i.saturating_mul(4));
            code.push(
                Instruction::decode(w).map_err(|op| self.err(at, format!("opcode {op} does not exist")))?,
            );
        }
        let proto = Proto {
            source,
            line_defined,
            last_line_defined,
            upvalues,
            params,
            vararg,
            max_stack,
            code,
            constants,
            protos,
            lines,
            locals,
            upvalue_names,
        };
        proto.check_code().map_err(|(pc, detail)| {
            self.err(
                code_at.saturating_add(pc.saturating_mul(4)),
                format!("instruction {pc}: {detail}"),
            )
        })?;
        Ok(proto)
    }
}

impl Chunk {
    /// Read a chunk, all of `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Chunk, Error> {
        let mut c = Cursor::new(bytes);
        let at = c.position();
        let fail = |at: usize, detail: &str| Error {
            offset: at,
            detail: detail.to_string(),
        };
        if c.array::<4>().map_err(bounds)? != *b"\x1bLua" {
            return Err(fail(at, "not a precompiled Lua chunk (missing ESC Lua)"));
        }
        let version = c.u8().map_err(bounds)?;
        if version != 0x51 {
            return Err(fail(
                at.saturating_add(4),
                "not Lua 5.1 (version byte is not 0x51)",
            ));
        }
        let format = c.u8().map_err(bounds)?;
        if format != 0 {
            return Err(fail(at.saturating_add(5), "a format other than the official 0"));
        }
        let endian = match c.u8().map_err(bounds)? {
            0 => Endian::Big,
            1 => Endian::Little,
            _ => return Err(fail(at.saturating_add(6), "a byte-order flag other than 0 or 1")),
        };
        let int_size = c.u8().map_err(bounds)?;
        let size_t_size = c.u8().map_err(bounds)?;
        let instruction_size = c.u8().map_err(bounds)?;
        let number_size = c.u8().map_err(bounds)?;
        let integral = c.u8().map_err(bounds)?;
        if !matches!(int_size, 4 | 8) {
            return Err(fail(at.saturating_add(7), "an int of other than 4 or 8 bytes"));
        }
        if !matches!(size_t_size, 4 | 8) {
            return Err(fail(at.saturating_add(8), "a size_t of other than 4 or 8 bytes"));
        }
        if instruction_size != 4 {
            return Err(fail(at.saturating_add(9), "an instruction of other than 4 bytes"));
        }
        if number_size != 8 || integral != 0 {
            return Err(fail(at.saturating_add(10), "numbers other than 8-byte floats"));
        }
        let header = Header {
            endian,
            int_size,
            size_t_size,
        };
        let mut r = Reader { c, header };
        let main = r.proto(0)?;
        if r.c.remaining() != 0 {
            return Err(r.err(
                r.c.position(),
                format!("{} bytes after the main function", r.c.remaining()),
            ));
        }
        Ok(Chunk { header, main })
    }
}

impl Proto {
    /// The verifier: a subset of `luaG_checkcode` that needs no symbolic execution. `Err` is the first bad
    /// instruction's index and why.
    fn check_code(&self) -> Result<(), (usize, String)> {
        let len = self.code.len();
        if !matches!(self.code.last(), Some(i) if i.op == Op::Return) {
            return Err((
                len.saturating_sub(1),
                "the function does not end in RETURN".into(),
            ));
        }
        let stack = usize::from(self.max_stack);
        let reg = |x: usize, what: &str| {
            if x < stack {
                Ok(())
            } else {
                Err(format!("{what} register {x} past the stack of {stack}"))
            }
        };
        let konst = |x: usize| {
            if x < self.constants.len() {
                Ok(())
            } else {
                Err(format!("constant {x} of {}", self.constants.len()))
            }
        };
        let operand = |kind: Operand, x: u16, what: &str| match kind {
            Operand::Register => reg(usize::from(x), what),
            Operand::RegisterOrConstant => match usize::from(x).checked_sub(256) {
                Some(k) => konst(k),
                None => reg(usize::from(x), what),
            },
            Operand::Unused | Operand::Number => Ok(()),
        };
        for (pc, i) in self.code.iter().enumerate() {
            let check = || -> Result<(), String> {
                if i.op.sets_a() {
                    reg(usize::from(i.a), "A")?;
                }
                match i.op.layout() {
                    Layout::Abc => {
                        operand(i.op.b_operand(), i.b, "B")?;
                        operand(i.op.c_operand(), i.c, "C")?;
                    }
                    Layout::Abx => {
                        let bx = usize::try_from(i.bx()).unwrap_or(usize::MAX);
                        if i.op == Op::Closure {
                            if bx >= self.protos.len() {
                                return Err(format!("function {bx} of {}", self.protos.len()));
                            }
                        } else {
                            konst(bx)?;
                        }
                    }
                    Layout::AsBx => {
                        let target = i64::try_from(pc)
                            .unwrap_or(i64::MAX)
                            .saturating_add(1)
                            .saturating_add(i.sbx());
                        if target < 0 || usize::try_from(target).map_or(true, |t| t >= len) {
                            return Err(format!("a jump to {target} out of the {len} instructions"));
                        }
                    }
                }
                let next = |n: usize| self.code.get(pc.saturating_add(n));
                match i.op {
                    Op::GetUpval | Op::SetUpval if i.b >= u16::from(self.upvalues) => {
                        return Err(format!("upvalue {} of {}", i.b, self.upvalues));
                    }
                    Op::LoadBool if i.c != 0 && next(2).is_none() => {
                        return Err("a LOADBOOL that skips past the code".into());
                    }
                    op if op.tests() && !matches!(next(1), Some(j) if j.op == Op::Jmp) => {
                        return Err("a test not followed by a jump".into());
                    }
                    Op::Closure => {
                        let child = self.protos.get(usize::try_from(i.bx()).unwrap_or(usize::MAX));
                        let ups = child.map_or(0, |p| usize::from(p.upvalues));
                        for k in 1..=ups {
                            if !matches!(next(k), Some(j) if matches!(j.op, Op::Move | Op::GetUpval)) {
                                return Err("a CLOSURE not followed by its upvalue instructions".into());
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            };
            check().map_err(|d| (pc, d))?;
        }
        Ok(())
    }
}
