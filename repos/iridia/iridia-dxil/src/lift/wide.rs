//! 64-bit integers. WGSL (and WebGPU) has none, so an `i64` is carried as the pair of 32-bit words of its bits, low
//! word first (an aggregate of two unsigned scalars, which gives it locals and phis like any aggregate). Addition,
//! subtraction, multiplication, the bit operations, shifts, comparisons, the extensions and truncations and the bit
//! counts are written out on the pair; a division and a conversion to or from a float are refused by name (a division
//! would be a loop, and a conversion's rounding would be a guess).

use iridia::build::{Kind, Val};
use naga::BinaryOperator as Bo;
use naga::MathFunction as Mf;

use super::{Lift, Lv};
use crate::Result;
use dxil_reader::module::{BinaryOp, CastOp, Type, TypeId, ValueId};

/// A 64-bit integer: its low and high words.
pub(super) type Wide = (Val, Val);

impl Lift<'_> {
    /// Whether a type is a 64-bit integer.
    pub(super) fn is_wide_type(&self, ty: TypeId) -> bool {
        matches!(self.m.types.get(ty), Some(Type::Integer(64)))
    }

    /// Whether a value is a 64-bit integer.
    pub(super) fn is_wide(&self, v: ValueId) -> bool {
        let ty = if v < self.f.first {
            self.m.value_types.get(v).copied()
        } else {
            self.f.local_types.get(v - self.f.first).copied()
        };
        ty.is_some_and(|t| self.is_wide_type(t))
    }

    /// The value as a pair of words.
    pub(super) fn wide(&mut self, v: ValueId) -> Result<Wide> {
        match self.get(v)? {
            Lv::Agg(items) => match items.as_slice() {
                [Lv::Scalar(lo), Lv::Scalar(hi)] => {
                    Ok((self.b.bitcast_to(*lo, Kind::U), self.b.bitcast_to(*hi, Kind::U)))
                }
                _ => self.refuse("a 64-bit integer that is not a pair of words"),
            },
            other => self.refuse(format!("a 64-bit integer was expected, found {other:?}")),
        }
    }

    pub(super) fn pair(&self, (lo, hi): Wide) -> Lv {
        Lv::Agg(vec![Lv::Scalar(lo), Lv::Scalar(hi)])
    }

    /// The constant of 64 bits.
    pub(super) fn wide_constant(&mut self, bits: u64) -> Lv {
        let (lo, hi) = (self.b.lit_u(bits as u32), self.b.lit_u((bits >> 32) as u32));
        self.pair((lo, hi))
    }

    /// A binary operation on 64-bit integers.
    pub(super) fn binary_wide(&mut self, op: BinaryOp, a: Wide, b: Wide) -> Result<Wide> {
        Ok(match op {
            BinaryOp::And => (self.b.binary(Bo::And, a.0, b.0), self.b.binary(Bo::And, a.1, b.1)),
            BinaryOp::Or => (
                self.b.binary(Bo::InclusiveOr, a.0, b.0),
                self.b.binary(Bo::InclusiveOr, a.1, b.1),
            ),
            BinaryOp::Xor => (
                self.b.binary(Bo::ExclusiveOr, a.0, b.0),
                self.b.binary(Bo::ExclusiveOr, a.1, b.1),
            ),
            BinaryOp::Add => {
                let lo = self.b.binary(Bo::Add, a.0, b.0);
                // the low words wrapped when the sum is below an addend
                let wrapped = self.b.binary(Bo::Less, lo, a.0);
                let carry = self.carry(wrapped);
                let hi = self.b.binary(Bo::Add, a.1, b.1);
                (lo, self.b.binary(Bo::Add, hi, carry))
            }
            BinaryOp::Sub => {
                let lo = self.b.binary(Bo::Subtract, a.0, b.0);
                let borrowed = self.b.binary(Bo::Less, a.0, b.0);
                let borrow = self.carry(borrowed);
                let hi = self.b.binary(Bo::Subtract, a.1, b.1);
                (lo, self.b.binary(Bo::Subtract, hi, borrow))
            }
            BinaryOp::Mul => {
                // (a1 2^32 + a0)(b1 2^32 + b0) mod 2^64: a0 b0 in full, the cross products in the high word
                let lo = self.b.binary(Bo::Multiply, a.0, b.0);
                let carried = self.b.mul_high_unsigned(a.0, b.0);
                let c0 = self.b.binary(Bo::Multiply, a.0, b.1);
                let c1 = self.b.binary(Bo::Multiply, a.1, b.0);
                let cross = self.b.binary(Bo::Add, c0, c1);
                (lo, self.b.binary(Bo::Add, carried, cross))
            }
            BinaryOp::Shl | BinaryOp::LShr | BinaryOp::AShr => self.shift_wide(op, a, b.0),
            BinaryOp::UDiv => self.divide_wide(a, b).0,
            BinaryOp::URem => self.divide_wide(a, b).1,
            BinaryOp::SDiv | BinaryOp::SRem => self.signed_divide_wide(op == BinaryOp::SRem, a, b),
        })
    }

    /// Unsigned division of 64-bit integers by restoring long division, a loop of 64 passes: the quotient and the
    /// remainder; a zero divisor gives all ones for both, as D3D's 32-bit division does.
    fn divide_wide(&mut self, n: Wide, d: Wide) -> (Wide, Wide) {
        let u = self.b.ty_of(Kind::U, 1);
        let q = [self.b.local("q_lo", u), self.b.local("q_hi", u)];
        let r = [self.b.local("r_lo", u), self.b.local("r_hi", u)];
        let zero = self.b.lit_u(0);
        for l in q.into_iter().chain(r) {
            let p = self.b.local_ptr(l);
            self.b.store(p, zero);
        }
        let (i, counter) = self.b.begin_repeat(64);
        let sixty_three = self.b.lit_u(63);
        let k = self.b.binary(Bo::Subtract, sixty_three, i);
        // the next bit of the dividend, from the top
        let bit = self.shift_wide(BinaryOp::LShr, n, k).0;
        let one = self.b.lit_u(1);
        let bit = self.b.binary(Bo::And, bit, one);
        let load = |lift: &mut Self, l| {
            let p = lift.b.local_ptr(l);
            lift.b.load(p, Kind::U, 1)
        };
        let rest = (load(self, r[0]), load(self, r[1]));
        let doubled = self.shift_wide(BinaryOp::Shl, rest, one);
        let doubled = (self.b.binary(Bo::InclusiveOr, doubled.0, bit), doubled.1);
        // the divisor goes in when it is not above the running remainder
        let below = self
            .compare_wide(36, doubled, d)
            .expect("a predicate of the table");
        let fits = self.b.unary(naga::UnaryOperator::LogicalNot, below);
        let reduced = self
            .binary_wide(BinaryOp::Sub, doubled, d)
            .expect("a subtraction");
        let rest = self.select_wide(fits, reduced, doubled);
        let quotient = (load(self, q[0]), load(self, q[1]));
        let place = self.shift_wide(BinaryOp::Shl, (one, zero), k);
        let with = (
            self.b.binary(Bo::InclusiveOr, quotient.0, place.0),
            self.b.binary(Bo::InclusiveOr, quotient.1, place.1),
        );
        let quotient = self.select_wide(fits, with, quotient);
        for (l, v) in [
            (r[0], rest.0),
            (r[1], rest.1),
            (q[0], quotient.0),
            (q[1], quotient.1),
        ] {
            let p = self.b.local_ptr(l);
            self.b.store(p, v);
        }
        self.b.end_repeat(counter);
        let (quotient, rest) = (
            (load(self, q[0]), load(self, q[1])),
            (load(self, r[0]), load(self, r[1])),
        );
        let ones = self.b.lit_u(u32::MAX);
        let no_divisor = {
            let low = self.b.binary(Bo::Equal, d.0, zero);
            let high = self.b.binary(Bo::Equal, d.1, zero);
            self.b.binary(Bo::LogicalAnd, low, high)
        };
        (
            self.select_wide(no_divisor, (ones, ones), quotient),
            self.select_wide(no_divisor, (ones, ones), rest),
        )
    }

    /// Signed division of 64-bit integers by their magnitudes (the quotient is negative where the signs differ, the
    /// remainder takes the dividend's sign).
    fn signed_divide_wide(&mut self, remainder: bool, a: Wide, b: Wide) -> Wide {
        let zero = self.b.lit_u(0);
        let negative = |lift: &mut Self, v: Wide| {
            let signed = lift.b.bitcast(v.1, Kind::I);
            let zero = lift.b.lit_i(0);
            lift.b.binary(Bo::Less, signed, zero)
        };
        let negate = |lift: &mut Self, v: Wide| {
            lift.binary_wide(BinaryOp::Sub, (zero, zero), v)
                .expect("a subtraction")
        };
        let (a_negative, b_negative) = (negative(self, a), negative(self, b));
        let (a_negated, b_negated) = (negate(self, a), negate(self, b));
        let magnitude_a = self.select_wide(a_negative, a_negated, a);
        let magnitude_b = self.select_wide(b_negative, b_negated, b);
        let (quotient, rest) = self.divide_wide(magnitude_a, magnitude_b);
        let (result, flip) = if remainder {
            (rest, a_negative)
        } else {
            (quotient, self.b.binary(Bo::NotEqual, a_negative, b_negative))
        };
        let negated = negate(self, result);
        self.select_wide(flip, negated, result)
    }

    /// 1 where `condition` holds, else 0.
    fn carry(&mut self, condition: Val) -> Val {
        let (one, zero) = (self.b.lit_u(1), self.b.lit_u(0));
        self.b.select(condition, one, zero)
    }

    /// A shift of a 64-bit integer by the low six bits of `count`.
    fn shift_wide(&mut self, op: BinaryOp, a: Wide, count: Val) -> Wide {
        let six = self.b.lit_u(63);
        let n = self.b.binary(Bo::And, count, six);
        let (thirty_two, zero) = (self.b.lit_u(32), self.b.lit_u(0));
        let small = self.b.binary(Bo::Less, n, thirty_two);
        // the count within a word, and the word's complement (a shift by 32 is a shift by 0 in WGSL, so a zero count
        // takes none of the other word)
        let m = self.b.d3d_shift_count(n);
        let back = self.b.binary(Bo::Subtract, thirty_two, m);
        let nothing = self.b.binary(Bo::Equal, m, zero);
        let (lo, hi) = a;
        match op {
            BinaryOp::Shl => {
                let shifted = self.b.binary(Bo::ShiftLeft, lo, m);
                let spill = self.b.binary(Bo::ShiftRight, lo, back);
                let spill = self.b.select(nothing, zero, spill);
                let high = self.b.binary(Bo::ShiftLeft, hi, m);
                let high = self.b.binary(Bo::InclusiveOr, high, spill);
                (
                    self.b.select(small, shifted, zero),
                    self.b.select(small, high, shifted),
                )
            }
            BinaryOp::LShr => {
                let shifted = self.b.binary(Bo::ShiftRight, hi, m);
                let spill = self.b.binary(Bo::ShiftLeft, hi, back);
                let spill = self.b.select(nothing, zero, spill);
                let low = self.b.binary(Bo::ShiftRight, lo, m);
                let low = self.b.binary(Bo::InclusiveOr, low, spill);
                (
                    self.b.select(small, low, shifted),
                    self.b.select(small, shifted, zero),
                )
            }
            _ => {
                let signed = self.b.bitcast(hi, Kind::I);
                let m_signed = self.b.bitcast(m, Kind::U);
                let shifted = self.b.binary(Bo::ShiftRight, signed, m_signed);
                let shifted = self.b.bitcast(shifted, Kind::U);
                let spill = self.b.binary(Bo::ShiftLeft, hi, back);
                let spill = self.b.select(nothing, zero, spill);
                let low = self.b.binary(Bo::ShiftRight, lo, m);
                let low = self.b.binary(Bo::InclusiveOr, low, spill);
                let thirty_one = self.b.lit_u(31);
                let sign = self.b.binary(Bo::ShiftRight, signed, thirty_one);
                let sign = self.b.bitcast(sign, Kind::U);
                (
                    self.b.select(small, low, shifted),
                    self.b.select(small, shifted, sign),
                )
            }
        }
    }

    /// An integer comparison of 64-bit integers (LLVM's predicates 32 to 41).
    pub(super) fn compare_wide(&mut self, predicate: u64, a: Wide, b: Wide) -> Result<Val> {
        let equal_hi = self.b.binary(Bo::Equal, a.1, b.1);
        let equal_lo = self.b.binary(Bo::Equal, a.0, b.0);
        let equal = self.b.binary(Bo::LogicalAnd, equal_hi, equal_lo);
        Ok(match predicate {
            32 => equal,
            33 => self.b.unary(naga::UnaryOperator::LogicalNot, equal),
            34..=41 => {
                // the high words decide, the low words (unsigned) break a tie; a signed predicate compares the high
                // words signed
                let signed = predicate >= 38;
                let (ah, bh) = if signed {
                    (self.b.bitcast(a.1, Kind::I), self.b.bitcast(b.1, Kind::I))
                } else {
                    (a.1, b.1)
                };
                let (greater, or_equal) = match predicate {
                    34 | 38 => (true, false),
                    35 | 39 => (true, true),
                    36 | 40 => (false, false),
                    _ => (false, true),
                };
                let order = if greater { Bo::Greater } else { Bo::Less };
                let high = self.b.binary(order, ah, bh);
                let low = self.b.binary(order, a.0, b.0);
                let low = if or_equal {
                    let low_equal = self.b.binary(Bo::Equal, a.0, b.0);
                    self.b.binary(Bo::LogicalOr, low, low_equal)
                } else {
                    low
                };
                let tied = self.b.binary(Bo::LogicalAnd, equal_hi, low);
                self.b.binary(Bo::LogicalOr, high, tied)
            }
            other => return self.refuse(format!("the integer predicate {other} of 64-bit integers")),
        })
    }

    /// A cast from or to a 64-bit integer.
    pub(super) fn cast_wide(&mut self, op: CastOp, value: ValueId, to: TypeId) -> Result<Lv> {
        if self.is_wide(value) {
            let (lo, _) = self.wide(value)?;
            return Ok(match op {
                // the low bits
                CastOp::Trunc => {
                    let narrow = self.narrow_width_of_type(to);
                    let boolean = matches!(self.m.types.get(to), Some(Type::Integer(1)));
                    let v = if let Some(w) = narrow {
                        self.mask_narrow(lo, w)
                    } else if boolean {
                        let one = self.b.lit_u(1);
                        let low = self.b.binary(Bo::And, lo, one);
                        let zero = self.b.lit_u(0);
                        self.b.binary(Bo::NotEqual, low, zero)
                    } else {
                        lo
                    };
                    Lv::Scalar(v)
                }
                other => return self.refuse(format!("the cast {other:?} of a 64-bit integer")),
            });
        }
        // to 64 bits, from a boolean, a narrow integer or a word
        let from = self.narrow_width(value);
        let v = self.scalar(value)?;
        let zero = self.b.lit_u(0);
        let word = if v.kind == Kind::B {
            match op {
                CastOp::SExt => {
                    let (ones, zero) = (self.b.lit_u(u32::MAX), self.b.lit_u(0));
                    self.b.select(v, ones, zero)
                }
                _ => self.carry(v),
            }
        } else {
            self.b.bitcast_to(v, Kind::U)
        };
        Ok(match op {
            CastOp::ZExt => self.pair((word, zero)),
            CastOp::SExt => {
                let word = match from {
                    Some(w) => self.sign_extend_narrow(word, w),
                    None => word,
                };
                let signed = self.b.bitcast(word, Kind::I);
                let thirty_one = self.b.lit_u(31);
                let high = self.b.binary(Bo::ShiftRight, signed, thirty_one);
                let high = self.b.bitcast(high, Kind::U);
                self.pair((word, high))
            }
            other => return self.refuse(format!("the cast {other:?} to a 64-bit integer")),
        })
    }

    /// `select` of 64-bit integers: word by word.
    pub(super) fn select_wide(&mut self, condition: Val, then: Wide, otherwise: Wide) -> Wide {
        (
            self.b.select(condition, then.0, otherwise.0),
            self.b.select(condition, then.1, otherwise.1),
        )
    }

    /// The bit operations `dx.op` has on 64-bit integers: `Countbits` (31), `FirstbitLo` (32), `FirstbitHi` (33); each
    /// gives a 32-bit count, and the first set bit's index (all ones for none) from the low or the high end.
    pub(super) fn bits_wide(&mut self, code: u64, a: Wide) -> Result<Val> {
        let (zero, none) = (self.b.lit_u(0), self.b.lit_u(u32::MAX));
        Ok(match code {
            31 => {
                let (l, h) = (
                    self.b.math(Mf::CountOneBits, &[a.0]),
                    self.b.math(Mf::CountOneBits, &[a.1]),
                );
                self.b.binary(Bo::Add, l, h)
            }
            32 | 33 => {
                let (first, second) = if code == 32 { (a.0, a.1) } else { (a.1, a.0) };
                let index = |lift: &mut Self, word: Val| {
                    if code == 32 {
                        lift.b.math(Mf::FirstTrailingBit, &[word])
                    } else {
                        lift.b.math(Mf::CountLeadingZeros, &[word])
                    }
                };
                let thirty_two = self.b.lit_u(32);
                let in_first = index(self, first);
                let in_second = index(self, second);
                let in_second = self.b.binary(Bo::Add, in_second, thirty_two);
                let first_empty = self.b.binary(Bo::Equal, first, zero);
                let second_empty = self.b.binary(Bo::Equal, second, zero);
                let rest = self.b.select(second_empty, none, in_second);
                self.b.select(first_empty, rest, in_first)
            }
            other => return self.refuse(format!("dx.op {other} on a 64-bit integer")),
        })
    }
}
