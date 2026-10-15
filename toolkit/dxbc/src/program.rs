use crate::{Result, chunks, error, opcodes::NAMES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Register {
    Temp,
    Input,
    Output,
    Constant,
    Immediate,
    Texture,
    Sampler,
    Null,
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
    pub modifier: u8,
}
#[derive(Clone, Debug)]
pub struct Index {
    pub base: u32,
    pub relative: Option<Box<Operand>>,
}
#[derive(Clone, Debug)]
pub struct Instruction {
    pub word: usize,
    pub opcode: u16,
    pub saturate: bool,
    pub operands: Vec<Operand>,
}
impl Instruction {
    pub fn name(&self) -> &'static str {
        NAMES[self.opcode as usize]
    }
}
#[derive(Clone, Debug)]
pub struct Program {
    pub(crate) code: Vec<Instruction>,
    pub(crate) temps: usize,
    pub(crate) inputs: usize,
    pub(crate) outputs: usize,
    pub(crate) constant_buffers: Vec<(u32, u32)>,
    pub(crate) textures: Vec<u32>,
    pub(crate) samplers: Vec<u32>,
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
        let register = match (t >> 12) & 255 {
            0 => Register::Temp,
            1 => Register::Input,
            2 => Register::Output,
            4 => Register::Immediate,
            6 => Register::Sampler,
            7 => Register::Texture,
            8 => Register::Constant,
            13 => Register::Null,
            n => return Err(error(pos, format!("unsupported operand register type {n}"))),
        };
        let mut modifier = 0;
        if t >> 31 != 0 {
            let ext = self.take()?;
            // Only the full-precision source modifier extension is supported.
            if ext & !0xff != 0 || ext & 63 != 1 {
                return Err(error(pos, "unsupported extended operand (precision/modifier)"));
            }
            modifier = ((ext >> 6) & 3) as u8;
        }
        let dimensions = (t >> 20) & 3;
        let expected = match register {
            Register::Immediate | Register::Null => 0,
            Register::Constant => 2,
            _ => 1,
        };
        if dimensions != expected {
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
                let op = self.operand(depth + 1)?;
                if !matches!(op.register, Register::Temp | Register::Input)
                    || !matches!(op.selection, Selection::Scalar(_))
                    || op.modifier != 0
                    || op.indices.iter().any(|i| i.relative.is_some())
                {
                    return Err(error(
                        pos,
                        "relative index must select a direct temp/input component",
                    ));
                }
                Some(Box::new(op))
            } else {
                None
            };
            indices.push(Index { base, relative });
        }
        // This bounded subset only permits dynamic addressing within a cbuffer.
        if indices
            .iter()
            .enumerate()
            .any(|(d, i)| i.relative.is_some() && !(register == Register::Constant && d == 1))
        {
            return Err(error(
                pos,
                "dynamic register addressing outside a constant buffer",
            ));
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
        })
    }
}
impl Program {
    pub fn temp_count(&self) -> usize {
        self.temps
    }
    pub fn input_count(&self) -> usize {
        self.inputs
    }
    pub fn output_count(&self) -> usize {
        self.outputs
    }
    pub fn constant_buffers(&self) -> &[(u32, u32)] {
        &self.constant_buffers
    }
    pub fn textures(&self) -> &[u32] {
        &self.textures
    }
    pub fn samplers(&self) -> &[u32] {
        &self.samplers
    }
    pub fn instructions(&self) -> &[Instruction] {
        &self.code
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let directory = chunks(bytes)?;
        let code = directory
            .iter()
            .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
            .ok_or_else(|| error(0, "missing shader code chunk"))?;
        if code.data.len() % 4 != 0 {
            return Err(error(0, "unaligned code length"));
        }
        let words: Vec<_> = code
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        Self::from_words(&words)
    }
    /// Compile a complete ps_5_0 token stream before permitting evaluation.
    pub fn from_words(words: &[u32]) -> Result<Self> {
        if words.first() != Some(&0x50) || words.get(1).copied() != Some(words.len() as u32) {
            return Err(error(0, "expected complete ps_5_0 program"));
        }
        if words.len() > 1_048_576 {
            return Err(error(1, "program exceeds word budget"));
        }
        let mut p = Self {
            code: Vec::new(),
            temps: 0,
            inputs: 0,
            outputs: 0,
            constant_buffers: Vec::new(),
            textures: Vec::new(),
            samplers: Vec::new(),
        };
        let mut at = 2;
        let mut ended = false;
        let mut execution = false;
        while at < words.len() {
            let t = words[at];
            let opcode = (t & 0x7ff) as u16;
            let name = NAMES.get(opcode as usize).copied().unwrap_or("unknown opcode");
            let count = match opcode {
                0 | 1 | 14..=17 | 51 | 56 | 57 => 3,
                25 | 28 | 47 | 54 | 64 | 68 | 75 | 86 => 2,
                35 | 38 | 50 | 55 | 69 => 4,
                62 | 88..=90 | 98..=100 | 101 | 104 | 106 => 0,
                _ => return Err(error(at, format!("unsupported {name} ({opcode})"))),
            };
            if ended {
                return Err(error(at, "instructions after final ret"));
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
            let mut extended = t >> 31 != 0;
            let mut seen = 0;
            while extended {
                let ext = c.take()?;
                let kind = ext & 63;
                if opcode != 69 || kind > 3 || kind == 0 || seen & (1 << kind) != 0 {
                    return Err(error(at, "unsupported or repeated opcode extension"));
                }
                seen |= 1 << kind;
                let payload = ext & 0x7fff_ffff;
                // sample offsets zero; texture2d dimension; float4 return type.
                let valid = match kind {
                    1 => payload == 1,
                    2 => payload == (2 | 3 << 6),
                    3 => payload == (3 | 0x5555 << 6),
                    _ => false,
                };
                if !valid {
                    return Err(error(at, "unsupported sample offset/dimension/return type"));
                }
                extended = ext >> 31 != 0;
            }
            if opcode >= 88 {
                if execution {
                    return Err(error(at, "declaration after executable instruction"));
                }
                p.declaration(opcode, t, &mut c)?;
            } else {
                execution = true;
                if t & 0x00ff_d800 != 0 {
                    return Err(error(at, "unsupported instruction controls"));
                }
                let mut operands = Vec::new();
                for _ in 0..count {
                    operands.push(c.operand(0)?);
                }
                let ins = Instruction {
                    word: at,
                    opcode,
                    saturate: t & 0x2000 != 0,
                    operands,
                };
                p.validate(&ins)?;
                ended = opcode == 62;
                p.code.push(ins);
            }
            if c.at != c.words.len() {
                return Err(error(at, format!("unconsumed {name} tokens")));
            }
            at += len;
        }
        if !ended {
            return Err(error(at, "missing final ret"));
        }
        Ok(p)
    }
    fn declaration(&mut self, opcode: u16, token: u32, c: &mut Cursor<'_>) -> Result<()> {
        let pos = c.origin - 1;
        let controls = match opcode {
            88 => 0xf800,
            89 | 106 => 0x800,
            98..=100 => 0x7800,
            _ => 0,
        };
        if token & 0x00ff_f800 & !controls != 0 {
            return Err(error(pos, "unsupported declaration controls"));
        }
        match opcode {
            104 => {
                self.temps = c.take()? as usize;
                if self.temps > 4096 {
                    return Err(error(pos, "temp limit"));
                }
            }
            106 => {
                if token & 0x00ff_f800 & !0x800 != 0 {
                    return Err(error(pos, "unsupported global flags"));
                }
            }
            _ => {
                let o = c.operand(0)?;
                if o.modifier != 0 || o.indices.iter().any(|i| i.relative.is_some()) {
                    return Err(error(pos, "indirect/modified declaration"));
                }
                let i = o
                    .indices
                    .first()
                    .ok_or_else(|| error(pos, "missing declaration index"))?
                    .base;
                match opcode {
                    88 if o.register == Register::Texture => {
                        if (token >> 11) & 31 != 3 || c.take()? != 0x5555 {
                            return Err(error(pos, "only float4 texture2d resources supported"));
                        }
                        if i >= 128 || self.textures.contains(&i) {
                            return Err(error(pos, "invalid/duplicate texture"));
                        }
                        self.textures.push(i);
                    }
                    89 if o.register == Register::Constant => {
                        let size = o.indices[1].base;
                        if i >= 14 || size > 4096 || self.constant_buffers.iter().any(|&(n, _)| n == i) {
                            return Err(error(pos, "invalid/duplicate cbuffer"));
                        }
                        self.constant_buffers.push((i, size));
                    }
                    90 if o.register == Register::Sampler => {
                        if (token >> 11) & 15 != 0 || i >= 16 || self.samplers.contains(&i) {
                            return Err(error(pos, "unsupported/duplicate sampler"));
                        }
                        self.samplers.push(i);
                    }
                    98..=100 if o.register == Register::Input => {
                        if i >= 32 {
                            return Err(error(pos, "input limit"));
                        }
                        self.inputs = self.inputs.max(i as usize + 1);
                        if opcode != 98 {
                            c.take()?;
                        }
                    }
                    101 if o.register == Register::Output => {
                        if i >= 8 {
                            return Err(error(pos, "output limit"));
                        }
                        self.outputs = self.outputs.max(i as usize + 1);
                    }
                    _ => return Err(error(pos, "declaration register mismatch")),
                }
            }
        }
        Ok(())
    }
    fn validate(&self, ins: &Instruction) -> Result<()> {
        let fail = |detail| error(ins.word, detail);
        let dests = if ins.opcode == 38 {
            2
        } else if ins.opcode == 62 {
            0
        } else {
            1
        };
        if ins.saturate && matches!(ins.opcode, 1 | 28 | 35 | 38 | 57 | 62) {
            return Err(fail("saturation on non-float result"));
        }
        for (n, o) in ins.operands.iter().enumerate() {
            if o.register == Register::Null && !(ins.opcode == 38 && n < 2) {
                return Err(fail("null is supported only as an imul destination"));
            }
            if matches!(ins.opcode, 1 | 35 | 38 | 86) && o.modifier != 0 {
                return Err(fail("unsupported integer source modifier"));
            }
            if n < dests {
                if o.modifier != 0
                    || !matches!(o.register, Register::Temp | Register::Output | Register::Null)
                    || (o.register != Register::Null && !matches!(o.selection, Selection::Mask(1..=15)))
                {
                    return Err(fail("invalid destination"));
                }
            } else if matches!(o.register, Register::Null | Register::Output) {
                return Err(fail("invalid source"));
            }
            if n >= dests
                && !matches!(o.register, Register::Texture | Register::Sampler)
                && !matches!(o.selection, Selection::Swizzle(_) | Selection::Scalar(_))
                && !(o.register == Register::Immediate && matches!(o.selection, Selection::Mask(0)))
            {
                return Err(fail("numeric source needs swizzle or scalar selection"));
            }
            if matches!(o.register, Register::Texture | Register::Sampler) && o.modifier != 0 {
                return Err(fail("resource operand modifier"));
            }
            if ins.opcode == 69 && n == 2 && !matches!(o.selection, Selection::Swizzle(_)) {
                return Err(fail("resource requires return swizzle"));
            }
            if ins.opcode == 69
                && ((n == 2 && o.register != Register::Texture)
                    || (n == 3 && o.register != Register::Sampler))
            {
                return Err(fail("sample requires texture and sampler operands"));
            }
            if matches!(o.register, Register::Texture | Register::Sampler) && !(ins.opcode == 69 && n >= 2) {
                return Err(fail("resource used as numeric operand"));
            }
            self.validate_operand(o, ins.word)?;
        }
        Ok(())
    }
    fn validate_operand(&self, o: &Operand, word: usize) -> Result<()> {
        let i = o.indices.first().map_or(0, |i| i.base);
        let valid = match o.register {
            Register::Temp => (i as usize) < self.temps,
            Register::Input => (i as usize) < self.inputs,
            Register::Output => (i as usize) < self.outputs,
            Register::Texture => self.textures.contains(&i),
            Register::Sampler => self.samplers.contains(&i),
            Register::Constant => self
                .constant_buffers
                .iter()
                .any(|&(n, size)| n == i && (o.indices[1].relative.is_some() || o.indices[1].base < size)),
            _ => true,
        };
        if !valid {
            return Err(error(word, "undeclared or out-of-range operand"));
        }
        for i in &o.indices {
            if let Some(r) = &i.relative {
                self.validate_operand(r, word)?;
            }
        }
        Ok(())
    }
}
