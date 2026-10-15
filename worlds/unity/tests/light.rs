//! Light gate against an independent reader's scan (`PRIMASE_UNITY_SCAN`: `lights`, the shining lights other than the
//! sun, and `sun`, the shining directional light; each `path`, `type`, `color`, `intensity`, `range`, `spot_angle`,
//! `inner_spot_angle`, `shadows`, `M` rounded to 6 decimals), for the player `PRIMASE_UNITY_DATA` read with
//! `PRIMASE_UNITY_TPK`: every light's fields exactly, its world matrix within the scan's rounding (5e-7).

#![allow(clippy::unwrap_used)]

use primase_core::gate::{GateReport, Tolerance, real_data};
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
