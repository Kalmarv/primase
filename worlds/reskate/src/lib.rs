//! ReSkate (skate.): the target's own crate. It owns the `skate.*` dialect ([`dialect`]), the preparation passes a
//! program selects to raise core data into it ([`prepare`], `raise-skate`, and the target's lowerings), the frozen
//! skate-scene interchange v1 ([`interchange`]: read, load, write), and the writer of ReSkate Studio normalized map
//! folders (`map.json`, `meshes/*.glb`, `textures/*`, `ui/*`, `trainer.json`, `stats.json`,
//! `textures_index.json`), gated against the reference C# writer by what Studio reads: meshes, tables and texture
//! payloads exactly, JSON as parsed values, PNGs by pixels.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod assembly;
pub mod blend_order;
pub mod build;
pub mod collision;
mod collision_pack;
pub mod components;
pub mod exact_sum;
pub mod glb;
pub mod grind;
pub mod hash;
pub mod interchange;
pub mod map;
pub mod material;
pub mod model;
pub mod overlay;
pub mod passes;
pub mod pausemap;
pub mod postcompile;
mod read;
pub mod records;
pub mod scene;
pub mod texture;
mod write;

/// The `skate.*` dialect by its former path: the components programs name, and the dialect's lowerings back to core.
pub mod dialect {
    pub use crate::components::{
        MarkerKind, MaterialClass, Part, SkateLight, SkateMarker, SkateMaterial, SkateParts, SkateScene,
        Snap, SnapMode,
    };
    pub use crate::interchange::soup::soup;
    pub use crate::passes::lower::{LowerSkateLights, LowerSkatePbr};
}

/// The preparation passes by their former path ([`crate::passes`]).
pub mod prepare {
    pub use crate::passes::{
        BudgetReskateTextures, ClipReskateRegion, OceanMaterials, PauseExtent, PauseSource,
        PlaceReskateSpawn, PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateFloor,
        PrepareReskateGrinds, PrepareReskateOverlays, PrepareReskatePauseMap, PrepareReskateTextures,
        PrepareReskateWater, RaiseOptions, RaiseSkate, ReskateSpawnAt, SKATE_LIGHT_SCALE,
        SelectReskateLights, SkatePauseImage, SkatePausePicture, SnapReskateMarkers, WaterExtent,
        budget_reskate_textures, class_to_skate, clip_reskate_region, place_reskate_spawn,
        plan_reskate_blend_budget, prepare_reskate_blend_order, prepare_reskate_floor,
        prepare_reskate_grinds, prepare_reskate_overlays, prepare_reskate_pause_map, prepare_reskate_water,
        raise_skate, raise_skate_pass, select_reskate_lights, snap_reskate_markers, studio_watts,
        tint_pixels,
    };
}

/// The summaries a map converted in parts merges over its parts, and the decisions made from them
/// ([`primase_core::convert::convert_parts_summarized`]).
pub fn register_summaries(reg: &mut primase_core::summary::SummaryRegistry) {
    reg.summary::<components::SkateFloorSummary>();
    reg.summary::<components::SkateLightsSummary>();
    reg.summary::<components::SkateTextureWeights>();
    reg.summary::<components::SkateMapSummary>();
    reg.summary::<components::SkateContentBounds>();
    reg.summary::<components::SkateSpawnSummary>();
    reg.summary::<components::SkateInstanceCounts>();
    reg.summary::<components::SkateWaterSurfaces>();
    reg.decision::<components::SkateFloorDecision>();
}

/// `lower-layered-reskate` and its components by their former path.
pub mod layered {
    pub use crate::passes::layered::{LowerLayeredReskate, OverlayLimit};
}

/// `lower-tint-reskate` by its former path.
pub mod tint {
    pub use crate::passes::tint::LowerTintReskate;
}

/// `lower-vertex-alpha-reskate` by its former path.
pub mod vertex_alpha {
    pub use crate::passes::vertex_alpha::LowerVertexAlphaReskate;
}

use std::path::PathBuf;

pub use assembly::{AssembledMaterial, EncodingOptions, assembled_materials};
pub use build::{BuildReport, MapSink, leave_out_optional, lod_choice, select_lod, target, write_map};
/// A skate-scene folder as the convert loop's reader.
pub use read::scene::SceneReader;

/// The passes of the ReSkate target, by name: raising core data into the dialect, the target's lowerings, and the
/// dialect's lowerings back to core.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    // the scene's name and the lights' calibration are the program's: no default
    reg.register_requiring(
        "raise-skate",
        passes::raise_skate_pass,
        &[("name", "n"), ("light_scale", "standard")],
    );
    passes::lower::register(reg);
    // where the spawn goes is the program's: no default
    reg.register_requiring(
        "place-reskate-spawn",
        passes::place_reskate_spawn,
        &[("at", "centre")],
    );
    reg.register("snap-reskate-markers", passes::snap_reskate_markers);
    reg.register("prepare-reskate-grinds", passes::prepare_reskate_grinds);
    // the step is the program's: no default
    reg.register_requiring(
        "prepare-reskate-overlays",
        passes::prepare_reskate_overlays,
        &[("step", "0")],
    );
    // which water a map draws and how far is the program's: no default
    reg.register_requiring(
        "prepare-reskate-water",
        passes::prepare_reskate_water,
        &[("extent", "none")],
    );
    // the pause-map picture, and its size and extent, are the program's: no default
    reg.register_requiring(
        "prepare-reskate-pause-map",
        passes::prepare_reskate_pause_map,
        &[("from", "geometry"), ("height", "9"), ("extent", "content")],
    );
    // a light cap and its grouping are the program's: no default
    reg.register_requiring(
        "select-reskate-lights",
        passes::select_reskate_lights,
        &[("per_cell", "1"), ("cell", "200")],
    );
    // what a map does with triangles over more texture repeats than Studio draws is the program's: no default
    reg.register_requiring(
        "prepare-reskate-uv-repeats",
        passes::prepare_reskate_uv_repeats,
        &[("decals", "omit"), ("blended", "cutout")],
    );
    // how the collision is written is the program's: no default
    reg.register_requiring(
        "prepare-reskate-collision",
        passes::prepare_reskate_collision,
        &[("surfaces", "coalesce"), ("weld", "mm")],
    );
    // the map's summary, for the encoding given (the writer's defaults unless stated)
    reg.register("summarize-reskate-map", passes::summarize_reskate_map);
    // the budget is the program's: no default
    reg.register_requiring(
        "plan-reskate-blend-budget",
        passes::plan_reskate_blend_budget,
        &[("objects", "pool")],
    );
    // the grid is the program's: no default
    reg.register_requiring(
        "prepare-reskate-blend-order",
        passes::prepare_reskate_blend_order,
        &[("grid", "0")],
    );
    // the floor is the program's: no default
    reg.register_requiring(
        "prepare-reskate-floor",
        passes::prepare_reskate_floor,
        &[("min_height", "0"), ("stub_below", "0")],
    );
    // a map in parts: the marker snaps' inputs over every part (which prototypes are instanced, the water)
    reg.register("summarize-reskate-instances", passes::summarize_reskate_instances);
    reg.register("summarize-reskate-water", passes::summarize_reskate_water);
    // a map in parts: its spawn placed once, from what every part holds
    reg.register("summarize-reskate-spawn", passes::summarize_reskate_spawn);
    // a map in parts: its content's extent measured over every part
    reg.register("summarize-reskate-content", passes::summarize_reskate_content);
    // a map in parts: its textures weighed over every part (with budget-reskate-textures's budget)
    reg.register_requiring(
        "summarize-reskate-textures",
        passes::summarize_reskate_textures,
        &[("max", "1")],
    );
    // a map in parts: its lights capped over every part (with select-reskate-lights's cap)
    reg.register_requiring(
        "summarize-reskate-lights",
        passes::summarize_reskate_lights,
        &[("per_cell", "1"), ("cell", "200")],
    );
    // a map in parts: its floor measured on every part, decided once (with prepare-reskate-floor's settings)
    reg.register("summarize-reskate-floor", passes::summarize_reskate_floor);
    reg.register_requiring(
        "decide-reskate-floor",
        passes::decide_reskate_floor,
        &[("min_height", "0"), ("stub_below", "0")],
    );
    // the region is the program's: no default
    reg.register_requiring(
        "clip-reskate-region",
        passes::clip_reskate_region,
        &[("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1"), ("apron", "0")],
    );
    // the budget is the program's number: no default
    reg.register_requiring(
        "budget-reskate-textures",
        passes::budget_reskate_textures,
        &[("max", "1")],
    );
    reg.register("prepare-reskate-textures", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(passes::PrepareReskateTextures)),
    });
    // how many overlays a triangle may get is the program's: no default
    reg.register_requiring(
        "lower-layered-reskate",
        passes::layered::lower_layered_reskate,
        &[("overlays", "all")],
    );
    reg.register("lower-tint-reskate", |o| {
        Ok(Box::new(passes::tint::LowerTintReskate::from_options(o)?))
    });
    reg.register("lower-vertex-alpha-reskate", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(passes::vertex_alpha::LowerVertexAlphaReskate)),
    });
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}")]
    Scene(String),
    #[error("texture: {0}")]
    Texture(String),
    #[error("not supported yet: {0}")]
    Unsupported(String),
    #[error("{file}: {source}")]
    Json {
        file: String,
        source: primase_core::JsonError,
    },
    /// A scene manifest that is not JSON.
    #[error("{}: {source}", path.display())]
    Manifest {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// A scene table that breaks a rule of the format; the message names the item.
    #[error("{path}: {message}")]
    Table { path: String, message: String },
    #[error("write: {0}")]
    Write(String),
    /// A scene's GLB that the GLB reader refused, or a prototype mesh the encoder did.
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    /// A scene's texture file that the image toolkit refused.
    #[error(transparent)]
    Image(#[from] primase_image::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The JSON text of one of the map's files (`primase_core::json::to_vec`: a non-finite number is an error), every
/// negative zero written as `0.0`: Studio's JSON reader keeps a written `-0.0` as negative zero, and a negative zero
/// in a section's origin changes its compiled mesh (the reference writer's `-0`, an integer, reads as 0).
pub(crate) fn json_bytes<T: serde::Serialize + ?Sized>(file: &str, v: &T, pretty: bool) -> Result<Vec<u8>> {
    let err = |source| Error::Json {
        file: file.to_owned(),
        source,
    };
    let text = primase_core::json::to_vec(v, pretty).map_err(err)?;
    let mut value: serde_json::Value = serde_json::from_slice(&text).expect("our own JSON parses");
    if !unsign_zeros(&mut value) {
        return Ok(text);
    }
    // the numbers parse to the f64 nearest their digits and serialize back to the same digits
    primase_core::json::to_vec(&value, pretty).map_err(err)
}

/// Replace every negative zero in `v` by a positive one; whether there was one.
fn unsign_zeros(v: &mut serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) if n.as_f64().is_some_and(|x| x == 0.0 && x.is_sign_negative()) => {
            *v = serde_json::Value::from(0.0);
            true
        }
        serde_json::Value::Array(items) => items.iter_mut().fold(false, |any, x| unsign_zeros(x) | any),
        serde_json::Value::Object(m) => m.values_mut().fold(false, |any, x| unsign_zeros(x) | any),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct R {
        name: &'static str,
        o: [f32; 3],
        d: f64,
        x: f32,
    }

    /// Negative zeros written unsigned; every other number keeps its digits, strings their text.
    #[test]
    fn json_zeros_are_unsigned() {
        let r = R {
            name: "a -0.0 b",
            o: [-0.0, 0.59155273, 0.1],
            d: -0.0,
            x: -2.5,
        };
        let text = String::from_utf8(json_bytes("t", &r, false).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"name":"a -0.0 b","o":[0.0,0.59155273,0.1],"d":0.0,"x":-2.5}"#
        );
        let plain = R {
            o: [1.0, 2.0, 3.0],
            d: 1.0,
            ..r
        };
        assert_eq!(
            json_bytes("t", &plain, true).unwrap(),
            primase_core::json::to_vec(&plain, true).unwrap()
        );
    }
}
