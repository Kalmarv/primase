//! Strict readers of FromSoftware's data (Sekiro: Shadows Die Twice and Elden Ring): the installed BHD5/BDT archives
//! ([`Archive`], with keys the caller supplies), DCX envelopes ([`dcx`]), BND4 binders ([`Binder`]) and the formats
//! they hold: FLVER models, map layouts (MSB), light banks (BTL), hang edges (EDGE), event scripts (EMEVD), text
//! (FMG), draw params (GPARAM), material definitions (MTD, MATBIN), shader metaparams, params (PARAM), animation
//! events (TAE), texture packages (TPF), collision (Havok, CLM2).
//!
//! Every read is checked: data that is shorter than it says, refers outside itself or holds a value the reader does
//! not know is an [`Error`] naming the format and the offset, never a panic or a silently skipped field. What a reader
//! keeps is what the file stores; what a file leaves to the game (which file is in force, what a flag means) is the
//! caller's to decide.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod affine;
pub mod archive;
pub mod area;
pub mod bhd;
pub mod binder;
pub mod btl;
mod bytes;
pub mod clm2;
pub mod collision;
pub mod dcx;
pub mod edge;
mod elden;
pub mod emevd;
pub mod equipment;
pub mod flver;
pub mod fmg;
pub mod game;
pub mod game_params;
pub mod gparam;
mod keys;
pub mod matbin;
pub mod messages;
pub mod metaparam;
pub mod msb;
pub mod mtd;
pub mod names;
pub mod param;
mod sekiro;
pub mod states;
pub mod tae;
pub mod tpf;

pub use archive::{Archive, Install, Keys, NameDictionary};
pub use binder::{Binder, BinderFile};
pub use game::Game;

/// A refusal: the data read is not what its format allows, or could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `format`'s data at `offset` holds what the reader refuses, or lacks what it needs.
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: usize,
        detail: String,
    },
    /// A Havok object the reader holds is not readable.
    #[error("{0}")]
    Havok(#[from] havok_formats::Error),
    /// A file could not be read from disk.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A compressed payload did not decompress.
    #[error("compression: {0}")]
    Compression(#[from] primase_foundations::compress::Error),
    /// The archives hold no entry of this path hash.
    #[error("entry hash {0:08x} not found")]
    Missing(u64),
}

impl Error {
    /// [`Error::Invalid`]: `format`'s data at `offset` is refused for `detail`.
    pub fn invalid(format: &'static str, offset: usize, detail: impl ToString) -> Self {
        Error::Invalid {
            format,
            offset,
            detail: detail.to_string(),
        }
    }
}

/// A read's result.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(format: &'static str, offset: usize, detail: impl ToString) -> Error {
    Error::invalid(format, offset, detail)
}
