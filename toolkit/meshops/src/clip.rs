//! Convex polygons clipped by half-planes (Sutherland–Hodgman): one step ([`clip_half_plane`]) over any vertex a caller
//! can mix, and a point outline clipped to a rectangle ([`clip_convex_to_rect`]).
//!
//! **Boundary rule.** A vertex is kept where its side is at least 0 (one on the line included, once). An edge adds the
//! crossing only where its ends are strictly on opposite sides (one above 0, the other below), at
//! `t = side(a) / (side(a) - side(b))`, strictly between them. An edge that only touches the line adds nothing, so no
//! vertex is ever doubled. A vertex whose side isn't a number is dropped and its edges add nothing.
//!
//! **Degenerates.** None are removed: what is left may have fewer than three vertices, or no area. Which polygon is too
//! small to keep is the caller's rule (a count, a threshold in its own units), applied with [`twice_signed_area`].

use std::ops::{Div, Sub};

/// `polygon` (convex, its vertices in order) cut to where `side` is at least 0 (`side` linear over the plane: a signed
/// distance, or any multiple of one), by the module's boundary rule; a crossing vertex is `mix(a, b, t)`, `t` in (0, 1)
/// from `a` towards `b`.
pub fn clip_half_plane<V: Copy, F>(
    polygon: &[V],
    side: impl Fn(&V) -> F,
    mix: impl Fn(&V, &V, F) -> V,
) -> Vec<V>
where
    F: Copy + Default + PartialOrd + Sub<Output = F> + Div<Output = F>,
{
    let zero = F::default();
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (i, a) in polygon.iter().enumerate() {
        let b = &polygon[(i + 1) % polygon.len()];
        let (sa, sb) = (side(a), side(b));
        if sa >= zero {
            out.push(*a);
        }
        if (sa > zero && sb < zero) || (sa < zero && sb > zero) {
            out.push(mix(a, b, sa / (sa - sb)));
        }
    }
    out
}

/// `polygon` (convex, points in order) clipped to the rectangle `[x0, y0, x1, y1]`: to x at least x0, x at most x1,
/// y at least y0 and y at most y1, in that order, each a [`clip_half_plane`]. A crossing lies on its boundary exactly
/// (the cut coordinate is the bound) and its other coordinate is interpolated, `a + (b - a) t`. No degenerate is
/// removed (see the module documentation).
pub fn clip_convex_to_rect(polygon: &[[f32; 2]], [x0, y0, x1, y1]: [f32; 4]) -> Vec<[f32; 2]> {
    let mut out = polygon.to_vec();
    for (axis, bound, above) in [(0, x0, true), (0, x1, false), (1, y0, true), (1, y1, false)] {
        let other = 1 - axis;
        out = clip_half_plane(
            &out,
            |v: &[f32; 2]| if above { v[axis] - bound } else { bound - v[axis] },
            |a, b, t| {
                let mut c = [0.0; 2];
                c[axis] = bound;
                c[other] = a[other] + (b[other] - a[other]) * t;
                c
            },
        );
    }
    out
}

/// Twice the signed area of the outline `p` (positive counter-clockwise, x right and y up), summed in order from 0.
pub fn twice_signed_area(p: &[[f32; 2]]) -> f32 {
    let mut a = 0f32;
    for i in 0..p.len() {
        let (q, n) = (p[i], p[(i + 1) % p.len()]);
        a += q[0] * n[1] - n[0] * q[1];
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, s: f32) -> Vec<[f32; 2]> {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
    }

    /// A square half inside keeps its inside half, the crossings on the boundary exactly; wholly inside it is
    /// unchanged, wholly outside nothing is left.
    #[test]
    fn a_rectangle_keeps_what_is_inside() {
        let c = clip_convex_to_rect(&square(0.0, 0.0, 10.0), [5.0, -5.0, 15.0, 10.0]);
        assert_eq!(c, vec![[5.0, 0.0], [10.0, 0.0], [10.0, 10.0], [5.0, 10.0]]);
        assert_eq!(twice_signed_area(&c), 100.0);
        let whole = square(1.0, 1.0, 2.0);
        assert_eq!(clip_convex_to_rect(&whole, [0.0, 0.0, 5.0, 5.0]), whole);
        assert!(clip_convex_to_rect(&square(0.0, 0.0, 10.0), [20.0, 0.0, 30.0, 10.0]).is_empty());
        // a corner triangle half in: the part with x <= 5 of (0, 0) (10, 0) (0, 10) is a trapezoid of 37.5
        let t = clip_convex_to_rect(&[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]], [0.0, 0.0, 5.0, 20.0]);
        assert_eq!(twice_signed_area(&t), 75.0);
        // the cut coordinate is the bound, not an interpolation of it
        let slant = clip_convex_to_rect(&[[0.1, 0.0], [0.7, 0.3], [0.2, 0.9]], [0.3, 0.0, 1.0, 1.0]);
        assert!(
            slant.iter().filter(|v| v[0] != 0.7).all(|v| v[0] == 0.3),
            "{slant:?}"
        );
    }

    /// A vertex on the boundary is kept once, and an edge that only touches the line adds nothing: no doubled vertex,
    /// and a polygon touching the rectangle at a corner or along an edge leaves what lies on it (no area).
    #[test]
    fn a_vertex_on_the_boundary_is_kept_once() {
        // corner (5, 0) on x = 5: kept once, the edge to it from outside and the edge from it outside add nothing
        let c = clip_convex_to_rect(
            &[[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]],
            [5.0, 0.0, 10.0, 10.0],
        );
        assert_eq!(c, vec![[5.0, 0.0], [5.0, 5.0]]);
        assert_eq!(twice_signed_area(&c), 0.0);
        // a triangle with one corner on x = 5 and the others inside: unchanged
        let t = vec![[5.0, 1.0], [8.0, 0.5], [8.0, 3.0]];
        assert_eq!(clip_convex_to_rect(&t, [5.0, 0.0, 10.0, 10.0]), t);
        // one corner on the line, the other two outside: that corner alone
        let p = clip_half_plane(
            &[[0.0, 0.0], [-1.0, 1.0], [-1.0, -1.0]],
            |v: &[f32; 2]| v[0],
            |a, _, _| *a,
        );
        assert_eq!(p, vec![[0.0, 0.0]]);
    }

    /// The step mixes whatever a vertex carries, at `t` from the first towards the second end, in the side's float
    /// type; a vertex whose side isn't a number is dropped.
    #[test]
    fn a_half_plane_mixes_what_its_vertices_carry() {
        // (position, weight) in f64, cut at x >= 1
        let v = [([0.0f64, 0.0], 0.0f64), ([4.0, 0.0], 1.0), ([0.0, 4.0], 2.0)];
        let mix = |a: &([f64; 2], f64), b: &([f64; 2], f64), t: f64| {
            (
                [a.0[0] + (b.0[0] - a.0[0]) * t, a.0[1] + (b.0[1] - a.0[1]) * t],
                a.1 + (b.1 - a.1) * t,
            )
        };
        let p = clip_half_plane(&v, |v| v.0[0] - 1.0, mix);
        assert_eq!(p, vec![([1.0, 0.0], 0.25), ([4.0, 0.0], 1.0), ([1.0, 3.0], 1.75)]);
        let nan = clip_half_plane(
            &[[0.0f32, 0.0], [f32::NAN, 1.0], [1.0, 1.0]],
            |v| v[0],
            |a, _, _| *a,
        );
        assert_eq!(nan, vec![[0.0, 0.0], [1.0, 1.0]]);
    }
}
