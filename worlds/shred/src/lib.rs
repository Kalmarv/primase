//! Tony Hawk: SHRED (PS3): RME PAK v4 archives and their CSTR streams.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod read;

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
