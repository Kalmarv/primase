//! Closest points on closed triangles, with explicit segment/point degeneracy.
type Point = [f64; 3];
fn sub(a: Point, b: Point) -> Point {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: Point, b: Point) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn squared(a: Point, b: Point) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}
fn segment(p: Point, a: Point, b: Point) -> Point {
    let ab = sub(b, a);
    let d = dot(ab, ab);
    let t = if d == 0. {
        0.
    } else {
        (dot(sub(p, a), ab) / d).clamp(0., 1.)
    };
    std::array::from_fn(|i| a[i] + t * ab[i])
}
/// Closest point on a triangle, in f64. Finite inputs whose intermediate products
/// are representable are required. All finite f32 coordinates widened to f64 fit
/// this range. Collinear triangles use their three closed segments; a collapsed
/// triangle is a point. Equal-distance candidates keep edge order AB, BC, CA.
pub fn closest_point_on_triangle(p: Point, triangle: [Point; 3]) -> Point {
    let [a, b, c] = triangle;
    let mut best = segment(p, a, b);
    let mut distance = squared(p, best);
    for (a, b) in [(b, c), (c, a)] {
        let q = segment(p, a, b);
        let d = squared(p, q);
        if d < distance {
            best = q;
            distance = d;
        }
    }
    let n = cross(sub(b, a), sub(c, a));
    let nn = dot(n, n);
    if nn != 0. {
        let signed = dot(sub(p, a), n);
        let q = std::array::from_fn(|i| p[i] - n[i] * signed / nn);
        if [(a, b), (b, c), (c, a)]
            .into_iter()
            .all(|(a, b)| dot(cross(sub(b, a), sub(q, a)), n) >= 0.)
        {
            best = q;
        }
    }
    best
}
/// Squared Euclidean distance to the closed triangle; see
/// [`closest_point_on_triangle`] for precision and degenerate input semantics.
pub fn point_triangle_distance_squared(p: Point, triangle: [Point; 3]) -> f64 {
    squared(p, closest_point_on_triangle(p, triangle))
}
