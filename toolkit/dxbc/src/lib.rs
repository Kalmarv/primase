//! Checked, bounded Shader Model 5 pixel-program evaluation.
//! Sampling belongs to the caller: a scalar interpreter cannot infer derivatives.
mod evaluate;
mod opcodes;
mod program;
mod reflection;
pub use evaluate::{Execution, RegisterValue, Sample, Sampler, Trace};
pub use program::{Instruction, Operand, Program, Register, Selection};
pub use reflection::{Buffer, Member, Reflection, Resource, Semantic, Type, Variable};

#[derive(Debug, thiserror::Error)]
#[error("DXBC at word {word}: {detail}")]
pub struct Error {
    pub word: usize,
    pub detail: String,
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(word: usize, detail: impl Into<String>) -> Error {
    Error {
        word,
        detail: detail.into(),
    }
}

/// A borrowed container chunk. Offsets and lengths are checked before exposure.
#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
}

/// Read the DXBC directory, rejecting truncated, overlapping and duplicate code chunks.
/// The container digest is not an authenticity check and is not verified here.
pub fn chunks(bytes: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let word = |at: usize| -> Result<usize> {
        let b = bytes
            .get(at..at + 4)
            .ok_or_else(|| error(at / 4, "short container"))?;
        Ok(u32::from_le_bytes(b.try_into().expect("four bytes")) as usize)
    };
    if bytes.get(..4) != Some(b"DXBC") || word(20)? != 1 || word(24)? != bytes.len() {
        return Err(error(0, "invalid container header or declared size"));
    }
    let n = word(28)?;
    if n > bytes.len().saturating_sub(32) / 4 {
        return Err(error(7, "chunk directory exceeds container"));
    }
    let mut ranges = Vec::new();
    let mut result = Vec::new();
    let mut code = false;
    for i in 0..n {
        let start = word(32 + 4 * i)?;
        if start < 32 + 4 * n || start > bytes.len().saturating_sub(8) || start % 4 != 0 {
            return Err(error(8 + i, "invalid chunk offset"));
        }
        let len = word(start + 4)?;
        let end = (start + 8)
            .checked_add(len)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| error(start / 4, "chunk exceeds container"))?;
        if ranges.iter().any(|&(a, b)| start < b && a < end) {
            return Err(error(start / 4, "overlapping chunks"));
        }
        ranges.push((start, end));
        let kind: [u8; 4] = bytes[start..start + 4].try_into().expect("checked header");
        if &kind == b"SHEX" || &kind == b"SHDR" {
            if code {
                return Err(error(start / 4, "multiple shader code chunks"));
            }
            code = true;
        }
        result.push(Chunk {
            kind,
            data: &bytes[start + 8..end],
        });
    }
    Ok(result)
}
