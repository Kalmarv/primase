use primase_core::Transform;
use primase_fromsoft::{
    btl::{Kind, Light},
    sekiro::lights::SrgbLightPolicy,
};
fn light() -> Light {
    Light {
        name: "fixture".into(),
        kind: Kind::Spot,
        diffuse: [0, 128, 255],
        diffuse_power: 2.,
        specular: [0; 3],
        specular_power: 0.,
        cast_shadows: true,
        cone_angle: 30.,
        position: [1., 2., 3.],
        rotation: [0.; 3],
        radius: 5.,
        auxiliary: Default::default(),
    }
}
#[test]
fn named_srgb_energy_and_axis_policy() {
    let identity = Transform {
        cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]],
    };
    let l = SrgbLightPolicy::default().convert(&light(), &identity).unwrap();
    assert_eq!(l.position, [1., 2., -3.]);
    assert_eq!(l.direction, [0., 0., -1.]);
    assert_eq!(l.tangent, [1., 0., 0.]);
    assert_eq!(l.energy, 200.);
    assert_eq!(l.range, 5.);
    assert!(l.cast_shadow);
    assert_eq!(l.color[0], 0.);
    assert_eq!(l.color[2], 1.);
    assert!((l.color[1] - 0.2158605).abs() < 1e-7);
    let reference = (128f64 / 255.).powf(2.2);
    assert!((f64::from(l.color[1]) - reference).abs() > 0.003);
    let mut l = light();
    l.rotation = [0., 90., 0.];
    let out = SrgbLightPolicy::default().convert(&l, &identity).unwrap();
    assert!((out.direction[0] - 1.).abs() < 1e-6);
    assert!(out.direction[2].abs() < 1e-6);
    l.kind = Kind::Point;
    let point = SrgbLightPolicy::default().convert(&l, &identity).unwrap();
    assert_eq!(point.spot_size, 0.);
    assert_eq!(point.spot_blend, 0.);
    assert_eq!(point.time_of_day, 127);
    assert!(
        SrgbLightPolicy {
            watts_per_power: f32::NAN
        }
        .convert(&l, &identity)
        .is_err()
    );
    l.kind = Kind::Directional;
    assert!(SrgbLightPolicy::default().convert(&l, &identity).is_err());
}

#[test]
fn ledge_join_uses_distance_across_cells_not_cell_identity() {
    use primase_fromsoft::sekiro::ledges::{GameSurfaceIndex, chain};
    let surface = GameSurfaceIndex::new(&[], &primase_core::Surfaces::default()).unwrap();
    let joined = chain(
        &[[[-1., 0., 0.], [0.009, 0., 0.]], [[0.011, 0., 0.], [1., 0., 0.]]],
        0.,
        |_| true,
        &surface,
    )
    .unwrap();
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].points, [[-1., 0., 0.], [0.009, 0., 0.], [1., 0., 0.]]);
    let separate = chain(
        &[
            [[-1., 0., 0.], [0.001, 0.001, 0.001]],
            [[0.009, 0.009, 0.009], [1., 0., 0.]],
        ],
        0.,
        |_| true,
        &surface,
    )
    .unwrap();
    assert_eq!(separate.len(), 2);
}
#[test]
fn spawn_region_values_are_read_without_map_offset_or_rounding() {
    use primase_fromsoft::msb::{MapLayout, Section};
    let mut bytes = vec![0u8; 100];
    bytes[..8].copy_from_slice(&96u64.to_le_bytes());
    bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
    bytes[96..98].copy_from_slice(&65u16.to_le_bytes());
    for (i, v) in [1f32, 2., 3., 4., 5., 6.].into_iter().enumerate() {
        bytes[20 + i * 4..24 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    let layout = MapLayout {
        models: vec![],
        parts: vec![],
        map_offsets: vec![],
        sections: vec![Section {
            name: "POINT_PARAM_ST".into(),
            version: 35,
            records: vec![bytes],
        }],
    };
    let p = layout.spawn_points().unwrap();
    assert_eq!(p[0].name, "A");
    assert_eq!(p[0].position, [1., 2., 3.]);
    assert_eq!(p[0].rotation_degrees, [4., 5., 6.]);
}
#[test]
fn ledges_chain_closed_and_open_segments_and_use_only_supplied_map_surfaces() {
    use primase_core::{Collision, Surfaces};
    use primase_fromsoft::sekiro::ledges::{GameSurfaceIndex, chain};
    let mut surfaces = Surfaces::default();
    let wood = surfaces.index("wood").unwrap();
    let map = Collision {
        positions: vec![[-5., 0., -5.], [5., 0., -5.], [0., 0., 5.]],
        triangles: vec![[0, 1, 2]],
        surfaces: vec![wood],
    };
    let surface = GameSurfaceIndex::new(&[&map], &surfaces).unwrap();
    let curves = chain(
        &[
            [[0., 1., 0.], [1., 1., 0.]],
            [[1., 1., 0.], [1., 1., 1.]],
            [[1., 1., 1.], [0., 1., 0.]],
        ],
        1.,
        |_| true,
        &surface,
    )
    .unwrap();
    assert_eq!(curves.len(), 1);
    assert!(curves[0].closed);
    assert_eq!(curves[0].points.len(), 3);
    assert_eq!(curves[0].surface, 4);
    let empty = GameSurfaceIndex::new(&[], &primase_core::Surfaces::default()).unwrap();
    assert_eq!(
        chain(&[[[0., 0., 0.], [2., 0., 0.]]], 1., |_| true, &empty).unwrap()[0].surface,
        3
    );
    assert!(
        chain(&[[[0., 0., 0.], [0.5, 0., 0.]]], 1., |_| true, &empty)
            .unwrap()
            .is_empty()
    );
    assert!(
        chain(&[[[0., 0., 0.], [2., 0., 0.]]], 1., |_| false, &empty)
            .unwrap()
            .is_empty()
    );
}
