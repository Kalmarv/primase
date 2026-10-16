//! GTA V read straight from an install: archives (RPF7 with their NG or AES ciphers) and resources (RSC7).
//!
//! Key material is never part of this crate: the caller supplies it ([`Keys`]) and the crate checks it against
//! published digests before using it.
pub mod archive_set;
mod key_hashes;
pub mod keys;
pub mod meta;
pub mod mount;
pub mod ng;
pub mod resource;
pub mod rpf;
mod xml;

pub use archive_set::{ArchiveSet, FileRef, Install};
pub use keys::Keys;
pub use resource::{PageFlags, Resource};
pub use rpf::{Archive, Encryption, Entry, EntryKind};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: u64,
        detail: String,
    },
    #[error("key material: {0}")]
    Keys(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("compression: {0}")]
    Compression(#[from] primase_compress::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(format: &'static str, offset: u64, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}
