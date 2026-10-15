//! DCX DFLT and KRAK envelopes. Only documented header permutations are accepted.
use crate::{Result, bytes::View, invalid};
use primase_compress::{Decompressor, Zlib, oodle::Oodle};
/// Explicit native decoder and caller-controlled allocation ceiling in bytes.
#[derive(Debug, Clone, Copy)]
pub struct Decoder<'a> {
    pub oodle: Option<&'a Oodle>,
    pub max_output: usize,
}
impl Decoder<'_> {
    pub fn decode(&self, data: &[u8]) -> Result<Vec<u8>> {
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
        let expected = v.u32(28)? as usize;
        let size = v.u32(32)? as usize;
        if expected > self.max_output {
            return Err(invalid("DCX", 28, "output exceeds configured limit"));
        }
        let src = v.slice(76, size)?;
        let end = 76 + size;
        if data.len() - end >= 16 || data[end..].iter().any(|&x| x != 0) {
            return Err(invalid(
                "DCX",
                end,
                "trailing data, expected at most 15 zero alignment bytes",
            ));
        }
        match v.slice(40, 4)? {
            b"KRAK" => {
                if version != 0x11000 || a != 0x44 || ![6, 9].contains(&level) || flag != 0 {
                    return Err(invalid("DCX", 4, "unknown KRAK variant"));
                }
                Ok(self
                    .oodle
                    .ok_or_else(|| invalid("DCX", 40, "Oodle required"))?
                    .decompress(src, expected)?)
            }
            b"DFLT" => {
                if !matches!(
                    (version, a, level, flag),
                    (0x10000, 0x24 | 0x44, 9, 0) | (0x11000, 0x44, 8 | 9, 0) | (0x11000, 0x44, 9, 15)
                ) {
                    return Err(invalid("DCX", 4, "unknown DFLT variant"));
                }
                Ok(Zlib.decompress(src, expected)?)
            }
            _ => Err(invalid("DCX", 40, "unsupported compression")),
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
