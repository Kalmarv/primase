//! Frostbite CAS: sequence of eight-byte headers and exactly sized payloads.
use primase_foundations::compress::{Compressor, Error, Result};

pub(crate) fn invalid(offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        codec: "cas",
        offset,
        detail: detail.to_string(),
    }
}
pub(crate) fn buffer(size: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| Error::Limit)?;
    out.resize(size, 0);
    Ok(out)
}
pub(crate) fn check_size(actual: usize, expected: usize) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::Size { expected, actual })
    }
}
/// A borrowed block, including its physical header offset.
#[derive(Debug, Clone, Copy)]
pub struct Block<'a> {
    pub offset: usize,
    pub kind: u8,
    pub decoded_size: usize,
    pub payload: &'a [u8],
}
/// Parse one complete CAS stream. Header: decoded size is BE u32 low 24 bits;
/// high byte is dictionary metadata (unsupported, rejected). Byte 4 low 7 bits
/// is codec; its high bit is reserved. Byte 5 low nibble supplies payload bits
/// 16..19; bytes 6..7 are BE payload low16. Byte5 high nibble is retained by the
/// format as flags (writer emits 0x70), not part of either length.
pub fn blocks(input: &[u8]) -> Result<Vec<Block<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        let start = pos;
        let h = input
            .get(pos..pos + 8)
            .ok_or_else(|| invalid(pos, "short header"))?;
        if h[0] != 0 || h[4] & 0x80 != 0 {
            return Err(invalid(pos, "dictionary/reserved flag unsupported"));
        }
        let decoded_size = (usize::from(h[1]) << 16) | (usize::from(h[2]) << 8) | usize::from(h[3]);
        let size = (usize::from(h[5] & 15) << 16) | (usize::from(h[6]) << 8) | usize::from(h[7]);
        pos += 8;
        let payload = input
            .get(pos..pos + size)
            .ok_or_else(|| invalid(pos, "short payload"))?;
        out.push(Block {
            offset: start,
            kind: h[4],
            decoded_size,
            payload,
        });
        pos += size;
    }
    Ok(out)
}
pub(crate) fn append(out: &mut Vec<u8>, decoded: usize, kind: u8, payload: &[u8]) {
    out.extend_from_slice(&(decoded as u32).to_be_bytes());
    out.extend_from_slice(&[kind, 0x70 | ((payload.len() >> 16) as u8)]);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
}
/// Encode raw 64 KiB blocks, including one empty raw block for empty input.
#[derive(Debug, Clone, Copy)]
pub struct CasRaw;
impl Compressor for CasRaw {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if input.is_empty() {
            append(&mut out, 0, 0, &[]);
        }
        for part in input.chunks(65536) {
            append(&mut out, part.len(), 0, part);
        }
        Ok(out)
    }
}
