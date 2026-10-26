use std::collections::BTreeMap;

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Collider, Collision, Instance, Shape, Surface, Surfaces, Transform, World};
use primase_shaunwhite::components::{CollisionSource, CollisionSurfaces};

fn manager() -> PassManager {
    let mut registry = PassRegistry::new();
    primase_shaunwhite::register(&mut registry);
    primase_meshops::register(&mut registry);
    PassManager::new(registry).with_pass_checks(true)
}

#[test]
fn exact_triangles_names_and_placements_survive_lowering() {
    let mut w = World::new();
    let p = w.spawn();
    let positions = vec![[0., 0., 0.], [2., 0., 0.], [0., 0., -3.], [2., 0., -3.]];
    let triangles = vec![[0, 1, 2], [2, 1, 3]];
    w.insert(
        p,
        CollisionSource::Mesh {
            positions: positions.clone(),
            triangles: triangles.clone(),
            surfaces: vec![90, 70],
        },
    );
    w.insert_resource(CollisionSurfaces(BTreeMap::from([
        (70, "WoodDeck".into()),
        (90, "ConcreteVert".into()),
    ])));
    w.insert_resource(Surfaces(vec![
        Surface::new("existing"),
        Surface::new("ConcreteVert"),
    ]));
    let e = w.spawn();
    let mut pose = Transform::IDENTITY;
    pose.cols[3] = [10., 4., -20.];
    w.insert(e, pose);
    w.insert(
        e,
        Instance {
            prototype: p,
            variant: 0,
        },
    );
    let manager = manager();
    let pipeline = manager.registry.parse("lower-shaunwhite-collision").unwrap();
    manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    let c = w.get::<Collision>(p).unwrap();
    assert_eq!(c.positions, positions);
    assert_eq!(c.triangles, triangles);
    assert_eq!(c.surfaces, [1, 2]);
    assert_eq!(
        w.resource::<Surfaces>()
            .unwrap()
            .0
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["existing", "ConcreteVert", "WoodDeck"]
    );
    assert_eq!(w.get::<Transform>(e), Some(&pose));
    assert_eq!(w.get::<Instance>(e).unwrap().prototype, p);
    assert!(w.resource::<CollisionSurfaces>().is_none());
}

#[test]
fn implicit_shapes_feed_the_shared_tessellator() {
    let mut w = World::new();
    w.insert_resource(CollisionSurfaces(BTreeMap::from([(7, "Metal".into())])));
    let shapes = [
        Shape::Box {
            center: [0.; 3],
            half_extents: [1., 2., 3.],
        },
        Shape::Sphere {
            center: [1., 2., 3.],
            radius: 0.5,
        },
        Shape::Capsule {
            a: [0.; 3],
            b: [0., 2., 0.],
            radius: 0.5,
        },
        Shape::Convex {
            points: vec![[0.; 3], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        },
    ];
    let mut entities = Vec::new();
    for shape in &shapes {
        let e = w.spawn();
        w.insert(
            e,
            CollisionSource::Shape {
                shape: shape.clone(),
                surface: 7,
            },
        );
        entities.push(e);
    }
    let manager = manager();
    let lower = manager.registry.parse("lower-shaunwhite-collision").unwrap();
    manager.run(&mut w, &lower, &mut RunReport::default()).unwrap();
    for (&e, shape) in entities.iter().zip(&shapes) {
        assert_eq!(&w.get::<Collider>(e).unwrap().shape, shape);
    }
    let tessellate = manager.registry.parse("tessellate-colliders").unwrap();
    manager
        .run(&mut w, &tessellate, &mut RunReport::default())
        .unwrap();
    assert_eq!(w.iter::<Collider>().count(), 0);
    for e in entities {
        let c = w.get::<Collision>(e).unwrap();
        assert!(!c.triangles.is_empty());
        assert_eq!(c.surfaces, vec![0; c.triangles.len()]);
    }
}
