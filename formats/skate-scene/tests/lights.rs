//! lower-skate-lights: the inverse of raise-skate's lights, checked by a round trip and a hand case.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog};
use primase_core::{Light, LightKind, Transform, World};
use primase_skate_scene::{LowerSkateLights, RaiseOptions, SkateLight, lower_light, raise_skate};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1.0)
}

/// A spot at (1, 2, 3) aimed down -X, tilted.
fn spot_transform() -> Transform {
    let z = [0.8f32, 0.6, 0.0]; // local -Z points along (-0.8, -0.6, 0)
    let x = [0.0, 0.0, -1.0];
    let y = [
        z[1] * x[2] - z[2] * x[1],
        z[2] * x[0] - z[0] * x[2],
        z[0] * x[1] - z[1] * x[0],
    ];
    Transform {
        cols: [x, y, z, [1.0, 2.0, 3.0]],
    }
}

#[test]
fn raise_then_lower_restores_core_lights() {
    let mut w = World::new();
    let lights = [
        (
            Light {
                kind: LightKind::Spot {
                    inner_cone: 0.3,
                    outer_cone: 0.5,
                },
                color: [1.0, 0.5, 0.25],
                intensity: 12.0,
                range: Some(20.0),
                cast_shadows: None,
            },
            spot_transform(),
        ),
        (
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 3.0,
                range: None,
                cast_shadows: None,
            },
            Transform {
                cols: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [-4.0, 5.0, 6.0],
                ],
            },
        ),
    ];
    let mut entities = Vec::new();
    for (light, t) in &lights {
        let e = w.spawn();
        w.insert(e, light.clone());
        w.insert(e, *t);
        entities.push(e);
    }
    raise_skate(
        &mut w,
        &RaiseOptions {
            name: "lights".into(),
            surfaces: BTreeMap::new(),
        },
    )
    .unwrap();
    for (e, (light, t)) in entities.iter().zip(&lights) {
        let (lowered, transform) = lower_light(w.get::<SkateLight>(*e).unwrap()).unwrap();
        assert!(
            close(lowered.intensity, light.intensity),
            "{} vs {}",
            lowered.intensity,
            light.intensity
        );
        assert_eq!(lowered.color, light.color);
        assert_eq!(lowered.range, light.range);
        match (lowered.kind, light.kind) {
            (
                LightKind::Spot {
                    inner_cone: a,
                    outer_cone: b,
                },
                LightKind::Spot {
                    inner_cone: c,
                    outer_cone: d,
                },
            ) => assert!(close(a, c) && close(b, d)),
            (a, b) => assert_eq!(a, b),
        }
        assert_eq!(transform.cols[3], t.cols[3]);
        // The aim (local -Z) survives; the roll is not a property of a round light.
        for i in 0..3 {
            assert!(
                close(transform.cols[2][i], t.cols[2][i]),
                "{:?} vs {:?}",
                transform.cols[2],
                t.cols[2]
            );
        }
    }
}

#[test]
fn pass_lowers_once_and_builds_an_orthonormal_basis() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(
        e,
        SkateLight {
            spot: true,
            position: [0.0, 10.0, 0.0],
            direction: [0.0, -1.0, 0.0], // straight down: world +Y can't be the up hint
            tangent: [0.0; 3],
            color: [1.0; 3],
            energy: (4.0 * std::f64::consts::PI * std::f64::consts::PI) as f32,
            range: 0.0,
            radius: 0.1,
            spot_size: 1.0,
            spot_blend: 0.25,
            time_of_day: 0x7F,
            cast_shadow: false,
            priority: None,
            group: String::new(),
            unknown_flags: 0,
        },
    );
    let mut log = PassLog::default();
    LowerSkateLights.run(&mut w, &mut log).unwrap();
    LowerSkateLights.run(&mut w, &mut log).unwrap();
    let light = w.get::<Light>(e).unwrap();
    assert!(close(light.intensity, 1.0));
    assert_eq!(light.range, None);
    assert_eq!(light.cast_shadows, Some(false), "the flag is a fact either way");
    assert_eq!(
        light.kind,
        LightKind::Spot {
            inner_cone: 0.375,
            outer_cone: 0.5
        }
    );
    let c = w.get::<Transform>(e).unwrap().cols;
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    assert_eq!(c[2], [0.0, 1.0, 0.0]);
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        assert!(dot(c[i], c[j]).abs() < 1e-6);
    }
    for v in &c[..3] {
        assert!(close(dot(*v, *v), 1.0));
    }
    let (casting, _) = lower_light(&SkateLight {
        cast_shadow: true,
        ..w.get::<SkateLight>(e).unwrap().clone()
    })
    .unwrap();
    assert_eq!(casting.cast_shadows, Some(true));
    assert!(
        lower_light(&SkateLight {
            direction: [0.0; 3],
            ..w.get::<SkateLight>(e).unwrap().clone()
        })
        .is_err()
    );
}
