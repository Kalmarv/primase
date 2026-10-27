//! Texture resources (type [`crate::TEXTURE`]): the 180-byte header skate. stores for a texture, whose pixels lie in
//! the chunk it names. Field layout after ReSkate's `Engine/Resource/texture.cpp` (`read_texture_header`, a port of
//! ReSkate Studio's texture resource), which reads the extended 180-byte form when the resource metadata's version is
//! 12 or 13:
//!
//! ```text
//! 0x00  8  unknown_00     not read by ReSkate
//! 0x08  u32 kind          0 a 2D texture, 1 a cube (depth and slices 6), 2 a volume (depth and slices its depth),
//!                         3 an array (depth and slices its layers)
//! 0x0c  u32 format        Frostbite format ordinal ([`block`])
//! 0x10  u32 unknown_10    skipped by ReSkate (metadata version >= 12)
//! 0x14  u16 flags, u16 width, u16 height, u16 depth, u16 slices
//! 0x1e  u8 mip count, u8 first mip
//! 0x20  8  unknown_20     skipped by ReSkate (extended form)
//! 0x28  16 chunk GUID     the chunk holding the pixels
//! 0x38  15 u32 mip sizes  bytes of each level of one slice, largest first
//! 0x74  u32 chunk size
//! 0x78  60 unknown_78     not read by ReSkate (a texture group name at 0x80 in the base game's textures)
//! ```
//!
//! The fields ReSkate does not read are kept as stored, so a header read and written again is byte-identical; what
//! they mean is not established. Refused: another length, a width or height of 0 or over 16384, a mip count of 0 or
//! over 15.

use crate::{Rd, Result, bad};

const W: &str = "texture";
/// The extended header's length.
pub const HEADER_LEN: usize = 180;
/// The largest side ReSkate accepts.
pub const MAX_SIDE: u16 = 16384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureHeader {
    pub unknown_00: [u8; 8],
    /// 0 a 2D texture.
    pub kind: u32,
    /// Frostbite format ordinal.
    pub format: u32,
    pub unknown_10: u32,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub slices: u16,
    pub mip_count: u8,
    pub first_mip: u8,
    pub unknown_20: [u8; 8],
    pub chunk: [u8; 16],
    /// Bytes of each level of one slice, largest first; the levels past `mip_count` are 0 in the game's textures.
    pub mip_sizes: [u32; 15],
    pub chunk_size: u32,
    pub unknown_78: [u8; 60],
}

/// Read a texture header. Refused: see the module documentation.
pub fn read(bytes: &[u8]) -> Result<TextureHeader> {
    if bytes.len() != HEADER_LEN {
        return Err(bad(W, 0, format!("{} bytes, not {HEADER_LEN}", bytes.len())));
    }
    let r = Rd::new(bytes, W);
    let mut mip_sizes = [0u32; 15];
    for (i, s) in mip_sizes.iter_mut().enumerate() {
        *s = r.u32le(0x38 + 4 * i)?;
    }
    let h = TextureHeader {
        unknown_00: r.arr(0)?,
        kind: r.u32le(0x08)?,
        format: r.u32le(0x0c)?,
        unknown_10: r.u32le(0x10)?,
        flags: r.u16le(0x14)?,
        width: r.u16le(0x16)?,
        height: r.u16le(0x18)?,
        depth: r.u16le(0x1a)?,
        slices: r.u16le(0x1c)?,
        mip_count: r.u8(0x1e)?,
        first_mip: r.u8(0x1f)?,
        unknown_20: r.arr(0x20)?,
        chunk: r.arr(0x28)?,
        mip_sizes,
        chunk_size: r.u32le(0x74)?,
        unknown_78: r.arr(0x78)?,
    };
    check(&h)?;
    Ok(h)
}

fn check(h: &TextureHeader) -> Result<()> {
    if !(1..=MAX_SIDE).contains(&h.width) || !(1..=MAX_SIDE).contains(&h.height) {
        return Err(bad(W, 0x16, format!("dimensions {}x{}", h.width, h.height)));
    }
    if !(1..=15).contains(&h.mip_count) {
        return Err(bad(W, 0x1e, format!("mip count {}", h.mip_count)));
    }
    Ok(())
}

/// Write a texture header: the reader's counterpart. Refused as [`read`] refuses.
pub fn write(h: &TextureHeader) -> Result<Vec<u8>> {
    check(h)?;
    let mut o = Vec::with_capacity(HEADER_LEN);
    o.extend_from_slice(&h.unknown_00);
    for w in [h.kind, h.format, h.unknown_10] {
        o.extend_from_slice(&w.to_le_bytes());
    }
    for v in [h.flags, h.width, h.height, h.depth, h.slices] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    o.extend_from_slice(&[h.mip_count, h.first_mip]);
    o.extend_from_slice(&h.unknown_20);
    o.extend_from_slice(&h.chunk);
    for s in h.mip_sizes {
        o.extend_from_slice(&s.to_le_bytes());
    }
    o.extend_from_slice(&h.chunk_size.to_le_bytes());
    o.extend_from_slice(&h.unknown_78);
    debug_assert_eq!(o.len(), HEADER_LEN);
    Ok(o)
}

/// How a format ordinal stores texels (ReSkate's colour-format table): bytes per texel, or per 4x4 block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    /// Uncompressed, this many bytes a texel: 18 and 20 RGBA8, 23 and 24 BGRA8.
    Texel(u32),
    /// Block-compressed, this many bytes a 4x4 block: 54-57 BC1, 58-59 BC2, 60-61 BC3, 66-67 BC7.
    Block(u32),
}

/// The storage of format ordinal `format`, for the colour formats ReSkate decodes; `None` for any other.
pub fn block(format: u32) -> Option<Storage> {
    Some(match format {
        18 | 20 | 23 | 24 => Storage::Texel(4),
        54..=57 => Storage::Block(8),
        58..=61 | 66 | 67 => Storage::Block(16),
        _ => return None,
    })
}

/// The bytes of a `width` x `height` level stored as `s` (blocks cover partial edges); `None` on overflow.
pub fn level_size(s: Storage, width: u32, height: u32) -> Option<u32> {
    match s {
        Storage::Texel(n) => width.checked_mul(height)?.checked_mul(n),
        Storage::Block(n) => width
            .div_ceil(4)
            .max(1)
            .checked_mul(height.div_ceil(4).max(1))?
            .checked_mul(n),
    }
}

/// The level sizes of a full chain of `mips` levels from `width` x `height` (each side halved, at least 1), largest
/// first; `None` on overflow.
pub fn chain(s: Storage, width: u32, height: u32, mips: u8) -> Option<Vec<u32>> {
    (0..u32::from(mips))
        .map(|l| level_size(s, (width >> l).max(1), (height >> l).max(1)))
        .collect()
}
