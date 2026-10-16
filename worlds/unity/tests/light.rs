//! Light gate against an independent reader's scan (`PRIMASE_UNITY_SCAN`: `lights`, the shining lights other than the
//! sun, and `sun`, the shining directional light; each `path`, `type`, `color`, `intensity`, `range`, `spot_angle`,
//! `inner_spot_angle`, `shadows`, `M` rounded to 6 decimals), for the player `PRIMASE_UNITY_DATA` read with
//! `PRIMASE_UNITY_TPK`: every light's fields exactly, its world matrix within the scan's rounding (5e-7).

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::player::Player;
use primase_unity::scene::{LightComponent, Scene};
use serde_json::{Value as Json, json};

fn record(l: &LightComponent) -> Json {
    let f = |x: f32| json!(f64::from(x));
    json!({"path": l.path, "type": l.kind,
        "color": {"r": f(l.color[0]), "g": f(l.color[1]), "b": f(l.color[2]), "a": f(l.color[3])},
        "intensity": f(l.intensity), "range": f(l.range), "spot_angle": f(l.spot_angle),
        "inner_spot_angle": f(l.inner_spot_angle), "shadows": l.shadows})
}

#[test]
fn lights_match_the_scan() {
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let scene = Scene::read(&player, scan["level"].as_str().unwrap()).unwrap();
    let (lights, off) = scene.lights(&player).unwrap();
    let (suns, others): (Vec<_>, Vec<_>) = lights.iter().partition(|l| l.kind == 1);
    let theirs = scan["lights"].as_array().unwrap();
    assert_eq!(others.len(), theirs.len(), "lights other than directional");
    assert_eq!(suns.len(), 1, "one shining directional light");
    let mut fields = GateReport::new(
        "lights: path, type, colour, intensity, range, angles, shadows",
        Tolerance::Exact,
    );
    let mut matrices = GateReport::new("lights: world matrix", Tolerance::Abs(5e-7));
    for (l, t) in others
        .iter()
        .chain(&suns)
        .zip(theirs.iter().chain([&scan["sun"]]))
    {
        let want = json!({"path": t["path"], "type": t["type"], "color": t["color"], "intensity": t["intensity"],
            "range": t["range"], "spot_angle": t["spot_angle"], "inner_spot_angle": t["inner_spot_angle"],
            "shadows": t["shadows"]});
        let mine = record(l);
        if mine == want {
            fields.record(&l.path, 0.0);
        } else {
            fields.fail(&l.path, format!("{mine} vs {want}"));
        }
        let mut worst = 0f64;
        for i in 0..4 {
            for j in 0..4 {
                worst = worst.max((l.world[i][j] - t["M"][i][j].as_f64().unwrap()).abs());
            }
        }
        matrices.record(&l.path, worst);
    }
    println!("{fields}\n{matrices}\nnot shining (inactive or disabled): {off}");
    fields.assert_passed();
    matrices.assert_passed();
}

/// The lights lowered: one `core.light` per `unity.light` of a punctual type, its kind, cones, linear colour, intensity
/// and range by the documented conversion, its direction (the transform's local -Z) Unity's forward (+Z) through the
/// frame conversion, bit for bit.
#[test]
fn lights_lower_to_core() {
    use primase_core::{Light, LightKind, Transform, World};
    use primase_unity::components::UnityLight;
    use primase_unity::level::{UNITY, read_level, transform_of};
    use primase_unity::lower_lights::lower_unity_lights;
    let (Some(data), Some(tpk), Some(scan)) = (
        real_data("PRIMASE_UNITY_DATA"),
        real_data("PRIMASE_UNITY_TPK"),
        real_data("PRIMASE_UNITY_SCAN"),
    ) else {
        return;
    };
    let scan: Json = serde_json::from_slice(&std::fs::read(&scan).unwrap()).unwrap();
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    read_level(&player, scan["level"].as_str().unwrap(), &mut w).unwrap();
    let log = lower_unity_lights(&mut w).unwrap();
    println!("light lowering: {:?}; {:?}", log.counts, log.approximations);
    assert_eq!(w.verify_components(), Vec::new());
    let scene = Scene::read(&player, scan["level"].as_str().unwrap()).unwrap();
    let (stored, _) = scene.lights(&player).unwrap();
    let mut gate = GateReport::new(
        "lights lowered: kind, cones, colour, intensity, range, direction",
        Tolerance::Exact,
    );
    let mut lowered = 0;
    for (e, u) in w.iter::<UnityLight>() {
        let s = stored
            .iter()
            .find(|s| s.light.path_id == u.light.path_id)
            .unwrap();
        let Some(l) = w.get::<Light>(e) else {
            assert!(u.kind > 2, "a punctual light not lowered");
            continue;
        };
        lowered += 1;
        let mut problems = Vec::new();
        let half = |deg: f32| (f64::from(deg) / 2.0).to_radians() as f32;
        let want_kind = match u.kind {
            0 => LightKind::Spot {
                inner_cone: half(u.inner_spot_angle).min(half(u.spot_angle)),
                outer_cone: half(u.spot_angle).min(std::f32::consts::FRAC_PI_2),
            },
            1 => LightKind::Directional,
            _ => LightKind::Point,
        };
        if l.kind != want_kind {
            problems.push(format!("kind {:?} vs {want_kind:?}", l.kind));
        }
        let srgb = |c: f32| {
            let c = f64::from(c);
            (if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }) as f32
        };
        if l.color != [0, 1, 2].map(|k| srgb(u.color[k])) || l.intensity != u.intensity {
            problems.push(format!("colour {:?} intensity {}", l.color, l.intensity));
        }
        if l.range != (u.kind != 1).then_some(u.range) {
            problems.push(format!("range {:?}", l.range));
        }
        // direction: the light's local -Z in the world, against Unity's local +Z through the frame
        let t: &Transform = w.get(e).unwrap();
        let ours = t.cols[2].map(|x| -x);
        let unity = transform_of(&s.world).cols[2];
        if ours.map(f32::to_bits) != UNITY.direction(unity).map(f32::to_bits) {
            problems.push(format!("direction {ours:?} vs {:?}", UNITY.direction(unity)));
        }
        if problems.is_empty() {
            gate.record(&s.path, 0.0);
        } else {
            gate.fail(&s.path, problems.join("; "));
        }
    }
    println!("{gate}");
    assert_eq!(lowered, stored.len());
    gate.assert_passed();
}
