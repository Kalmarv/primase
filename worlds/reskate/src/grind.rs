//! Grind curves as Studio builds them: the polyline simplified (Douglas-Peucker, 3 mm), runs split at corners sharper
//! than 75 degrees, an invisible 8-sided collision prism hanging under each run with one edge on the line.

use crate::scene::GrindCurve;
use crate::vec::Num;

/// Studio's smooth-grind surfaces.
pub const SURFACES: [(&str, i32); 5] = [
    ("metal_rail", 37227424),
    ("metal", 37226528),
    ("metal_thin", 37226144),
    ("concrete", 37225248),
    ("wood", 37228128),
];

const SIDES: usize = 8;
const TOLERANCE: f32 = 0.003;
const MIN_SPACING: f32 = 0.02;

fn split_cosine() -> f32 {
    (75.0f32 * std::f32::consts::PI / 180.0).cos()
}

fn simplify(pts: &[Num], tol: f32) -> Vec<Num> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    *keep.last_mut().expect("points") = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let start = pts[first];
        let span = pts[last] - start;
        let lsq = span.length_squared();
        let (mut worst, mut wi) = (-1f32, None);
        for (i, &p) in pts.iter().enumerate().take(last).skip(first + 1) {
            let mut off = p - start;
            if lsq > 1e-12 {
                off -= span * (off.dot(span) / lsq).clamp(0.0, 1.0);
            }
            if off.length() > worst {
                worst = off.length();
                wi = Some(i);
            }
        }
        if let Some(wi) = wi
            && worst > tol
        {
            keep[wi] = true;
            stack.push((first, wi));
            stack.push((wi, last));
        }
    }
    pts.iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

fn sharp(m: &[Num], i: usize) -> bool {
    let n = m.len();
    let before = (m[i] - m[(i + n - 1) % n]).normalize();
    let after = (m[(i + 1) % n] - m[i]).normalize();
    before.dot(after) < split_cosine()
}

/// Simplified point runs for prisms.
pub fn runs(points: &[Num], closed: bool) -> Vec<(Vec<Num>, bool)> {
    let mut src = points.to_vec();
    if closed {
        src.push(points[0]);
    }
    let p = simplify(&src, TOLERANCE);
    let mut res = Vec::new();
    if p.len() < 2 {
        return res;
    }
    let mut merged = vec![p[0]];
    for &q in &p[1..p.len() - 1] {
        if (q - *merged.last().expect("one")).length() >= MIN_SPACING {
            merged.push(q);
        }
    }
    let last = *p.last().expect("two");
    if merged.len() > 1 && (last - *merged.last().expect("one")).length() < MIN_SPACING {
        *merged.last_mut().expect("one") = last;
    } else {
        merged.push(last);
    }
    let mut closed = closed;
    if closed {
        merged.pop();
        closed = merged.len() >= 3;
    }
    if merged.len() < 2 || ((*merged.last().expect("two") - merged[0]).length() < 1e-6 && !closed) {
        return res;
    }
    let mut count = merged.len();
    if closed {
        let corners: Vec<usize> = (0..count).filter(|&i| sharp(&merged, i)).collect();
        if corners.is_empty() {
            res.push((merged, true));
            return res;
        }
        let c0 = corners[0];
        merged = merged[c0..].iter().chain(merged[..=c0].iter()).copied().collect();
        count = merged.len();
    }
    let mut start = 0;
    for i in 1..count - 1 {
        if sharp(&merged, i) {
            res.push((merged[start..=i].to_vec(), false));
            start = i;
        }
    }
    res.push((merged[start..count].to_vec(), false));
    res.into_iter().filter(|r| r.0.len() >= 2).collect()
}

/// Positions and indices of an open-ended prism hanging under a run (faces wind outward, z up).
pub fn prism(pts: &[Num], closed: bool, radius: f32, pos: &mut Vec<Num>, idx: &mut Vec<u32>) {
    let count = pts.len();
    let segments = if closed { count } else { count - 1 };
    let b0 = pos.len();
    let dirs: Vec<Num> = (0..segments)
        .map(|i| (pts[(i + 1) % count] - pts[i]).normalize())
        .collect();
    let mut prev_up: Option<Num> = None;
    for i in 0..count {
        let (before, after) = if closed {
            (dirs[(i + segments - 1) % segments], dirs[i % segments])
        } else {
            (dirs[i.saturating_sub(1)], dirs[i.min(segments - 1)])
        };
        let mut tangent = before + after;
        tangent = if tangent.length() > 1e-6 {
            tangent.normalize()
        } else {
            after
        };
        let mut up = Num::UNIT_Z - tangent * Num::UNIT_Z.dot(tangent);
        if up.length() < 1e-3 {
            let fb = prev_up.unwrap_or(Num::UNIT_Y);
            up = fb - tangent * fb.dot(tangent);
        }
        up = up.normalize();
        prev_up = Some(up);
        let side = tangent.cross(up);
        let mut bend = after - before;
        bend -= tangent * bend.dot(tangent);
        let mut stretch = 0f32;
        if bend.length() > 1e-6 {
            bend = bend.normalize();
            stretch = 1.0 / before.dot(tangent).clamp(0.5, 1.0) - 1.0;
        }
        let mut ring = [Num::ZERO; SIDES];
        for (c, r) in ring.iter_mut().enumerate() {
            let a = 2.0 * std::f32::consts::PI * c as f32 / SIDES as f32;
            let mut o = (up * a.cos() + side * a.sin()) * radius;
            if stretch != 0.0 {
                o += bend * (o.dot(bend) * stretch);
            }
            *r = o;
        }
        let centre = pts[i] - ring[0];
        for o in ring {
            pos.push(centre + o);
        }
    }
    for i in 0..segments {
        let here = b0 + i * SIDES;
        let there = b0 + ((i + 1) % count) * SIDES;
        for c in 0..SIDES {
            let f = (c + 1) % SIDES;
            let (a, b, cc, d) = (here + c, here + f, there + f, there + c);
            idx.extend_from_slice(&[a as u32, b as u32, cc as u32, a as u32, cc as u32, d as u32]);
        }
    }
}

/// The prism of a whole curve; with the number of runs and their length.
pub fn build(g: &GrindCurve) -> (Vec<Num>, Vec<u32>, i32, f32) {
    let (mut pos, mut idx) = (Vec::new(), Vec::new());
    let (mut pieces, mut length) = (0, 0f32);
    for (run, closed) in runs(&g.points, g.closed) {
        prism(&run, closed, g.radius, &mut pos, &mut idx);
        pieces += 1;
        let n = if closed { run.len() } else { run.len() - 1 };
        for i in 0..n {
            length += (run[(i + 1) % run.len()] - run[i]).length();
        }
    }
    (pos, idx, pieces, length)
}
