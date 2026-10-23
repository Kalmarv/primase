//! LLVM values and the instructions that are not `dx.op` calls: constants, arithmetic, casts, comparisons, selects,
//! aggregates, phis.

use naga::{BinaryOperator as Bo, Expression as E, MathFunction as Mf, UnaryOperator as Uo};
use primase_shader::build::{Kind, Val};

use super::{Lift, Lv, Slot, Stored};
use crate::Result;
use crate::module::{BinaryOp, CastOp, Constant, Instruction, Op, Type, TypeId, Value, ValueId};

impl Lift<'_> {
    /// The value of a constant of this type.
    pub(crate) fn constant(&mut self, c: &Constant, ty: TypeId) -> Result<Lv> {
        let kind = |lift: &Self| lift.kind_of_type(ty);
        Ok(match c {
            Constant::Integer(i) => match self.m.types.get(ty) {
                Some(Type::Integer(1)) => Lv::Scalar(self.b.lit_b(i & 1 != 0)),
                Some(Type::Integer(32)) => Lv::Scalar(self.b.lit_u(*i as u32)),
                Some(Type::Integer(16)) => Lv::Scalar(self.b.lit_u(*i as u32 & 0xffff)),
                _ => return kind(self).map(|_| unreachable!("an integer constant of another type")),
            },
            Constant::Float(bits) => match self.m.types.get(ty) {
                Some(Type::Float) => Lv::Scalar(self.b.lit_f(f32::from_bits(*bits as u32))),
                Some(Type::Half) => Lv::Scalar(self.b.lit_f(half_to_f32(*bits as u16))),
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
            let kind = self.kind_of_type(ty)?;
            let t = self.b.ty_of(kind, 1);
            let local = self.b.local(&format!("phi{}", v - self.f.first), t);
            self.slots[v - self.f.first] = Some(Slot::Stored(Stored::Scalar {
                local,
                kind,
                width: 1,
            }));
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
            let value = self.scalar(*value)?;
            pending.push((v, value));
        }
        for (v, value) in pending {
            let Some(Slot::Stored(Stored::Scalar { local, kind, .. })) = self.slots[v - self.f.first].clone()
            else {
                return self.refuse("a phi without its local");
            };
            let value = self.b.bitcast_to(value, kind);
            let p = self.b.local_ptr(local);
            self.b.store(p, value);
        }
        Ok(())
    }

    pub(crate) fn instruction(&mut self, inst: &Instruction) -> Result<()> {
        self.at = inst.at;
        let result = inst.result;
        match &inst.op {
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
                let r = if self.is_sixteen(*lhs) {
                    self.binary_sixteen(*op, a, b)?
                } else {
                    self.binary(*op, a, b)?
                };
                self.define_result(result, Lv::Scalar(r))
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
                let r = if self.is_sixteen(*value) || matches!(self.m.types.get(to), Some(Type::Integer(16)))
                {
                    self.cast_sixteen(*op, v, self.is_sixteen(*value), to)?
                } else {
                    self.cast(*op, v, to)?
                };
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Compare { predicate, lhs, rhs } => {
                let (mut a, mut b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
                // a signed comparison of 16-bit integers is on their sign extensions
                if self.is_sixteen(*lhs) && (38..=41).contains(predicate) {
                    a = self.sign_extend_sixteen(a);
                    b = self.sign_extend_sixteen(b);
                }
                let r = self.compare(*predicate, a, b)?;
                self.define_result(result, Lv::Scalar(r))
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

    /// Whether a value is a 16-bit integer.
    fn is_sixteen(&self, v: ValueId) -> bool {
        let ty = if v < self.f.first {
            self.m.value_types.get(v).copied()
        } else {
            self.f.local_types.get(v - self.f.first).copied()
        };
        matches!(ty.and_then(|t| self.m.types.get(t)), Some(Type::Integer(16)))
    }

    /// The low 16 bits of a 32-bit value, zero-extended: a 16-bit integer's carried form.
    fn mask_sixteen(&mut self, v: Val) -> Val {
        let v = self.b.bitcast_to(v, Kind::U);
        let mask = self.b.lit_u(0xffff);
        self.b.binary(Bo::And, v, mask)
    }

    /// The sign extension of a carried 16-bit integer to 32 bits, as the same bits.
    fn sign_extend_sixteen(&mut self, v: Val) -> Val {
        let v = self.b.bitcast_to(v, Kind::U);
        let sixteen = self.b.lit_u(16);
        let high = self.b.binary(Bo::ShiftLeft, v, sixteen);
        let signed = self.b.bitcast(high, Kind::I);
        let back = self.b.binary(Bo::ShiftRight, signed, sixteen);
        self.b.bitcast(back, Kind::U)
    }

    /// A binary operation on 16-bit integers: the 32-bit operation, signed ones on sign extensions, the result cut
    /// back to 16 bits.
    fn binary_sixteen(&mut self, op: BinaryOp, a: Val, b: Val) -> Result<Val> {
        let (a, b) = (self.b.bitcast_to(a, Kind::U), self.b.bitcast_to(b, Kind::U));
        let (a, b) = if matches!(op, BinaryOp::AShr | BinaryOp::SDiv | BinaryOp::SRem) {
            (self.sign_extend_sixteen(a), b)
        } else {
            (a, b)
        };
        let b = if matches!(op, BinaryOp::SDiv | BinaryOp::SRem) {
            self.sign_extend_sixteen(b)
        } else {
            b
        };
        let r = self.binary(op, a, b)?;
        Ok(self.mask_sixteen(r))
    }

    /// A cast from or to a 16-bit integer.
    fn cast_sixteen(&mut self, op: CastOp, v: Val, from_sixteen: bool, to: TypeId) -> Result<Val> {
        let v = if v.kind == Kind::F {
            v
        } else {
            self.b.bitcast_to(v, Kind::U)
        };
        Ok(match (op, from_sixteen) {
            (CastOp::ZExt, true) => v,
            (CastOp::SExt, true) => self.sign_extend_sixteen(v),
            (CastOp::Trunc, false) => self.mask_sixteen(v),
            (CastOp::UiToFp, true) => self.b.convert(v, Kind::F),
            (CastOp::SiToFp, true) => {
                let s = self.sign_extend_sixteen(v);
                let i = self.b.bitcast_to(s, Kind::I);
                self.b.convert(i, Kind::F)
            }
            (CastOp::FpToUi, false) => {
                let u = self.b.d3d_float_to_uint(v);
                self.mask_sixteen(u)
            }
            (CastOp::FpToSi, false) => {
                let i = self.b.d3d_float_to_int(v);
                let u = self.b.bitcast(i, Kind::U);
                self.mask_sixteen(u)
            }
            (CastOp::BitCast, _) => {
                let kind = self.kind_of_type(to)?;
                self.b.bitcast_to(v, kind)
            }
            (other, _) => return self.refuse(format!("the cast {other:?} of a 16-bit integer")),
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

/// The value of an IEEE half, from its bits.
fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 31);
    let fraction = f32::from(bits & 1023);
    sign * match exponent {
        0 => fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + fraction / 1024.0) * 2f32.powi(e - 15),
    }
}
