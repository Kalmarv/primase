use primase_core::pass::PassLog;
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Component, Layer, LayerWeights, LayeredMaterial, MaterialVariants, Mesh, MeshFile, Primitive, Semantic,
    Stream, Topology, WeightMapping, World, remap_list,
};
use primase_meshops::{SelectionPolicy, plan_layered_pbr};
use std::collections::BTreeMap;

fn fixture() -> (World, primase_core::Entity) {
    let mut world = World::new();
    let ordinary = world.spawn();
    world.insert(ordinary, PbrMaterial::default());
    let layered = world.spawn();
    world.insert(
        layered,
        LayeredMaterial {
            layers: vec![
                Some(Layer {
                    material: ordinary,
                    texcoord: None,
                    transform: None,
                }),
                None,
            ],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::Identity),
        },
    );
    let entity = world.spawn();
    let part = Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![
                    [0., 0., 0.],
                    [1., 0., 0.],
                    [0., 1., 0.],
                    [2., 0., 0.],
                    [3., 0., 0.],
                    [2., 1., 0.],
                ]),
            ),
            (
                Semantic::Color(1),
                Stream::F32x4(vec![
                    [1., 0., 0., 0.],
                    [1., 0., 0., 0.],
                    [1., 0., 0., 0.],
                    [0., 1., 0., 0.],
                    [0., 1., 0., 0.],
                    [0., 1., 0., 0.],
                ]),
            ),
            (Semantic::Tangent, Stream::F32x4(vec![[1., 0., 0., -1.]; 6])),
            (Semantic::TexCoord(4), Stream::F32x2(vec![[0.25, 0.75]; 6])),
        ]),
        indices: Some(vec![0, 1, 2, 3, 4, 5]),
        material: Some(layered),
    };
    world.insert(
        entity,
        Mesh {
            primitives: vec![part],
        },
    );
    world.insert(entity, MaterialVariants(vec![BTreeMap::new()]));
    world.insert(
        entity,
        MeshFile {
            path: "source.glb".into(),
            mesh: 0,
            materials: vec![Some(layered)],
        },
    );
    (world, entity)
}

#[test]
fn planned_sources_keep_null_slots_and_apply_preserves_order_and_streams() {
    let (mut world, entity) = fixture();
    let before = format!("{world:?}");
    let original = world.get::<Mesh>(entity).unwrap().primitives[0].clone();
    let plan = plan_layered_pbr(&world, SelectionPolicy::CoreWeights).unwrap();
    assert_eq!(format!("{world:?}"), before, "planning does not mutate");
    assert_eq!(plan.policy(), SelectionPolicy::CoreWeights);
    assert_eq!(plan.sources().len(), 1);
    let source = &plan.sources()[0];
    assert_eq!((source.entity, source.primitive_count), (entity, 1));
    assert_eq!(
        source
            .parts
            .iter()
            .map(|p| (p.primitive, p.layer))
            .collect::<Vec<_>>(),
        [(0, Some(0)), (0, Some(1))]
    );
    let mut log = PassLog::default();
    primase_meshops::lower_layered_pbr(&mut world, SelectionPolicy::CoreWeights, &mut log, |_, _| Ok(()))
        .unwrap();
    let mesh = world.get::<Mesh>(entity).unwrap();
    assert_eq!(mesh.primitives.len(), 2);
    assert_eq!(mesh.primitives[0].indices, Some(vec![0, 1, 2]));
    assert_eq!(mesh.primitives[1].indices, Some(vec![3, 4, 5]));
    for part in &mesh.primitives {
        for (semantic, stream) in &original.streams {
            assert_eq!(&part.streams[semantic], stream);
        }
    }
    let black = world
        .get::<PbrMaterial>(mesh.primitives[1].material.unwrap())
        .unwrap();
    assert_eq!(black.base_color, [0., 0., 0., 1.]);
    assert!(!world.has::<MeshFile>(entity));
    assert_eq!(world.get::<MaterialVariants>(entity).unwrap().0.len(), 2);
    assert_eq!(log.counts["triangles selected with core weights layer 0"], 1);
    assert_eq!(log.counts["triangles selected with core weights layer 1"], 1);
}

#[test]
fn invalid_later_mesh_does_not_publish_earlier_plans() {
    let (mut world, entity) = fixture();
    let second = world.spawn();
    let mut bad = world.get::<Mesh>(entity).unwrap().clone();
    bad.primitives[0].streams.remove(&Semantic::Color(1));
    world.insert(second, bad);
    let before = format!("{world:?}");
    assert!(plan_layered_pbr(&world, SelectionPolicy::CoreWeights).is_err());
    assert_eq!(format!("{world:?}"), before);
}

#[test]
fn unloaded_layered_variants_are_refused_before_mutation() {
    let (mut world, entity) = fixture();
    let layered = world.get::<Mesh>(entity).unwrap().primitives[0].material.unwrap();
    let ordinary = world.get::<LayeredMaterial>(layered).unwrap().layers[0]
        .as_ref()
        .unwrap()
        .material;
    world.remove::<Mesh>(entity);
    world.insert(
        entity,
        MeshFile {
            path: "source.glb".into(),
            mesh: 0,
            materials: vec![Some(ordinary)],
        },
    );
    world.insert(entity, MaterialVariants(vec![BTreeMap::from([(1, layered)])]));
    let before = format!("{world:?}");
    assert!(plan_layered_pbr(&world, SelectionPolicy::CoreWeights).is_err());
    assert_eq!(format!("{world:?}"), before);
}

#[test]
fn metadata_validation_failure_does_not_mutate_world_or_log() {
    let (mut world, _) = fixture();
    let before = format!("{world:?}");
    let mut log = PassLog::default();
    let error = primase_meshops::lower_layered_pbr(
        &mut world,
        SelectionPolicy::CoreWeights,
        &mut log,
        |snapshot, sources| {
            assert_eq!(format!("{snapshot:?}"), before);
            assert_eq!(sources.len(), 1);
            Err(primase_meshops::Error("invalid dialect metadata".into()))
        },
    );
    assert!(error.is_err());
    assert_eq!(format!("{world:?}"), before);
    assert!(log.counts.is_empty());
}

#[test]
fn empty_split_still_prunes_the_replaced_host() {
    let (mut world, entity) = fixture();
    let host = world.get::<Mesh>(entity).unwrap().primitives[0].material.unwrap();
    world.get_mut::<Mesh>(entity).unwrap().primitives[0].indices = Some(vec![]);
    primase_meshops::lower_layered_pbr(
        &mut world,
        SelectionPolicy::CoreWeights,
        &mut PassLog::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!world.entities().any(|e| e == host));
    assert!(world.get::<Mesh>(entity).unwrap().primitives.is_empty());
    assert_eq!(world.closure([entity]).len(), world.entity_count());
}

/// A dialect's per-primitive data (a reader's source of each primitive), as it types it.
#[derive(Clone, Debug, PartialEq)]
struct Sources(Vec<&'static str>);
impl Component for Sources {
    const NAME: &'static str = "test.sources";
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(remap_list(&self.0, from).map(Sources))
    }
    fn verify(&self, entity: primase_core::Entity, world: &World) -> Result<(), String> {
        let n = world.get::<Mesh>(entity).map_or(0, |m| m.primitives.len());
        if self.0.len() != n {
            return Err(format!("{} sources for {n} primitives", self.0.len()));
        }
        Ok(())
    }
}

/// Worked by hand: the fixture's one primitive, its two triangles choosing layers 0 and 1, becomes two primitives
/// from source 0: a dialect's per-primitive component follows (["a"] to ["a", "a"]) and the world verifies.
#[test]
fn a_split_carries_every_per_primitive_component() {
    let (mut world, entity) = fixture();
    world.insert(entity, Sources(vec!["a"]));
    let mut log = PassLog::default();
    primase_meshops::lower_layered_pbr(&mut world, SelectionPolicy::CoreWeights, &mut log, |_, _| Ok(()))
        .unwrap();
    assert_eq!(world.get::<Mesh>(entity).unwrap().primitives.len(), 2);
    assert_eq!(world.get::<Sources>(entity), Some(&Sources(vec!["a", "a"])));
    assert_eq!(world.verify_components(), vec![]);
}

/// A per-primitive component that does not have the source index refuses the split, and nothing changes.
#[test]
fn a_component_that_cannot_follow_refuses_the_split_unchanged() {
    let (mut world, entity) = fixture();
    world.insert(entity, Sources(vec![]));
    let before = format!("{world:?}");
    let mut log = PassLog::default();
    let e =
        primase_meshops::lower_layered_pbr(&mut world, SelectionPolicy::CoreWeights, &mut log, |_, _| Ok(()))
            .unwrap_err();
    assert!(e.0.contains("test.sources"), "{}", e.0);
    assert_eq!(format!("{world:?}"), before);
    assert!(log.counts.is_empty());
}
