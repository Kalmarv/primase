//! QOI ("Quite OK Image", specification 1.0, 2022-01-05) decoded to straight RGBA8.
//!
//! A 14-byte header (`qoif`, width and height big-endian u32, channels 3 or 4, colorspace 0 or 1), then chunks, each
//! making one or more pixels from the previous pixel (initially opaque black) and a 64-entry table of pixels seen,
//! indexed by `(r * 3 + g * 5 + b * 7 + a * 11) % 64` (initially all zero): `QOI_OP_RGB` (0xFE, alpha kept),
//! `QOI_OP_RGBA` (0xFF), `QOI_OP_INDEX` (`00`, a table entry), `QOI_OP_DIFF` (`01`, each channel -2..1, wrapping),
//! `QOI_OP_LUMA` (`10`, green -32..31 and red and blue -8..7 relative to it, wrapping) and `QOI_OP_RUN` (`11`, the
//! previous pixel 1..62 times); the stream ends with seven 0x00 bytes and a 0x01. Strict: a run past the last pixel,
//! a stream ending early, or bytes other than the end marker after the last pixel are errors. The header's channels
//! and colorspace describe the source and change no pixel: they are returned with it.

use crate::{Error, Image, Result, Samples, allocate, sample_count};

/// The channels the header says the source had: the pixels are RGBA either way (alpha 255 throughout for `Rgb`,
/// as the encoder saw it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QoiChannels {
    Rgb,
    Rgba,
}

/// The header's colorspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QoiColorspace {
    /// sRGB colour channels, linear alpha.
    SrgbLinearAlpha,
    /// Every channel linear.
    Linear,
}

/// A decoded QOI image: its pixels and what the header says of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qoi {
    /// Straight RGBA8, rows top first.
    pub image: Image,
    pub channels: QoiChannels,
    pub colorspace: QoiColorspace,
}

const MAGIC: &[u8; 4] = b"qoif";
const HEADER: usize = 14;
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
const OP_RGB: u8 = 0xFE;
const OP_RGBA: u8 = 0xFF;
const OP_INDEX: u8 = 0x00;
const OP_DIFF: u8 = 0x40;
const OP_LUMA: u8 = 0x80;
const OP_RUN: u8 = 0xC0;
const MASK: u8 = 0xC0;

fn hash([r, g, b, a]: [u8; 4]) -> usize {
    (usize::from(r) * 3 + usize::from(g) * 5 + usize::from(b) * 7 + usize::from(a) * 11) % 64
}

fn invalid(detail: String) -> Error {
    Error::Invalid(format!("QOI: {detail}"))
}

/// Decode a QOI file. See the module documentation.
pub fn read_qoi(data: &[u8]) -> Result<Qoi> {
    let header = data
        .get(..HEADER)
        .ok_or_else(|| invalid(format!("{} bytes, shorter than the header", data.len())))?;
    if &header[..4] != MAGIC {
        return Err(invalid(format!("magic {:02x?}, not \"qoif\"", &header[..4])));
    }
    let width = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    let height = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let channels = match header[12] {
        3 => QoiChannels::Rgb,
        4 => QoiChannels::Rgba,
        c => return Err(invalid(format!("channels {c}, not 3 or 4"))),
    };
    let colorspace = match header[13] {
        0 => QoiColorspace::SrgbLinearAlpha,
        1 => QoiColorspace::Linear,
        c => return Err(invalid(format!("colorspace {c}, not 0 or 1"))),
    };
    let samples = sample_count(width, height)?;
    let mut out: Vec<u8> = allocate(samples)?;
    let pixels = samples / 4;
    let mut table = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut at = HEADER;
    let byte = |at: &mut usize| -> Result<u8> {
        let b = *data
            .get(*at)
            .ok_or_else(|| invalid(format!("the stream ends at byte {at} before the last pixel")))?;
        *at += 1;
        Ok(b)
    };
    let mut done = 0usize;
    while done < pixels {
        let op = byte(&mut at)?;
        let mut run = 1usize;
        match op {
            OP_RGB => {
                px = [byte(&mut at)?, byte(&mut at)?, byte(&mut at)?, px[3]];
            }
            OP_RGBA => {
                px = [byte(&mut at)?, byte(&mut at)?, byte(&mut at)?, byte(&mut at)?];
            }
            _ => match op & MASK {
                OP_INDEX => px = table[usize::from(op)],
                OP_DIFF => {
                    let d = |shift: u8| ((op >> shift) & 0x03).wrapping_sub(2);
                    px = [
                        px[0].wrapping_add(d(4)),
                        px[1].wrapping_add(d(2)),
                        px[2].wrapping_add(d(0)),
                        px[3],
                    ];
                }
                OP_LUMA => {
                    let dg = (op & 0x3F).wrapping_sub(32);
                    let next = byte(&mut at)?;
                    let dr = (next >> 4).wrapping_sub(8).wrapping_add(dg);
                    let db = (next & 0x0F).wrapping_sub(8).wrapping_add(dg);
                    px = [
                        px[0].wrapping_add(dr),
                        px[1].wrapping_add(dg),
                        px[2].wrapping_add(db),
                        px[3],
                    ];
                }
                // 0xFE and 0xFF are taken above, so the bias leaves runs of 1..=62
                OP_RUN => {
                    run = usize::from(op & 0x3F) + 1;
                    if run > pixels - done {
                        return Err(invalid(format!(
                            "a run of {run} at byte {} passes the last pixel ({} left)",
                            at - 1,
                            pixels - done
                        )));
                    }
                }
                _ => unreachable!("two bits"),
            },
        }
        table[hash(px)] = px;
        for _ in 0..run {
            out[done * 4..done * 4 + 4].copy_from_slice(&px);
            done += 1;
        }
    }
    let rest = &data[at..];
    if rest != END {
        return Err(invalid(format!(
            "after the last pixel (byte {at}): {:02x?}, not the end marker {END:02x?}",
            &rest[..rest.len().min(16)]
        )));
    }
    Ok(Qoi {
        image: Image {
            width,
            height,
            samples: Samples::U8(out),
        },
        channels,
        colorspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A QOI file of `w` x `h` (channels 4, colorspace 0) with these chunks and the end marker.
    fn file(w: u32, h: u32, chunks: &[u8]) -> Vec<u8> {
        let mut f = b"qoif".to_vec();
        f.extend_from_slice(&w.to_be_bytes());
        f.extend_from_slice(&h.to_be_bytes());
        f.extend_from_slice(&[4, 0]);
        f.extend_from_slice(chunks);
        f.extend_from_slice(&END);
        f
    }

    fn pixels(q: &Qoi) -> Vec<[u8; 4]> {
        match &q.image.samples {
            Samples::U8(v) => v.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect(),
            Samples::U16(_) => panic!("QOI is 8-bit"),
        }
    }

    /// Every chunk the specification defines, each pixel worked out by hand from it.
    #[test]
    fn each_chunk_decodes_as_the_specification_says() {
        let chunks = [
            // RGB: alpha stays the initial 255
            OP_RGB,
            10,
            20,
            30, //
            // RGBA
            OP_RGBA,
            100,
            150,
            200,
            128, //
            // INDEX of the first pixel: hash(10, 20, 30, 255) = (30 + 100 + 210 + 2805) % 64 = 9
            OP_INDEX | 9, //
            // DIFF dr -2 (0 in the top pair), dg +1 (3), db 0 (2): 10-2, 20+1, 30
            OP_DIFF | (3 << 2) | 2, //
            // LUMA dg +5 (37), dr-dg -3 (5), db-dg +7 (15): 8+2, 21+5, 30+12
            OP_LUMA | 37,
            (5 << 4) | 15, //
            // RUN of 3 (bias -1)
            OP_RUN | 2,
        ];
        let q = read_qoi(&file(4, 2, &chunks)).unwrap();
        assert_eq!((q.image.width, q.image.height), (4, 2));
        assert_eq!(
            (q.channels, q.colorspace),
            (QoiChannels::Rgba, QoiColorspace::SrgbLinearAlpha)
        );
        assert_eq!(
            pixels(&q),
            [
                [10, 20, 30, 255],
                [100, 150, 200, 128],
                [10, 20, 30, 255],
                [8, 21, 30, 255],
                [10, 26, 42, 255],
                [10, 26, 42, 255],
                [10, 26, 42, 255],
                [10, 26, 42, 255],
            ]
        );
    }

    /// Differences wrap around 0 and 255; the table starts all zero (an index before any pixel is transparent
    /// black), and the first pixel's predecessor is opaque black.
    #[test]
    fn differences_wrap_and_the_starting_state_is_the_specifications() {
        let chunks = [
            // DIFF -1 on every channel from (0, 0, 0, 255)
            OP_DIFF | (1 << 4) | (1 << 2) | 1,
            // LUMA dg +31, dr-dg +7, db-dg -8 from (255, 255, 255): 255+38, 255+31, 255+23
            OP_LUMA | 63,
            15 << 4,
            OP_INDEX | 5,
        ];
        let q = read_qoi(&file(3, 1, &chunks)).unwrap();
        assert_eq!(
            pixels(&q),
            [[255, 255, 255, 255], [37, 30, 22, 255], [0, 0, 0, 0]]
        );
    }

    #[test]
    fn the_header_and_the_stream_are_strict() {
        let one = file(1, 1, &[OP_RGB, 1, 2, 3]);
        assert!(read_qoi(&one).is_ok());
        let err = |f: &[u8]| read_qoi(f).unwrap_err().to_string();
        let mut bad = one.clone();
        bad[0] = b'Q';
        assert!(err(&bad).contains("magic"));
        let mut bad = one.clone();
        bad[12] = 2;
        assert!(err(&bad).contains("channels 2"));
        let mut bad = one.clone();
        bad[13] = 2;
        assert!(err(&bad).contains("colorspace 2"));
        assert!(err(&one[..10]).contains("shorter than the header"));
        assert!(err(&file(0, 1, &[])).contains("zero dimension"));
        // the stream ends before the last pixel
        assert!(err(&one[..16]).contains("before the last pixel"));
        // a run past the last pixel
        assert!(err(&file(2, 1, &[OP_RUN | 2])).contains("passes the last pixel"));
        // no end marker, or bytes after it
        assert!(err(&one[..one.len() - 1]).contains("end marker"));
        let mut long = one.clone();
        long.push(0);
        assert!(err(&long).contains("end marker"));
        // channels 3: the pixels are RGBA all the same
        let mut rgb = one.clone();
        rgb[12] = 3;
        let q = read_qoi(&rgb).unwrap();
        assert_eq!(q.channels, QoiChannels::Rgb);
        assert_eq!(pixels(&q), [[1, 2, 3, 255]]);
    }
}
