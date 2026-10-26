//! Questions about a world's collision, in the core frame: the box of the play space ([`collision_box`]) and the first
//! surface below a point ([`ground_below`]). Every collision soup placed in the world counts: one on an entity no
//! placement takes as its prototype, by that entity's own transform (as stored without one), and a prototype's once
//! per placement, by the placement's transform.

use std::collections::BTreeMap;

use primase_core::{Collision, Entity, Instance, Transform, World};

/// Each placed soup with the transform placing it.
fn placed(w: &World) -> Vec<(&Collision, Option<Transform>)> {
    let mut by_prototype: BTreeMap<Entity, Vec<Transform>> = BTreeMap::new();
    for (e, i) in w.iter::<Instance>() {
        by_prototype
            .entry(i.prototype)
            .or_default()
            .push(w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY));
    }
    let mut out = Vec::new();
    for (e, c) in w.iter::<Collision>() {
        match by_prototype.get(&e) {
            Some(ts) => out.extend(ts.iter().map(|t| (c, Some(*t)))),
            None => out.push((c, w.get::<Transform>(e).copied())),
        }
    }
    out
}

/// Point `v` in the core frame, through `t` when there is one.
pub fn at(t: Option<&Transform>, v: [f32; 3]) -> [f64; 3] {
    match t {
        Some(t) => std::array::from_fn(|r| {
            (0..3)
                .map(|k| f64::from(t.cols[k][r]) * f64::from(v[k]))
                .sum::<f64>()
                + f64::from(t.cols[3][r])
        }),
        None => v.map(f64::from),
    }
}

/// One placed collision soup: its box in the core frame and its triangle count.
struct SoupBox {
    lo: [f64; 3],
    hi: [f64; 3],
    triangles: usize,
}

/// Every placed soup no wider than `max_extent` metres on any axis: a soup too large on an axis (or with no finite
/// extent) is not play space.
fn play_soups(w: &World, max_extent: f64) -> Vec<SoupBox> {
    let mut out = Vec::new();
    for (c, t) in placed(w) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for &p in &c.positions {
            let q = at(t.as_ref(), p);
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        if (0..3).any(|i| (hi[i] - lo[i]).is_nan() || hi[i] - lo[i] > max_extent) {
            continue;
        }
        out.push(SoupBox {
            lo,
            hi,
            triangles: c.triangles.len(),
        });
    }
    out
}

/// The box enclosing `soups`; `None` when there are none.
fn enclosing<'a>(soups: impl IntoIterator<Item = &'a SoupBox>) -> Option<[[f64; 3]; 2]> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for s in soups {
        for i in 0..3 {
            lo[i] = lo[i].min(s.lo[i]);
            hi[i] = hi[i].max(s.hi[i]);
        }
    }
    (lo[0] <= hi[0]).then_some([lo, hi])
}

/// The box (min and max corners) of every placed collision soup no wider than `max_extent` metres on any axis: the
/// play space of a level whose backdrops (skylines, building sides seen from afar) are large soups. `None` when no soup
/// is left.
pub fn collision_box(w: &World, max_extent: f64) -> Option<[[f64; 3]; 2]> {
    enclosing(&play_soups(w, max_extent))
}

/// The box of the largest connected group of placed collision soups no wider than `max_extent` metres on any axis:
/// soups are joined when the gap between their boxes on the ground plane (x, z) is at most `max_gap` metres, and the
/// group with the most triangles is taken (a tie goes to the group of the last soup in placement order). Where
/// [`collision_box`] takes in a small piece lying far from the level, this leaves it out: a level's detached pieces
/// (another district's streets 300 m off) do not stretch the play space. `None` when no soup is left.
pub fn connected_collision_box(w: &World, max_extent: f64, max_gap: f64) -> Option<[[f64; 3]; 2]> {
    let soups = play_soups(w, max_extent);
    enclosing(largest_group(&soups, max_gap))
}

/// The gap between two soups' boxes on the ground plane (x, z), metres: 0 where they overlap.
fn gap(a: &SoupBox, b: &SoupBox) -> f64 {
    let dx = (a.lo[0] - b.hi[0]).max(b.lo[0] - a.hi[0]).max(0.0);
    let dz = (a.lo[2] - b.hi[2]).max(b.lo[2] - a.hi[2]).max(0.0);
    dx.hypot(dz)
}

/// The group of `soups` joined by gaps of at most `max_gap` with the most triangles.
fn largest_group(soups: &[SoupBox], max_gap: f64) -> Vec<&SoupBox> {
    let mut parent: Vec<usize> = (0..soups.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..soups.len() {
        for j in i + 1..soups.len() {
            if gap(&soups[i], &soups[j]) <= max_gap {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<&SoupBox>> = BTreeMap::new();
    for (i, soup) in soups.iter().enumerate() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(soup);
    }
    groups
        .into_values()
        .max_by_key(|g| g.iter().map(|s| s.triangles).sum::<usize>())
        .unwrap_or_default()
}

/// The height of the first collision surface straight below `p` (core frame, +Y up): the highest triangle of any placed
/// soup whose ground footprint holds `p`'s (x, z), at its height there, counting a surface up to `tolerance` metres
/// above `p` (so a point standing on one keeps it). `None` when nothing is below.
pub fn ground_below(w: &World, p: [f64; 3], tolerance: f64) -> Option<f64> {
    Ground::of(w).surface_below(p, tolerance).map(|s| s.height)
}

/// The surface [`Ground::surface_below`] finds: its height at the point, and how level it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    /// Its height below the point (core +Y).
    pub height: f64,
    /// Its unit normal's vertical part, unsigned: 1 level, 0 a vertical wall.
    pub level: f64,
}

/// Every placed collision triangle in the core frame, for asking many points what lies below them.
#[derive(Clone, Debug, Default)]
pub struct Ground {
    triangles: Vec<[[f64; 3]; 3]>,
}

impl Ground {
    /// The world's placed soups' triangles (as [`ground_below`] counts them).
    pub fn of(w: &World) -> Ground {
        let mut triangles = Vec::new();
        for (c, t) in placed(w) {
            for tri in &c.triangles {
                if let Some(corners) = tri
                    .iter()
                    .map(|&i| c.positions.get(i as usize).map(|&v| at(t.as_ref(), v)))
                    .collect::<Option<Vec<_>>>()
                {
                    triangles.push([corners[0], corners[1], corners[2]]);
                }
            }
        }
        Ground { triangles }
    }

    /// The first surface straight below `p`: the highest triangle whose ground footprint holds `p`'s (x, z), counting
    /// one up to `tolerance` metres above `p`. `None` when nothing is below.
    pub fn surface_below(&self, p: [f64; 3], tolerance: f64) -> Option<Surface> {
        let mut best: Option<Surface> = None;
        for &[a, b, d] in &self.triangles {
            // barycentric coordinates of (p.x, p.z) in the triangle's ground footprint
            let det = (b[2] - d[2]) * (a[0] - d[0]) + (d[0] - b[0]) * (a[2] - d[2]);
            if det.abs() < 1e-12 {
                continue;
            }
            let l1 = ((b[2] - d[2]) * (p[0] - d[0]) + (d[0] - b[0]) * (p[2] - d[2])) / det;
            let l2 = ((d[2] - a[2]) * (p[0] - d[0]) + (a[0] - d[0]) * (p[2] - d[2])) / det;
            let l3 = 1.0 - l1 - l2;
            if l1 < 0.0 || l2 < 0.0 || l3 < 0.0 {
                continue;
            }
            let y = l1 * a[1] + l2 * b[1] + l3 * d[1];
            if y <= p[1] + tolerance && best.is_none_or(|s| y > s.height) {
                let (u, v) = (
                    [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                    [d[0] - a[0], d[1] - a[1], d[2] - a[2]],
                );
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                best = Some(Surface {
                    height: y,
                    level: if len > 0.0 { n[1].abs() / len } else { 0.0 },
                });
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soup(w: &mut World, corners: [[f32; 3]; 3], t: Option<Transform>) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Collision {
                positions: corners.to_vec(),
                triangles: vec![[0, 1, 2]],
                surfaces: vec![0],
            },
        );
        if let Some(t) = t {
            w.insert(e, t);
        }
        e
    }

    /// A floor and a 300 m backdrop: the box is the floor's; a point 2 m above the floor finds it, one beside it
    /// nothing; a prototype's soup counts once per placement.
    #[test]
    fn the_play_box_leaves_out_backdrops_and_the_ground_is_found_below() {
        let mut w = World::new();
        soup(
            &mut w,
            [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
            None,
        );
        soup(
            &mut w,
            [[-150.0, 0.0, -50.0], [150.0, 0.0, -50.0], [0.0, 80.0, -50.0]],
            None,
        );
        assert_eq!(
            collision_box(&w, 100.0),
            Some([[0.0, 0.0, 0.0], [10.0, 0.0, 10.0]])
        );
        assert_eq!(ground_below(&w, [1.0, 2.0, 1.0], 0.05), Some(0.0));
        assert_eq!(ground_below(&w, [20.0, 2.0, 20.0], 0.05), None);
        // a surface 3 cm above the point counts, 10 cm does not
        assert_eq!(ground_below(&w, [1.0, -0.03, 1.0], 0.05), Some(0.0));
        assert_eq!(ground_below(&w, [1.0, -0.1, 1.0], 0.05), None);

        let mut w = World::new();
        let proto = soup(&mut w, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], None);
        for x in [0.0f32, 50.0] {
            let p = w.spawn();
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 0.0, 0.0];
            w.insert(p, t);
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
        }
        assert_eq!(
            collision_box(&w, 100.0),
            Some([[0.0, 0.0, 0.0], [51.0, 0.0, 1.0]])
        );
    }

    /// Over a 45-degree ramp under a level ledge, the ledge is the surface found, level; beside the ledge the ramp,
    /// at its height there, its normal half way between level and a wall.
    #[test]
    fn the_surface_below_tells_how_level_it_is() {
        let mut w = World::new();
        soup(&mut w, [[0.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 0.0, 4.0]], None);
        soup(&mut w, [[0.0, 0.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0]], None);
        soup(&mut w, [[0.0, 2.0, 0.0], [1.0, 2.0, 0.0], [0.0, 2.0, 1.0]], None);
        let ground = Ground::of(&w);
        let ledge = ground.surface_below([0.2, 2.5, 0.2], 0.0).unwrap();
        assert_eq!((ledge.height, ledge.level), (2.0, 1.0));
        let ramp = ground.surface_below([3.0, 3.5, 2.0], 0.0).unwrap();
        assert!((ramp.height - 3.0).abs() < 1e-9 && (ramp.level - 0.5f64.sqrt()).abs() < 1e-9);
        assert_eq!(ground.surface_below([3.0, 2.9, 2.0], 0.05), None);
        assert!(ground.surface_below([3.0, 2.9, 2.0], 0.2).is_some());
    }

    /// A flat square soup `size` metres across centred on core (x, z) with `triangles` triangles.
    fn square(w: &mut World, x: f32, z: f32, size: f32, triangles: usize) {
        let h = size / 2.0;
        let e = w.spawn();
        w.insert(
            e,
            Collision {
                positions: vec![[-h, 0.0, -h], [h, 0.0, -h], [h, 0.0, h], [-h, 0.0, h]],
                triangles: vec![[0, 2, 1]; triangles],
                surfaces: vec![0; triangles],
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 0.0, z];
        w.insert(e, t);
    }

    /// A chain of soups 80 m apart joins; a small piece 300 m past its end is left out; the group with the most
    /// triangles wins, not the widest; a soup wider than the limit is no play space; with no gap to join over, every
    /// soup is its own group.
    #[test]
    fn the_connected_box_leaves_out_detached_pieces() {
        let mut w = World::new();
        for k in 0..4 {
            square(&mut w, k as f32 * 140.0, 0.0, 60.0, 10);
        }
        square(&mut w, 3.0 * 140.0 + 400.0, 0.0, 20.0, 3);
        assert_eq!(
            connected_collision_box(&w, 400.0, 100.0),
            Some([[-30.0, 0.0, -30.0], [450.0, 0.0, 30.0]])
        );
        // the plain box takes the stray in
        assert_eq!(collision_box(&w, 400.0).unwrap()[1][0], 3.0 * 140.0 + 410.0);

        let mut w = World::new();
        square(&mut w, 0.0, 0.0, 300.0, 4);
        square(&mut w, 1000.0, 0.0, 10.0, 40);
        assert_eq!(
            connected_collision_box(&w, 400.0, 100.0),
            Some([[995.0, 0.0, -5.0], [1005.0, 0.0, 5.0]])
        );

        let mut w = World::new();
        square(&mut w, 0.0, 0.0, 30.0, 2);
        square(&mut w, 10.0, 0.0, 500.0, 2000);
        assert_eq!(
            connected_collision_box(&w, 400.0, 100.0),
            Some([[-15.0, 0.0, -15.0], [15.0, 0.0, 15.0]])
        );
        let mut w = World::new();
        square(&mut w, 0.0, 0.0, 500.0, 2);
        assert_eq!(connected_collision_box(&w, 400.0, 100.0), None);
    }
}
