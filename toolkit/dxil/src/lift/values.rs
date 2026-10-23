//! LLVM values and the instructions that are not `dx.op` calls: constants, arithmetic, casts, comparisons, selects,
//! aggregates, phis.

use half::f16;
use naga::{BinaryOperator as Bo, Expression as E, MathFunction as Mf, UnaryOperator as Uo};
use primase_shader::build::{Kind, Val};

use super::setup::const_int;
use super::{Lift, Lv, Slot, Stored};
use crate::Result;
use crate::module::{BinaryOp, CastOp, Constant, Instruction, Op, Type, TypeId, Value, ValueId};

impl Lift<'_> {
    /// The lanes a shuffle's constant mask picks (`None` for an undefined lane).
    fn vector_mask(&self, mask: ValueId) -> Result<Vec<Option<usize>>> {
        let value = if mask < self.f.first {
            self.m.values.get(mask)
        } else {
            self.f.locals.get(mask - self.f.first)
        };
        let ty = if mask < self.f.first {
            self.m.value_types.get(mask).copied()
        } else {
            self.f.local_types.get(mask - self.f.first).copied()
        };
        let lanes = match ty.and_then(|t| self.m.types.get(t)) {
            Some(Type::Vector(n, _)) => *n as usize,
            _ => return self.refuse("a shuffle mask that is not a vector"),
        };
        let lane = |item: Option<&Value>| match item {
            Some(Value::Constant(Constant::Integer(k))) => usize::try_from(*k).ok().map(Some),
            Some(Value::Constant(Constant::Null)) => Some(Some(0)),
            Some(Value::Constant(Constant::Undef)) => Some(None),
            _ => None,
        };
        let picks = match value {
            Some(Value::Constant(Constant::Null)) => vec![Some(Some(0)); lanes],
            Some(Value::Constant(Constant::Undef)) => vec![Some(None); lanes],
            Some(Value::Constant(Constant::Aggregate(items))) => items
                .iter()
                .map(|&i| {
                    lane(if i < self.f.first {
                        self.m.values.get(i)
                    } else {
                        self.f.locals.get(i - self.f.first)
                    })
                })
                .collect(),
            Some(Value::Constant(Constant::Data(words))) => {
                words.iter().map(|&w| Some(Some(w as usize))).collect()
            }
            _ => return self.refuse("a shuffle mask that is not constant"),
        };
        picks
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map_or_else(|| self.refuse("a shuffle mask lane that is not a number"), Ok)
    }

    /// The value of a constant of this type.
    pub(crate) fn constant(&mut self, c: &Constant, ty: TypeId) -> Result<Lv> {
        let kind = |lift: &Self| lift.kind_of_type(ty);
        // a vector constant: its lanes composed
        if let Some(Type::Vector(n, element)) = self.m.types.get(ty).cloned() {
            let lanes: Vec<Val> = match c {
                Constant::Null | Constant::Undef => {
                    let k = self.kind_of_type(element)?;
                    (0..n).map(|_| self.b.lit_bits(k, 0)).collect()
                }
                Constant::Aggregate(items) => {
                    let mut lanes = Vec::new();
                    for &item in items {
                        lanes.push(self.scalar(item)?);
                    }
                    lanes
                }
                Constant::Data(words) => {
                    let k = self.kind_of_type(element)?;
                    words.iter().map(|&w| self.b.lit_bits(k, w as u32)).collect()
                }
                other => return self.refuse(format!("a vector constant of this form ({other:?})")),
            };
            let kind = lanes.first().map_or(Kind::U, |l| l.kind);
            let lanes: Vec<Val> = lanes.into_iter().map(|l| self.b.bitcast_to(l, kind)).collect();
            return Ok(Lv::Scalar(self.b.compose(&lanes)));
        }
        Ok(match c {
            Constant::Integer(i) if self.is_wide_type(ty) => self.wide_constant(*i as u64),
            Constant::Null | Constant::Undef if self.is_wide_type(ty) => self.wide_constant(0),
            Constant::Integer(i) => match self.m.types.get(ty) {
                Some(Type::Integer(1)) => Lv::Scalar(self.b.lit_b(i & 1 != 0)),
                Some(Type::Integer(32)) => Lv::Scalar(self.b.lit_u(*i as u32)),
                Some(Type::Integer(w @ 2..=31)) => Lv::Scalar(self.b.lit_u(*i as u32 & ((1u32 << w) - 1))),
                _ => return kind(self).map(|_| unreachable!("an integer constant of another type")),
            },
            Constant::Float(bits) => match self.m.types.get(ty) {
                Some(Type::Float) => Lv::Scalar(self.b.lit_f(f32::from_bits(*bits as u32))),
                Some(Type::Half) => Lv::Scalar(self.b.lit_f(f16::from_bits(*bits as u16).to_f32())),
                _ => return kind(self).map(|_| unreachable!("a float constant of another type")),
            },
            Constant::Null | Constant::Undef => match self.m.types.get(ty) {
                Some(Type::Float | Type::Half | Type::Integer(_)) => {
                    let k = kind(self)?;
                    Lv::Scalar(self.b.lit_bits(k, 0))
                }
                Some(Type::Struct { elements, .. }) => {
                    let elements = elements.clone();
                    Lv::Agg(
                        elements
                            .iter()
                            .map(|&e| self.constant(c, e))
                            .collect::<Result<Vec<_>>>()?,
                    )
                }
                other => return self.refuse(format!("a null or undefined value of type {other:?}")),
            },
            Constant::Gep { base, indices } => {
                let (base, indices) = (*base, indices.clone());
                self.constant_gep(base, &indices)?
            }
            // a constant cast keeps a pointer (a module global seen as another type), else it is the cast made
            Constant::Cast { op, value } if self.is_wide(*value) || self.is_wide_type(ty) => {
                self.cast_wide(*op, *value, ty)?
            }
            Constant::Binary { op, lhs, rhs } if self.is_wide(*lhs) => {
                let (a, b) = (self.wide(*lhs)?, self.wide(*rhs)?);
                let r = self.binary_wide(*op, a, b)?;
                self.pair(r)
            }
            Constant::Cast { op, value } => {
                if *op == CastOp::BitCast
                    && let Lv::Pointer(p) = self.get(*value)?
                {
                    return Ok(Lv::Pointer(p));
                }
                let v = self.scalar(*value)?;
                let from = self.narrow_width(*value);
                if from.is_some() || self.narrow_width_of_type(ty).is_some() {
                    Lv::Scalar(self.cast_narrow(*op, v, from, ty)?)
                } else {
                    Lv::Scalar(self.cast(*op, v, ty)?)
                }
            }
            Constant::Binary { op, lhs, rhs } => {
                let (a, b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
                let r = match self.narrow_width(*lhs) {
                    Some(w) => self.binary_narrow(*op, a, b, w)?,
                    None => self.binary(*op, a, b)?,
                };
                Lv::Scalar(r)
            }
            other => return self.refuse(format!("a constant of this form ({other:?})")),
        })
    }

    /// A module value or a function value, translated where it is read.
    pub(crate) fn get(&mut self, v: ValueId) -> Result<Lv> {
        if v < self.f.first {
            let ty = self.m.value_types[v];
            return match &self.m.values[v] {
                Value::Constant(c) => {
                    let c = c.clone();
                    self.constant(&c, ty)
                }
                Value::Global { .. } => self.global_pointer(v),
                other => self.refuse(format!("a module value that is {other:?}")),
            };
        }
        let i = v - self.f.first;
        let ty = self.f.local_types[i];
        match self.slots[i].clone() {
            Some(Slot::Value(lv)) => Ok(lv),
            Some(Slot::Stored(stored)) => Ok(self.load_stored(&stored)),
            None => match &self.f.locals[i] {
                crate::module::Value::Constant(c) => {
                    let c = c.clone();
                    self.constant(&c, ty)
                }
                _ => self.refuse("a value read before it is defined"),
            },
        }
    }

    pub(crate) fn scalar(&mut self, v: ValueId) -> Result<Val> {
        match self.get(v)? {
            Lv::Scalar(s) => Ok(s),
            other => self.refuse(format!("a scalar was expected, found {other:?}")),
        }
    }

    pub(crate) fn float(&mut self, v: ValueId) -> Result<Val> {
        let s = self.scalar(v)?;
        Ok(self.b.bitcast_to(s, Kind::F))
    }
    pub(crate) fn uint(&mut self, v: ValueId) -> Result<Val> {
        let s = self.scalar(v)?;
        Ok(self.b.bitcast_to(s, Kind::U))
    }

    fn load_stored(&mut self, s: &Stored) -> Lv {
        match s {
            Stored::Scalar { local, kind, width } => {
                let p = self.b.local_ptr(*local);
                Lv::Scalar(self.b.load(p, *kind, *width))
            }
            Stored::Agg(items) => Lv::Agg(items.iter().map(|s| self.load_stored(s)).collect()),
            Stored::Plain(lv) => lv.clone(),
            Stored::Handle {
                class,
                resource,
                element,
            } => {
                let element = self.load_stored(element);
                let Lv::Scalar(element) = element else {
                    unreachable!("a handle's element is stored as a scalar")
                };
                Lv::Handle {
                    class: *class,
                    resource: *resource,
                    element: Some(element),
                }
            }
        }
    }

    fn store_new(&mut self, lv: &Lv, hint: &str) -> Result<Stored> {
        Ok(match lv {
            Lv::Scalar(v) => {
                let ty = self.b.ty_of(v.kind, v.width);
                let local = self.b.local(hint, ty);
                let p = self.b.local_ptr(local);
                self.b.store(p, *v);
                Stored::Scalar {
                    local,
                    kind: v.kind,
                    width: v.width,
                }
            }
            Lv::Agg(items) => Stored::Agg(
                items
                    .iter()
                    .map(|i| self.store_new(i, hint))
                    .collect::<Result<Vec<_>>>()?,
            ),
            // a query is a local of the function, the same wherever it is read
            Lv::RayQuery(..) => Stored::Plain(lv.clone()),
            Lv::Handle {
                class,
                resource,
                element: Some(element),
            } => Stored::Handle {
                class: *class,
                resource: *resource,
                element: Box::new(self.store_new(&Lv::Scalar(*element), hint)?),
            },
            Lv::Handle { element: None, .. } => Stored::Plain(lv.clone()),
            Lv::Pointer(p) => {
                let p = self.spill_pointer(p)?;
                Stored::Plain(Lv::Pointer(p))
            }
        })
    }

    /// Define a function value: kept as the expressions of its block, or in locals when another block reads it.
    pub(crate) fn define(&mut self, v: ValueId, lv: Lv) -> Result<()> {
        let i = v - self.f.first;
        let slot = if self.cross[i] {
            let stored = self.store_new(&lv, &format!("v{i}"))?;
            Slot::Stored(stored)
        } else {
            Slot::Value(lv)
        };
        self.slots[i] = Some(slot);
        Ok(())
    }

    /// One local per phi, before any code.
    pub(crate) fn define_phis(&mut self) -> Result<()> {
        for inst in &self.f.instructions {
            let (Op::Phi { .. }, Some((v, ty))) = (&inst.op, inst.result) else {
                continue;
            };
            self.at = inst.at;
            let name = format!("phi{}", v - self.f.first);
            let t = self.b.ty_of(Kind::U, 1);
            let stored = if self.is_wide_type(ty) {
                let (lo, hi) = (
                    self.b.local(&format!("{name}_lo"), t),
                    self.b.local(&format!("{name}_hi"), t),
                );
                let word = |local| Stored::Scalar {
                    local,
                    kind: Kind::U,
                    width: 1,
                };
                Stored::Agg(vec![word(lo), word(hi)])
            } else {
                let kind = self.kind_of_type(ty)?;
                let t = self.b.ty_of(kind, 1);
                let local = self.b.local(&name, t);
                Stored::Scalar {
                    local,
                    kind,
                    width: 1,
                }
            };
            self.slots[v - self.f.first] = Some(Slot::Stored(stored));
        }
        Ok(())
    }

    /// Store the incoming values of the phis of block `to` for the edge from block `from`.
    pub(crate) fn phi_copies(&mut self, from: usize, to: usize) -> Result<()> {
        let mut pending = Vec::new();
        for &i in &self.blocks[to].clone() {
            let inst = &self.f.instructions[i];
            let Op::Phi { incoming } = &inst.op else { break };
            self.at = inst.at;
            let (v, _) = inst.result.expect("a phi has a result");
            let Some((value, _)) = incoming.iter().find(|(_, b)| *b == from) else {
                return self.refuse("a phi without a value for one of its predecessors");
            };
            let value = if self.is_wide(*value) {
                let (lo, hi) = self.wide(*value)?;
                vec![lo, hi]
            } else {
                vec![self.scalar(*value)?]
            };
            pending.push((v, value));
        }
        for (v, values) in pending {
            let locals = match self.slots[v - self.f.first].clone() {
                Some(Slot::Stored(Stored::Scalar { local, kind, .. })) => vec![(local, kind)],
                Some(Slot::Stored(Stored::Agg(words))) => {
                    let mut locals = Vec::new();
                    for word in words {
                        let Stored::Scalar { local, kind, .. } = word else {
                            return self.refuse("a phi without its local");
                        };
                        locals.push((local, kind));
                    }
                    locals
                }
                _ => return self.refuse("a phi without its local"),
            };
            for ((local, kind), value) in locals.into_iter().zip(values) {
                let value = self.b.bitcast_to(value, kind);
                let p = self.b.local_ptr(local);
                self.b.store(p, value);
            }
        }
        Ok(())
    }

    pub(crate) fn instruction(&mut self, inst: &Instruction) -> Result<()> {
        self.lift_instruction(inst)?;
        self.b.check().map_err(|detail| crate::error(inst.at, detail))
    }

    fn lift_instruction(&mut self, inst: &Instruction) -> Result<()> {
        self.at = inst.at;
        let result = inst.result;
        match &inst.op {
            Op::Binary { op, lhs, rhs } if self.is_wide(*lhs) => {
                let (a, b) = (self.wide(*lhs)?, self.wide(*rhs)?);
                let r = self.binary_wide(*op, a, b)?;
                let r = self.pair(r);
                self.define_result(result, r)
            }
            Op::Binary { op, lhs, rhs } => {
                let (mut a, mut b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
                // an integer operation on a value carried as a float (memory of one kind read as another) is on its bits
                let lhs_type = if *lhs < self.f.first {
                    self.m.value_types.get(*lhs).copied()
                } else {
                    self.f.local_types.get(*lhs - self.f.first).copied()
                };
                let integer = matches!(lhs_type.and_then(|t| self.m.types.get(t)), Some(Type::Integer(_)));
                if integer && a.kind == Kind::F {
                    a = self.b.bitcast_to(a, Kind::U);
                    b = self.b.bitcast_to(b, Kind::U);
                }
                let r = match self.narrow_width(*lhs) {
                    Some(w) => self.binary_narrow(*op, a, b, w)?,
                    None => self.binary(*op, a, b)?,
                };
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Cast { op, value }
                if self.is_wide(*value) || result.is_some_and(|(_, t)| self.is_wide_type(t)) =>
            {
                let to = result.map(|(_, t)| t).expect("a cast has a result");
                let r = self.cast_wide(*op, *value, to)?;
                self.define_result(result, r)
            }
            Op::Cast { op, value } => {
                // a pointer cast (dxc casts a local array to bytes for its lifetime markers) keeps the pointer
                if *op == CastOp::BitCast
                    && let Lv::Pointer(p) = self.get(*value)?
                {
                    return self.define_result(result, Lv::Pointer(p));
                }
                let v = self.scalar(*value)?;
                let to = result.map(|(_, t)| t).expect("a cast has a result");
                let from = self.narrow_width(*value);
                let r = if from.is_some() || self.narrow_width_of_type(to).is_some() {
                    self.cast_narrow(*op, v, from, to)?
                } else {
                    self.cast(*op, v, to)?
                };
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Compare { predicate, lhs, rhs } if self.is_wide(*lhs) => {
                let (a, b) = (self.wide(*lhs)?, self.wide(*rhs)?);
                let r = self.compare_wide(*predicate, a, b)?;
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Compare { predicate, lhs, rhs } => {
                let (mut a, mut b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
                // a signed comparison of narrow integers is on their sign extensions
                if let Some(w) = self.narrow_width(*lhs)
                    && (38..=41).contains(predicate)
                {
                    a = self.sign_extend_narrow(a, w);
                    b = self.sign_extend_narrow(b, w);
                }
                let r = self.compare(*predicate, a, b)?;
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Select {
                condition,
                then,
                otherwise,
            } if self.is_wide(*then) => {
                let c = self.scalar(*condition)?;
                let (t, o) = (self.wide(*then)?, self.wide(*otherwise)?);
                let r = self.select_wide(c, t, o);
                let r = self.pair(r);
                self.define_result(result, r)
            }
            Op::Select {
                condition,
                then,
                otherwise,
            } => {
                let c = self.scalar(*condition)?;
                let (t, o) = (self.scalar(*then)?, self.scalar(*otherwise)?);
                let o = self.b.bitcast_to(o, t.kind);
                let r = self.b.select(c, t, o);
                self.define_result(result, Lv::Scalar(r))
            }
            Op::ExtractElement { vector, index } => {
                let v = self.scalar(*vector)?;
                let r = match const_int(self.m, self.f, *index) {
                    Some(k) if (0..i64::from(v.width)).contains(&k) => self.b.lane(v, k as u8),
                    Some(_) => return self.refuse("a vector lane outside the vector"),
                    None => {
                        let at = self.uint(*index)?;
                        Val {
                            expr: self.b.app(E::Access {
                                base: v.expr,
                                index: at.expr,
                            }),
                            kind: v.kind,
                            width: 1,
                        }
                    }
                };
                self.define_result(result, Lv::Scalar(r))
            }
            Op::InsertElement { vector, value, index } => {
                let v = self.scalar(*vector)?;
                let new = self.scalar(*value)?;
                let new = self.b.bitcast_to(new, v.kind);
                let lanes: Vec<Val> = (0..v.width).map(|k| self.b.lane(v, k)).collect();
                let lanes = match const_int(self.m, self.f, *index) {
                    Some(k) if (0..i64::from(v.width)).contains(&k) => {
                        let mut lanes = lanes;
                        lanes[k as usize] = new;
                        lanes
                    }
                    Some(_) => return self.refuse("a vector lane outside the vector"),
                    None => {
                        let at = self.uint(*index)?;
                        let mut replaced = Vec::new();
                        for (k, lane) in lanes.into_iter().enumerate() {
                            let here = self.b.lit_u(k as u32);
                            let is_here = self.b.binary(Bo::Equal, at, here);
                            replaced.push(self.b.select(is_here, new, lane));
                        }
                        replaced
                    }
                };
                let r = self.b.compose(&lanes);
                self.define_result(result, Lv::Scalar(r))
            }
            Op::ShuffleVector { first, second, mask } => {
                let (a, b) = (self.scalar(*first)?, self.scalar(*second)?);
                let b = self.b.bitcast_to(b, a.kind);
                let mut source: Vec<Val> = (0..a.width).map(|k| self.b.lane(a, k)).collect();
                source.extend((0..b.width).map(|k| self.b.lane(b, k)));
                let picks = self.vector_mask(*mask)?;
                let mut lanes = Vec::new();
                for pick in picks {
                    match pick {
                        Some(k) if k < source.len() => lanes.push(source[k]),
                        // an undefined lane may be anything
                        None => lanes.push(source[0]),
                        Some(_) => return self.refuse("a shuffle that picks a lane beyond both vectors"),
                    }
                }
                let r = self.b.compose(&lanes);
                self.define_result(result, Lv::Scalar(r))
            }
            Op::ExtractValue { aggregate, indices } => {
                let mut lv = self.get(*aggregate)?;
                for &k in indices {
                    lv = match lv {
                        Lv::Agg(mut items) if (k as usize) < items.len() => items.swap_remove(k as usize),
                        _ => return self.refuse("extractvalue of something that is not an aggregate"),
                    };
                }
                self.define_result(result, lv)
            }
            Op::InsertValue {
                aggregate,
                value,
                indices,
            } => {
                let mut agg = self.get(*aggregate)?;
                let new = self.get(*value)?;
                fn put(agg: &mut Lv, path: &[u64], new: Lv) -> bool {
                    match (agg, path) {
                        (slot, []) => {
                            *slot = new;
                            true
                        }
                        (Lv::Agg(items), [k, rest @ ..]) => match items.get_mut(*k as usize) {
                            Some(item) => put(item, rest, new),
                            None => false,
                        },
                        _ => false,
                    }
                }
                if !put(&mut agg, indices, new) {
                    return self.refuse("insertvalue into something that is not an aggregate");
                }
                self.define_result(result, agg)
            }
            Op::Call { function, args } => self.call(inst, *function, args),
            Op::Alloca { .. } | Op::Load { .. } | Op::Store { .. } | Op::Gep { .. } => {
                self.memory_instruction(inst)
            }
            Op::AtomicRmw { pointer, value, op } => self.atomic_rmw(inst, *pointer, *value, *op),
            Op::CmpXchg {
                pointer,
                compare,
                value,
            } => self.cmp_xchg(inst, *pointer, *compare, *value),
            // invocations run in step, so there is no ordering to enforce
            Op::Fence => Ok(()),
            // flow is the structurizer's, phis the edges'
            Op::Phi { .. }
            | Op::Branch { .. }
            | Op::CondBranch { .. }
            | Op::Switch { .. }
            | Op::Return(_)
            | Op::Unreachable => Ok(()),
        }
    }

    pub(crate) fn define_result(&mut self, result: Option<(ValueId, TypeId)>, lv: Lv) -> Result<()> {
        match result {
            Some((v, ty)) => {
                let lv = self.quantized(lv, ty);
                self.define(v, lv)
            }
            None => Ok(()),
        }
    }

    /// A value of type `ty` as the half it is: a half is carried as a float, so each half made is rounded to the
    /// nearest half where it is defined (the elements of a structure of halves, a sample's lanes, one by one).
    fn quantized(&mut self, lv: Lv, ty: TypeId) -> Lv {
        match (lv, self.m.types.get(ty).cloned()) {
            (Lv::Scalar(v), Some(Type::Half)) => {
                self.notes.insert(
                    "a half is computed as a 32-bit float and rounded to a half after each operation: exact for \
                     addition, subtraction, multiplication, division and square root, a fused multiply-add may \
                     differ in the last bit where the two roundings disagree, and the rounding itself is the \
                     implementation's (quantizeToF16)"
                        .into(),
                );
                Lv::Scalar(self.b.math(Mf::QuantizeToF16, &[v]))
            }
            (Lv::Agg(items), Some(Type::Struct { elements, .. })) if items.len() == elements.len() => {
                Lv::Agg(
                    items
                        .into_iter()
                        .zip(elements)
                        .map(|(item, e)| self.quantized(item, e))
                        .collect(),
                )
            }
            (lv, _) => lv,
        }
    }

    fn binary(&mut self, op: BinaryOp, a: Val, b: Val) -> Result<Val> {
        if a.kind == Kind::F {
            let b = self.b.bitcast_to(b, Kind::F);
            return Ok(match op {
                BinaryOp::Add => self.b.binary(Bo::Add, a, b),
                BinaryOp::Sub => self.b.binary(Bo::Subtract, a, b),
                BinaryOp::Mul => self.b.binary(Bo::Multiply, a, b),
                BinaryOp::UDiv | BinaryOp::SDiv => self.b.binary(Bo::Divide, a, b),
                BinaryOp::URem | BinaryOp::SRem => self.b.binary(Bo::Modulo, a, b),
                other => return self.refuse(format!("float {other:?}")),
            });
        }
        if a.kind == Kind::B {
            let b = self.b.bitcast_to(b, Kind::B);
            return Ok(match op {
                BinaryOp::And => self.b.binary(Bo::LogicalAnd, a, b),
                BinaryOp::Or => self.b.binary(Bo::LogicalOr, a, b),
                BinaryOp::Xor | BinaryOp::Add | BinaryOp::Sub => self.b.binary(Bo::NotEqual, a, b),
                other => return self.refuse(format!("boolean {other:?}")),
            });
        }
        let a = self.b.bitcast_to(a, Kind::U);
        let b = self.b.bitcast_to(b, Kind::U);
        Ok(match op {
            BinaryOp::Add => self.b.binary(Bo::Add, a, b),
            BinaryOp::Sub => self.b.binary(Bo::Subtract, a, b),
            BinaryOp::Mul => self.b.binary(Bo::Multiply, a, b),
            BinaryOp::And => self.b.binary(Bo::And, a, b),
            BinaryOp::Or => self.b.binary(Bo::InclusiveOr, a, b),
            BinaryOp::Xor => self.b.binary(Bo::ExclusiveOr, a, b),
            BinaryOp::Shl => {
                let c = self.b.d3d_shift_count(b);
                self.b.binary(Bo::ShiftLeft, a, c)
            }
            BinaryOp::LShr => {
                let c = self.b.d3d_shift_count(b);
                self.b.binary(Bo::ShiftRight, a, c)
            }
            BinaryOp::AShr => {
                let c = self.b.d3d_shift_count(b);
                let signed = self.b.bitcast(a, Kind::I);
                let r = self.b.binary(Bo::ShiftRight, signed, c);
                self.b.bitcast(r, Kind::U)
            }
            BinaryOp::UDiv => self.b.d3d_unsigned_divide(a, b).0,
            BinaryOp::URem => self.b.d3d_unsigned_divide(a, b).1,
            BinaryOp::SDiv | BinaryOp::SRem => {
                // by magnitudes, so the executor and every API divide the same way; a zero divisor gives all ones
                // as the unsigned division does
                let (ai, bi) = (self.b.bitcast(a, Kind::I), self.b.bitcast(b, Kind::I));
                let zi = self.b.lit_i(0);
                let a_negative = self.b.binary(Bo::Less, ai, zi);
                let b_negative = self.b.binary(Bo::Less, bi, zi);
                let zero = self.b.lit_u(0);
                let (na, nb) = (
                    self.b.binary(Bo::Subtract, zero, a),
                    self.b.binary(Bo::Subtract, zero, b),
                );
                let (ma, mb) = (self.b.select(a_negative, na, a), self.b.select(b_negative, nb, b));
                let (q, r) = self.b.d3d_unsigned_divide(ma, mb);
                let magnitude = if op == BinaryOp::SDiv { q } else { r };
                let negated = self.b.binary(Bo::Subtract, zero, magnitude);
                let flip = if op == BinaryOp::SDiv {
                    self.b.binary(Bo::NotEqual, a_negative, b_negative)
                } else {
                    a_negative
                };
                let signed = self.b.select(flip, negated, magnitude);
                let by_zero = self.b.binary(Bo::Equal, b, zero);
                let ones = self.b.lit_u(u32::MAX);
                self.b.select(by_zero, ones, signed)
            }
        })
    }

    /// The width of a value that is an integer narrower than 32 bits and wider than a boolean (16 bits for a half's
    /// sibling, 4 for a bit-field cut by the compiler), which is carried as the 32-bit unsigned integer of its bits,
    /// zero-extended.
    pub(super) fn narrow_width(&self, v: ValueId) -> Option<u32> {
        let ty = if v < self.f.first {
            self.m.value_types.get(v).copied()
        } else {
            self.f.local_types.get(v - self.f.first).copied()
        };
        self.narrow_width_of_type(ty?)
    }

    pub(super) fn narrow_width_of_type(&self, ty: TypeId) -> Option<u32> {
        match self.m.types.get(ty) {
            Some(Type::Integer(w @ 2..=31)) => Some(*w),
            _ => None,
        }
    }

    /// The low `width` bits of a 32-bit value, zero-extended: a narrow integer's carried form.
    pub(super) fn mask_narrow(&mut self, v: Val, width: u32) -> Val {
        let v = self.b.bitcast_to(v, Kind::U);
        let mask = self.b.lit_u((1u32 << width) - 1);
        self.b.binary(Bo::And, v, mask)
    }

    /// The sign extension of a carried narrow integer to 32 bits, as the same bits.
    pub(super) fn sign_extend_narrow(&mut self, v: Val, width: u32) -> Val {
        let v = self.b.bitcast_to(v, Kind::U);
        let shift = self.b.lit_u(32 - width);
        let high = self.b.binary(Bo::ShiftLeft, v, shift);
        let signed = self.b.bitcast(high, Kind::I);
        let back = self.b.binary(Bo::ShiftRight, signed, shift);
        self.b.bitcast(back, Kind::U)
    }

    /// A binary operation on narrow integers: the 32-bit operation, signed ones on sign extensions, the result cut
    /// back to the width.
    fn binary_narrow(&mut self, op: BinaryOp, a: Val, b: Val, width: u32) -> Result<Val> {
        let (a, b) = (self.b.bitcast_to(a, Kind::U), self.b.bitcast_to(b, Kind::U));
        let (a, b) = if matches!(op, BinaryOp::AShr | BinaryOp::SDiv | BinaryOp::SRem) {
            (self.sign_extend_narrow(a, width), b)
        } else {
            (a, b)
        };
        let b = if matches!(op, BinaryOp::SDiv | BinaryOp::SRem) {
            self.sign_extend_narrow(b, width)
        } else {
            b
        };
        let r = self.binary(op, a, b)?;
        Ok(self.mask_narrow(r, width))
    }

    /// A cast from a narrow integer (of width `from`) or to one (the type `to`).
    fn cast_narrow(&mut self, op: CastOp, v: Val, from: Option<u32>, to: TypeId) -> Result<Val> {
        let v = if v.kind == Kind::F {
            v
        } else {
            self.b.bitcast_to(v, Kind::U)
        };
        let target = self.narrow_width_of_type(to);
        Ok(match (op, from, target) {
            (CastOp::ZExt, Some(_), _) => v,
            (CastOp::SExt, Some(w), _) => {
                let extended = self.sign_extend_narrow(v, w);
                match target {
                    Some(t) => self.mask_narrow(extended, t),
                    None => extended,
                }
            }
            (CastOp::Trunc, _, Some(t)) => self.mask_narrow(v, t),
            (CastOp::UiToFp, Some(_), _) => self.b.convert(v, Kind::F),
            (CastOp::SiToFp, Some(w), _) => {
                let s = self.sign_extend_narrow(v, w);
                let i = self.b.bitcast_to(s, Kind::I);
                self.b.convert(i, Kind::F)
            }
            (CastOp::FpToUi, None, Some(t)) => {
                let u = self.b.d3d_float_to_uint(v);
                self.mask_narrow(u, t)
            }
            (CastOp::FpToSi, None, Some(t)) => {
                let i = self.b.d3d_float_to_int(v);
                let u = self.b.bitcast(i, Kind::U);
                self.mask_narrow(u, t)
            }
            (CastOp::BitCast, _, _) => {
                let kind = self.kind_of_type(to)?;
                self.b.bitcast_to(v, kind)
            }
            // a truncation to a boolean
            (CastOp::Trunc, Some(_), None) => self.cast(op, v, to)?,
            (other, ..) => return self.refuse(format!("the cast {other:?} of a narrow integer")),
        })
    }

    fn cast(&mut self, op: CastOp, v: Val, to: TypeId) -> Result<Val> {
        let target = self.kind_of_type(to)?;
        Ok(match op {
            CastOp::BitCast => self.b.bitcast_to(v, target),
            CastOp::ZExt | CastOp::SExt => {
                if v.kind != Kind::B {
                    return self.refuse("a zero or sign extension from something that is not a boolean");
                }
                let on = self.b.lit_u(if op == CastOp::SExt { u32::MAX } else { 1 });
                let off = self.b.lit_u(0);
                self.b.select(v, on, off)
            }
            CastOp::Trunc => {
                if target != Kind::B {
                    return self.refuse("a truncation to something that is not a boolean");
                }
                let u = self.b.bitcast_to(v, Kind::U);
                let one = self.b.lit_u(1);
                let low = self.b.binary(Bo::And, u, one);
                let zero = self.b.lit_u(0);
                self.b.binary(Bo::NotEqual, low, zero)
            }
            // a half is a float carried at 32 bits: widening changes nothing, narrowing is the rounding `define_result` does
            CastOp::FpExt | CastOp::FpTrunc => v,
            CastOp::FpToUi => self.b.d3d_float_to_uint(v),
            CastOp::FpToSi => {
                let i = self.b.d3d_float_to_int(v);
                self.b.bitcast(i, Kind::U)
            }
            CastOp::UiToFp => {
                let u = self.b.bitcast_to(v, Kind::U);
                self.b.convert(u, Kind::F)
            }
            CastOp::SiToFp => {
                let i = self.b.bitcast_to(v, Kind::I);
                self.b.convert(i, Kind::F)
            }
            other => return self.refuse(format!("the cast {other:?}")),
        })
    }

    fn compare(&mut self, predicate: u64, a: Val, b: Val) -> Result<Val> {
        let _ = (E::Literal(naga::Literal::Bool(true)), Mf::Abs, Uo::Negate);
        if predicate < 32 {
            let (a, b) = (self.b.bitcast_to(a, Kind::F), self.b.bitcast_to(b, Kind::F));
            let not = |lift: &mut Self, v: Val| lift.b.unary(Uo::LogicalNot, v);
            return Ok(match predicate {
                0 => self.b.lit_b(false),
                1 => self.b.binary(Bo::Equal, a, b),
                2 => self.b.binary(Bo::Greater, a, b),
                3 => self.b.binary(Bo::GreaterEqual, a, b),
                4 => self.b.binary(Bo::Less, a, b),
                5 => self.b.binary(Bo::LessEqual, a, b),
                6 => {
                    let (l, g) = (self.b.binary(Bo::Less, a, b), self.b.binary(Bo::Greater, a, b));
                    self.b.binary(Bo::LogicalOr, l, g)
                }
                7 | 8 => {
                    let (na, nb) = (self.b.is_nan(a), self.b.is_nan(b));
                    let unordered = self.b.binary(Bo::LogicalOr, na, nb);
                    if predicate == 8 {
                        unordered
                    } else {
                        not(self, unordered)
                    }
                }
                9 => {
                    let (l, g) = (self.b.binary(Bo::Less, a, b), self.b.binary(Bo::Greater, a, b));
                    let one = self.b.binary(Bo::LogicalOr, l, g);
                    not(self, one)
                }
                10 => {
                    let c = self.b.binary(Bo::LessEqual, a, b);
                    not(self, c)
                }
                11 => {
                    let c = self.b.binary(Bo::Less, a, b);
                    not(self, c)
                }
                12 => {
                    let c = self.b.binary(Bo::GreaterEqual, a, b);
                    not(self, c)
                }
                13 => {
                    let c = self.b.binary(Bo::Greater, a, b);
                    not(self, c)
                }
                14 => self.b.binary(Bo::NotEqual, a, b),
                15 => self.b.lit_b(true),
                other => return self.refuse(format!("the float predicate {other}")),
            });
        }
        let boolean = a.kind == Kind::B;
        let (x, y) = if boolean {
            (a, self.b.bitcast_to(b, Kind::B))
        } else {
            (self.b.bitcast_to(a, Kind::U), self.b.bitcast_to(b, Kind::U))
        };
        let signed = |lift: &mut Self, op: Bo| {
            let (sx, sy) = (lift.b.bitcast(x, Kind::I), lift.b.bitcast(y, Kind::I));
            lift.b.binary(op, sx, sy)
        };
        Ok(match predicate {
            32 => self.b.binary(Bo::Equal, x, y),
            33 => self.b.binary(Bo::NotEqual, x, y),
            34 => self.b.binary(Bo::Greater, x, y),
            35 => self.b.binary(Bo::GreaterEqual, x, y),
            36 => self.b.binary(Bo::Less, x, y),
            37 => self.b.binary(Bo::LessEqual, x, y),
            38 => signed(self, Bo::Greater),
            39 => signed(self, Bo::GreaterEqual),
            40 => signed(self, Bo::Less),
            41 => signed(self, Bo::LessEqual),
            other => return self.refuse(format!("the integer predicate {other}")),
        })
    }
}
