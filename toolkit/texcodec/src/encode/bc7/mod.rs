//! BC7 blocks, encoded here.
//!
//! Every block is searched over all eight modes, and the block of least squared error over its four channels (8-bit
//! levels, all weighted alike) is kept. A subset's endpoints start on the principal axis of its texels (their extremes
//! along it), are quantized under each choice of p-bits, and are then refitted: each texel takes its nearest palette
//! entry, the endpoints are solved by least squares for those indices and quantized again, while the error falls. Last,
//! each endpoint channel is moved a step either way and each p-bit flipped, while the error falls. Modes with
//! partitions fit every partition quickly (the axis's extremes, quantized once) and refine the few whose quick fit has
//! the least error. Modes 4 and 5 are tried with each rotation (mode 4 with each index selection too), their colour and alpha
//! fitted apart. A block of one colour is also tried as mode 5's exact one-colour block, which every colour has.
//!
//! The arithmetic is integer where it decides (palettes and errors) and plain `f32` elsewhere, without CPU-specific
//! dispatch: the same block gives the same bytes on any machine.
mod tables;

use tables::{ANCHORS2, ANCHORS3, PARTITIONS2, PARTITIONS3};

const WEIGHTS2: [i32; 4] = [0, 21, 43, 64];
const WEIGHTS3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// The interpolation weights of indices of `bits` bits.
fn weights(bits: u32) -> &'static [i32] {
    match bits {
        2 => &WEIGHTS2,
        3 => &WEIGHTS3,
        _ => &WEIGHTS4,
    }
}

/// How a mode stores the endpoints' lowest bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PBits {
    None,
    /// One for both endpoints of a subset.
    Shared,
    /// One per endpoint.
    Each,
}

/// A mode's layout.
struct Mode {
    subsets: usize,
    partition_bits: u32,
    /// Bits per colour channel of an endpoint, the p-bit apart.
    colour: u32,
    /// Bits per alpha channel of an endpoint (0: the mode has no alpha, which decodes as 255).
    alpha: u32,
    pbits: PBits,
    index: u32,
    /// The second index set's bits (modes 4 and 5).
    index2: u32,
}

const fn mode(
    subsets: usize,
    partition_bits: u32,
    colour: u32,
    alpha: u32,
    pbits: PBits,
    index: u32,
    index2: u32,
) -> Mode {
    Mode {
        subsets,
        partition_bits,
        colour,
        alpha,
        pbits,
        index,
        index2,
    }
}

const MODES: [Mode; 8] = [
    mode(3, 4, 4, 0, PBits::Each, 3, 0),
    mode(2, 6, 6, 0, PBits::Shared, 3, 0),
    mode(3, 6, 5, 0, PBits::None, 2, 0),
    mode(2, 6, 7, 0, PBits::Each, 2, 0),
    mode(1, 0, 5, 6, PBits::None, 2, 3),
    mode(1, 0, 7, 8, PBits::None, 2, 2),
    mode(1, 0, 7, 7, PBits::Each, 4, 0),
    mode(2, 6, 5, 5, PBits::Each, 2, 0),
];

/// How many of a mode's partitions, best by their quick fit first, are refined.
const PARTITIONS_REFINED: usize = 3;

/// The texel subsets of `partition` for a mode of `subsets` subsets.
fn subset_of(subsets: usize, partition: usize, texel: usize) -> usize {
    match subsets {
        2 => PARTITIONS2[partition][texel] as usize,
        3 => PARTITIONS3[partition][texel] as usize,
        _ => 0,
    }
}

/// The anchor texel of subset `s` (its index has one bit fewer).
fn anchor_of(subsets: usize, partition: usize, s: usize) -> usize {
    match (subsets, s) {
        (_, 0) => 0,
        (2, _) => ANCHORS2[partition] as usize,
        _ => ANCHORS3[partition][s - 1] as usize,
    }
}

/// Whether `texel` is the anchor of its subset.
fn anchor(subsets: usize, partition: usize, texel: usize) -> bool {
    (0..subsets).any(|s| anchor_of(subsets, partition, s) == texel)
}

/// The texels of subset `s` of `partition`: their values and their places in the block.
fn gather(
    texels: &[[i32; 4]; 16],
    subsets: usize,
    partition: usize,
    s: usize,
) -> ([[i32; 4]; 16], [usize; 16], usize) {
    let (mut at, mut places, mut n) = ([[0; 4]; 16], [0; 16], 0);
    for (i, t) in texels.iter().enumerate() {
        if subset_of(subsets, partition, i) == s {
            at[n] = *t;
            places[n] = i;
            n += 1;
        }
    }
    (at, places, n)
}

/// An endpoint quantization: `bits` per channel and a mode's p-bits.
#[derive(Clone, Copy)]
struct Quant {
    bits: u32,
    pbits: PBits,
}

impl Quant {
    fn max(self) -> i32 {
        (1 << self.bits) - 1
    }

    /// The 8-bit level of `code` with p-bit `p`.
    fn value(self, code: i32, p: i32) -> i32 {
        let (v, n) = match self.pbits {
            PBits::None => (code, self.bits),
            _ => (code << 1 | p, self.bits + 1),
        };
        if n == 8 {
            v
        } else {
            v << (8 - n) | v >> (2 * n - 8)
        }
    }

    /// The code whose level with p-bit `p` is nearest `v` (the lower of two as near).
    fn nearest(self, v: f32, p: i32) -> i32 {
        let n = if self.pbits == PBits::None {
            self.bits
        } else {
            self.bits + 1
        };
        let scaled = v.clamp(0., 255.) * ((1 << n) - 1) as f32 / 255.;
        let guess = match self.pbits {
            PBits::None => scaled.round() as i32,
            _ => ((scaled - p as f32) / 2.).round() as i32,
        };
        let mut best = (f32::INFINITY, 0);
        for code in (guess - 1).max(0)..=(guess + 1).min(self.max()) {
            let d = (self.value(code, p) as f32 - v).abs();
            if d < best.0 {
                best = (d, code);
            }
        }
        best.1
    }

    /// The p-bit choices for a subset's two endpoints.
    fn choices(self) -> &'static [[i32; 2]] {
        match self.pbits {
            PBits::None => &[[0, 0]],
            PBits::Shared => &[[0, 0], [1, 1]],
            PBits::Each => &[[0, 0], [0, 1], [1, 0], [1, 1]],
        }
    }
}

/// A subset's two quantized endpoints: codes per channel and p-bits.
#[derive(Clone, Copy, Default, Debug)]
struct Ends {
    code: [[i32; 4]; 2],
    p: [i32; 2],
}

/// The points of a subset, `n` channels of each used.
struct Points<'a> {
    at: &'a [[i32; 4]],
    n: usize,
}

/// A subset fitted: its endpoints, each point's index and the squared error.
#[derive(Clone, Copy)]
struct Fit {
    ends: Ends,
    index: [u8; 16],
    err: i64,
}

/// The palette of `ends`.
fn palette(q: Quant, ends: &Ends, n: usize, w: &[i32]) -> [[i32; 4]; 16] {
    let end = |j: usize| -> [i32; 4] {
        std::array::from_fn(|c| {
            if c < n {
                q.value(ends.code[j][c], ends.p[j])
            } else {
                0
            }
        })
    };
    let (a, b) = (end(0), end(1));
    let mut out = [[0; 4]; 16];
    for (entry, &w) in out.iter_mut().zip(w) {
        *entry = std::array::from_fn(|c| ((64 - w) * a[c] + w * b[c] + 32) >> 6);
    }
    out
}

/// Each point's nearest palette entry and the summed squared error, or `None` once the error reaches `bound`.
fn assign(points: &Points, pal: &[[i32; 4]; 16], entries: usize, bound: i64) -> Option<(i64, [u8; 16])> {
    let mut index = [0u8; 16];
    let mut err = 0i64;
    for (i, t) in points.at.iter().enumerate() {
        let mut best = (i32::MAX, 0);
        for (k, e) in pal[..entries].iter().enumerate() {
            let d: i32 = (0..points.n).map(|c| (t[c] - e[c]) * (t[c] - e[c])).sum();
            if d < best.0 {
                best = (d, k);
            }
        }
        err += i64::from(best.0);
        if err >= bound {
            return None;
        }
        index[i] = best.1 as u8;
    }
    Some((err, index))
}

/// `ends` evaluated (indices and error), or `None` when the error is `bound` or more.
fn evaluate(points: &Points, q: Quant, w: &[i32], ends: Ends, bound: i64) -> Option<Fit> {
    let (err, index) = assign(points, &palette(q, &ends, points.n, w), w.len(), bound)?;
    Some(Fit { ends, index, err })
}

/// The best quantization of the float endpoints `e` over the p-bit choices.
fn quantize(points: &Points, q: Quant, w: &[i32], e: [[f32; 4]; 2]) -> Fit {
    let mut best: Option<Fit> = None;
    for p in q.choices() {
        let mut ends = Ends {
            p: *p,
            ..Ends::default()
        };
        for (j, end) in e.iter().enumerate() {
            for (code, &v) in ends.code[j].iter_mut().zip(end).take(points.n) {
                *code = q.nearest(v, p[j]);
            }
        }
        if let Some(fit) = evaluate(points, q, w, ends, best.map_or(i64::MAX, |b| b.err)) {
            best = Some(fit);
        }
    }
    best.expect("a p-bit choice")
}

/// The mean of the points and their principal axis (zero when they are all one point).
fn principal_axis(points: &Points) -> ([f32; 4], [f32; 4]) {
    let n = points.n;
    let count = points.at.len() as f32;
    let mut mean = [0f32; 4];
    for t in points.at {
        for c in 0..n {
            mean[c] += t[c] as f32;
        }
    }
    mean = mean.map(|m| m / count);
    let mut cov = [[0f32; 4]; 4];
    for t in points.at {
        let d: [f32; 4] = std::array::from_fn(|c| if c < n { t[c] as f32 - mean[c] } else { 0. });
        for a in 0..n {
            for b in 0..n {
                cov[a][b] += d[a] * d[b];
            }
        }
    }
    // power iteration from the column of the largest variance
    let start = (0..n)
        .max_by(|&a, &b| cov[a][a].total_cmp(&cov[b][b]))
        .unwrap_or(0);
    let mut axis = cov[start];
    for _ in 0..8 {
        let next: [f32; 4] = std::array::from_fn(|a| (0..n).map(|b| cov[a][b] * axis[b]).sum());
        let len = next.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len <= f32::EPSILON {
            return (mean, [0.; 4]);
        }
        axis = next.map(|v| v / len);
    }
    (mean, axis)
}

/// Least-squares endpoints for the points' indices, or `None` when every point has one weight.
fn least_squares(points: &Points, w: &[i32], index: &[u8; 16]) -> Option<[[f32; 4]; 2]> {
    let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
    let (mut xa, mut xb) = ([0f32; 4], [0f32; 4]);
    for (i, t) in points.at.iter().enumerate() {
        let s = w[index[i] as usize] as f32 / 64.;
        let r = 1. - s;
        aa += r * r;
        ab += r * s;
        bb += s * s;
        for c in 0..points.n {
            xa[c] += r * t[c] as f32;
            xb[c] += s * t[c] as f32;
        }
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-6 {
        return None;
    }
    Some([
        std::array::from_fn(|c| ((bb * xa[c] - ab * xb[c]) / det).clamp(0., 255.)),
        std::array::from_fn(|c| ((aa * xb[c] - ab * xa[c]) / det).clamp(0., 255.)),
    ])
}

/// The subset's endpoints, fitted as the module documentation says; the quick fit (`refine` false) is the first
/// quantization of the axis's extremes.
fn fit(points: &Points, q: Quant, w: &[i32], refine: bool) -> Fit {
    let (mean, axis) = principal_axis(points);
    let (mut lo, mut hi) = (0f32, 0f32);
    for t in points.at {
        let along: f32 = (0..points.n).map(|c| (t[c] as f32 - mean[c]) * axis[c]).sum();
        lo = lo.min(along);
        hi = hi.max(along);
    }
    let end = |s: f32| -> [f32; 4] { std::array::from_fn(|c| (mean[c] + axis[c] * s).clamp(0., 255.)) };
    let mut best = quantize(points, q, w, [end(lo), end(hi)]);
    if !refine {
        return best;
    }
    // refit by least squares while the error falls
    for _ in 0..4 {
        if best.err == 0 {
            return best;
        }
        let Some(e) = least_squares(points, w, &best.index) else {
            break;
        };
        let fit = quantize(points, q, w, e);
        if fit.err >= best.err {
            break;
        }
        best = fit;
    }
    perturb(points, q, w, best)
}

/// Moves each endpoint channel a step either way, and flips each p-bit, while the error falls.
fn perturb(points: &Points, q: Quant, w: &[i32], mut best: Fit) -> Fit {
    loop {
        if best.err == 0 {
            return best;
        }
        let mut improved = false;
        for j in 0..2 {
            for c in 0..points.n {
                for step in [-1, 1] {
                    let code = best.ends.code[j][c] + step;
                    if !(0..=q.max()).contains(&code) {
                        continue;
                    }
                    let mut ends = best.ends;
                    ends.code[j][c] = code;
                    if let Some(fit) = evaluate(points, q, w, ends, best.err) {
                        best = fit;
                        improved = true;
                    }
                }
            }
        }
        let flips: &[[i32; 2]] = match q.pbits {
            PBits::None => &[],
            PBits::Shared => &[[1, 1]],
            PBits::Each => &[[1, 0], [0, 1]],
        };
        for flip in flips {
            let mut ends = best.ends;
            ends.p = [ends.p[0] ^ flip[0], ends.p[1] ^ flip[1]];
            // each code moved to its nearest for the new p-bit
            for (j, &flipped) in flip.iter().enumerate() {
                if flipped == 1 {
                    for c in 0..points.n {
                        let level = q.value(best.ends.code[j][c], best.ends.p[j]) as f32;
                        ends.code[j][c] = q.nearest(level, ends.p[j]);
                    }
                }
            }
            if let Some(fit) = evaluate(points, q, w, ends, best.err) {
                best = fit;
                improved = true;
            }
        }
        if !improved {
            return best;
        }
    }
}

/// A block chosen: its mode, partition (or rotation and index selection), each subset's endpoints and each texel's
/// indices (colour, and alpha for modes 4 and 5).
#[derive(Clone, Copy)]
struct Block {
    mode: usize,
    partition: usize,
    rotation: usize,
    selection: usize,
    ends: [Ends; 3],
    alpha: Ends,
    index: [u8; 16],
    index2: [u8; 16],
    err: i64,
}

/// `mode`'s block with `partition`, each subset's texels fitted on `n` channels (quickly unless `refine`); `extra` is
/// the error of the channels the mode does not store.
fn fit_partition(
    texels: &[[i32; 4]; 16],
    mode: usize,
    partition: usize,
    n: usize,
    extra: i64,
    refine: bool,
) -> Block {
    let m = &MODES[mode];
    let q = Quant {
        bits: m.colour,
        pbits: m.pbits,
    };
    let w = weights(m.index);
    let mut block = Block {
        mode,
        partition,
        rotation: 0,
        selection: 0,
        ends: [Ends::default(); 3],
        alpha: Ends::default(),
        index: [0; 16],
        index2: [0; 16],
        err: extra,
    };
    for s in 0..m.subsets {
        let (at, places, count) = gather(texels, m.subsets, partition, s);
        let fit = fit(&Points { at: &at[..count], n }, q, w, refine);
        block.ends[s] = fit.ends;
        for (&place, &index) in places[..count].iter().zip(&fit.index) {
            block.index[place] = index;
        }
        block.err += fit.err;
    }
    block
}

/// Mode 4 or 5 with `rotation` (the channel swapped with alpha: none, R, G, B) and `selection` (mode 4: whether colour
/// takes the 3-bit indices).
fn fit_separate(texels: &[[i32; 4]; 16], mode: usize, rotation: usize, selection: usize) -> Block {
    let m = &MODES[mode];
    let rotated: [[i32; 4]; 16] = texels.map(|mut t| {
        if rotation > 0 {
            t.swap(rotation - 1, 3);
        }
        t
    });
    let (colour_bits, alpha_bits) = if selection == 1 {
        (m.index2, m.index)
    } else {
        (m.index, m.index2)
    };
    let colour = fit(
        &Points { at: &rotated, n: 3 },
        Quant {
            bits: m.colour,
            pbits: PBits::None,
        },
        weights(colour_bits),
        true,
    );
    let alone: [[i32; 4]; 16] = rotated.map(|t| [t[3], 0, 0, 0]);
    let alpha = fit(
        &Points { at: &alone, n: 1 },
        Quant {
            bits: m.alpha,
            pbits: PBits::None,
        },
        weights(alpha_bits),
        true,
    );
    let mut alpha_ends = Ends::default();
    for j in 0..2 {
        alpha_ends.code[j][3] = alpha.ends.code[j][0];
    }
    Block {
        mode,
        partition: 0,
        rotation,
        selection,
        ends: [colour.ends, Ends::default(), Ends::default()],
        alpha: alpha_ends,
        index: colour.index,
        index2: alpha.index,
        err: colour.err + alpha.err,
    }
}

/// For mode 5's colour channels: for each index weight (of the four) and each level, the endpoint codes whose
/// interpolation is nearest the level, and how far it is.
fn one_colour() -> &'static [[(u8, u8, u8); 256]; 4] {
    static TABLE: std::sync::OnceLock<[[(u8, u8, u8); 256]; 4]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let q = Quant {
            bits: 7,
            pbits: PBits::None,
        };
        let mut table = [[(0, 0, u8::MAX); 256]; 4];
        for (k, &w) in WEIGHTS2.iter().enumerate() {
            for a in 0..128 {
                for b in 0..128 {
                    let v = ((64 - w) * q.value(a, 0) + w * q.value(b, 0) + 32) >> 6;
                    let e = &mut table[k][v as usize];
                    if e.2 > 0 {
                        *e = (a as u8, b as u8, 0);
                    }
                }
            }
            // levels no pair reaches take the nearest that one does
            for v in 0..256usize {
                if table[k][v].2 != 0 {
                    let near = (0..256usize)
                        .filter(|&u| table[k][u].2 == 0)
                        .min_by_key(|&u| u.abs_diff(v))
                        .expect("a level reached");
                    table[k][v] = (
                        table[k][near].0,
                        table[k][near].1,
                        u8::try_from(near.abs_diff(v)).unwrap_or(u8::MAX),
                    );
                }
            }
        }
        table
    })
}

/// A block of one colour as mode 5: the colour by the index weight that comes nearest, the alpha exact.
fn fit_one_colour(c: [i32; 4]) -> Block {
    let table = one_colour();
    let k = (0..4)
        .min_by_key(|&k| {
            (0..3)
                .map(|ch| i64::from(table[k][c[ch] as usize].2).pow(2))
                .sum::<i64>()
        })
        .unwrap_or(0);
    let mut ends = Ends::default();
    let mut err = 0;
    for ch in 0..3 {
        let (a, b, d) = table[k][c[ch] as usize];
        ends.code[0][ch] = i32::from(a);
        ends.code[1][ch] = i32::from(b);
        err += 16 * i64::from(d).pow(2);
    }
    let alpha = Ends {
        code: [[0, 0, 0, c[3]], [0, 0, 0, c[3]]],
        ..Ends::default()
    };
    Block {
        mode: 5,
        partition: 0,
        rotation: 0,
        selection: 0,
        ends: [ends, Ends::default(), Ends::default()],
        alpha,
        index: [k as u8; 16],
        index2: [0; 16],
        err,
    }
}

/// The best block for `texels` (RGBA levels).
pub(super) fn encode(texels: &[[u8; 4]; 16]) -> [u8; 16] {
    let t: [[i32; 4]; 16] = texels.map(|p| p.map(i32::from));
    let mut best: Option<Block> = None;
    let keep = |b: Block, best: &mut Option<Block>| {
        if best.is_none_or(|x| b.err < x.err) {
            *best = Some(b);
        }
    };
    if t.iter().all(|p| *p == t[0]) {
        keep(fit_one_colour(t[0]), &mut best);
    }
    let done = |best: &Option<Block>| best.is_some_and(|b| b.err == 0);
    // one subset, colour and alpha together
    if !done(&best) {
        keep(fit_partition(&t, 6, 0, 4, 0, true), &mut best);
    }
    for rotation in 0..4 {
        for (mode, selection) in [(5, 0), (4, 0), (4, 1)] {
            if !done(&best) {
                keep(fit_separate(&t, mode, rotation, selection), &mut best);
            }
        }
    }
    // modes without alpha decode it as 255: its error is the same for any of their blocks
    let opaque: i64 = t.iter().map(|p| i64::from(255 - p[3]).pow(2)).sum();
    // every partition fitted quickly, the best few refined
    for (mode, n, extra) in [
        (7, 4, 0),
        (1, 3, opaque),
        (3, 3, opaque),
        (0, 3, opaque),
        (2, 3, opaque),
    ] {
        if done(&best) || best.is_some_and(|b| extra >= b.err) {
            continue;
        }
        let mut quick: Vec<(i64, usize)> = (0..1usize << MODES[mode].partition_bits)
            .map(|p| (fit_partition(&t, mode, p, n, extra, false).err, p))
            .collect();
        quick.sort_unstable();
        for &(_, p) in quick.iter().take(PARTITIONS_REFINED) {
            keep(fit_partition(&t, mode, p, n, extra, true), &mut best);
        }
    }
    pack(&best.expect("a block"))
}

/// Bits written from the lowest of the block up.
struct Writer {
    bits: u128,
    at: u32,
}

impl Writer {
    fn put(&mut self, v: i32, n: u32) {
        debug_assert!(n == 0 || (v as u32) < 1 << n, "{v} in {n} bits");
        self.bits |= u128::from(v as u32) << self.at;
        self.at += n;
    }
}

/// Swaps a subset's endpoints, and reverses its indices, where its anchor's index would need its top bit.
fn fix_anchor(
    ends: &mut Ends,
    index: &mut [u8; 16],
    bits: u32,
    anchor: usize,
    members: impl Fn(usize) -> bool,
) {
    let top = 1u8 << (bits - 1);
    if index[anchor] & top != 0 {
        ends.code.swap(0, 1);
        ends.p.swap(0, 1);
        let max = (1u8 << bits) - 1;
        for (i, v) in index.iter_mut().enumerate() {
            if members(i) {
                *v = max - *v;
            }
        }
    }
}

/// The block's bits.
fn pack(block: &Block) -> [u8; 16] {
    let m = &MODES[block.mode];
    let mut b = *block;
    let subsets = m.subsets;
    let mut w = Writer { bits: 0, at: 0 };
    w.put(1 << block.mode, block.mode as u32 + 1);
    w.put(b.partition as i32, m.partition_bits);
    if block.mode == 4 || block.mode == 5 {
        w.put(b.rotation as i32, 2);
    }
    if block.mode == 4 {
        w.put(b.selection as i32, 1);
    }
    let (colour_bits, alpha_bits) = if b.selection == 1 {
        (m.index2, m.index)
    } else {
        (m.index, m.index2)
    };
    let partition = b.partition;
    for s in 0..subsets {
        let at = anchor_of(subsets, partition, s);
        fix_anchor(&mut b.ends[s], &mut b.index, colour_bits, at, |i| {
            subset_of(subsets, partition, i) == s
        });
    }
    if m.index2 > 0 {
        fix_anchor(&mut b.alpha, &mut b.index2, alpha_bits, 0, |_| true);
    }
    for c in 0..3 {
        for s in 0..subsets {
            for j in 0..2 {
                w.put(b.ends[s].code[j][c], m.colour);
            }
        }
    }
    if m.alpha > 0 {
        for s in 0..subsets {
            for j in 0..2 {
                let code = if m.index2 > 0 {
                    b.alpha.code[j][3]
                } else {
                    b.ends[s].code[j][3]
                };
                w.put(code, m.alpha);
            }
        }
    }
    match m.pbits {
        PBits::None => {}
        PBits::Shared => (0..subsets).for_each(|s| w.put(b.ends[s].p[0], 1)),
        PBits::Each => (0..subsets).for_each(|s| (0..2).for_each(|j| w.put(b.ends[s].p[j], 1))),
    }
    // mode 4 writes its 2-bit indices first, whichever channels take them
    let sets: [(&[u8; 16], u32); 2] = if b.selection == 1 {
        [(&b.index2, alpha_bits), (&b.index, colour_bits)]
    } else {
        [(&b.index, colour_bits), (&b.index2, alpha_bits)]
    };
    for (n, (index, bits)) in sets.into_iter().enumerate() {
        if n == 1 && m.index2 == 0 {
            break;
        }
        for (i, &v) in index.iter().enumerate() {
            let anchored = if n == 0 && m.index2 == 0 {
                anchor(subsets, b.partition, i)
            } else {
                i == 0
            };
            w.put(i32::from(v), bits - u32::from(anchored));
        }
    }
    debug_assert_eq!(w.at, 128, "mode {}", block.mode);
    w.bits.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks of several kinds: noise, a ramp with alpha, an opaque ramp, two flat halves.
    fn blocks() -> Vec<[[i32; 4]; 16]> {
        let mut s = 0x9e37_79b9_u64;
        let mut next = move || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 56) as i32
        };
        let mut out = Vec::new();
        for _ in 0..12 {
            out.push(std::array::from_fn(|_| [next(), next(), next(), next()]));
            let (a, b) = ([next(), next(), next(), next()], [next(), next(), next(), next()]);
            out.push(std::array::from_fn(|i| {
                std::array::from_fn(|c| a[c] + (b[c] - a[c]) * i as i32 / 15)
            }));
            out.push(std::array::from_fn(|i| {
                let mut t: [i32; 4] = std::array::from_fn(|c| a[c] + (b[c] - a[c]) * (i as i32 % 4) / 3);
                t[3] = 255;
                t
            }));
            out.push(std::array::from_fn(|i| if i % 4 < 2 { a } else { b }));
        }
        out
    }

    /// The squared error of `bytes` decoded (by bcdec) against `t`.
    fn decoded_error(bytes: &[u8; 16], t: &[[i32; 4]; 16]) -> i64 {
        let mut out = [0u8; 64];
        bcdec_rs::bc7(bytes, &mut out, 16);
        (0..64)
            .map(|i| i64::from(i32::from(out[i]) - t[i / 4][i % 4]).pow(2))
            .sum()
    }

    /// Every kind of block the search makes decodes (in bcdec) to the error it was chosen by, in the mode it was made in.
    #[test]
    fn every_block_decodes_to_the_error_it_was_chosen_by() {
        for t in blocks() {
            let opaque: i64 = t.iter().map(|p| i64::from(255 - p[3]).pow(2)).sum();
            let flat = [t[0]; 16];
            let one = fit_one_colour(t[0]);
            assert_eq!(
                decoded_error(&pack(&one), &flat),
                one.err,
                "one colour {:?}",
                t[0]
            );
            let mut made = vec![fit_partition(&t, 6, 0, 4, 0, true)];
            for rotation in 0..4 {
                made.extend([
                    fit_separate(&t, 5, rotation, 0),
                    fit_separate(&t, 4, rotation, 0),
                    fit_separate(&t, 4, rotation, 1),
                ]);
            }
            for (mode, n, extra) in [
                (7, 4, 0),
                (1, 3, opaque),
                (3, 3, opaque),
                (0, 3, opaque),
                (2, 3, opaque),
            ] {
                let count = 1 << MODES[mode].partition_bits;
                for p in [0, 5, 13, 15, 37, 63].into_iter().filter(|&p| p < count) {
                    made.push(fit_partition(&t, mode, p, n, extra, p % 2 == 0));
                }
            }
            for block in made {
                let packed = pack(&block);
                assert_eq!(packed[0].trailing_zeros() as usize, block.mode);
                assert_eq!(
                    decoded_error(&packed, &t),
                    block.err,
                    "mode {} partition {} rotation {} selection {}",
                    block.mode,
                    block.partition,
                    block.rotation,
                    block.selection
                );
            }
        }
    }

    /// Mode 5's one-colour block is exact for every colour: one index weight reaches each level of a channel.
    #[test]
    fn every_colour_is_one_exact_block() {
        let table = one_colour();
        let exact: Vec<usize> = (0..4).filter(|&k| table[k].iter().all(|e| e.2 == 0)).collect();
        assert!(!exact.is_empty(), "no index weight reaches every level");
        for c in [
            [0, 0, 0, 0],
            [101, 33, 7, 255],
            [254, 1, 128, 3],
            [127, 128, 129, 200],
        ] {
            assert_eq!(fit_one_colour(c).err, 0, "{c:?}");
        }
    }
}
