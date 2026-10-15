use primase_meshops::{closest_point_on_triangle as closest, point_triangle_distance_squared as distance};
const T: [[f64; 3]; 3] = [[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]];
#[test]
fn each_voronoi_region_and_reversed_winding() {
    for (p, want) in [
        ([-1., -1., 1.], [0., 0., 0.]),
        ([3., -1., 1.], [2., 0., 0.]),
        ([-1., 3., 1.], [0., 2., 0.]),
        ([1., -1., 1.], [1., 0., 0.]),
        ([-1., 1., 1.], [0., 1., 0.]),
        ([2., 2., 1.], [1., 1., 0.]),
        ([0.5, 0.5, 1.], [0.5, 0.5, 0.]),
    ] {
        assert_eq!(closest(p, T), want);
        assert_eq!(closest(p, [T[2], T[1], T[0]]), want);
    }
    assert_eq!(distance([0.5, 0.5, 2.], T), 4.);
}
#[test]
fn degenerate_point_segment_and_thin_triangle() {
    assert_eq!(closest([1., 2., 3.], [[0.; 3]; 3]), [0.; 3]);
    assert_eq!(
        closest([1., 2., 0.], [[0.; 3], [2., 0., 0.], [1., 0., 0.]]),
        [1., 0., 0.]
    );
    assert_eq!(
        closest([0.5, 0.25e-20, 1.], [[0.; 3], [1., 0., 0.], [0., 1e-20, 0.]]),
        [0.5, 0.25e-20, 0.]
    );
}
#[test]
fn analytic_minimum_bounds_brute_force_barycentric_grid() {
    let triangles = [
        T,
        [[1., 2., 3.], [-2., 1., 4.], [0., -1., 2.]],
        [[0.; 3], [1., 2., 3.], [2., 4., 6.]],
    ];
    for t in triangles {
        for p in [[0.4, 0.7, 2.], [-3., 2., 1.], [4., -2., 1.], [0., 0., 0.]] {
            let exact = distance(p, t);
            let mut sampled = f64::INFINITY;
            for i in 0..=100 {
                for j in 0..=100 - i {
                    let u = i as f64 / 100.;
                    let v = j as f64 / 100.;
                    let q: [f64; 3] =
                        std::array::from_fn(|k| t[0][k] * (1. - u - v) + t[1][k] * u + t[2][k] * v);
                    let d = p.into_iter().zip(q).map(|(a, b)| (a - b).powi(2)).sum::<f64>();
                    sampled = sampled.min(d);
                }
            }
            assert!(exact <= sampled + 1e-12, "{exact} vs {sampled}");
            assert!(sampled - exact < 0.01, "grid approaches analytic minimum");
        }
    }
}
