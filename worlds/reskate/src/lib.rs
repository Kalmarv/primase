//! ReSkate (skate.): the writer of ReSkate Studio normalized map folders from skate-scene worlds (`map.json`,
//! `meshes/*.glb`, `textures/*`, `ui/*`, `trainer.json`, `stats.json`, `pieces.json`, `textures_index.json`), gated
//! against the reference C# writer by what Studio reads: meshes, tables and texture payloads exactly, JSON as parsed
//! values, PNGs by pixels.

pub mod build;
pub mod collision;
pub mod glb;
pub mod grind;
pub mod layered;
pub mod map;
pub mod material;
pub mod model;
pub mod overlay;
pub mod pausemap;
pub mod postcompile;
pub mod records;
pub mod scene;
pub mod texture;

use std::path::PathBuf;

pub use build::{BuildOptions, BuildReport, build};

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
