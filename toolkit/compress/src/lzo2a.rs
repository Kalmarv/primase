//! LZO2A (Markus Oberhumer's LZO, `lzo2a_decompress_safe` of LZO 2.10, built with its end-of-stream code): a
//! byte-oriented LZ77 whose literal/match flags come from a bit buffer.
//!
//! The stream interleaves the bit buffer's bytes with literal and match bytes: whenever the decoder needs a flag bit
//! and holds too few, it takes the next input byte into the buffer, least significant bit first. Each step:
//! - flag 0: one literal byte follows;
//! - flags 1 0: a short match, length 2 + the next two flag bits, distance 1 + the next byte (back up to 256);
//! - flags 1 1: two bytes `a b`: distance `(a & 31) | b << 5`, length `a >> 5`. A length of 0 is a long match:
//!   length 9 + the bytes after it (each zero byte adds 255, the first non-zero ends the count). A length of 1 to 7
//!   is a match of that length + 2, except distance 0, which ends the stream.
//!
//! Strict as the crate's other codecs: the end-of-stream code is required, every input byte must be consumed by then,
//! a match may not reach before the output's start, and the output must be exactly the expected size.

use crate::{Decompressor, Error, Result, invalid};

const CODEC: &str = "lzo2a";

/// LZO2A, one complete stream (the module documentation).
#[derive(Debug, Clone, Copy)]
pub struct Lzo2a;

struct Stream<'a> {
    input: &'a [u8],
    at: usize,
    bits: u32,
    held: u32,
}

impl Stream<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self
            .input
            .get(self.at)
            .ok_or_else(|| invalid(CODEC, self.at, "stream ends before its end code"))?;
        self.at += 1;
        Ok(b)
    }

    /// The next `n` (1 or 2) flag bits, least significant first.
    fn flags(&mut self, n: u32) -> Result<u32> {
        if self.held < n {
            let b = self.byte()?;
            self.bits |= u32::from(b) << self.held;
            self.held += 8;
        }
        let v = self.bits & ((1 << n) - 1);
        self.bits >>= n;
        self.held -= n;
        Ok(v)
    }
}

impl Decompressor for Lzo2a {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve_exact(expected).map_err(|_| Error::Limit)?;
        let mut s = Stream {
            input,
            at: 0,
            bits: 0,
            held: 0,
        };
        loop {
            let (length, distance) = if s.flags(1)? == 0 {
                let b = s.byte()?;
                if out.len() == expected {
                    return Err(Error::Size {
                        expected,
                        actual: expected + 1,
                    });
                }
                out.push(b);
                continue;
            } else if s.flags(1)? == 0 {
                let length = 2 + s.flags(2)? as usize;
                (length, 1 + usize::from(s.byte()?))
            } else {
                let a = s.byte()?;
                let b = s.byte()?;
                let distance = usize::from(a & 31) | usize::from(b) << 5;
                let short = usize::from(a >> 5);
                if short == 0 {
                    let mut length = 9usize;
                    loop {
                        match s.byte()? {
                            0 => length = length.checked_add(255).ok_or(Error::Limit)?,
                            n => {
                                length = length.checked_add(usize::from(n)).ok_or(Error::Limit)?;
                                break;
                            }
                        }
                    }
                    (length, distance)
                } else if distance == 0 {
                    break;
                } else {
                    (short + 2, distance)
                }
            };
            let from = out
                .len()
                .checked_sub(distance)
                .filter(|_| distance > 0)
                .ok_or_else(|| {
                    invalid(
                        CODEC,
                        s.at,
                        format!("match {distance} back at output {}", out.len()),
                    )
                })?;
            let end = out.len().checked_add(length).ok_or(Error::Limit)?;
            if end > expected {
                return Err(Error::Size {
                    expected,
                    actual: end,
                });
            }
            // the match may overlap what it writes (distance < length repeats a run): copy byte by byte
            for k in from..from + length {
                let b = out[k];
                out.push(b);
            }
        }
        if s.at != input.len() {
            return Err(invalid(CODEC, s.at, "input after the end code"));
        }
        if out.len() != expected {
            return Err(Error::Size {
                expected,
                actual: out.len(),
            });
        }
        Ok(out)
    }
}
