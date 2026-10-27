//! Lift Shader Model 6 programs (DXIL) to an [`iridia::Program`], every stage a DXIL container holds, and map their
//! reflection, signatures and a library's subobjects into [`iridia_interface`]'s vocabulary. The bytes (the container,
//! the LLVM bitcode, the reflection and runtime-data records) are read by [`dxil_reader`]; a lifted program is run by
//! `iridia_exec`.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
mod lift;
mod reflection;
pub use lift::{LIFTER_VERSION, lift, lift_with};
pub use reflection::{interface, subobjects};

/// A DXIL program or container refused or unreadable: the bit it is at (in the bitcode, or eight times a byte of the
/// container) and why ([`iridia::Refusal`]).
pub type Error = iridia::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(bit: usize, detail: impl Into<String>) -> Error {
    Error::new("DXIL", iridia::Unit::Bit, bit, detail)
}

/// What the reader could not read, as a refusal of the program at the same bit.
pub(crate) fn read(e: dxil_reader::Error) -> Error {
    error(e.bit, e.detail)
}
