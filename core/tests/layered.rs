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
        weights: LayerWeights {
            attribute: Semantic::Color(1),
            mapping: WeightMapping::RgbProduct4,
        },
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
