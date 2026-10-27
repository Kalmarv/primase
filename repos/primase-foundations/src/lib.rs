//! The plumbing the primase format crates share, for reading game files safely:
//!
//! - [`bytes`]: checked byte spans and cursors, the byte order stated on every multi-byte read;
//! - [`hash`]: the hashes game files key names by (CRC-32, Adler-32 from any start, Jenkins one-at-a-time, MD5 with
//!   its block function, SDBM), never normalizing their input;
//! - [`vertex`]: checked vertex layouts, quantization, packed positions and index topology;
//! - [`parallel`]: an ordered, panic-safe parallel map over independent items;
//! - [`compress`] (feature `compress`): one decompress/compress interface over the codecs games store: Oodle through
//!   the game's own library (Windows) or oozextract (elsewhere), LZ4, deflate, zlib, zstd and LZO2A.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bytes;
#[cfg(feature = "compress")]
pub mod compress;
pub mod hash;
pub mod parallel;
pub mod vertex;
