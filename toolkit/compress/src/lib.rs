//! Strict, size-bounded compression adapters. No format autodetection.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use std::io::Write;

pub mod cas;
#[allow(unsafe_code)]
pub mod oodle;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{codec} at input offset {offset}: {detail}")]
    Invalid {
        codec: &'static str,
        offset: usize,
        detail: String,
    },
    #[error("output size {actual}, expected {expected}")]
    Size { expected: usize, actual: usize },
    #[error("allocation/size limit exceeded")]
    Limit,
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("Oodle library: {0}")]
    Library(#[from] libloading::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn invalid(codec: &'static str, offset: usize, detail: impl ToString) -> Error {
    Error::Invalid {
        codec,
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
/// Decode one exactly framed payload with an externally supplied output size.
/// Implementations reject output overrun; the caller must bound untrusted sizes.
pub trait Decompressor {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>>;
}
/// Encode one payload; framing belongs to the selected codec.
pub trait Compressor {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>>;
}

/// Raw LZ4 block, no size prefix. All input bytes belong to the block; a final
/// literal sequence terminates it. Output size comes from the enclosing archive.
#[derive(Debug, Clone, Copy)]
pub struct Lz4;
impl Decompressor for Lz4 {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        if input.is_empty() {
            return Err(invalid("lz4", 0, "missing token"));
        }
        let mut out = buffer(expected)?;
        let n = lz4_flex::block::decompress_into(input, &mut out).map_err(|e| invalid("lz4", 0, e))?;
        check_size(n, expected)?;
        Ok(out)
    }
}
impl Compressor for Lz4 {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        Ok(lz4_flex::block::compress(input))
    }
}

/// RFC1951 raw DEFLATE. Requires the final block marker and exact input consumption.
#[derive(Debug, Clone, Copy)]
pub struct Deflate;
/// RFC1950 zlib: header, DEFLATE, Adler32 trailer. Requires StreamEnd and exact consumption.
#[derive(Debug, Clone, Copy)]
pub struct Zlib;
/// Sunset TOC compatibility: accepts a zlib stream with missing final marker or
/// checksum ONLY if all input is consumed, no decoder error occurs, and exactly
/// the expected bytes are produced. Incomplete streams cannot authenticate Adler32;
/// this is a size check, not an integrity check. Complete streams are also accepted.
#[derive(Debug, Clone, Copy)]
pub struct ZlibUnterminated;
fn inflate(input: &[u8], expected: usize, zlib: bool, incomplete: bool) -> Result<Vec<u8>> {
    let mut d = flate2::Decompress::new(zlib);
    let mut out = buffer(expected.checked_add(1).ok_or(Error::Limit)?)?;
    let status = d
        .decompress(input, &mut out, flate2::FlushDecompress::None)
        .map_err(|e| invalid("deflate/zlib", d.total_in() as usize, e))?;
    check_size(d.total_out() as usize, expected)?;
    if d.total_in() as usize != input.len() {
        return Err(invalid("deflate/zlib", d.total_in() as usize, "trailing input"));
    }
    if status != flate2::Status::StreamEnd && !incomplete {
        return Err(invalid("deflate/zlib", input.len(), "missing stream end"));
    }
    // Even incomplete empty zlib needs a valid two-byte header.
    if zlib && input.len() < 2 {
        return Err(invalid("zlib", input.len(), "short header"));
    }
    out.truncate(expected);
    Ok(out)
}
impl Decompressor for Deflate {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        inflate(input, expected, false, false)
    }
}
impl Decompressor for Zlib {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        inflate(input, expected, true, false)
    }
}
impl Decompressor for ZlibUnterminated {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        inflate(input, expected, true, true)
    }
}
impl Compressor for Deflate {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(input)?;
        Ok(e.finish()?)
    }
}
impl Compressor for Zlib {
    fn compress(&self, input: &[u8]) -> Result<Vec<u8>> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(input)?;
        Ok(e.finish()?)
    }
}
