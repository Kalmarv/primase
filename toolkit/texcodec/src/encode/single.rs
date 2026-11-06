//! A one-channel block: BC4, each channel of BC5, and BC3's alpha. Two 8-bit endpoints and a 3-bit index per texel; the
//! endpoints' order picks the palette: the first greater gives eight entries between them, else six and the range's two
//! ends (0 and 255, or -127 and 127 for SNORM: -1 and 1).
//!
//! Both palettes are fitted and the better kept. Each starts from the texels' range (for six entries, the range of the
//! texels the two ends do not serve better), then alternates giving each texel its nearest entry with the least-squares
//! endpoints for those choices, and last moves each endpoint a step at a time while that lowers the error. Entries are
//! the exact interpolations `((n - i) a + i b) / n`; the error is the sum of squared differences.

/// The palette of endpoints `a`, `b`: eight entries, or six and the range's ends, by their order.
fn palette(a: i32, b: i32, signed: bool) -> [f32; 8] {
    let (a, b) = (a as f32, b as f32);
    let mut p = [a, b, 0., 0., 0., 0., 0., 0.];
    let n = if a > b { 7. } else { 5. };
    let count = if a > b { 6 } else { 4 };
    for i in 1..=count {
        p[i + 1] = ((n - i as f32) * a + i as f32 * b) / n;
    }
    if a <= b {
        (p[6], p[7]) = if signed { (-127., 127.) } else { (0., 255.) };
    }
    p
}

/// The error of `values` with endpoints `a`, `b`, and each texel's nearest entry.
fn assign(values: &[f32; 16], a: i32, b: i32, signed: bool) -> (f32, [u8; 16]) {
    let p = palette(a, b, signed);
    let mut indices = [0u8; 16];
    let mut error = 0.;
    for (i, &v) in values.iter().enumerate() {
        let mut best = (f32::MAX, 0u8);
        for (j, &e) in p.iter().enumerate() {
            let d = (v - e) * (v - e);
            if d < best.0 {
                best = (d, j as u8);
            }
        }
        error += best.0;
        indices[i] = best.1;
    }
    (error, indices)
}

/// The weight of the first endpoint in entry `index` of an `eight`-entry palette, or `None` for the range's ends.
fn weight(index: u8, eight: bool) -> Option<f32> {
    let n = if eight { 7. } else { 5. };
    match index {
        0 => Some(1.),
        1 => Some(0.),
        6 | 7 if !eight => None,
        i => Some((n - f32::from(i - 1)) / n),
    }
}

/// The best endpoints of one palette size, from `start`, with the ordering that selects it kept.
fn fit(values: &[f32; 16], signed: bool, eight: bool, start: (i32, i32)) -> (f32, i32, i32, [u8; 16]) {
    let (lo, hi) = if signed { (-127, 127) } else { (0, 255) };
    // in range, and in the order that selects the palette: eight entries need a > b, six a <= b
    let order = |a: i32, b: i32| -> (i32, i32) {
        let (a, b) = (a.clamp(lo, hi), b.clamp(lo, hi));
        let (high, low) = (a.max(b), a.min(b));
        match (eight, high == low) {
            (false, _) => (low, high),
            (true, false) => (high, low),
            (true, true) if high < hi => (high + 1, low),
            (true, true) => (high, low - 1),
        }
    };
    let (mut a, mut b) = order(start.0, start.1);
    let (mut error, mut indices) = assign(values, a, b, signed);
    // alternate: least-squares endpoints for the current choices, then the choices for those endpoints
    for _ in 0..8 {
        let (mut aa, mut bb, mut ab, mut ax, mut bx) = (0f32, 0f32, 0f32, 0f32, 0f32);
        for (i, &v) in values.iter().enumerate() {
            if let Some(w) = weight(indices[i], eight) {
                let u = 1. - w;
                aa += w * w;
                bb += u * u;
                ab += w * u;
                ax += w * v;
                bx += u * v;
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() <= 1e-6 {
            break;
        }
        let na = ((ax * bb - bx * ab) / det).round() as i32;
        let nb = ((bx * aa - ax * ab) / det).round() as i32;
        let (na, nb) = order(na, nb);
        if (na, nb) == (a, b) {
            break;
        }
        let (e, idx) = assign(values, na, nb, signed);
        if e >= error {
            break;
        }
        (a, b, error, indices) = (na, nb, e, idx);
    }
    // move each endpoint a step while that lowers the error
    for _ in 0..16 {
        let mut improved = false;
        for (da, db) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (na, nb) = (a + da, b + db);
            if na < lo || na > hi || nb < lo || nb > hi || (eight && na <= nb) || (!eight && na > nb) {
                continue;
            }
            let (e, idx) = assign(values, na, nb, signed);
            if e < error {
                (a, b, error, indices) = (na, nb, e, idx);
                improved = true;
            }
        }
        if !improved || error == 0. {
            break;
        }
    }
    (error, a, b, indices)
}

/// Encodes `values` (0..=255 units, or -127..=127 for SNORM) as a one-channel block.
pub(super) fn encode(values: &[f32; 16], signed: bool) -> [u8; 8] {
    let (lo, hi) = if signed { (-127f32, 127f32) } else { (0f32, 255f32) };
    let values = values.map(|v| v.clamp(lo, hi));
    let min = values.iter().copied().fold(f32::MAX, f32::min);
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    let eight = fit(&values, signed, true, (max.round() as i32, min.round() as i32));
    // six entries: the range of the texels nearer an interior value than either end
    let inner: Vec<f32> = values
        .iter()
        .copied()
        .filter(|&v| v - lo > 0.5 && hi - v > 0.5)
        .collect();
    let (imin, imax) = if inner.is_empty() {
        (min, max)
    } else {
        (
            inner.iter().copied().fold(f32::MAX, f32::min),
            inner.iter().copied().fold(f32::MIN, f32::max),
        )
    };
    let six = fit(&values, signed, false, (imin.round() as i32, imax.round() as i32));
    let (_, a, b, indices) = if six.0 < eight.0 { six } else { eight };
    let mut out = [0u8; 8];
    // SNORM endpoints are two's complement bytes; UNORM ones are 0..=255, the same low byte
    out[0] = a as u8;
    out[1] = b as u8;
    let bits = indices
        .iter()
        .enumerate()
        .fold(0u64, |acc, (i, &x)| acc | (u64::from(x) << (3 * i)));
    out[2..].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}
