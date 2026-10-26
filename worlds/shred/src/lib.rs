//! Tony Hawk: SHRED (PS3): RME archives, source materials, rigid meshes and scene placements.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

/// Register SHRED's source lowerings. Fog is a named linear approximation of the game's fullscreen pass. Normal-map interpretation is the program's
/// explicit choice until shader and tangent-basis evidence establishes it.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.add(&passes::LOWER_SHRED_MATERIALS);
    reg.add(&passes::LOWER_SHRED_FOG);
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Field(#[from] primase_bytes::FieldError),
    #[error("at {offset:#x}: {detail}")]
    Invalid { offset: usize, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        offset,
        detail: detail.to_string(),
    }
}
