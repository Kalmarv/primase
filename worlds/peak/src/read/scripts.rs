//! PEAK's script fields, decoded from the bytes a MonoBehaviour stores after its base fields
//! ([`unity_formats::scene::UnityBehaviour::data`]; the player stores no type tree for them). Each layout is the
//! script's serialized fields in declaration order (the game's own `Assembly-CSharp`), named for the player's Unity
//! version it was measured on. Bytes that are not that layout (another length) are refused with what was found, never
//! guessed at; another build's layout is a new named layout.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::{Error, Result};

/// The Unity version of the PEAK player these layouts were measured on (its level4): `Peak.FogShaderParams`.
pub const MEASURED: &str = "6000.3.15f1";

fn refuse(what: &str, version: &str, detail: String) -> Error {
    Error::Layout(format!(
        "{what}: {detail}: not the layout measured on Unity {MEASURED} (this player: {version})"
    ))
}

/// An object reference as a script stores it: the file index (0 this file, else one past its index in the file's
/// externals) and the path id; a path id of 0 is no object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PPtr {
    pub file_id: i32,
    pub path_id: i64,
}

/// A stored `PPtr` at the start of `bytes`: a 32-bit file id and a 64-bit path id, little-endian.
fn pptr(bytes: &[u8]) -> Option<PPtr> {
    let file_id = i32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let path_id = i64::from_le_bytes(bytes.get(4..12)?.try_into().ok()?);
    Some(PPtr { file_id, path_id })
}

/// `Peak.FogShaderParams.fogTexture`, from its script bytes: on Unity [`MEASURED`] its one serialized field, a
/// `Texture` reference (12 bytes); `Start` hands it to every shader as `FogTexture`.
pub fn fog_shader_params(version: &str, data: &[u8]) -> Result<PPtr> {
    const WHAT: &str = "FogShaderParams";
    if version != MEASURED {
        return Err(refuse(WHAT, version, "another Unity version".into()));
    }
    if data.len() != 12 {
        return Err(refuse(
            WHAT,
            version,
            format!("{} script bytes, not 12", data.len()),
        ));
    }
    pptr(data).ok_or_else(|| refuse(WHAT, version, "no texture reference".into()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;

    /// The one field is the texture reference; any other length or Unity version is refused, saying which.
    #[test]
    fn fog_shader_params_is_one_texture_reference() {
        let mut data = 3i32.to_le_bytes().to_vec();
        data.extend(621_203i64.to_le_bytes());
        assert_eq!(
            fog_shader_params(MEASURED, &data).unwrap(),
            PPtr {
                file_id: 3,
                path_id: 621_203
            }
        );
        let short = fog_shader_params(MEASURED, &data[..8]).unwrap_err().to_string();
        assert!(short.contains("8 script bytes, not 12"), "{short}");
        let mut long = data.clone();
        long.extend([0; 4]);
        assert!(fog_shader_params(MEASURED, &long).is_err());
        let other = fog_shader_params("6000.3.17f1", &data).unwrap_err().to_string();
        assert!(
            other.contains("another Unity version") && other.contains("6000.3.17f1"),
            "{other}"
        );
    }
}
