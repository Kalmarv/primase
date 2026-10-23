//! `lower-skate-lights`: a skate spot becomes a core spot only where core's cone holds it.

#![allow(clippy::unwrap_used)]

use primase_core::LightKind;
use primase_reskate::components::SkateLight;
use primase_reskate::passes::lower::lower_light;

fn spot(spot_size: f32, spot_blend: f32) -> SkateLight {
    SkateLight {
        spot: true,
        position: [0.0; 3],
        direction: [0.0, -1.0, 0.0],
        tangent: [1.0, 0.0, 0.0],
        color: [1.0; 3],
        energy: 100.0,
        range: 10.0,
        radius: 0.1,
        spot_size,
        spot_blend,
        time_of_day: 127,
        cast_shadow: false,
        priority: None,
        group: String::new(),
        unknown_flags: 0,
    }
}

/// The outer cone is half the size across, the inner that less the blend; a size of nothing, past pi across, or not
/// a number is refused (core's cone is at most pi / 2 as a half angle), and the pass counts it as not lowered.
#[test]
fn a_spots_cone_is_bounded() {
    let (light, _) = lower_light(&spot(1.0, 0.25)).unwrap();
    assert_eq!(
        light.kind,
        LightKind::Spot {
            inner_cone: 0.375,
            outer_cone: 0.5
        }
    );
    let (light, _) = lower_light(&spot(std::f32::consts::PI, 0.0)).unwrap();
    assert!(
        matches!(light.kind, LightKind::Spot { outer_cone, .. } if outer_cone == std::f32::consts::FRAC_PI_2)
    );
    for size in [0.0, -1.0, 3.2, f32::NAN, f32::INFINITY] {
        let e = lower_light(&spot(size, 0.0)).unwrap_err();
        assert!(e.contains("spot light"), "{size}: {e}");
    }
}
