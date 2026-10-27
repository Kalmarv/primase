//! DCX DFLT, KRAK and ZSTD envelopes. Only documented header permutations are accepted.
use crate::{Result, bytes::View, invalid};
use primase_foundations::compress::{Decompressor, Zlib, Zstd, oodle::Oodle};
/// The compression a DCX envelope names at offset 40 (`KRAK`, `DFLT`), or `None` for bytes that are not one.
pub fn compression(data: &[u8]) -> Option<[u8; 4]> {
    if !data.starts_with(b"DCX\0") {
        return None;
    }
    data.get(40..44)?.try_into().ok()
}
/// How an envelope's payload is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Oodle Kraken: needs the game's library.
    Kraken,
    /// zlib.
    Deflate,
    /// One zstd frame: Elden Ring's `regulation.bin` once decrypted.
    Zstd,
}

/// A validated DCX envelope: how its payload is compressed, the payload as stored and the size it decodes to.
#[derive(Debug, Clone, Copy)]
pub struct Envelope<'a> {
    pub compression: Compression,
    pub payload: &'a [u8],
    pub decoded_size: usize,
}

impl<'a> Envelope<'a> {
    /// Reads the envelope's header, accepting only the documented permutations; a decoded size over `max_output` is refused.
    pub fn parse(data: &'a [u8], max_output: usize) -> Result<Self> {
        let v = View {
            data,
            format: "DCX",
            be: true,
        };
        v.expect(0, b"DCX\0")?;
        for (p, value) in [
            (8, 0x18),
            (12, 0x24),
            (44, 0x20),
            (52, 0),
            (60, 0),
            (64, 0x10100),
            (72, 8),
        ] {
            if v.u32(p)? != value {
                return Err(invalid("DCX", p, "unsupported header field"));
            }
        }
        v.expect(24, b"DCS\0")?;
        v.expect(36, b"DCP\0")?;
        v.expect(68, b"DCA\0")?;
        v.expect(49, &[0, 0, 0])?;
        v.expect(57, &[0, 0, 0])?;
        let version = v.u32(4)?;
        let a = v.u32(16)?;
        let b = v.u32(20)?;
        let level = v.u8(48)?;
        let flag = v.u8(56)?;
        if b != if a == 0x24 { 0x2c } else { 0x4c } {
            return Err(invalid("DCX", 20, "header offsets"));
        }
        let decoded_size = v.u32(28)? as usize;
        let size = v.u32(32)? as usize;
        if decoded_size > max_output {
            return Err(invalid("DCX", 28, "output exceeds configured limit"));
        }
        let payload = v.slice(76, size)?;
        let end = 76 + size;
        if data.len() - end >= 16 || data[end..].iter().any(|&x| x != 0) {
            return Err(invalid(
                "DCX",
                end,
                "trailing data, expected at most 15 zero alignment bytes",
            ));
        }
        let compression = match v.slice(40, 4)? {
            b"KRAK" => {
                if version != 0x11000 || a != 0x44 || ![6, 9].contains(&level) || flag != 0 {
                    return Err(invalid("DCX", 4, "unknown KRAK variant"));
                }
                Compression::Kraken
            }
            b"DFLT" => {
                if !matches!(
                    (version, a, level, flag),
                    (0x10000, 0x24 | 0x44, 9, 0) | (0x11000, 0x44, 8 | 9, 0) | (0x11000, 0x44, 9, 15)
                ) {
                    return Err(invalid("DCX", 4, "unknown DFLT variant"));
                }
                Compression::Deflate
            }
            // SoulsFormats' `DCX.DecompressDCXZSTD`: KRAK's header with any level byte
            b"ZSTD" => {
                if version != 0x11000 || a != 0x44 || flag != 0 {
                    return Err(invalid("DCX", 4, "unknown ZSTD variant"));
                }
                Compression::Zstd
            }
            _ => return Err(invalid("DCX", 40, "unsupported compression")),
        };
        Ok(Self {
            compression,
            payload,
            decoded_size,
        })
    }
}

/// Explicit native decoder and caller-controlled allocation ceiling in bytes.
#[derive(Debug, Clone, Copy)]
pub struct Decoder<'a> {
    pub oodle: Option<&'a Oodle>,
    pub max_output: usize,
}
impl Decoder<'_> {
    pub fn decode(&self, data: &[u8]) -> Result<Vec<u8>> {
        let envelope = Envelope::parse(data, self.max_output)?;
        match envelope.compression {
            Compression::Kraken => Ok(self
                .oodle
                .ok_or_else(|| invalid("DCX", 40, "Oodle required"))?
                .decompress(envelope.payload, envelope.decoded_size)?),
            Compression::Deflate => Ok(Zlib.decompress(envelope.payload, envelope.decoded_size)?),
            Compression::Zstd => Ok(Zstd.decompress(envelope.payload, envelope.decoded_size)?),
        }
    }
    pub fn decode_if_dcx(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.starts_with(b"DCX\0") {
            self.decode(data)
        } else {
            Ok(data.to_vec())
        }
    }
}
#[cfg(test)]
mod compression_tests {
    use super::{Compression, Decoder, Envelope};

    /// A KRAK envelope (the layout of a 0x11000 `DCX` with the 0x44 `DCP` block, level 9) around `payload`.
    fn krak(decoded: u32, payload: &[u8]) -> Vec<u8> {
        envelope(b"KRAK", decoded, payload)
    }

    /// An envelope of `kind` laid out as [`krak`]'s.
    fn envelope(kind: &[u8; 4], decoded: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = b"DCX\0".to_vec();
        for (at, word) in [(4, 0x11000u32), (8, 0x18), (12, 0x24), (16, 0x44), (20, 0x4c)] {
            b.resize(at, 0);
            b.extend(word.to_be_bytes());
        }
        b.extend(*b"DCS\0");
        b.extend(decoded.to_be_bytes());
        b.extend((payload.len() as u32).to_be_bytes());
        b.extend(*b"DCP\0");
        b.extend(*kind);
        b.extend(0x20u32.to_be_bytes());
        b.extend([9, 0, 0, 0]);
        b.extend(0u32.to_be_bytes());
        b.extend([0, 0, 0, 0]);
        b.extend(0u32.to_be_bytes());
        b.extend(0x10100u32.to_be_bytes());
        b.extend(*b"DCA\0");
        b.extend(8u32.to_be_bytes());
        b.extend(payload);
        b
    }

    #[test]
    fn an_envelope_names_its_payload_and_decoded_size() {
        let bytes = krak(1000, &[1, 2, 3]);
        let e = Envelope::parse(&bytes, usize::MAX).unwrap();
        assert_eq!(e.compression, Compression::Kraken);
        assert_eq!((e.decoded_size, e.payload), (1000, &[1, 2, 3][..]));
        // a decoded size over the ceiling, and trailing bytes that are not alignment zeros, are refused
        assert!(Envelope::parse(&bytes, 999).is_err());
        let mut dirty = bytes;
        dirty.push(1);
        assert!(Envelope::parse(&dirty, usize::MAX).is_err());
    }

    #[test]
    fn a_zstd_envelope_decodes_its_frame() {
        let text = b"NpcParam.param";
        // one zstd frame (RFC 8878): magic, a single-segment descriptor with a one-byte content size, and one raw block
        // (last, type 0, its size in the top bits of the three-byte header)
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, text.len() as u8];
        frame.extend(&(((text.len() as u32) << 3) | 1).to_le_bytes()[..3]);
        frame.extend(text);
        let bytes = envelope(b"ZSTD", text.len() as u32, &frame);
        assert_eq!(
            Envelope::parse(&bytes, usize::MAX).unwrap().compression,
            Compression::Zstd
        );
        let decoder = Decoder {
            oodle: None,
            max_output: 1 << 20,
        };
        assert_eq!(decoder.decode(&bytes).unwrap(), text);
        // a size the frame does not decode to is refused
        assert!(decoder.decode(&envelope(b"ZSTD", 3, &frame)).is_err());
    }

    #[test]
    fn the_envelope_names_its_compression() {
        let mut krak = b"DCX\0".to_vec();
        krak.resize(40, 0);
        krak.extend_from_slice(b"KRAK");
        assert_eq!(super::compression(&krak), Some(*b"KRAK"));
        assert_eq!(super::compression(&krak[..42]), None);
        assert_eq!(super::compression(b"BND4"), None);
    }
}
