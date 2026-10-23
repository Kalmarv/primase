// Test code: helpers outside #[test] functions unwrap too (clippy.toml allows it only inside them).
#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    Layer, LayerWeights, LayeredMaterial, Mesh, Primitive, Semantic, Stream, Topology, WeightMapping, World,
};
use std::collections::BTreeMap;

#[test]
fn layer_transform_precedes_texture_transform_and_second_run_is_noop() {
    let mut world = World::default();
    let texture = world.spawn();
    let ordinary = world.spawn();
    let slot_transform = UvTransform {
        scale: [3., 4.],
        ..UvTransform::IDENTITY
    };
    world.insert(
        ordinary,
        PbrMaterial {
            base_color_texture: Some(TextureRef {
                transform: Some(slot_transform),
                ..TextureRef::new(texture)
            }),
            ..Default::default()
        },
    );
    let layered = world.spawn();
    world.insert(
        layered,
        LayeredMaterial {
            layers: vec![
                Some(Layer {
                    material: ordinary,
                    texcoord: Some(1),
                    transform: Some(UvTransform {
                        offset: [1., 2.],
                        ..UvTransform::IDENTITY
                    }),
                }),
                None,
            ],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            material: Some(layered),
            indices: Some(vec![0, 1, 2]),
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
                ),
                (Semantic::TexCoord(1), Stream::F32x2(vec![[0.5, 0.5]; 3])),
                (Semantic::Color(1), Stream::F32x4(vec![[1., 0., 0., 0.]; 3])),
            ]),
        }]),
    );
    let mut subset = world.subset(&world.closure([mesh]));
    let unrelated = world.spawn();
    let mut other = world.get::<Mesh>(mesh).unwrap().clone();
    *other.primitive_mut(0).expect("a listed primitive").material_mut() = None;
    other
        .primitive_mut(0)
        .expect("a listed primitive")
        .streams_mut()
        .insert(Semantic::TexCoord(0), Stream::F32x2(vec![[0.; 2]; 3]));
    world.insert(unrelated, other);
    primase_reskate::passes::LowerLayeredPbr
        .run(&mut subset, &mut PassLog::default())
        .unwrap();
    let mut log = PassLog::default();
    primase_reskate::passes::LowerLayeredPbr
        .run(&mut world, &mut log)
        .unwrap();
    assert_eq!(log.counts["triangles selected layer 0"], 1);
    let part = &world.get::<Mesh>(mesh).unwrap().primitives()[0];
    let output = world
        .get::<PbrMaterial>(part.material.unwrap())
        .unwrap()
        .base_color_texture
        .unwrap();
    assert_eq!(output.transform, Some(slot_transform));
    assert_eq!(
        part.streams[&Semantic::TexCoord(output.texcoord)],
        Stream::F32x2(vec![[1.5, 2.5]; 3])
    );
    let subset_part = &subset.get::<Mesh>(mesh).unwrap().primitives()[0];
    let subset_texture = subset
        .get::<PbrMaterial>(subset_part.material.unwrap())
        .unwrap()
        .base_color_texture
        .unwrap();
    assert_ne!(
        output.texcoord, subset_texture.texcoord,
        "free set depends on the world"
    );
    assert_eq!(output.transform, subset_texture.transform);
    assert_eq!(
        part.streams[&Semantic::TexCoord(output.texcoord)],
        subset_part.streams[&Semantic::TexCoord(subset_texture.texcoord)]
    );
    let before = world.entities().count();
    let mut again = PassLog::default();
    primase_reskate::passes::LowerLayeredPbr
        .run(&mut world, &mut again)
        .unwrap();
    assert!(again.counts.is_empty());
    assert_eq!(world.entities().count(), before);
}

#[test]
fn invalid_skate_metadata_refuses_all_changes() {
    let mut world = World::new();
    let layered = world.spawn();
    world.insert(
        layered,
        LayeredMaterial {
            layers: vec![None],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (Semantic::Position, Stream::F32x3(vec![[0.; 3]; 3])),
                (Semantic::Color(1), Stream::F32x4(vec![[1., 0., 0., 0.]; 3])),
            ]),
            indices: None,
            material: Some(layered),
        }]),
    );
    world.insert(
        mesh,
        primase_reskate::components::SkatePrototype {
            id: "invalid".into(),
            glb: None,
            key: None,
            foliage: None,
            ground: None,
            collision: None,
            other: Default::default(),
        },
    );
    world.insert(mesh, primase_reskate::components::SkateParts(vec![]));
    let before = format!("{world:?}");
    let mut log = PassLog::default();
    assert!(
        primase_reskate::passes::LowerLayeredPbr
            .run(&mut world, &mut log)
            .is_err()
    );
    assert_eq!(format!("{world:?}"), before);
    assert!(log.counts.is_empty());
}

#[test]
fn lowering_prunes_only_unreachable_replaced_dependencies() {
    for shared in [false, true] {
        let mut world = World::default();
        let selected_texture = world.spawn();
        let unused_texture = world.spawn();
        let selected = world.spawn();
        let unused = world.spawn();
        for (entity, texture) in [(selected, selected_texture), (unused, unused_texture)] {
            world.insert(
                entity,
                PbrMaterial {
                    base_color_texture: Some(TextureRef::new(texture)),
                    ..Default::default()
                },
            );
        }
        let host = world.spawn();
        world.insert(
            host,
            LayeredMaterial {
                layers: [selected, unused]
                    .into_iter()
                    .map(|material| {
                        Some(Layer {
                            material,
                            texcoord: None,
                            transform: None,
                        })
                    })
                    .collect(),
                weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
            },
        );
        let primitive = Primitive {
            topology: Topology::Triangles,
            material: Some(host),
            indices: Some(vec![0, 1, 2]),
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]),
                ),
                (Semantic::TexCoord(0), Stream::F32x2(vec![[0.; 2]; 3])),
                (Semantic::Color(1), Stream::F32x4(vec![[1., 0., 0., 0.]; 3])),
            ]),
        };
        let mesh = world.spawn();
        world.insert(mesh, Mesh::new(vec![primitive.clone()]));
        let unrelated = world.spawn();
        let mut roots = vec![mesh, unrelated];
        if shared {
            let other = world.spawn();
            world.insert(
                other,
                Mesh::new(vec![Primitive {
                    material: Some(unused),
                    ..primitive
                }]),
            );
            roots.push(other);
        }
        assert_eq!(world.closure(roots.iter().copied()).len(), world.entity_count());
        primase_reskate::passes::LowerLayeredPbr
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        let alive: std::collections::BTreeSet<_> = world.entities().collect();
        assert!(!alive.contains(&host));
        assert!(!alive.contains(&selected));
        assert!(alive.contains(&selected_texture));
        assert_eq!(alive.contains(&unused), shared);
        assert_eq!(alive.contains(&unused_texture), shared);
        assert!(alive.contains(&unrelated));
        assert_eq!(
            world.closure(roots),
            alive,
            "lowering must retain the exact root closure"
        );
    }
}
