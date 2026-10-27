//! Lift Direct3D Shader Model 4 and 5 programs (DXBC) to an [`iridia::Program`], any stage, and map their reflection
//! and signatures into [`iridia_interface`]'s vocabulary. The bytes are read by [`dxbc_reader`]; a lifted program is
//! run by `iridia_exec`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod arity;
mod lift;
mod reflection;
pub use lift::{LIFTER_VERSION, lift, lift_with};
pub use reflection::{reflection, resource_reflection, signatures, varying};

/// A DXBC program or container refused or unreadable: the word it is at and why ([`iridia::Refusal`]).
pub type Error = iridia::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error::new("DXBC", iridia::Unit::Word, word, detail)
}

/// What the reader could not read, as a refusal of the program at the same word.
pub(crate) fn read(e: dxbc_reader::Error) -> Error {
    error(e.word, e.detail)
}

/// The parts of a DXBC container, an error of the container as one of the program (the word it is at).
pub(crate) fn parts(bytes: &[u8]) -> Result<Vec<dxcontainer::Chunk<'_>>> {
    dxbc_reader::parts(bytes).map_err(read)
}
