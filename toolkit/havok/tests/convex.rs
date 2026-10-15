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
