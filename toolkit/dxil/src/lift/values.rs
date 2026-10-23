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
                _ => return kind(self).map(|_| unreachable!("an integer constant of another type")),
            },
            Constant::Float(bits) => match self.m.types.get(ty) {
                Some(Type::Float) => Lv::Scalar(self.b.lit_f(f32::from_bits(*bits as u32))),
                _ => return kind(self).map(|_| unreachable!("a float constant of another type")),
            },
            Constant::Null | Constant::Undef => match self.m.types.get(ty) {
                Some(Type::Float | Type::Integer(_)) => {
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
                let (a, b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
                let r = self.binary(*op, a, b)?;
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
                let r = self.cast(*op, v, to)?;
                self.define_result(result, Lv::Scalar(r))
            }
            Op::Compare { predicate, lhs, rhs } => {
                let (a, b) = (self.scalar(*lhs)?, self.scalar(*rhs)?);
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
            Some((v, _)) => self.define(v, lv),
            None => Ok(()),
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
