use primase_havok::ConvexCore;
fn cube(radius: f32) -> ConvexCore {
    ConvexCore {
        vertices: vec![
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., 1., -1.],
            [-1., 1., -1.],
            [-1., -1., 1.],
            [1., -1., 1.],
            [1., 1., 1.],
            [-1., 1., 1.],
        ],
        planes: vec![
            [0., 0., -1., -1.],
            [0., 0., 1., -1.],
            [0., -1., 0., -1.],
            [1., 0., 0., -1.],
            [0., 1., 0., -1.],
            [-1., 0., 0., -1.],
        ],
        faces: vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ],
        radius,
    }
}
fn check_planes(core: &ConvexCore, triangles: &[[[f32; 3]; 3]]) {
    for p in triangles.iter().flatten() {
        let mut incident = 0;
        for plane in &core.planes {
            let residual = (0..3).map(|i| f64::from(plane[i]) * f64::from(p[i])).sum::<f64>()
                + f64::from(plane[3])
                - f64::from(core.radius);
            assert!(residual <= 1e-5, "outside {residual}");
            incident += usize::from(residual.abs() <= 1e-5);
        }
        assert!(incident >= 3);
    }
}
#[test]
fn offset_planes_expand_box_with_outward_faces() {
    let c = cube(0.25);
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 12);
    check_planes(&c, &t);
    for tri in &t {
        assert!(tri.iter().flatten().all(|v| v.abs() == 1.25));
        let p = tri[0];
        let a = tri[0];
        let b = tri[1];
        let d = tri[2];
        let u = std::array::from_fn::<_, 3, _>(|k| b[k] - a[k]);
        let v = std::array::from_fn::<_, 3, _>(|k| d[k] - a[k]);
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        assert!(cross.iter().zip(p).map(|(a, b)| a * b).sum::<f32>() > 0.);
    }
    assert_eq!(t, c.expanded_triangles().unwrap());
}
#[test]
fn zero_radius_retains_exact_cube_corners() {
    let c = cube(0.);
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 12);
    check_planes(&c, &t);
    assert!(t.iter().flatten().all(|v| c.vertices.contains(v)));
}
#[test]
fn invalid_and_unbounded_geometry_is_rejected() {
    let mut c = cube(0.1);
    c.faces[0][0] = 99;
    assert!(c.expanded_triangles().is_err());
    let mut c = cube(0.1);
    c.planes.pop();
    assert!(c.expanded_triangles().is_err());
    let mut c = cube(-0.1);
    assert!(c.expanded_triangles().is_err());
    c.radius = f32::NAN;
    assert!(c.expanded_triangles().is_err());
    let mut c = cube(0.1);
    c.planes.remove(0);
    c.faces.remove(0);
    assert!(c.expanded_triangles().is_err());
}
#[test]
fn offset_nonsimple_corner_splits_into_extra_vertices() {
    let mut c = ConvexCore {
        vertices: vec![
            [-1., -0.5, 0.],
            [1., -0.5, 0.],
            [1., 1., 0.],
            [-1., 1., 0.],
            [0., 0., 1.],
        ],
        planes: vec![
            [1., 0., 1., -1.],
            [-1., 0., 1., -1.],
            [0., 1., 1., -1.],
            [0., -2., 1., -1.],
            [0., 0., -1., 0.],
        ],
        faces: vec![
            vec![1, 2, 4],
            vec![3, 0, 4],
            vec![2, 3, 4],
            vec![0, 1, 4],
            vec![3, 2, 1, 0],
        ],
        radius: 0.2,
    };
    for p in &mut c.planes {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        for x in p {
            *x /= len;
        }
    }
    let t = c.expanded_triangles().unwrap();
    check_planes(&c, &t);
    let mut corners = Vec::new();
    for p in t.iter().flatten() {
        if !corners.contains(p) {
            corners.push(*p);
        }
    }
    assert!(corners.len() > c.vertices.len(), "offset apex must split");
}
/// Box faces in the order Havok stores them for a box: vertex i has x from bit 0, z from bit 1, y from bit 2.
const BOX_FACES: [[usize; 4]; 6] = [
    [2, 6, 4, 0],
    [1, 5, 7, 3],
    [1, 0, 4, 5],
    [7, 6, 2, 3],
    [3, 2, 0, 1],
    [7, 5, 4, 6],
];
fn normal_of(t: &[[f32; 3]; 3]) -> [f32; 3] {
    let u: [f32; 3] = std::array::from_fn(|k| t[1][k] - t[0][k]);
    let v: [f32; 3] = std::array::from_fn(|k| t[2][k] - t[0][k]);
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}
#[test]
fn zero_thickness_box_is_a_two_sided_rectangle() {
    // A 2 x 2 box of height 0 at y = -0.5: the top and bottom planes coincide (n.v + d = 0 on both).
    let vertices = (0..8)
        .map(|i| {
            [
                if i & 1 == 0 { 1. } else { -1. },
                -0.5,
                if i & 2 == 0 { 1. } else { -1. },
            ]
        })
        .collect();
    let c = ConvexCore {
        vertices,
        planes: vec![
            [1., 0., 0., -1.],
            [-1., 0., 0., -1.],
            [0., 0., 1., -1.],
            [0., 0., -1., -1.],
            [0., -1., 0., -0.5],
            [0., 1., 0., 0.5],
        ],
        faces: BOX_FACES.iter().map(|f| f.to_vec()).collect(),
        radius: 0.,
    };
    let t = c.expanded_triangles().unwrap();
    // Two triangles facing down and two facing up, every corner at (+-1, -0.5, +-1).
    assert_eq!(t.len(), 4);
    assert!(
        t.iter()
            .flatten()
            .all(|v| v[0].abs() == 1. && v[1] == -0.5 && v[2].abs() == 1.)
    );
    let down = t.iter().filter(|t| normal_of(t)[1] < 0.).count();
    let up = t.iter().filter(|t| normal_of(t)[1] > 0.).count();
    assert_eq!((down, up), (2, 2));
    for tri in &t {
        let n = normal_of(tri);
        assert_eq!((n[0], n[2]), (0., 0.));
    }
}
#[test]
fn flat_polygon_offsets_each_face_by_the_radius_once() {
    // A unit square in y = 0 stored as a polytope with its bottom face (and plane) repeated, radius 0.25.
    let c = ConvexCore {
        vertices: vec![[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
        planes: vec![
            [0., -1., 0., 0.],
            [0., 1., 0., 0.],
            [0., -1., 0., 0.],
            [0., -1., 0., 0.],
        ],
        faces: vec![
            vec![0, 1, 2, 3],
            vec![3, 2, 1, 0],
            vec![0, 1, 2, 3],
            vec![0, 1, 2, 3],
        ],
        radius: 0.25,
    };
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 4);
    for tri in &t {
        let n = normal_of(tri);
        // The bottom face sits at y = -0.25 facing down, the top at y = 0.25 facing up.
        assert!(tri.iter().all(|v| v[1] == 0.25 * n[1].signum()), "{tri:?}");
        assert!(
            tri.iter()
                .all(|v| (v[0] == 0. || v[0] == 1.) && (v[2] == 0. || v[2] == 1.))
        );
    }
    // A flat core with a vertex off its plane is refused.
    let mut off = c;
    off.vertices[2][1] = 0.1;
    assert!(off.expanded_triangles().is_err());
}
#[test]
fn thin_box_far_from_its_origin_closes() {
    // A 2.7 m x 1.6 mm x 10.4 m box turned 15.6 degrees about y, 300 m from its body's origin, radius 0.8 mm: its
    // f32 corners are coarser than 0.01 mm, so every check uses the same four-ULP bound.
    let (s, co) = 15.6f64.to_radians().sin_cos();
    let centre = [-209., -24.7, -293.5];
    let half = [1.35, 0.0008, 5.2];
    let axes = [[co, 0., -s], [0., 1., 0.], [s, 0., co]];
    let vertices = (0..8)
        .map(|i| {
            // Vertices 0-3 are the bottom (-y) face.
            let sign = [
                1. - 2. * (i & 1) as f64,
                2. * ((i >> 2) & 1) as f64 - 1.,
                1. - 2. * ((i >> 1) & 1) as f64,
            ];
            std::array::from_fn(|k| {
                (centre[k] + (0..3).map(|a| sign[a] * half[a] * axes[a][k]).sum::<f64>()) as f32
            })
        })
        .collect();
    let mut planes = Vec::new();
    for a in 0..3 {
        for sign in [1., -1.] {
            let n: [f64; 3] = axes[a].map(|x| sign * x);
            let d = -(0..3).map(|k| n[k] * centre[k]).sum::<f64>() - half[a];
            planes.push([n[0] as f32, n[1] as f32, n[2] as f32, d as f32]);
        }
    }
    // Planes in BOX_FACES order: +x, -x, +z, -z, -y, +y.
    let planes = vec![planes[0], planes[1], planes[4], planes[5], planes[3], planes[2]];
    let c = ConvexCore {
        vertices,
        planes,
        faces: BOX_FACES.iter().map(|f| f.to_vec()).collect(),
        radius: 0.0008,
    };
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 12);
}
#[test]
fn a_slab_thinner_than_the_plane_bound_is_a_polygon() {
    // A unit square in y = 0 whose stored top and bottom planes are 0.86 µm apart (an asset stores one so), with a side
    // plane whose face is three collinear corners, and its top plane repeated: a polygon, not an unbounded volume.
    let c = ConvexCore {
        vertices: vec![[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
        planes: vec![
            [0., 1., 0., 0.],
            [0., -1., 0., -8.6e-7],
            [1., 0., 0., -1.],
            [0., 1., 0., 0.],
        ],
        faces: vec![
            vec![3, 2, 1, 0],
            vec![0, 1, 2, 3],
            vec![1, 2, 1],
            vec![3, 2, 1, 0],
        ],
        radius: 0.,
    };
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 4);
    assert!(t.iter().flatten().all(|v| v[1] == 0.));
    // offsets further apart than the bound are a volume: three distinct planes bound none
    let mut thick = c;
    thick.planes[1][3] = -2e-5;
    assert!(thick.expanded_triangles().is_err());
}
#[test]
fn a_sphere_is_the_cube_of_its_radius_around_its_centre() {
    // a sphere of radius 0.5 centred at (1, 2, 3), as Sekiro stores one: its centre the core's only vertex
    let c = ConvexCore {
        vertices: vec![[1., 2., 3.]],
        planes: Vec::new(),
        faces: Vec::new(),
        radius: 0.5,
    };
    let t = c.expanded_triangles().unwrap();
    assert_eq!(t.len(), 12);
    let centre = [1f32, 2., 3.];
    for tri in &t {
        // every corner on the cube, every triangle facing away from the centre
        assert!(
            tri.iter()
                .all(|v| (0..3).all(|k| (v[k] - centre[k]).abs() == 0.5))
        );
        let n = normal_of(tri);
        let out: f32 = (0..3).map(|k| n[k] * (tri[0][k] - centre[k])).sum();
        assert!(out > 0., "{tri:?}");
    }
    // each of the six faces twice
    let mut faces = std::collections::BTreeMap::<(usize, bool), usize>::new();
    for tri in &t {
        let n = normal_of(tri);
        let axis = (0..3).max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs())).unwrap();
        *faces.entry((axis, n[axis] > 0.)).or_default() += 1;
    }
    assert_eq!(faces.len(), 6);
    assert!(faces.values().all(|&n| n == 2));
    // a point of no radius bounds nothing
    let mut none = c;
    none.radius = 0.;
    assert!(none.expanded_triangles().is_err());
}
