//! ReSkate (skate.): the target's own crate. It owns the `skate.*` dialect ([`dialect`]), the preparation passes a
//! program selects to raise core data into it ([`prepare`], `raise-skate`, and the target's lowerings), the frozen
//! skate-scene interchange v1 ([`interchange`]: read, load, write), and the writer of ReSkate Studio normalized map
//! folders (`map.json`, `meshes/*.glb`, `textures/*`, `ui/*`, `trainer.json`, `stats.json`,
//! `textures_index.json`), gated against the reference C# writer by what Studio reads: meshes, tables and texture
//! payloads exactly, JSON as parsed values, PNGs by pixels.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod blend_order;
pub mod build;
pub mod collision;
pub mod dialect;
pub mod glb;
pub mod grind;
pub mod hash;
pub mod interchange;
pub mod layered;
pub mod map;
pub mod material;
pub mod model;
pub mod overlay;
pub mod pausemap;
pub mod postcompile;
pub mod prepare;
pub mod records;
pub mod scene;
pub mod texture;
pub mod tint;
pub mod vertex_alpha;

use std::path::PathBuf;

pub use build::{
    AssembledMaterial, BuildOptions, BuildReport, EncodingOptions, assembled_materials, build,
    leave_out_optional, lod_choice, select_lod, write_map,
};

/// The passes of the ReSkate target, by name: raising core data into the dialect, the target's lowerings, and the
/// dialect's lowerings back to core.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    // the scene's name has no default
    reg.register_requiring("raise-skate", prepare::raise_skate_pass, &[("name", "n")]);
    dialect::lower::register(reg);
    reg.register("place-reskate-spawn", prepare::place_reskate_spawn);
    reg.register("snap-reskate-markers", prepare::snap_reskate_markers);
    reg.register("prepare-reskate-grinds", prepare::prepare_reskate_grinds);
    // the step is the program's: no default
    reg.register_requiring(
        "prepare-reskate-overlays",
        prepare::prepare_reskate_overlays,
        &[("step", "0")],
    );
    // which water a map draws and how far is the program's: no default
    reg.register_requiring(
        "prepare-reskate-water",
        prepare::prepare_reskate_water,
        &[("extent", "none")],
    );
    // the pause-map picture, and its size and extent, are the program's: no default
    reg.register_requiring(
        "prepare-reskate-pause-map",
        prepare::prepare_reskate_pause_map,
        &[("from", "geometry"), ("height", "9"), ("extent", "content")],
    );
    // a light cap and its grouping are the program's: no default
    reg.register_requiring(
        "select-reskate-lights",
        prepare::select_reskate_lights,
        &[("per_cell", "1"), ("cell", "200")],
    );
    // the budget is the program's: no default
    reg.register_requiring(
        "plan-reskate-blend-budget",
        prepare::plan_reskate_blend_budget,
        &[("objects", "pool")],
    );
    // the grid is the program's: no default
    reg.register_requiring(
        "prepare-reskate-blend-order",
        prepare::prepare_reskate_blend_order,
        &[("grid", "0")],
    );
    // the floor is the program's: no default
    reg.register_requiring(
        "prepare-reskate-floor",
        prepare::prepare_reskate_floor,
        &[("min_height", "0"), ("stub_below", "0")],
    );
    // the region is the program's: no default
    reg.register_requiring(
        "clip-reskate-region",
        prepare::clip_reskate_region,
        &[("x0", "0"), ("y0", "0"), ("x1", "1"), ("y1", "1")],
    );
    // the budget is the program's number: no default
    reg.register_requiring(
        "budget-reskate-textures",
        prepare::budget_reskate_textures,
        &[("max", "1")],
    );
    reg.register("prepare-reskate-textures", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(prepare::PrepareReskateTextures)),
    });
    reg.register("lower-layered-reskate", layered::lower_layered_reskate);
    reg.register("lower-tint-reskate", |o| {
        let mut pass = tint::LowerTintReskate::default();
        for (k, v) in o {
            match k.as_str() {
                "copies" => {
                    pass.copies = Some(v.parse().map_err(|_| format!("copies={v} is not a count"))?);
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Box::new(pass))
    });
    reg.register("lower-vertex-alpha-reskate", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(vertex_alpha::LowerVertexAlphaReskate)),
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
    /// A scene's GLB that the glTF reader refused.
    #[error(transparent)]
    Gltf(#[from] primase_gltf::Error),
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
