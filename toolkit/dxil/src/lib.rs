//! Checked, bounded Shader Model 6 (DXIL) pixel-program evaluation, implementing [`primase_shader::PixelShader`].
//! The container is the DXBC format ([`chunks`]); the program is LLVM bitcode in its `DXIL` part, read by
//! a bounded bitstream reader of our own. Sampling belongs to the caller.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
pub mod bitstream;
mod evaluate;
pub mod module;
pub mod reflection;
pub use evaluate::{Census, Shader};
pub use reflection::interface;

#[derive(Debug, thiserror::Error)]
#[error("DXIL at bit {bit}: {detail}")]
pub struct Error {
    /// Bit offset in the bitcode (or byte offset times eight in the container) where the problem is.
    pub bit: usize,
    pub detail: String,
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(bit: usize, detail: impl Into<String>) -> Error {
    Error {
        bit,
        detail: detail.into(),
    }
}

/// The `DXIL` part of a DXBC container: the program header and its LLVM bitcode.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    /// Shader kind in the high 16 bits (0 pixel), model major and minor in the low byte (`0x60` is 6.0).
    pub program_version: u32,
    /// DXIL version (`0x100` is 1.0).
    pub dxil_version: u32,
    pub bitcode: &'a [u8],
}

/// A container part: its four-character code and data.
#[derive(Clone, Copy, Debug)]
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: &'a [u8],
}

/// Read a DXBC container's part directory: header (`DXBC`, a digest that is not checked, version 1.0, the total
/// size), part offsets inside the container (SM6 containers do not align them), parts that do not overlap and a
/// code of each kind at most once.
pub fn chunks(container: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let view = primase_bytes::View::new(container);
    let word = |at: usize| -> Result<usize> {
        view.array::<4>(at)
            .map(|b| u32::from_le_bytes(b) as usize)
            .map_err(|e| error(at * 8, e.to_string()))
    };
    if container.get(..4) != Some(b"DXBC") || word(20)? != 1 || word(24)? != container.len() {
        return Err(error(0, "container header or declared size"));
    }
    let n = word(28)?;
    let directory_end = n
        .checked_mul(4)
        .and_then(|d| d.checked_add(32))
        .filter(|&e| e <= container.len())
        .ok_or_else(|| error(28 * 8, "part directory exceeds the container"))?;
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(n);
    let mut out: Vec<Chunk<'_>> = Vec::with_capacity(n);
    for i in 0..n {
        let start = word(32 + 4 * i)?;
        if start < directory_end {
            return Err(error((32 + 4 * i) * 8, "part inside the directory"));
        }
        let kind = view
            .array::<4>(start)
            .map_err(|e| error(start * 8, e.to_string()))?;
        let len = word(start + 4)?;
        let data = view
            .slice(start + 8, len)
            .map_err(|e| error(start * 8, e.to_string()))?;
        if out.iter().any(|c| c.kind == kind) {
            return Err(error(start * 8, "a part kind twice"));
        }
        spans.push((start, start + 8 + len));
        out.push(Chunk { kind, data });
    }
    spans.sort_unstable();
    if spans.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(error(0, "overlapping parts"));
    }
    Ok(out)
}

/// A program part's header (`DXIL` and the `STAT` reflection part share it) and its bitcode.
pub fn program(data: &[u8]) -> Result<Part<'_>> {
    let view = primase_bytes::View::new(data);
    let word = |at: usize| -> Result<u32> {
        view.array::<4>(at)
            .map(u32::from_le_bytes)
            .map_err(|e| error(at * 8, e.to_string()))
    };
    let program_version = word(0)?;
    let dwords = word(4)? as usize;
    if dwords.checked_mul(4) != Some(data.len()) {
        return Err(error(32, "program size disagrees with the part"));
    }
    if view.slice(8, 4).map_err(|e| error(64, e.to_string()))? != b"DXIL" {
        return Err(error(64, "program header magic"));
    }
    let dxil_version = word(12)?;
    // The bitcode offset counts from the magic.
    let offset = word(16)? as usize;
    let size = word(20)? as usize;
    let bitcode = offset
        .checked_add(8)
        .and_then(|start| view.slice(start, size).ok())
        .ok_or_else(|| error(128, "bitcode outside the part"))?;
    Ok(Part {
        program_version,
        dxil_version,
        bitcode,
    })
}

/// Find and check a container's `DXIL` part.
pub fn part(container: &[u8]) -> Result<Part<'_>> {
    let chunks = chunks(container)?;
    let data = chunks
        .iter()
        .find(|c| &c.kind == b"DXIL")
        .ok_or_else(|| error(0, "no DXIL part"))?
        .data;
    program(data)
}
