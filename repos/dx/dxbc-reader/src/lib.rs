//! A reader of Shader Model 4 and 5 programs as Direct3D stores them in a `DXBC` container: the token stream of any
//! stage ([`decode`]), and the reflection and signature chunks as the records the file holds ([`reflection`]).
//!
//! It reads and checks bytes and never interprets them for an API: a register binding is the RDEF record's own
//! numbers, a signature element its stored name, index and masks. Mapping those into a graphics API's vocabulary is
//! a consumer's choice. Everything it can't read (a truncated instruction, an out-of-range table, a cyclic type) is an
//! [`Error`] naming the 32-bit word where it happened; it never panics on untrusted input.
//!
//! ```no_run
//! # fn main() -> Result<(), dxbc_reader::Error> {
//! let container = std::fs::read("shader.dxbc").unwrap();
//! let code = dxbc_reader::code(&container)?;
//! let program = dxbc_reader::decode(&code)?;
//! println!("{} with {} instructions", program.target(), program.instructions.len());
//! # Ok(()) }
//! ```
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod decode;
mod opcodes;
pub mod reflection;

pub use decode::{
    Declaration, Declared, Decoded, Index, Instruction, Operand, ProgramType, Register, Selection, decode,
    opcode_name,
};
pub use reflection::{
    Rdef, RdefBinding, RdefBuffer, RdefMember, RdefType, RdefVariable, SignatureElement,
    patch_signature_elements, program_type, rdef, rdef_target, signature, signature_elements,
};

/// Something the reader can't read: the 32-bit word of the container's chunk or token stream where it is, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The word offset, counted from the start of the chunk or token stream being read.
    pub word: usize,
    pub detail: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DXBC at word {}: {}", self.word, self.detail)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error {
        word,
        detail: detail.into(),
    }
}

/// The container's parts (its `DXBC` directory).
pub fn parts(bytes: &[u8]) -> Result<Vec<dxcontainer::Chunk<'_>>> {
    dxcontainer::chunks(bytes).map_err(|e| error(e.offset / 4, e.detail))
}

/// The container's one part of kind `tag`: an error where it has none, or more than one.
pub fn chunk<'a>(directory: &[dxcontainer::Chunk<'a>], tag: &[u8; 4]) -> Result<&'a [u8]> {
    let mut matching = directory.iter().filter(|c| &c.kind == tag);
    let data = matching
        .next()
        .ok_or_else(|| {
            error(
                0,
                format!("missing reflection chunk {}", String::from_utf8_lossy(tag)),
            )
        })?
        .data;
    if matching.next().is_some() {
        return Err(error(0, "duplicate reflection chunk"));
    }
    Ok(data)
}

/// The program's token stream (its `SHEX` or `SHDR` part) as 32-bit little-endian words, for [`decode`].
pub fn code(bytes: &[u8]) -> Result<Vec<u32>> {
    let directory = parts(bytes)?;
    let code = directory
        .iter()
        .find(|c| matches!(&c.kind, b"SHDR" | b"SHEX"))
        .ok_or_else(|| error(0, "missing shader code chunk"))?;
    if code.data.len() % 4 != 0 {
        return Err(error(0, "unaligned code length"));
    }
    Ok(code
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect())
}
