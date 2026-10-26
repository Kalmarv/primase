use primase_core::layered::{Channel, Composition, Source, mask_value, over_as_sum};
use primase_core::pbr::TextureRef;
use primase_core::{Component, Layer, LayerWeights, LayeredMaterial, Semantic, WeightMapping, World};

#[test]
fn pixel_mapping_keeps_darkening_and_interpolates_channels_before_products() {
    let map = WeightMapping::RgbProduct4;
    assert_eq!(map.evaluate([0.5, 0.5, 0.5, 1.]), [0.125; 4]);
    assert_eq!(map.evaluate([1., 0.5, 0.5, 0.]), [0.; 4]);
    let ends = [map.evaluate([0., 0., 0., 1.]), map.evaluate([0., 1., 1., 1.])];
    let interpolated_products: [f32; 4] = std::array::from_fn(|i| (ends[0][i] + ends[1][i]) / 2.);
    assert_ne!(map.evaluate([0., 0.5, 0.5, 1.]), interpolated_products);
    assert_eq!(
        WeightMapping::Identity.evaluate([0.2, 0.3, 0.1, 0.]),
        [0.2, 0.3, 0.1, 0.]
    );
}

#[test]
fn verifies_null_slots_references_and_nonrecursive_materials() {
    let mut world = World::default();
    let ordinary = world.spawn();
    let entity = world.spawn();
    let mut material = LayeredMaterial {
        layers: vec![
            Some(Layer {
                material: ordinary,
                texcoord: Some(1),
                transform: None,
            }),
            None,
            None,
            None,
        ],
        weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
    };
    assert!(material.verify(entity, &world).is_ok());
    world.insert(entity, material.clone());
    assert!(world.closure([entity]).contains(&ordinary));
    world.insert(ordinary, material.clone());
    assert!(material.verify(entity, &world).is_err());
    world.despawn(ordinary);
    // a layer whose material is gone: the world's reference check, not the material's own
    assert!(material.verify(entity, &world).is_ok());
    assert_eq!(
        world.verify_components(),
        [(
            "core.layered",
            entity,
            format!("refers to {ordinary:?}, which is not alive")
        )]
    );
    material.layers[0] = None;
    assert!(material.verify(entity, &world).is_ok());
    material.layers.pop();
    assert!(material.verify(entity, &world).is_err());
    material.weights.mapping = WeightMapping::Identity;
    assert!(material.verify(entity, &world).is_ok());
    material.layers.clear();
    assert!(material.verify(entity, &world).is_err());
}

/// Worked by hand: Sekiro's threshold k = (0, s, -s, s) with s = 2 at v = 0.5, m = 0.25 is 1 - 0.5 + 0.25 = 0.75;
/// past 1 it saturates; a lerp of 0.2 to 0.6 at 0.25 is 0.3; a mask of rgba (1, 0.5, 0, 0) by (0.5, 0.5, 0, 0) and
/// bias -0.1 is 0.65; over with slot weights (_, 0.5, 0.25) sums as (0.375, 0.375, 0.25).
#[test]
fn channels_evaluate_by_hand() {
    let v = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 0,
    };
    let t = Channel::Threshold {
        v: v.clone(),
        m: v.clone(),
        k: [0., 2., -2., 2.],
    };
    assert_eq!(t.evaluate(&[0.5, 0.25]), 0.75);
    assert_eq!(t.evaluate(&[1.0, 0.0]), 1.0);
    let l = Channel::Lerp {
        a: v.clone(),
        b: v.clone(),
        t: v,
    };
    assert!((l.evaluate(&[0.2, 0.6, 0.25]) - 0.3).abs() < 1e-7);
    assert!((mask_value([0.5, 0.5, 0., 0.], -0.1, [1., 0.5, 0., 0.]) - 0.65).abs() < 1e-7);
    assert_eq!(over_as_sum([0., 0.5, 0.25, 0.], 3), [0.375, 0.375, 0.25, 0.]);
    // no more slots than the four there are
    assert_eq!(over_as_sum([0.5; 4], 9), over_as_sum([0.5; 4], 4));
}

#[test]
fn channels_match_the_mapping_and_their_masks_are_references() {
    let mut world = World::default();
    let (a, b, mask, e) = (world.spawn(), world.spawn(), world.spawn(), world.spawn());
    let layer = |m| {
        Some(Layer {
            material: m,
            texcoord: None,
            transform: None,
        })
    };
    let masked = Channel::Source(Source::Mask {
        texture: TextureRef::new(mask),
        channels: [1., 0., 0., 0.],
        bias: 0.,
    });
    let mut material = LayeredMaterial {
        layers: vec![layer(a), layer(b)],
        weights: LayerWeights {
            channels: Some(vec![masked.clone(), masked.clone()]),
            composition: Composition::Over,
            ..LayerWeights::new(Semantic::Color(1), WeightMapping::Identity)
        },
    };
    assert!(material.weights.per_pixel());
    assert_eq!(material.verify(e, &world), Ok(()));
    world.insert(e, material.clone());
    assert!(
        world.closure([e]).contains(&mask),
        "a mask is part of its material"
    );
    // one channel for two slots
    material.weights.channels = Some(vec![masked.clone()]);
    assert_eq!(
        material.verify(e, &world),
        Err("1 channels for a mapping of 2".into())
    );
    // a vertex channel past alpha, a threshold that is not finite
    let past = Channel::Source(Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 4,
    });
    material.weights.channels = Some(vec![masked.clone(), past]);
    assert_eq!(
        material.verify(e, &world),
        Err("channel 1: vertex channel 4 past 3".into())
    );
    let v = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 0,
    };
    let nan = Channel::Threshold {
        v: v.clone(),
        m: v,
        k: [f32::NAN, 0., 0., 0.],
    };
    material.weights.channels = Some(vec![masked, nan]);
    assert!(material.verify(e, &world).unwrap_err().contains("not finite"));
    // a mask that is gone
    world.despawn(mask);
    assert!(
        world
            .get::<LayeredMaterial>(e)
            .unwrap()
            .verify(e, &world)
            .unwrap_err()
            .contains("not alive")
    );
}

/// The height blend worked by hand from the formula (MAD_Tex2's, riders-republic docs/material-spec.md §4.3): weights
/// (0.5, 0.5), heights (0.8, 0.4), so p = (0.4, 0.2) and m = 0.4.
#[test]
fn height_blend_weights_worked_by_hand() {
    use primase_core::layered::height_blend;
    let close = |a: [f32; 4], b: [f32; 4]| (0..4).all(|i| (a[i] - b[i]).abs() < 1e-6);
    // contrast 0.5: u = (0.4 + 0.1, 0.2 + 0.1) = (0.5, 0.3), normalized (0.625, 0.375)
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 0.5);
    assert!(close(got, [0.625, 0.375, 0.0, 0.0]), "{got:?}");
    // contrast 0.9: u = (0.4 - 0.3, max(0.2 - 0.3, 0)) = (0.1, 0): the lower layer drops out
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 0.9);
    assert!(close(got, [1.0, 0.0, 0.0, 0.0]), "{got:?}");
    // contrast 1: every u is 0, no weight
    let got = height_blend([0.5, 0.5, 0.0, 0.0], [0.8, 0.4, 0.0, 0.0], 2, 1.0);
    assert!(close(got, [0.0; 4]), "{got:?}");
}

/// Hand-worked: vertex weight 1/2 times mask 1/4 times sampled alpha 3/4 is 3/32. Products retain signs and values
/// outside 0..1; each factor, including the last, participates in reference and source validation.
#[test]
fn products_are_bounded_ordered_and_keep_all_sources() {
    use primase_core::layered::PRODUCT_LIMIT;
    let mut w = World::new();
    let material = w.spawn();
    let host = w.spawn();
    let mask = w.spawn();
    let vertex = Source::Vertex {
        semantic: Semantic::Color(0),
        channel: 1,
    };
    let texture = Source::Mask {
        texture: TextureRef::new(mask),
        channels: [0., 0., 0., 1.],
        bias: 0.,
    };
    let channel = Channel::Product {
        sources: vec![vertex.clone(), texture.clone(), texture.clone()],
    };
    assert_eq!(channel.evaluate(&[0.5, 0.25, 0.75]), 0.09375);
    assert_eq!(channel.evaluate(&[-2., 3., 0.5]), -3.);
    assert_eq!(channel.sources(), vec![&vertex, &texture, &texture]);
    for n in [0, 1, PRODUCT_LIMIT + 1] {
        assert!(
            Channel::Product {
                sources: vec![vertex.clone(); n]
            }
            .check()
            .unwrap_err()
            .contains("product")
        );
    }
    assert!(
        Channel::Product {
            sources: vec![vertex.clone(); PRODUCT_LIMIT]
        }
        .check()
        .is_ok()
    );
    let mut layered = LayeredMaterial {
        layers: vec![Some(Layer {
            material,
            texcoord: None,
            transform: None,
        })],
        weights: LayerWeights {
            channels: Some(vec![channel]),
            ..LayerWeights::new(Semantic::Color(0), WeightMapping::Identity)
        },
    };
    w.insert(host, layered.clone());
    assert!(w.closure([host]).contains(&mask));
    assert!(layered.reads(Semantic::Color(0)));
    assert_eq!(layered.mask_textures().count(), 2);
    assert!(layered.verify(host, &w).is_ok());
    if let Some(channels) = &mut layered.weights.channels {
        channels[0] = Channel::Product {
            sources: vec![
                vertex.clone(),
                texture,
                Source::Vertex {
                    semantic: Semantic::Color(0),
                    channel: 4,
                },
            ],
        };
    }
    assert!(layered.verify(host, &w).unwrap_err().contains("past 3"));
    // The order is specified even when reassociating floating-point multiplication would change the result.
    let three = Channel::Product {
        sources: vec![vertex; 3],
    };
    assert_eq!(three.evaluate(&[1e20, 1e20, 1e-20]), f32::INFINITY);
}
