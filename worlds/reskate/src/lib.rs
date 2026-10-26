//! ReSkate (skate.): the target's own crate. It owns the `skate.*` dialect ([`dialect`]), the preparation passes a
//! program selects to raise core data into it ([`prepare`], `raise-skate`, and the target's lowerings), the frozen
//! skate-scene interchange v1 ([`interchange`], the format; read and loaded by [`read::interchange`], written by
//! [`write::interchange`]), and the writer of ReSkate Studio normalized map
//! folders (`map.json`, `meshes/*.glb`, `textures/*`, `ui/*`, `trainer.json`, `stats.json`,
//! `textures_index.json`), gated against the reference C# writer by what Studio reads: meshes, tables and texture
//! payloads exactly, JSON as parsed values, PNGs by pixels.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod assembly;
pub mod blend_order;
pub mod collision;
mod collision_pack;
pub mod compile;
pub mod components;
pub mod costume;
pub mod decal;
pub mod game;
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
pub mod physics;
pub mod postcompile;
pub mod read;
pub mod records;
pub mod reference;
pub mod scene;
pub mod spool;
pub mod texture;
pub mod write;

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
        BUDGET_RESKATE_TEXTURES, BudgetReskateTextures, CLIP_RESKATE_REGION, ClipReskateRegion,
        OceanMaterials, PLACE_RESKATE_SPAWN, PLAN_RESKATE_BLEND_BUDGET, PREPARE_RESKATE_BLEND_ORDER,
        PREPARE_RESKATE_FLOOR, PREPARE_RESKATE_GRINDS, PREPARE_RESKATE_OVERLAYS, PREPARE_RESKATE_PAUSE_MAP,
        PREPARE_RESKATE_WATER, PauseExtent, PauseSource, PlaceReskateSpawn, PlanReskateBlendBudget,
        PrepareReskateBlendOrder, PrepareReskateFloor, PrepareReskateGrinds, PrepareReskateOverlays,
        PrepareReskatePauseMap, PrepareReskateTextures, PrepareReskateWater, RAISE_SKATE, RaiseOptions,
        RaiseSkate, ReskateSpawnAt, SELECT_RESKATE_LIGHTS, SKATE_LIGHT_SCALE, SNAP_RESKATE_MARKERS,
        SelectReskateLights, SkatePauseImage, SkatePausePicture, SnapReskateMarkers, Unclassified,
        WaterExtent, class_to_skate, raise_skate, studio_watts, tint_pixels,
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
    reg.summary::<components::SkateLayoutSummary>();
    reg.summary::<components::SkateCellCounts>();
    reg.decision::<components::SkateFloorDecision>();
    reg.decision::<components::SkateBlendBudget>();
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
/// A skate-scene folder as the convert loop's reader.
pub use read::scene::SceneReader;
pub use reference::{leave_out_optional, lod_choice, select_lod};
pub use write::map::{BuildReport, MapSink, target, write_map};

/// The passes of the ReSkate target, by name: raising core data into the dialect, the target's lowerings, and the
/// dialect's lowerings back to core.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    use passes::*;
    for spec in [
        &RAISE_SKATE,
        &lower::LOWER_LAYERED_PBR,
        &lower::LOWER_SKATE_PBR,
        &lower::LOWER_SKATE_LIGHTS,
        &PLACE_RESKATE_SPAWN,
        &GROUND_RESKATE_MARKERS,
        &SNAP_RESKATE_MARKERS,
        &game::clip::LOWER_SKATE_CLIPS,
        &LOWER_RESKATE_GRIND_CURVES,
        &PREPARE_RESKATE_GRINDS,
        &PREPARE_RESKATE_OVERLAYS,
        &PREPARE_RESKATE_WATER,
        &PREPARE_RESKATE_PAUSE_MAP,
        &SELECT_RESKATE_LIGHTS,
        &PREPARE_RESKATE_BLENDED_SURFACES,
        &PREPARE_RESKATE_UV_TRANSFORMS,
        &PREPARE_RESKATE_WRAP,
        &LOWER_RESKATE_UNLIT,
        // shadow casting as core states it (core.shadows), lowered to skate's per-material flag
        &LOWER_RESKATE_SHADOWS,
        &PREPARE_RESKATE_UV_REPEATS,
        &PREPARE_RESKATE_COLLISION,
        // a part's collision per cell, for a map in parts (the gather traversal), for the encoding given
        &SHARE_RESKATE_COLLISION,
        // a collision cell of a map in parts planned (the group pipeline)
        &PREPARE_RESKATE_CELL_COLLISION,
        // the map's summary, for the encoding given (the writer's defaults unless stated)
        &SUMMARIZE_RESKATE_MAP,
        &PLAN_RESKATE_BLEND_BUDGET,
        &FINALIZE_RESKATE_BLEND_BUDGET,
        &PREPARE_RESKATE_BLEND_ORDER,
        &PREPARE_RESKATE_FLOOR,
        // a map in parts: the marker snaps' inputs over every part (which prototypes are instanced, the water)
        &SUMMARIZE_RESKATE_INSTANCES,
        &SUMMARIZE_RESKATE_WATER,
        // a map in parts: what each part lays out, for the map sink
        &SUMMARIZE_RESKATE_LAYOUT,
        // a map in parts: its spawn placed once, from what every part holds
        &SUMMARIZE_RESKATE_SPAWN,
        // a map in parts: its content's extent measured over every part
        &SUMMARIZE_RESKATE_CONTENT,
        // a map in parts: its textures weighed over every part (with budget-reskate-textures's budget)
        &SUMMARIZE_RESKATE_TEXTURES,
        // a map in parts: its lights capped over every part (with select-reskate-lights's cap)
        &SUMMARIZE_RESKATE_LIGHTS,
        // a map in parts: its floor measured on every part, decided once (with prepare-reskate-floor's settings)
        &SUMMARIZE_RESKATE_FLOOR,
        &DECIDE_RESKATE_FLOOR,
        &CLIP_RESKATE_REGION,
        &BUDGET_RESKATE_TEXTURES,
        &PREPARE_RESKATE_TEXTURES,
        &layered::LOWER_LAYERED_RESKATE,
        &tint::LOWER_TINT_RESKATE,
        &vertex_alpha::LOWER_VERTEX_ALPHA_RESKATE,
    ] {
        reg.add(spec);
    }
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
    /// A number JSON cannot hold, in the scene the writer writes.
    #[error("skate scene: {0}")]
    Number(#[from] primase_core::JsonError),
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

    /// The crate registers exactly these passes by their declarations: those with required options are made only with
    /// them, and every one says what it takes.
    #[test]
    fn the_reskate_passes_register_by_name() {
        use primase_core::pass::{Options, PassRegistry};
        let mut reg = PassRegistry::new();
        register(&mut reg);
        let names = [
            "budget-reskate-textures",
            "clip-reskate-region",
            "decide-reskate-floor",
            "finalize-reskate-blend-budget",
            "ground-reskate-markers",
            "lower-layered-pbr",
            "lower-layered-reskate",
            "lower-reskate-grind-curves",
            "lower-reskate-shadows",
            "lower-reskate-unlit",
            "lower-skate-clips",
            "lower-skate-lights",
            "lower-skate-pbr",
            "lower-tint-reskate",
            "lower-vertex-alpha-reskate",
            "place-reskate-spawn",
            "plan-reskate-blend-budget",
            "prepare-reskate-blend-order",
            "prepare-reskate-blended-surfaces",
            "prepare-reskate-cell-collision",
            "prepare-reskate-collision",
            "prepare-reskate-floor",
            "prepare-reskate-grinds",
            "prepare-reskate-overlays",
            "prepare-reskate-pause-map",
            "prepare-reskate-textures",
            "prepare-reskate-uv-repeats",
            "prepare-reskate-uv-transforms",
            "prepare-reskate-water",
            "prepare-reskate-wrap",
            "raise-skate",
            "select-reskate-lights",
            "share-reskate-collision",
            "snap-reskate-markers",
            "summarize-reskate-content",
            "summarize-reskate-floor",
            "summarize-reskate-instances",
            "summarize-reskate-layout",
            "summarize-reskate-lights",
            "summarize-reskate-map",
            "summarize-reskate-spawn",
            "summarize-reskate-textures",
            "summarize-reskate-water",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        let free = [
            "lower-layered-pbr",
            "lower-reskate-shadows",
            "lower-skate-lights",
            "lower-skate-pbr",
            "lower-tint-reskate",
            "lower-vertex-alpha-reskate",
            "prepare-reskate-grinds",
            "prepare-reskate-textures",
            "share-reskate-collision",
            "snap-reskate-markers",
            "summarize-reskate-content",
            "summarize-reskate-floor",
            "summarize-reskate-instances",
            "summarize-reskate-map",
            "summarize-reskate-spawn",
            "summarize-reskate-water",
        ];
        for name in names {
            assert_eq!(reg.requires_options(name), !free.contains(&name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert_eq!(
                reg.make(name, &Options::new()).is_err(),
                !free.contains(&name),
                "{name}"
            );
        }
        assert!(
            reg.describe("prepare-reskate-water")
                .unwrap()
                .starts_with("prepare-reskate-water{(extent|x0)*,y0,x1,y1,margin,ocean}\n")
        );
        assert!(
            reg.describe("place-reskate-spawn")
                .unwrap()
                .starts_with("place-reskate-spawn{(at|east)*,north}\n")
        );
    }

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
