use primase_core::{LayerWeights, LayeredMaterial, Primitive, Semantic, Stream, Topology, WeightMapping};
use std::collections::BTreeMap;

#[test]
fn varying_red_intentionally_changes_the_triangle_choice() {
    let channels = vec![[1., 0., 0., 1.], [1., 0., 0., 1.], [0., 1., 1., 1.]];
    let material = LayeredMaterial {
        layers: vec![None; 4],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
    };
    let full_sum = channels.iter().fold([0.; 4], |sum, &v| {
        let w = material.weights.mapping.evaluate(v);
        std::array::from_fn(|i| sum[i] + w[i])
    });
    assert_eq!(full_sum, [0., 0., 0., 1.]);
    let primitive = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
            ),
            (Semantic::Color(1), Stream::F32x4(channels)),
        ]),
        indices: None,
        material: None,
    };
    assert_eq!(
        primase_meshops::strongest_layer_parts(
            &primitive,
            &material,
            primase_meshops::SelectionPolicy::CoreWeights
        )
        .unwrap()[0]
            .0,
        3
    );
    // The two dark corners still vote for slot zero in this approximation.
    assert_eq!(
        primase_meshops::strongest_layer_parts(
            &primitive,
            &material,
            primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening
        )
        .unwrap()[0]
            .0,
        0
    );
}

#[test]
fn triangle_selection_omits_darkening_preserves_streams_and_counts_null_slots() {
    let mut p = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]]),
            ),
            (
                Semantic::Color(1),
                Stream::F32x4(vec![
                    [1., 0., 0., 1.],
                    [1., 0., 0., 1.],
                    [1., 1., 1., 1.],
                    [1., 1., 1., 1.],
                ]),
            ),
            (Semantic::Tangent(0), Stream::F32x4(vec![[1., 0., 0., -1.]; 4])),
        ]),
        indices: Some(vec![0, 1, 2, 1, 2, 3]),
        material: None,
    };
    let material = LayeredMaterial {
        layers: vec![None; 4],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
    };
    let out = primase_meshops::strongest_layer_parts(
        &p,
        &material,
        primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening,
    )
    .unwrap();
    assert_eq!(out.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 3]);
    assert_eq!(out[0].1.indices.as_deref(), Some([0, 1, 2].as_slice()));
    assert_eq!(out[1].1.indices.as_deref(), Some([1, 2, 3].as_slice()));
    for (_, part) in &out {
        for (key, value) in &p.streams {
            assert_eq!(&part.streams[key], value);
        }
        assert!(part.streams.contains_key(&Semantic::Normal));
    }
    p.streams.remove(&Semantic::Color(1));
    assert!(
        primase_meshops::strongest_layer_parts(
            &p,
            &material,
            primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening
        )
        .is_err()
    );
}

#[test]
fn identity_vec3_has_alpha_one_and_ties_choose_first() {
    let mut p = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (Semantic::Position, Stream::F32x3(vec![[0., 0., 0.]; 3])),
            (Semantic::Color(1), Stream::F32x3(vec![[0., 0., 0.]; 3])),
        ]),
        indices: None,
        material: None,
    };
    let material = LayeredMaterial {
        layers: vec![None; 4],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
    };
    assert_eq!(
        primase_meshops::strongest_layer_parts(
            &p,
            &material,
            primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening
        )
        .unwrap()[0]
            .0,
        3
    );
    p.streams
        .insert(Semantic::Color(1), Stream::U8x4(vec![[255; 4]; 3]));
    assert_eq!(
        primase_meshops::strongest_layer_parts(
            &p,
            &material,
            primase_meshops::SelectionPolicy::OmitRgbProduct4Darkening
        )
        .unwrap()[0]
            .0,
        0
    );
}

#[test]
fn per_pixel_weights_are_refused_by_the_vertex_weight_lowering() {
    let mut material = LayeredMaterial {
        layers: vec![None; 4],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
    };
    material.weights.composition = primase_core::layered::Composition::Over;
    let primitive = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
            ),
            (Semantic::Color(1), Stream::F32x4(vec![[0.; 4]; 3])),
        ]),
        indices: None,
        material: None,
    };
    let err = primase_meshops::strongest_layer_parts(
        &primitive,
        &material,
        primase_meshops::SelectionPolicy::CoreWeights,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("bake-layer-channels"), "{err}");
}
