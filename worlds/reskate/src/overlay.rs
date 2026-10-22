//! Stacked overlay lifts for one merged mesh: decal- and water-class pieces moved along their faces' normals until
//! each lies at least `step` in front of every parallel surface it overlaps that should be under it (water over
//! everything; inside one entity the geometry order; across entities an opaque decal under an alpha-tested or
//! blended one, then the tile loop's order), in the shared vector arithmetic (`primase_meshops::vector`).

use std::collections::{BTreeSet, HashMap, HashSet};

use primase_meshops::vector::Vec3;

const BEHIND: f32 = 0.005;
const PARALLEL: f32 = 0.995;
const REACH: f32 = 0.05;
const CELL: f32 = 1.0;
const PAD: f32 = REACH + 0.002;
const BAND_TOL: f32 = 1e-4;
const MAX_CELLS: i64 = 20000;

/// One piece: triangles [t0, t1) and vertices [v0, v1) of a merged part.
#[derive(Clone, Debug)]
pub struct Piece {
    /// Which part (accumulator) the piece belongs to.
    pub part: usize,
    pub t0: usize,
    pub t1: usize,
    pub v0: usize,
    pub v1: usize,
    pub overlay: bool,
    /// An overlay drawn where it was authored, never lifted: a vertex-blend layer, coplanar with its base by design
    /// (decals draw coplanar and are hidden behind; its opacity separates it, a lift only risks changing the
    /// layers' look). It still takes part in the relations (draw order) and other overlays stack above it.
    pub fixed: bool,
    pub water: bool,
    pub pass: i32,
    pub order: i32,
    pub entity: i32,
    pub shift: f32,
}

#[derive(Clone, Copy, Debug)]
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    n: Vec3,
    piece: usize,
    t: usize,
}

/// What stacking did: its totals, and which overlay piece lies under which (`(below, above)` piece indices, sorted).
#[derive(Clone, Debug, Default)]
pub struct Stacked {
    pub stats: OverlayStats,
    pub under: Vec<(usize, usize)>,
    /// The pieces whose lift could not clear every surface ([`OverlayStats::conflicts_left`]), each with the pieces
    /// whose surfaces it still lies in the unclear band of (`(piece, other piece)`, sorted, once each).
    pub left: Vec<(usize, usize)>,
}

/// Totals over a run.
#[derive(Clone, Debug, Default)]
pub struct OverlayStats {
    pub pieces: i64,
    pub shifted: i64,
    pub water_shifted: i64,
    pub relations: i64,
    pub cycles: i64,
    pub conflicts: i64,
    pub conflicts_left: i64,
    pub max_shift: f32,
    pub shift_steps: [i64; 6],
}

impl OverlayStats {
    pub fn add(&mut self, o: &OverlayStats) {
        self.pieces += o.pieces;
        self.shifted += o.shifted;
        self.water_shifted += o.water_shifted;
        self.relations += o.relations;
        self.cycles += o.cycles;
        self.conflicts += o.conflicts;
        self.conflicts_left += o.conflicts_left;
        self.max_shift = self.max_shift.max(o.max_shift);
        for i in 0..6 {
            self.shift_steps[i] += o.shift_steps[i];
        }
    }
}

/// The parts the pieces index: positions (x, y, z per vertex) and indices.
#[derive(Debug)]
pub struct Parts<'a> {
    pub pos: Vec<&'a mut Vec<f32>>,
    pub idx: Vec<&'a Vec<u32>>,
}

fn on_top(a: &Piece, b: &Piece) -> bool {
    if a.water != b.water {
        return a.water;
    }
    if a.entity == b.entity || a.pass == b.pass {
        return a.order > b.order;
    }
    a.pass > b.pass
}

fn v(pos: &[f32], i: usize) -> Vec3 {
    Vec3::new(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2])
}

fn make(parts: &Parts, p: &Piece, t: usize, k: usize) -> Tri {
    let idx = parts.idx[p.part];
    let pos = &*parts.pos[p.part];
    let (a, b, c) = (
        idx[t * 3] as usize,
        idx[t * 3 + 1] as usize,
        idx[t * 3 + 2] as usize,
    );
    let (a, b, c) = (v(pos, a), v(pos, b), v(pos, c));
    let n = (b - a).cross(c - a);
    let len = n.length();
    Tri {
        a,
        b,
        c,
        n: if len > 1e-10 { n / len } else { Vec3::ZERO },
        piece: k,
        t,
    }
}

fn cells(t: &Tri) -> ((i32, i32, i32), (i32, i32, i32)) {
    let mn = t.a.min(t.b.min(t.c)) - Vec3::splat(PAD);
    let mx = t.a.max(t.b.max(t.c)) + Vec3::splat(PAD);
    let f = |x: f32| (x / CELL).floor() as i32;
    ((f(mn.x), f(mn.y), f(mn.z)), (f(mx.x), f(mx.y), f(mx.z)))
}

fn inside(p: Vec3, u: &Tri) -> bool {
    let q = p - u.n * (p - u.a).dot(u.n);
    let (v0, v1, v2) = (u.c - u.a, u.b - u.a, q - u.a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den <= 1e-12 {
        return false;
    }
    let a = (d11 * d20 - d01 * d21) / den;
    let b = (d00 * d21 - d01 * d20) / den;
    a >= -1e-4 && b >= -1e-4 && a + b <= 1.0 + 1e-4
}

fn overlaps(t: &Tri, u: &Tri) -> bool {
    let c = (t.a + t.b + t.c) / 3.0;
    if inside(c, u)
        || inside(Vec3::lerp(t.a, c, 0.25), u)
        || inside(Vec3::lerp(t.b, c, 0.25), u)
        || inside(Vec3::lerp(t.c, c, 0.25), u)
    {
        return true;
    }
    inside((u.a + u.b + u.c) / 3.0, t)
}

fn related(t: &Tri, u: &Tri) -> Option<f32> {
    if t.n.dot(u.n).abs() < PARALLEL {
        return None;
    }
    let d = t.n.dot(((t.a + t.b + t.c) / 3.0) - u.a);
    overlaps(t, u).then_some(d)
}

/// Move the piece's vertices by `by` along their drawn faces' summed normal.
fn shift(parts: &mut Parts, p: &Piece, by: f32) {
    let idx = parts.idx[p.part];
    let pos = &mut *parts.pos[p.part];
    let mut fnrm = vec![Vec3::ZERO; p.v1 - p.v0];
    for t in p.t0..p.t1 {
        let (a, b, c) = (
            idx[t * 3] as usize,
            idx[t * 3 + 1] as usize,
            idx[t * 3 + 2] as usize,
        );
        let n = (v(pos, b) - v(pos, a)).cross(v(pos, c) - v(pos, a));
        fnrm[a - p.v0] += n;
        fnrm[b - p.v0] += n;
        fnrm[c - p.v0] += n;
    }
    for vi in p.v0..p.v1 {
        let n = fnrm[vi - p.v0];
        if n.length_squared() < 1e-20 {
            continue;
        }
        let n = n.normalize() * by;
        pos[vi * 3] += n.x;
        pos[vi * 3 + 1] += n.y;
        pos[vi * 3 + 2] += n.z;
    }
}

/// Stack the overlays of `pieces` (in place on `parts`), recording each piece's shift.
pub fn apply(step: f32, parts: &mut Parts, pieces: &mut [Piece]) -> Stacked {
    let mut st = OverlayStats::default();
    if step <= 0.0 || !pieces.iter().any(|p| p.overlay) {
        return Stacked {
            stats: st,
            under: Vec::new(),
            left: Vec::new(),
        };
    }
    let mut tris: Vec<Tri> = Vec::new();
    let mut piece_tris: Vec<Vec<usize>> = Vec::with_capacity(pieces.len());
    for (k, p) in pieces.iter().enumerate() {
        let mut list = Vec::with_capacity(p.t1 - p.t0);
        for t in p.t0..p.t1 {
            let tr = make(parts, p, t, k);
            if tr.n == Vec3::ZERO {
                continue;
            }
            list.push(tris.len());
            tris.push(tr);
        }
        piece_tris.push(list);
    }
    let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
    let mut huge: Vec<usize> = Vec::new();
    let mut huge_set: HashSet<usize> = HashSet::new();
    let file = |i: usize,
                tris: &[Tri],
                grid: &mut HashMap<(i32, i32, i32), Vec<usize>>,
                huge: &mut Vec<usize>,
                huge_set: &mut HashSet<usize>| {
        if huge_set.contains(&i) {
            return;
        }
        let (lo, hi) = cells(&tris[i]);
        let n = i64::from(hi.0 - lo.0 + 1) * i64::from(hi.1 - lo.1 + 1) * i64::from(hi.2 - lo.2 + 1);
        if n > MAX_CELLS {
            huge_set.insert(i);
            huge.push(i);
            return;
        }
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for z in lo.2..=hi.2 {
                    let l = grid.entry((x, y, z)).or_default();
                    if l.last() != Some(&i) {
                        l.push(i);
                    }
                }
            }
        }
    };
    for i in 0..tris.len() {
        file(i, &tris, &mut grid, &mut huge, &mut huge_set);
    }
    // every triangle near t (the cells its box touches, and the huge ones), each once
    let near =
        |t: &Tri, grid: &HashMap<(i32, i32, i32), Vec<usize>>, huge: &[usize], out: &mut Vec<usize>| {
            out.clear();
            let mut seen: HashSet<usize> = HashSet::new();
            let (lo, hi) = cells(t);
            let n = i64::from(hi.0 - lo.0 + 1) * i64::from(hi.1 - lo.1 + 1) * i64::from(hi.2 - lo.2 + 1);
            if n <= MAX_CELLS {
                for x in lo.0..=hi.0 {
                    for y in lo.1..=hi.1 {
                        for z in lo.2..=hi.2 {
                            if let Some(l) = grid.get(&(x, y, z)) {
                                for &j in l {
                                    if seen.insert(j) {
                                        out.push(j);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for &j in huge {
                if seen.insert(j) {
                    out.push(j);
                }
            }
        };

    // 1. which overlays lie on which: edges below -> above
    let overlays: Vec<usize> = (0..pieces.len()).filter(|&k| pieces[k].overlay).collect();
    let mut above: HashMap<usize, HashSet<usize>> = overlays.iter().map(|&k| (k, HashSet::new())).collect();
    let mut indeg: HashMap<usize, i32> = overlays.iter().map(|&k| (k, 0)).collect();
    let mut buf = Vec::new();
    for &k in &overlays {
        for &i in &piece_tris[k] {
            let t = tris[i];
            near(&t, &grid, &huge, &mut buf);
            for &j in &buf {
                let u = tris[j];
                let o = u.piece;
                if o == k || !pieces[o].overlay || above[&k].contains(&o) || above[&o].contains(&k) {
                    continue;
                }
                let Some(d) = related(&t, &u) else { continue };
                if d.abs() > REACH {
                    continue;
                }
                if on_top(&pieces[k], &pieces[o]) {
                    above.get_mut(&o).expect("overlay").insert(k);
                    *indeg.get_mut(&k).expect("overlay") += 1;
                } else {
                    above.get_mut(&k).expect("overlay").insert(o);
                    *indeg.get_mut(&o).expect("overlay") += 1;
                }
                st.relations += 1;
            }
        }
    }

    // 2. placement order: topological; ties and cycles by (pass, order, index)
    let key = |k: usize| (pieces[k].pass, pieces[k].order, k);
    let mut ready: BTreeSet<(i32, i32, usize)> = BTreeSet::new();
    let mut left: BTreeSet<(i32, i32, usize)> = BTreeSet::new();
    for &k in &overlays {
        left.insert(key(k));
        if indeg[&k] == 0 {
            ready.insert(key(k));
        }
    }
    let mut order = Vec::with_capacity(overlays.len());
    while let Some(&first) = left.first() {
        let k = match ready.pop_first() {
            Some(r) => r.2,
            None => {
                st.cycles += 1;
                first.2
            }
        };
        left.remove(&key(k));
        order.push(k);
        let ups: Vec<usize> = above[&k].iter().copied().collect();
        for a in ups {
            if left.contains(&key(a)) {
                let d = indeg.get_mut(&a).expect("overlay");
                *d -= 1;
                if *d == 0 {
                    ready.insert(key(a));
                }
            }
        }
    }

    // 3. place each piece against everything placed so far
    let mut placed: Vec<bool> = pieces.iter().map(|p| !p.overlay).collect();
    let mut ds: Vec<(f32, usize)> = Vec::new();
    let mut left: BTreeSet<(usize, usize)> = BTreeSet::new();
    for &k in &order {
        st.pieces += 1;
        if pieces[k].fixed {
            placed[k] = true;
            continue;
        }
        ds.clear();
        for &i in &piece_tris[k] {
            let t = tris[i];
            near(&t, &grid, &huge, &mut buf);
            for &j in &buf {
                let u = tris[j];
                if u.piece == k || !placed[u.piece] {
                    continue;
                }
                if let Some(d) = related(&t, &u)
                    && d.abs() <= REACH
                {
                    ds.push((d, u.piece));
                }
            }
        }
        let (mut lo, mut hi) = (0f32, f32::MAX);
        for &(d, _) in &ds {
            if d >= -BEHIND {
                lo = lo.max(step - d);
            } else {
                hi = hi.min(-BEHIND - d);
            }
        }
        let in_band = |l: f32, d: f32| d + l > -BEHIND + BAND_TOL && d + l < step - BAND_TOL;
        let band = |l: f32, ds: &[(f32, usize)]| ds.iter().filter(|&&(d, _)| in_band(l, d)).count();
        let mut total = lo;
        if lo > hi {
            st.conflicts += 1;
            let mut past = lo;
            for &(d, _) in &ds {
                if d < -BEHIND && d + lo > -BEHIND {
                    past = past.max(step - d);
                }
            }
            let mut cands = [lo, 0f32.max(hi), past];
            // OrderBy(Band).ThenBy(l): stable, by band count then value
            cands.sort_by(|a, b| {
                band(*a, &ds)
                    .cmp(&band(*b, &ds))
                    .then(a.partial_cmp(b).expect("finite"))
            });
            total = cands[0];
            if band(total, &ds) > 0 {
                st.conflicts_left += 1;
                left.extend(
                    ds.iter()
                        .filter(|&&(d, _)| in_band(total, d))
                        .map(|&(_, u)| (k, u)),
                );
            }
        }
        if total > 0.0 {
            let p = pieces[k].clone();
            shift(parts, &p, total);
            for &i in &piece_tris[k] {
                tris[i] = make(parts, &p, tris[i].t, k);
                file(i, &tris, &mut grid, &mut huge, &mut huge_set);
            }
            pieces[k].shift = total;
            st.shifted += 1;
            if pieces[k].water {
                st.water_shifted += 1;
            }
            st.max_shift = st.max_shift.max(total);
            let b = ((total / step - 1e-3).ceil() as i32).clamp(1, 6) as usize;
            st.shift_steps[b - 1] += 1;
        }
        placed[k] = true;
    }
    let mut under: Vec<(usize, usize)> = above
        .iter()
        .flat_map(|(&k, ups)| ups.iter().map(move |&a| (k, a)))
        .collect();
    under.sort_unstable();
    Stacked {
        stats: st,
        under,
        left: left.into_iter().collect(),
    }
}

/// The stack `apply` made, put on unshifted parts again: each piece moved once by its recorded `shift`, as `apply`
/// moved it (a piece's vertices are its own, so the order does not matter). The same positions, bit for bit.
pub fn replay(parts: &mut Parts, pieces: &[Piece]) {
    for p in pieces.iter().filter(|p| p.shift > 0.0) {
        shift(parts, p, p.shift);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 x 1 m square at height `y` (two triangles, wound to face +y), appended to `pos` / `idx`.
    fn square(pos: &mut Vec<f32>, idx: &mut Vec<u32>, y: f32) -> (usize, usize, usize, usize) {
        let (v0, t0) = (pos.len() / 3, idx.len() / 3);
        pos.extend([0.0, y, 0.0, 1.0, y, 0.0, 1.0, y, 1.0, 0.0, y, 1.0]);
        let b = v0 as u32;
        idx.extend([b, b + 2, b + 1, b, b + 3, b + 2]);
        (t0, idx.len() / 3, v0, pos.len() / 3)
    }

    fn piece(
        part: usize,
        span: (usize, usize, usize, usize),
        overlay: bool,
        fixed: bool,
        order: i32,
    ) -> Piece {
        Piece {
            part,
            t0: span.0,
            t1: span.1,
            v0: span.2,
            v1: span.3,
            overlay,
            fixed,
            water: false,
            pass: 1,
            order,
            entity: 0,
            shift: 0.0,
        }
    }

    #[test]
    fn fixed_overlays_stay_and_other_overlays_stack_above_them() {
        // part 0: the base; part 1: two vertex-blend layers (fixed), all coplanar; part 2: an ordinary decal
        let (mut p0, mut i0, mut p1, mut i1, mut p2, mut i2) =
            (vec![], vec![], vec![], vec![], vec![], vec![]);
        let base = square(&mut p0, &mut i0, 0.0);
        let l1 = square(&mut p1, &mut i1, 0.0);
        let l2 = square(&mut p1, &mut i1, 0.0);
        let decal = square(&mut p2, &mut i2, 0.0);
        let mut pieces = vec![
            piece(0, base, false, false, 0),
            piece(1, l1, true, true, 1),
            piece(1, l2, true, true, 2),
            piece(2, decal, true, false, 3),
        ];
        let (i0c, i1c, i2c) = (i0.clone(), i1.clone(), i2.clone());
        let mut parts = Parts {
            pos: vec![&mut p0, &mut p1, &mut p2],
            idx: vec![&i0c, &i1c, &i2c],
        };
        let st = apply(0.004, &mut parts, &mut pieces);
        assert!(
            p1.as_chunks::<3>().0.iter().all(|v| v[1] == 0.0),
            "fixed layers lifted: {p1:?}"
        );
        assert_eq!((pieces[1].shift, pieces[2].shift), (0.0, 0.0));
        assert!(
            (pieces[3].shift - 0.004).abs() < 1e-6,
            "decal shift {}",
            pieces[3].shift
        );
        assert!(p2.as_chunks::<3>().0.iter().all(|v| (v[1] - 0.004).abs() < 1e-6));
        // the layers keep their order relations: layer 1 under layer 2 under the decal
        assert!(
            st.under.contains(&(1, 2)) && st.under.contains(&(2, 3)),
            "{:?}",
            st.under
        );
        assert_eq!((st.stats.pieces, st.stats.shifted), (3, 1));
        drop((i0, i1, i2));
    }

    /// A piece that has to clear the base (a 4 mm lift) and also stay behind pieces placed before it that lie 6 and 12
    /// mm in front of it (more than 5 mm behind: no lift may reach them) cannot do both; nor can lifting past them
    /// (10 mm, which puts it 2 mm behind the 12 mm one): the pass counts a conflict it could not clear and says
    /// against which piece it is left. Worked by hand (y = 0 base, the movable piece at y = 0, fixed pieces at 6 and
    /// 12 mm placed under it by order; step 4 mm, 5 mm "behind", band (-5 mm, +4 mm) exclusive of 0.1 mm at both ends):
    /// the three lifts it can take, 4 mm (the 6 mm piece 2 mm in front: in the band), 1 mm (the base 1 mm behind: in
    /// the band) and 10 mm (the 12 mm piece 2 mm in front: in the band) each leave one surface in the band; the
    /// smallest lift wins the tie, 1 mm, and what it leaves is the base.
    #[test]
    fn a_piece_that_cannot_clear_every_surface_is_reported_with_the_piece_it_is_left_against() {
        let (mut p0, mut i0, mut p1, mut i1, mut p2, mut i2, mut p3, mut i3) =
            (vec![], vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        let base = square(&mut p0, &mut i0, 0.0);
        let near = square(&mut p1, &mut i1, 0.006);
        let far = square(&mut p2, &mut i2, 0.012);
        let movable = square(&mut p3, &mut i3, 0.0);
        let mut pieces = vec![
            piece(0, base, false, false, 0),
            piece(1, near, true, true, 1),
            piece(2, far, true, true, 2),
            piece(3, movable, true, false, 3),
        ];
        let (i0c, i1c, i2c, i3c) = (i0.clone(), i1.clone(), i2.clone(), i3.clone());
        let mut parts = Parts {
            pos: vec![&mut p0, &mut p1, &mut p2, &mut p3],
            idx: vec![&i0c, &i1c, &i2c, &i3c],
        };
        let st = apply(0.004, &mut parts, &mut pieces);
        assert_eq!(
            (st.stats.conflicts, st.stats.conflicts_left),
            (1, 1),
            "{:?}",
            st.stats
        );
        assert!((pieces[3].shift - 0.001).abs() < 1e-6, "lift {}", pieces[3].shift);
        assert_eq!(
            st.left,
            vec![(3, 0)],
            "the movable piece is left against the base"
        );
        drop((i0, i1, i2, i3));
    }

    /// Replaying the shifts `apply` recorded on the unshifted parts gives its positions exactly: three decals stacked
    /// over a tilted, uneven base (normals that are not axes, so a shift moves every coordinate).
    #[test]
    fn a_replayed_stack_is_the_stack() {
        let tilt = |pos: &mut Vec<f32>, idx: &mut Vec<u32>, lift: f32| {
            let (v0, t0) = (pos.len() / 3, idx.len() / 3);
            pos.extend([
                0.1,
                0.3 + lift,
                0.0,
                1.3,
                0.5 + lift,
                0.2,
                1.1,
                0.9 + lift,
                1.4,
                0.0,
                0.6 + lift,
                1.1,
            ]);
            let b = v0 as u32;
            idx.extend([b, b + 2, b + 1, b, b + 3, b + 2]);
            (t0, idx.len() / 3, v0, pos.len() / 3)
        };
        let (mut base, mut bi, mut decals, mut di) = (vec![], vec![], vec![], vec![]);
        let b = tilt(&mut base, &mut bi, 0.0);
        let spans: Vec<_> = (0..3)
            .map(|k| tilt(&mut decals, &mut di, k as f32 * 1e-3))
            .collect();
        let fresh = || {
            let mut pieces = vec![piece(0, b, false, false, 0)];
            for (k, &sp) in spans.iter().enumerate() {
                pieces.push(piece(1, sp, true, false, k as i32 + 1));
            }
            pieces
        };
        let (bi2, di2) = (bi.clone(), di.clone());
        let (mut base_a, mut decals_a) = (base.clone(), decals.clone());
        let mut stacked = fresh();
        apply(
            0.004,
            &mut Parts {
                pos: vec![&mut base_a, &mut decals_a],
                idx: vec![&bi2, &di2],
            },
            &mut stacked,
        );
        assert!(
            stacked.iter().filter(|p| p.shift > 0.0).count() >= 2,
            "{stacked:?}"
        );
        let (mut base_r, mut decals_r) = (base.clone(), decals.clone());
        let mut replayed = fresh();
        for (r, a) in replayed.iter_mut().zip(&stacked) {
            r.shift = a.shift;
        }
        replay(
            &mut Parts {
                pos: vec![&mut base_r, &mut decals_r],
                idx: vec![&bi2, &di2],
            },
            &replayed,
        );
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&decals_r), bits(&decals_a));
        assert_eq!(bits(&base_r), bits(&base_a));
        assert_ne!(bits(&decals_r), bits(&decals));
        drop((bi, di));
    }
}
