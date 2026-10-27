//! The 24-byte `CellGcmTexture` descriptor libgcm uses to describe a texture, read as data.
//!
//! Every field is big endian. The descriptor carries GPU address metadata (`offset`, `location`); both are kept as
//! stored and neither says anything about where a payload lies in a file. Whether a descriptor describes storage
//! that can be decoded is a separate question for the caller: this module reads the fields and refuses only a
//! descriptor of the wrong length.
//!
//! References for libgcm's vocabulary:
//! <https://github.com/ps3dev/PSL1GHT/blob/master/ppu/include/rsx/gcm_sys.h> (`gcmTexture`).

/// A descriptor of the wrong size.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A `CellGcmTexture` is exactly 24 bytes.
    #[error("CellGcmTexture requires 24 bytes, got {0}")]
    DescriptorLength(usize),
}

/// The 24-byte descriptor, including GPU address metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGcmTexture {
    /// The pixel format byte. Bit `0x20` (LN) selects linear storage with a constant base pitch and bit `0x40`
    /// (UN) selects unnormalized coordinates; the format proper is `format & !0x60`.
    pub format: u8,
    /// The mip count.
    pub mipmap: u8,
    /// 1, 2 or 3 (a volume).
    pub dimension: u8,
    /// 1 for a cube map.
    pub cubemap: u8,
    /// The libgcm component remap word; identity is `0xaae4`.
    pub remap: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// The memory location code, as stored.
    pub location: u8,
    pub pad: u8,
    /// The base row pitch in bytes, meaningful for linear storage.
    pub pitch: u32,
    /// The GPU address offset, as stored.
    pub offset: u32,
}

impl CellGcmTexture {
    /// Parse exactly one descriptor as data.
    ///
    /// # Errors
    /// [`Error::DescriptorLength`] when `data` is not 24 bytes.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let b: &[u8; 24] = data.try_into().map_err(|_| Error::DescriptorLength(data.len()))?;
        let u16_at = |i| u16::from_be_bytes([b[i], b[i + 1]]);
        let u32_at = |i| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(Self {
            format: b[0],
            mipmap: b[1],
            dimension: b[2],
            cubemap: b[3],
            remap: u32_at(4),
            width: u16_at(8),
            height: u16_at(10),
            depth: u16_at(12),
            location: b[14],
            pad: b[15],
            pitch: u32_at(16),
            offset: u32_at(20),
        })
    }

    /// LN: linear storage with a constant base pitch.
    pub fn linear(self) -> bool {
        self.format & 0x20 != 0
    }

    /// UN: unnormalized sampling coordinates. It does not change pixel storage.
    pub fn unnormalized_coordinates(self) -> bool {
        self.format & 0x40 != 0
    }
}
