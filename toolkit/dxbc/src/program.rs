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
    /// An unordered-access view (`u<n>`).
    Uav,
    /// The immediate constant buffer (`icb[n]`), its rows the program's own.
    ImmediateBuffer,
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
    /// instruction's negate).
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
    /// A conditional (`if`, `breakc`, `continuec`) taken when its operand is nonzero (`_nz`), else when zero (`_z`).
    pub nonzero: bool,
    pub operands: Vec<Operand>,
}
impl Instruction {
    pub fn name(&self) -> &'static str {
        NAMES[self.opcode as usize]
    }
}
/// A declared resource's shape: what its loads and samples address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    /// A typed buffer: `ld` addresses its elements by x alone.
    Buffer,
    Texture2d,
    Texture2dArray,
    Texture3d,
    /// A cube (or an array of cubes): declared; sampling one is refused at run time.
    TextureCube,
    TextureCubeArray,
    /// A structured buffer of elements this many bytes long.
    Structured(u32),
}

/// What a declared texture's loads return in each lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnType {
    Float,
    Uint,
    Sint,
    /// A structured buffer's words, as stored.
    Mixed,
}

/// A declared `t<slot>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDeclaration {
    pub slot: u32,
    pub dimension: Dimension,
    pub returns: ReturnType,
}

impl Dimension {
    /// Its code in an instruction's resource-dimension extension (D3D's resource dimension).
    fn code(self) -> u32 {
        match self {
            Dimension::Buffer => 1,
            Dimension::Texture2d => 3,
            Dimension::Texture3d => 5,
            Dimension::TextureCube => 6,
            Dimension::Texture2dArray => 8,
            Dimension::TextureCubeArray => 10,
            Dimension::Structured(_) => 12,
        }
    }
}

impl ReturnType {
    /// A return-type token's four lanes, each this type.
    fn of(token: u32) -> Option<ReturnType> {
        match token {
            0x5555 => Some(ReturnType::Float),
            0x4444 => Some(ReturnType::Uint),
            0x3333 => Some(ReturnType::Sint),
            0x6666 => Some(ReturnType::Mixed),
            _ => None,
        }
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
    /// 3D texture slots: declared, refused when a sample reaches one (loaded by `ld`).
    pub(crate) volumes: Vec<u32>,
    /// Every declared `t<n>`, its shape and return type.
    pub(crate) resources: Vec<ResourceDeclaration>,
    /// Every declared unordered-access view `u<n>` (typed, or structured with its stride), its shape and return type.
    pub(crate) views: Vec<ResourceDeclaration>,
    /// The immediate constant buffer's rows (`dcl_immediateConstantBuffer`), empty when it declares none.
    pub(crate) immediates: Vec<[u32; 4]>,
    pub(crate) samplers: Vec<u32>,
    /// Per instruction, its structured flow target: `if` its `else` or `endif`; `else` its `endif`; `loop` its
    /// `endloop` and `endloop` its `loop`; `break(c)` and `continue(c)` their innermost `loop`. `usize::MAX` otherwise.
    pub(crate) jumps: Vec<usize>,
}

/// Instructions without destination operands: flow control and `ret`.
fn flow(opcode: u16) -> bool {
    matches!(opcode, 2 | 3 | 7 | 8 | 13 | 18 | 21 | 22 | 31 | 48 | 62)
}

/// Instructions whose result is not a float (integer, bitwise, comparison masks, conversions to integers) or that
/// have none: saturation is invalid on them.
fn non_float(opcode: u16) -> bool {
    flow(opcode)
        || matches!(
            opcode,
            1 | 24 | 27..=30 | 32..=42 | 45 | 49 | 57 | 59 | 60 | 78..=80 | 83..=85 | 87 | 138 | 140 | 167 | 180
        )
}

/// Instructions reading integer sources (and the flow conditions, which test bits): source modifiers are refused.
fn integer_sources(opcode: u16) -> bool {
    matches!(opcode, 1 | 3 | 8 | 13 | 30..=43 | 45 | 59 | 60 | 78..=80 | 83..=87 | 138 | 140 | 167 | 180)
}

/// The operand an instruction names its unordered-access view by: a store's first, an atomic's second (after its
/// result).
pub(crate) fn view_operand(opcode: u16) -> Option<usize> {
    match opcode {
        164 | 168 => Some(0),
        180 => Some(1),
        _ => None,
    }
}

/// The lanes of an address into a view of this shape: a structured view's element and byte offset, a 2D texture's x
/// and y, an array's or a 3D texture's three.
pub(crate) fn view_address(dimension: Dimension) -> u8 {
    match dimension {
        Dimension::Structured(_) | Dimension::Texture2d => 0b011,
        _ => 0b111,
    }
}

/// The operand an instruction names its resource by: the samples' and `ld`'s third, `ld_structured`'s fourth.
pub(crate) fn resource_operand(opcode: u16) -> Option<usize> {
    match opcode {
        45 | 69 | 72 | 73 | 74 => Some(2),
        167 => Some(3),
        _ => None,
    }
}

/// The screen-derivative instructions (`deriv_rtx`, `deriv_rty` and their SM5 coarse and fine forms).
pub(crate) fn derivative(opcode: u16) -> bool {
    matches!(opcode, 11 | 12 | 122..=125)
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
            9 => Register::ImmediateBuffer,
            13 => Register::Null,
            30 => Register::Uav,
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
            Register::ImmediateBuffer => 1,
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
        // This bounded subset only permits dynamic addressing within a cbuffer or the immediate constant buffer.
        if indices.iter().enumerate().any(|(d, i)| {
            i.relative.is_some()
                && !((register == Register::Constant && d == 1) || register == Register::ImmediateBuffer)
        }) {
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
    /// 3D texture slots the program declares; sampling one at run time is an error.
    pub fn volumes(&self) -> &[u32] {
        &self.volumes
    }
    pub fn samplers(&self) -> &[u32] {
        &self.samplers
    }
    /// Every declared `t<n>`, its shape and return type.
    pub fn resources(&self) -> &[ResourceDeclaration] {
        &self.resources
    }
    /// Every declared unordered-access view `u<n>`, its shape and return type.
    pub fn views(&self) -> &[ResourceDeclaration] {
        &self.views
    }
    /// The immediate constant buffer's rows, as stored (empty when the program declares none).
    pub fn immediates(&self) -> &[[u32; 4]] {
        &self.immediates
    }
    pub(crate) fn resource(&self, slot: u32) -> Option<ResourceDeclaration> {
        self.resources.iter().copied().find(|r| r.slot == slot)
    }
    pub(crate) fn view(&self, slot: u32) -> Option<ResourceDeclaration> {
        self.views.iter().copied().find(|r| r.slot == slot)
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
    /// Compile a complete ps_4_0, ps_4_1 or ps_5_0 token stream before permitting evaluation.
    pub fn from_words(words: &[u32]) -> Result<Self> {
        if !matches!(words.first(), Some(0x40 | 0x41 | 0x50))
            || words.get(1).copied() != Some(words.len() as u32)
        {
            return Err(error(0, "expected complete ps_4_0, ps_4_1 or ps_5_0 program"));
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
            volumes: Vec::new(),
            resources: Vec::new(),
            views: Vec::new(),
            immediates: Vec::new(),
            samplers: Vec::new(),
            jumps: Vec::new(),
        };
        let mut at = 2;
        let mut ended = false;
        let mut execution = false;
        while at < words.len() {
            let t = words[at];
            let opcode = (t & 0x7ff) as u16;
            // customdata: its class in the opcode token's upper bits, its length (these two words included) the next
            // word; only the immediate constant buffer (class 3), a declaration, is read
            if opcode == 53 {
                let len = words.get(at + 1).copied().unwrap_or(0) as usize;
                if len < 2 || at + len > words.len() {
                    return Err(error(at, "invalid customdata length"));
                }
                if t >> 11 != 3 {
                    return Err(error(at, format!("unsupported customdata class {}", t >> 11)));
                }
                if execution || !p.immediates.is_empty() || !(len - 2).is_multiple_of(4) {
                    return Err(error(
                        at,
                        "an immediate constant buffer after code, repeated, or of partial rows",
                    ));
                }
                p.immediates = words[at + 2..at + len].as_chunks::<4>().0.to_vec();
                at += len;
                continue;
            }
            let name = NAMES.get(opcode as usize).copied().unwrap_or("unknown opcode");
            let count = match opcode {
                // break, continue, else, endif, endloop, loop, ret; and the declarations, read on their own
                2 | 7 | 18 | 21 | 22 | 48 | 62 | 88..=90 | 98..=101 | 104 | 106 | 156 | 158 | 162 => 0,
                // breakc, continuec, if, discard
                3 | 8 | 13 | 31 => 1,
                11 | 12 | 25..=28 | 40 | 43 | 47 | 54 | 59 | 64..=68 | 75 | 86 | 122..=125 | 129 => 2,
                0
                | 1
                | 14..=17
                | 24
                | 29
                | 30
                | 32..=34
                | 36
                | 37
                | 39
                | 41
                | 42
                | 49
                | 51
                | 52
                | 56
                | 57
                | 60
                | 77
                | 79
                | 80
                | 83..=85
                | 87
                // ld: destination, address, resource; store_uav_typed: view, address, value
                | 45
                | 164 => 3,
                // ubfe (width, offset, source); ld_structured (element, byte offset, resource); imm_atomic_iadd
                // (result, view, address, value); store_structured (view, element, byte offset, value); imul and
                // udiv (two destinations, two sources)
                35 | 38 | 50 | 55 | 69 | 78 | 138 | 167 | 168 | 180 => 4,
                // sample_b and sample_l (a bias or level after the sampler); bfi (width, offset, insert, base)
                72 | 74 | 140 => 5,
                73 => 6,
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
            let (mut dimension, mut returns) = (None, None);
            while extended {
                let ext = c.take()?;
                let kind = ext & 63;
                if !matches!(opcode, 45 | 69 | 72 | 73 | 74 | 167)
                    || kind > 3
                    || kind == 0
                    || seen & (1 << kind) != 0
                {
                    return Err(error(at, "unsupported or repeated opcode extension"));
                }
                seen |= 1 << kind;
                let payload = ext & 0x7fff_ffff;
                // sample offsets zero; the resource's dimension (a structured buffer's stride with it) and return type,
                // checked against its declaration
                let valid = match kind {
                    1 => payload == 1,
                    2 => {
                        dimension = Some(((payload >> 6) & 31, payload >> 11));
                        true
                    }
                    3 => {
                        returns = ReturnType::of(payload >> 6);
                        returns.is_some()
                    }
                    _ => false,
                };
                if !valid {
                    return Err(error(at, "unsupported sample offset/dimension/return type"));
                }
                extended = ext >> 31 != 0;
            }
            if (88..=106).contains(&opcode) || matches!(opcode, 156 | 158 | 162) {
                if execution {
                    return Err(error(at, "declaration after executable instruction"));
                }
                p.declaration(opcode, t, &mut c)?;
            } else {
                execution = true;
                // the conditionals' test (bit 18: nonzero) is their only control besides saturation
                let test = if matches!(opcode, 3 | 8 | 13 | 31) {
                    0x4_0000
                } else {
                    0
                };
                if t & 0x00ff_d800 & !test != 0 {
                    return Err(error(at, "unsupported instruction controls"));
                }
                let mut operands = Vec::new();
                for _ in 0..count {
                    operands.push(c.operand(0)?);
                }
                // on an integer instruction the negate modifier is two's-complement negation (modifier 4 here)
                if integer_sources(opcode) {
                    for o in &mut operands {
                        if o.modifier == 1 {
                            o.modifier = 4;
                        }
                    }
                }
                let ins = Instruction {
                    word: at,
                    opcode,
                    saturate: t & 0x2000 != 0,
                    nonzero: t & 0x4_0000 != 0,
                    operands,
                };
                p.validate(&ins)?;
                p.validate_extension(&ins, dimension, returns)?;
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
        p.jumps = p.structure()?;
        Ok(p)
    }
    /// Match the structured flow control (see [`Program::jumps`]): every `if` closed by its `endif` (at most one
    /// `else` between), every `loop` by its `endloop`, `break` and `continue` inside a loop, nothing open at `ret`.
    fn structure(&self) -> Result<Vec<usize>> {
        enum Open {
            If(usize),
            Else(usize),
            Loop(usize),
        }
        let mut jumps = vec![usize::MAX; self.code.len()];
        let mut open: Vec<Open> = Vec::new();
        for (i, ins) in self.code.iter().enumerate() {
            let fail = |detail: &str| error(ins.word, detail.to_owned());
            match ins.opcode {
                31 => open.push(Open::If(i)),
                18 => match open.pop() {
                    Some(Open::If(at)) => {
                        jumps[at] = i;
                        open.push(Open::Else(i));
                    }
                    _ => return Err(fail("else without its if")),
                },
                21 => match open.pop() {
                    Some(Open::If(at) | Open::Else(at)) => jumps[at] = i,
                    _ => return Err(fail("endif without its if")),
                },
                48 => open.push(Open::Loop(i)),
                22 => match open.pop() {
                    Some(Open::Loop(at)) => {
                        jumps[at] = i;
                        jumps[i] = at;
                    }
                    _ => return Err(fail("endloop without its loop")),
                },
                2 | 3 | 7 | 8 => {
                    jumps[i] = open
                        .iter()
                        .rev()
                        .find_map(|o| match o {
                            Open::Loop(at) => Some(*at),
                            _ => None,
                        })
                        .ok_or_else(|| fail("break or continue outside a loop"))?;
                }
                62 if !open.is_empty() => return Err(fail("ret inside an if or loop")),
                _ => {}
            }
        }
        Ok(jumps)
    }
    fn declaration(&mut self, opcode: u16, token: u32, c: &mut Cursor<'_>) -> Result<()> {
        let pos = c.origin - 1;
        let controls = match opcode {
            88 => 0xf800,
            // the dimension, and globally coherent (bit 16)
            156 => 0x1_f800,
            // globally coherent
            158 => 0x1_0000,
            162 => 0,
            89 => 0x800,
            // refactoring allowed (bit 11) and early depth-stencil (bit 13): neither changes an evaluation's outputs
            106 => 0x2800,
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
            106 => {}
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
                        // a buffer (1), texture2d (3), texture3d (5), cube (6), texture2darray (8) or cube array
                        // (10), its four lanes one return type
                        let dimension = match (token >> 11) & 31 {
                            1 => Dimension::Buffer,
                            3 => Dimension::Texture2d,
                            5 => Dimension::Texture3d,
                            6 => Dimension::TextureCube,
                            8 => Dimension::Texture2dArray,
                            10 => Dimension::TextureCubeArray,
                            _ => {
                                return Err(error(
                                    pos,
                                    "only buffer, texture2d, texture2darray, texture3d, cube and cube-array resources \
                                     supported",
                                ));
                            }
                        };
                        let returns = ReturnType::of(c.take()?)
                            .filter(|r| *r != ReturnType::Mixed)
                            .ok_or_else(|| error(pos, "a texture's lanes of one float, uint or sint type"))?;
                        self.declare(pos, i, dimension, returns)?;
                    }
                    156 if o.register == Register::Uav => {
                        let dimension = match (token >> 11) & 31 {
                            3 => Dimension::Texture2d,
                            5 => Dimension::Texture3d,
                            8 => Dimension::Texture2dArray,
                            _ => {
                                return Err(error(
                                    pos,
                                    "only texture2d, texture2darray and texture3d views supported",
                                ));
                            }
                        };
                        let returns = ReturnType::of(c.take()?)
                            .filter(|r| *r != ReturnType::Mixed)
                            .ok_or_else(|| error(pos, "a view's lanes of one float, uint or sint type"))?;
                        self.declare_view(pos, i, dimension, returns)?;
                    }
                    158 if o.register == Register::Uav => {
                        let stride = c.take()?;
                        if stride == 0 || stride % 4 != 0 || stride > 2048 {
                            return Err(error(pos, "structured stride"));
                        }
                        self.declare_view(pos, i, Dimension::Structured(stride), ReturnType::Mixed)?;
                    }
                    162 if o.register == Register::Texture => {
                        let stride = c.take()?;
                        if stride == 0 || stride % 4 != 0 || stride > 2048 {
                            return Err(error(pos, "structured stride"));
                        }
                        self.declare(pos, i, Dimension::Structured(stride), ReturnType::Mixed)?;
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
    /// Declare `u<slot>`.
    fn declare_view(
        &mut self,
        pos: usize,
        slot: u32,
        dimension: Dimension,
        returns: ReturnType,
    ) -> Result<()> {
        if slot >= 64 || self.views.iter().any(|v| v.slot == slot) {
            return Err(error(pos, "invalid/duplicate view"));
        }
        self.views.push(ResourceDeclaration {
            slot,
            dimension,
            returns,
        });
        Ok(())
    }
    /// Declare `t<slot>`.
    fn declare(&mut self, pos: usize, slot: u32, dimension: Dimension, returns: ReturnType) -> Result<()> {
        if slot >= 128 || self.resources.iter().any(|r| r.slot == slot) {
            return Err(error(pos, "invalid/duplicate texture"));
        }
        self.resources.push(ResourceDeclaration {
            slot,
            dimension,
            returns,
        });
        match dimension {
            Dimension::Texture2d | Dimension::Texture2dArray => self.textures.push(slot),
            Dimension::Texture3d => self.volumes.push(slot),
            Dimension::Buffer
            | Dimension::TextureCube
            | Dimension::TextureCubeArray
            | Dimension::Structured(_) => {}
        }
        Ok(())
    }
    /// The resource an instruction reads (at its operand `n`), and its extension tokens against its declaration.
    fn validate_extension(
        &self,
        ins: &Instruction,
        dimension: Option<(u32, u32)>,
        returns: Option<ReturnType>,
    ) -> Result<()> {
        let fail = |detail: &str| error(ins.word, detail.to_owned());
        let Some(n) = resource_operand(ins.opcode) else {
            return Ok(());
        };
        let slot = ins.operands[n].indices[0].base;
        let declared = self.resource(slot).ok_or_else(|| fail("undeclared resource"))?;
        let sampling = matches!(ins.opcode, 69 | 72 | 73 | 74);
        let fits = match (ins.opcode, declared.dimension) {
            (167, Dimension::Structured(_)) => true,
            (167, _) | (_, Dimension::Structured(_)) => false,
            // no `ld` of a cube; no sample of a buffer
            (45, d) => !matches!(d, Dimension::TextureCube | Dimension::TextureCubeArray),
            (_, d) => d != Dimension::Buffer,
        };
        if !fits {
            return Err(fail("an instruction on a resource of another shape"));
        }
        if sampling && declared.returns != ReturnType::Float {
            return Err(fail("a sample of an integer texture"));
        }
        if let Some((code, stride)) = dimension {
            let want = match declared.dimension {
                Dimension::Structured(s) => (12, s),
                d => (d.code(), 0),
            };
            if (code, stride) != want {
                return Err(fail("a resource-dimension extension other than its declaration"));
            }
        }
        if returns.is_some_and(|r| r != declared.returns) {
            return Err(fail("a return-type extension other than its declaration"));
        }
        Ok(())
    }
    fn validate(&self, ins: &Instruction) -> Result<()> {
        let fail = |detail| error(ins.word, detail);
        let sampling = matches!(ins.opcode, 69 | 72 | 73 | 74);
        let resource = resource_operand(ins.opcode);
        let dests = if matches!(ins.opcode, 38 | 77 | 78) {
            2
        } else if flow(ins.opcode) || matches!(ins.opcode, 164 | 168) {
            0
        } else {
            1
        };
        // a store's or an atomic's view: declared, a store's with a write mask, an atomic's with no selection; a typed
        // store's view typed, a structured store's structured, an atomic's structured or of integers
        let view = view_operand(ins.opcode);
        if let Some(n) = view {
            let o = &ins.operands[n];
            let selected = match ins.opcode {
                164 => matches!(o.selection, Selection::Mask(1..=15)),
                // a structured store's lanes consecutive from x
                168 => matches!(o.selection, Selection::Mask(1 | 3 | 7 | 15)),
                _ => o.selection == Selection::None,
            };
            if o.register != Register::Uav || !selected {
                return Err(fail("a store's view with a write mask, an atomic's with none"));
            }
            let declared = self
                .view(o.indices[0].base)
                .ok_or_else(|| fail("undeclared view"))?;
            let fits = match (ins.opcode, declared.dimension, declared.returns) {
                (164, Dimension::Structured(_), _) => false,
                (168, d, _) => matches!(d, Dimension::Structured(_)),
                (164, ..) | (_, Dimension::Structured(_), _) => true,
                (_, _, returns) => matches!(returns, ReturnType::Uint | ReturnType::Sint),
            };
            if !fits {
                return Err(fail("an instruction on a view of another shape"));
            }
        }
        if ins.saturate && non_float(ins.opcode) {
            return Err(fail("saturation on non-float result"));
        }
        for (n, o) in ins.operands.iter().enumerate() {
            if view == Some(n) {
                continue;
            }
            if o.register == Register::Uav {
                return Err(fail("a view outside a store"));
            }
            if o.register == Register::Null && !(matches!(ins.opcode, 38 | 77 | 78) && n < 2) {
                return Err(fail(
                    "null is supported only as an imul, udiv or sincos destination",
                ));
            }
            if integer_sources(ins.opcode) && !matches!(o.modifier, 0 | 4) {
                return Err(fail("unsupported integer source modifier (abs)"));
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
            if resource == Some(n) && !matches!(o.selection, Selection::Swizzle(_)) {
                return Err(fail("resource requires return swizzle"));
            }
            if (resource == Some(n) && o.register != Register::Texture)
                || (sampling && n == 3 && o.register != Register::Sampler)
            {
                return Err(fail("sample or load requires texture and sampler operands"));
            }
            if matches!(o.register, Register::Texture | Register::Sampler)
                && !(resource == Some(n) || (sampling && n == 3))
            {
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
            Register::Texture => self.resource(i).is_some(),
            Register::Uav => self.views.iter().any(|v| v.slot == i),
            Register::ImmediateBuffer => {
                o.indices[0].relative.is_some() || (i as usize) < self.immediates.len()
            }
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
