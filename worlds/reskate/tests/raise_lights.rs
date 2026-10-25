//! lower-skate-lights: the inverse of raise-skate's lights, checked by a round trip and a hand case.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog};
use primase_core::{Light, LightKind, Transform, World};
use primase_reskate::{
    components::SkateLight,
    passes::lower::{LowerSkateLights, lower_light},
    passes::{LightCalibration, RAISE_SKATE, RaiseOptions, raise_skate},
};

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
                radius: Some(0.15),
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
                radius: None,
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
        &RaiseOptions::new(
            "lights",
            LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
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
        // the emitter's size survives (an unscaled placement); a point stays a point
        match (lowered.radius, light.radius) {
            (Some(a), Some(b)) => assert!(close(a, b), "{a} vs {b}"),
            (a, b) => assert_eq!(a, b),
        }
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
            // 1 cd through raise-skate's calibration
            energy: (4.0
                * std::f64::consts::PI
                * std::f64::consts::PI
                * primase_reskate::passes::SKATE_LIGHT_SCALE) as f32,
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

/// raise-skate's light calibration, worked by hand: 1,000 cd x SKATE_LIGHT_SCALE 3.007e-4 = 0.3007 cd-equivalent,
/// x 4 pi^2 = 11.871 Studio watts; light_scale=0.5 gives 19.739 W and is recorded as an approximation; a scale that
/// is not finite and above 0 is refused.
#[test]
fn raise_skate_calibrates_light_energy() {
    let light = |w: &mut World| {
        let e = w.spawn();
        w.insert(
            e,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 1000.0,
                range: None,
                cast_shadows: None,
                radius: None,
            },
        );
        w.insert(e, Transform::IDENTITY);
        e
    };
    for (calibration, watts, recorded) in [
        (LightCalibration::Standard, 11.871_f32, false),
        (LightCalibration::Scale(0.5e-3), 19.739, true),
    ] {
        let scale = calibration.scale();
        let mut w = World::new();
        let e = light(&mut w);
        let log = raise_skate(
            &mut w,
            &RaiseOptions::new(
                "lit",
                calibration,
                primase_reskate::passes::Unclassified::LeaveOut,
            )
            .unwrap(),
        )
        .unwrap();
        let energy = w.get::<SkateLight>(e).unwrap().energy;
        assert!((energy - watts).abs() < 1e-3, "{energy}");
        // every raised light is an approximation by the calibration's value, the default included
        let (reason, n) = log
            .approximations
            .iter()
            .find(|(k, _)| k.starts_with("light energy as Studio watts"))
            .unwrap();
        assert_eq!(*n, 1);
        assert!(reason.contains(&format!("light_scale {scale} ")), "{reason}");
        assert_eq!(
            reason.contains("not skate's fitted calibration"),
            recorded,
            "{reason}"
        );
    }
    // refused when the options are made, before anything runs
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let e = RaiseOptions::new(
            "bad",
            LightCalibration::Scale(bad),
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap_err();
        assert!(e.contains("light_scale"), "{e}");
    }
}

/// The calibration is the program's: raise-skate's options name it (`standard` or a scale), print it, and read it back.
#[test]
fn the_calibration_is_stated() {
    let o = |kv: &[(&str, &str)]| -> primase_core::pass::Options {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    let e = RAISE_SKATE
        .instance(&o(&[("name", "n"), ("unclassified", "leave-out")]))
        .err()
        .unwrap();
    assert!(e.contains("light_scale is required"), "{e}");
    assert!(
        RAISE_SKATE
            .instance(&o(&[
                ("name", "n"),
                ("light_scale", "bright"),
                ("unclassified", "leave-out")
            ]))
            .is_err()
    );
    for (given, calibration) in [
        ("standard", LightCalibration::Standard),
        ("0.0003007", LightCalibration::Scale(3.007e-4)),
    ] {
        let made = RAISE_SKATE
            .instance(&o(&[
                ("name", "n"),
                ("light_scale", given),
                ("unclassified", "leave-out"),
            ]))
            .unwrap();
        let r = RaiseOptions::new("n", calibration, primase_reskate::passes::Unclassified::LeaveOut).unwrap();
        assert_eq!(made.options(), r.options());
        assert_eq!(r.options()["light_scale"], given);
        assert_eq!(RAISE_SKATE.instance(&r.options()).unwrap().options(), r.options());
    }
}
