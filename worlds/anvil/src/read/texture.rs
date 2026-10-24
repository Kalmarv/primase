//! A TextureMap resource's body: its size, format code, mip count, references to its external mips, and the mip
//! levels it holds itself, read as stored. Pixels are not decoded here (`primase-texcodec` decodes a level once its
//! format is known); colour space and role are not read.
//!
//! Width, height, mip count, format code and the references are established by the fixtures: the levels they give
//! fill the pixels exactly, the referenced CompiledMips are exactly the missing levels' sizes, and the levels decode
//! (worlds/anvil/tests/texture.rs). The u32s at 9, 13 and 21 are read by the reference tool's names, depth, array size
//! and dimension (1 flat, 2 cube, 3 volume; another is refused): with `layers = depth x array size x (6 for a cube)`
//! each level holds its layers in turn, and the pixels are exactly the layers times the levels' sizes on every one of
//! Riders Republic's 25 cube and volume maps (the Riders Republic notes, docs/texture-formats.md, the 49 refused maps;
//! the order within a level's bytes the tool's, the lengths established). The u32 at 25 is kept as stored. A mip count
//! of 0 is one level (15 maps hold exactly one level's pixels so). A body that ends after a reference count of 0 and
//! one byte 3 (a null reference's tag, its nine zeros not stored) holds no image (two debug font maps, filled at run
//! time): it reads with no levels.
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
//! disagreeing with the first, or pixels not ending the body. [`StoredTextureMap::levels`] slices the levels for
//! the format codes whose storage the census establishes ([`storage`]), refusing another code or pixels the levels
//! don't fill; [`format`] names the codec only for the codes whose codec is established.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::fields::LE;
use primase_bytes::{Cursor, Field, FieldError};
use primase_core::TexFormat;

pub use crate::read::material::TEXTURE_MAP;

/// The compiled header's marker: equals the CRC-32 of `CompiledTextureMap` (the name a label).
pub const COMPILED_TEXTURE_MAP: u32 = 0x1323_7FE9;

/// The texture format a TextureMap's engine format code names, for the codes established by two third-party tools
/// agreeing (AnvilToolkit 1.3.7's Breakpoint table and RRAssetStudio v3's) with the census's storage, and by a real
/// sample decoding by primase-texcodec equal to the third-party bcdec's on every byte (worlds/anvil/tests/texture.rs):
/// 2 and 3 BC1, 4 BC2, 5 BC3, 6 BC4, 7 BC5, 10 BC7, 12 R8, 20 RGBA16F (BC7 also equal to texture2ddecoder's; BC2, R8
/// and RGBA16F also to RRAssetStudio's). 0 is B8G8R8A8, where the two tools disagree (BGRA8 or RGBA8), by what the
/// game's own data means: the engine's built-in default normal texture (DataPC entry 139325) stores `ff 80 80 ff`, a
/// flat normal only as B, G, R, A, and the skin-colour palette decodes to skin tones only so (the Riders Republic notes,
/// docs/material-spec.md §7). 11 is A8 (the tools disagree, A8 or R8) by the engine itself: its create-resource record
/// in a memory snapshot of the game gives DXGI format 65 (A8_UNORM) to the one TextureMap of its size and mip count,
/// a code-11 map (the notes, docs/texture-formats.md). 1 is R8G8B8A8_SNORM, 8 BC6H unsigned and 9 BC6H signed:
/// AnvilToolkit's table names them (RRAssetStudio's reader refuses them), and the content agrees, the rule for these
/// three Caleb's (content evidence counts): read as signed bytes the code-1 noise map is exactly Perlin's gradient
/// table (12 cube-edge directions, two components of +-127 and one 0 on all 256 pixels), and the code-8 and 9 maps'
/// 2,066,602 blocks are valid BC6H on every one and decode to coherent images (the notes, docs/texture-formats.md,
/// the 49 refused maps). Colour space and role are not read:
/// neither tool's table holds an sRGB format. Another code is `None`: not refused as stored, but its codec isn't
/// established.
pub fn format(code: u32) -> Option<TexFormat> {
    match code {
        0 => Some(TexFormat::Bgra8Unorm),
        1 => Some(TexFormat::Rgba8Snorm),
        2 | 3 => Some(TexFormat::Bc1),
        4 => Some(TexFormat::Bc2),
        5 => Some(TexFormat::Bc3),
        6 => Some(TexFormat::Bc4Unorm),
        7 => Some(TexFormat::Bc5Unorm),
        8 => Some(TexFormat::Bc6hUfloat),
        9 => Some(TexFormat::Bc6hSfloat),
        10 => Some(TexFormat::Bc7),
        11 => Some(TexFormat::A8Unorm),
        12 => Some(TexFormat::R8Unorm),
        20 => Some(TexFormat::Rgba16Float),
        _ => None,
    }
}

/// How a format code's levels are stored: bytes per 4x4 block, or bytes per pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Blocks(u32),
    Pixels(u32),
}

impl Storage {
    /// A `w` x `h` level's bytes (blocks round each axis up to 4).
    pub fn level_size(self, w: u32, h: u32) -> Option<usize> {
        let (units, bytes) = match self {
            Storage::Blocks(b) => (u64::from(w.div_ceil(4)).checked_mul(u64::from(h.div_ceil(4)))?, b),
            Storage::Pixels(b) => (u64::from(w).checked_mul(u64::from(h))?, b),
        };
        usize::try_from(units.checked_mul(u64::from(bytes))?).ok()
    }
}

/// How a format code's levels are stored, for the codes where every TextureMap of Riders Republic (build 25394987)
/// with that code fills its held levels exactly in this one class and in no other of the five measured (8 and 16 bytes
/// per block; 1, 4 and 8 bytes per pixel; worlds/anvil/tests/census.rs): 2, 3, 6 (8 per block); 4, 5, 7, 10 (16 per
/// block); 11, 12 (1 per pixel); 0, 15 (4 per pixel; 14 of code 0's 344 and 1 of code 15's 4 fill none, and are
/// refused); 20 (8 per pixel). This is the storage only: which codec a code is ([`format`]) the bytes don't say.
pub fn storage(code: u32) -> Option<Storage> {
    match code {
        2 | 3 | 6 => Some(Storage::Blocks(8)),
        4 | 5 | 7 | 8 | 9 | 10 => Some(Storage::Blocks(16)),
        11 | 12 => Some(Storage::Pixels(1)),
        0 | 1 | 15 => Some(Storage::Pixels(4)),
        20 => Some(Storage::Pixels(8)),
        _ => None,
    }
}

/// Why a TextureMap body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("TextureMap body: {0}")]
    Short(#[from] FieldError),
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
    /// The u32s at 9, 13 and 21: depth, array size and dimension (the reference tool's names).
    pub depth: u32,
    pub array_size: u32,
    pub dimension: u32,
    /// The u32 at 25, as stored (1 on map 217, 0 on 218 and 31; what it selects is not established).
    pub field_25: u32,
    pub mip_count: u32,
    /// The 14 bytes after the mip count.
    pub head_rest: [u8; 14],
    /// The CompiledMips holding the first levels, in level order.
    pub references: Vec<MipReference>,
    /// The 9 bytes after the references; `None` where the body ends after a reference count of 0 and one byte 3 (no
    /// image stored).
    pub references_end: Option<[u8; 9]>,
    /// The compiled header's 16 bytes before its pixel byte length (zeros where no image is stored).
    pub compiled_rest: [u8; 16],
    /// Levels `references.len()` onwards, as stored.
    pub pixels: Vec<u8>,
}

impl StoredTextureMap {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredTextureMap, Error> {
        let mut c = Cursor::new(body);
        let [flag] = c.array::<1>().field("first byte")?;
        if flag != 1 {
            return Err(Error::Value {
                what: "first byte",
                found: u64::from(flag),
                read: 1,
            });
        }
        let width = c.u32(LE).field("width")?;
        let height = c.u32(LE).field("height")?;
        let depth = c.u32(LE).field("u32 at 9")?;
        let array_size = c.u32(LE).field("u32 at 13")?;
        let format = c.u32(LE).field("format")?;
        let dimension = c.u32(LE).field("u32 at 21")?;
        if !(1..=3).contains(&dimension) {
            return Err(Error::Value {
                what: "u32 at 21 (dimension)",
                found: u64::from(dimension),
                read: 1,
            });
        }
        let field_25 = c.u32(LE).field("u32 at 25")?;
        let mip_count = c.u32(LE).field("mip count")?;
        let head_rest = c.array::<14>().field("bytes after the mip count")?;
        let count = c.u32(LE).field("reference count")?;
        let mut references = Vec::new();
        for _ in 0..count {
            let [tag] = c.array::<1>().field("reference tag")?;
            if tag != 0 {
                return Err(Error::Value {
                    what: "reference tag",
                    found: u64::from(tag),
                    read: 0,
                });
            }
            references.push(MipReference {
                id: u64::from_le_bytes(c.array::<8>().field("reference")?),
            });
        }
        if count == 0 && body.get(c.position()..) == Some(&[3][..]) {
            return Ok(StoredTextureMap {
                width,
                height,
                format,
                depth,
                array_size,
                dimension,
                field_25,
                mip_count,
                head_rest,
                references,
                references_end: None,
                compiled_rest: [0; 16],
                pixels: Vec::new(),
            });
        }
        let references_end = Some(c.array::<9>().field("bytes after the references")?);
        only(&mut c, "compiled header marker", COMPILED_TEXTURE_MAP)?;
        only(&mut c, "compiled header", 1)?;
        let again = [
            ("width", width),
            ("height", height),
            ("u32 at 9", depth),
            ("u32 at 13", array_size),
            ("mip count", mip_count),
            ("format", format),
            ("u32 at 21", dimension),
            ("u32 at 25", field_25),
        ];
        for (what, stored) in again {
            let found = c.u32(LE).field(what)?;
            if found != stored {
                return Err(Error::Compiled { what, found, stored });
            }
        }
        let compiled_rest = c.array::<16>().field("compiled header")?;
        let length = c.u32(LE).field("pixel byte length")?;
        let pixels = body.get(c.position()..).unwrap_or_default();
        if usize::try_from(length).ok() != Some(pixels.len()) {
            return Err(Error::Pixels {
                why: format!("{length} bytes stated, {} ending the body", pixels.len()),
            });
        }
        Ok(StoredTextureMap {
            width,
            height,
            format,
            depth,
            array_size,
            dimension,
            field_25,
            mip_count,
            head_rest,
            references,
            references_end,
            compiled_rest,
            pixels: pixels.to_vec(),
        })
    }

    /// The layers each level holds: depth x array size, times 6 for a cube.
    pub fn layers(&self) -> u32 {
        let cube = if self.dimension == 2 { 6 } else { 1 };
        self.depth.saturating_mul(self.array_size).saturating_mul(cube)
    }

    /// Whether the body stores an image.
    pub fn has_image(&self) -> bool {
        self.references_end.is_some()
    }

    /// Level `i`'s width and height (each halved per level, at least 1).
    pub fn level_size(&self, i: u32) -> (u32, u32) {
        let half = |x: u32| x.checked_shr(i).unwrap_or(0).max(1);
        (half(self.width), half(self.height))
    }

    /// The levels this body holds, each its bytes: from `references.len()` to the mip count; or, where those don't fill
    /// the pixels and the whole chain to 1 x 1 does exactly, that chain (on two maps of Riders Republic the stored mip
    /// count, 2, is less than the ten levels the pixels hold; RRAssetStudio reads them the same way). Refused unless
    /// the format code's storage is established ([`storage`]) and one of the two fills the pixels exactly.
    pub fn levels(&self) -> Result<Vec<&[u8]>, Error> {
        if !self.has_image() {
            return Ok(Vec::new());
        }
        let f = storage(self.format).ok_or_else(|| Error::Pixels {
            why: format!("format code {}: its storage isn't established", self.format),
        })?;
        let first = u32::try_from(self.references.len()).unwrap_or(u32::MAX);
        let chain = self.width.max(self.height).max(1).ilog2().saturating_add(1);
        for last in [self.mip_count.max(1), chain] {
            if let Some(levels) = self.slice(f, first, last) {
                return Ok(levels);
            }
        }
        Err(Error::Pixels {
            why: format!(
                "neither levels {first} to {} nor the chain to 1 x 1 ({chain} levels) fill the {} pixel bytes",
                self.mip_count,
                self.pixels.len()
            ),
        })
    }

    /// Levels `first` to `last` (exclusive), if they fill the pixels exactly.
    fn slice(&self, f: Storage, first: u32, last: u32) -> Option<Vec<&[u8]>> {
        let mut out = Vec::new();
        let mut at = 0usize;
        let layers = usize::try_from(self.layers()).ok()?;
        for i in first..last {
            let (w, h) = self.level_size(i);
            let end = at.checked_add(f.level_size(w, h)?.checked_mul(layers)?)?;
            out.push(self.pixels.get(at..end)?);
            at = end;
        }
        (at == self.pixels.len()).then_some(out)
    }
}

/// A u32 that must be `read`.
fn only(c: &mut Cursor, what: &'static str, read: u32) -> Result<(), Error> {
    let found = c.u32(LE).field(what)?;
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
    fn a_stated_count_short_of_the_chain_held_reads_the_chain() {
        // 8 x 4 BC1 stating 2 levels (1 external) but holding levels 1 to 3 (4 x 2, 2 x 1, 1 x 1)
        let t = StoredTextureMap::parse(&body(8, 4, 2, 2, &[77], &[1; 24])).expect("parse");
        assert_eq!(t.levels().expect("levels").len(), 3);
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
        let t = StoredTextureMap::parse(&body(8, 4, 2, 3, &[77], &[1; 20])).expect("parse");
        assert!(t.levels().is_err());
        let t = StoredTextureMap::parse(&body(8, 4, 13, 3, &[77], &[1; 32])).expect("parse");
        assert!(t.levels().is_err());
    }
}
