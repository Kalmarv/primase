//! A TNLChunk's coded blocks decoded: block 0 (heights) and blocks 2 to 7 (rasters of byte values), each a u32 N
//! (little-endian), A = the block's bytes 4 to N and B = the rest; and blocks 1 and 8, LZ4 blocks of a 132 x 132 tile
//! of planar BC1 and BC4. A tile is 132 x 132 samples, row by row (y along the node's rows): a node's 128 plus the 4
//! its +x and +y neighbours start with.
//!
//! **A** is read by an H.264 CABAC arithmetic decoder (ITU-T H.264 9.3.3.2: a 9-bit range from 510 and an offset of
//! A's bits 1 to 9, its bit 0 being 0 (the game loads a 33-bit value whose top bit must be clear; refused otherwise), the
//! LPS range by `rangeTabLPS[state][(range >> 6) & 3]`, renormalised a bit at a time; its tables ITU-T H.264 Tables
//! 9-44 and 9-45, as Cisco's openh264 gives them, equal to the Riders Republic notes' copy, which they compared with
//! the game's), with 50 contexts each starting at state 0, MPS 0. Its bits run on past A into B, as the game's do; a
//! read past the block's end is refused. **B** is read as a big-endian string of raw bits, from its first.
//!
//! - **Rasters:** from B a u32 chunk length, a u32 count n and n palette bytes; the symbol list is 1-based, the
//!   palette in order. The samples are visited along an order-8 Hilbert curve from (0, 0), first step +x, clipped to
//!   the tile, cut into chunks of that length (the last shorter). Each chunk: one decision in context 8; 1 means every
//!   sample of the chunk is the list's first symbol, 0 that each is coded: a unary count k (ones, each in context
//!   min(j, 7) for the j-th, ended by a zero), k raw bits, rank = 2^k + the bits, the sample the rank-th symbol, which
//!   then swaps with the (rank / 2)-th where the rank is over 1.
//! - **Heights:** 17,424 values row by row, each a unary count k (contexts min(j, 15)), k raw bits, v = 2^k - 1 + the
//!   bits, the value (v + 1) / 2 for an odd v and -v / 2 for an even one; then summed: column 0 downward, row 0
//!   rightward, every sample from the second row on plus the one above it (in index order), every row rightward.
//!
//! The coding is the Riders Republic notes' (docs/terrain.md §3.2, from their reading of the game's decoder),
//! measured on every node of MAD_USNAT by the census here (tests/tnl.rs): what it checks is stored or
//! neighbouring values the decoding doesn't use, each B's length, each raster's palette and the node's mask, each
//! node's stored height range, and the 4 samples every node shares with its neighbours.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_foundations::compress::{Decompressor, Lz4};

/// A tile's side, in samples.
pub const SIDE: usize = 132;
/// A tile's samples.
pub const SAMPLES: usize = SIDE * SIDE;
/// An LZ4 tile's decoded bytes: 33 x 33 blocks of 8 bytes.
const TILE_BYTES: usize = 8_712;
/// The 4x4 blocks of a tile.
const TILE_BLOCKS: usize = 1_089;

/// ITU-T H.264 Table 9-44, rangeTabLPS.
const RANGE_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// ITU-T H.264 Table 9-45: (transIdxLPS, transIdxMPS) by state.
const TRANSITION: [(u8, u8); 64] = [
    (0, 1),
    (0, 2),
    (1, 3),
    (2, 4),
    (2, 5),
    (4, 6),
    (4, 7),
    (5, 8),
    (6, 9),
    (7, 10),
    (8, 11),
    (9, 12),
    (9, 13),
    (11, 14),
    (11, 15),
    (12, 16),
    (13, 17),
    (13, 18),
    (15, 19),
    (15, 20),
    (16, 21),
    (16, 22),
    (18, 23),
    (18, 24),
    (19, 25),
    (19, 26),
    (21, 27),
    (21, 28),
    (22, 29),
    (22, 30),
    (23, 31),
    (24, 32),
    (24, 33),
    (25, 34),
    (26, 35),
    (26, 36),
    (27, 37),
    (27, 38),
    (28, 39),
    (29, 40),
    (29, 41),
    (30, 42),
    (30, 43),
    (30, 44),
    (31, 45),
    (32, 46),
    (32, 47),
    (33, 48),
    (33, 49),
    (33, 50),
    (34, 51),
    (34, 52),
    (35, 53),
    (35, 54),
    (35, 55),
    (36, 56),
    (36, 57),
    (36, 58),
    (37, 59),
    (37, 60),
    (37, 61),
    (38, 62),
    (38, 62),
    (63, 63),
];

/// Why a block can't be decoded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("coded block: {0} bytes, too short for its length word")]
    Short(usize),
    #[error("coded block: A ends at {n}, outside the block of {len} bytes")]
    Split { n: usize, len: usize },
    #[error("coded block: the decoder reads past the block's end")]
    PastBlock,
    #[error("coded block: B ends after {0} raw bits")]
    RawEnd(u64),
    #[error("coded block: {what} {found} (read: {read})")]
    Value {
        what: &'static str,
        found: u64,
        read: &'static str,
    },
    #[error("coded block: height sums overflow")]
    Overflow,
    #[error("LZ4 tile: {0}")]
    Lz4(#[from] primase_foundations::compress::Error),
}

/// Bits of `bytes`, most significant first, from bit `at`.
struct Bits<'a> {
    bytes: &'a [u8],
    at: u64,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = self.bytes.get(usize::try_from(self.at / 8).ok()?)?;
        let shift = 7u64.checked_sub(self.at % 8)?;
        self.at = self.at.checked_add(1)?;
        Some(u32::from(byte >> shift & 1))
    }
}

/// The CABAC arithmetic decoder over A (running on into B).
struct Arithmetic<'a> {
    bits: Bits<'a>,
    range: u32,
    offset: u32,
    contexts: [(u8, u8); 50],
}

impl Arithmetic<'_> {
    fn new(from: &[u8]) -> Result<Arithmetic<'_>, Error> {
        let mut a = Arithmetic {
            bits: Bits { bytes: from, at: 0 },
            range: 510,
            offset: 0,
            contexts: [(0, 0); 50],
        };
        // A's first bit is 0 (the game loads 33 bits whose top must be clear), the offset the 9 after it
        if a.bits.bit().ok_or(Error::PastBlock)? != 0 {
            return Err(Error::Value {
                what: "A's first bit",
                found: 1,
                read: "0",
            });
        }
        for _ in 0..9 {
            a.offset = a.offset << 1 | a.bits.bit().ok_or(Error::PastBlock)?;
        }
        Ok(a)
    }

    /// One decision in context `k`.
    fn decide(&mut self, k: usize) -> Result<u32, Error> {
        let (state, mps) = *self.contexts.get(k).ok_or(Error::Value {
            what: "context",
            found: u64::try_from(k).unwrap_or(u64::MAX),
            read: "under 50",
        })?;
        let row = RANGE_LPS.get(usize::from(state)).ok_or(Error::PastBlock)?;
        let lps = u32::from(
            *row.get(usize::try_from(self.range >> 6 & 3).unwrap_or(0))
                .unwrap_or(&2),
        );
        let (lps_state, mps_state) = *TRANSITION.get(usize::from(state)).ok_or(Error::PastBlock)?;
        self.range = self.range.saturating_sub(lps);
        let bin = if self.offset >= self.range {
            self.offset = self.offset.saturating_sub(self.range);
            self.range = lps;
            let flipped = if state == 0 { 1u8.saturating_sub(mps) } else { mps };
            if let Some(c) = self.contexts.get_mut(k) {
                *c = (lps_state, flipped);
            }
            u32::from(1u8.saturating_sub(mps))
        } else {
            if let Some(c) = self.contexts.get_mut(k) {
                *c = (mps_state, mps);
            }
            u32::from(mps)
        };
        while self.range < 256 {
            self.range <<= 1;
            self.offset = self.offset << 1 | self.bits.bit().ok_or(Error::PastBlock)?;
        }
        Ok(bin)
    }

    /// A unary count: ones, the j-th in context `min(j, last)`, ended by a zero; refused past `most`.
    fn unary(&mut self, last: usize, most: u32) -> Result<u32, Error> {
        let mut k = 0u32;
        while self.decide(usize::try_from(k).unwrap_or(usize::MAX).min(last))? == 1 {
            k = k.checked_add(1).ok_or(Error::Overflow)?;
            if k > most {
                return Err(Error::Value {
                    what: "unary count",
                    found: u64::from(k),
                    read: "a count the code can hold",
                });
            }
        }
        Ok(k)
    }
}

/// B's raw bits.
struct Raw<'a>(Bits<'a>);

impl Raw<'_> {
    fn take(&mut self, n: u32) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..n {
            v = v << 1 | self.0.bit().ok_or(Error::RawEnd(self.0.at))?;
        }
        Ok(v)
    }
}

/// A coded block split: A's start (offset 4) to the block's end, and B.
fn split(block: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let word: [u8; 4] = block
        .get(..4)
        .and_then(|w| w.try_into().ok())
        .ok_or(Error::Short(block.len()))?;
    let n = usize::try_from(u32::from_le_bytes(word)).unwrap_or(usize::MAX);
    let split = Error::Split { n, len: block.len() };
    if n < 4 {
        return Err(split);
    }
    let b = block.get(n..).ok_or(split)?;
    Ok((block.get(4..).unwrap_or_default(), b))
}

/// A decoded raster: its chunk length, palette, samples row by row, and the raw bits it read from B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub chunk: u32,
    pub palette: Vec<u8>,
    pub samples: Vec<u8>,
    pub raw_bits: u64,
    /// B's length in bytes.
    pub b_bytes: usize,
}

/// The tile's samples in the order of an order-8 Hilbert curve from (0, 0), first step +x, clipped to the tile: each
/// as its row-major index.
pub fn hilbert() -> Vec<u16> {
    let mut out = Vec::with_capacity(SAMPLES);
    for d in 0..65_536u32 {
        let (mut x, mut y, mut t) = (0u32, 0u32, d);
        let mut s = 1u32;
        while s < 256 {
            let rx = 1 & (t / 2);
            let ry = 1 & (t ^ rx);
            if ry == 0 {
                if rx == 1 {
                    x = s.wrapping_sub(1).wrapping_sub(x);
                    y = s.wrapping_sub(1).wrapping_sub(y);
                }
                std::mem::swap(&mut x, &mut y);
            }
            x = x.wrapping_add(s.wrapping_mul(rx));
            y = y.wrapping_add(s.wrapping_mul(ry));
            t /= 4;
            s = s.wrapping_mul(2);
        }
        let side = u32::try_from(SIDE).unwrap_or(0);
        if x < side && y < side {
            out.push(u16::try_from(y.wrapping_mul(side).wrapping_add(x)).unwrap_or(0));
        }
    }
    out
}

/// Block `block` (2 to 7) decoded; `order` is [`hilbert`]'s.
pub fn raster(block: &[u8], order: &[u16]) -> Result<Raster, Error> {
    let (a, b) = split(block)?;
    let mut code = Arithmetic::new(a)?;
    let mut raw = Raw(Bits { bytes: b, at: 0 });
    let chunk = raw.take(32)?;
    let n = raw.take(32)?;
    if !(1..=256).contains(&n) {
        return Err(Error::Value {
            what: "symbol count",
            found: u64::from(n),
            read: "1 to 256",
        });
    }
    let palette: Vec<u8> = (0..n)
        .map(|_| raw.take(8).map(|v| u8::try_from(v).unwrap_or(0)))
        .collect::<Result<_, _>>()?;
    if chunk == 0 {
        return Err(Error::Value {
            what: "chunk length",
            found: 0,
            read: "at least 1",
        });
    }
    let mut list: Vec<u8> = std::iter::once(0).chain(palette.iter().copied()).collect();
    let mut samples = vec![0u8; SAMPLES];
    let chunk_len = usize::try_from(chunk).unwrap_or(usize::MAX);
    for part in order.chunks(chunk_len) {
        if code.decide(8)? == 1 {
            let front = *list.get(1).unwrap_or(&0);
            for &i in part {
                if let Some(s) = samples.get_mut(usize::from(i)) {
                    *s = front;
                }
            }
            continue;
        }
        for &i in part {
            let k = code.unary(7, 8)?;
            let bits = raw.take(k)?;
            let rank = usize::try_from((1u32 << k).saturating_add(bits)).unwrap_or(usize::MAX);
            let value = *list.get(rank).filter(|_| rank >= 1).ok_or(Error::Value {
                what: "rank",
                found: u64::try_from(rank).unwrap_or(u64::MAX),
                read: "at most the symbol count",
            })?;
            if let Some(s) = samples.get_mut(usize::from(i)) {
                *s = value;
            }
            if rank > 1 {
                list.swap(rank, rank / 2);
            }
        }
    }
    Ok(Raster {
        chunk,
        palette,
        samples,
        raw_bits: raw.0.at,
        b_bytes: b.len(),
    })
}

/// A decoded height block: its values row by row (fractions of the terrain's range, in units of 2^(level - 28)
/// at the node's level), and the raw bits it read from B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heights {
    pub values: Vec<i64>,
    pub raw_bits: u64,
    pub b_bytes: usize,
}

/// Block 0 decoded.
pub fn heights(block: &[u8]) -> Result<Heights, Error> {
    let (a, b) = split(block)?;
    let mut code = Arithmetic::new(a)?;
    let mut raw = Raw(Bits { bytes: b, at: 0 });
    let mut v: Vec<i64> = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let k = code.unary(15, 31)?;
        let bits = i64::from(raw.take(k)?);
        let u = (1i64 << k)
            .checked_sub(1)
            .and_then(|x| x.checked_add(bits))
            .ok_or(Error::Overflow)?;
        let half = u.checked_div(2).ok_or(Error::Overflow)?;
        let value = if u.checked_rem(2) == Some(1) {
            half.checked_add(1)
        } else {
            half.checked_neg()
        };
        v.push(value.ok_or(Error::Overflow)?);
    }
    // `to` gains `from`'s value, both sample indices (under SAMPLES, so their sums and products here can't overflow)
    let add = |v: &mut Vec<i64>, to: usize, from: usize| -> Result<(), Error> {
        let f = *v.get(from).ok_or(Error::Overflow)?;
        let t = v.get_mut(to).ok_or(Error::Overflow)?;
        *t = t.checked_add(f).ok_or(Error::Overflow)?;
        Ok(())
    };
    let at = |r: usize, x: usize| r.wrapping_mul(SIDE).wrapping_add(x);
    for r in 1..SIDE {
        add(&mut v, at(r, 0), at(r.wrapping_sub(1), 0))?;
    }
    for x in 1..SIDE {
        add(&mut v, x, x.wrapping_sub(1))?;
    }
    for i in SIDE..SAMPLES {
        add(&mut v, i, i.wrapping_sub(SIDE))?;
    }
    for r in 0..SIDE {
        for x in 1..SIDE {
            add(&mut v, at(r, x), at(r, x.wrapping_sub(1)))?;
        }
    }
    Ok(Heights {
        values: v,
        raw_bits: raw.0.at,
        b_bytes: b.len(),
    })
}

/// Block 1 or 8 decoded to its tile's 1,089 interleaved 8-byte blocks, row by row: BC1 blocks (two RGB565 words, then
/// the indices) from block 1's planar endpoints-then-indices, or BC4 blocks (two endpoints, then six index bytes)
/// from block 8's.
pub fn tile(block: &[u8], bc4: bool) -> Result<Vec<u8>, Error> {
    let planar = Lz4.decompress(block, TILE_BYTES)?;
    let head = if bc4 { 2 } else { 4 };
    let split = TILE_BLOCKS.saturating_mul(head);
    let (ends, rest) = planar.split_at(split.min(planar.len()));
    let tail = 8usize.saturating_sub(head);
    let mut out = Vec::with_capacity(TILE_BYTES);
    for (e, i) in ends.chunks(head).zip(rest.chunks(tail)) {
        out.extend_from_slice(e);
        out.extend_from_slice(i);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hilbert_walk_starts_as_the_notes_list() {
        let h = hilbert();
        assert_eq!(h.len(), SAMPLES);
        assert_eq!(h.get(..8), Some(&[0, 1, 133, 132, 264, 396, 397, 265][..]));
        let mut seen = h.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), SAMPLES);
    }

    #[test]
    fn a_constant_raster_stub_decodes() {
        // the notes' 41-byte stub for the value 0x2d: N = 15, A = 43 80 00 .., B = chunk 32768, one symbol
        let mut b = vec![15, 0, 0, 0, 0x43, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend([0x00, 0x00, 0x80, 0x00, 0, 0, 0, 1, 0x2d]);
        b.extend([0; 9]);
        b.extend([1, 2, 3, 4, 5, 6, 7, 8]);
        let r = raster(&b, &hilbert()).expect("stub");
        assert_eq!(
            (r.chunk, r.palette.clone(), r.raw_bits, r.b_bytes),
            (32_768, vec![0x2d], 72, 26)
        );
        assert!(r.samples.iter().all(|&s| s == 0x2d));
    }
}
