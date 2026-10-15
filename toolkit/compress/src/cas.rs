//! Frostbite CAS: sequence of eight-byte headers and exactly sized payloads.
use crate::{Compressor, Decompressor, Result, Zlib, buffer, check_size, invalid, oodle::Oodle};
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
            .ok_or_else(|| invalid("cas", pos, "short header"))?;
        if h[0] != 0 || h[4] & 0x80 != 0 {
            return Err(invalid("cas", pos, "dictionary/reserved flag unsupported"));
        }
        let decoded_size = (usize::from(h[1]) << 16) | (usize::from(h[2]) << 8) | usize::from(h[3]);
        let size = (usize::from(h[5] & 15) << 16) | (usize::from(h[6]) << 8) | usize::from(h[7]);
        pos += 8;
        let payload = input
            .get(pos..pos + size)
            .ok_or_else(|| invalid("cas", pos, "short payload"))?;
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
/// CAS codec, optional explicit Oodle implementation. Decode supports raw, zlib,
/// and kinds 0x11/0x15/0x19. Unknown kinds are errors with physical offsets.
#[derive(Debug, Default)]
pub struct Cas<'a> {
    pub oodle: Option<&'a Oodle>,
}
impl Decompressor for Cas<'_> {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        let blocks = blocks(input)?;
        let total = blocks.iter().try_fold(0usize, |s, b| {
            s.checked_add(b.decoded_size).ok_or(crate::Error::Limit)
        })?;
        check_size(total, expected)?;
        let mut out = buffer(expected)?;
        let mut pos = 0;
        for b in blocks {
            let dst = &mut out[pos..pos + b.decoded_size];
            match b.kind {
                0 => {
                    check_size(b.payload.len(), b.decoded_size)?;
                    dst.copy_from_slice(b.payload);
                }
                2 => dst.copy_from_slice(
                    &Zlib
                        .decompress(b.payload, b.decoded_size)
                        .map_err(|e| invalid("cas", b.offset, e))?,
                ),
                0x11 | 0x15 | 0x19 => {
                    let oodle = self
                        .oodle
                        .ok_or_else(|| invalid("cas", b.offset, "Oodle library required"))?;
                    dst.copy_from_slice(
                        &oodle
                            .decompress(b.payload, b.decoded_size)
                            .map_err(|e| invalid("cas", b.offset, e))?,
                    );
                }
                _ => {
                    return Err(invalid(
                        "cas",
                        b.offset,
                        format!("unsupported kind {:#x}", b.kind),
                    ));
                }
            }
            pos += b.decoded_size;
        }
        Ok(out)
    }
}
fn append(out: &mut Vec<u8>, decoded: usize, kind: u8, payload: &[u8]) {
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
impl Compressor for Cas<'_> {
    /// Kraken level 4 in independent 64 KiB blocks; uses stored blocks when
    /// compression is no smaller (the reference CAS writer policy).
    /// Requires an Oodle handle explicitly; absent DLL is not a silent fallback.
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        let oodle = self
            .oodle
            .ok_or_else(|| invalid("cas", 0, "Oodle library required for encoding"))?;
        let mut out = Vec::new();
        if input.is_empty() {
            append(&mut out, 0, 0, &[]);
        }
        for part in input.chunks(65536) {
            let payload = oodle.compress(part)?;
            if payload.len() < part.len() {
                append(&mut out, part.len(), 0x11, &payload);
            } else {
                append(&mut out, part.len(), 0, part);
            }
        }
        Ok(out)
    }
}
