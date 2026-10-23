//! Read-only FromSoftware (Sekiro, Elden Ring) BHD5/BDT archives, DCX envelopes, BND4 binders and the formats in them.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bhd;
mod bytes;
pub mod collision;
mod elden;
pub mod emevd;
pub mod flver;
pub mod game;
mod keys;
pub mod materials;
pub mod metaparam;
pub mod msb;
pub mod mtd;
mod passes;
mod pose;
mod read;
pub mod sekiro;
pub use archive::{Archive, Keys, NameDictionary};
pub use binder::{Binder, BinderFile};
pub use game::Game;
mod components;
pub use passes::{geometry, layer_bake, preview, rigid};
pub use read::{archive, binder, dcx};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: usize,
        detail: String,
    },
    #[error("{0}")]
    Havok(#[from] primase_havok::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("compression: {0}")]
    Compression(#[from] primase_compress::Error),
    #[error("entry hash {0:08x} not found")]
    Missing(u64),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn invalid(format: &'static str, offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}
pub mod btl;
pub mod edge;
pub mod tpf;
