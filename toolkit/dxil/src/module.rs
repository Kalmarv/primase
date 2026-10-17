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
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Function {
        ty: TypeId,
        declaration: bool,
    },
    Global {
        ty: TypeId,
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
    /// reference. Returns the value id and advances `i`.
    fn value_type(&self, r: &Record, i: &mut usize) -> Result<(ValueId, TypeId)> {
        let rel = op(r, *i)?;
        *i += 1;
        let next = self.next() as u64;
        let id = (next as u32).wrapping_sub(rel as u32) as usize;
        if id < self.next() {
            Ok((id, self.m.value_types[id]))
        } else {
            Err(error(
                r.at,
                "forward reference (only straight-line definitions before use are supported)",
            ))
        }
    }
    fn value(&self, r: &Record, i: &mut usize) -> Result<ValueId> {
        self.value_type(r, i).map(|(v, _)| v)
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
        4 | 30 | 43 => "getelementptr",
        6 => "extractelement",
        7 => "insertelement",
        8 => "shufflevector",
        12 => "switch",
        13 => "invoke",
        15 => "unreachable",
        16 => "phi",
        19 => "alloca",
        20 | 41 => "load",
        23 => "va_arg",
        24 | 44 | 42 | 45 => "store",
        31 => "indirectbr",
        36 => "fence",
        37 | 46 => "cmpxchg",
        38 => "atomicrmw",
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
                        let (a, at) = body.value_type(r, &mut i)?;
                        if at != *p {
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
            let terminator = matches!(op, Op::Return(_) | Op::Branch { .. } | Op::CondBranch { .. });
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
                // Control flow only moves forward: loops are refused.
                Op::Branch { target } if *target <= inst.block => {
                    return Err(error(inst.at, "backward branch (loops are not supported)"));
                }
                Op::CondBranch { then, otherwise, .. } if *then <= inst.block || *otherwise <= inst.block => {
                    return Err(error(inst.at, "backward branch (loops are not supported)"));
                }
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
                        // GLOBALVAR: [type, flags (bit 1: explicit type), ...]
                        let t = m.ty(op(r, 0)?, r.at)?;
                        let explicit = op(r, 1)? & 2 != 0;
                        let ty = if explicit {
                            m.types.push(Type::Pointer(t));
                            m.types.len() - 1
                        } else {
                            t
                        };
                        m.push(Value::Global { ty }, ty, r.at)?;
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
                        // Bodies are stored in reverse order of their definitions.
                        let value = bodies
                            .pop()
                            .ok_or_else(|| error(b.at, "function body without a definition"))?;
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
