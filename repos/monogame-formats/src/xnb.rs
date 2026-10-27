//! The XNB container, version 5 (XNA 4.0; MonoGame writes the same), uncompressed.
//!
//! Layout: `XNB`, the target platform (one byte, as stored: `w` Windows, `d` DesktopGL, ...), the version (5), flags
//! (0x01 HiDef profile, 0x40 LZ4-compressed, 0x80 LZX-compressed), the file size (u32, little-endian, the whole
//! file); then the type-reader table (a 7-bit encoded count, each reader a 7-bit length-prefixed UTF-8 name, as
//! stored and assembly-qualified, and an i32 version), the shared-resource count (7-bit), and the primary object: a
//! 7-bit type id (0 null, else 1 + its reader's index) and the bytes its reader wrote. A compressed file is refused,
//! naming its flag ([`Error::Compressed`]); so is any other version, a flag this reader does not know, and a size
//! other than the file's.
//!
//! The built-in readers' values: [`Content::string`] (`StringReader`: a 7-bit byte length, then UTF-8) and
//! [`Content::byte_array`] (`ArrayReader<Byte>`: an i32 count, then the bytes). Which reader wrote the object is the
//! caller's to check by [`TypeReader::type_name`]; [`Xnb::string`] and [`Xnb::byte_array`] do it for the built-in
//! two. [`Xnb::texture2d`] reads XNA's `Texture2DReader` (a raw 8-bit `Color` surface; any other surface format is
//! refused, naming it). Shared resources are not read: [`Content::end`] refuses a file that has any.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_foundations::bytes::{BoundsError, Cursor, Endian};

/// The compression a compressed XNB's flags name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// Flag 0x80.
    Lzx,
    /// Flag 0x40.
    Lz4,
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Compression::Lzx => "LZX (flag 0x80)",
            Compression::Lz4 => "LZ4 (flag 0x40)",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The bytes break the format, at this absolute offset.
    #[error("XNB at byte {offset}: {detail}")]
    Format { offset: u128, detail: String },
    /// The bytes of an effect (`mgfx`) break its format, at this offset in the effect's own bytes.
    #[error("MGFX effect at byte {offset} of its bytes: {detail}")]
    Effect { offset: u128, detail: String },
    /// A compressed XNB: only uncompressed content is read.
    #[error("XNB compressed with {0}: only uncompressed content is read")]
    Compressed(Compression),
}

pub type Result<T> = std::result::Result<T, Error>;

fn bad(offset: usize, detail: impl Into<String>) -> Error {
    Error::Format {
        offset: offset as u128,
        detail: detail.into(),
    }
}

impl From<BoundsError> for Error {
    fn from(e: BoundsError) -> Error {
        Error::Format {
            offset: e.absolute(),
            detail: e.to_string(),
        }
    }
}

/// The version read.
pub const VERSION: u8 = 5;
const HI_DEF: u8 = 0x01;
const LZ4: u8 = 0x40;
const LZX: u8 = 0x80;
/// The fixed header: magic, platform, version, flags, file size.
const HEADER: usize = 10;

/// XNA's texture reader, as [`TypeReader::type_name`] gives it.
pub const TEXTURE_2D_READER: &str = "Microsoft.Xna.Framework.Content.Texture2DReader";
/// MonoGame's effect reader, as [`TypeReader::type_name`] gives it.
pub const EFFECT_READER: &str = "Microsoft.Xna.Framework.Content.EffectReader";
/// XNA's string reader, as [`TypeReader::type_name`] gives it.
const STRING_READER: &str = "Microsoft.Xna.Framework.Content.StringReader";
/// XNA's array reader, as [`TypeReader::base_name`] gives it; its one generic argument is the element type.
pub const ARRAY_READER: &str = "Microsoft.Xna.Framework.Content.ArrayReader`1";
/// The element type of a byte array, as [`TypeReader::generic_arguments`] gives it.
pub const BYTE: &str = "System.Byte";

/// One entry of the type-reader table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeReader {
    /// The reader's type name as stored, assembly-qualified
    /// (`BytingPipeline.GLTFReader, BytingPipeline`, `Microsoft.Xna.Framework.Content.ArrayReader`1[[System.Byte,
    /// System.Private.CoreLib, ...]]`).
    pub name: String,
    pub version: i32,
}

/// `s` up to its first comma outside brackets: a .NET type name without its assembly qualifier.
fn unqualified(s: &str) -> &str {
    let mut depth = 0u32;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth = depth.saturating_add(1),
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return s.get(..i).unwrap_or(s).trim(),
            _ => {}
        }
    }
    s.trim()
}

impl TypeReader {
    /// The name without its assembly qualifier: `BytingPipeline.GLTFReader`; a generic keeps its arguments.
    pub fn type_name(&self) -> &str {
        unqualified(&self.name)
    }

    /// The type name without generic arguments: `Microsoft.Xna.Framework.Content.ArrayReader`1`.
    pub fn base_name(&self) -> &str {
        let t = self.type_name();
        t.find('[').and_then(|i| t.get(..i)).unwrap_or(t)
    }

    /// A generic reader's type arguments, each without its assembly qualifier (`System.Byte`); none for another.
    pub fn generic_arguments(&self) -> Vec<&str> {
        let t = self.type_name();
        // `Base`n[[A, asm],[B, asm]]`: the arguments are the bracketed groups inside the outer brackets
        let Some(inner) = t
            .find('[')
            .and_then(|i| t.get(i..))
            .and_then(|s| s.strip_prefix('['))
            .and_then(|s| s.strip_suffix(']'))
        else {
            return Vec::new();
        };
        let mut args = Vec::new();
        let (mut depth, mut start) = (0u32, None);
        for (i, c) in inner.char_indices() {
            match c {
                '[' => {
                    if depth == 0 {
                        start = i.checked_add(1);
                    }
                    depth = depth.saturating_add(1);
                }
                ']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some(arg) = start.take().and_then(|s| inner.get(s..i))
                    {
                        args.push(unqualified(arg));
                    }
                }
                _ => {}
            }
        }
        args
    }

    /// XNA's string reader.
    pub fn is_string(&self) -> bool {
        self.type_name() == STRING_READER
    }

    /// XNA's array reader of bytes.
    pub fn is_byte_array(&self) -> bool {
        self.base_name() == ARRAY_READER && self.generic_arguments() == [BYTE]
    }

    /// MonoGame's effect reader.
    pub fn is_effect(&self) -> bool {
        self.type_name() == EFFECT_READER
    }

    /// XNA's texture reader.
    pub fn is_texture_2d(&self) -> bool {
        self.type_name() == TEXTURE_2D_READER
    }
}

/// A `Texture2D` as XNA's reader stores it: raw 8-bit `Color` pixels (R, G, B, A bytes, rows top to bottom), each mip
/// level a slice of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture2D<'a> {
    pub width: u32,
    pub height: u32,
    /// The levels, largest first; level `i` is `max(1, width >> i)` by `max(1, height >> i)` pixels of 4 bytes.
    pub levels: Vec<&'a [u8]>,
}

/// The `SurfaceFormat` `Color` of XNA and MonoGame: 8 bits per channel, R, G, B, A.
const SURFACE_FORMAT_COLOR: i32 = 0;

/// A parsed XNB: its header, its type-reader table, and the bytes after them (the primary object).
#[derive(Clone, Debug)]
pub struct Xnb<'a> {
    /// The target platform byte as stored (`b'w'` Windows, `b'd'` DesktopGL, ...).
    pub platform: u8,
    /// Always [`VERSION`].
    pub version: u8,
    /// Built for the HiDef profile (flag 0x01).
    pub hi_def: bool,
    pub readers: Vec<TypeReader>,
    pub shared_resources: u32,
    /// The primary object, from its type id to the end of the file.
    body: &'a [u8],
    /// The body's offset in the file.
    origin: usize,
}

impl<'a> Xnb<'a> {
    /// Parse an XNB's header and type-reader table. See the module documentation.
    pub fn parse(bytes: &'a [u8]) -> Result<Xnb<'a>> {
        let mut c = Content {
            cursor: Cursor::new(bytes),
            origin: 0,
            shared_resources: 0,
        };
        let magic = c.bytes(3)?;
        if magic != b"XNB" {
            return Err(bad(0, format!("magic {magic:02x?}, not \"XNB\"")));
        }
        let platform = c.u8()?;
        let version = c.u8()?;
        if version != VERSION {
            return Err(bad(
                4,
                format!("version {version}: only {VERSION} (XNA 4.0, MonoGame) is read"),
            ));
        }
        let flags = c.u8()?;
        if flags & LZX != 0 {
            return Err(Error::Compressed(Compression::Lzx));
        }
        if flags & LZ4 != 0 {
            return Err(Error::Compressed(Compression::Lz4));
        }
        if flags & !HI_DEF != 0 {
            return Err(bad(5, format!("flags {flags:#04x}: unknown bits")));
        }
        let size = c.u32()?;
        if usize::try_from(size).ok() != Some(bytes.len()) {
            return Err(bad(
                6,
                format!("file size {size}, the file is {} bytes", bytes.len()),
            ));
        }
        debug_assert_eq!(c.position(), HEADER);
        let count = c.seven_bit()?;
        let mut readers = Vec::new();
        for _ in 0..count {
            let name = c.string()?.to_owned();
            let version = c.i32()?;
            readers.push(TypeReader { name, version });
        }
        let shared_resources = c.seven_bit()?;
        let origin = c.position();
        let body = bytes
            .get(origin..)
            .ok_or_else(|| bad(origin, "the body starts past the end"))?;
        Ok(Xnb {
            platform,
            version,
            hi_def: flags & HI_DEF != 0,
            readers,
            shared_resources,
            body,
            origin,
        })
    }

    /// The primary object: its reader and its bytes, after its type id. A null object is refused.
    pub fn primary(&self) -> Result<(&TypeReader, Content<'a>)> {
        let mut c = Content {
            cursor: Cursor::with_origin(self.body, self.origin),
            origin: self.origin,
            shared_resources: self.shared_resources,
        };
        let at = c.position();
        let id = c.seven_bit()?;
        let index = id
            .checked_sub(1)
            .ok_or_else(|| bad(at, "the primary object is null"))?;
        let reader = usize::try_from(index)
            .ok()
            .and_then(|i| self.readers.get(i))
            .ok_or_else(|| bad(at, format!("type id {id}, {} type readers", self.readers.len())))?;
        Ok((reader, c))
    }

    /// The primary object as XNA's string reader wrote it; the whole file read.
    pub fn string(&self) -> Result<&'a str> {
        let (reader, mut c) = self.primary()?;
        if !reader.is_string() {
            return Err(bad(
                self.origin,
                format!("the reader is {}, not a string's", reader.name),
            ));
        }
        let s = c.string()?;
        c.end()?;
        Ok(s)
    }

    /// The primary object as XNA's `Texture2DReader` wrote it (the surface format, the size, the level count, then each
    /// level's byte count and bytes); the whole file read. Only the `Color` surface format (0) is read: another is
    /// refused naming its number, as is a level whose byte count is not its pixels', zero or too many levels and a
    /// zero size.
    pub fn texture2d(&self) -> Result<Texture2D<'a>> {
        let (reader, mut c) = self.primary()?;
        if !reader.is_texture_2d() {
            return Err(bad(
                self.origin,
                format!("the reader is {}, not a texture's", reader.name),
            ));
        }
        let at = c.position();
        let format = c.i32()?;
        if format != SURFACE_FORMAT_COLOR {
            return Err(bad(at, format!("SurfaceFormat {format}: only Color (0) is read")));
        }
        let (width, height, levels) = (c.i32()?, c.i32()?, c.i32()?);
        let dimension = |v: i32, what: &str| match u32::try_from(v) {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(bad(at, format!("{what} {v}"))),
        };
        let (width, height) = (dimension(width, "width")?, dimension(height, "height")?);
        let most = width.max(height).ilog2().saturating_add(1);
        let levels = match u32::try_from(levels) {
            Ok(n) if (1..=most).contains(&n) => n,
            _ => {
                return Err(bad(
                    at,
                    format!("{levels} levels for {width} x {height} (1 to {most})"),
                ));
            }
        };
        let mut out = Vec::new();
        for level in 0..levels {
            let at = c.position();
            let count = c.i32()?;
            let pixels = u64::from(width.checked_shr(level).unwrap_or(0).max(1))
                .saturating_mul(u64::from(height.checked_shr(level).unwrap_or(0).max(1)));
            let expected = pixels.saturating_mul(4);
            if u64::try_from(count).ok() != Some(expected) {
                return Err(bad(
                    at,
                    format!("level {level}: {count} bytes, not {expected} (Color pixels)"),
                ));
            }
            out.push(c.bytes(usize::try_from(expected).map_err(|_| bad(at, "level size"))?)?);
        }
        c.end()?;
        Ok(Texture2D {
            width,
            height,
            levels: out,
        })
    }

    /// The primary object as MonoGame's `EffectReader` wrote it, an i32 count and the bytes of the compiled effect
    /// ([`crate::mgfx::Effect::parse`] reads them); the whole file read.
    pub fn effect(&self) -> Result<&'a [u8]> {
        let (reader, mut c) = self.primary()?;
        if !reader.is_effect() {
            return Err(bad(
                self.origin,
                format!("the reader is {}, not an effect's", reader.name),
            ));
        }
        let b = c.byte_array()?;
        c.end()?;
        Ok(b)
    }

    /// The primary object as XNA's array reader of bytes wrote it; the whole file read.
    pub fn byte_array(&self) -> Result<&'a [u8]> {
        let (reader, mut c) = self.primary()?;
        if !reader.is_byte_array() {
            return Err(bad(
                self.origin,
                format!("the reader is {}, not a byte array's", reader.name),
            ));
        }
        let b = c.byte_array()?;
        c.end()?;
        Ok(b)
    }
}

/// A content reader's bytes, read in order with checked bounds; offsets in errors are the file's.
#[derive(Clone, Debug)]
pub struct Content<'a> {
    cursor: Cursor<'a>,
    origin: usize,
    shared_resources: u32,
}

impl<'a> Content<'a> {
    /// A reader of `bytes` that are not a file (an effect inside an XNB): offsets in its errors are the bytes'.
    pub fn of_bytes(bytes: &'a [u8]) -> Content<'a> {
        Content {
            cursor: Cursor::new(bytes),
            origin: 0,
            shared_resources: 0,
        }
    }

    /// How many bytes are left.
    pub fn remaining(&self) -> usize {
        self.cursor.remaining()
    }

    /// The absolute offset of the next byte.
    pub fn position(&self) -> usize {
        self.origin.saturating_add(self.cursor.position())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        Ok(self.cursor.take(n)?)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.cursor.array()?)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.cursor.u8()?)
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.cursor.i16(Endian::Little)?)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(self.cursor.u16(Endian::Little)?)
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.cursor.i32(Endian::Little)?)
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(self.cursor.u32(Endian::Little)?)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(self.cursor.f32(Endian::Little)?)
    }

    /// A 7-bit encoded unsigned integer (.NET `Read7BitEncodedInt`): low groups first, at most five bytes, the
    /// fifth holding the top four bits.
    pub fn seven_bit(&mut self) -> Result<u32> {
        let at = self.position();
        let mut value = 0u32;
        for shift in [0u32, 7, 14, 21, 28] {
            let b = self.u8()?;
            if shift == 28 && b > 0x0F {
                return Err(bad(at, "a 7-bit encoded integer over 32 bits"));
            }
            let bits = u32::from(b & 0x7F)
                .checked_shl(shift)
                .ok_or_else(|| bad(at, "a 7-bit encoded integer over 32 bits"))?;
            value |= bits;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        // the fifth byte, at most 0x0F, has no continuation bit
        Err(bad(at, "a 7-bit encoded integer over 32 bits"))
    }

    /// A string as .NET's `BinaryWriter` writes it (`StringReader`): a 7-bit byte length, then UTF-8.
    pub fn string(&mut self) -> Result<&'a str> {
        let at = self.position();
        let len = self.seven_bit()?;
        let len = usize::try_from(len).map_err(|_| bad(at, format!("string length {len}")))?;
        let bytes = self.bytes(len)?;
        std::str::from_utf8(bytes).map_err(|e| bad(at, format!("a string that is not UTF-8: {e}")))
    }

    /// A byte array as XNA's `ArrayReader<Byte>` writes it: an i32 count, then the bytes.
    pub fn byte_array(&mut self) -> Result<&'a [u8]> {
        let at = self.position();
        let count = self.i32()?;
        let count = usize::try_from(count).map_err(|_| bad(at, format!("byte array count {count}")))?;
        self.bytes(count)
    }

    /// The object read to its end: no byte left, and no shared resource (they are not read).
    pub fn end(self) -> Result<()> {
        if self.shared_resources != 0 {
            return Err(bad(
                self.position(),
                format!("{} shared resources: not read", self.shared_resources),
            ));
        }
        match self.cursor.remaining() {
            0 => Ok(()),
            n => Err(bad(self.position(), format!("{n} bytes after the object"))),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;

    const GLTF_READER: &str = "BytingPipeline.GLTFReader, BytingPipeline";
    const BYTES_READER: &str = "Microsoft.Xna.Framework.Content.ArrayReader`1[[System.Byte, System.Private.CoreLib, \
                                Version=8.0.0.0, Culture=neutral, PublicKeyToken=7cec85d7bea7798e]]";

    fn seven(mut v: u32, out: &mut Vec<u8>) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn string(s: &str, out: &mut Vec<u8>) {
        seven(s.len() as u32, out);
        out.extend_from_slice(s.as_bytes());
    }

    /// An uncompressed DesktopGL XNB with these readers, shared-resource count and body (type id and object).
    fn xnb(flags: u8, readers: &[&str], shared: u32, body: &[u8]) -> Vec<u8> {
        let mut f = b"XNBd".to_vec();
        f.extend_from_slice(&[VERSION, flags, 0, 0, 0, 0]);
        seven(readers.len() as u32, &mut f);
        for r in readers {
            string(r, &mut f);
            f.extend_from_slice(&0i32.to_le_bytes());
        }
        seven(shared, &mut f);
        f.extend_from_slice(body);
        let size = f.len() as u32;
        f[6..10].copy_from_slice(&size.to_le_bytes());
        f
    }

    fn string_body(id: u32, s: &str) -> Vec<u8> {
        let mut b = Vec::new();
        seven(id, &mut b);
        string(s, &mut b);
        b
    }

    #[test]
    fn a_string_reads_with_its_reader_as_stored() {
        // longer than 127 bytes: a two-byte length
        let json = format!(
            "{{\"asset\":{{\"version\":\"2.0\"}},\"pad\":\"{}\"}}",
            "x".repeat(200)
        );
        let f = xnb(HI_DEF, &[GLTF_READER], 0, &string_body(1, &json));
        let x = Xnb::parse(&f).unwrap();
        assert_eq!((x.platform, x.version, x.hi_def), (b'd', 5, true));
        assert_eq!(
            x.readers,
            [TypeReader {
                name: GLTF_READER.into(),
                version: 0
            }]
        );
        let (reader, mut c) = x.primary().unwrap();
        assert_eq!(reader.type_name(), "BytingPipeline.GLTFReader");
        assert!(reader.generic_arguments().is_empty() && !reader.is_string());
        assert_eq!(c.string().unwrap(), json);
        c.end().unwrap();
        // the game's reader is not the built-in one
        assert!(
            Xnb::parse(&f)
                .unwrap()
                .string()
                .unwrap_err()
                .to_string()
                .contains("not a string's")
        );
        let f = xnb(
            0,
            &["Microsoft.Xna.Framework.Content.StringReader"],
            0,
            &string_body(1, "{}"),
        );
        assert_eq!(Xnb::parse(&f).unwrap().string().unwrap(), "{}");
    }

    #[test]
    fn a_byte_array_reads_and_its_reader_is_recognized() {
        // type id 2: the second reader
        let mut body = vec![2];
        body.extend_from_slice(&4i32.to_le_bytes());
        body.extend_from_slice(b"qoif");
        let f = xnb(0, &[STRING_READER, BYTES_READER], 0, &body);
        let x = Xnb::parse(&f).unwrap();
        let r = &x.readers[1];
        assert_eq!(r.base_name(), ARRAY_READER);
        assert_eq!(r.generic_arguments(), [BYTE]);
        assert!(r.is_byte_array());
        assert_eq!(x.byte_array().unwrap(), b"qoif");
        // a negative count
        let mut neg = vec![2];
        neg.extend_from_slice(&(-1i32).to_le_bytes());
        let f = xnb(0, &[STRING_READER, BYTES_READER], 0, &neg);
        assert!(
            Xnb::parse(&f)
                .unwrap()
                .byte_array()
                .unwrap_err()
                .to_string()
                .contains("count -1")
        );
    }

    /// A 2 x 2 `Color` texture with a second level (1 x 1) reads as stored; another surface format, a wrong byte count,
    /// a missing level and a non-texture reader are refused.
    #[test]
    fn a_texture_reads_as_stored_and_is_strict() {
        const TEXTURE: &str =
            "Microsoft.Xna.Framework.Content.Texture2DReader, Microsoft.Xna.Framework.Graphics";
        let body = |format: i32, levels: i32, sizes: &[i32]| {
            let mut b = vec![1];
            for v in [format, 2, 2, levels] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for (i, n) in sizes.iter().enumerate() {
                b.extend_from_slice(&n.to_le_bytes());
                b.extend((0..*n).map(|k| (k as u8).wrapping_add(i as u8 * 100)));
            }
            b
        };
        let file = |b: &[u8]| xnb(0, &[TEXTURE], 0, b);
        let f = file(&body(0, 2, &[16, 4]));
        let x = Xnb::parse(&f).unwrap();
        assert!(x.readers[0].is_texture_2d());
        let t = x.texture2d().unwrap();
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(t.levels.len(), 2);
        assert_eq!(t.levels[0], (0..16).collect::<Vec<u8>>());
        assert_eq!(t.levels[1], [100, 101, 102, 103]);
        let err = |b: &[u8]| Xnb::parse(&file(b)).unwrap().texture2d().unwrap_err().to_string();
        assert!(err(&body(4, 1, &[16])).contains("SurfaceFormat 4"), "DXT1");
        assert!(err(&body(0, 1, &[12])).contains("12 bytes, not 16"));
        assert!(err(&body(0, 2, &[16])).contains("short") || err(&body(0, 2, &[16])).contains("XNB at byte"));
        assert!(err(&body(0, 3, &[16, 4, 4])).contains("3 levels for 2 x 2"));
        let mut trailing = body(0, 1, &[16]);
        trailing.push(0);
        assert!(!err(&trailing).is_empty());
        let other = xnb(0, &[STRING_READER], 0, &string_body(1, ""));
        assert!(
            Xnb::parse(&other)
                .unwrap()
                .texture2d()
                .unwrap_err()
                .to_string()
                .contains("not a texture's")
        );
    }

    #[test]
    fn compressed_files_are_refused_naming_the_flag() {
        for (flag, name) in [(LZX, "LZX (flag 0x80)"), (LZ4, "LZ4 (flag 0x40)")] {
            let f = xnb(flag | HI_DEF, &[STRING_READER], 0, &string_body(1, ""));
            let e = Xnb::parse(&f).unwrap_err();
            assert!(e.to_string().contains(name), "{e}");
        }
    }

    #[test]
    fn the_header_and_the_object_are_strict() {
        let good = xnb(0, &[STRING_READER], 0, &string_body(1, "a"));
        let err = |f: &[u8]| match Xnb::parse(f).and_then(|x| x.string().map(|_| ())) {
            Ok(()) => panic!("read"),
            Err(e) => e.to_string(),
        };
        let mut bad = good.clone();
        bad[0] = b'Y';
        assert!(err(&bad).contains("magic"));
        let mut bad = good.clone();
        bad[4] = 4;
        assert!(err(&bad).contains("version 4"));
        let mut bad = good.clone();
        bad[5] = 0x02;
        assert!(err(&bad).contains("unknown bits"));
        let mut bad = good.clone();
        bad.push(0);
        assert!(err(&bad).contains("file size"));
        // a null primary object, a type id past the table
        assert!(err(&xnb(0, &[STRING_READER], 0, &[0])).contains("null"));
        assert!(
            err(&xnb(0, &[STRING_READER], 0, &string_body(2, "a"))).contains("type id 2, 1 type readers")
        );
        // bytes after the object, shared resources
        assert!(
            err(&xnb(
                0,
                &[STRING_READER],
                0,
                &[string_body(1, "a"), vec![0]].concat()
            ))
            .contains("1 bytes after")
        );
        assert!(err(&xnb(0, &[STRING_READER], 1, &string_body(1, "a"))).contains("1 shared resources"));
        // a string running past the end, a string that is not UTF-8
        let mut short = vec![1];
        seven(5, &mut short);
        short.push(b'a');
        assert!(err(&xnb(0, &[STRING_READER], 0, &short)).contains("outside"));
        assert!(err(&xnb(0, &[STRING_READER], 0, &[1, 1, 0xFF])).contains("not UTF-8"));
        // a 7-bit integer over 32 bits (the fifth byte holds four)
        assert!(
            err(&xnb(0, &[STRING_READER], 0, &[1, 0x80, 0x80, 0x80, 0x80, 0x10])).contains("over 32 bits")
        );
        assert!(Xnb::parse(&good[..8]).is_err());
    }

    #[test]
    fn type_names_lose_their_assembly_qualifiers_only() {
        let r = |name: &str| TypeReader {
            name: name.into(),
            version: 0,
        };
        let dict = r(
            "Microsoft.Xna.Framework.Content.DictionaryReader`2[[System.String, mscorlib],[System.Int32, \
                      mscorlib]], MonoGame.Framework",
        );
        assert_eq!(
            dict.type_name(),
            "Microsoft.Xna.Framework.Content.DictionaryReader`2[[System.String, mscorlib],[System.Int32, mscorlib]]"
        );
        assert_eq!(
            dict.base_name(),
            "Microsoft.Xna.Framework.Content.DictionaryReader`2"
        );
        assert_eq!(dict.generic_arguments(), ["System.String", "System.Int32"]);
        assert!(
            !r("Microsoft.Xna.Framework.Content.ArrayReader`1[[System.Int32, mscorlib]]").is_byte_array()
        );
        assert!(r(STRING_READER).is_string());
    }
}
