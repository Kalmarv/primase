//! A source material's opaque bits follow ordinary destination collision filtering through its surface index.
#![allow(clippy::unwrap_used)]

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Collision, Surface, SurfaceClass, Surfaces, World};
use primase_shred::components::{CollisionSurface, CollisionSurfaces};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn raise_keeps_source_identities_when_some_or_all_triangles_are_omitted() {
    let mut world = World::new();
    world.insert_resource(Surfaces(vec![
        Surface {
            name: "source".into(),
            class: Some(SurfaceClass::Other),
        },
        Surface::new("hazard"),
        Surface {
            name: "source [shred-flags=0x00001000]".into(),
            class: Some(SurfaceClass::Other),
        },
    ]));
    let origins = CollisionSurfaces(BTreeMap::from([
        (
            0,
            CollisionSurface {
                database_id: 0,
                database_name: "source".into(),
                flags: 0,
            },
        ),
        (
            1,
            CollisionSurface {
                database_id: 1,
                database_name: "hazard".into(),
                flags: 0,
            },
        ),
        (
            2,
            CollisionSurface {
                database_id: 0,
                database_name: "source".into(),
                flags: 0x1000,
            },
        ),
    ]));
    world.insert_resource(origins.clone());
    world.insert_resource(primase_reskate::components::SkateLaidLayers {
        gap: 0.001,
        share: 1.0,
        layers: vec![],
    });
    let mut soup = Collision {
        positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        triangles: vec![[0, 1, 2]; 4],
        surfaces: vec![2, 1, 0, 2],
    };
    let keep = world.spawn();
    world.insert(keep, soup.clone());
    let omit = world.spawn();
    soup.triangles.truncate(1);
    soup.surfaces = vec![1];
    world.insert(omit, soup);
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let steps = registry
        .parse("raise-skate{name=MaterialIdentity,light_scale=standard,unclassified=leave-out}")
        .unwrap();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .run(&mut world, &steps, &mut report)
        .unwrap();
    assert_eq!(report.steps.len(), 1);
    assert!(report.steps[0].skipped.is_empty());
    assert!(
        world.verify_components().is_empty(),
        "{:?}",
        world.verify_components()
    );
    assert!(world.get::<Collision>(omit).is_none());
    let kept = world.get::<Collision>(keep).unwrap();
    assert_eq!(kept.surfaces, [2, 0, 2]);
    let after = world.resource::<CollisionSurfaces>().unwrap();
    assert_eq!(after, &origins);
    assert_eq!(
        kept.surfaces
            .iter()
            .map(|i| (after.0[i].database_id, after.0[i].flags))
            .collect::<Vec<_>>(),
        [(0, 0x1000), (0, 0), (0, 0x1000)]
    );
    assert_eq!(
        world
            .subset(&BTreeSet::from([keep]))
            .resource::<CollisionSurfaces>(),
        Some(&origins)
    );
}
