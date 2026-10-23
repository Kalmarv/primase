//! LLVM 3.7 IR as DXIL stores it, read from the bitstream tree: the type table, module values (globals, functions,
//! constants), metadata, value names and function bodies. Only what DXIL pixel shaders use is decoded; any other
//! record in a function body is refused by name, never skipped.
use crate::bitstream::{Block, Record};
use crate::{Result, error};

const MODULE: u64 = 8;
const PARAMATTR: u64 = 9;
const PARAMATTR_GROUP: u64 = 10;
const CONSTANTS: u64 = 11;
const FUNCTION: u64 = 12;
const VALUE_SYMTAB: u64 = 14;
const METADATA: u64 = 15;
const METADATA_ATTACHMENT: u64 = 16;
const TYPES: u64 = 17;
const USELIST: u64 = 18;
const BLOCKINFO: u64 = 0;

/// Values a module may hold (globals, constants and function-body values together).
const MAX_VALUES: usize = 1 << 22;
const MAX_TYPES: usize = 1 << 16;

pub type TypeId = usize;
pub type ValueId = usize;
pub type MetadataId = usize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Void,
    Label,
    Metadata,
    Half,
    Float,
    Double,
    Integer(u32),
    Pointer(TypeId),
    Array(u64, TypeId),
    Vector(u64, TypeId),
    Struct {
        name: Option<String>,
        elements: Vec<TypeId>,
    },
    Opaque(Option<String>),
    Function {
        vararg: bool,
        ret: TypeId,
        params: Vec<TypeId>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Null,
    Undef,
    /// An integer's bits, sign-extended from its width.
    Integer(i64),
    /// A float's bits (half, float or double by its type).
    Float(u64),
    Aggregate(Vec<ValueId>),
    Data(Vec<u64>),
    String(Vec<u8>),
    /// A constant `getelementptr`: the base (a global) and its indices (module constants).
    Gep {
        base: ValueId,
        indices: Vec<ValueId>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Function {
        ty: TypeId,
        declaration: bool,
    },
    Global {
        /// The global's type: a pointer to `value_ty`.
        ty: TypeId,
        /// The type of the value it holds.
        value_ty: TypeId,
        /// Its initial value (a module constant), if it has one.
        init: Option<ValueId>,
        /// Whether it is a constant (read only).
        constant: bool,
        /// LLVM's address space: 0 default, 1 device memory, 3 group shared, 4 constants.
        address_space: u32,
    },
    Constant(Constant),
    /// A function argument or an instruction's result.
    Local,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Metadata {
    String(Vec<u8>),
    Value {
        ty: TypeId,
        value: ValueId,
    },
    /// Operands; `None` is a null operand.
    Node(Vec<Option<MetadataId>>),
}

/// A binary operator code (LLVM's `BinaryOpcodes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    UDiv,
    SDiv,
    URem,
    SRem,
    Shl,
    LShr,
    AShr,
    And,
    Or,
    Xor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastOp {
    Trunc,
    ZExt,
    SExt,
    FpToUi,
    FpToSi,
    UiToFp,
    SiToFp,
    FpTrunc,
    FpExt,
    PtrToInt,
    IntToPtr,
    BitCast,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Integer opcodes apply to integers; for floating types Add, Sub, Mul, (S)Div, (S)Rem are the float forms.
    Binary {
        op: BinaryOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    Cast {
        op: CastOp,
        value: ValueId,
    },
    /// A comparison by its LLVM predicate (0..15 float, 32..41 integer).
    Compare {
        predicate: u64,
        lhs: ValueId,
        rhs: ValueId,
    },
    Select {
        condition: ValueId,
        then: ValueId,
        otherwise: ValueId,
    },
    ExtractValue {
        aggregate: ValueId,
        indices: Vec<u64>,
    },
    InsertValue {
        aggregate: ValueId,
        value: ValueId,
        indices: Vec<u64>,
    },
    Call {
        function: ValueId,
        args: Vec<ValueId>,
    },
    /// The value from the predecessor block control came from: (value, block) pairs.
    Phi {
        incoming: Vec<(ValueId, usize)>,
    },
    Return(Option<ValueId>),
    Branch {
        target: usize,
    },
    CondBranch {
        condition: ValueId,
        then: usize,
        otherwise: usize,
    },
    /// A multi-way branch: the value, the default block and the (case constant, block) pairs.
    Switch {
        value: ValueId,
        default: usize,
        cases: Vec<(ValueId, usize)>,
    },
    Unreachable,
    /// A local variable of this type (the result is a pointer to it).
    Alloca {
        ty: TypeId,
    },
    Load {
        pointer: ValueId,
    },
    Store {
        pointer: ValueId,
        value: ValueId,
    },
    /// `getelementptr`: the base pointer and its indices (the first steps over the pointer itself).
    Gep {
        base: ValueId,
        indices: Vec<ValueId>,
    },
    /// `atomicrmw`: the operation (LLVM's code: 0 xchg, 1 add, 2 sub, 3 and, 4 nand, 5 or, 6 xor, 7 max, 8 min, 9 umax,
    /// 10 umin); the result is the value the pointer held.
    AtomicRmw {
        pointer: ValueId,
        value: ValueId,
        op: u64,
    },
    /// `cmpxchg`: the result is the pair (the value the pointer held, whether it was swapped).
    CmpXchg {
        pointer: ValueId,
        compare: ValueId,
        value: ValueId,
    },
    /// `fence`: ordering between invocations, which a lockstep reading does not have.
    Fence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub op: Op,
    /// The result's value id and type, when the instruction produces one.
    pub result: Option<(ValueId, TypeId)>,
    /// The basic block it belongs to.
    pub block: usize,
    /// Bit offset of its record.
    pub at: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// The function's module value id.
    pub value: ValueId,
    pub blocks: usize,
    pub instructions: Vec<Instruction>,
    /// The body's own values (arguments, constants, instruction results) and their types; their ids start at
    /// `first`, after the module's values.
    pub first: ValueId,
    pub locals: Vec<Value>,
    pub local_types: Vec<TypeId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub types: Vec<Type>,
    /// The module's values by id (globals, functions, module constants); a function body's own values continue the
    /// numbering in its [`Function`].
    pub values: Vec<Value>,
    /// Each value's type.
    pub value_types: Vec<TypeId>,
    pub names: Vec<(ValueId, String)>,
    pub metadata: Vec<Metadata>,
    pub named_metadata: Vec<(String, Vec<MetadataId>)>,
    pub functions: Vec<Function>,
}

fn op(r: &Record, i: usize) -> Result<u64> {
    r.operands
        .get(i)
        .copied()
        .ok_or_else(|| error(r.at, format!("record {} lacks operand {i}", r.code)))
}
fn text(ops: &[u64], at: usize) -> Result<Vec<u8>> {
    ops.iter()
        .map(|&c| u8::try_from(c).map_err(|_| error(at, "character above 255")))
        .collect()
}
fn utf8(bytes: Vec<u8>, at: usize) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| error(at, "name is not UTF-8"))
}
fn signed_vbr(v: u64) -> i64 {
    if v & 1 == 0 {
        (v >> 1) as i64
    } else if v == 1 {
        i64::MIN
    } else {
        -((v >> 1) as i64)
    }
}

impl Module {
    fn ty(&self, id: u64, at: usize) -> Result<TypeId> {
        let id = usize::try_from(id).map_err(|_| error(at, "type id"))?;
        if id < self.types.len() {
            Ok(id)
        } else {
            Err(error(at, format!("type {id} undefined")))
        }
    }
    pub fn type_of(&self, value: ValueId) -> Option<&Type> {
        self.value_types.get(value).and_then(|&t| self.types.get(t))
    }
    pub fn name(&self, value: ValueId) -> Option<&str> {
        self.names
            .iter()
            .find(|(v, _)| *v == value)
            .map(|(_, n)| n.as_str())
    }
    fn push(&mut self, value: Value, ty: TypeId, at: usize) -> Result<ValueId> {
        if self.values.len() >= MAX_VALUES {
            return Err(error(at, "too many values"));
        }
        self.values.push(value);
        self.value_types.push(ty);
        Ok(self.values.len() - 1)
    }

    fn read_types(&mut self, block: &Block) -> Result<()> {
        let mut struct_name: Option<String> = None;
        for r in block.records() {
            if self.types.len() >= MAX_TYPES {
                return Err(error(r.at, "too many types"));
            }
            let t = match r.code {
                1 => continue, // NUMENTRY
                2 => Type::Void,
                3 => Type::Float,
                4 => Type::Double,
                5 => Type::Label,
                6 => Type::Opaque(struct_name.take()),
                7 => Type::Integer(
                    u32::try_from(op(r, 0)?)
                        .ok()
                        .filter(|w| (1..=64).contains(w))
                        .ok_or_else(|| error(r.at, "integer width"))?,
                ),
                8 => Type::Pointer(self.ty(op(r, 0)?, r.at)?),
                10 => Type::Half,
                11 => Type::Array(op(r, 0)?, self.ty(op(r, 1)?, r.at)?),
                12 => Type::Vector(op(r, 0)?, self.ty(op(r, 1)?, r.at)?),
                16 => Type::Metadata,
                18 | 20 => Type::Struct {
                    name: if r.code == 20 { struct_name.take() } else { None },
                    elements: r.operands[1.min(r.operands.len())..]
                        .iter()
                        .map(|&t| self.ty(t, r.at))
                        .collect::<Result<_>>()?,
                },
                19 => {
                    struct_name = Some(utf8(text(&r.operands, r.at)?, r.at)?);
                    continue;
                }
                21 => Type::Function {
                    vararg: op(r, 0)? != 0,
                    ret: self.ty(op(r, 1)?, r.at)?,
                    params: r.operands[2.min(r.operands.len())..]
                        .iter()
                        .map(|&t| self.ty(t, r.at))
                        .collect::<Result<_>>()?,
                },
                other => return Err(error(r.at, format!("type record {other}"))),
            };
            self.types.push(t);
        }
        Ok(())
    }

    /// Read a constants block, appending its values. Value ids inside aggregates are absolute.
    fn read_constants(&mut self, block: &Block) -> Result<()> {
        let mut ty: Option<TypeId> = None;
        for r in block.records() {
            if r.code == 1 {
                ty = Some(self.ty(op(r, 0)?, r.at)?);
                continue;
            }
            let t = ty.ok_or_else(|| error(r.at, "constant before SETTYPE"))?;
            let c = match r.code {
                2 => Constant::Null,
                3 => Constant::Undef,
                4 => Constant::Integer(signed_vbr(op(r, 0)?)),
                6 => Constant::Float(op(r, 0)?),
                7 => Constant::Aggregate(
                    r.operands
                        .iter()
                        .map(|&v| usize::try_from(v).map_err(|_| error(r.at, "value id")))
                        .collect::<Result<_>>()?,
                ),
                8 | 9 => Constant::String(text(&r.operands, r.at)?),
                22 => Constant::Data(r.operands.clone()),
                // CE_GEP and CE_INBOUNDS_GEP: [source type,] then (type, value) pairs, the first the base
                12 | 20 => {
                    let ops = &r.operands[usize::from(r.operands.len() % 2 == 1)..];
                    let (pairs, rest) = ops.as_chunks::<2>();
                    if !rest.is_empty() || pairs.is_empty() {
                        return Err(error(r.at, "constant getelementptr operands"));
                    }
                    let id = |v: u64| usize::try_from(v).map_err(|_| error(r.at, "value id"));
                    Constant::Gep {
                        base: id(pairs[0][1])?,
                        indices: pairs[1..].iter().map(|p| id(p[1])).collect::<Result<_>>()?,
                    }
                }
                other => {
                    return Err(error(
                        r.at,
                        format!("constant record {other} (constant expressions are not supported)"),
                    ));
                }
            };
            self.push(Value::Constant(c), t, r.at)?;
        }
        Ok(())
    }

    fn read_metadata(&mut self, block: &Block) -> Result<()> {
        let mut name: Option<String> = None;
        let md = |v: u64, at: usize| usize::try_from(v).map_err(|_| error(at, "metadata id"));
        for r in block.records() {
            match r.code {
                1 => self.metadata.push(Metadata::String(text(&r.operands, r.at)?)),
                2 => self.metadata.push(Metadata::Value {
                    ty: self.ty(op(r, 0)?, r.at)?,
                    value: md(op(r, 1)?, r.at)?,
                }),
                3 | 5 => self.metadata.push(Metadata::Node(
                    r.operands
                        .iter()
                        .map(|&v| {
                            if v == 0 {
                                Ok(None)
                            } else {
                                md(v - 1, r.at).map(Some)
                            }
                        })
                        .collect::<Result<_>>()?,
                )),
                4 => name = Some(utf8(text(&r.operands, r.at)?, r.at)?),
                10 => {
                    let n = name
                        .take()
                        .ok_or_else(|| error(r.at, "named node without a name"))?;
                    let nodes = r.operands.iter().map(|&v| md(v, r.at)).collect::<Result<_>>()?;
                    self.named_metadata.push((n, nodes));
                }
                6 => {} // KIND: names of attachment kinds
                other => return Err(error(r.at, format!("metadata record {other}"))),
            }
        }
        Ok(())
    }

    fn read_symbols(&mut self, block: &Block, base: usize) -> Result<()> {
        for r in block.records() {
            match r.code {
                1 | 3 => {
                    let value = usize::try_from(op(r, 0)?).map_err(|_| error(r.at, "value id"))?;
                    let skip = if r.code == 3 { 2 } else { 1 };
                    let n = utf8(text(&r.operands[skip.min(r.operands.len())..], r.at)?, r.at)?;
                    self.names.push((base + value, n));
                }
                2 => {} // basic-block names
                other => return Err(error(r.at, format!("symbol record {other}"))),
            }
        }
        Ok(())
    }
}

/// Function-body decoding state: relative value numbering (module version 1).
struct Body<'m> {
    m: &'m mut Module,
    block: usize,
    blocks: usize,
}
impl Body<'_> {
    fn next(&self) -> usize {
        self.m.values.len()
    }
    /// A relative operand (absolute = next - operand), with its explicit type following when it is a forward
    /// reference (a value defined later in the function's layout). Returns the value id and its type, and advances `i`.
    fn value_type(&self, r: &Record, i: &mut usize) -> Result<(ValueId, TypeId)> {
        let rel = op(r, *i)?;
        *i += 1;
        let next = self.next() as u64;
        let id = (next as u32).wrapping_sub(rel as u32) as usize;
        if id < self.next() {
            Ok((id, self.m.value_types[id]))
        } else {
            let t = self.m.ty(op(r, *i)?, r.at)?;
            *i += 1;
            Ok((id, t))
        }
    }
    /// A relative operand whose type is known from another operand: a forward reference carries none.
    fn value(&self, r: &Record, i: &mut usize) -> Result<ValueId> {
        let rel = op(r, *i)?;
        *i += 1;
        Ok((self.next() as u32).wrapping_sub(rel as u32) as usize)
    }
    /// The type of a value defined before this point, or of a forward reference by the type given.
    fn pointer_to(&mut self, t: TypeId) -> TypeId {
        match self.m.types.iter().position(|x| *x == Type::Pointer(t)) {
            Some(p) => p,
            None => {
                self.m.types.push(Type::Pointer(t));
                self.m.types.len() - 1
            }
        }
    }
    /// The index a constant integer value stands for, if it is one defined in the module or this function.
    fn constant_index(&self, v: ValueId) -> Option<usize> {
        match self.m.values.get(v) {
            Some(Value::Constant(Constant::Integer(i))) => usize::try_from(*i).ok(),
            Some(Value::Constant(Constant::Null)) => Some(0),
            _ => None,
        }
    }
    fn result(&mut self, ty: TypeId, at: usize) -> Result<Option<(ValueId, TypeId)>> {
        if self.m.types[ty] == Type::Void {
            return Ok(None);
        }
        Ok(Some((self.m.push(Value::Local, ty, at)?, ty)))
    }
}

fn binary(code: u64, at: usize) -> Result<BinaryOp> {
    Ok(match code {
        0 => BinaryOp::Add,
        1 => BinaryOp::Sub,
        2 => BinaryOp::Mul,
        3 => BinaryOp::UDiv,
        4 => BinaryOp::SDiv,
        5 => BinaryOp::URem,
        6 => BinaryOp::SRem,
        7 => BinaryOp::Shl,
        8 => BinaryOp::LShr,
        9 => BinaryOp::AShr,
        10 => BinaryOp::And,
        11 => BinaryOp::Or,
        12 => BinaryOp::Xor,
        other => return Err(error(at, format!("binary opcode {other}"))),
    })
}
fn cast(code: u64, at: usize) -> Result<CastOp> {
    Ok(match code {
        0 => CastOp::Trunc,
        1 => CastOp::ZExt,
        2 => CastOp::SExt,
        3 => CastOp::FpToUi,
        4 => CastOp::FpToSi,
        5 => CastOp::UiToFp,
        6 => CastOp::SiToFp,
        7 => CastOp::FpTrunc,
        8 => CastOp::FpExt,
        9 => CastOp::PtrToInt,
        10 => CastOp::IntToPtr,
        11 => CastOp::BitCast,
        other => return Err(error(at, format!("cast opcode {other}"))),
    })
}
/// The name of a function-body record code this reader refuses.
fn refused(code: u64) -> &'static str {
    match code {
        4 | 30 => "getelementptr (old form)",
        6 => "extractelement",
        7 => "insertelement",
        8 => "shufflevector",
        13 => "invoke",
        41 => "load (atomic)",
        23 => "va_arg",
        24 | 42 | 45 => "store (atomic)",
        31 => "indirectbr",
        37 => "cmpxchg (old form)",
        59 => "atomicrmw (new form)",
        39 => "resume",
        40 | 47 => "landingpad",
        _ => "unknown instruction record",
    }
}

impl Module {
    fn read_function(&mut self, block: &Block, value: ValueId) -> Result<Function> {
        let fty = match self.types.get(self.value_types[value]) {
            Some(Type::Pointer(t)) => *t,
            _ => return Err(error(block.at, "function value is not a function pointer")),
        };
        let params = match &self.types[fty] {
            Type::Function { params, .. } => params.clone(),
            _ => return Err(error(block.at, "function type")),
        };
        let base = self.values.len();
        for p in params {
            self.push(Value::Local, p, block.at)?;
        }
        let mut body = Body {
            m: self,
            block: 0,
            blocks: 0,
        };
        let mut instructions = Vec::new();
        for e in &block.elements {
            let r = match e {
                crate::bitstream::Element::Block(b) => {
                    match b.id {
                        CONSTANTS => body.m.read_constants(b)?,
                        VALUE_SYMTAB => body.m.read_symbols(b, 0)?,
                        METADATA | METADATA_ATTACHMENT | USELIST => {}
                        other => return Err(error(b.at, format!("block {other} in a function"))),
                    }
                    continue;
                }
                crate::bitstream::Element::Record(r) => r,
            };
            let mut i = 0;
            let (op, ty): (Op, Option<TypeId>) = match r.code {
                1 => {
                    body.blocks = usize::try_from(op(r, 0)?)
                        .ok()
                        .filter(|&n| n > 0 && n <= 1 << 16)
                        .ok_or_else(|| error(r.at, "basic block count"))?;
                    continue;
                }
                33 | 35 => continue, // debug locations
                2 => {
                    let (lhs, t) = body.value_type(r, &mut i)?;
                    let rhs = body.value(r, &mut i)?;
                    let o = binary(op(r, i)?, r.at)?;
                    (Op::Binary { op: o, lhs, rhs }, Some(t))
                }
                3 => {
                    let value = body.value(r, &mut i)?;
                    let to = body.m.ty(op(r, i)?, r.at)?;
                    let o = cast(op(r, i + 1)?, r.at)?;
                    (Op::Cast { op: o, value }, Some(to))
                }
                9 | 28 => {
                    let (lhs, t) = body.value_type(r, &mut i)?;
                    let rhs = body.value(r, &mut i)?;
                    let predicate = op(r, i)?;
                    let bool_ty = match &body.m.types[t] {
                        Type::Vector(..) => {
                            return Err(error(r.at, "vector comparison"));
                        }
                        _ => body
                            .m
                            .types
                            .iter()
                            .position(|x| *x == Type::Integer(1))
                            .ok_or_else(|| error(r.at, "no i1 type for a comparison"))?,
                    };
                    (Op::Compare { predicate, lhs, rhs }, Some(bool_ty))
                }
                29 => {
                    let (then, t) = body.value_type(r, &mut i)?;
                    let otherwise = body.value(r, &mut i)?;
                    let condition = body.value(r, &mut i)?;
                    (
                        Op::Select {
                            condition,
                            then,
                            otherwise,
                        },
                        Some(t),
                    )
                }
                26 => {
                    let (aggregate, mut t) = body.value_type(r, &mut i)?;
                    let indices = r.operands[i..].to_vec();
                    for &k in &indices {
                        t = match &body.m.types[t] {
                            Type::Struct { elements, .. } => *elements
                                .get(k as usize)
                                .ok_or_else(|| error(r.at, "extractvalue index"))?,
                            Type::Array(n, e) if k < *n => *e,
                            _ => return Err(error(r.at, "extractvalue index")),
                        };
                    }
                    (Op::ExtractValue { aggregate, indices }, Some(t))
                }
                27 => {
                    let (aggregate, t) = body.value_type(r, &mut i)?;
                    let value = body.value(r, &mut i)?;
                    let indices = r.operands[i..].to_vec();
                    (
                        Op::InsertValue {
                            aggregate,
                            value,
                            indices,
                        },
                        Some(t),
                    )
                }
                34 => {
                    i = 1; // paramattrs
                    let cc = op(r, i)?;
                    i += 1;
                    if cc & (1 << 17) != 0 {
                        i += 1; // fast-math flags
                    }
                    let explicit = if cc & (1 << 15) != 0 {
                        let t = body.m.ty(op(r, i)?, r.at)?;
                        i += 1;
                        Some(t)
                    } else {
                        None
                    };
                    let (function, callee_ty) = body.value_type(r, &mut i)?;
                    let fty = match (explicit, &body.m.types[callee_ty]) {
                        (Some(t), _) => t,
                        (None, Type::Pointer(t)) => *t,
                        _ => return Err(error(r.at, "callee is not a function pointer")),
                    };
                    let (ret, params, vararg) = match &body.m.types[fty] {
                        Type::Function { ret, params, vararg } => (*ret, params.clone(), *vararg),
                        _ => return Err(error(r.at, "call through a non-function type")),
                    };
                    if vararg {
                        return Err(error(r.at, "vararg call"));
                    }
                    let mut args = Vec::with_capacity(params.len());
                    for p in &params {
                        // a fixed parameter carries no type, even for a value defined later in the layout
                        let a = body.value(r, &mut i)?;
                        if a < body.next() && body.m.value_types[a] != *p {
                            return Err(error(r.at, "call argument type"));
                        }
                        args.push(a);
                    }
                    if i != r.operands.len() {
                        return Err(error(r.at, "call operands left over"));
                    }
                    (Op::Call { function, args }, Some(ret))
                }
                16 => {
                    let t = body.m.ty(op(r, 0)?, r.at)?;
                    let pairs = &r.operands[1..];
                    if pairs.is_empty() || pairs.len() % 2 != 0 {
                        return Err(error(r.at, "phi operands"));
                    }
                    let next = body.next() as i64;
                    let mut incoming = Vec::with_capacity(pairs.len() / 2);
                    for pair in pairs.as_chunks::<2>().0 {
                        // Signed and relative to the phi itself; it may name a value defined later.
                        let id = next
                            .checked_sub(signed_vbr(pair[0]))
                            .and_then(|v| usize::try_from(v).ok())
                            .ok_or_else(|| error(r.at, "phi value"))?;
                        let block = usize::try_from(pair[1])
                            .ok()
                            .filter(|&b| b < body.blocks)
                            .ok_or_else(|| error(r.at, "phi block"))?;
                        incoming.push((id, block));
                    }
                    (Op::Phi { incoming }, Some(t))
                }
                10 => {
                    if r.operands.is_empty() {
                        (Op::Return(None), None)
                    } else {
                        (Op::Return(Some(body.value(r, &mut i)?)), None)
                    }
                }
                11 => {
                    let target = |v: u64| -> Result<usize> {
                        usize::try_from(v)
                            .ok()
                            .filter(|&b| b < body.blocks)
                            .ok_or_else(|| error(r.at, "branch target"))
                    };
                    if r.operands.len() == 1 {
                        (
                            Op::Branch {
                                target: target(op(r, 0)?)?,
                            },
                            None,
                        )
                    } else {
                        let then = target(op(r, 0)?)?;
                        let otherwise = target(op(r, 1)?)?;
                        i = 2;
                        let condition = body.value(r, &mut i)?;
                        (
                            Op::CondBranch {
                                condition,
                                then,
                                otherwise,
                            },
                            None,
                        )
                    }
                }
                12 => {
                    // [type, value, default block, (case constant, block)*]; case constants are absolute ids
                    let block = |v: u64| -> Result<usize> {
                        usize::try_from(v)
                            .ok()
                            .filter(|&b| b < body.blocks)
                            .ok_or_else(|| error(r.at, "switch target"))
                    };
                    i = 1;
                    let value = body.value(r, &mut i)?;
                    let default = block(op(r, i)?)?;
                    let pairs = &r.operands[i + 1..];
                    if pairs.len() % 2 != 0 {
                        return Err(error(r.at, "switch cases"));
                    }
                    let cases = pairs
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|p| {
                            Ok((
                                usize::try_from(p[0]).map_err(|_| error(r.at, "case value"))?,
                                block(p[1])?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    (
                        Op::Switch {
                            value,
                            default,
                            cases,
                        },
                        None,
                    )
                }
                15 => (Op::Unreachable, None),
                19 => {
                    // [allocated type, size type, size (absolute id), alignment]
                    let ty = body.m.ty(op(r, 0)?, r.at)?;
                    (Op::Alloca { ty }, Some(body.pointer_to(ty)))
                }
                20 => {
                    let (pointer, pt) = body.value_type(r, &mut i)?;
                    let _ = pt;
                    let ty = body.m.ty(op(r, i)?, r.at)?;
                    (Op::Load { pointer }, Some(ty))
                }
                44 => {
                    let (pointer, _) = body.value_type(r, &mut i)?;
                    let (value, _) = body.value_type(r, &mut i)?;
                    (Op::Store { pointer, value }, None)
                }
                // atomicrmw (LLVM 3.7): [pointer with its type, value, operation, volatile, ordering, scope]
                38 => {
                    let (pointer, _) = body.value_type(r, &mut i)?;
                    let value = body.value(r, &mut i)?;
                    let op = op(r, i)?;
                    let ty = body.m.value_types.get(value).copied();
                    let ty = match ty {
                        Some(t) if value < body.next() => t,
                        _ => return Err(error(r.at, "an atomicrmw of a value defined later")),
                    };
                    (Op::AtomicRmw { pointer, value, op }, Some(ty))
                }
                // cmpxchg: [pointer with its type, compare with its type, new value, volatile, orderings, ...]; the
                // result is a structure of the value and a flag
                46 => {
                    let (pointer, _) = body.value_type(r, &mut i)?;
                    let (compare, ty) = body.value_type(r, &mut i)?;
                    let value = body.value(r, &mut i)?;
                    let flag = body
                        .m
                        .types
                        .iter()
                        .position(|t| *t == Type::Integer(1))
                        .ok_or_else(|| error(r.at, "a cmpxchg without a boolean type"))?;
                    let pair = Type::Struct {
                        name: None,
                        elements: vec![ty, flag],
                    };
                    let result = match body.m.types.iter().position(|t| *t == pair) {
                        Some(p) => p,
                        None => {
                            body.m.types.push(pair);
                            body.m.types.len() - 1
                        }
                    };
                    (
                        Op::CmpXchg {
                            pointer,
                            compare,
                            value,
                        },
                        Some(result),
                    )
                }
                36 => (Op::Fence, None),
                43 => {
                    // [inbounds, source element type, then each operand with its type when a forward reference]
                    i = 2;
                    let source = body.m.ty(op(r, 1)?, r.at)?;
                    let (base, _) = body.value_type(r, &mut i)?;
                    let mut indices = Vec::new();
                    while i < r.operands.len() {
                        indices.push(body.value_type(r, &mut i)?.0);
                    }
                    // the result points at what the indices reach: the first steps over the pointer, the rest into
                    // arrays, vectors and structures
                    let mut t = source;
                    for (n, &index) in indices.iter().enumerate().skip(1) {
                        let _ = n;
                        t = match body.m.types[t].clone() {
                            Type::Array(_, e) | Type::Vector(_, e) => e,
                            Type::Struct { elements, .. } => {
                                let k = match body.constant_index(index) {
                                    Some(k) => k,
                                    None => {
                                        return Err(error(r.at, "a structure index that is not constant"));
                                    }
                                };
                                *elements
                                    .get(k)
                                    .ok_or_else(|| error(r.at, "getelementptr structure index"))?
                            }
                            _ => return Err(error(r.at, "getelementptr into a scalar")),
                        };
                    }
                    (Op::Gep { base, indices }, Some(body.pointer_to(t)))
                }
                other => {
                    return Err(error(
                        r.at,
                        format!("instruction {} (record {other}) is not supported", refused(other)),
                    ));
                }
            };
            let result = match ty {
                Some(t) => body.result(t, r.at)?,
                None => None,
            };
            let terminator = matches!(
                op,
                Op::Return(_)
                    | Op::Branch { .. }
                    | Op::CondBranch { .. }
                    | Op::Switch { .. }
                    | Op::Unreachable
            );
            instructions.push(Instruction {
                op,
                result,
                block: body.block,
                at: r.at,
            });
            if terminator {
                body.block += 1;
            }
        }
        if body.block != body.blocks {
            return Err(error(block.at, "basic blocks without terminators"));
        }
        let end = body.next();
        for inst in &instructions {
            match &inst.op {
                Op::Phi { incoming } if incoming.iter().any(|(v, _)| *v >= end) => {
                    return Err(error(inst.at, "phi names an undefined value"));
                }
                _ => {}
            }
        }
        let blocks = body.blocks;
        let locals = self.values.split_off(base);
        let local_types = self.value_types.split_off(base);
        Ok(Function {
            value,
            blocks,
            instructions,
            first: base,
            locals,
            local_types,
        })
    }

    /// Read a module from the bitcode's top-level blocks.
    pub fn read(top: &[Block]) -> Result<Self> {
        let module = match top {
            [m] if m.id == MODULE => m,
            _ => return Err(error(0, "bitcode is not one module block")),
        };
        let mut m = Module::default();
        let mut bodies: Vec<ValueId> = Vec::new();
        let mut version = None;
        for e in &module.elements {
            match e {
                crate::bitstream::Element::Record(r) => match r.code {
                    1 => version = Some(op(r, 0)?),
                    2..=6 | 11 | 13 | 16 => {} // triple, datalayout, asm, section, deplib, gc, source file
                    7 => {
                        // GLOBALVAR: [type, flags (bit 0 constant, bit 1 explicit type, from bit 2 the address space),
                        // initial value + 1 or 0, ...]
                        let t = m.ty(op(r, 0)?, r.at)?;
                        let flags = op(r, 1)?;
                        let (ty, value_ty) = if flags & 2 != 0 {
                            let pointer = match m.types.iter().position(|x| *x == Type::Pointer(t)) {
                                Some(p) => p,
                                None => {
                                    m.types.push(Type::Pointer(t));
                                    m.types.len() - 1
                                }
                            };
                            (pointer, t)
                        } else {
                            match &m.types[t] {
                                Type::Pointer(inner) => (t, *inner),
                                _ => return Err(error(r.at, "a global whose type is not a pointer")),
                            }
                        };
                        let init = match op(r, 2)? {
                            0 => None,
                            n => Some(usize::try_from(n - 1).map_err(|_| error(r.at, "initial value id"))?),
                        };
                        m.push(
                            Value::Global {
                                ty,
                                value_ty,
                                init,
                                constant: flags & 1 != 0,
                                address_space: u32::try_from(flags >> 2)
                                    .map_err(|_| error(r.at, "address space"))?,
                            },
                            ty,
                            r.at,
                        )?;
                    }
                    8 => {
                        // FUNCTION: [function type, cc, isproto, ...]
                        let t = m.ty(op(r, 0)?, r.at)?;
                        let pointer = match m.types.iter().position(|x| *x == Type::Pointer(t)) {
                            Some(p) => p,
                            None => {
                                m.types.push(Type::Pointer(t));
                                m.types.len() - 1
                            }
                        };
                        let declaration = op(r, 2)? != 0;
                        let v = m.push(Value::Function { ty: t, declaration }, pointer, r.at)?;
                        if !declaration {
                            bodies.push(v);
                        }
                    }
                    other => return Err(error(r.at, format!("module record {other}"))),
                },
                crate::bitstream::Element::Block(b) => match b.id {
                    BLOCKINFO | PARAMATTR | PARAMATTR_GROUP | USELIST => {}
                    TYPES => m.read_types(b)?,
                    CONSTANTS => m.read_constants(b)?,
                    METADATA => m.read_metadata(b)?,
                    VALUE_SYMTAB => m.read_symbols(b, 0)?,
                    FUNCTION => {
                        if version != Some(1) {
                            return Err(error(b.at, "only relative value ids (module version 1)"));
                        }
                        // Bodies are stored in the order of their definitions (a hull program has two).
                        if bodies.is_empty() {
                            return Err(error(b.at, "function body without a definition"));
                        }
                        let value = bodies.remove(0);
                        // A body's values are its own; the next body numbers from the module's values again.
                        let f = m.read_function(b, value)?;
                        m.functions.push(f);
                    }
                    other => return Err(error(b.at, format!("module block {other}"))),
                },
            }
        }
        if !bodies.is_empty() {
            return Err(error(module.at, "a defined function has no body"));
        }
        Ok(m)
    }
}
