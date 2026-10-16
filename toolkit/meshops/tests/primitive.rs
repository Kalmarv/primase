use primase_core::{Primitive, Semantic, Stream, Topology};
use primase_meshops::{Mesh, compact, compact_primitive};
use std::collections::BTreeMap;
fn fixture() -> Primitive {
    Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.; 3], [1.; 3], [2.; 3], [3.; 3]]),
            ),
            (Semantic::Normal, Stream::F32x3(vec![[0., 1., 0.]; 4])),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.; 2], [1.; 2], [2.; 2], [3.; 2]]),
            ),
            (
                Semantic::Tangent,
                Stream::F32x4(vec![
                    [1., 0., 0., -1.],
                    [0., 1., 0., 1.],
                    [0., 0., 1., -1.],
                    [1., 1., 0., 1.],
                ]),
            ),
            (
                Semantic::Color(3),
                Stream::U8x4(vec![[0; 4], [1; 4], [2; 4], [3; 4]]),
            ),
            (
                Semantic::Joints(2),
                Stream::U16x4(vec![[1000; 4], [2000; 4], [3000; 4], [4000; 4]]),
            ),
            (Semantic::Weights(2), Stream::F32x4(vec![[0.25; 4]; 4])),
        ]),
        indices: Some(vec![2, 0, 2, 0, 3, 2]),
        material: Some(primase_core::world::Entity::from_index(7)),
    }
}
#[test]
fn all_streams_and_remap_survive() {
    let p = fixture();
    let c = compact_primitive(&p).unwrap();
    assert_eq!(c.gather, [2, 0, 3]);
    assert_eq!(c.old_to_new, [Some(1), None, Some(0), Some(2)]);
    assert_eq!(c.primitive.indices, Some(vec![0, 1, 0, 1, 2, 0]));
    assert_eq!(c.primitive.material, p.material);
    assert_eq!(c.primitive.topology, p.topology);
    for (new, &old) in c.gather.iter().enumerate() {
        assert_eq!(c.old_to_new[old as usize], Some(new as u32));
    }
    assert_eq!(c.primitive.streams.len(), p.streams.len());
    for (key, value) in &c.primitive.streams {
        macro_rules! check {
            ($v:expr,$kind:ident) => {
                if let Stream::$kind(original) = &p.streams[key] {
                    assert_eq!($v, &vec![original[2], original[0], original[3]]);
                } else {
                    panic!("changed representation")
                }
            };
        }
        match value {
            Stream::F32x2(v) => check!(v, F32x2),
            Stream::F32x3(v) => check!(v, F32x3),
            Stream::F32x4(v) => check!(v, F32x4),
            Stream::U8x4(v) => check!(v, U8x4),
            Stream::U16x4(v) => check!(v, U16x4),
        }
    }
}
#[test]
fn legacy_position_normal_uv_output_agrees() {
    let p = fixture();
    let c = compact_primitive(&p).unwrap();
    let Stream::F32x3(normals) = &p.streams[&Semantic::Normal] else {
        panic!()
    };
    let Stream::F32x2(uv) = &p.streams[&Semantic::TexCoord(0)] else {
        panic!()
    };
    let old = compact(&Mesh {
        positions: p.positions().unwrap().to_vec(),
        normals: normals.clone(),
        uv: uv.clone(),
        indices: p.indices.unwrap(),
    })
    .unwrap();
    assert_eq!(c.primitive.positions().unwrap(), old.positions);
    assert_eq!(c.primitive.streams[&Semantic::Normal], Stream::F32x3(old.normals));
    assert_eq!(c.primitive.streams[&Semantic::TexCoord(0)], Stream::F32x2(old.uv));
    assert_eq!(c.primitive.indices, Some(old.indices));
}
#[test]
fn nonindexed_and_empty_primitives_preserve_index_contract() {
    let mut p = fixture();
    p.topology = Topology::Points;
    p.indices = None;
    let c = compact_primitive(&p).unwrap();
    assert_eq!(c.primitive, p);
    assert_eq!(c.gather, [0, 1, 2, 3]);
    p.indices = Some(vec![]);
    let c = compact_primitive(&p).unwrap();
    assert!(c.gather.is_empty());
    assert_eq!(c.old_to_new, [None; 4]);
    assert!(c.primitive.streams.values().all(Stream::is_empty));
}
#[test]
fn malformed_streams_and_indices_fail() {
    let mut p = fixture();
    p.indices = Some(vec![0, 1, 4]);
    assert!(compact_primitive(&p).is_err());
    p.indices = Some(vec![0, 1]);
    assert!(compact_primitive(&p).is_err());
    p = fixture();
    p.streams.insert(Semantic::Color(0), Stream::U8x4(vec![[0; 4]]));
    assert!(compact_primitive(&p).is_err());
    p = fixture();
    p.streams
        .insert(Semantic::Tangent, Stream::F32x4(vec![[f32::NAN; 4]; 4]));
    assert!(compact_primitive(&p).is_err());
}

/// The original web tiler gathered these exact source vertices in first-use order.
#[test]
fn explicit_subset_preserves_the_web_gather_order_and_every_stream() {
    let source = fixture();
    let selected = primase_meshops::subset_primitive(&source, &[3, 1, 3, 1, 2, 3]).unwrap();
    assert_eq!(selected.indices, Some(vec![0, 1, 0, 1, 2, 0]));
    let mut reference = source.clone();
    reference.indices = Some(vec![3, 1, 3, 1, 2, 3]);
    assert_eq!(selected, compact_primitive(&reference).unwrap().primitive);
    assert_eq!(source, fixture(), "source is unchanged");
    let empty = primase_meshops::subset_primitive(&source, &[]).unwrap();
    assert!(empty.streams.values().all(|s| s.is_empty()));
    assert_eq!(empty.indices, Some(vec![]));
    assert!(primase_meshops::subset_primitive(&source, &[0, 1]).is_err());
    assert!(primase_meshops::subset_primitive(&source, &[0, 1, 4]).is_err());
    let mut unindexed = source;
    unindexed.indices = None;
    assert_eq!(
        primase_meshops::subset_primitive(&unindexed, &[3, 1, 3, 1, 2, 3]).unwrap(),
        selected
    );
}

#[test]
fn sparse_subset_checks_only_selected_vertices() {
    let mut positions = vec![[f32::NAN; 3]; 100_000];
    positions[99_999] = [1., 2., 3.];
    let source = Primitive {
        topology: Topology::Points,
        streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(positions))]),
        indices: None,
        material: None,
    };
    let out = primase_meshops::subset_primitive(&source, &[99_999, 99_999]).unwrap();
    assert_eq!(out.positions().unwrap(), &[[1., 2., 3.]]);
    assert_eq!(out.indices, Some(vec![0, 0]));
    assert!(compact_primitive(&source).is_err());
    assert!(primase_meshops::subset_primitive(&source, &[0]).is_err());
}
