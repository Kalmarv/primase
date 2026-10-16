use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    Layer, LayerWeights, LayeredMaterial, Mesh, Primitive, Semantic, Stream, Topology, WeightMapping, World,
};
use std::collections::BTreeMap;

/// Real scene gate: source GLBs remain untouched; every source triangle occurs
/// once after strongest-layer splitting and every resulting material is PBR.
#[test]
fn installed_layered_scene() {
    let Some(root) = primase_core::gate::real_data("PRIMASE_SCENES_GTA") else {
        return;
    };
    if !root.join("scene.json").is_file() {
        return;
    }
    let mut world = World::default();
    primase_skate_scene::read(&root, &mut world).unwrap();
    let layered = world.iter::<LayeredMaterial>().count();
    if layered == 0 {
        return;
    }
    primase_gltf::load_mesh_files(&mut world, &root).unwrap();
    let triangles = |w: &World| {
        w.iter::<Mesh>()
            .flat_map(|(_, m)| &m.primitives)
            .map(|p| {
                p.indices
                    .as_ref()
                    .map_or_else(|| p.positions().unwrap().len(), Vec::len)
                    / 3
            })
            .sum::<usize>()
    };
    let before = triangles(&world);
    primase_skate_scene::LowerSkatePbr
        .run(&mut world, &mut PassLog::default())
        .unwrap();
    let mut log = PassLog::default();
    primase_skate_scene::LowerLayeredPbr
        .run(&mut world, &mut log)
        .unwrap();
    assert_eq!(triangles(&world), before);
    for (_, mesh) in world.iter::<Mesh>() {
        for p in &mesh.primitives {
            if let Some(m) = p.material {
                assert!(world.has::<PbrMaterial>(m));
            }
        }
    }
    println!(
        "{layered} layered materials, {before} total triangles preserved; {:?}",
        log.counts
    );
}

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
            weights: LayerWeights {
                attribute: Semantic::Color(1),
                mapping: WeightMapping::Identity,
            },
        },
    );
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh {
            primitives: vec![Primitive {
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
            }],
        },
    );
    let mut log = PassLog::default();
    primase_skate_scene::LowerLayeredPbr
        .run(&mut world, &mut log)
        .unwrap();
    assert_eq!(log.counts["triangles selected layer 0"], 1);
    let part = &world.get::<Mesh>(mesh).unwrap().primitives[0];
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
    let before = world.entities().count();
    let mut again = PassLog::default();
    primase_skate_scene::LowerLayeredPbr
        .run(&mut world, &mut again)
        .unwrap();
    assert!(again.counts.is_empty());
    assert_eq!(world.entities().count(), before);
}
