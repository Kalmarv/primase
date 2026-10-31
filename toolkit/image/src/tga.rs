//! Truevision TGA (specification 2.0, 1989) decoded to straight RGBA8.
//!
//! An 18-byte header (identification field length, colour map type, image type, colour map first entry, length and entry
//! size, the image's x and y origin, width and height as little-endian u16, pixel depth, image descriptor), the
//! identification field, the colour map, then the pixels: colour-mapped (type 1), true-colour (2) or greyscale (3),
//! each run-length encoded as types 9, 10 and 11 (packets of a header byte, its top bit set for one pixel repeated
//! `(byte & 0x7F) + 1` times, clear for that many pixels stored; a packet may run on past the end of a row).
//!
//! Pixels: true-colour 15 or 16 bits (x5 red, green, blue, low bits blue; its top bit alpha when the descriptor gives one
//! alpha bit), 24 (blue, green, red) or 32 (blue, green, red, alpha); greyscale 8 or 16 (grey, then alpha); a
//! colour map's entries are true-colour pixels of its entry size, indexed by 8- or 16-bit pixels from its first entry.
//! Five-bit channels widen as `(v << 3) | (v >> 2)`. The descriptor's low four bits are the alpha bits per pixel: with
//! none a pixel's attribute bits are not alpha and the image is opaque (alpha 255), as the specification has it. An
//! 8-bit grey pixel is grey whatever alpha bits the descriptor states (some writers state 8 for a single-channel
//! image, which leaves the pixel no bits for grey): opaque. Its
//! bit 5 set stores rows top first (else bottom first), bit 4 columns right to left; rows are returned top first, left
//! to right. The footer and extension area (TGA 2.0) and anything else after the pixels are not read.
//!
//! Strict: a header stating more pixels than the data can hold (refused before the image is allocated), data ending
//! early, a colour map index past its entries, or a type, depth or colour map this does not describe are errors.

use crate::{Error, Image, Result, Samples, allocate, sample_count};

/// How the file stores its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TgaKind {
    /// Indices into the file's colour map (types 1 and 9).
    ColorMapped,
    /// Colour pixels (types 2 and 10).
    TrueColor,
    /// Grey pixels (types 3 and 11).
    Greyscale,
}

/// A decoded TGA image: its pixels and what the header says of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tga {
    /// Straight RGBA8, rows top first.
    pub image: Image,
    pub kind: TgaKind,
    /// Run-length encoded (types 9 to 11).
    pub run_length: bool,
    /// Bits per stored pixel.
    pub depth: u8,
    /// Whether the pixels carry alpha (the descriptor states alpha bits); else alpha is 255 throughout.
    pub alpha: bool,
}

const HEADER: usize = 18;
/// The most pixels one packet makes.
const MOST_PIXELS_PER_PACKET: usize = 128;

fn invalid(detail: String) -> Error {
    Error::Invalid(format!("TGA: {detail}"))
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn widen5(v: u16) -> u8 {
    let v = (v & 0x1F) as u8;
    (v << 3) | (v >> 2)
}

/// One true-colour pixel of `bytes` (its depth's length) as RGBA, its alpha kept when `alpha`.
fn true_color(bytes: &[u8], alpha: bool) -> [u8; 4] {
    match bytes.len() {
        2 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            let a = if alpha && v & 0x8000 == 0 { 0 } else { 255 };
            [widen5(v >> 10), widen5(v >> 5), widen5(v), a]
        }
        3 => [bytes[2], bytes[1], bytes[0], 255],
        _ => [bytes[2], bytes[1], bytes[0], if alpha { bytes[3] } else { 255 }],
    }
}

/// Decode a TGA file. See the module documentation.
pub fn read_tga(data: &[u8]) -> Result<Tga> {
    let h = data
        .get(..HEADER)
        .ok_or_else(|| invalid(format!("{} bytes, shorter than the header", data.len())))?;
    let (id_len, map_type, image_type) = (usize::from(h[0]), h[1], h[2]);
    let (map_first, map_len, map_depth) = (usize::from(u16_at(h, 3)), usize::from(u16_at(h, 5)), h[7]);
    let (width, height, depth, descriptor) =
        (u32::from(u16_at(h, 12)), u32::from(u16_at(h, 14)), h[16], h[17]);
    let (kind, run_length) = match image_type {
        1 => (TgaKind::ColorMapped, false),
        2 => (TgaKind::TrueColor, false),
        3 => (TgaKind::Greyscale, false),
        9 => (TgaKind::ColorMapped, true),
        10 => (TgaKind::TrueColor, true),
        11 => (TgaKind::Greyscale, true),
        t => return Err(invalid(format!("image type {t}, none of 1, 2, 3, 9, 10 and 11"))),
    };
    let alpha_bits = descriptor & 0x0F;
    // an 8-bit grey pixel has no bits for alpha beside its grey, whatever the descriptor says
    let alpha = alpha_bits > 0 && !(kind == TgaKind::Greyscale && depth == 8);
    let pixel_ok = match kind {
        TgaKind::TrueColor => matches!((depth, alpha_bits), (15 | 16, 0 | 1) | (24, 0) | (32, 0 | 8)),
        TgaKind::Greyscale => matches!((depth, alpha_bits), (8, 0 | 8) | (16, 0 | 8)),
        TgaKind::ColorMapped => matches!(depth, 8 | 16),
    };
    if !pixel_ok {
        return Err(invalid(format!(
            "{depth} bits per {kind:?} pixel with {alpha_bits} alpha bits"
        )));
    }
    // the colour map: kept as RGBA, its entries true-colour pixels of its own depth
    let map_bytes = match (kind, map_type) {
        (TgaKind::ColorMapped, 1) => {
            if !matches!(map_depth, 15 | 16 | 24 | 32) {
                return Err(invalid(format!("colour map entries of {map_depth} bits")));
            }
            usize::from(map_depth).div_ceil(8)
        }
        (TgaKind::ColorMapped, t) => {
            return Err(invalid(format!("a colour-mapped image with colour map type {t}")));
        }
        (_, 0) => 0,
        // a colour map beside a true-colour or grey image is skipped, as the specification allows
        (_, 1) => usize::from(map_depth).div_ceil(8),
        (_, t) => return Err(invalid(format!("colour map type {t}"))),
    };
    let map_start = HEADER + id_len;
    let pixels_start = map_start + map_len * map_bytes;
    let stored = data.get(pixels_start..).ok_or_else(|| {
        invalid(format!(
            "{} bytes end before the pixels at byte {pixels_start}",
            data.len()
        ))
    })?;
    let map: Vec<[u8; 4]> = if kind == TgaKind::ColorMapped {
        data[map_start..pixels_start]
            .chunks_exact(map_bytes)
            .map(|e| true_color(e, alpha))
            .collect()
    } else {
        Vec::new()
    };
    let bytes_per = usize::from(depth).div_ceil(8);
    let samples = sample_count(width, height)?;
    let pixels = samples / 4;
    // refused before the image is allocated, which the header alone sizes
    let most = if run_length {
        stored.len() / (1 + bytes_per) * MOST_PIXELS_PER_PACKET
    } else {
        stored.len() / bytes_per
    };
    if most < pixels {
        return Err(invalid(format!(
            "{} bytes of pixels cannot hold the {width} x {height} it states",
            stored.len()
        )));
    }
    let pixel = |p: &[u8]| -> Result<[u8; 4]> {
        Ok(match kind {
            TgaKind::TrueColor => true_color(p, alpha),
            TgaKind::Greyscale => [p[0], p[0], p[0], if alpha { p[1] } else { 255 }],
            TgaKind::ColorMapped => {
                let index = if p.len() == 1 {
                    usize::from(p[0])
                } else {
                    usize::from(u16::from_le_bytes([p[0], p[1]]))
                };
                *index
                    .checked_sub(map_first)
                    .and_then(|i| map.get(i))
                    .ok_or_else(|| {
                        invalid(format!(
                            "colour map index {index} outside {map_first}..{}",
                            map_first + map.len()
                        ))
                    })?
            }
        })
    };
    // the pixels in stored order
    let mut order: Vec<[u8; 4]> = allocate(pixels)?;
    let mut at = 0usize;
    let take = |at: &mut usize, n: usize| -> Result<&[u8]> {
        let s = stored.get(*at..*at + n).ok_or_else(|| {
            invalid(format!(
                "the pixels end at byte {} before the last pixel",
                pixels_start + *at
            ))
        })?;
        *at += n;
        Ok(s)
    };
    if run_length {
        let mut done = 0usize;
        while done < pixels {
            let packet = take(&mut at, 1)?[0];
            let n = usize::from(packet & 0x7F) + 1;
            if n > pixels - done {
                return Err(invalid(format!(
                    "a packet of {n} at byte {} passes the last pixel ({} left)",
                    pixels_start + at - 1,
                    pixels - done
                )));
            }
            if packet & 0x80 != 0 {
                let px = pixel(take(&mut at, bytes_per)?)?;
                order[done..done + n].fill(px);
            } else {
                for slot in &mut order[done..done + n] {
                    *slot = pixel(take(&mut at, bytes_per)?)?;
                }
            }
            done += n;
        }
    } else {
        for slot in order.iter_mut() {
            *slot = pixel(take(&mut at, bytes_per)?)?;
        }
    }
    // rows top first, left to right
    let (w, ht) = (width as usize, height as usize);
    let (top_first, right_to_left) = (descriptor & 0x20 != 0, descriptor & 0x10 != 0);
    let mut out: Vec<u8> = allocate(samples)?;
    for (k, px) in order.into_iter().enumerate() {
        let (row, col) = (k / w, k % w);
        let y = if top_first { row } else { ht - 1 - row };
        let x = if right_to_left { w - 1 - col } else { col };
        let o = (y * w + x) * 4;
        out[o..o + 4].copy_from_slice(&px);
    }
    Ok(Tga {
        image: Image {
            width,
            height,
            samples: Samples::U8(out),
        },
        kind,
        run_length,
        depth,
        alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TGA file of `w` x `h` of `image_type`, `depth` bits and `descriptor`, with these bytes after the header
    /// (identification field none, no colour map unless the bytes hold one and `map` states it).
    fn file(
        image_type: u8,
        w: u16,
        h: u16,
        depth: u8,
        descriptor: u8,
        map: Option<(u16, u16, u8)>,
        rest: &[u8],
    ) -> Vec<u8> {
        let (first, len, entry) = map.unwrap_or((0, 0, 0));
        let mut f = vec![0, u8::from(map.is_some()), image_type];
        f.extend_from_slice(&first.to_le_bytes());
        f.extend_from_slice(&len.to_le_bytes());
        f.push(entry);
        f.extend_from_slice(&[0, 0, 0, 0]);
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&h.to_le_bytes());
        f.extend_from_slice(&[depth, descriptor]);
        f.extend_from_slice(rest);
        f
    }

    fn pixels(t: &Tga) -> Vec<[u8; 4]> {
        match &t.image.samples {
            Samples::U8(v) => v.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect(),
            Samples::U16(_) => panic!("TGA decodes to 8 bits"),
        }
    }

    /// 32-bit BGRA stored bottom row first (descriptor 8: eight alpha bits, bottom-left origin): rows come back top
    /// first, channels as RGBA, alpha kept.
    #[test]
    fn true_color_bottom_first_comes_back_top_first() {
        let rest = [
            0, 0, 255, 10, /* bottom left: red */ 0, 255, 0, 20, /* bottom right: green */
            255, 0, 0, 30, /* top left: blue */ 1, 2, 3, 40, /* top right */
        ];
        let t = read_tga(&file(2, 2, 2, 32, 8, None, &rest)).unwrap();
        assert_eq!(
            pixels(&t),
            [[0, 0, 255, 30], [3, 2, 1, 40], [255, 0, 0, 10], [0, 255, 0, 20]]
        );
        assert!(t.alpha);
        assert_eq!((t.kind, t.run_length, t.depth), (TgaKind::TrueColor, false, 32));
    }

    /// 24 bits, top first and right to left (descriptor 0x30), then a TGA 2.0 footer after the pixels: opaque, mirrored
    /// back, the footer not read.
    #[test]
    fn origin_bits_and_trailing_footer() {
        let mut rest = vec![1, 2, 3, 4, 5, 6];
        rest.extend_from_slice(b"\0\0\0\0\0\0\0\0TRUEVISION-XFILE.\0");
        let t = read_tga(&file(2, 2, 1, 24, 0x30, None, &rest)).unwrap();
        assert_eq!(pixels(&t), [[6, 5, 4, 255], [3, 2, 1, 255]]);
    }

    /// 32 bits with no alpha bits stated: the fourth byte is not alpha.
    #[test]
    fn no_alpha_bits_is_opaque() {
        let t = read_tga(&file(2, 1, 1, 32, 0x20, None, &[1, 2, 3, 0])).unwrap();
        assert_eq!(pixels(&t), [[3, 2, 1, 255]]);
        assert!(!t.alpha);
    }

    /// Greyscale 8 and 16 bits (grey then alpha), 15-bit colour widened, and a run-length packet that runs on past a row.
    #[test]
    fn grey_sixteen_bit_and_run_length() {
        let g = read_tga(&file(3, 2, 1, 8, 0x20, None, &[7, 200])).unwrap();
        assert_eq!(pixels(&g), [[7, 7, 7, 255], [200, 200, 200, 255]]);
        // 8 alpha bits stated for an 8-bit grey pixel: grey, opaque
        let g8 = read_tga(&file(3, 1, 1, 8, 0x08, None, &[42])).unwrap();
        assert_eq!(pixels(&g8), [[42, 42, 42, 255]]);
        assert!(!g8.alpha);
        let ga = read_tga(&file(3, 1, 1, 16, 0x28, None, &[9, 99])).unwrap();
        assert_eq!(pixels(&ga), [[9, 9, 9, 99]]);
        // 0b0_11111_00000_00001: red 31, green 0, blue 1 -> 255, 0, 8
        let c = read_tga(&file(2, 1, 1, 16, 0x20, None, &0x7C01u16.to_le_bytes())).unwrap();
        assert_eq!(pixels(&c), [[255, 0, 8, 255]]);
        // 3 x 2, top first: a run of 4 then 2 raw pixels
        let rle = read_tga(&file(11, 3, 2, 8, 0x20, None, &[0x83, 50, 0x01, 60, 70])).unwrap();
        assert_eq!(
            pixels(&rle).iter().map(|p| p[0]).collect::<Vec<_>>(),
            [50, 50, 50, 50, 60, 70]
        );
        assert!(rle.run_length);
    }

    /// A colour map's entries from its first index, 24-bit, indexed by 8-bit pixels.
    #[test]
    fn color_mapped() {
        let mut rest = vec![0, 0, 255, 0, 255, 0];
        rest.extend_from_slice(&[5, 4]);
        let t = read_tga(&file(1, 2, 1, 8, 0x20, Some((4, 2, 24)), &rest)).unwrap();
        assert_eq!(pixels(&t), [[0, 255, 0, 255], [255, 0, 0, 255]]);
        let mut bad = vec![0, 0, 255, 0, 255, 0];
        bad.extend_from_slice(&[3, 4]);
        assert!(read_tga(&file(1, 2, 1, 8, 0x20, Some((4, 2, 24)), &bad)).is_err());
    }

    /// Short data, a packet past the last pixel, and unknown types are refused.
    #[test]
    fn malformed_files_are_refused() {
        assert!(read_tga(&[0; 10]).is_err());
        // 65535 x 65535 stated by 4 bytes: refused before allocating
        assert!(read_tga(&file(2, u16::MAX, u16::MAX, 32, 8, None, &[0; 4])).is_err());
        assert!(read_tga(&file(2, 2, 1, 24, 0, None, &[1, 2, 3])).is_err());
        assert!(read_tga(&file(11, 1, 1, 8, 0, None, &[0x81, 5])).is_err());
        assert!(read_tga(&file(4, 1, 1, 8, 0, None, &[0])).is_err());
        assert!(read_tga(&file(2, 1, 1, 24, 8, None, &[0, 0, 0])).is_err());
    }
}
