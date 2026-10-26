//! Raw Direct3D 9 `ps_3_0` tokens, their CTAB reflection, and lifting to the shared shader IR.
//!
//! This is a bounded pixel-shader reader: straight-line float arithmetic, declarations, immediate constants,
//! 2D/cube/volume sampling and discard. Other models, control flow, relative addressing and modifiers outside
//! this subset are refused by name and word offset. No translated text or native compiler is needed at runtime.
//! [`read`] preserves comments and CTAB's type/default metadata; [`lift`] normalizes registers into varyings and
//! a float constant bank (raw words in `cb0`, register `cN` at byte `16*N`). A `def` overrides that bank locally.
//!
//! Token layout: Microsoft's [instruction](https://learn.microsoft.com/windows-hardware/drivers/display/instruction-token),
//! [source](https://learn.microsoft.com/windows-hardware/drivers/display/source-parameter-token) and
//! [destination](https://learn.microsoft.com/windows-hardware/drivers/display/destination-parameter-token) tokens.
//! Arithmetic follows the [ps_3_0 instruction reference](https://learn.microsoft.com/windows/win32/direct3dhlsl/dx9-graphics-reference-asm-ps-instructions-ps-3-0).
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod ctab;
mod lower;
mod tokens;

pub use ctab::{Constant, ConstantTable, Type, TypeMember};
pub use tokens::{Comment, Destination, Input, Instruction, Opcode, Register, Shader, Source, read};

/// A refusal states the word offset in the raw token stream and the unsupported or malformed feature.
pub type Error = primase_shader::Refusal;
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(at: usize, detail: impl Into<String>) -> Error {
    Error::new("D3D9", primase_shader::Unit::Word, at, detail)
}

/// Increment when the same tokens lift differently. Hand-worked fixtures gate this version.
pub const LIFTER_VERSION: u32 = 1;

/// Lift one complete `ps_3_0` program; trailing bytes and unknown features are errors.
pub fn lift(bytes: &[u8]) -> Result<primase_shader::Program> {
    lower::lift(&read(bytes)?, bytes)
}
