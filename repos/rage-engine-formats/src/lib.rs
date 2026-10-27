//! The RAGE engine's file formats, as GTA V Enhanced stores them, read from bytes into plain records.
//!
//! - Archives ([`archive`]): RPF7 with its NG and AES ciphers, the mount data that says which archives are live
//!   ([`mount`]), and the install's listing. The cipher keys are never part of this crate: the caller supplies the key
//!   material ([`archive::keys::Keys`]) and the digests it is checked against ([`archive::keys::KeyDigests`]).
//! - Resources (RSC7, [`resource`]) and what they hold: drawables ([`drawable`]), texture dictionaries ([`texture`],
//!   buffer formats in [`sga`]), collision bounds ([`bounds`]), clip dictionaries ([`ycd`], [`ycd_channels`]).
//! - META documents ([`meta`]) and the map data ([`map`]) and ped metadata ([`ped`]) read from them.
//! - Shader containers: compiled effects ([`fxc`]) and shader databases ([`sgd2`], [`sgd2_effect`], [`sgd2_table`]).
//! - The game's XML data files ([`xml`]), their binary form RBF ([`rbf`]) and the texture-dictionary parent list
//!   ([`gtxd`]).
//!
//! Every reader takes bytes (or, for archives, paths) and returns records; every read is bounded and a malformed
//! file is an [`Error`], never a panic.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod archive;
pub mod bounds;
pub mod drawable;
pub mod fxc;
pub mod gtxd;
pub mod le;
pub mod map;
pub mod meta;
pub mod mount;
pub mod ped;
pub mod rbf;
pub mod resource;
pub mod sga;
pub mod sgd2;
pub mod sgd2_effect;
pub mod sgd2_table;
pub mod texture;
pub mod xml;
pub mod ycd;
pub mod ycd_channels;

/// Why a file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The bytes do not hold what the format says: the format, the offset and what is wrong.
    #[error("{format} at {offset:#x}: {detail}")]
    Invalid {
        format: &'static str,
        offset: u64,
        detail: String,
    },
    /// The key material is missing, the wrong size or does not match its digest.
    #[error("key material: {0}")]
    Keys(String),
    /// A file could not be read.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A compressed block did not inflate.
    #[error("compression: {0}")]
    Compression(#[from] primase_foundations::compress::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A read past the bytes there are ([`primase_foundations::bytes::View`], [`primase_foundations::bytes::Cursor`]): the format's error at its
/// offset.
impl From<primase_foundations::bytes::BoundsError> for Error {
    fn from(e: primase_foundations::bytes::BoundsError) -> Self {
        Error::from(primase_foundations::bytes::FieldError {
            what: "bytes",
            bounds: e,
        })
    }
}

/// A read past the bytes there are, of a named format (`c.u32(LE).field("RBF")`).
impl From<primase_foundations::bytes::FieldError> for Error {
    fn from(e: primase_foundations::bytes::FieldError) -> Self {
        let offset = u64::try_from(e.bounds.absolute()).unwrap_or(u64::MAX);
        let detail = match e.bounds {
            primase_foundations::bytes::BoundsError::OutOfBounds { length, total, .. } => {
                format!("{length} bytes past the {total} there are")
            }
            primase_foundations::bytes::BoundsError::LengthOverflow { count, stride, .. } => {
                format!("{count} entries of {stride} bytes overflow")
            }
        };
        invalid(e.what, offset, detail)
    }
}

pub(crate) fn invalid(format: &'static str, offset: u64, detail: impl ToString) -> Error {
    Error::Invalid {
        format,
        offset,
        detail: detail.to_string(),
    }
}
