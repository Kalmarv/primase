//! JPEG (ITU-T T.81 baseline and progressive, Huffman coded, 8 bits per sample) decoded to straight RGBA8 by the
//! pure-Rust `zune-jpeg` decoder.
//!
//! - **Pixels.** Grey, YCbCr and RGB-coded files give RGBA with alpha 255 (JPEG has none); the file's colour model is
//!   returned with them ([`JpegColor`]). The values are the decoder's own (its integer IDCT and chroma upsampling),
//!   not a bit-exact libjpeg's: a different decoder's pixels differ by a few levels at most, which the gates state.
//! - **Deterministic.** The decoder is built without its SIMD features (the only unsafe code it has) and is asked for
//!   none, so every machine decodes to the same bytes.
//! - **Strict.** The decoder's strict mode is on: a file that departs from the standard is an error rather than a
//!   best-effort picture. CMYK and YCCK files (four components, whose inversion convention the file does not state
//!   reliably), 12-bit, arithmetic-coded and lossless files are refused by name, never guessed.
//! - **Ignored.** EXIF (including its orientation), an ICC profile and XMP are not applied: the pixels are the coded
//!   ones. Those present are returned in [`Jpeg::ignored`].

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::{Error, Image, Result, Samples, allocate, sample_count};

/// The colour model the file codes its pixels in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegColor {
    /// One component.
    Grey,
    /// Three components, luma and two chroma (the usual JFIF file).
    YCbCr,
    /// Three components coded as red, green and blue (an Adobe marker with transform 0).
    Rgb,
}

/// A decoded JPEG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jpeg {
    /// Straight RGBA8, rows top first, alpha 255.
    pub image: Image,
    pub color: JpegColor,
    /// Whether the file was coded in progressive scans.
    pub progressive: bool,
    /// Metadata the file carries that the pixels do not reflect (`EXIF`, `ICC profile`, `XMP`).
    pub ignored: Vec<&'static str>,
}

fn invalid(detail: impl std::fmt::Display) -> Error {
    Error::Jpeg(detail.to_string())
}

/// Whether `data` starts like a JPEG file (the start-of-image marker and the next marker's prefix).
#[must_use]
pub fn is_jpeg(data: &[u8]) -> bool {
    data.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// Decode a JPEG file. See the module documentation.
pub fn read_jpeg(data: &[u8]) -> Result<Jpeg> {
    if !is_jpeg(data) {
        return Err(invalid("no start-of-image marker"));
    }
    let options = DecoderOptions::default()
        .set_strict_mode(true)
        .set_use_unsafe(false)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().map_err(invalid)?;
    let info = decoder.info().ok_or_else(|| invalid("headers not read"))?;
    let color = match decoder.input_colorspace() {
        Some(ColorSpace::Luma) => JpegColor::Grey,
        Some(ColorSpace::YCbCr) => JpegColor::YCbCr,
        Some(ColorSpace::RGB) => JpegColor::Rgb,
        Some(other @ (ColorSpace::CMYK | ColorSpace::YCCK)) => {
            return Err(invalid(format!("{other:?} files are not decoded")));
        }
        other => return Err(invalid(format!("colour model {other:?} is not decoded"))),
    };
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    // the buffer is allocated fallibly, and sized from the header before any pixel is decoded
    let mut rgba = allocate::<u8>(sample_count(width, height)?)?;
    decoder.decode_into(&mut rgba).map_err(invalid)?;
    let mut ignored = Vec::new();
    if decoder.exif().is_some() {
        ignored.push("EXIF");
    }
    if decoder.icc_profile().is_some() {
        ignored.push("ICC profile");
    }
    if decoder.xmp().is_some() {
        ignored.push("XMP");
    }
    Ok(Jpeg {
        image: Image {
            width,
            height,
            samples: Samples::U8(rgba),
        },
        color,
        // the decoder does not export its frame-type enum, only its Debug names (`ProgressiveDctHuffman`)
        progressive: format!("{:?}", info.sof).starts_with("Progressive"),
        ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_not_a_jpeg_is_refused_by_name() {
        for bytes in [&b""[..], b"\x89PNG\r\n\x1a\n", b"\xFF\xD8"] {
            assert!(matches!(read_jpeg(bytes), Err(Error::Jpeg(_))), "{bytes:?}");
        }
        assert!(is_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 1]));
        assert!(!is_jpeg(b"\x89PNG"));
    }

    #[test]
    fn a_truncated_header_is_an_error_not_a_panic() {
        let truncated = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F'];
        assert!(read_jpeg(&truncated).is_err());
    }
}
