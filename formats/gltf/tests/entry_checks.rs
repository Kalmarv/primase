//! What the GLB writer checks at its entry: a world its components' verifiers refuse is an error, not an index out
//! of range; a sheared light is left out and counted; a spot's cones are glTF's.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::{
    Deltas, Instance, Light, LightKind, Mesh, MorphFrame, MorphId, MorphTarget, Morphs, Primitive, Semantic,
    Stream, Topology, Transform, World,
};
use primase_gltf::{WriteOptions, write};

fn triangle() -> Mesh {
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
        )]),
        indices: None,
        material: None,
    }])
}

fn light(kind: LightKind) -> Light {
    Light {
        kind,
        color: [1.0; 3],
        intensity: 1.0,
        range: None,
        cast_shadows: None,
        radius: None,
    }
}

/// A morph target with one delta for a mesh of three vertices: the verifier refuses it, and the writer, which copied
/// the deltas into a dense list by their length, panicked.
#[test]
fn a_world_its_verifiers_refuse_is_an_error_naming_the_component() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, triangle());
    w.insert(
        proto,
        Morphs {
            targets: vec![MorphTarget {
                id: MorphId(1),
                name: "short".into(),
                default_weight: 0.0,
                frames: vec![MorphFrame {
                    weight: 1.0,
                    primitives: vec![Some(Deltas {
                        positions: Some(vec![[1.0, 0.0, 0.0]]),
                        ..Deltas::default()
                    })],
                }],
            }],
        },
    );
    let p = w.spawn();
    w.insert(
        p,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    w.insert(p, Transform::IDENTITY);
    let e = write(&w, &WriteOptions::default()).err().unwrap().to_string();
    assert!(
        e.contains("core.morphs") && e.contains("1 deltas for 3 entries"),
        "{e}"
    );
}

/// A sheared light has no node matrix glTF allows: it is not written, and it is counted; a rigid one is written.
#[test]
fn a_sheared_light_is_not_written_and_is_counted() {
    let mut w = World::new();
    for sheared in [false, true] {
        let e = w.spawn();
        w.insert(e, light(LightKind::Point));
        let mut t = Transform::IDENTITY;
        if sheared {
            t.cols[1] = [0.5, 1.0, 0.0];
        }
        w.insert(e, t);
    }
    let (_, report) = write(&w, &WriteOptions::default()).unwrap();
    assert_eq!((report.lights, report.lights_sheared_skipped), (1, 1));
}

/// A spot's cones are glTF's: the inner not below 0 or past the outer, the outer above 0 and at most pi / 2. (Core's
/// verifier says the same, so the world is built unverified for the ones it would refuse first.)
#[test]
fn a_spots_cones_are_gltfs() {
    let write_spot = |inner_cone: f32, outer_cone: f32| {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(
            e,
            light(LightKind::Spot {
                inner_cone,
                outer_cone,
            }),
        );
        w.insert(e, Transform::IDENTITY);
        let options = WriteOptions {
            verified: true,
            ..WriteOptions::default()
        };
        write(&w, &options).map(|_| ()).map_err(|e| e.to_string())
    };
    let half_pi = std::f32::consts::FRAC_PI_2;
    assert!(write_spot(0.2, 0.4).is_ok());
    assert!(write_spot(0.0, half_pi).is_ok());
    for (inner, outer) in [(0.2, 0.1), (-0.1, 0.4), (0.2, half_pi + 0.01), (0.0, 0.0)] {
        let e = write_spot(inner, outer).unwrap_err();
        assert!(e.contains("spot cones"), "{inner} {outer}: {e}");
    }
    // a NaN cone never reaches the cone check: the light's component is written into the node's extras first, as
    // JSON, which has no NaN, and that refusal names the field
    let e = write_spot(0.2, f32::NAN).unwrap_err();
    assert!(e.contains("outer_cone") && e.contains("non-finite"), "{e}");
}
