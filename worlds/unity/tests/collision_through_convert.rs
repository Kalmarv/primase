//! A shape the collision lowering makes apart from its collider survives a conversion: the pass declares it as made
//! from its collider, so the part that owns the collider writes it.
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use primase_core::convert::{Finished, Reader, Report, Result, WholeWorld, Writer, convert_parts};
use primase_core::pass::{PassManager, PassRegistry, Pipeline, Target};
use primase_core::{Collider, Entity, Name, Shape, Transform, World};
use primase_unity::components::Stored;
use primase_unity::components::{UnityCollider, UnityColliderOf, UnityColliderShape};
use primase_unity::lower_collision::LowerUnityCollision;
use unity_formats::player::ObjRef;

/// One sphere collider, named, at (10, 0, 0).
struct OneSphere;

impl Reader for OneSphere {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Name("ball".into()));
        let mut t = Transform::IDENTITY;
        t.cols[3] = [10.0, 0.0, 0.0];
        w.insert(e, t);
        w.insert(
            e,
            UnityCollider {
                collider: Stored(ObjRef {
                    file: "level1".into(),
                    path_id: 1,
                }),
                layer: 0,
                shape: UnityColliderShape::Sphere {
                    center: [0.0; 3],
                    radius: 0.5,
                },
                trigger: false,
                physic_material: None,
                rigidbody: None,
                lowered: false,
            },
        );
        Ok((w, Report::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "unity"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// What each part held: its colliders, with the collider each was made from.
type Written = Rc<RefCell<Vec<(Collider, Option<Entity>, Transform)>>>;

struct Colliders(Written);

impl Writer for Colliders {
    fn target(&self) -> Target {
        Target::new("colliders")
            .legal("core.name")
            .legal("core.transform")
            .legal("core.collider")
            .legal("core.surfaces")
            .legal("unity.collider")
            .legal("unity.collider_of")
    }
    fn write(&mut self, part: &World) -> Result<()> {
        for (e, c) in part.iter::<Collider>() {
            self.0.borrow_mut().push((
                c.clone(),
                part.get::<UnityColliderOf>(e).map(|of| of.0),
                *part.get::<Transform>(e).unwrap(),
            ));
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

#[test]
fn a_sphere_lowered_apart_from_its_collider_reaches_the_writer() {
    let written = Written::default();
    let manager = PassManager::new(PassRegistry::new());
    convert_parts(
        &mut OneSphere,
        &mut WholeWorld::default(),
        Box::new(Colliders(written.clone())),
        &manager,
        &Pipeline { passes: Vec::new() },
        &Pipeline {
            passes: vec![Box::new(LowerUnityCollision)],
        },
    )
    .unwrap();
    let written = written.borrow();
    assert_eq!(
        written.len(),
        1,
        "the sphere made apart from its collider is written"
    );
    let (collider, of, at) = &written[0];
    assert!(matches!(collider.shape, Shape::Sphere { radius, .. } if radius == 0.5));
    assert!(of.is_some());
    assert_eq!(at.cols[3], [10.0, 0.0, 0.0]);
}
