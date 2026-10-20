//! Sledding Game's lodge: its scene's stored ambient lowered by `lower-unity-ambient`, for the player
//! `PRIMASE_UNITY_DATA` read with `PRIMASE_UNITY_TPK`. The lodge is not a scene of its own: it stands in
//! `Main Mountain Scene` (found by name in BuildSettings' scene list, `levelN` by its index), and RenderSettings'
//! ambient is one per scene, so the lodge shares the mountain's. Whether the game varies ambient inside the lodge (a
//! volume of its own) is a separate question this does not answer. Per stored mode: trilight lowers to the gradient
//! of the stored colours made linear, flat to the sky colour, skybox and custom to nothing (omitted), the stored
//! gradient (an approximation) or the stored probe's average (flat, an approximation; none for an all-zero probe), as
//! the program chooses. A player without that scene (another game) is skipped.
//!
//! As read (Sledding Game on Unity 6000.3.12f1): skybox (mode 0), its stored colours Unity's defaults (reported
//! under `stored-gradient`), its probe's average 0.1792, 0.2244, 0.3062 (Unity's `m_IndirectSpecularColor`):
//! `probe=probe-average` is the choice that lights the lodge as the game does on average.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog};
use primase_core::{Ambient, AmbientLight, Environment, World};
use primase_image::srgb_to_linear_f32;
use primase_testdata::real_data;
use primase_unity::TypeTreePackage;
use primase_unity::components::UnityRenderSettings;
use primase_unity::lower_ambient::{LowerUnityAmbient, UnityAmbientProbe, probe_average};
use primase_unity::player::{ObjRef, Player};
use primase_unity::scene::Scene;
use primase_unity::value::{Array, Value};

const LODGE_SCENE: &str = "Main Mountain Scene";

/// The level file of the build's scene named `name` (its path's file name without `.unity`).
fn level_of(player: &Player, name: &str) -> Option<String> {
    let file = "globalgamemanagers";
    let settings = player
        .file(file)
        .unwrap()
        .objects
        .iter()
        .map(|o| ObjRef {
            file: file.to_owned(),
            path_id: o.path_id,
        })
        .find(|r| player.class_name(r) == Some("BuildSettings"))
        .expect("globalgamemanagers holds BuildSettings");
    let settings = player.decode(&settings).unwrap();
    let Some(Array::Values(scenes)) = settings.get("scenes").and_then(Value::as_array) else {
        panic!("BuildSettings without a scene list");
    };
    let paths: Vec<&str> = scenes.iter().map(|s| s.as_str().unwrap()).collect();
    println!("scenes: {paths:?}");
    paths
        .iter()
        .position(|p| p.rsplit('/').next() == Some(&format!("{name}.unity")))
        .map(|i| format!("level{i}"))
}

#[test]
fn the_lodges_ambient_lowers_by_its_stored_mode() {
    let (Some(data), Some(tpk)) = (real_data("PRIMASE_UNITY_DATA"), real_data("PRIMASE_UNITY_TPK")) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let Some(level) = level_of(&player, LODGE_SCENE) else {
        println!("SKIPPED: no scene {LODGE_SCENE:?} in this player (not Sledding Game)");
        return;
    };
    let scene = Scene::read(&player, &level).unwrap();
    let stored = scene
        .render_settings(&player)
        .unwrap()
        .expect("the scene has RenderSettings")
        .ambient
        .expect("its RenderSettings store an ambient mode");
    println!("{LODGE_SCENE} ({level}) stores {stored:?}");
    let lower = |probe| {
        let mut w = World::new();
        w.insert_resource(UnityRenderSettings {
            sun: None,
            pipeline: None,
            ambient: Some(stored.clone()),
        });
        let mut log = PassLog::default();
        LowerUnityAmbient { probe }.run(&mut w, &mut log).unwrap();
        assert!(w.verify_resources().is_empty());
        let ambient = w.resource::<Environment>().unwrap().ambient.clone();
        (ambient, log)
    };
    let lin = |c: [f32; 4]| [c[0], c[1], c[2]].map(srgb_to_linear_f32);
    let gradient = Ambient {
        intensity: stored.intensity,
        light: AmbientLight::Gradient {
            sky: lin(stored.sky),
            equator: lin(stored.equator),
            ground: lin(stored.ground),
        },
    };
    for probe in [
        UnityAmbientProbe::Omit,
        UnityAmbientProbe::StoredGradient,
        UnityAmbientProbe::ProbeAverage,
    ] {
        let (ambient, log) = lower(probe);
        println!(
            "{probe:?}: {ambient:?}; approximated {:?}; omitted {:?}",
            log.approximations, log.omitted
        );
        match (stored.mode, probe) {
            (1, _) => assert_eq!(ambient, Some(gradient.clone())),
            (3, _) => assert_eq!(
                ambient,
                Some(Ambient {
                    intensity: stored.intensity,
                    light: AmbientLight::Flat(lin(stored.sky)),
                })
            ),
            (0 | 4, UnityAmbientProbe::Omit) => {
                assert_eq!(ambient, None);
                assert_eq!(log.omitted.len(), 1);
            }
            (0 | 4, UnityAmbientProbe::StoredGradient) => {
                assert_eq!(ambient, Some(gradient.clone()));
                assert!(log.approximations.keys().any(|k| k.contains("skybox or custom")));
            }
            (0 | 4, UnityAmbientProbe::ProbeAverage) => {
                match stored.probe.filter(|p| p.iter().any(|&c| c != 0.0)) {
                    Some(p) => assert_eq!(
                        ambient,
                        Some(Ambient {
                            intensity: 1.0,
                            light: AmbientLight::Flat(probe_average(&p)),
                        })
                    ),
                    None => assert_eq!(ambient, None),
                }
            }
            (m, _) => panic!("{LODGE_SCENE} stores ambient mode {m}, which Unity does not define"),
        }
    }
}
