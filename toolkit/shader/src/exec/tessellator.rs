//! The Direct3D 11 fixed-function tessellator: from the tessellation factors of a patch, the points of its domain and
//! how they connect.
//!
//! This is a port to Rust of the reference tessellator Microsoft published with the Direct3D 11.3 functional
//! specification (`CHWTessellator` of `tessellator.cpp`, "Microsoft D3D11 Fixed Function Tessellator Reference - May 7,
//! 2012", in <https://github.com/microsoft/DirectX-Specs>, `d3d/archive/images/d3d11/`), whose repository licenses its
//! code under the MIT License:
//!
//! > The MIT License (MIT)
//! > Copyright (c) Microsoft Corporation
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
//! > documentation files (the "Software"), to deal in the Software without restriction, including without limitation
//! > the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! > permit persons to whom the Software is furnished to do so, subject to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in all copies or substantial portions of
//! > the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
//! > THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! > AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
//! > TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! > SOFTWARE.
//!
//! The structure follows the reference so that the two can be read side by side: the domain locations are 16.16
//! fixed-point numbers converted to `f32` at the end, the points of the outermost ring come first (edge by edge), the
//! inner rings follow, spiralling in, and the triangles stitch ring to ring, with the "ruler function" order in which
//! the points of an edge appear as its tessellation factor grows. What is checked against it (and against the
//! Direct3D 11 reference device of WARP) is in the tests.

/// How the tessellator spaces the segments along an edge (`partitioning(...)` of the hull program).
pub use crate::Partitioning;
/// What the tessellator emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputPrimitive {
    Point,
    Line,
    TriangleCw,
    TriangleCcw,
}

/// The tessellation factors of a patch: the edges' (a triangle uses 3, a quad 4, an isoline the line density then the
/// line detail) and the inside ones (a triangle 1, a quad 2: the factor along u, then along v).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Factors {
    pub edges: [f32; 4],
    pub inside: [f32; 2],
}

/// The points and connections of a tessellated patch: `points` are (u, v) domain locations (a triangle's third
/// barycentric coordinate is `1 - u - v`), `indices` are points by their number, as the points of a point list, the
/// ends of lines of a line list, or the corners of triangles of a triangle list, as the output primitive says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tessellation {
    pub points: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Tessellate a triangle patch.
pub fn tessellate_triangle(
    partitioning: Partitioning,
    output: OutputPrimitive,
    factors: Factors,
) -> Tessellation {
    let mut t = Tessellator::new(partitioning, output);
    t.triangle(
        factors.edges[0],
        factors.edges[1],
        factors.edges[2],
        factors.inside[0],
    );
    t.finish()
}

/// Tessellate a quad patch.
pub fn tessellate_quad(
    partitioning: Partitioning,
    output: OutputPrimitive,
    factors: Factors,
) -> Tessellation {
    let mut t = Tessellator::new(partitioning, output);
    t.quad(
        factors.edges[0],
        factors.edges[1],
        factors.edges[2],
        factors.edges[3],
        factors.inside[0],
        factors.inside[1],
    );
    t.finish()
}

/// Tessellate an isoline patch: `density` lines (the first factor), each of `detail` segments (the second).
pub fn tessellate_isolines(
    partitioning: Partitioning,
    output: OutputPrimitive,
    density: f32,
    detail: f32,
) -> Tessellation {
    let mut t = Tessellator::new(partitioning, output);
    t.isolines(density, detail);
    t.finish()
}

const MIN_ODD: f32 = 1.0;
const MAX_ODD: f32 = 63.0;
const MIN_EVEN: f32 = 2.0;
const MAX_EVEN: f32 = 64.0;
const MIN_ISOLINE_DENSITY: f32 = 1.0;
const MAX_ISOLINE_DENSITY: f32 = 64.0;
/// 2^-16, the smallest positive fixed-point fraction.
const EPSILON: f32 = 1.0 / 65536.0;
const MAX_FACTOR: usize = 64;
const MAX_POINTS: usize = (MAX_FACTOR + 1) * (MAX_FACTOR + 1);
const MAX_INDICES: usize = MAX_FACTOR * MAX_FACTOR * 2 * 3;

/// A 16.16 fixed-point number (15 integer bits and 16 fraction bits, unsigned).
type Fxp = u32;
const FRACTION_BITS: u32 = 16;
const FRACTION_MASK: Fxp = 0x0000_ffff;
const INTEGER_MASK: Fxp = 0x7fff_0000;
const FXP_ONE: Fxp = 1 << FRACTION_BITS;
const FXP_ONE_THIRD: Fxp = 0x0000_5555;
const FXP_TWO_THIRDS: Fxp = 0x0000_aaaa;
const FXP_ONE_HALF: Fxp = 0x0000_8000;
/// 1 / n in 16.16 for n in 1..=64 (entry 0 is unused).
const FIXED_RECIPROCAL: [Fxp; MAX_FACTOR + 1] = [
    0xffff_ffff,
    0x10000,
    0x8000,
    0x5555,
    0x4000,
    0x3333,
    0x2aab,
    0x2492,
    0x2000,
    0x1c72,
    0x199a,
    0x1746,
    0x1555,
    0x13b1,
    0x1249,
    0x1111,
    0x1000,
    0xf0f,
    0xe39,
    0xd79,
    0xccd,
    0xc31,
    0xba3,
    0xb21,
    0xaab,
    0xa3d,
    0x9d9,
    0x97b,
    0x925,
    0x8d4,
    0x889,
    0x842,
    0x800,
    0x7c2,
    0x788,
    0x750,
    0x71c,
    0x6eb,
    0x6bd,
    0x690,
    0x666,
    0x63e,
    0x618,
    0x5f4,
    0x5d1,
    0x5b0,
    0x591,
    0x572,
    0x555,
    0x539,
    0x51f,
    0x505,
    0x4ec,
    0x4d5,
    0x4be,
    0x4a8,
    0x492,
    0x47e,
    0x46a,
    0x457,
    0x444,
    0x432,
    0x421,
    0x410,
    0x400,
];

/// The reference's Direct3D-compliant float helpers: denormals flush, NaN is passed over.
fn is_nan(a: f32) -> bool {
    a.is_nan()
}
fn flush(a: f32) -> f32 {
    if a.to_bits() & 0x7fff_ffff < 0x0080_0000 {
        f32::from_bits(a.to_bits() & 0x8000_0000)
    } else {
        a
    }
}
/// Whether an edge factor culls the patch: not above 0 (a NaN too, as the reference's `!(factor > 0)`).
fn culls(factor: f32) -> bool {
    factor.is_nan() || factor <= 0.0
}
/// IEEE 754R min.
fn fmin(a: f32, b: f32) -> f32 {
    let (fa, fb) = (flush(a), flush(b));
    if is_nan(fb) {
        a
    } else if fa == 0.0 && fb == 0.0 {
        if fa.to_bits() & 0x8000_0000 != 0 { a } else { b }
    } else if fa < fb {
        a
    } else {
        b
    }
}
/// IEEE 754R max.
fn fmax(a: f32, b: f32) -> f32 {
    let (fa, fb) = (flush(a), flush(b));
    if is_nan(fb) {
        a
    } else if fa == 0.0 && fb == 0.0 {
        if fb.to_bits() & 0x8000_0000 != 0 { a } else { b }
    } else if fa >= fb {
        a
    } else {
        b
    }
}

/// `floatToFixed`: a float to 16.16 unsigned fixed point, to nearest even, NaN and negative to 0, saturating.
fn float_to_fixed(input: f32) -> Fxp {
    const BIAS: i32 = 127;
    const HIDDEN: i32 = 1 << 23;
    const MANTISSA_MASK: i32 = HIDDEN - 1;
    const EXPONENT_MASK: i32 = 0xff << 23;
    const SIGN: i32 = i32::MIN;
    // the largest positive value as a float's bits: 2^15 (the fraction needs more than the float's 24 bits of
    // precision to reach below it, so nothing is subtracted)
    const MAX_POSITIVE: i32 = (BIAS + 15) << 23;
    let mut output = input.to_bits() as i32;
    let unbiased = ((output & EXPONENT_MASK) >> 23) - BIAS;
    let negative = output & SIGN != 0;
    if unbiased == BIAS + 1 && output & MANTISSA_MASK != 0 {
        output = 0;
    } else if !negative && output >= MAX_POSITIVE {
        output = -1;
    } else if negative {
        // every negative value is below the smallest unsigned one
        output = 0;
    } else if unbiased < -(FRACTION_BITS as i32) - 1 {
        output = 0;
    } else {
        output = (output & MANTISSA_MASK) | HIDDEN;
        let extra = 23 - FRACTION_BITS as i32 - unbiased;
        if extra >= 0 {
            let lsb = 1i32 << extra;
            let extra_mask = lsb - 1;
            let half = lsb >> 1;
            if output & lsb != 0 || (output & extra_mask) > half {
                output += half;
            }
            output >>= extra;
        } else {
            output <<= -extra;
        }
    }
    output as Fxp
}

fn fixed_to_float(input: Fxp) -> f32 {
    (input >> FRACTION_BITS) as f32 + (input & FRACTION_MASK) as f32 / (1u32 << FRACTION_BITS) as f32
}

fn is_even(input: f32) -> bool {
    (input as i32) & 1 == 0
}

fn fxp_ceil(input: Fxp) -> Fxp {
    if input & FRACTION_MASK != 0 {
        (input & INTEGER_MASK).wrapping_add(FXP_ONE)
    } else {
        input
    }
}

fn fxp_floor(input: Fxp) -> Fxp {
    input & INTEGER_MASK
}

/// The most significant set bit removed (0 stays 0).
fn remove_msb(val: i32) -> i32 {
    if val <= 0 {
        0
    } else {
        val & !(1 << (31 - val.leading_zeros()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parity {
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Default)]
struct FactorContext {
    inv_segments_on_floor: Fxp,
    inv_segments_on_ceil: Fxp,
    half_fraction: Fxp,
    num_half_points: i32,
    split_point_on_floor: i32,
}

/// A row of points to stitch: the number of its first point, how many points are on half of its edge, its parity.
#[derive(Clone, Copy)]
struct Row {
    base: i32,
    half_points: i32,
    parity: Parity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Diagonals {
    InsideToOutside,
    InsideToOutsideExceptMiddle,
    Mirrored,
}

#[derive(Clone, Copy, Debug, Default)]
struct IndexPatch {
    inside_delta: i32,
    inside_bad: i32,
    inside_replacement: i32,
    outside_base: i32,
    outside_delta: i32,
    outside_bad: i32,
    outside_replacement: i32,
}

#[derive(Clone, Copy, Debug, Default)]
struct IndexPatch2 {
    base_to_invert: i32,
    inversion_end: i32,
    corner_bad: i32,
    corner_replacement: i32,
}

struct Tessellator {
    original_parity: Parity,
    parity: Parity,
    original_partitioning: Partitioning,
    partitioning: Partitioning,
    output: OutputPrimitive,
    points: Vec<[f32; 2]>,
    index: Vec<i32>,
    num_points: i32,
    num_indices: i32,
    using_patch: bool,
    using_patch2: bool,
    patch: IndexPatch,
    patch2: IndexPatch2,
}

struct Quad {
    outside: [Fxp; 4],
    inside: [Fxp; 2],
    outside_parity: [Parity; 4],
    inside_parity: [Parity; 2],
    outside_ctx: [FactorContext; 4],
    inside_ctx: [FactorContext; 2],
    just_minimum: bool,
    culled: bool,
    points_for_outside_edge: [i32; 4],
    points_for_inside: [i32; 2],
    inside_edge_point_base: i32,
}

struct Tri {
    outside: [Fxp; 3],
    inside: Fxp,
    outside_parity: [Parity; 3],
    inside_parity: Parity,
    outside_ctx: [FactorContext; 3],
    inside_ctx: FactorContext,
    just_minimum: bool,
    culled: bool,
    points_for_outside_edge: [i32; 3],
    points_for_inside: i32,
    inside_edge_point_base: i32,
}

struct Isolines {
    line_density_parity: Parity,
    line_detail_parity: Parity,
    line_density_ctx: FactorContext,
    line_detail_ctx: FactorContext,
    culled: bool,
    points_per_line: i32,
    lines: i32,
}

impl Tessellator {
    fn new(partitioning: Partitioning, output: OutputPrimitive) -> Self {
        let parity = match partitioning {
            Partitioning::FractionalOdd => Parity::Odd,
            // integer and pow2 take either, per factor; the reference leaves the initial parity at even
            _ => Parity::Even,
        };
        Self {
            original_parity: parity,
            parity,
            original_partitioning: partitioning,
            partitioning,
            output,
            points: vec![[0.0; 2]; MAX_POINTS],
            index: vec![0; MAX_INDICES],
            num_points: 0,
            num_indices: 0,
            using_patch: false,
            using_patch2: false,
            patch: IndexPatch::default(),
            patch2: IndexPatch2::default(),
        }
    }

    fn finish(mut self) -> Tessellation {
        self.points.truncate(self.num_points.max(0) as usize);
        Tessellation {
            points: self.points,
            indices: self.index[..self.num_indices.max(0) as usize]
                .iter()
                .map(|&i| i as u32)
                .collect(),
        }
    }

    fn odd(&self) -> bool {
        self.parity == Parity::Odd
    }

    /// Hardware does not tell integer from pow2 partitioning: both round to the next integer.
    fn hw_integer(&self) -> bool {
        matches!(self.partitioning, Partitioning::Integer | Partitioning::Pow2)
    }

    /// The factor range the original partitioning allows.
    fn bounds(&self) -> (f32, f32) {
        match self.original_partitioning {
            Partitioning::Integer | Partitioning::Pow2 => (MIN_ODD, MAX_EVEN),
            Partitioning::FractionalEven => (MIN_EVEN, MAX_EVEN),
            Partitioning::FractionalOdd => (MIN_ODD, MAX_ODD),
        }
    }

    fn define_point(&mut self, u: Fxp, v: Fxp, at: i32) {
        self.points[at as usize] = [fixed_to_float(u), fixed_to_float(v)];
    }

    fn define_index(&mut self, index: i32, at: i32) {
        self.index[at as usize] = self.patch_index_value(index);
    }

    /// A triangle given clockwise, stored clockwise or counter-clockwise as the output wants.
    fn define_clockwise_triangle(&mut self, i0: i32, i1: i32, i2: i32, at: i32) {
        self.define_index(i0, at);
        if self.output == OutputPrimitive::TriangleCw {
            self.define_index(i1, at + 1);
            self.define_index(i2, at + 2);
        } else {
            self.define_index(i2, at + 1);
            self.define_index(i1, at + 2);
        }
    }

    fn dump_all_points(&mut self) {
        for p in 0..self.num_points {
            let at = self.num_indices;
            self.num_indices += 1;
            self.define_index(p, at);
        }
    }

    fn dump_all_points_as_in_order_line_list(&mut self) {
        for p in 1..self.num_points {
            for index in [p - 1, p] {
                let at = self.num_indices;
                self.num_indices += 1;
                self.define_index(index, at);
            }
        }
    }

    fn num_points_for_factor(&self, fxp: Fxp) -> i32 {
        if self.odd() {
            (fxp_ceil(FXP_ONE_HALF.wrapping_add(fxp.wrapping_add(1) / 2)).wrapping_mul(2) >> FRACTION_BITS)
                as i32
        } else {
            ((fxp_ceil(fxp.wrapping_add(1) / 2).wrapping_mul(2) >> FRACTION_BITS) + 1) as i32
        }
    }

    fn factor_context(&self, fxp: Fxp) -> FactorContext {
        let mut half = fxp.wrapping_add(1) / 2;
        if self.odd() || half == FXP_ONE_HALF {
            // a factor of 1 is treated as even
            half = half.wrapping_add(FXP_ONE_HALF);
        }
        let floor_half = fxp_floor(half);
        let ceil_half = fxp_ceil(half);
        let half_fraction = half - floor_half;
        let num_half_points = (ceil_half >> FRACTION_BITS) as i32;
        let split_point_on_floor = if ceil_half == floor_half {
            // a value that causes the split to be ignored
            num_half_points + 1
        } else if self.odd() {
            if floor_half == FXP_ONE {
                0
            } else {
                (remove_msb(((floor_half >> FRACTION_BITS) as i32) - 1) << 1) + 1
            }
        } else {
            (remove_msb((floor_half >> FRACTION_BITS) as i32) << 1) + 1
        };
        let mut floor_segments = ((floor_half.wrapping_mul(2)) >> FRACTION_BITS) as usize;
        let mut ceil_segments = ((ceil_half.wrapping_mul(2)) >> FRACTION_BITS) as usize;
        if self.odd() {
            floor_segments -= 1;
            ceil_segments -= 1;
        }
        FactorContext {
            inv_segments_on_floor: FIXED_RECIPROCAL[floor_segments],
            inv_segments_on_ceil: FIXED_RECIPROCAL[ceil_segments],
            half_fraction,
            num_half_points,
            split_point_on_floor,
        }
    }

    /// Where point `point` of an edge is, in 16.16: points are introduced in ruler-function order as the factor
    /// grows, and an edge is mirrored about its middle.
    fn place_point_in_1d(&self, ctx: &FactorContext, point: i32) -> Fxp {
        let mut point = point;
        let flip = point >= ctx.num_half_points;
        if flip {
            point = (ctx.num_half_points << 1) - point;
            if self.odd() {
                point -= 1;
            }
        }
        if point == ctx.num_half_points {
            return FXP_ONE_HALF;
        }
        let on_ceil = point as u32;
        let mut on_floor = on_ceil;
        if point > ctx.split_point_on_floor {
            on_floor = on_floor.wrapping_sub(1);
        }
        let location_on_floor = on_floor.wrapping_mul(ctx.inv_segments_on_floor);
        let location_on_ceil = on_ceil.wrapping_mul(ctx.inv_segments_on_ceil);
        let mut location = location_on_floor
            .wrapping_mul(FXP_ONE.wrapping_sub(ctx.half_fraction))
            .wrapping_add(location_on_ceil.wrapping_mul(ctx.half_fraction));
        location = location.wrapping_add(FXP_ONE_HALF) >> FRACTION_BITS;
        if flip {
            location = FXP_ONE.wrapping_sub(location);
        }
        location
    }

    fn patch_index_value(&self, mut index: i32) -> i32 {
        if self.using_patch {
            let p = &self.patch;
            if index >= p.outside_base {
                if index == p.outside_bad {
                    index = p.outside_replacement;
                } else {
                    index += p.outside_delta;
                }
            } else if index == p.inside_bad {
                index = p.inside_replacement;
            } else {
                index += p.inside_delta;
            }
        } else if self.using_patch2 {
            let p = &self.patch2;
            if index >= p.base_to_invert {
                if index == p.corner_bad {
                    index = p.corner_replacement;
                } else {
                    index = p.inversion_end - index;
                }
            } else if index == p.corner_bad {
                index = p.corner_replacement;
            }
        }
        index
    }

    /// Two parallel rows of points stitched as the same number of points on each side (a trapezoid at each end when
    /// `trapezoid`), with the diagonals as `diagonals` says.
    fn stitch_regular(
        &mut self,
        trapezoid: bool,
        diagonals: Diagonals,
        mut base: i32,
        inside_points: i32,
        inside_base: i32,
        outside_base: i32,
    ) {
        let mut inside = inside_base;
        let mut outside = outside_base;
        if trapezoid {
            self.define_clockwise_triangle(outside, outside + 1, inside, base);
            base += 3;
            outside += 1;
        }
        let mut p;
        match diagonals {
            Diagonals::InsideToOutside => {
                for _ in 0..inside_points - 1 {
                    self.define_clockwise_triangle(inside, outside, outside + 1, base);
                    base += 3;
                    self.define_clockwise_triangle(inside, outside + 1, inside + 1, base);
                    base += 3;
                    inside += 1;
                    outside += 1;
                }
            }
            Diagonals::InsideToOutsideExceptMiddle => {
                // first half
                p = 0;
                while p < inside_points / 2 - 1 {
                    self.define_clockwise_triangle(outside, outside + 1, inside, base);
                    base += 3;
                    self.define_clockwise_triangle(inside, outside + 1, inside + 1, base);
                    base += 3;
                    inside += 1;
                    outside += 1;
                    p += 1;
                }
                // middle
                self.define_clockwise_triangle(outside, inside + 1, inside, base);
                base += 3;
                self.define_clockwise_triangle(outside, outside + 1, inside + 1, base);
                base += 3;
                inside += 1;
                outside += 1;
                p += 2;
                // second half
                while p < inside_points {
                    self.define_clockwise_triangle(outside, outside + 1, inside, base);
                    base += 3;
                    self.define_clockwise_triangle(inside, outside + 1, inside + 1, base);
                    base += 3;
                    inside += 1;
                    outside += 1;
                    p += 1;
                }
            }
            Diagonals::Mirrored => {
                // first half, diagonals from the outside of the outside edge to the inside of the inside edge
                p = 0;
                while p < inside_points / 2 {
                    self.define_clockwise_triangle(outside, inside + 1, inside, base);
                    base += 3;
                    self.define_clockwise_triangle(outside, outside + 1, inside + 1, base);
                    base += 3;
                    inside += 1;
                    outside += 1;
                    p += 1;
                }
                // second half, diagonals from the inside of the inside edge to the outside of the outside edge
                while p < inside_points - 1 {
                    self.define_clockwise_triangle(inside, outside, outside + 1, base);
                    base += 3;
                    self.define_clockwise_triangle(inside, outside + 1, inside + 1, base);
                    base += 3;
                    inside += 1;
                    outside += 1;
                    p += 1;
                }
            }
        }
        if trapezoid {
            self.define_clockwise_triangle(outside, outside + 1, inside, base);
        }
    }

    /// Two parallel rows of points with different factors stitched, in the order the points of an edge appear as its
    /// factor grows (the ruler function), walking in from one end to the middle and back out of it.
    fn stitch_transition(&mut self, mut base: i32, inside: Row, outside: Row) {
        // where point i of the half edge ends up on the half edge at the largest tessellation
        const FINAL_POINT_POSITION: [i32; 33] = [
            0, 32, 16, 8, 17, 4, 18, 9, 19, 2, 20, 10, 21, 5, 22, 11, 23, 1, 24, 12, 25, 6, 26, 13, 27, 3,
            28, 14, 29, 7, 30, 15, 31,
        ];
        // the first entry other than 0 of the table above under the half factor, and the last one
        const LOOP_START: [i32; 33] = [
            1, 1, 17, 9, 9, 5, 5, 5, 5, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            2,
        ];
        const LOOP_END: [i32; 33] = [
            0, 0, 17, 17, 25, 25, 25, 25, 29, 29, 29, 29, 29, 29, 29, 29, 31, 31, 31, 31, 31, 31, 31, 31, 31,
            31, 31, 31, 31, 31, 31, 31, 32,
        ];
        let (inside_parity, outside_parity) = (inside.parity, outside.parity);
        let inside_half_points = inside.half_points - i32::from(inside_parity == Parity::Odd);
        let outside_half_points = outside.half_points - i32::from(outside_parity == Parity::Odd);
        let mut outside = outside.base;
        let mut inside = inside.base;
        let start = LOOP_START[inside_half_points as usize].min(LOOP_START[outside_half_points as usize]);
        let end = LOOP_END[inside_half_points as usize].max(LOOP_END[outside_half_points as usize]);

        // walk the first half
        if FINAL_POINT_POSITION[0] < outside_half_points {
            // advance the outside
            self.define_clockwise_triangle(outside, outside + 1, inside, base);
            base += 3;
            outside += 1;
        }
        for i in start..=end {
            if FINAL_POINT_POSITION[i as usize] < inside_half_points {
                self.define_clockwise_triangle(inside, outside, inside + 1, base);
                base += 3;
                inside += 1;
            }
            if FINAL_POINT_POSITION[i as usize] < outside_half_points {
                self.define_clockwise_triangle(outside, outside + 1, inside, base);
                base += 3;
                outside += 1;
            }
        }
        if inside_parity != outside_parity || inside_parity == Parity::Odd {
            if inside_parity == outside_parity {
                // a quad in the middle
                self.define_clockwise_triangle(inside, outside, inside + 1, base);
                base += 3;
                self.define_clockwise_triangle(inside + 1, outside, outside + 1, base);
                base += 3;
                inside += 1;
                outside += 1;
            } else if inside_parity == Parity::Even {
                // a triangle pointing inside
                self.define_clockwise_triangle(inside, outside, outside + 1, base);
                base += 3;
                outside += 1;
            } else {
                // a triangle pointing outside
                self.define_clockwise_triangle(inside, outside, inside + 1, base);
                base += 3;
                inside += 1;
            }
        }
        // walk the second half
        for i in (start..=end).rev() {
            if FINAL_POINT_POSITION[i as usize] < outside_half_points {
                self.define_clockwise_triangle(outside, outside + 1, inside, base);
                base += 3;
                outside += 1;
            }
            if FINAL_POINT_POSITION[i as usize] < inside_half_points {
                self.define_clockwise_triangle(inside, outside, inside + 1, base);
                base += 3;
                inside += 1;
            }
        }
        if FINAL_POINT_POSITION[0] < outside_half_points {
            self.define_clockwise_triangle(outside, outside + 1, inside, base);
        }
    }

    // --- quads ---

    fn quad(&mut self, u0: f32, v0: f32, u1: f32, v1: f32, inside_u: f32, inside_v: f32) {
        let q = self.quad_process(u0, v0, u1, v1, inside_u, inside_v);
        if q.culled {
            self.num_points = 0;
            self.num_indices = 0;
            return;
        }
        if q.just_minimum {
            self.define_point(0, 0, 0);
            self.define_point(FXP_ONE, 0, 1);
            self.define_point(FXP_ONE, FXP_ONE, 2);
            self.define_point(0, FXP_ONE, 3);
            self.num_points = 4;
            match self.output {
                OutputPrimitive::TriangleCw | OutputPrimitive::TriangleCcw => {
                    self.define_clockwise_triangle(0, 1, 3, 0);
                    self.define_clockwise_triangle(1, 2, 3, 3);
                    self.num_indices = 6;
                }
                OutputPrimitive::Point => self.dump_all_points(),
                OutputPrimitive::Line => self.dump_all_points_as_in_order_line_list(),
            }
            return;
        }
        self.quad_generate_points(&q);
        match self.output {
            OutputPrimitive::Point => self.dump_all_points(),
            OutputPrimitive::Line => self.dump_all_points_as_in_order_line_list(),
            _ => self.quad_generate_connectivity(&q),
        }
    }

    fn quad_process(
        &mut self,
        mut u0: f32,
        mut v0: f32,
        mut u1: f32,
        mut v1: f32,
        mut in_u: f32,
        mut in_v: f32,
    ) -> Quad {
        let mut q = Quad {
            outside: [0; 4],
            inside: [0; 2],
            outside_parity: [Parity::Even; 4],
            inside_parity: [Parity::Even; 2],
            outside_ctx: [FactorContext::default(); 4],
            inside_ctx: [FactorContext::default(); 2],
            just_minimum: false,
            culled: false,
            points_for_outside_edge: [0; 4],
            points_for_inside: [0; 2],
            inside_edge_point_base: 0,
        };
        // a culled patch: any edge factor not above 0 (NaN too)
        if culls(u0) || culls(v0) || culls(u1) || culls(v1) {
            q.culled = true;
            return q;
        }
        let (mut lower, upper) = self.bounds();
        u0 = fmin(upper, fmax(lower, u0));
        v0 = fmin(upper, fmax(lower, v0));
        u1 = fmin(upper, fmax(lower, u1));
        v1 = fmin(upper, fmax(lower, v1));
        if self.hw_integer() {
            u0 = u0.ceil();
            v0 = v0.ceil();
            u1 = u1.ceil();
            v1 = v1.ceil();
        }
        if self.original_partitioning == Partitioning::FractionalOdd {
            let limit = MIN_ODD + EPSILON / 2.0;
            // if any factor ends up above 1 once it is fixed point, the inside factors must be too: a picture frame
            if u0 > limit || v0 > limit || u1 > limit || v1 > limit || in_u > limit || in_v > limit {
                lower = MIN_ODD + EPSILON;
            }
        }
        in_u = fmin(upper, fmax(lower, in_u));
        in_v = fmin(upper, fmax(lower, in_v));
        if self.hw_integer() {
            in_u = in_u.ceil();
            in_v = in_v.ceil();
        }
        self.num_points = 0;
        self.num_indices = 0;
        let outside = [u0, v0, u1, v1];
        let inside = [in_u, in_v];
        if self.hw_integer() {
            q.outside_parity = outside.map(|f| if is_even(f) { Parity::Even } else { Parity::Odd });
            q.inside_parity = inside.map(|f| {
                if is_even(f) || f == 1.0 {
                    Parity::Even
                } else {
                    Parity::Odd
                }
            });
        } else {
            q.outside_parity = [self.original_parity; 4];
            q.inside_parity = [self.original_parity; 2];
        }
        q.outside = outside.map(float_to_fixed);
        q.inside = inside.map(float_to_fixed);
        if self.hw_integer() || self.odd() {
            // all factors 1
            if q.inside.iter().chain(&q.outside).all(|&f| f == FXP_ONE) {
                q.just_minimum = true;
                return q;
            }
        }
        q.just_minimum = false;
        for e in 0..4 {
            self.parity = q.outside_parity[e];
            q.outside_ctx[e] = self.factor_context(q.outside[e]);
        }
        for a in 0..2 {
            self.parity = q.inside_parity[a];
            q.inside_ctx[a] = self.factor_context(q.inside[a]);
        }
        for e in 0..4 {
            self.parity = q.outside_parity[e];
            q.points_for_outside_edge[e] = self.num_points_for_factor(q.outside[e]);
            self.num_points += q.points_for_outside_edge[e];
        }
        self.num_points -= 4;
        for a in 0..2 {
            self.parity = q.inside_parity[a];
            q.points_for_inside[a] = self.num_points_for_factor(q.inside[a]);
            let min = if q.inside_parity[a] == Parity::Odd { 4 } else { 3 };
            // this allows degenerate transition regions when the inside factor is 1
            q.points_for_inside[a] = min.max(q.points_for_inside[a]);
        }
        q.inside_edge_point_base = self.num_points;
        self.num_points += (q.points_for_inside[0] - 2) * (q.points_for_inside[1] - 2);
        q
    }

    fn quad_generate_points(&mut self, q: &Quad) {
        // the exterior ring's edge points, clockwise from the top left
        let mut offset = 0;
        for edge in 0..4usize {
            let parity = edge & 1;
            let end = q.points_for_outside_edge[edge] - 1;
            for p in 0..end {
                let n = if edge == 1 || edge == 2 { p } else { end - p };
                self.parity = q.outside_parity[edge];
                let param = self.place_point_in_1d(&q.outside_ctx[edge], n);
                if parity != 0 {
                    self.define_point(param, if edge == 3 { FXP_ONE } else { 0 }, offset);
                } else {
                    self.define_point(if edge == 2 { FXP_ONE } else { 0 }, param, offset);
                }
                offset += 1;
            }
        }
        // the interior rings, clockwise from (u = 0, v = 1) (the bottom left) spiralling in
        let min_points = q.points_for_inside[0].min(q.points_for_inside[1]);
        let rings = min_points >> 1; // for even tessellation the center point is not counted here
        for ring in 1..rings {
            let start = ring;
            let end = [
                q.points_for_inside[0] - 1 - start,
                q.points_for_inside[1] - 1 - start,
            ];
            for edge in 0..4usize {
                let parity = [edge & 1, (edge + 1) & 1];
                let perpendicular_point = if edge < 2 { start } else { end[parity[0]] };
                self.parity = q.inside_parity[parity[0]];
                let perpendicular = self.place_point_in_1d(&q.inside_ctx[parity[0]], perpendicular_point);
                self.parity = q.inside_parity[parity[1]];
                for p in start..end[parity[1]] {
                    let n = if edge == 1 || edge == 2 {
                        p
                    } else {
                        end[parity[1]] - (p - start)
                    };
                    let param = self.place_point_in_1d(&q.inside_ctx[parity[1]], n);
                    if parity[1] != 0 {
                        self.define_point(perpendicular, param, offset);
                    } else {
                        self.define_point(param, perpendicular, offset);
                    }
                    offset += 1;
                }
            }
        }
        // for even tessellation the inner ring is degenerate: a row of points
        if q.points_for_inside[0] > q.points_for_inside[1] && q.inside_parity[1] == Parity::Even {
            let start = rings;
            let end = q.points_for_inside[0] - 1 - start;
            self.parity = q.inside_parity[0];
            for p in start..=end {
                let param = self.place_point_in_1d(&q.inside_ctx[0], p);
                self.define_point(param, FXP_ONE_HALF, offset);
                offset += 1;
            }
        } else if q.points_for_inside[1] >= q.points_for_inside[0] && q.inside_parity[0] == Parity::Even {
            let start = rings;
            let end = q.points_for_inside[1] - 1 - start;
            self.parity = q.inside_parity[1];
            for p in (start..=end).rev() {
                let param = self.place_point_in_1d(&q.inside_ctx[1], p);
                self.define_point(FXP_ONE_HALF, param, offset);
                offset += 1;
            }
        }
    }

    fn quad_generate_connectivity(&mut self, q: &Quad) {
        // the primitives of the concentric rings, one side at a time for each
        let rows_to_center = [
            (q.points_for_inside[0] + 1) >> 1,
            (q.points_for_inside[1] + 1) >> 1,
        ];
        let rings = rows_to_center[0].min(rows_to_center[1]);
        // even partitioning makes a degenerate row of points, an exception to the point ordering
        let degenerate_ring = [
            if q.inside_parity[1] == Parity::Even {
                rows_to_center[1] - 1
            } else {
                -1
            },
            if q.inside_parity[0] == Parity::Even {
                rows_to_center[0] - 1
            } else {
                -1
            },
        ];
        let mut outside_ctx = [
            q.outside_ctx[0],
            q.outside_ctx[1],
            q.outside_ctx[2],
            q.outside_ctx[3],
        ];
        let mut outside_parity = q.outside_parity;
        let mut points_for_outside_edge = q.points_for_outside_edge;
        let mut inside_edge_base = q.inside_edge_point_base;
        let mut outside_edge_base = 0;
        for ring in 1..rings {
            let points_for_inside_edge = [
                q.points_for_inside[0] - 2 * ring,
                q.points_for_inside[1] - 2 * ring,
            ];
            let edge0_inside_base = inside_edge_base;
            let edge0_outside_base = outside_edge_base;
            for edge in 0..4usize {
                let parity = (edge + 1) & 1;
                let triangles = points_for_inside_edge[parity] + points_for_outside_edge[edge] - 2;
                let inside_base;
                let outside_base;
                if edge == 3 {
                    // patch the indexing so the stitch sees two sequentially increasing rows of points even though
                    // the ring has wrapped around to its first point
                    if ring == degenerate_ring[parity] {
                        self.patch2.base_to_invert = inside_edge_base + 1;
                        self.patch2.corner_bad = outside_edge_base + points_for_outside_edge[edge] - 1;
                        self.patch2.corner_replacement = edge0_outside_base;
                        self.patch2.inversion_end = (self.patch2.base_to_invert << 1) - 1;
                        inside_base = self.patch2.base_to_invert;
                        outside_base = outside_edge_base;
                        self.using_patch2 = true;
                    } else {
                        self.patch.inside_delta = inside_edge_base;
                        self.patch.inside_bad = points_for_inside_edge[parity] - 1;
                        self.patch.inside_replacement = edge0_inside_base;
                        self.patch.outside_base = self.patch.inside_bad + 1;
                        self.patch.outside_delta = outside_edge_base - self.patch.outside_base;
                        self.patch.outside_bad = self.patch.outside_base + points_for_outside_edge[edge] - 1;
                        self.patch.outside_replacement = edge0_outside_base;
                        inside_base = 0;
                        outside_base = self.patch.outside_base;
                        self.using_patch = true;
                    }
                } else if edge == 2 && ring == degenerate_ring[parity] {
                    self.patch2.base_to_invert = inside_edge_base;
                    self.patch2.corner_bad = -1;
                    self.patch2.corner_replacement = -1;
                    self.patch2.inversion_end = self.patch2.base_to_invert << 1;
                    inside_base = self.patch2.base_to_invert;
                    outside_base = outside_edge_base;
                    self.using_patch2 = true;
                } else {
                    inside_base = inside_edge_base;
                    outside_base = outside_edge_base;
                }
                if ring == 1 {
                    self.stitch_transition(
                        self.num_indices,
                        Row {
                            base: inside_base,
                            half_points: q.inside_ctx[parity].num_half_points,
                            parity: q.inside_parity[parity],
                        },
                        Row {
                            base: outside_base,
                            half_points: outside_ctx[edge].num_half_points,
                            parity: outside_parity[edge],
                        },
                    );
                } else {
                    self.stitch_regular(
                        true,
                        Diagonals::Mirrored,
                        self.num_indices,
                        points_for_inside_edge[parity],
                        inside_base,
                        outside_base,
                    );
                }
                self.using_patch = false;
                self.using_patch2 = false;
                self.num_indices += triangles * 3;
                outside_edge_base += points_for_outside_edge[edge] - 1;
                if edge == 2 && ring == degenerate_ring[parity] {
                    inside_edge_base -= points_for_inside_edge[parity] - 1;
                } else {
                    inside_edge_base += points_for_inside_edge[parity] - 1;
                }
                points_for_outside_edge[edge] = points_for_inside_edge[parity];
            }
            if ring == 1 {
                for edge in 0..4usize {
                    outside_ctx[edge] = q.inside_ctx[edge & 1];
                    outside_parity[edge] = q.inside_parity[edge & 1];
                }
            }
        }
        // the center: a row of quads if odd
        if q.points_for_inside[0] > q.points_for_inside[1] && q.inside_parity[1] == Parity::Odd {
            self.using_patch2 = true;
            let strip_quads = (((q.points_for_inside[0] >> 1) - (q.points_for_inside[1] >> 1)) << 1)
                + if q.inside_parity[0] == Parity::Even { 2 } else { 1 };
            self.patch2.base_to_invert = outside_edge_base + strip_quads + 2;
            self.patch2.corner_bad = self.patch2.base_to_invert;
            self.patch2.corner_replacement = outside_edge_base;
            self.patch2.inversion_end = self.patch2.base_to_invert + self.patch2.base_to_invert + strip_quads;
            self.stitch_regular(
                false,
                Diagonals::InsideToOutside,
                self.num_indices,
                strip_quads + 1,
                self.patch2.base_to_invert,
                outside_edge_base + 1,
            );
            self.using_patch2 = false;
            self.num_indices += strip_quads * 6;
        } else if q.points_for_inside[1] >= q.points_for_inside[0] && q.inside_parity[0] == Parity::Odd {
            self.using_patch2 = true;
            let strip_quads = (((q.points_for_inside[1] >> 1) - (q.points_for_inside[0] >> 1)) << 1)
                + if q.inside_parity[1] == Parity::Even { 2 } else { 1 };
            self.patch2.base_to_invert = outside_edge_base + strip_quads + 1;
            self.patch2.corner_bad = -1;
            self.patch2.inversion_end = self.patch2.base_to_invert + self.patch2.base_to_invert + strip_quads;
            let diagonals = if q.inside_parity[1] == Parity::Even {
                Diagonals::InsideToOutside
            } else {
                Diagonals::InsideToOutsideExceptMiddle
            };
            self.stitch_regular(
                false,
                diagonals,
                self.num_indices,
                strip_quads + 1,
                self.patch2.base_to_invert,
                outside_edge_base,
            );
            self.using_patch2 = false;
            self.num_indices += strip_quads * 6;
        }
    }

    // --- triangles ---

    fn triangle(&mut self, u0: f32, v0: f32, w0: f32, inside: f32) {
        let t = self.tri_process(u0, v0, w0, inside);
        if t.culled {
            self.num_points = 0;
            self.num_indices = 0;
            return;
        }
        if t.just_minimum {
            self.define_point(0, FXP_ONE, 0);
            self.define_point(0, 0, 1);
            self.define_point(FXP_ONE, 0, 2);
            self.num_points = 3;
            match self.output {
                OutputPrimitive::TriangleCw | OutputPrimitive::TriangleCcw => {
                    self.define_clockwise_triangle(0, 1, 2, self.num_indices);
                    self.num_indices = 3;
                }
                OutputPrimitive::Point => self.dump_all_points(),
                OutputPrimitive::Line => self.dump_all_points_as_in_order_line_list(),
            }
            return;
        }
        self.tri_generate_points(&t);
        match self.output {
            OutputPrimitive::Point => self.dump_all_points(),
            OutputPrimitive::Line => self.dump_all_points_as_in_order_line_list(),
            _ => self.tri_generate_connectivity(&t),
        }
    }

    fn tri_process(&mut self, mut u0: f32, mut v0: f32, mut w0: f32, mut inside: f32) -> Tri {
        let mut t = Tri {
            outside: [0; 3],
            inside: 0,
            outside_parity: [Parity::Even; 3],
            inside_parity: Parity::Even,
            outside_ctx: [FactorContext::default(); 3],
            inside_ctx: FactorContext::default(),
            just_minimum: false,
            culled: false,
            points_for_outside_edge: [0; 3],
            points_for_inside: 0,
            inside_edge_point_base: 0,
        };
        if culls(u0) || culls(v0) || culls(w0) {
            t.culled = true;
            return t;
        }
        let (mut lower, upper) = self.bounds();
        u0 = fmin(upper, fmax(lower, u0));
        v0 = fmin(upper, fmax(lower, v0));
        w0 = fmin(upper, fmax(lower, w0));
        if self.hw_integer() {
            u0 = u0.ceil();
            v0 = v0.ceil();
            w0 = w0.ceil();
        }
        if self.original_partitioning == Partitioning::FractionalOdd {
            let limit = MIN_ODD + EPSILON / 2.0;
            // (a triangle has only one inside factor, so the inside factor itself is not checked)
            if u0 > limit || v0 > limit || w0 > limit {
                lower = MIN_ODD + EPSILON;
            }
        }
        inside = fmin(upper, fmax(lower, inside));
        if self.hw_integer() {
            inside = inside.ceil();
        }
        self.num_points = 0;
        self.num_indices = 0;
        let outside = [u0, v0, w0];
        if self.hw_integer() {
            t.outside_parity = outside.map(|f| if is_even(f) { Parity::Even } else { Parity::Odd });
            t.inside_parity = if is_even(inside) || inside == 1.0 {
                Parity::Even
            } else {
                Parity::Odd
            };
        } else {
            t.outside_parity = [self.original_parity; 3];
            t.inside_parity = self.original_parity;
        }
        t.outside = outside.map(float_to_fixed);
        t.inside = float_to_fixed(inside);
        if (self.hw_integer() || self.odd()) && t.inside == FXP_ONE && t.outside.iter().all(|&f| f == FXP_ONE)
        {
            t.just_minimum = true;
            return t;
        }
        t.just_minimum = false;
        for e in 0..3 {
            self.parity = t.outside_parity[e];
            t.outside_ctx[e] = self.factor_context(t.outside[e]);
        }
        self.parity = t.inside_parity;
        t.inside_ctx = self.factor_context(t.inside);
        for e in 0..3 {
            self.parity = t.outside_parity[e];
            t.points_for_outside_edge[e] = self.num_points_for_factor(t.outside[e]);
            self.num_points += t.points_for_outside_edge[e];
        }
        self.num_points -= 3;
        self.parity = t.inside_parity;
        t.points_for_inside = self.num_points_for_factor(t.inside);
        let min = if self.odd() { 4 } else { 3 };
        t.points_for_inside = min.max(t.points_for_inside);
        t.inside_edge_point_base = self.num_points;
        let interior_rings = (t.points_for_inside >> 1) - 1;
        let interior_points = if self.odd() {
            3 * (interior_rings * (interior_rings + 1) - interior_rings)
        } else {
            3 * (interior_rings * (interior_rings + 1)) + 1
        };
        self.num_points += interior_points;
        t
    }

    fn tri_generate_points(&mut self, t: &Tri) {
        // the exterior ring's edge points, clockwise from point V (edge VW, the U = 0 edge)
        let mut offset = 0;
        for edge in 0..3usize {
            let parity = edge & 1;
            let end = t.points_for_outside_edge[edge] - 1;
            for p in 0..end {
                // edge 0, VW, has V decreasing: reverse; edge 1, WU, has U increasing; edge 2, UV, has U decreasing
                let n = if parity != 0 { p } else { end - p };
                self.parity = t.outside_parity[edge];
                let param = self.place_point_in_1d(&t.outside_ctx[edge], n);
                if edge == 0 {
                    self.define_point(0, param, offset);
                } else {
                    self.define_point(
                        param,
                        if edge == 2 { FXP_ONE.wrapping_sub(param) } else { 0 },
                        offset,
                    );
                }
                offset += 1;
            }
        }
        // the interior rings, clockwise spiralling in
        self.parity = t.inside_parity;
        let rings = t.points_for_inside >> 1;
        for ring in 1..rings {
            let start = ring;
            let end = t.points_for_inside - 1 - start;
            for edge in 0..3usize {
                let parity = edge & 1;
                let mut perpendicular = self.place_point_in_1d(&t.inside_ctx, start);
                // maps the location to the right size in barycentric space
                perpendicular = perpendicular.wrapping_mul(FXP_TWO_THIRDS);
                perpendicular = perpendicular.wrapping_add(FXP_ONE_HALF) >> FRACTION_BITS;
                for p in start..end {
                    let n = if parity != 0 { p } else { end - (p - start) };
                    let param = self.place_point_in_1d(&t.inside_ctx, n);
                    // the rate of change of edge-parallel parameters as they are pushed into the triangle
                    let pushed = param.wrapping_sub(perpendicular.wrapping_add(1) / 2);
                    match edge {
                        0 => self.define_point(perpendicular, pushed, offset),
                        1 => self.define_point(pushed, perpendicular, offset),
                        _ => self.define_point(
                            pushed,
                            FXP_ONE.wrapping_sub(pushed).wrapping_sub(perpendicular),
                            offset,
                        ),
                    }
                    offset += 1;
                }
            }
        }
        if !self.odd() {
            // the last point is the center
            self.define_point(FXP_ONE_THIRD, FXP_ONE_THIRD, offset);
        }
    }

    fn tri_generate_connectivity(&mut self, t: &Tri) {
        let rings = (t.points_for_inside + 1) >> 1; // so that even tessellation includes the center point
        let mut outside_ctx = t.outside_ctx;
        let mut outside_parity = t.outside_parity;
        let mut points_for_outside_edge = t.points_for_outside_edge;
        let mut inside_edge_base = t.inside_edge_point_base;
        let mut outside_edge_base = 0;
        for ring in 1..rings {
            let points_for_inside_edge = t.points_for_inside - 2 * ring;
            let edge0_inside_base = inside_edge_base;
            let edge0_outside_base = outside_edge_base;
            for edge in 0..3usize {
                let triangles = points_for_inside_edge + points_for_outside_edge[edge] - 2;
                let (inside_base, outside_base) = if edge == 2 {
                    self.patch.inside_delta = inside_edge_base;
                    self.patch.inside_bad = points_for_inside_edge - 1;
                    self.patch.inside_replacement = edge0_inside_base;
                    self.patch.outside_base = self.patch.inside_bad + 1;
                    self.patch.outside_delta = outside_edge_base - self.patch.outside_base;
                    self.patch.outside_bad = self.patch.outside_base + points_for_outside_edge[edge] - 1;
                    self.patch.outside_replacement = edge0_outside_base;
                    self.using_patch = true;
                    (0, self.patch.outside_base)
                } else {
                    (inside_edge_base, outside_edge_base)
                };
                if ring == 1 {
                    self.stitch_transition(
                        self.num_indices,
                        Row {
                            base: inside_base,
                            half_points: t.inside_ctx.num_half_points,
                            parity: t.inside_parity,
                        },
                        Row {
                            base: outside_base,
                            half_points: outside_ctx[edge].num_half_points,
                            parity: outside_parity[edge],
                        },
                    );
                } else {
                    self.stitch_regular(
                        true,
                        Diagonals::Mirrored,
                        self.num_indices,
                        points_for_inside_edge,
                        inside_base,
                        outside_base,
                    );
                }
                if edge == 2 {
                    self.using_patch = false;
                }
                self.num_indices += triangles * 3;
                outside_edge_base += points_for_outside_edge[edge] - 1;
                inside_edge_base += points_for_inside_edge - 1;
                points_for_outside_edge[edge] = points_for_inside_edge;
            }
            if ring == 1 {
                for edge in 0..3usize {
                    outside_ctx[edge] = t.inside_ctx;
                    outside_parity[edge] = t.inside_parity;
                }
            }
        }
        if self.odd() {
            // the center is one triangle
            self.define_clockwise_triangle(
                outside_edge_base,
                outside_edge_base + 1,
                outside_edge_base + 2,
                self.num_indices,
            );
            self.num_indices += 3;
        }
    }

    // --- isolines ---

    fn isolines(&mut self, density: f32, detail: f32) {
        let l = self.isoline_process(density, detail);
        if l.culled {
            self.num_points = 0;
            self.num_indices = 0;
            return;
        }
        // the points: every line's points in order
        let mut offset = 0;
        for line in 0..l.lines {
            for point in 0..l.points_per_line {
                self.parity = l.line_density_parity;
                let v = self.place_point_in_1d(&l.line_density_ctx, line);
                self.parity = l.line_detail_parity;
                let u = self.place_point_in_1d(&l.line_detail_ctx, point);
                self.define_point(u, v, offset);
                offset += 1;
            }
        }
        // the connections
        let (mut point_offset, mut index_offset) = (0, 0);
        if self.output == OutputPrimitive::Point {
            for _ in 0..l.lines {
                for _ in 0..l.points_per_line {
                    self.define_index(point_offset, index_offset);
                    point_offset += 1;
                    index_offset += 1;
                }
            }
        } else {
            for _ in 0..l.lines {
                for point in 0..l.points_per_line {
                    if point > 0 {
                        self.define_index(point_offset - 1, index_offset);
                        index_offset += 1;
                        self.define_index(point_offset, index_offset);
                        index_offset += 1;
                    }
                    point_offset += 1;
                }
            }
        }
    }

    fn isoline_process(&mut self, mut density: f32, mut detail: f32) -> Isolines {
        let mut l = Isolines {
            line_density_parity: Parity::Even,
            line_detail_parity: Parity::Even,
            line_density_ctx: FactorContext::default(),
            line_detail_ctx: FactorContext::default(),
            culled: false,
            points_per_line: 0,
            lines: 0,
        };
        if culls(density) || culls(detail) {
            l.culled = true;
            return l;
        }
        let (lower, upper) = self.bounds();
        density = fmin(MAX_ISOLINE_DENSITY, fmax(MIN_ISOLINE_DENSITY, density));
        detail = fmin(upper, fmax(lower, detail));
        self.num_points = 0;
        self.num_indices = 0;
        if self.hw_integer() {
            detail = detail.ceil();
            l.line_detail_parity = if is_even(detail) {
                Parity::Even
            } else {
                Parity::Odd
            };
        } else {
            l.line_detail_parity = self.original_parity;
        }
        let fxp_detail = float_to_fixed(detail);
        self.parity = l.line_detail_parity;
        l.line_detail_ctx = self.factor_context(fxp_detail);
        l.points_per_line = self.num_points_for_factor(fxp_detail);
        // the line density is always integer partitioned
        self.partitioning = Partitioning::Integer;
        density = density.ceil();
        l.line_density_parity = if is_even(density) {
            Parity::Even
        } else {
            Parity::Odd
        };
        self.parity = l.line_density_parity;
        let fxp_density = float_to_fixed(density);
        l.line_density_ctx = self.factor_context(fxp_density);
        // the last line, at v = 1, is not drawn
        l.lines = self.num_points_for_factor(fxp_density) - 1;
        self.partitioning = self.original_partitioning;
        self.num_points = l.points_per_line * l.lines;
        self.num_indices = if self.output == OutputPrimitive::Point {
            self.num_points
        } else {
            l.lines * (l.points_per_line - 1) * 2
        };
        l
    }
}
