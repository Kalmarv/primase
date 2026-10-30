//! PEAK read over the Unity reader (`primase-unity`): what the game's own scripts hand every shader
//! (`Shader.SetGlobalTexture` and its kin), which Unity's own data does not state. Its programs sample those globals
//! by name; the read puts what the scripts' stored fields set into `unity.shader_globals`
//! ([`primase_unity::components::UnityShaderGlobals`]), which `resolve-unity-shaders` binds a parameter the material
//! has no property of from ([`read::read_globals`]): the fog texture (`Peak.FogShaderParams`) and the baked light volume
//! with the values that go with it (`LightVolume`), assembled as the game's own `Decompress` does
//! ([`read::light_volume`]).
//!
//! The reader states facts only: a global a script sets from state the data does not hold is named unset, with why.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod read;

pub use read::{GlobalsReport, read_globals};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Unity(#[from] unity_formats::Error),
    /// A script's stored fields are not a layout this crate knows for the player's Unity version.
    #[error("{0}")]
    Layout(String),
    /// Stored data a script reads in a way this crate does not reproduce exactly (said how).
    #[error("{0}")]
    Refused(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
