//! The plumbing the primase format crates share, for reading game files safely:
//!
//! - [`bytes`]: checked byte spans and cursors, the byte order stated on every multi-byte read;
//! - [`hash`]: the hashes game files key names by (CRC-32, Adler-32 from any start, Jenkins one-at-a-time, MD5 with
//!   its block function, SDBM), never normalizing their input;
//! - [`vertex`]: checked vertex layouts, quantization, packed positions and index topology;
//! - [`parallel`]: an ordered, panic-safe parallel map over independent items;
//! - [`color`]: the sRGB transfer curve, in f64 and f32, and its byte forms;
//! - [`sampling`]: texture sampling as GPUs define it: address modes, bilinear and trilinear filtering, a level of
//!   detail from a sample's footprint, 2D and 3D;
//! - [`compress`] (feature `compress`): one decompress/compress interface over the codecs games store: Oodle through
//!   the game's own library (Windows) or oozextract (elsewhere), LZ4, deflate, zlib, zstd and LZO2A.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bytes;
pub mod color;
#[cfg(feature = "compress")]
pub mod compress;
pub mod hash;
pub mod parallel;
pub mod sampling;
pub mod vertex;

/// This crate's source files, for a consumer that keys a disk cache by the code that computed its values: editing any
/// of them retires those entries. A test keeps the list equal to the `src` folder.
pub const SOURCES: &[(&str, &[u8])] = &[
    ("foundations/bytes.rs", include_bytes!("bytes.rs")),
    ("foundations/color.rs", include_bytes!("color.rs")),
    (
        "foundations/compress/lzo2a.rs",
        include_bytes!("compress/lzo2a.rs"),
    ),
    ("foundations/compress/mod.rs", include_bytes!("compress/mod.rs")),
    (
        "foundations/compress/oodle.rs",
        include_bytes!("compress/oodle.rs"),
    ),
    (
        "foundations/compress/reference.rs",
        include_bytes!("compress/reference.rs"),
    ),
    ("foundations/hash.rs", include_bytes!("hash.rs")),
    ("foundations/lib.rs", include_bytes!("lib.rs")),
    ("foundations/parallel.rs", include_bytes!("parallel.rs")),
    ("foundations/sampling.rs", include_bytes!("sampling.rs")),
    ("foundations/vertex.rs", include_bytes!("vertex.rs")),
];
