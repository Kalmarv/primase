use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_fromsoft::layer_bake::grouping::{cluster, split};
use std::collections::BTreeMap;

#[test]
fn levels_are_quantized_and_small_groups_merge() {
    let features = [[0.; 8], [0.5; 8], [1.; 8]];
    let groups = cluster(&features, &[1., 1., 1.]).unwrap();
    assert_eq!(groups.centers, features);
    assert_eq!(groups.triangles, [0, 1, 2]);
    let groups = cluster(&features, &[100., 100., 0.01]).unwrap();
    assert_eq!(groups.centers, [[0.; 8], [0.5; 8]]);
    assert_eq!(groups.triangles, [0, 1, 1]);
    assert_eq!(cluster(&[[0.1; 8]], &[1.]).unwrap().centers, [[6. / 64.; 8]]);
    assert!(cluster(&[], &[]).is_err());
    assert!(cluster(&features, &[1.]).is_err());
    assert!(cluster(&features, &[1., -1., 1.]).is_err());
    assert!(cluster(&[[f32::NAN; 8]], &[1.]).is_err());
    assert!(cluster(&[[f32::MAX; 8]], &[1.]).is_err());
}

#[test]
fn splitting_preserves_triangle_order_and_vertex_payloads() {
    let primitive = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.; 3], [1.; 3], [2.; 3], [3.; 3]]),
            ),
            (Semantic::Tangent, Stream::F32x4(vec![[1., 0., 0., -1.]; 4])),
            (
                Semantic::Color(0),
                Stream::U8x4(vec![[0; 4], [1; 4], [2; 4], [3; 4]]),
            ),
        ]),
        indices: Some(vec![2, 0, 1, 3, 2, 1, 0, 3, 2]),
        material: None,
    };
    let parts = split(&primitive, &[1, 0, 1], 3).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].0, 0);
    assert_eq!(parts[0].1.indices.as_deref(), Some([0, 1, 2].as_slice()));
    assert_eq!(parts[1].1.indices.as_deref(), Some([0, 1, 2, 1, 3, 0].as_slice()));
    assert_eq!(
        parts[1].1.streams[&Semantic::Color(0)],
        Stream::U8x4(vec![[2; 4], [0; 4], [1; 4], [3; 4]])
    );
    // each part's vertex map: part vertex -> source vertex
    assert_eq!(parts[0].2, [3, 2, 1]);
    assert_eq!(parts[1].2, [2, 0, 1, 3]);
    for (_, p, _) in &parts {
        assert_eq!(
            p.streams[&Semantic::Tangent],
            Stream::F32x4(vec![[1., 0., 0., -1.]; p.positions().unwrap().len()])
        );
    }
    assert!(split(&primitive, &[0], 1).is_err());
    assert!(split(&primitive, &[0, 0, 1], 1).is_err());
}
