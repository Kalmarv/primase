//! `lower-shred-fog` on hand-written level parameters: the colour and the distances in metres, the cases that give no
//! fog, the refusals, and the approximations named in the report.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Atmosphere, FogFalloff, World};
use primase_shred::components::{LevelParams, Stored};
use rme_formats::level_params::{ScriptCall, ScriptValue};

fn call(function: &str, args: &[f64]) -> ScriptCall {
    ScriptCall {
        function: function.into(),
        args: args.iter().map(|n| ScriptValue::Number(*n)).collect(),
    }
}

fn params(calls: Vec<ScriptCall>) -> LevelParams {
    Stored(rme_formats::level_params::LevelParams {
        level: "zulu".into(),
        script: r"script\vfx_level_load_zulu.lua".into(),
        calls,
    })
}

fn run(w: &mut World, scale: &str) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    let pipeline = registry
        .parse(&format!("lower-shred-fog{{metres-per-unit={scale}}}"))
        .map_err(|e| e.to_string())?;
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn world(calls: Vec<ScriptCall>) -> World {
    let mut w = World::new();
    w.insert_resource(params(calls));
    w
}

const FOG: [f64; 9] = [400.0, 700.0, -10.0, 400.0, 0.5, 0.25, 0.125, 0.5, 1.0];

#[test]
fn linear_fog_in_metres_with_the_stored_colour() {
    let mut w = world(vec![call("change_fog", &FOG), call("enable_fog", &[])]);
    let report = run(&mut w, "0.5").unwrap();
    let fog = w.resource::<Atmosphere>().unwrap().fog.clone().unwrap();
    assert_eq!(fog.colour, [0.5, 0.25, 0.125]);
    assert_eq!(
        fog.falloff,
        FogFalloff::Linear {
            start: 200.0,
            end: 350.0
        }
    );
    let text = format!("{report:?}");
    for named in [
        "opacity a8 0.5 dropped",
        "S(tD)",
        "height window dropped",
        "radial",
        "as stored",
    ] {
        assert!(text.contains(named), "{named} missing from {text}");
    }
}

#[test]
fn full_opacity_names_no_opacity_gap() {
    let mut fog = FOG;
    fog[7] = 1.0;
    let mut w = world(vec![call("change_fog", &fog), call("enable_fog", &[])]);
    let report = run(&mut w, "1").unwrap();
    assert!(w.resource::<Atmosphere>().is_some());
    assert!(!format!("{report:?}").contains("opacity a8 1 dropped"));
}

#[test]
fn no_fog_when_opacity_zero_not_enabled_or_not_set() {
    let mut zero = FOG;
    zero[7] = 0.0;
    for calls in [
        vec![call("change_fog", &zero), call("enable_fog", &[])],
        vec![call("change_fog", &FOG)],
        vec![call("enable_fog", &[])],
        vec![],
    ] {
        let mut w = world(calls);
        run(&mut w, "1").unwrap();
        assert!(w.resource::<Atmosphere>().is_none());
    }
}

#[test]
fn refuses_what_it_cannot_read() {
    let enable = call("enable_fog", &[]);
    let mut backwards = FOG;
    backwards[0] = 800.0;
    let mut negative = FOG;
    negative[0] = -1.0;
    let mut opaque = FOG;
    opaque[7] = 1.5;
    let cases = [
        (
            vec![call("change_fog", &FOG[..8]), enable.clone()],
            "takes 9 arguments",
        ),
        (
            vec![call("change_fog", &FOG), call("change_fog", &FOG), enable.clone()],
            "2 calls to change_fog",
        ),
        (
            vec![call("change_fog", &backwards), enable.clone()],
            "0 <= a1 < a2",
        ),
        (
            vec![call("change_fog", &negative), enable.clone()],
            "0 <= a1 < a2",
        ),
        (vec![call("change_fog", &opaque), enable.clone()], "not in 0..=1"),
        (
            vec![
                ScriptCall {
                    function: "change_fog".into(),
                    args: vec![ScriptValue::Nil; 9],
                },
                enable,
            ],
            "is not a number",
        ),
    ];
    for (calls, why) in cases {
        let mut w = world(calls);
        let e = run(&mut w, "1").unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
}

#[test]
fn an_existing_atmosphere_is_kept_and_the_unit_is_checked() {
    let mut w = world(vec![call("change_fog", &FOG), call("enable_fog", &[])]);
    w.insert_resource(Atmosphere {
        fog: None,
        exposure: None,
        volumetric: None,
    });
    run(&mut w, "1").unwrap();
    assert!(w.resource::<Atmosphere>().unwrap().fog.is_none());
    for bad in ["0", "-1", "x", "inf"] {
        let mut w = world(vec![]);
        assert!(run(&mut w, bad).is_err(), "{bad}");
    }
}

#[test]
fn a_world_without_level_parameters_skips_the_pass() {
    let mut w = World::new();
    let report = run(&mut w, "1").unwrap();
    assert!(w.resource::<Atmosphere>().is_none());
    assert!(format!("{report:?}").to_lowercase().contains("skip"));
}
