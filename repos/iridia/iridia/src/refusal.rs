//! Why a lifter refused a program, and the hash of the bytes it was given: what the three lifters (DXBC, DXIL and
//! MojoShader's GLSL) share about saying no.

use std::fmt;

use sha2::{Digest, Sha256};

/// What an offset in a [`Refusal`] counts, in the source's own terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// A 32-bit word of a DXBC token stream.
    Word,
    /// A bit of a DXIL module's bitcode (or eight times a byte of its container).
    Bit,
    /// A line of GLSL text.
    Line,
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Unit::Word => "word",
            Unit::Bit => "bit",
            Unit::Line => "line",
        })
    }
}

/// A program a lifter did not lift, or a container it could not read: the bytecode, where in it and the reason, by name.
/// The reason is stated whole, numbers included; [`Refusal::group`] is the key to count refusals by.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{bytecode} at {unit} {at}: {detail}")]
pub struct Refusal {
    /// The bytecode's name (`DXBC`, `DXIL`, `MojoShader GLSL`).
    pub bytecode: &'static str,
    pub unit: Unit,
    /// Where, counted in `unit`.
    pub at: usize,
    pub detail: String,
}

impl Refusal {
    pub fn new(bytecode: &'static str, unit: Unit, at: usize, detail: impl Into<String>) -> Self {
        Self {
            bytecode,
            unit,
            at,
            detail: detail.into(),
        }
    }

    /// The reason as the lifter stated it.
    pub fn reason(&self) -> &str {
        &self.detail
    }

    /// The reason with each number as `N`: refusals of one cause that differ only in an offset, a register or a count share it,
    /// to count them by. The reason itself is [`Refusal::reason`].
    pub fn group(&self) -> String {
        let mut out = String::with_capacity(self.detail.len());
        let mut digits = false;
        for c in self.detail.chars() {
            if c.is_ascii_digit() {
                if !digits {
                    out.push('N');
                }
                digits = true;
            } else {
                out.push(c);
                digits = false;
            }
        }
        out
    }
}

/// The hash of the bytes a lifter was given (`sha256:` and 64 hexadecimal digits), which a program keeps as its source's
/// identity in [`crate::Provenance::source_hash`]: the lifter computes it, the caller does not state it.
pub fn source_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
