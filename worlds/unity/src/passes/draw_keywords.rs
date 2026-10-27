//! The keywords a renderer sets for a draw, from the scene's own state (`resolve-unity-shaders{keywords=scene}`).
//!
//! A player stores a program per keyword set; the set a draw has is the renderer's, from the lights, shadows, fog and
//! probes of the frame. The rules here are the ones an editor observed in a probe shader that writes the keywords it was
//! compiled with (primase-oracles `unity/keywords`, editor 6000.6.4f1, Universal Render Pipeline package 17.6.0; the
//! players that read them were built with other versions, which the variants a player stores are checked against).
//!
//! **Built-in pipeline** (forward base pass, `multi_compile_fwdbase` and `multi_compile_fog`):
//! - `LIGHTPROBE_SH` on every draw that is not lightmapped (the light probe usage of the renderer, and the ambient mode,
//!   do not change it; the scene's lightmaps are not read, so every renderer is taken as not lightmapped).
//! - `SHADOWS_SCREEN` when the main directional light casts shadows, the quality level draws shadows, and the renderer
//!   receives them.
//! - `FOG_LINEAR`, `FOG_EXP` or `FOG_EXP2` when the scene's fog is on, by its mode.
//! - `DIRECTIONAL` on every draw whatever the lights (the variants a player stores all have it: the pass resolves it as
//!   an implied keyword).
//!
//! **Universal Render Pipeline** (forward; the keyword groups the pipeline sets globally per frame):
//! - Main light shadows, when the asset supports them and lights the main light per pixel: `_MAIN_LIGHT_SHADOWS_CASCADE`
//!   when the main directional light casts shadows (within the asset's shadow distance) and the asset has more than one
//!   cascade, else `_MAIN_LIGHT_SHADOWS` (the pipeline sets it for an empty shadow map too). `_SHADOWS_SOFT` when the
//!   asset supports soft shadows and that light's shadows are soft.
//! - Additional lights: `_ADDITIONAL_LIGHTS` (per pixel) or `_ADDITIONAL_LIGHTS_VERTEX` (per vertex) by the asset's mode.
//! - `_LIGHT_LAYERS` when the asset supports light layers; `_LIGHT_COOKIES` when it supports cookies and a light has one.
//! - `_REFLECTION_PROBE_BLENDING` and `_REFLECTION_PROBE_BOX_PROJECTION` by the asset's two settings.
//! - Fog as above. Not derived: `_CLUSTER_LIGHT_LOOP` and `_REFLECTION_PROBE_ATLAS` (the renderer's rendering path, which
//!   is not read), `_ADDITIONAL_LIGHT_SHADOWS`, `_SCREEN_SPACE_OCCLUSION` and `_DBUFFER_MRT3` (the frame's lights and the
//!   renderer's features): where every stored variant of a stage has them, the pass takes them as implied.
//!
//! Not derived, so absent: `VERTEXLIGHT_ON` (the lights an object's position puts per vertex), `INSTANCING_ON` (a draw
//! batched with others), the lightmap and shadowmask keywords.
use primase_core::{Entity, World};

use crate::components::{UnityLight, UnityRenderSettings, UnityRenderer, UnityShadowQuality};
use unity_formats::pipeline::UnityUrpSettings;
use unity_formats::player::RenderPipeline;
use unity_formats::scene::UnityFog;

/// Unity's `LightType::Directional`.
const DIRECTIONAL: i64 = 1;

/// The pipeline the scene draws with, as far as the keywords depend on it.
#[derive(Clone, Debug, PartialEq)]
enum Pipeline {
    BuiltIn,
    Universal(Box<UnityUrpSettings>),
    /// Another pipeline, or a Universal one whose asset was not read.
    Unknown,
}

/// What the scene states about its draws' keywords: see the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneKeywords {
    pipeline: Pipeline,
    fog: Option<UnityFog>,
    quality: Option<UnityShadowQuality>,
    /// The main directional light's shadow setting (`LightShadows`: 0 none, 1 hard, 2 soft), where the scene has one.
    main_light_shadows: Option<i64>,
    /// Whether any light has a cookie.
    cookies: bool,
}

impl SceneKeywords {
    /// The scene's state from `unity.render_settings` and the lights: the main light is the sun RenderSettings names,
    /// else the brightest directional light.
    pub fn read(world: &World) -> SceneKeywords {
        let settings = world.resource::<UnityRenderSettings>();
        let pipeline = match settings.and_then(|s| s.pipeline.as_ref()) {
            Some(RenderPipeline::BuiltIn) => Pipeline::BuiltIn,
            Some(RenderPipeline::Universal) => settings
                .and_then(|s| s.urp.clone())
                .map_or(Pipeline::Unknown, |u| Pipeline::Universal(Box::new(u))),
            _ => Pipeline::Unknown,
        };
        let sun: Option<Entity> = settings.and_then(|s| s.sun);
        let light = sun
            .and_then(|e| world.get::<UnityLight>(e))
            .filter(|l| l.kind == DIRECTIONAL)
            .or_else(|| {
                world
                    .iter::<UnityLight>()
                    .map(|(_, l)| l)
                    .filter(|l| l.kind == DIRECTIONAL)
                    .max_by(|a, b| a.intensity.total_cmp(&b.intensity))
            });
        SceneKeywords {
            pipeline,
            fog: settings.and_then(|s| s.fog.clone()).filter(|f| f.enabled),
            quality: settings.and_then(|s| s.shadow_quality.clone()),
            main_light_shadows: light.map(|l| l.shadows),
            cookies: world.iter::<UnityLight>().any(|(_, l)| l.cookie.is_some()),
        }
    }

    /// Whether the scene states enough to derive keywords: its pipeline is one with rules here.
    pub fn known(&self) -> bool {
        self.pipeline != Pipeline::Unknown
    }

    /// The keywords of a draw by a renderer that does (`receives_shadows`) or does not receive shadows.
    pub fn for_draw(&self, receives_shadows: bool) -> Vec<&'static str> {
        let mut out = Vec::new();
        let shadows_drawn = self.quality.as_ref().is_some_and(|q| q.shadows != 0);
        let main_light_casts = self.main_light_shadows.is_some_and(|s| s != 0);
        match &self.pipeline {
            Pipeline::BuiltIn => {
                out.push("LIGHTPROBE_SH");
                if main_light_casts && shadows_drawn && receives_shadows {
                    out.push("SHADOWS_SCREEN");
                }
            }
            Pipeline::Universal(u) => {
                if u.main_light_shadows && u.main_light_mode == 1 {
                    let casting = main_light_casts && u.shadow_distance > 0.0;
                    out.push(if casting && u.shadow_cascades > 1 {
                        "_MAIN_LIGHT_SHADOWS_CASCADE"
                    } else {
                        "_MAIN_LIGHT_SHADOWS"
                    });
                    if u.soft_shadows && casting && self.main_light_shadows == Some(2) {
                        out.push("_SHADOWS_SOFT");
                    }
                }
                match u.additional_lights_mode {
                    1 => out.push("_ADDITIONAL_LIGHTS"),
                    2 => out.push("_ADDITIONAL_LIGHTS_VERTEX"),
                    _ => {}
                }
                if u.light_layers {
                    out.push("_LIGHT_LAYERS");
                }
                if u.light_cookies && self.cookies {
                    out.push("_LIGHT_COOKIES");
                }
                if u.reflection_probe_blending {
                    out.push("_REFLECTION_PROBE_BLENDING");
                }
                if u.reflection_probe_box_projection {
                    out.push("_REFLECTION_PROBE_BOX_PROJECTION");
                }
            }
            Pipeline::Unknown => {}
        }
        if let Some(fog) = &self.fog {
            match fog.mode {
                1 => out.push("FOG_LINEAR"),
                2 => out.push("FOG_EXP"),
                3 => out.push("FOG_EXP2"),
                _ => {}
            }
        }
        out
    }
}

/// The renderers' shadow reception per material, keyed by the material's object (file, path id): how many of the
/// renderers drawing it receive shadows and how many do not (a renderer that does not state it receives, as Unity's
/// default does).
pub fn receivers(world: &World) -> std::collections::BTreeMap<(String, i64), (u64, u64)> {
    let mut out: std::collections::BTreeMap<(String, i64), (u64, u64)> = Default::default();
    for (_, r) in world.iter::<UnityRenderer>() {
        let receives = r.receive_shadows.unwrap_or(true);
        for m in r.materials.iter().flatten() {
            let n = out.entry((m.file.clone(), m.path_id)).or_default();
            if receives {
                n.0 += 1;
            } else {
                n.1 += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urp() -> UnityUrpSettings {
        UnityUrpSettings {
            main_light_mode: 1,
            main_light_shadows: true,
            additional_lights_mode: 1,
            additional_light_shadows: false,
            reflection_probe_blending: false,
            reflection_probe_box_projection: false,
            reflection_probe_atlas: true,
            shadow_distance: 50.0,
            shadow_cascades: 1,
            soft_shadows: false,
            mixed_lighting: true,
            light_cookies: true,
            light_layers: false,
            lod_cross_fade: true,
            sh_eval_mode: 0,
            light_probe_system: 0,
        }
    }

    fn scene(u: UnityUrpSettings, sun: Option<i64>) -> SceneKeywords {
        SceneKeywords {
            pipeline: Pipeline::Universal(Box::new(u)),
            fog: None,
            quality: None,
            main_light_shadows: sun,
            cookies: false,
        }
    }

    /// Each case is one the editor's probe shader observed (primase-oracles `unity/keywords`, URP 17.6.0): the keywords
    /// of a draw by the asset's settings and the sun's shadows (0 none, 1 hard, 2 soft).
    #[test]
    fn universal_draw_keywords_are_the_ones_the_editor_observed() {
        let set = |u: UnityUrpSettings, sun| scene(u, Some(sun)).for_draw(true);
        assert_eq!(set(urp(), 1), ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS"]);
        let cascades = |n| UnityUrpSettings {
            shadow_cascades: n,
            ..urp()
        };
        assert_eq!(
            set(cascades(2), 1),
            ["_MAIN_LIGHT_SHADOWS_CASCADE", "_ADDITIONAL_LIGHTS"]
        );
        assert_eq!(
            set(cascades(4), 1),
            ["_MAIN_LIGHT_SHADOWS_CASCADE", "_ADDITIONAL_LIGHTS"]
        );
        // a sun without shadows, or a shadow distance of 0, still gets the empty shadow map's keyword
        assert_eq!(set(cascades(4), 0), ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS"]);
        let flat = UnityUrpSettings {
            shadow_distance: 0.0,
            ..cascades(4)
        };
        assert_eq!(set(flat, 1), ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS"]);
        // shadows unsupported, or the main light per vertex: none
        let off = UnityUrpSettings {
            main_light_shadows: false,
            ..cascades(4)
        };
        assert_eq!(set(off, 1), ["_ADDITIONAL_LIGHTS"]);
        let vertex = UnityUrpSettings {
            main_light_mode: 2,
            ..cascades(4)
        };
        assert_eq!(set(vertex, 1), ["_ADDITIONAL_LIGHTS"]);
        // soft: the asset and the sun's own shadows
        let soft = UnityUrpSettings {
            soft_shadows: true,
            ..urp()
        };
        assert_eq!(
            set(soft.clone(), 2),
            ["_MAIN_LIGHT_SHADOWS", "_SHADOWS_SOFT", "_ADDITIONAL_LIGHTS"]
        );
        assert_eq!(set(soft, 1), ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS"]);
        assert_eq!(set(urp(), 2), ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS"]);
        // additional lights by mode
        let add = |mode| UnityUrpSettings {
            additional_lights_mode: mode,
            ..urp()
        };
        assert_eq!(
            set(add(2), 1),
            ["_MAIN_LIGHT_SHADOWS", "_ADDITIONAL_LIGHTS_VERTEX"]
        );
        assert_eq!(set(add(0), 1), ["_MAIN_LIGHT_SHADOWS"]);
        // light layers and the reflection probe settings
        let layers = UnityUrpSettings {
            light_layers: true,
            ..urp()
        };
        assert_eq!(set(layers, 1).last(), Some(&"_LIGHT_LAYERS"));
        let probes = UnityUrpSettings {
            reflection_probe_blending: true,
            reflection_probe_box_projection: true,
            ..urp()
        };
        assert_eq!(
            set(probes, 1),
            [
                "_MAIN_LIGHT_SHADOWS",
                "_ADDITIONAL_LIGHTS",
                "_REFLECTION_PROBE_BLENDING",
                "_REFLECTION_PROBE_BOX_PROJECTION"
            ]
        );
        // cookies only where a light has one
        let mut cookie = scene(urp(), Some(1));
        assert!(!cookie.for_draw(true).contains(&"_LIGHT_COOKIES"));
        cookie.cookies = true;
        assert!(cookie.for_draw(true).contains(&"_LIGHT_COOKIES"));
        // fog by mode
        let mut fog = scene(urp(), Some(1));
        fog.fog = Some(UnityFog {
            enabled: true,
            mode: 3,
            color: [0.0; 4],
            density: 0.01,
            start: 0.0,
            end: 0.0,
        });
        assert_eq!(fog.for_draw(true).last(), Some(&"FOG_EXP2"));
    }

    /// The built-in pipeline's forward base pass, as observed: the light probe keyword on every draw, the screen shadows
    /// keyword only where the main light casts, the quality level draws shadows and the renderer receives them.
    #[test]
    fn built_in_draw_keywords_are_the_ones_the_editor_observed() {
        let built_in = |sun, shadows| SceneKeywords {
            pipeline: Pipeline::BuiltIn,
            fog: None,
            quality: Some(UnityShadowQuality {
                shadows,
                distance: 150.0,
                cascades: 4,
            }),
            main_light_shadows: sun,
            cookies: false,
        };
        assert_eq!(built_in(Some(0), 2).for_draw(true), ["LIGHTPROBE_SH"]);
        assert_eq!(built_in(None, 2).for_draw(true), ["LIGHTPROBE_SH"]);
        for sun in [1, 2] {
            assert_eq!(
                built_in(Some(sun), 2).for_draw(true),
                ["LIGHTPROBE_SH", "SHADOWS_SCREEN"]
            );
            // a renderer that does not receive shadows, and a quality level that draws none
            assert_eq!(built_in(Some(sun), 2).for_draw(false), ["LIGHTPROBE_SH"]);
            assert_eq!(built_in(Some(sun), 0).for_draw(true), ["LIGHTPROBE_SH"]);
        }
    }
}
