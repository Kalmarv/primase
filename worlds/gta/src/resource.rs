//! Resources: a raw-DEFLATE body that inflates to the system (virtual) pages followed by the graphics (physical)
//! pages, laid out by two page-flag words. Pointers inside point at `0x5000_0000 + offset` (system) or
//! `0x6000_0000 + offset` (graphics). Loose resource files start with an RSC7 header carrying the flags; inside an
//! archive the entry carries them and the 16 bytes before the body hold no header (only the scattered bytes of an
//! oversized length, see `rpf`).
use primase_compress::{Decompressor, Deflate};

use crate::{Result, invalid};

/// The "RSC7" magic, little-endian.
pub const MAGIC: u32 = 0x3743_5352;

/// A resource's page layout for one memory kind: a base size and how many pages of each of nine sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(pub u32);

impl PageFlags {
    /// The base page size: 512 bytes shifted by the low four bits.
    pub fn base_size(self) -> u64 {
        0x200u64 << (self.0 & 0xF)
    }

    /// Page counts, largest page size (base << 8) first.
    pub fn page_counts(self) -> [u32; 9] {
        let v = self.0;
        [
            (v >> 4) & 0x1,
            (v >> 5) & 0x3,
            (v >> 7) & 0xF,
            (v >> 11) & 0x3F,
            (v >> 17) & 0x7F,
            (v >> 24) & 0x1,
            (v >> 25) & 0x1,
            (v >> 26) & 0x1,
            (v >> 27) & 0x1,
        ]
    }

    /// The pages' total size in bytes.
    pub fn size(self) -> u64 {
        let base = self.base_size();
        self.page_counts()
            .iter()
            .enumerate()
            .map(|(i, &c)| u64::from(c) * (base << (8 - i)))
            .sum()
    }

    /// The size of the smallest page present (the last one laid out); 0 without pages.
    pub fn last_page_size(self) -> u64 {
        let base = self.base_size();
        self.page_counts()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| **c > 0)
            .map_or(0, |(i, _)| base << (8 - i))
    }

    /// The four high bits: half of the resource version.
    pub fn version_bits(self) -> u32 {
        self.0 >> 28
    }
}

/// An inflated resource.
#[derive(Clone, Debug)]
pub struct Resource {
    /// The resource type's version: the flags' high bits, system then graphics.
    pub version: u32,
    pub system: PageFlags,
    pub graphics: PageFlags,
    /// System pages then graphics pages, exactly `system.size() + graphics.size()` bytes.
    pub data: Vec<u8>,
    /// Bytes at the end of the last page the body did not reach, zero-filled (the game allocates whole pages and
    /// inflates into them, so a body may end inside its last page). 0 for almost every resource.
    pub zero_filled: u64,
}

impl Resource {
    /// The system (virtual) pages.
    pub fn system_data(&self) -> &[u8] {
        &self.data[..self.system.size() as usize]
    }

    /// The graphics (physical) pages.
    pub fn graphics_data(&self) -> &[u8] {
        &self.data[self.system.size() as usize..]
    }

    /// The `len` bytes a pointer points at (`0x5000_0000 + offset` in the system pages, `0x6000_0000 + offset` in
    /// the graphics pages, whose offsets may pass 256 MiB and so reach `0x7...`); an error when the pointer is of
    /// neither kind or the bytes run past its pages.
    pub fn at(&self, pointer: u64, len: usize) -> Result<&[u8]> {
        let (pages, offset) = match pointer {
            0x5000_0000..0x6000_0000 => (self.system_data(), pointer - 0x5000_0000),
            0x6000_0000.. => (self.graphics_data(), pointer - 0x6000_0000),
            _ => return Err(invalid("RSC7", pointer, "not a resource pointer")),
        };
        usize::try_from(offset)
            .ok()
            .and_then(|o| pages.get(o..o.checked_add(len)?))
            .ok_or_else(|| invalid("RSC7", pointer, format!("{len} bytes run past the pages")))
    }
}

/// The 16-byte RSC7 header of a loose resource file: version, system flags, graphics flags.
pub fn header(stored: &[u8]) -> Result<(u32, PageFlags, PageFlags)> {
    let h = stored
        .get(..16)
        .ok_or_else(|| invalid("RSC7", 0, "shorter than its header"))?;
    let w = |i: usize| u32::from_le_bytes([h[4 * i], h[4 * i + 1], h[4 * i + 2], h[4 * i + 3]]);
    if w(0) != MAGIC {
        return Err(invalid("RSC7", 0, format!("magic {:#010x}", w(0))));
    }
    Ok((w(1), PageFlags(w(2)), PageFlags(w(3))))
}

/// Inflate a resource body (already decrypted) laid out by `system` and `graphics`. The body must be one complete
/// DEFLATE stream consumed to its end that fills the pages exactly, or ends inside the last page (the rest of that
/// page is zero-filled and counted in `zero_filled`); anything else is an error.
pub fn inflate(body: &[u8], system: PageFlags, graphics: PageFlags) -> Result<Resource> {
    let size = system.size() + graphics.size();
    let size = usize::try_from(size).map_err(|_| invalid("RSC7", 0, "page size overflows"))?;
    let last = if graphics.size() > 0 {
        graphics.last_page_size()
    } else {
        system.last_page_size()
    };
    let (data, zero_filled) = match Deflate.decompress(body, size) {
        Ok(d) => (d, 0),
        Err(primase_compress::Error::Size { expected, actual })
            if actual < expected && ((expected - actual) as u64) < last =>
        {
            let mut d = Deflate.decompress(body, actual)?;
            d.resize(expected, 0);
            (d, (expected - actual) as u64)
        }
        Err(e) => return Err(e.into()),
    };
    Ok(Resource {
        version: system.version_bits() << 4 | graphics.version_bits(),
        system,
        graphics,
        data,
        zero_filled,
    })
}

/// A loose resource file (RSC7 header + body).
pub fn inflate_file(stored: &[u8]) -> Result<Resource> {
    let (version, system, graphics) = header(stored)?;
    let mut r = inflate(&stored[16..], system, graphics)?;
    r.version = version;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known values: base 0x2000 (shift 4) with one page of each of the four smallest sizes.
    #[test]
    fn page_sizes() {
        let f = PageFlags(4 | 1 << 24 | 1 << 25 | 1 << 26 | 1 << 27);
        assert_eq!(f.base_size(), 0x2000);
        assert_eq!(f.size(), 0x2000 * (8 + 4 + 2 + 1));
        // the largest page alone: base << 8
        assert_eq!(PageFlags(1 << 4).size(), 0x200 << 8);
        // 127 pages of base << 4
        assert_eq!(PageFlags(0x7F << 17).size(), 127 * (0x200 << 4));
    }
}
