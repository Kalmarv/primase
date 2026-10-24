use crate::ooz::error::{ErrorContext, Res};

/// Oodle's LZNIB: a byte-aligned LZ77 with no entropy coding, in blocks of [`crate::ooz::SMALL_BLOCK`] bytes.
///
/// A block is a sequence of commands. A command's control byte holds the literal count in its low nibble and the
/// match code in its high nibble; the literal bytes follow it, then the match:
///
/// - literal count 0 to 14 is the count; 15 adds a length extension (below) to 15;
/// - match code 0 to 14: length `code + 4`, then the distance as two bytes, little endian;
/// - match code 15: one more byte `e`. With bit 7 set it is `1 LLLL DDD`: length `LLLL + 4` (when `LLLL` is 15 a length
///   extension follows and adds to 19) and distance `DDD`, 1 to 7, no distance bytes follow. With bit 7 clear the length
///   is `19 + e`, then two distance bytes, little endian, and when `e` is 127 a length extension (after the
///   distance) adds to 146.
///
/// A length extension is one byte, or two when the first is 0xC0 or more: `((first & 0x3F) << 8) | second`. The
/// block ends with the output: a final literal run is cut to what remains of the output.
///
/// Matches may copy from any earlier output back to the block's history start, overlapping their own output.
///
/// Derived from the bytes of 50 streams of Sekiro's archives and the output the game's library gives for them, not
/// from another decoder's source. Checked against the library: every one of those 50, and the streams its compressor 4
/// (decoder type 2) writes for 60 inputs of 1 to 600000 bytes (primase-compress `lznib_cross_check`).
#[derive(Debug)]
pub(crate) struct Lznib<'a> {
    input: &'a [u8],
    out: &'a mut [u8],
    offset: usize,
    history: usize,
}

impl ErrorContext for Lznib<'_> {
    fn describe(&self) -> Option<String> {
        Some("Lznib".into())
    }
}

impl<'a> Lznib<'a> {
    /// Decode `input` into `out[offset..]`; matches reach back to `out[history..]`.
    pub fn new(input: &'a [u8], out: &'a mut [u8], offset: usize, history: usize) -> Self {
        Lznib {
            input,
            out,
            offset,
            history,
        }
    }

    fn byte(&self, pos: &mut usize) -> Res<u8> {
        match self.input.get(*pos).copied() {
            Some(b) => {
                *pos += 1;
                Ok(b)
            }
            None => Ok(self.raise("input ends inside a command".into())?),
        }
    }

    fn extension(&self, pos: &mut usize) -> Res<usize> {
        let first = self.byte(pos)?;
        if first < 0xC0 {
            Ok(usize::from(first))
        } else {
            let second = self.byte(pos)?;
            Ok((usize::from(first & 0x3F) << 8) | usize::from(second))
        }
    }

    /// Decodes the block; returns the input bytes it used.
    pub fn decode(&mut self) -> Res<usize> {
        let end = self.out.len();
        let mut at = self.offset;
        let mut pos = 0;
        while at < end {
            let control = self.byte(&mut pos)?;
            let mut literals = usize::from(control & 0xF);
            if literals == 15 {
                literals += self.extension(&mut pos)?;
            }
            let literals = literals.min(end - at);
            let Some(from) = self.input.get(pos..pos + literals) else {
                return Ok(self.raise("input ends inside a literal run".into())?);
            };
            self.out[at..at + literals].copy_from_slice(from);
            pos += literals;
            at += literals;
            if at == end {
                break;
            }
            let code = control >> 4;
            let (length, distance) = if code < 15 {
                let low = self.byte(&mut pos)?;
                let high = self.byte(&mut pos)?;
                (usize::from(code) + 4, usize::from(low) | (usize::from(high) << 8))
            } else {
                let e = self.byte(&mut pos)?;
                if e & 0x80 != 0 {
                    let mut length = usize::from((e >> 3) & 0xF) + 4;
                    if length == 19 {
                        length += self.extension(&mut pos)?;
                    }
                    (length, usize::from(e & 7))
                } else {
                    let low = self.byte(&mut pos)?;
                    let high = self.byte(&mut pos)?;
                    let mut length = 19 + usize::from(e);
                    if e == 127 {
                        length += self.extension(&mut pos)?;
                    }
                    (length, usize::from(low) | (usize::from(high) << 8))
                }
            };
            self.assert_le(1, distance)?;
            self.assert_le(distance, at - self.history)?;
            self.assert_le(length, end - at)?;
            for i in at..at + length {
                self.out[i] = self.out[i - distance];
            }
            at += length;
        }
        Ok(pos)
    }
}
