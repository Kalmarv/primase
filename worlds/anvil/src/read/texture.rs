//! A TextureMap resource's body: its size, format code, mip count, references to its external mips, and the mip
//! levels it holds itself, read as stored. Pixels are not decoded here (`primase-texcodec` decodes a level once its
//! format is known); colour space and role are not read.
//!
//! Width, height, mip count, format code and the references are established by the fixtures: the levels they give
//! fill the pixels exactly, the referenced CompiledMips are exactly the missing levels' sizes, and the levels decode
//! (worlds/anvil/tests/texture.rs). The reference tool's names for the u32s at 9, 13, 21 and 25 (depth, array size,
//! dimension, gamma) are its claims, not established here: the first three are 1 on every fixture and only 1 is read,
//! the fourth is kept as stored.
//!
//! The body (little-endian, after the resource's class id and extension): a byte (1); u32 width, height, the u32 at 9,
//! the u32 at 13, the engine format code, the u32 at 21, the u32 at 25 and the mip count; 14 bytes; the reference
//! count r (u32 at 47) and r
//! references (a tag byte, 0, and a u64 ClassID: the CompiledMips holding the first r levels); 9 bytes; at 60 + 9r the
//! compiled header (60 bytes: 0x13237FE9, equal to the CRC-32 of `CompiledTextureMap`; u32 1; then width, height,
//! the u32s at 9 and 13, mip count, format code and the u32s at 21 and 25 again; 16 bytes; and the pixel byte
//! length); then the pixels, ending
//! the body: levels r to mip count - 1, largest first, each its format's size for its dimensions.
//!
//! Established on three TextureMaps of the base WhiteRoom forge (Riders Republic build 25394987: entries 217, 218 and
//! 31; BC1 and BC7, 9 and 11 levels, none and two external; games/riders-republic docs/recon/texture-format-validation.md,
//! bff28dc and 0ad60f1, where each level's bytes and both formats' decodes are checked independently). Refused, naming
//! it: another first byte, u32 at 9, 13 or 21, reference tag or compiled header marker, a compiled header
//! disagreeing with the first, or pixels not ending the body. [`format`] knows the format codes those fixtures
//! establish (2 BC1, 10 BC7); [`StoredTextureMap::levels`] refuses another.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_core::TexFormat;

pub use crate::read::material::TEXTURE_MAP;

/// The compiled header's marker: equals the CRC-32 of `CompiledTextureMap` (the name a label).
pub const COMPILED_TEXTURE_MAP: u32 = 0x1323_7FE9;

/// The texture format a TextureMap's engine format code names, for the codes established on fixtures: 2 (BC1: map
/// 217, every level's bytes and its top level decoded equal to the third-party bcdec's) and 10 (BC7: maps 218 and 31,
/// 218's top level decoded equal to the independent texture2ddecoder's). Another code is `None`: not refused as
/// stored, but its layout isn't known.
pub fn format(code: u32) -> Option<TexFormat> {
    match code {
        2 => Some(TexFormat::Bc1),
        10 => Some(TexFormat::Bc7),
        _ => None,
    }
}

/// Why a TextureMap body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("TextureMap body: {have} bytes, shorter than its {what}")]
    Short { what: &'static str, have: usize },
    #[error("TextureMap {what}: {found}, only {read} is read")]
    Value {
        what: &'static str,
        found: u64,
        read: u64,
    },
    #[error("TextureMap compiled header: {what} {found}, the header's {stored}")]
    Compiled {
        what: &'static str,
        found: u32,
        stored: u32,
    },
    #[error("TextureMap pixels: {why}")]
    Pixels { why: String },
}

/// A reference to an external mip level as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MipReference {
    /// The CompiledMip's ClassID.
    pub id: u64,
}

/// A TextureMap body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTextureMap {
    pub width: u32,
    pub height: u32,
    /// The engine's format code ([`format`]).
    pub format: u32,
    /// The u32 at 25, as stored (1 on map 217, 0 on 218 and 31; what it selects is not established).
    pub field_25: u32,
    pub mip_count: u32,
    /// The 14 bytes after the mip count.
    pub head_rest: [u8; 14],
    /// The CompiledMips holding the first levels, in level order.
    pub references: Vec<MipReference>,
    /// The 9 bytes after the references.
    pub references_end: [u8; 9],
    /// The compiled header's 16 bytes before its pixel byte length.
    pub compiled_rest: [u8; 16],
    /// Levels `references.len()` onwards, as stored.
    pub pixels: Vec<u8>,
}

/// Little-endian fields read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let short = Error::Short {
            what,
            have: self.b.len(),
        };
        let end = self.at.checked_add(N).ok_or(Error::Short {
            what,
            have: self.b.len(),
        })?;
        let s: [u8; N] = self
            .b
            .get(self.at..end)
            .and_then(|s| s.try_into().ok())
            .ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    /// A u32 that must be `read`.
    fn only(&mut self, what: &'static str, read: u32) -> Result<(), Error> {
        let found = self.u32(what)?;
        if found == read {
            Ok(())
        } else {
            Err(Error::Value {
                what,
                found: u64::from(found),
                read: u64::from(read),
            })
        }
    }
}

impl StoredTextureMap {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredTextureMap, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let [flag] = c.array::<1>("first byte")?;
        if flag != 1 {
            return Err(Error::Value {
                what: "first byte",
                found: u64::from(flag),
                read: 1,
            });
        }
        let width = c.u32("width")?;
        let height = c.u32("height")?;
        c.only("u32 at 9", 1)?;
        c.only("u32 at 13", 1)?;
        let format = c.u32("format")?;
        c.only("u32 at 21", 1)?;
        let field_25 = c.u32("u32 at 25")?;
        let mip_count = c.u32("mip count")?;
        let head_rest = c.array::<14>("bytes after the mip count")?;
        let count = c.u32("reference count")?;
        let mut references = Vec::new();
        for _ in 0..count {
            let [tag] = c.array::<1>("reference tag")?;
            if tag != 0 {
                return Err(Error::Value {
                    what: "reference tag",
                    found: u64::from(tag),
                    read: 0,
                });
            }
            references.push(MipReference {
                id: u64::from_le_bytes(c.array::<8>("reference")?),
            });
        }
        let references_end = c.array::<9>("bytes after the references")?;
        c.only("compiled header marker", COMPILED_TEXTURE_MAP)?;
        c.only("compiled header", 1)?;
        let again = [
            ("width", width),
            ("height", height),
            ("u32 at 9", 1),
            ("u32 at 13", 1),
            ("mip count", mip_count),
            ("format", format),
            ("u32 at 21", 1),
            ("u32 at 25", field_25),
        ];
        for (what, stored) in again {
            let found = c.u32(what)?;
            if found != stored {
                return Err(Error::Compiled { what, found, stored });
            }
        }
        let compiled_rest = c.array::<16>("compiled header")?;
        let length = c.u32("pixel byte length")?;
        let pixels = body.get(c.at..).unwrap_or_default();
        if usize::try_from(length).ok() != Some(pixels.len()) {
            return Err(Error::Pixels {
                why: format!("{length} bytes stated, {} ending the body", pixels.len()),
            });
        }
        Ok(StoredTextureMap {
            width,
            height,
            format,
            field_25,
            mip_count,
            head_rest,
            references,
            references_end,
            compiled_rest,
            pixels: pixels.to_vec(),
        })
    }

    /// Level `i`'s width and height (each halved per level, at least 1).
    pub fn level_size(&self, i: u32) -> (u32, u32) {
        let half = |x: u32| x.checked_shr(i).unwrap_or(0).max(1);
        (half(self.width), half(self.height))
    }

    /// The levels this body holds (from `references.len()` to the mip count), each its bytes, refused unless the
    /// format is known and they fill the pixels exactly.
    pub fn levels(&self) -> Result<Vec<&[u8]>, Error> {
        let f = format(self.format).ok_or_else(|| Error::Pixels {
            why: format!("format code {} isn't established", self.format),
        })?;
        let first = u32::try_from(self.references.len()).unwrap_or(u32::MAX);
        let mut out = Vec::new();
        let mut at = 0usize;
        for i in first..self.mip_count {
            let (w, h) = self.level_size(i);
            let size = f.checked_level_size(w, h).ok_or_else(|| Error::Pixels {
                why: format!("level {i}: {w} x {h} past this machine"),
            })?;
            let end = at.saturating_add(size);
            out.push(self.pixels.get(at..end).ok_or_else(|| Error::Pixels {
                why: format!("level {i} ({w} x {h}) past the {} pixel bytes", self.pixels.len()),
            })?);
            at = end;
        }
        if at != self.pixels.len() {
            return Err(Error::Pixels {
                why: format!(
                    "levels {first} to {} fill {at} of {} pixel bytes",
                    self.mip_count,
                    self.pixels.len()
                ),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    /// A body of these fields, references and pixels.
    fn body(w: u32, h: u32, format: u32, mips: u32, refs: &[u64], pixels: &[u8]) -> Vec<u8> {
        let mut b = vec![1];
        for x in [w, h, 1, 1, format, 1, 0, mips] {
            b.extend(x.to_le_bytes());
        }
        b.extend([0; 14]);
        b.extend(u32::try_from(refs.len()).expect("few").to_le_bytes());
        for r in refs {
            b.push(0);
            b.extend(r.to_le_bytes());
        }
        b.extend([0; 9]);
        b.extend(COMPILED_TEXTURE_MAP.to_le_bytes());
        for x in [1, w, h, 1, 1, mips, format, 1, 0] {
            b.extend(x.to_le_bytes());
        }
        b.extend([0; 16]);
        b.extend(u32::try_from(pixels.len()).expect("small").to_le_bytes());
        b.extend(pixels);
        b
    }

    #[test]
    fn a_body_reads_its_fields_and_slices_its_levels() {
        // 8 x 4 BC1 with 3 levels, the first external: levels 1 (4 x 2) and 2 (2 x 1) are one block each
        let t = StoredTextureMap::parse(&body(8, 4, 2, 3, &[77], &[1; 16])).expect("parse");
        assert_eq!((t.width, t.height, t.format, t.mip_count), (8, 4, 2, 3));
        assert_eq!(t.references, [MipReference { id: 77 }]);
        assert_eq!(t.levels().expect("levels"), [&[1u8; 8][..], &[1; 8][..]]);
    }

    #[test]
    fn a_malformed_body_or_level_run_is_refused() {
        let good = body(8, 4, 2, 3, &[77], &[1; 16]);
        let mut b = good.clone();
        b[0] = 2;
        assert!(matches!(
            StoredTextureMap::parse(&b),
            Err(Error::Value {
                what: "first byte",
                ..
            })
        ));
        // the compiled header's width (after its marker and u32 1)
        let compiled = 60 + 9;
        let mut b = good.clone();
        b[compiled + 8] = 9;
        assert!(matches!(
            StoredTextureMap::parse(&b),
            Err(Error::Compiled { what: "width", .. })
        ));
        let mut b = good.clone();
        b.push(0);
        assert!(matches!(StoredTextureMap::parse(&b), Err(Error::Pixels { .. })));
        // pixels not filling the levels, and an unestablished format
        let t = StoredTextureMap::parse(&body(8, 4, 2, 3, &[77], &[1; 24])).expect("parse");
        assert!(t.levels().is_err());
        let t = StoredTextureMap::parse(&body(8, 4, 5, 3, &[77], &[1; 32])).expect("parse");
        assert!(t.levels().is_err());
    }
}
