//! The colour block of BC1, BC2 and BC3: two RGB 5:6:5 endpoints and a 2-bit index per texel.
//!
//! The fit is a cluster fit: the texels are ordered along their principal axis, and every split of that order into the
//! palette's runs (four, or three for BC1's three-colour mode) gets the least-squares endpoints for it, snapped to the
//! 5:6:5 grid and scored there; the best split's axis is refitted until the order no longer changes. The endpoints are
//! then nudged one grid step at a time while that lowers the error, and every texel takes the palette entry nearest to it
//! as [`crate::Decoder`] decodes the block. A block of one colour takes the endpoints whose interpolated entry is nearest
//! to it on each channel. The error is the sum of squared RGB differences, every channel weighted alike.
use crate::blocks::palette;

/// The texels of a block in 0..=255 units, and the ones that count: BC1 leaves out the texels it makes transparent.
pub(super) struct Texels {
    pub rgb: [[f32; 3]; 16],
    /// Bit `i` set when texel `i` is transparent (BC1 index 3): left out of the fit.
    pub transparent: u16,
}

/// Which palettes a block may use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Palettes {
    /// BC2 and BC3: the four-colour palette always, whatever the endpoints' order.
    Four,
    /// BC1: four colours when the first endpoint is the greater, else three and transparent black.
    Bc1,
}

/// A 5:6:5 endpoint: its levels, and those levels as decoders expand them to 8 bits.
#[derive(Clone, Copy, PartialEq)]
struct Endpoint {
    levels: [u8; 3],
}

const BITS: [u32; 3] = [5, 6, 5];

impl Endpoint {
    /// The grid point nearest to `rgb` (0..=255 units, clamped).
    fn snap(rgb: [f32; 3]) -> Endpoint {
        Endpoint {
            levels: std::array::from_fn(|c| {
                let max = ((1u32 << BITS[c]) - 1) as f32;
                (rgb[c].clamp(0., 255.) * max / 255.).round() as u8
            }),
        }
    }
    /// The 8-bit value decoders give each level (bit replication).
    fn expanded(self) -> [f32; 3] {
        std::array::from_fn(|c| {
            let v = u32::from(self.levels[c]);
            (if BITS[c] == 5 {
                (v << 3) | (v >> 2)
            } else {
                (v << 2) | (v >> 4)
            }) as f32
        })
    }
    fn packed(self) -> u16 {
        (u16::from(self.levels[0]) << 11) | (u16::from(self.levels[1]) << 5) | u16::from(self.levels[2])
    }
}

/// A finished block and its error against the texels that count.
struct Candidate {
    bytes: [u8; 8],
    error: f32,
}

/// The block for endpoints `a` and `b` in a `three`-colour or four-colour palette: endpoints put in the order that
/// selects that palette, and each texel given its nearest entry as the block decodes.
fn finish(texels: &Texels, palettes: Palettes, a: Endpoint, b: Endpoint, three: bool) -> Candidate {
    let (mut c0, mut c1) = (a.packed(), b.packed());
    // four colours need c0 > c1 in BC1 (BC2/BC3 ignore the order); three need c0 <= c1
    if (three && c0 > c1) || (!three && c0 < c1) {
        std::mem::swap(&mut c0, &mut c1);
    }
    let mut bytes = [0u8; 8];
    bytes[..2].copy_from_slice(&c0.to_le_bytes());
    bytes[2..4].copy_from_slice(&c1.to_le_bytes());
    let entries = palette(&bytes, palettes == Palettes::Four);
    // a BC1 block whose endpoints are equal is a three-colour block: its index 3 is transparent black
    let usable = if palettes == Palettes::Bc1 && c0 <= c1 {
        3
    } else {
        4
    };
    let mut bits = 0u32;
    let mut error = 0.;
    for (i, texel) in texels.rgb.iter().enumerate() {
        let index = if texels.transparent >> i & 1 == 1 {
            3
        } else {
            let mut best = (f32::MAX, 0);
            for (j, entry) in entries.iter().take(usable).enumerate() {
                let e: f32 = (0..3).map(|c| (texel[c] - f32::from(entry[c])).powi(2)).sum();
                if e < best.0 {
                    best = (e, j);
                }
            }
            error += best.0;
            best.1 as u32
        };
        bits |= index << (2 * i);
    }
    bytes[4..].copy_from_slice(&bits.to_le_bytes());
    Candidate { bytes, error }
}

/// The interpolation weights of a palette's runs, from the first endpoint's run to the second's.
const FOUR: [f32; 4] = [1., 2. / 3., 1. / 3., 0.];
const THREE: [f32; 3] = [1., 0.5, 0.];

/// The principal axis of `points` (power iteration on their covariance), or `None` when they are all one colour.
fn axis(points: &[[f32; 3]]) -> Option<[f32; 3]> {
    let n = points.len() as f32;
    let mean: [f32; 3] = std::array::from_fn(|c| points.iter().map(|p| p[c]).sum::<f32>() / n);
    let mut cov = [[0f32; 3]; 3];
    for p in points {
        let d: [f32; 3] = std::array::from_fn(|c| p[c] - mean[c]);
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v += d[i] * d[j];
            }
        }
    }
    let trace = cov[0][0] + cov[1][1] + cov[2][2];
    if trace <= 1e-6 {
        return None;
    }
    // start from the channel that varies most, so the iteration cannot start orthogonal to the answer
    let start = (0..3)
        .max_by(|&i, &j| cov[i][i].total_cmp(&cov[j][j]))
        .unwrap_or(0);
    let mut v = [0f32; 3];
    v[start] = 1.;
    for _ in 0..8 {
        let w: [f32; 3] = std::array::from_fn(|i| (0..3).map(|j| cov[i][j] * v[j]).sum());
        let len = w.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len <= 1e-12 {
            break;
        }
        v = w.map(|x| x / len);
    }
    Some(v)
}

/// A split's endpoints: least-squares, and snapped to the grid.
struct Split {
    error: f32,
    snapped: (Endpoint, Endpoint),
    exact: ([f32; 3], [f32; 3]),
}

/// The cluster fit of `points` along `dir` with `weights` (a palette's run weights): the split whose snapped endpoints
/// give the least error.
fn cluster_fit(points: &[[f32; 3]], dir: [f32; 3], weights: &[f32]) -> Option<Split> {
    let mut order: Vec<usize> = (0..points.len()).collect();
    let dot = |p: &[f32; 3]| p[0] * dir[0] + p[1] * dir[1] + p[2] * dir[2];
    order.sort_by(|&i, &j| dot(&points[i]).total_cmp(&dot(&points[j])).then(i.cmp(&j)));
    // sorted from the second endpoint's end: the first endpoint (weight 1) takes the far end of the axis
    order.reverse();
    let sorted: Vec<[f32; 3]> = order.iter().map(|&i| points[i]).collect();
    let n = sorted.len();
    let mut prefix = vec![[0f32; 3]; n + 1];
    for (i, p) in sorted.iter().enumerate() {
        prefix[i + 1] = std::array::from_fn(|c| prefix[i][c] + p[c]);
    }
    let sum =
        |from: usize, to: usize| -> [f32; 3] { std::array::from_fn(|c| prefix[to][c] - prefix[from][c]) };
    let runs = weights.len();
    let mut best: Option<Split> = None;
    // the boundaries between runs: run k holds sorted[cuts[k]..cuts[k + 1]]
    let mut cuts = vec![0usize; runs + 1];
    cuts[runs] = n;
    loop {
        let (mut aa, mut bb, mut ab) = (0f32, 0f32, 0f32);
        let (mut ax, mut bx) = ([0f32; 3], [0f32; 3]);
        for k in 0..runs {
            let count = (cuts[k + 1] - cuts[k]) as f32;
            if count == 0. {
                continue;
            }
            let (alpha, beta) = (weights[k], 1. - weights[k]);
            aa += alpha * alpha * count;
            bb += beta * beta * count;
            ab += alpha * beta * count;
            let s = sum(cuts[k], cuts[k + 1]);
            for c in 0..3 {
                ax[c] += alpha * s[c];
                bx[c] += beta * s[c];
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() > 1e-6 {
            let a: [f32; 3] = std::array::from_fn(|c| (ax[c] * bb - bx[c] * ab) / det);
            let b: [f32; 3] = std::array::from_fn(|c| (bx[c] * aa - ax[c] * ab) / det);
            let (qa, qb) = (Endpoint::snap(a), Endpoint::snap(b));
            let (ea, eb) = (qa.expanded(), qb.expanded());
            // the squared error of the split with the snapped endpoints, but for the texels' own constant sum of squares
            let error: f32 = (0..3)
                .map(|c| {
                    ea[c] * ea[c] * aa + eb[c] * eb[c] * bb + 2. * ea[c] * eb[c] * ab
                        - 2. * ea[c] * ax[c]
                        - 2. * eb[c] * bx[c]
                })
                .sum();
            if best.as_ref().is_none_or(|b| error < b.error) {
                best = Some(Split {
                    error,
                    snapped: (qa, qb),
                    exact: (a, b),
                });
            }
        }
        // the next split, in lexicographic order of the inner boundaries (each at most the next)
        let mut k = runs - 1;
        loop {
            if k == 0 {
                return best;
            }
            if cuts[k] < cuts[k + 1] {
                cuts[k] += 1;
                for later in k + 1..runs {
                    cuts[later] = cuts[k];
                }
                break;
            }
            k -= 1;
        }
    }
}

/// The best block in one palette size: cluster fits (refitted along the best split's axis), then endpoint nudging.
fn fit(texels: &Texels, palettes: Palettes, points: &[[f32; 3]], three: bool) -> Option<Candidate> {
    let weights: &[f32] = if three { &THREE } else { &FOUR };
    let mut dir = axis(points)?;
    let mut best: Option<Candidate> = None;
    let mut ends: Option<(Endpoint, Endpoint)> = None;
    for _ in 0..3 {
        let Some(Split {
            snapped: (qa, qb),
            exact: (a, b),
            ..
        }) = cluster_fit(points, dir, weights)
        else {
            break;
        };
        if ends == Some((qa, qb)) {
            break;
        }
        ends = Some((qa, qb));
        let candidate = finish(texels, palettes, qa, qb, three);
        if best.as_ref().is_none_or(|b| candidate.error < b.error) {
            best = Some(candidate);
        }
        let d: [f32; 3] = std::array::from_fn(|c| a[c] - b[c]);
        let len = d.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len <= 1e-6 {
            break;
        }
        dir = d.map(|x| x / len);
    }
    let (mut qa, mut qb) = ends?;
    let mut best = best?;
    // nudge each endpoint channel a grid step while that lowers the block's error
    for _ in 0..4 {
        let mut improved = false;
        for which in 0..2 {
            for (c, bits) in BITS.iter().enumerate() {
                for step in [-1i32, 1] {
                    let (mut ta, mut tb) = (qa, qb);
                    let end = if which == 0 { &mut ta } else { &mut tb };
                    let level = i32::from(end.levels[c]) + step;
                    if level < 0 || level >= 1 << bits {
                        continue;
                    }
                    end.levels[c] = level as u8;
                    let candidate = finish(texels, palettes, ta, tb, three);
                    if candidate.error < best.error {
                        best = candidate;
                        (qa, qb) = (ta, tb);
                        improved = true;
                    }
                }
            }
        }
        if !improved || best.error == 0. {
            break;
        }
    }
    Some(best)
}

/// For each 8-bit value, the 5- and 6-bit endpoint pair whose 2/3 : 1/3 entry (index 2 of a four-colour palette, as
/// [`crate::Decoder`] interpolates it) is nearest, and for three colours the pair whose midpoint is.
struct SingleColour {
    four: [[[u8; 2]; 256]; 2],
    three: [[[u8; 2]; 256]; 2],
}

fn single_colour() -> &'static SingleColour {
    static TABLES: std::sync::OnceLock<SingleColour> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = SingleColour {
            four: [[[0; 2]; 256]; 2],
            three: [[[0; 2]; 256]; 2],
        };
        for (k, bits) in [5u32, 6].into_iter().enumerate() {
            let max = (1u32 << bits) - 1;
            // the decoder's entries for levels a and b, on a channel of `bits` bits (the arithmetic of blocks::palette)
            let four = |a: u32, b: u32| {
                let v = 2 * a + b;
                if bits == 5 {
                    (v * 351 + 61) >> 7
                } else {
                    (v * 2763 + 1039) >> 11
                }
            };
            let three = |a: u32, b: u32| {
                if bits == 5 {
                    ((a + b) * 1053 + 125) >> 8
                } else {
                    ((a + b) * 4145 + 1019) >> 11
                }
            };
            for v in 0..256u32 {
                let mut best4 = (u32::MAX, [0u8; 2]);
                let mut best3 = (u32::MAX, [0u8; 2]);
                for a in 0..=max {
                    for b in 0..=max {
                        let e4 = four(a, b).abs_diff(v);
                        // ties go to the closer pair, so the endpoints stay near the colour
                        let e4 = e4 * 1024 + a.abs_diff(b);
                        if e4 < best4.0 {
                            best4 = (e4, [a as u8, b as u8]);
                        }
                        let e3 = three(a, b).abs_diff(v) * 1024 + a.abs_diff(b);
                        if e3 < best3.0 {
                            best3 = (e3, [a as u8, b as u8]);
                        }
                    }
                }
                t.four[k][v as usize] = best4.1;
                t.three[k][v as usize] = best3.1;
            }
        }
        t
    })
}

/// The block of one colour: every channel's best pair for the shared index (2 of four colours, or 2 of three).
fn single(texels: &Texels, palettes: Palettes, colour: [f32; 3], three: bool) -> Candidate {
    let tables = single_colour();
    let table = if three { &tables.three } else { &tables.four };
    let pairs: [[u8; 2]; 3] =
        std::array::from_fn(|c| table[usize::from(c == 1)][colour[c].clamp(0., 255.).round() as usize]);
    let a = Endpoint {
        levels: std::array::from_fn(|c| pairs[c][0]),
    };
    let b = Endpoint {
        levels: std::array::from_fn(|c| pairs[c][1]),
    };
    finish(texels, palettes, a, b, three)
}

/// Encodes the colour block of `texels` in `palettes`.
pub(super) fn encode(texels: &Texels, palettes: Palettes) -> [u8; 8] {
    let points: Vec<[f32; 3]> = (0..16)
        .filter(|i| texels.transparent >> i & 1 == 0)
        .map(|i| texels.rgb[i])
        .collect();
    if points.is_empty() {
        // every texel transparent: a three-colour block with index 3 everywhere
        let mut bytes = [0u8; 8];
        bytes[2..4].copy_from_slice(&0xffffu16.to_le_bytes());
        bytes[4..].copy_from_slice(&u32::MAX.to_le_bytes());
        return bytes;
    }
    // the palette sizes the block may use: BC1 needs three colours to keep a texel transparent
    let sizes: &[bool] = match palettes {
        Palettes::Four => &[false],
        Palettes::Bc1 if texels.transparent != 0 => &[true],
        Palettes::Bc1 => &[false, true],
    };
    let mut best: Option<Candidate> = None;
    for &three in sizes {
        let candidate = match fit(texels, palettes, &points, three) {
            Some(c) => c,
            // one colour (no axis): the table pair for it
            None => single(texels, palettes, points[0], three),
        };
        if best.as_ref().is_none_or(|b| candidate.error < b.error) {
            best = Some(candidate);
        }
    }
    best.map_or([0; 8], |b| b.bytes)
}
