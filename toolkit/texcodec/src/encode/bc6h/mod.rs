//! BC6H blocks: the encoder of DirectXTex's BC6HBC7.cpp (MIT licence, Copyright (c) Microsoft Corporation), ported.
//!
//! Texels are taken as half-float bit patterns read as integers (finite halves: 0..=0x7bff, signed in -0x7bff..=0x7bff
//! for SF16), where the block's arithmetic happens and where the error is measured, as squared differences. For each of
//! the 14 modes, each shape (the 32 partitions for two-region modes) gets rough endpoints (a Newton fit of the texels'
//! colour line) and a rough error with unquantized endpoints; the best quarter of the shapes (the one shape of a
//! one-region mode) is refined: the endpoints are quantized to the mode's precision, each texel takes its nearest palette
//! entry, the endpoints are swapped where an anchor texel's index would need its high bit, and a mode whose endpoints
//! (or deltas from the first) do not fit its bits is passed over. Each endpoint channel is then searched (steps halving
//! from half the precision) while the error falls, and the best block over all modes is kept; a block of error zero
//! ends the search.
//!
//! Three slips of DirectXTex's are fixed here. Its endpoint search gathers region 0's texels from the one-region
//! partition table, so in two-region modes region 0 is fitted to all sixteen texels; here each region is fitted to its
//! own. Its search keeps quantized endpoints at or above zero, which for SF16 leaves negative endpoints where the first
//! quantization put them; here signed endpoints range over the whole precision. And at full precision (16 bits, mode
//! 0x0f) it takes the half pattern itself as the quantized endpoint, which decodes to 31/64 of it (31/32 for SF16);
//! here the full-precision endpoint is the one that decodes back to the pattern.
//!
//! The palette is searched whole for each texel (DirectXTex stops at the first entry farther than the one before).
mod tables;

use tables::{ANCHORS, LAYOUT, SHAPES};

/// The largest finite half's bit pattern.
const F16MAX: i32 = 0x7bff;

/// A mode, in DirectXTex's order.
struct Mode {
    /// The mode's bits at the start of the block.
    code: u8,
    two: bool,
    /// The endpoints after the first are stored as differences from it.
    transformed: bool,
    index_bits: u32,
    /// The first endpoint's bits, per channel.
    base: u32,
    /// The other endpoints' bits, per channel.
    other: [u32; 3],
}

const fn mode(code: u8, two: bool, transformed: bool, index_bits: u32, base: u32, other: [u32; 3]) -> Mode {
    Mode {
        code,
        two,
        transformed,
        index_bits,
        base,
        other,
    }
}

const MODES: [Mode; 14] = [
    mode(0x00, true, true, 3, 10, [5, 5, 5]),
    mode(0x01, true, true, 3, 7, [6, 6, 6]),
    mode(0x02, true, true, 3, 11, [5, 4, 4]),
    mode(0x06, true, true, 3, 11, [4, 5, 4]),
    mode(0x0a, true, true, 3, 11, [4, 4, 5]),
    mode(0x0e, true, true, 3, 9, [5, 5, 5]),
    mode(0x12, true, true, 3, 8, [6, 5, 5]),
    mode(0x16, true, true, 3, 8, [5, 6, 5]),
    mode(0x1a, true, true, 3, 8, [5, 5, 6]),
    mode(0x1e, true, false, 3, 6, [6, 6, 6]),
    mode(0x03, false, false, 4, 10, [10, 10, 10]),
    mode(0x07, false, true, 4, 11, [9, 9, 9]),
    mode(0x0b, false, true, 4, 12, [8, 8, 8]),
    mode(0x0f, false, true, 4, 16, [4, 4, 4]),
];

const WEIGHTS3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

type Colour = [i32; 3];

#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Pair {
    a: Colour,
    b: Colour,
}

/// The region of texel `i` in `shape` (always 0 in one-region modes).
fn region(two: bool, shape: usize, i: usize) -> usize {
    usize::from(two && SHAPES[shape] >> i & 1 == 1)
}

/// Texel `i` holds an anchor index (one bit shorter, high bit clear).
fn anchor(two: bool, shape: usize, i: usize) -> bool {
    i == 0 || (two && i == usize::from(ANCHORS[shape]))
}

/// A float as a half's bit pattern read as an integer: negative values are 0 for UF16; magnitudes stop at the largest
/// finite half.
fn to_int(v: f32, signed: bool) -> i32 {
    let h = half::f16::from_f32(v).to_bits();
    let magnitude = i32::from(h & 0x7fff).min(F16MAX);
    match (h & 0x8000 != 0, signed) {
        (false, _) => magnitude,
        (true, true) => -magnitude,
        (true, false) => 0,
    }
}

/// An endpoint channel at `bits` precision (DirectXTex's `Quantize`, but for full precision; see the module notes).
fn quantize(v: i32, bits: u32, signed: bool) -> i32 {
    if signed {
        let m = v.abs();
        let q = if bits >= 16 {
            // the magnitude whose 31/32 is `m`
            ((m * 32 + 30) / 31).min(0x7fff)
        } else {
            (m << (bits - 1)) / (F16MAX + 1)
        };
        if v < 0 { -q } else { q }
    } else if bits >= 15 {
        // the value whose 31/64 is `v`
        ((v * 64 + 30) / 31).min(0xffff)
    } else {
        (v << bits) / (F16MAX + 1)
    }
}

/// A quantized channel back to 16 bits (the decoder's arithmetic).
fn unquantize(q: i32, bits: u32, signed: bool) -> i32 {
    if signed {
        if bits >= 16 {
            return q;
        }
        let m = q.abs();
        let u = if m == 0 {
            0
        } else if m >= (1 << (bits - 1)) - 1 {
            0x7fff
        } else {
            ((m << 15) + 0x4000) >> (bits - 1)
        };
        if q < 0 { -u } else { u }
    } else if bits >= 15 {
        q
    } else if q == 0 {
        0
    } else if q == (1 << bits) - 1 {
        0xffff
    } else {
        ((q << 16) + 0x8000) >> bits
    }
}

/// An interpolated 16-bit value to the half pattern the decoder outputs.
fn finish(v: i32, signed: bool) -> i32 {
    if !signed {
        (v * 31) >> 6
    } else if v < 0 {
        -((-v * 31) >> 5)
    } else {
        (v * 31) >> 5
    }
}

/// The bits `n` needs (one more for a sign when `signed`).
fn bits_of(n: i32, signed: bool) -> u32 {
    match n {
        0 => 0,
        n if n > 0 => 32 - n.leading_zeros() + u32::from(signed),
        // the bits of !n, and the sign
        n => 32 - (!n).leading_zeros() + 1,
    }
}

fn weights(index_bits: u32) -> &'static [i32] {
    if index_bits == 3 { &WEIGHTS3 } else { &WEIGHTS4 }
}

/// The squared distance of two colours.
fn distance(a: Colour, b: Colour) -> i64 {
    (0..3).map(|c| i64::from(a[c] - b[c]).pow(2)).sum()
}

/// The error of `points` against `palette`, each taking its nearest entry.
fn map(points: impl Iterator<Item = Colour>, palette: &[Colour]) -> i64 {
    points
        .map(|p| palette.iter().map(|&e| distance(p, e)).min().unwrap_or(0))
        .sum()
}

/// The palette of quantized endpoints, as the decoder makes it.
fn palette(mode: &Mode, pair: &Pair, signed: bool) -> ([Colour; 16], usize) {
    let w = weights(mode.index_bits);
    let a = pair.a.map(|v| unquantize(v, mode.base, signed));
    let b = pair.b.map(|v| unquantize(v, mode.base, signed));
    let mut out = [[0; 3]; 16];
    for (i, entry) in out.iter_mut().take(w.len()).enumerate() {
        *entry = std::array::from_fn(|c| finish((a[c] * (64 - w[i]) + b[c] * w[i] + 32) >> 6, signed));
    }
    (out, w.len())
}

/// A block's texels: as given (clamped) for the rough fit, and as half patterns.
struct Texels {
    signed: bool,
    floats: [[f32; 3]; 16],
    ints: [Colour; 16],
}

/// DirectXTex's `OptimizeRGB` for BC6H's rough pass: the endpoints of a four-step line through `points`, by Newton's
/// method from the diagonal of their bounding box that best follows them.
fn optimize_rgb(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    const C: [f32; 4] = [1., 2. / 3., 1. / 3., 0.];
    const D: [f32; 4] = [0., 1. / 3., 2. / 3., 1.];
    const EPSILON: f32 = (0.25 / 64.) * (0.25 / 64.);
    let mut x = [f32::MAX; 3];
    let mut y = [-f32::MAX; 3];
    for p in points {
        for c in 0..3 {
            x[c] = x[c].min(p[c]);
            y[c] = y[c].max(p[c]);
        }
    }
    let ab: [f32; 3] = std::array::from_fn(|c| y[c] - x[c]);
    let f_ab = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    if f_ab < f32::MIN_POSITIVE {
        return (x, y);
    }
    let dir = ab.map(|v| v / f_ab);
    let mid: [f32; 3] = std::array::from_fn(|c| (x[c] + y[c]) * 0.5);
    let mut f_dir = [0f32; 4];
    for p in points {
        let pt: [f32; 3] = std::array::from_fn(|c| (p[c] - mid[c]) * dir[c]);
        for (k, f) in [
            pt[0] + pt[1] + pt[2],
            pt[0] + pt[1] - pt[2],
            pt[0] - pt[1] + pt[2],
            pt[0] - pt[1] - pt[2],
        ]
        .into_iter()
        .enumerate()
        {
            f_dir[k] += f * f;
        }
    }
    let mut best = 0;
    for k in 1..4 {
        if f_dir[k] > f_dir[best] {
            best = k;
        }
    }
    if best & 2 != 0 {
        std::mem::swap(&mut x[1], &mut y[1]);
    }
    if best & 1 != 0 {
        std::mem::swap(&mut x[2], &mut y[2]);
    }
    if f_ab < 1. / 4096. {
        return (x, y);
    }
    for _ in 0..8 {
        let steps: [[f32; 3]; 4] =
            std::array::from_fn(|s| std::array::from_fn(|c| x[c] * C[s] + y[c] * D[s]));
        let d: [f32; 3] = std::array::from_fn(|c| y[c] - x[c]);
        let len = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if len < 1. / 4096. {
            break;
        }
        let d = d.map(|v| v * 3. / len);
        let (mut d2x, mut d2y) = (0f32, 0f32);
        let (mut dx, mut dy) = ([0f32; 3], [0f32; 3]);
        for p in points {
            let dot = (p[0] - x[0]) * d[0] + (p[1] - x[1]) * d[1] + (p[2] - x[2]) * d[2];
            let s = if dot <= 0. {
                0
            } else if dot >= 3. {
                3
            } else {
                (dot + 0.5) as usize
            };
            let fc = C[s] / 8.;
            let fd = D[s] / 8.;
            d2x += fc * C[s];
            d2y += fd * D[s];
            for c in 0..3 {
                let diff = steps[s][c] - p[c];
                dx[c] += fc * diff;
                dy[c] += fd * diff;
            }
        }
        if d2x > 0. {
            for c in 0..3 {
                x[c] -= dx[c] / d2x;
            }
        }
        if d2y > 0. {
            for c in 0..3 {
                y[c] -= dy[c] / d2y;
            }
        }
        if dx.iter().chain(&dy).all(|v| v * v < EPSILON) {
            break;
        }
    }
    (x, y)
}

/// The rough endpoints of each region of `shape` (unquantized half patterns) and their error with the unquantized
/// palette of `index_bits` (DirectXTex's `RoughMSE`).
fn rough(texels: &Texels, two: bool, shape: usize, index_bits: u32) -> (i64, [Pair; 2]) {
    let (lo, hi) = if texels.signed {
        (-F16MAX, F16MAX)
    } else {
        (0, F16MAX)
    };
    let w = weights(index_bits);
    let mut ends = [Pair::default(); 2];
    let mut error = 0;
    for (r, end) in ends.iter_mut().enumerate().take(1 + usize::from(two)) {
        let members: Vec<usize> = (0..16).filter(|&i| region(two, shape, i) == r).collect();
        match members[..] {
            [] => continue,
            [i] => {
                *end = Pair {
                    a: texels.ints[i],
                    b: texels.ints[i],
                }
            }
            [i, j] => {
                *end = Pair {
                    a: texels.ints[i],
                    b: texels.ints[j],
                }
            }
            _ => {
                let points: Vec<[f32; 3]> = members.iter().map(|&i| texels.floats[i]).collect();
                let (x, y) = optimize_rgb(&points);
                *end = Pair {
                    a: x.map(|v| to_int(v, texels.signed).clamp(lo, hi)),
                    b: y.map(|v| to_int(v, texels.signed).clamp(lo, hi)),
                };
                let entries: Vec<Colour> = w
                    .iter()
                    .map(|&w| std::array::from_fn(|c| (end.a[c] * (64 - w) + end.b[c] * w + 32) >> 6))
                    .collect();
                error += map(members.iter().map(|&i| texels.ints[i]), &entries);
            }
        }
    }
    (error, ends)
}

/// A candidate block being refined: its mode (and its place in [`MODES`]), shape and the texels.
struct Fit<'a> {
    index: usize,
    mode: &'a Mode,
    shape: usize,
    texels: &'a Texels,
}

impl Fit<'_> {
    fn regions(&self) -> usize {
        1 + usize::from(self.mode.two)
    }

    fn members(&self, r: usize) -> impl Iterator<Item = Colour> + '_ {
        (0..16)
            .filter(move |&i| region(self.mode.two, self.shape, i) == r)
            .map(|i| self.texels.ints[i])
    }

    /// Each texel's nearest entry, and the error of each region.
    fn assign(&self, ends: &[Pair; 2]) -> ([u8; 16], [i64; 2]) {
        let palettes = ends.map(|p| palette(self.mode, &p, self.texels.signed));
        let mut indices = [0u8; 16];
        let mut errors = [0i64; 2];
        for (i, &t) in self.texels.ints.iter().enumerate() {
            let r = region(self.mode.two, self.shape, i);
            let (entries, n) = &palettes[r];
            let mut best = (i64::MAX, 0);
            for (j, &e) in entries[..*n].iter().enumerate() {
                let d = distance(t, e);
                if d < best.0 {
                    best = (d, j);
                }
            }
            indices[i] = best.1 as u8;
            errors[r] += best.0;
        }
        (indices, errors)
    }

    /// Endpoints swapped where an anchor's index has its high bit set, and those regions' indices mirrored.
    fn swap(&self, ends: &mut [Pair; 2], indices: &mut [u8; 16]) {
        let n = 1u8 << self.mode.index_bits;
        for (r, end) in ends.iter_mut().enumerate().take(self.regions()) {
            let at = if r == 0 {
                0
            } else {
                usize::from(ANCHORS[self.shape])
            };
            if indices[at] & n >> 1 != 0 {
                std::mem::swap(&mut end.a, &mut end.b);
                for (i, index) in indices.iter_mut().enumerate() {
                    if region(self.mode.two, self.shape, i) == r {
                        *index = n - 1 - *index;
                    }
                }
            }
        }
    }

    /// The endpoints as stored: after the first, differences from it in transformed modes.
    fn stored(&self, ends: &[Pair; 2]) -> [Pair; 2] {
        if !self.mode.transformed {
            return *ends;
        }
        let base = ends[0].a;
        let less = |v: Colour| -> Colour { std::array::from_fn(|c| v[c] - base[c]) };
        [
            Pair {
                a: base,
                b: less(ends[0].b),
            },
            Pair {
                a: less(ends[1].a),
                b: less(ends[1].b),
            },
        ]
    }

    /// The stored endpoints fit the mode's bits.
    fn fits(&self, stored: &[Pair; 2]) -> bool {
        let signed = self.texels.signed;
        let other = self.mode.transformed || signed;
        (0..3).all(|c| {
            bits_of(stored[0].a[c], signed) <= self.mode.base
                && bits_of(stored[0].b[c], other) <= self.mode.other[c]
                && (!self.mode.two
                    || (bits_of(stored[1].a[c], other) <= self.mode.other[c]
                        && bits_of(stored[1].b[c], other) <= self.mode.other[c]))
        })
    }

    /// The error of region `r`'s texels with endpoints `pair`.
    fn error(&self, r: usize, pair: &Pair) -> i64 {
        let (entries, n) = palette(self.mode, pair, self.texels.signed);
        map(self.members(r), &entries[..n])
    }

    /// The range of a quantized endpoint channel.
    fn range(&self) -> (i32, i32) {
        let bits = self.mode.base;
        if self.texels.signed {
            let m = (1 << (bits - 1)) - 1;
            (-m, m)
        } else {
            (0, (1 << bits) - 1)
        }
    }

    /// DirectXTex's `PerturbOne`: endpoint `which` (0 for A) of channel `c` moved by halving steps while the error of
    /// region `r` falls; the lowest error and the channel's value there.
    fn perturb(&self, r: usize, pair: &Pair, c: usize, which: usize, error: i64) -> (i64, i32) {
        let (lo, hi) = self.range();
        let mut best = error;
        let mut value = if which == 0 { pair.a[c] } else { pair.b[c] };
        let mut trial = *pair;
        let mut step = 1 << (self.mode.base - 1);
        while step > 0 {
            let mut moved = 0;
            for sign in [-1, 1] {
                let v = value + sign * step;
                if v < lo || v > hi {
                    continue;
                }
                if which == 0 {
                    trial.a[c] = v;
                } else {
                    trial.b[c] = v;
                }
                let e = self.error(r, &trial);
                if e < best {
                    best = e;
                    moved = sign * step;
                }
            }
            value += moved;
            if which == 0 {
                trial.a[c] = value;
            } else {
                trial.b[c] = value;
            }
            step >>= 1;
        }
        (best, value)
    }

    /// DirectXTex's `OptimizeOne`: each channel, starting with the endpoint whose search lowers the error most, then
    /// alternating endpoints while the error falls.
    fn optimize(&self, r: usize, pair: Pair, error: i64) -> Pair {
        let mut pair = pair;
        let mut error = error;
        for c in 0..3 {
            let (ea, va) = self.perturb(r, &pair, c, 0, error);
            let (eb, vb) = self.perturb(r, &pair, c, 1, error);
            let mut which = if ea < eb {
                if ea >= error {
                    continue;
                }
                pair.a[c] = va;
                error = ea;
                1
            } else {
                if eb >= error {
                    continue;
                }
                pair.b[c] = vb;
                error = eb;
                0
            };
            loop {
                let (e, v) = self.perturb(r, &pair, c, which, error);
                if e >= error {
                    break;
                }
                if which == 0 {
                    pair.a[c] = v;
                } else {
                    pair.b[c] = v;
                }
                error = e;
                which = 1 - which;
            }
        }
        pair
    }

    /// The block's bytes.
    fn emit(&self, stored: &[Pair; 2], indices: &[u8; 16]) -> [u8; 16] {
        let header = if self.mode.two { 82 } else { 65 };
        let endpoint = |e: usize| -> Colour {
            let pair = &stored[e / 2];
            if e.is_multiple_of(2) { pair.a } else { pair.b }
        };
        let mut bits = 0u128;
        for (k, &[field, bit]) in LAYOUT[self.index].iter().take(header).enumerate() {
            let value = match field {
                0 => 0,
                1 => i32::from(self.mode.code),
                2 => self.shape as i32,
                f => {
                    let f = usize::from(f - 3);
                    endpoint(f / 3)[f % 3]
                }
            };
            bits |= u128::from((value >> bit) as u32 & 1) << k;
        }
        let mut at = header;
        for (i, &index) in indices.iter().enumerate() {
            bits |= u128::from(index) << at;
            at += self.mode.index_bits as usize - usize::from(anchor(self.mode.two, self.shape, i));
        }
        bits.to_le_bytes()
    }

    /// DirectXTex's `Refine`: the block from the rough endpoints `rough`, and its error, unless they do not fit.
    fn refine(&self, rough: &[Pair; 2]) -> Option<(i64, [u8; 16])> {
        let mut ends = rough.map(|p| Pair {
            a: p.a.map(|v| quantize(v, self.mode.base, self.texels.signed)),
            b: p.b.map(|v| quantize(v, self.mode.base, self.texels.signed)),
        });
        let (mut indices, errors) = self.assign(&ends);
        self.swap(&mut ends, &mut indices);
        let stored = self.stored(&ends);
        if !self.fits(&stored) {
            return None;
        }
        let mut better = ends;
        for (r, pair) in better.iter_mut().enumerate().take(self.regions()) {
            *pair = self.optimize(r, *pair, errors[r]);
        }
        let (mut better_indices, better_errors) = self.assign(&better);
        self.swap(&mut better, &mut better_indices);
        let better_stored = self.stored(&better);
        let (total, better_total) = (errors[0] + errors[1], better_errors[0] + better_errors[1]);
        Some(if better_total < total && self.fits(&better_stored) {
            (better_total, self.emit(&better_stored, &better_indices))
        } else {
            (total, self.emit(&stored, &indices))
        })
    }
}

/// Encodes 16 RGB texels as a BC6H block, UF16 or SF16 (`signed`). Values are clamped to the format's range (UF16: 0 to
/// 65504; SF16: -65504 to 65504); NaN is 0.
pub(super) fn encode(rgb: &[[f32; 3]; 16], signed: bool) -> [u8; 16] {
    let low = if signed { -65504. } else { 0. };
    let floats = rgb.map(|p| p.map(|v| if v.is_nan() { 0. } else { v.clamp(low, 65504.) }));
    let texels = Texels {
        signed,
        floats,
        ints: floats.map(|p| p.map(|v| to_int(v, signed))),
    };
    // the rough pass depends only on the shape and the palette size, which every mode of a region count shares
    let one = rough(&texels, false, 0, 4);
    let two: [(i64, [Pair; 2]); 32] = std::array::from_fn(|s| rough(&texels, true, s, 3));
    let mut best: Option<(i64, [u8; 16])> = None;
    for (index, mode) in MODES.iter().enumerate() {
        if best.is_some_and(|(e, _)| e == 0) {
            break;
        }
        let shapes: Vec<usize> = if mode.two {
            // DirectXTex's partial bubble sort: the best eight shapes, in its order
            let mut errors: [i64; 32] = two.map(|(e, _)| e);
            let mut order: [usize; 32] = std::array::from_fn(|s| s);
            for i in 0..8 {
                for j in i + 1..32 {
                    if errors[i] > errors[j] {
                        errors.swap(i, j);
                        order.swap(i, j);
                    }
                }
            }
            order[..8].to_vec()
        } else {
            vec![0]
        };
        for shape in shapes {
            if best.is_some_and(|(e, _)| e == 0) {
                break;
            }
            let fit = Fit {
                index,
                mode,
                shape,
                texels: &texels,
            };
            let ends = if mode.two { &two[shape].1 } else { &one.1 };
            if let Some((error, bytes)) = fit.refine(ends)
                && best.is_none_or(|(e, _)| error < e)
            {
                best = Some((error, bytes));
            }
        }
    }
    // mode 0x03 (ten-bit endpoints, stored whole) holds any quantized endpoints, so some mode always fits
    best.map_or([0; 16], |(_, bytes)| bytes)
}
