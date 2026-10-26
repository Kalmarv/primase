//! Strict, size-bounded compression adapters. No format autodetection.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

use std::io::Write;

mod lzo2a;
pub use lzo2a::Lzo2a;
#[allow(unsafe_code)]
pub mod oodle;
#[cfg(any(feature = "reference-decoder", not(windows)))]
pub mod reference;

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

/// The most bytes an LZ4 block can produce for one byte of input (a match length is extended by bytes of 255, each
/// adding 255 to it), rounded up: a claimed output size above this times the input is false.
pub const LZ4_MAX_RATIO: usize = 255;

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

/// One zstd frame (RFC 8878), decoded exactly: no skippable frame, no dictionary, a stated content size (where the
/// frame states one) equal to the expected size, every input byte consumed, a stated content checksum verified, and
/// never more than the expected output (decoded a block at a time, refused as soon as it would exceed it).
#[derive(Debug, Clone, Copy)]
pub struct Zstd;
impl Decompressor for Zstd {
    fn decompress(&self, input: &[u8], expected: usize) -> Result<Vec<u8>> {
        use ruzstd::{BlockDecodingStrategy, FrameDecoder};
        use std::io::Read;
        let (frame, _) = ruzstd::frame::read_frame_header(input).map_err(|e| invalid("zstd", 0, e))?;
        if let Some(id) = frame.header.dictionary_id() {
            return Err(invalid("zstd", 0, format!("frame needs dictionary {id}")));
        }
        let descriptor = &frame.header.descriptor;
        let states_size = descriptor.single_segment_flag()
            || descriptor
                .frame_content_size_bytes()
                .map_err(|e| invalid("zstd", 0, e))?
                > 0;
        if states_size {
            let stated = usize::try_from(frame.header.frame_content_size()).map_err(|_| Error::Limit)?;
            check_size(stated, expected)?;
        }
        let mut d = FrameDecoder::new();
        let mut src = input;
        d.init(&mut src).map_err(|e| invalid("zstd", 0, e))?;
        let mut out = Vec::new();
        out.try_reserve_exact(expected).map_err(|_| Error::Limit)?;
        loop {
            let before = src.len();
            d.decode_blocks(&mut src, BlockDecodingStrategy::UptoBlocks(1))
                .map_err(|e| invalid("zstd", input.len() - before, e))?;
            let ready = d.can_collect();
            let total = out.len().checked_add(ready).ok_or(Error::Limit)?;
            if total > expected {
                return Err(Error::Size {
                    expected,
                    actual: total,
                });
            }
            let at = out.len();
            out.resize(total, 0);
            d.read_exact(&mut out[at..])?;
            if d.is_finished() && d.can_collect() == 0 {
                break;
            }
            if src.len() == before && ready == 0 {
                return Err(invalid(
                    "zstd",
                    input.len() - src.len(),
                    "frame ends before its last block",
                ));
            }
        }
        if !src.is_empty() {
            return Err(invalid(
                "zstd",
                input.len() - src.len(),
                "trailing input after the frame",
            ));
        }
        if let (Some(stored), Some(computed)) = (d.get_checksum_from_data(), d.get_calculated_checksum())
            && stored != computed
        {
            return Err(invalid(
                "zstd",
                input.len(),
                format!("content checksum {stored:08x}, computed {computed:08x}"),
            ));
        }
        check_size(out.len(), expected)?;
        Ok(out)
    }
}

/// RFC1951 raw DEFLATE. Requires the final block marker and exact input consumption.
#[derive(Debug, Clone, Copy)]
pub struct Deflate;
/// RFC1950 zlib: header, DEFLATE, Adler32 trailer. Requires StreamEnd and exact consumption.
#[derive(Debug, Clone, Copy)]
pub struct Zlib;
/// TOC compatibility: some sources' TOCs store a zlib stream with a missing final marker or
/// checksum; that is accepted ONLY if all input is consumed, no decoder error occurs, and exactly
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
/// The most bytes a DEFLATE stream can produce for one byte of input (RFC 1951: a match of 258 bytes takes about two
/// bits), rounded up: a claimed output size above this times the input is false.
pub const DEFLATE_MAX_RATIO: usize = 1032;

impl Deflate {
    /// A complete raw DEFLATE stream whose output size is not stated, at most `max` bytes. The output grows with what
    /// the stream produces (at most [`DEFLATE_MAX_RATIO`] times its input), never with `max`, so a caller's bound
    /// taken from an untrusted header costs no memory by itself. Requires the final block marker and exact input
    /// consumption; more than `max` bytes of output is [`Error::Size`] (`actual` is then `max + 1`).
    pub fn decompress_up_to(&self, input: &[u8], max: usize) -> Result<Vec<u8>> {
        let mut d = flate2::Decompress::new(false);
        let mut out: Vec<u8> = Vec::new();
        loop {
            if out.len() == out.capacity() {
                // double, but never past one byte more than `max`, which is what shows the stream overran it
                let want = out
                    .capacity()
                    .max(4096)
                    .saturating_mul(2)
                    .min(max.saturating_add(1));
                let more = want.saturating_sub(out.len());
                if more == 0 {
                    return Err(Error::Size {
                        expected: max,
                        actual: out.len(),
                    });
                }
                out.try_reserve_exact(more).map_err(|_| Error::Limit)?;
            }
            let taken = usize::try_from(d.total_in()).unwrap_or(usize::MAX);
            let rest = input.get(taken..).unwrap_or_default();
            let before = (d.total_in(), d.total_out());
            let status = d
                .decompress_vec(rest, &mut out, flate2::FlushDecompress::None)
                .map_err(|e| invalid("deflate", taken, e))?;
            if out.len() > max {
                return Err(Error::Size {
                    expected: max,
                    actual: out.len(),
                });
            }
            if status == flate2::Status::StreamEnd {
                break;
            }
            if (d.total_in(), d.total_out()) == before && out.len() < out.capacity() {
                return Err(invalid("deflate", input.len(), "missing stream end"));
            }
        }
        if usize::try_from(d.total_in()).unwrap_or(usize::MAX) != input.len() {
            return Err(invalid("deflate", d.total_in() as usize, "trailing input"));
        }
        Ok(out)
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
