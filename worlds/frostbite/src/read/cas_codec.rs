//! The CAS codec: a stream of blocks ([`crate::cas`]) decoded to its bytes, the Oodle ones through the caller's handle to the
//! game's library.
use primase_compress::{Compressor, Decompressor, Error, Result, Zlib, oodle::Oodle};

use crate::cas::{append, blocks, buffer, check_size, invalid};

/// CAS codec, optional explicit Oodle implementation. Decode supports raw, zlib,
/// and kinds 0x11/0x15/0x19. Unknown kinds are errors with physical offsets.
#[derive(Debug, Default)]
pub struct Cas<'a> {
    pub oodle: Option<&'a Oodle>,
}
impl Decompressor for Cas<'_> {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        let blocks = blocks(input)?;
        let total = blocks
            .iter()
            .try_fold(0usize, |s, b| s.checked_add(b.decoded_size).ok_or(Error::Limit))?;
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
                        .map_err(|e| invalid(b.offset, e))?,
                ),
                0x11 | 0x15 | 0x19 => {
                    let oodle = self
                        .oodle
                        .ok_or_else(|| invalid(b.offset, "Oodle library required"))?;
                    dst.copy_from_slice(
                        &oodle
                            .decompress(b.payload, b.decoded_size)
                            .map_err(|e| invalid(b.offset, e))?,
                    );
                }
                _ => {
                    return Err(invalid(b.offset, format!("unsupported kind {:#x}", b.kind)));
                }
            }
            pos += b.decoded_size;
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
            .ok_or_else(|| invalid(0, "Oodle library required for encoding"))?;
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
