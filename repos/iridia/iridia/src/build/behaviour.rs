//! Direct3D's results where they differ from other APIs' and from naga's own, written out as plain operations so the
//! program means one thing everywhere (see [`crate::D3dModes`]). Each function states the D3D result it implements;
//! the DXBC and DXIL lifters share them.
//!
//! A NaN is tested on its bits, never as `x != x`: drivers that assume finite math fold that comparison away, which
//! the spike measured on a real GPU (a float to integer guard that vanished).

use naga::{BinaryOperator as Bo, MathFunction as Mf};

use super::{Builder, Kind, Val};

impl Builder {
    /// Whether a float is NaN, by its bits: the exponent all ones and a nonzero mantissa.
    pub fn is_nan(&mut self, v: Val) -> Val {
        let bits = self.bitcast(v, Kind::U);
        let abs = self.lit_u(0x7fff_ffff);
        let infinity = self.lit_u(0x7f80_0000);
        let magnitude = self.binary(Bo::And, bits, abs);
        self.binary(Bo::Greater, magnitude, infinity)
    }

    /// D3D `saturate`: clamp to 0 to 1, and a NaN becomes 0.
    pub fn d3d_saturate(&mut self, v: Val) -> Val {
        let clamped = self.math(Mf::Saturate, &[v]);
        let nan = self.is_nan(v);
        let zero = self.lit_f(0.0);
        self.select(nan, zero, clamped)
    }

    /// D3D `min` and `max`: one NaN operand yields the other operand.
    pub fn d3d_min_max(&mut self, fun: Mf, a: Val, b: Val) -> Val {
        let plain = self.math(fun, &[a, b]);
        let a_nan = self.is_nan(a);
        let b_nan = self.is_nan(b);
        let unless_b = self.select(b_nan, a, plain);
        self.select(a_nan, b, unless_b)
    }

    /// D3D `ftoi`: truncate toward zero, saturate to the i32 range, NaN becomes 0.
    pub fn d3d_float_to_int(&mut self, x: Val) -> Val {
        // the largest float below 2^31 is 2147483520; from 2^31 on the result is i32::MAX
        let low = self.lit_f(-2147483648.0);
        let high = self.lit_f(2147483520.0);
        let clamped = self.math(Mf::Clamp, &[x, low, high]);
        let converted = self.convert(clamped, Kind::I);
        let limit = self.lit_f(2147483648.0);
        let too_big = self.binary(Bo::GreaterEqual, x, limit);
        let max = self.lit_i(i32::MAX);
        let saturated = self.select(too_big, max, converted);
        let nan = self.is_nan(x);
        let zero = self.lit_i(0);
        self.select(nan, zero, saturated)
    }

    /// D3D `ftou`: truncate toward zero, negatives and NaN become 0, saturate at u32::MAX.
    pub fn d3d_float_to_uint(&mut self, x: Val) -> Val {
        let low = self.lit_f(0.0);
        let high = self.lit_f(4294967040.0);
        let clamped = self.math(Mf::Clamp, &[x, low, high]);
        let converted = self.convert(clamped, Kind::U);
        let limit = self.lit_f(4294967296.0);
        let too_big = self.binary(Bo::GreaterEqual, x, limit);
        let max = self.lit_u(u32::MAX);
        let saturated = self.select(too_big, max, converted);
        let nan = self.is_nan(x);
        let zero = self.lit_u(0);
        self.select(nan, zero, saturated)
    }

    /// D3D `udiv`: the quotient and the remainder of unsigned values; a division by zero gives all ones for both.
    pub fn d3d_unsigned_divide(&mut self, a: Val, b: Val) -> (Val, Val) {
        let zero = self.lit_u(0);
        let one = self.lit_u(1);
        let by_zero = self.binary(Bo::Equal, b, zero);
        let divisor = self.select(by_zero, one, b);
        let quotient = self.binary(Bo::Divide, a, divisor);
        let remainder = self.binary(Bo::Modulo, a, divisor);
        let ones = self.lit_u(u32::MAX);
        (
            self.select(by_zero, ones, quotient),
            self.select(by_zero, ones, remainder),
        )
    }

    /// A shift or bit-field count: D3D uses the low five bits.
    pub fn d3d_shift_count(&mut self, count: Val) -> Val {
        let mask = self.lit_u(31);
        self.binary(Bo::And, count, mask)
    }

    /// The high 32 bits of the 64-bit product of two unsigned values, from 16-bit halves.
    pub fn mul_high_unsigned(&mut self, a: Val, b: Val) -> Val {
        let low16 = self.lit_u(0xffff);
        let sixteen = self.lit_u(16);
        let (al, ah) = (
            self.binary(Bo::And, a, low16),
            self.binary(Bo::ShiftRight, a, sixteen),
        );
        let (bl, bh) = (
            self.binary(Bo::And, b, low16),
            self.binary(Bo::ShiftRight, b, sixteen),
        );
        let ll = self.binary(Bo::Multiply, al, bl);
        let lh = self.binary(Bo::Multiply, al, bh);
        let hl = self.binary(Bo::Multiply, ah, bl);
        let hh = self.binary(Bo::Multiply, ah, bh);
        let carry_in = self.binary(Bo::ShiftRight, ll, sixteen);
        let lh_low = self.binary(Bo::And, lh, low16);
        let hl_low = self.binary(Bo::And, hl, low16);
        let mid = {
            let s = self.binary(Bo::Add, carry_in, lh_low);
            self.binary(Bo::Add, s, hl_low)
        };
        let lh_high = self.binary(Bo::ShiftRight, lh, sixteen);
        let hl_high = self.binary(Bo::ShiftRight, hl, sixteen);
        let highs = self.binary(Bo::Add, lh_high, hl_high);
        let hi = self.binary(Bo::Add, hh, highs);
        let mid_carry = self.binary(Bo::ShiftRight, mid, sixteen);
        self.binary(Bo::Add, hi, mid_carry)
    }
}
