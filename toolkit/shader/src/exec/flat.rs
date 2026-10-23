//! The flat form of a program: operations on registers of four 32-bit words, and the structured statements around
//! them.

use super::request::{Dim, QueryKind};
use crate::ResourceId;

pub(super) type Reg = [u32; 4];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BinOp {
    AddF,
    SubF,
    MulF,
    DivF,
    AddU,
    SubU,
    MulU,
    DivU,
    ModU,
    And,
    Or,
    Xor,
    Shl,
    ShrU,
    ShrI,
    EqW,
    NeW,
    EqF,
    NeF,
    LtF,
    LeF,
    GtF,
    GeF,
    LtU,
    LeU,
    GtU,
    GeU,
    LtI,
    LeI,
    GtI,
    GeI,
    LAnd,
    LOr,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum UnOp {
    NegF,
    NegI,
    Not,
    BitNot,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CvtOp {
    F2I,
    F2U,
    I2F,
    U2F,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MathOp {
    AbsF,
    AbsI,
    MinF,
    MaxF,
    MinU,
    MaxU,
    MinI,
    MaxI,
    ClampF,
    SatF,
    Floor,
    Ceil,
    Trunc,
    Round,
    Fract,
    Exp2,
    Log2,
    Sin,
    Cos,
    Sqrt,
    Rsqrt,
    Dot,
    Popcnt,
    RevBits,
    Clz,
    Ctz,
    PackHalf,
    UnpackHalf,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Loc {
    Local(u32),
    Global(u32, u32),
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Ptr {
    pub loc: Loc,
    /// Slot of the index register, words per index step, element count.
    pub dynamic: Option<(u32, u32, u32)>,
}

#[derive(Clone, Debug)]
pub(super) enum Op {
    Swizzle {
        src: u32,
        pat: [u8; 4],
        n: u8,
    },
    Splat {
        src: u32,
        n: u8,
    },
    Compose {
        parts: [(u32, u8); 4],
        np: u8,
    },
    Lane {
        src: u32,
        i: u8,
    },
    Load {
        loc: Loc,
        n: u8,
    },
    LoadLanes {
        loc: Loc,
        offs: [u8; 4],
        n: u8,
    },
    LoadDyn {
        ptr: Ptr,
        n: u8,
    },
    Un {
        op: UnOp,
        src: u32,
        n: u8,
    },
    Bin {
        op: BinOp,
        a: u32,
        b: u32,
        n: u8,
        a1: bool,
        b1: bool,
    },
    Select {
        c: u32,
        a: u32,
        b: u32,
        n: u8,
        c1: bool,
    },
    Cvt {
        op: CvtOp,
        src: u32,
        n: u8,
    },
    Math {
        f: MathOp,
        a: u32,
        b: u32,
        c: u32,
        n: u8,
    },
    Deriv {
        src: u32,
        axis: u8,
        fine: bool,
        n: u8,
        prov: Option<[(u32, u8); 4]>,
    },
    Sample {
        tex: Res,
        smp: Res,
        coord: u32,
        cn: u8,
        layer: Option<u32>,
        level: LevelOp,
        dim: Dim,
        offset: [i32; 3],
        gather: Option<u8>,
        depth_ref: Option<u32>,
        depth: bool,
    },
    Texel {
        tex: Res,
        coord: u32,
        cn: u8,
        dim: Dim,
        storage: bool,
        layer: Option<u32>,
        sample: Option<u32>,
        mip: Option<u32>,
    },
    Query {
        tex: Res,
        dim: Dim,
        kind: QueryKind,
        level: Option<u32>,
    },
    ArrLen {
        gid: u32,
        elem_words: u32,
    },
}
/// Which element of a resource array an operation reads.
#[derive(Clone, Copy, Debug)]
pub(super) enum Pick {
    /// The resource is not an array.
    One,
    Fixed(u32),
    /// The first lane of a register, per invocation.
    Slot(u32),
}
/// A resource an operation names, and for an array the element it picks and how many there are.
#[derive(Clone, Copy, Debug)]
pub(super) struct Res {
    pub id: ResourceId,
    pub pick: Pick,
    pub count: u32,
}
impl Res {
    /// The register an element index is read from.
    pub(super) fn slot(&self) -> Option<u32> {
        match self.pick {
            Pick::Slot(s) => Some(s),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) enum LevelOp {
    Auto,
    Zero,
    Exact(u32),
    Bias(u32),
    Gradient(u32, u32),
}

#[derive(Clone, Debug)]
pub(super) enum St {
    Run(u32, u32),
    Store {
        loc: Loc,
        src: u32,
        n: u8,
    },
    StoreDyn {
        ptr: Ptr,
        src: u32,
        n: u8,
    },
    ImageStore {
        tex: ResourceId,
        dim: Dim,
        coord: u32,
        cn: u8,
        layer: Option<u32>,
        value: u32,
        vn: u8,
    },
    Atomic {
        ptr: Ptr,
        value: u32,
        result: Option<u32>,
    },
    If {
        c: u32,
        a: Vec<St>,
        r: Vec<St>,
    },
    Loop {
        body: Vec<St>,
        cont: Vec<St>,
        break_if: Option<u32>,
    },
    Switch {
        sel: u32,
        cases: Vec<(Option<u32>, bool, Vec<St>)>,
    },
    Break,
    Continue,
    /// Leave, with these members as the result (slot and lanes each, in the order of the interface's outputs).
    Return(Vec<(u32, u8)>),
    Kill,
}

#[derive(Clone, Debug)]
pub(super) enum GlobalKind {
    /// A constant buffer of the interface.
    Constants(ResourceId),
    /// Private data: an immediate constant buffer's words.
    Private(Vec<u32>),
    /// A buffer of the interface read through the storage address space.
    Storage(ResourceId),
}
#[derive(Clone, Debug)]
pub(super) struct GlobalDesc {
    pub kind: GlobalKind,
    /// Words of one element of the global's array (1 for anything else).
    pub elem_words: u32,
}

/// A program compiled once into a flat form: typed operations on four-lane word registers, static pointer arithmetic
/// folded, bitcasts aliased, dead operations removed. Running it is [`Runner`](super::Runner)'s.
#[derive(Debug)]
pub struct Executable {
    pub(super) ops: Vec<(u32, Op)>,
    pub(super) body: Vec<St>,
    pub(super) slots: usize,
    pub(super) inits: Vec<(u32, Reg)>,
    /// Per entry argument: its slot and lanes.
    pub(super) args: Vec<(u32, u8)>,
    pub(super) local_words: usize,
    pub(super) globals: Vec<GlobalDesc>,
    /// Whether 32-bit denormals are flushed to zero (the program's D3D modes).
    pub(super) flush_denormals: bool,
}
