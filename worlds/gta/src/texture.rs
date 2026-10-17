//! Texture dictionaries (ytd, and the ones drawables embed) in Enhanced's layout.
//!
//! A dictionary (64 bytes): the resource file header, a sorted list of name hashes and a parallel list of texture
//! pointers. A texture (128 bytes; the first 80 are the base a shader parameter may point at): block count and block
//! size, flags, width, height, depth, dimension, buffer format, tile mode, level count, name, data. The data is every
//! level of every slice, largest first, each level whole blocks of the format; its size must equal the block count
//! times the block size.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::le;
use crate::resource::Resource;
use crate::sga::BufferFormat;
use crate::{Result, invalid};

/// A texture as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub levels: u8,
    /// `rage::sga` image dimension (1: 2D, 2: 3D, 3: cube, as stored).
    pub dimension: u8,
    pub format: BufferFormat,
    pub tile_mode: u8,
    pub flags: u32,
    pub usage: u32,
    pub data: Vec<u8>,
}

fn err(at: u64, detail: impl ToString) -> crate::Error {
    invalid("texture", at, detail)
}

/// A NUL-terminated name at a resource pointer.
pub(crate) fn c_string(r: &Resource, pointer: u64) -> Result<String> {
    let mut s = Vec::new();
    let mut at = pointer;
    loop {
        let [b] = *r.block::<1>(at)?;
        if b == 0 {
            break;
        }
        s.push(b);
        if s.len() > 4096 {
            return Err(err(pointer, "name longer than 4096 bytes"));
        }
        at = at
            .checked_add(1)
            .ok_or_else(|| err(pointer, "a name past the address space"))?;
    }
    String::from_utf8(s).map_err(|e| err(pointer, e))
}

impl Texture {
    /// The 80-byte base at `pointer`: everything but the data.
    pub(crate) fn read_base(r: &Resource, pointer: u64) -> Result<(Texture, u64, u32, u32)> {
        let d = r.block::<0x50>(pointer)?;
        let name_ptr = le::u64(d, 0x28)?;
        let t = Texture {
            name: if name_ptr == 0 {
                String::new()
            } else {
                c_string(r, name_ptr)?
            },
            width: le::u16(d, 0x18)?,
            height: le::u16(d, 0x1A)?,
            depth: le::u16(d, 0x1C)?,
            dimension: d[0x1E],
            format: BufferFormat(d[0x1F]),
            tile_mode: d[0x20],
            levels: d[0x22],
            flags: le::u32(d, 0x10)?,
            usage: le::u32(d, 0x40)?,
            data: Vec::new(),
        };
        Ok((t, le::u64(d, 0x38)?, le::u32(d, 0x08)?, le::u32(d, 0x0C)?))
    }

    /// The texture at `pointer`, with its data.
    pub fn read(r: &Resource, pointer: u64) -> Result<Texture> {
        let (mut t, data_ptr, blocks, block_size) = Self::read_base(r, pointer)?;
        let (format, _) = t
            .format
            .texture()
            .ok_or_else(|| err(pointer, format!("{}: buffer format {:#x}", t.name, t.format.0)))?;
        if t.levels == 0 || t.width == 0 || t.height == 0 || t.depth == 0 {
            return Err(err(pointer, format!("{}: empty size or no levels", t.name)));
        }
        // levels of at most 2^16 texels a side, at most 255 of them: the sums and shifts are checked anyway
        let too_big = || err(pointer, format!("{}: a size past the address space", t.name));
        let mut size = 0usize;
        for level in 0..u32::from(t.levels) {
            let side = |s: u16| u32::from(s).checked_shr(level).unwrap_or(0).max(1);
            size = size
                .checked_add(format.level_size(side(t.width), side(t.height)))
                .ok_or_else(too_big)?;
        }
        let size = size.checked_mul(usize::from(t.depth)).ok_or_else(too_big)?;
        if u64::try_from(size).ok() != u64::from(blocks).checked_mul(u64::from(block_size)) {
            return Err(err(
                pointer,
                format!(
                    "{}: {size} bytes of levels, {blocks} blocks of {block_size}",
                    t.name
                ),
            ));
        }
        t.data = r.at(data_ptr, size)?.to_vec();
        Ok(t)
    }
}

/// A texture dictionary: textures sorted by name hash.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureDictionary {
    /// Name hashes as stored (the joaat of the lowercase name), parallel to `textures`.
    pub hashes: Vec<u32>,
    /// The textures; `None` where the list holds a null pointer (a name kept without its texture, as some patch
    /// drawables' dictionaries do).
    pub textures: Vec<Option<Texture>>,
}

/// The resource version of Enhanced's texture dictionaries (ytd).
pub const VERSION: u32 = 5;

impl TextureDictionary {
    /// A ytd's dictionary: its root, after checking the resource is in Enhanced's layout.
    pub fn read_resource(r: &Resource) -> Result<TextureDictionary> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ytd layout is {VERSION}",
                    r.version
                ),
            ));
        }
        Self::read(r, 0x5000_0000)
    }

    /// The dictionary at `pointer` (`0x5000_0000` for a ytd's root).
    pub fn read(r: &Resource, pointer: u64) -> Result<TextureDictionary> {
        let d = r.block::<0x40>(pointer)?;
        // a pointer list's header: pointer, count, capacity
        let list = |(at, count, cap): (usize, usize, usize)| -> Result<(u64, usize)> {
            let (count, cap) = (le::u16(d, count)?, le::u16(d, cap)?);
            if cap < count {
                return Err(err(pointer, format!("list count {count} over capacity {cap}")));
            }
            Ok((le::u64(d, at)?, usize::from(count)))
        };
        let (hash_ptr, n) = list((0x20, 0x28, 0x2A))?;
        let (tex_ptr, m) = list((0x30, 0x38, 0x3A))?;
        if n != m {
            return Err(err(pointer, format!("{n} name hashes for {m} textures")));
        }
        let hashes: Vec<u32> = if n == 0 {
            Vec::new()
        } else {
            let h = r.records(hash_ptr, n, 4)?;
            h.as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect()
        };
        if hashes.iter().zip(hashes.iter().skip(1)).any(|(a, b)| a >= b) {
            return Err(err(pointer, "name hashes not strictly ascending"));
        }
        let mut textures = Vec::with_capacity(n);
        if n > 0 {
            let p = r.records(tex_ptr, n, 8)?.as_chunks::<8>().0;
            for (&hash, q) in hashes.iter().zip(p) {
                let t = match u64::from_le_bytes(*q) {
                    0 => None,
                    q => Some(Texture::read(r, q)?),
                };
                if let Some(t) = &t
                    && primase_hash::joaat(t.name.as_bytes()) != hash
                {
                    return Err(err(pointer, format!("{}: name hash {hash:#010x} listed", t.name)));
                }
                textures.push(t);
            }
        }
        Ok(TextureDictionary { hashes, textures })
    }
}
