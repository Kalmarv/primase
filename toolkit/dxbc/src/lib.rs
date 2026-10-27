//! Shader Model 4 and 5 programs: the token reader (`decode`), the signatures and reflection (`reflection`,
//! `signatures`) and the lifter to `iridia::Program` (`lift`, any stage). A lifted program is run by
//! `iridia_exec`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod arity;
pub mod decode;
mod lift;
mod opcodes;
mod reflection;
pub use decode::{Index, decode};
pub use decode::{Instruction, Operand, Register, Selection};
pub use lift::{LIFTER_VERSION, lift, lift_with};
pub use reflection::{
    SignatureElement, patch_signature_elements, reflection, resource_reflection, signature_elements,
    signatures,
};

/// A DXBC program or container refused or unreadable: the word it is at and why ([`iridia::Refusal`]).
pub type Error = iridia::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error::new("DXBC", iridia::Unit::Word, word, detail)
}

/// The parts of a DXBC container, an error of the container as one of the program (the word it is at).
pub(crate) fn parts(bytes: &[u8]) -> Result<Vec<dxcontainer::Chunk<'_>>> {
    dxcontainer::chunks(bytes).map_err(|e| error(e.offset / 4, e.detail))
}
