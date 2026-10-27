//! The IEC sRGB transfer curve both ways, in f64 and in f32 (one definition, evaluated at the precision the caller
//! asks for), and the byte forms: a stored sRGB byte to linear light from a 256-entry table, and linear light to the
//! nearest sRGB byte from a table of the curve's rounding thresholds (the curve's own byte for every input).

use std::sync::LazyLock;

// One curve definition, evaluated at the precision explicitly requested by the caller.
macro_rules! srgb_transfer {
    ($decode:ident, $encode:ident, $float:ty) => {
        /// IEC sRGB encoded value to linear light, with no clamping.
        pub fn $decode(c: $float) -> $float {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        /// Linear light to IEC sRGB encoded value, with no clamping.
        pub fn $encode(c: $float) -> $float {
            if c <= 0.0031308 {
                12.92 * c
            } else {
                1.055 * c.powf(1. / 2.4) - 0.055
            }
        }
    };
}
srgb_transfer!(srgb_to_linear, linear_to_srgb, f64);
srgb_transfer!(srgb_to_linear_f32, linear_to_srgb_f32, f32);
/// Encode linear light with f64 arithmetic, clamping before transfer and rounding to nearest byte: the byte of
/// `(linear_to_srgb(c.clamp(0, 1)) * 255).round()`, found in a table of the curve's 255 rounding thresholds (no `powf`
/// per call; see [`Thresholds`]).
pub fn linear_to_srgb8(c: f64) -> u8 {
    static TABLE: LazyLock<Thresholds<f64>> = LazyLock::new(|| Thresholds::<f64>::new(linear_to_srgb8_curve));
    TABLE.byte(c, linear_to_srgb8_curve)
}
/// [`linear_to_srgb8`] evaluated on the curve itself.
fn linear_to_srgb8_curve(c: f64) -> u8 {
    (linear_to_srgb(c.clamp(0., 1.)) * 255.).round() as u8
}
/// Encode linear light with f32 arithmetic and half-up byte quantization.
/// Clamping follows transfer and the saturating float-to-integer conversion. Found in a table of the curve's rounding
/// thresholds, as [`linear_to_srgb8`].
pub fn linear_to_srgb8_f32(c: f32) -> u8 {
    static TABLE: LazyLock<Thresholds<f32>> =
        LazyLock::new(|| Thresholds::<f32>::new(linear_to_srgb8_f32_curve));
    TABLE.byte(c, linear_to_srgb8_f32_curve)
}
/// [`linear_to_srgb8_f32`] evaluated on the curve itself.
fn linear_to_srgb8_f32_curve(c: f32) -> u8 {
    ((linear_to_srgb_f32(c) * 255. + 0.5) as i32).clamp(0, 255) as u8
}
/// A stored sRGB byte to linear light, `srgb_to_linear(byte / 255)` in f64: the same bits, from a 256-entry table.
pub fn srgb8_to_linear(byte: u8) -> f64 {
    static TABLE: LazyLock<[f64; 256]> =
        LazyLock::new(|| std::array::from_fn(|i| srgb_to_linear(f64::from(i as u8) / 255.)));
    TABLE[usize::from(byte)]
}
/// A stored sRGB byte to linear light, `srgb_to_linear_f32(byte / 255)` in f32: the same bits, from a 256-entry table.
pub fn srgb8_to_linear_f32(byte: u8) -> f32 {
    static TABLE: LazyLock<[f32; 256]> =
        LazyLock::new(|| std::array::from_fn(|i| srgb_to_linear_f32(f32::from(i as u8) / 255.)));
    TABLE[usize::from(byte)]
}
/// The inputs where an encoding curve's byte steps up: `at[k - 1]` is the smallest input in (0, 1) giving byte `k` or
/// more, found by bisection over the input's bit patterns (ordered like the values for non-negative floats) with the
/// curve itself. A byte is then the count of thresholds at or below the input. The curve is monotone in exact
/// arithmetic but `powf` is not correctly rounded on every platform, so it may wobble by a few ulps where its output
/// crosses a half byte: inputs within a relative band of a threshold (`BAND`), and inputs outside (0, 1)
/// (clamped, NaN), are evaluated on the curve itself. The table therefore gives the curve's byte for every input.
struct Thresholds<F> {
    at: [F; 255],
}
macro_rules! thresholds {
    ($float:ty, $bits:ty, $band:expr) => {
        impl Thresholds<$float> {
            const BAND: $float = $band;

            fn new(curve: fn($float) -> u8) -> Self {
                let one = (1.0 as $float).to_bits();
                Thresholds {
                    at: std::array::from_fn(|i| {
                        let k = i as u8 + 1;
                        // curve(0) = 0 < k <= 255 = curve(1): the smallest bit pattern in (0, 1] reaching k
                        let (mut lo, mut hi): ($bits, $bits) = (0, one);
                        while hi - lo > 1 {
                            let mid = lo + (hi - lo) / 2;
                            if curve(<$float>::from_bits(mid)) >= k {
                                hi = mid;
                            } else {
                                lo = mid;
                            }
                        }
                        <$float>::from_bits(hi)
                    }),
                }
            }

            fn byte(&self, c: $float, curve: fn($float) -> u8) -> u8 {
                if !(c > 0.0 && c < 1.0) {
                    return curve(c);
                }
                let k = self.at.partition_point(|&t| t <= c);
                let near = |i: usize| {
                    self.at
                        .get(i)
                        .is_some_and(|&t| (c - t).abs() <= t * Self::BAND)
                };
                if near(k) || (k > 0 && near(k - 1)) {
                    return curve(c);
                }
                k as u8
            }
        }
    };
}
// The bands are far wider than the curves' rounding wobble near a threshold. There the curve's slope is at least 0.43
// (its smallest, at 1), and `powf` and the products after it are each within a few ulps of the output, so an input
// whose byte could differ from its neighbours' lies within some 10 ulps of the threshold: 2^-24 of the input is ~10^8
// f64 ulps, 2^-16 ~256 f32 ulps. The linear segment below 0.0031308 (12.92 c) is monotone as computed, and the knee
// is no threshold (its byte is 10.3).
thresholds!(f64, u64, 1.0 / 16_777_216.0);
thresholds!(f32, u32, 1.0 / 65_536.0);
