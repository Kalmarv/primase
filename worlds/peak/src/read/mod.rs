//! PEAK's reader: what its scripts hand every shader, on top of the Unity level read ([`read_globals`]).
pub mod light_volume;
pub mod scripts;

use std::collections::BTreeSet;
use std::sync::Arc;

use primase_core::{Name, TextureLayers, World};
use primase_unity::components::{GlobalTexture, GlobalValue, UnityShaderGlobals};
use primase_unity::level::texture_entity;
use unity_formats::player::{ObjRef, Player};
use unity_formats::scene::{Scene, UnityBehaviour};
use unity_formats::value::Value;

use crate::Result;

/// The global `Peak.FogShaderParams.Start` sets.
pub const FOG_TEXTURE: &str = "FogTexture";
/// The global texture `LightVolume.Start` sets: its baked light volume.
pub const LIGHT_MAP: &str = "_LightMap";
/// The values `LightVolume.Start` sets with it (`SetShaderVars`).
pub const LIGHT_VOLUME_VALUES: [&str; 6] = [
    "_Brightness",
    "_AmbienceStrength",
    "_AmbienceMin",
    "_GridRes",
    "_RaySpacing",
    "_GridOffset",
];

/// The global `FogConfig.Start` sets (to 0) and its `SetFog` raises.
pub const WEATHER_BLEND: &str = "_WeatherBlend";
/// The globals only `FogConfig.SetFog` sets, which runs only while the observed character is inside a storm zone with
/// its wind on (`StormVisual.LateUpdate`).
pub const WIND: [&str; 11] = [
    "_WindTexture",
    "WindTint",
    "WindSkyBrightnessValue",
    "WindTextureInfluence",
    "WindFogDensity",
    "WindFogTextureDensity",
    "WindMixInfluence",
    "WindSpeed",
    "WindRotationAxis",
    "WindRotationAngle",
    "WindSphereScale",
];

/// What [`read_globals`] read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalsReport {
    /// `Peak.FogShaderParams` that run: enabled, on a GameObject active with every one above it.
    pub fog_shader_params: usize,
    /// Those stored off (disabled, or on an inactive GameObject): their `Start` runs only if the game turns them on.
    pub fog_shader_params_off: usize,
    /// `LightVolume`s that run, as [`GlobalsReport::fog_shader_params`].
    pub light_volumes: usize,
    /// `LightVolume`s stored off.
    pub light_volumes_off: usize,
    /// `FogConfig`s that run.
    pub fog_configs: usize,
    /// `FogConfig`s stored off.
    pub fog_configs_off: usize,
}

/// A stored reference as the value [`Player::resolve`] reads.
pub(crate) fn pptr_value(p: scripts::PPtr) -> Value {
    Value::Struct(vec![
        (Arc::from("m_FileID"), Value::Int(i64::from(p.file_id))),
        (Arc::from("m_PathID"), Value::Int(p.path_id)),
    ])
}

/// The scene's behaviours of the script `script`, each with whether it runs (enabled, its GameObject and every one
/// above it active).
fn of_script<'a>(
    scene: &Scene,
    behaviours: &'a [(i64, UnityBehaviour)],
    script: &str,
) -> Vec<(&'a UnityBehaviour, bool)> {
    behaviours
        .iter()
        .filter(|(_, b)| b.script == script)
        .map(|(go, b)| {
            let active = scene
                .ancestors(*go)
                .iter()
                .all(|g| scene.game_objects.get(g).is_some_and(|g| g.active));
            (b, b.enabled && active)
        })
        .collect()
}

/// The globals PEAK's scripts hand every shader in scene `level` of `player`, as their stored fields state them, into
/// `w`'s `unity.shader_globals`, after [`primase_unity::level::read_level`] read the level into the same world (the
/// globals already there are kept). Script fields are decoded by [`scripts`]' named layouts; another layout is an
/// error. Where the stored scene does not decide a global (several of its scripts run with different values, and
/// which `Start` runs last is not stated; the one that runs sets no texture; every one is stored off), it is named
/// unset, with why; one no script sets when the level loads is named never set, with why.
///
/// - `FogTexture`: the `fogTexture` of the scene's `Peak.FogShaderParams` that runs, which its `Start` sets: the
///   Texture2D's entity, the one the level read made where a material samples it too.
/// - `_LightMap` and [`LIGHT_VOLUME_VALUES`]: what the `Start` of the scene's `LightVolume` that runs sets when it holds
///   a compressed light map (both slice arrays non-empty; otherwise it sets nothing): the volume
///   [`light_volume::assemble`] builds, as an entity named `LightVolumeBakeTexture` with `core.texture_layers` (made
///   once: a second read keeps the entity), `brightness`, `ambienceStrength`, `ambienceMin` and `raySpacing`, and the
///   light map's `gridRes` and `gridOffset` (which `Start` copies into the component first) as four floats, the last
///   0.
/// - `_WeatherBlend`: 0, which the `Start` of every `FogConfig` that runs sets.
/// - [`WIND`]: never set when the level loads: only `FogConfig.SetFog` sets them, from runtime state (the character
///   inside a storm zone whose wind is on), and no other script names them.
pub fn read_globals(player: &Player, level: &str, w: &mut World) -> Result<GlobalsReport> {
    let scene = Scene::read(player, level)?;
    let version = player
        .files
        .get(&scene.file)
        .map_or("", |f| f.unity_version.as_str())
        .to_owned();
    let behaviours = scene.behaviours(player)?;
    let mut report = GlobalsReport::default();
    let mut globals = w.resource::<UnityShaderGlobals>().cloned().unwrap_or_default();

    // the FogShaderParams that run, and the texture each names
    let mut running: Vec<(ObjRef, Option<ObjRef>)> = Vec::new();
    for (b, runs) in of_script(&scene, &behaviours, "Peak.FogShaderParams") {
        let field = scripts::fog_shader_params(&version, &b.data)?;
        if !runs {
            report.fog_shader_params_off += 1;
            continue;
        }
        report.fog_shader_params += 1;
        let texture = player.resolve(&b.behaviour.file, &pptr_value(field))?;
        running.push((b.behaviour.clone(), texture));
    }
    let textures: BTreeSet<&Option<ObjRef>> = running.iter().map(|(_, t)| t).collect();
    match (running.first(), textures.len()) {
        (None, _) if report.fog_shader_params_off > 0 => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "the scene's {} Peak.FogShaderParams are stored off: their Start sets it only if the game turns one on",
                    report.fog_shader_params_off
                ),
            );
        }
        (None, _) => {}
        (Some((by, Some(texture))), 1) => {
            let entity = texture_entity(player, texture, w)?;
            globals.unset.remove(FOG_TEXTURE);
            globals.textures.insert(
                FOG_TEXTURE.into(),
                GlobalTexture {
                    texture: entity,
                    set_by: format!("Peak.FogShaderParams.fogTexture of {}/{}", by.file, by.path_id),
                },
            );
        }
        (Some((by, None)), 1) => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "Peak.FogShaderParams of {}/{} sets it to no texture",
                    by.file, by.path_id
                ),
            );
        }
        (Some(_), n) => {
            globals.unset.insert(
                FOG_TEXTURE.into(),
                format!(
                    "{} Peak.FogShaderParams that run set {n} different textures: which Start runs last is not stated",
                    running.len()
                ),
            );
        }
    }

    // the LightVolumes that run, each with its stored fields
    let mut volumes: Vec<(&UnityBehaviour, scripts::LightVolume)> = Vec::new();
    for (b, runs) in of_script(&scene, &behaviours, "LightVolume") {
        let fields = scripts::light_volume(&version, &b.data)?;
        if runs {
            report.light_volumes += 1;
            volumes.push((b, fields));
        } else {
            report.light_volumes_off += 1;
        }
    }
    let unset = |globals: &mut UnityShaderGlobals, why: String| {
        for name in std::iter::once(LIGHT_MAP).chain(LIGHT_VOLUME_VALUES) {
            globals.unset.insert(name.into(), why.clone());
        }
    };
    match volumes.as_slice() {
        [] if report.light_volumes_off > 0 => unset(
            &mut globals,
            format!(
                "the scene's {} LightVolumes are stored off: their Start sets it only if the game turns one on",
                report.light_volumes_off
            ),
        ),
        [] => {}
        [(b, v)] if v.light_map.rgb_slices.is_empty() || v.light_map.alpha_slices.is_empty() => {
            let why = format!(
                "the LightVolume of {}/{} holds no compressed light map: its Start sets nothing",
                b.behaviour.file, b.behaviour.path_id
            );
            for name in std::iter::once(LIGHT_MAP).chain(LIGHT_VOLUME_VALUES) {
                globals.never_set.insert(name.into(), why.clone());
            }
        }
        [(b, v)] => {
            let at = format!("{}/{}", b.behaviour.file, b.behaviour.path_id);
            let set_by = format!("LightVolume.lightMapData of {at}");
            let kept = globals
                .textures
                .get(LIGHT_MAP)
                .filter(|g| g.set_by == set_by && w.has::<TextureLayers>(g.texture))
                .map(|g| g.texture);
            let texture = match kept {
                Some(e) => e,
                None => {
                    let volume = light_volume::assemble(player, &b.behaviour.file, &v.light_map)?;
                    let e = w.spawn();
                    w.insert(e, Name("LightVolumeBakeTexture".into()));
                    w.insert(e, volume);
                    e
                }
            };
            globals.unset.remove(LIGHT_MAP);
            globals
                .textures
                .insert(LIGHT_MAP.into(), GlobalTexture { texture, set_by });
            let [x, y, z] = v.light_map.grid_res;
            let [ox, oy, oz] = v.light_map.grid_offset;
            let values: [(&str, Vec<f32>, &str); 6] = [
                ("_Brightness", vec![v.brightness], "brightness"),
                ("_AmbienceStrength", vec![v.ambience_strength], "ambienceStrength"),
                ("_AmbienceMin", vec![v.ambience_min], "ambienceMin"),
                // `assemble` held the light map's gridRes to positive sizes
                (
                    "_GridRes",
                    vec![x as f32, y as f32, z as f32, 0.0],
                    "lightMapData.gridRes",
                ),
                ("_RaySpacing", vec![v.ray_spacing], "raySpacing"),
                ("_GridOffset", vec![ox, oy, oz, 0.0], "lightMapData.gridOffset"),
            ];
            for (name, floats, field) in values {
                globals.unset.remove(name);
                globals.values.insert(
                    name.into(),
                    GlobalValue {
                        words: floats.into_iter().map(f32::to_bits).collect(),
                        set_by: format!("LightVolume.{field} of {at}"),
                    },
                );
            }
        }
        several => unset(
            &mut globals,
            format!(
                "{} LightVolumes run: which Start runs last is not stated",
                several.len()
            ),
        ),
    }
    // the FogConfigs: Start sets _WeatherBlend to 0, SetFog the wind only later
    let mut first_running = None;
    for (b, runs) in of_script(&scene, &behaviours, "FogConfig") {
        scripts::fog_config(&version, &b.data)?;
        if runs {
            report.fog_configs += 1;
            first_running.get_or_insert(&b.behaviour);
        } else {
            report.fog_configs_off += 1;
        }
    }
    match first_running {
        Some(by) => {
            globals.unset.remove(WEATHER_BLEND);
            globals.values.insert(
                WEATHER_BLEND.into(),
                GlobalValue {
                    words: vec![0f32.to_bits()],
                    set_by: format!(
                        "FogConfig.Start of {}/{} (each of the scene's {} FogConfigs that run sets 0)",
                        by.file, by.path_id, report.fog_configs
                    ),
                },
            );
        }
        None if report.fog_configs_off > 0 => {
            globals.unset.insert(
                WEATHER_BLEND.into(),
                format!(
                    "the scene's {} FogConfigs are stored off: their Start sets it only if the game turns one on",
                    report.fog_configs_off
                ),
            );
        }
        None => {
            globals.never_set.insert(
                WEATHER_BLEND.into(),
                "the scene holds no FogConfig, the only script that sets it".into(),
            );
        }
    }
    let why = if report.fog_configs + report.fog_configs_off > 0 {
        "only FogConfig.SetFog sets it, while the observed character is inside a storm zone whose wind is on \
         (StormVisual.LateUpdate): no script sets it when the level loads"
    } else {
        "the scene holds no FogConfig, the only script that sets it"
    };
    for name in WIND {
        globals.never_set.insert(name.into(), why.into());
    }
    w.insert_resource(globals);
    Ok(report)
}
