//! Resources: a raw-DEFLATE body that inflates to the system (virtual) pages followed by the graphics (physical)
//! pages, laid out by two page-flag words. Pointers inside point at `0x5000_0000 + offset` (system) or
//! `0x6000_0000 + offset` (graphics). Loose resource files start with an RSC7 header carrying the flags; inside an
//! archive the entry carries them and the 16 bytes before the body hold no header (only the scattered bytes of an
//! oversized length, see `rpf`).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use primase_foundations::compress::{DEFLATE_MAX_RATIO, Deflate};

use crate::{Result, invalid};

/// The "RSC7" magic, little-endian.
pub const MAGIC: u32 = 0x3743_5352;

/// A resource's page layout for one memory kind: a base size and how many pages of each of nine sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFlags(pub u32);

impl PageFlags {
    /// The base page size: 512 bytes shifted by the low four bits.
    pub fn base_size(self) -> u64 {
        // a shift of at most 15 bits: no overflow
        0x200u64.wrapping_shl(self.0 & 0xF)
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

    /// Each page size's shift from the base size, largest first (parallel to [`PageFlags::page_counts`]).
    const PAGE_SHIFTS: [u32; 9] = [8, 7, 6, 5, 4, 3, 2, 1, 0];

    /// The pages' total size in bytes. The flags' fields bound it (a base of at most 2^24 bytes, at most 127 pages
    /// of a size, shifts of at most 8), far below u64's range: saturating arithmetic never saturates.
    pub fn size(self) -> u64 {
        let base = self.base_size();
        self.page_counts()
            .iter()
            .zip(Self::PAGE_SHIFTS)
            .map(|(&c, s)| u64::from(c).saturating_mul(base.wrapping_shl(s)))
            .fold(0u64, u64::saturating_add)
    }

    /// The size of the smallest page present (the last one laid out); 0 without pages.
    pub fn last_page_size(self) -> u64 {
        let base = self.base_size();
        self.page_counts()
            .iter()
            .zip(Self::PAGE_SHIFTS)
            .rev()
            .find(|(c, _)| **c > 0)
            .map_or(0, |(_, s)| base.wrapping_shl(s))
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
    /// What the readers have taken out of the pages so far ([`Resource::charge`]).
    pub spent: Spent,
}

/// How many times over a resource's own bytes the readers of it may copy out of it in all. Lists in a resource hold
/// pointers, and nothing stops two entries naming one object, or the objects of a list each naming a list: a few
/// hundred bytes can describe gigabytes (measured on bounds: a 185-byte body, 1.7 million bounds, 553 MB). A reader
/// that copies charges what it takes ([`Resource::charge`]) and a read that has taken more than this many times the
/// resource is refused. A judgement: an object that a real file shares by reference is charged every time it is read,
/// and the censuses (`tests/resource_census.rs`) say if any does.
pub const SPEND_TIMES: usize = 4;

/// The charge counter of a [`Resource`]: shared by the readers of it, copied with it.
#[derive(Debug, Default)]
pub struct Spent(std::sync::atomic::AtomicUsize);

impl Clone for Spent {
    fn clone(&self) -> Self {
        Spent(std::sync::atomic::AtomicUsize::new(
            self.0.load(std::sync::atomic::Ordering::Relaxed),
        ))
    }
}

impl Resource {
    /// Take `bytes` of the resource's allowance ([`SPEND_TIMES`]) for a read that copies them out, at `pointer`. An
    /// error once the readers have taken more than the allowance: the resource's lists share what they point at.
    pub fn charge(&self, bytes: usize, pointer: u64) -> Result<()> {
        use std::sync::atomic::Ordering;
        let allowance = self.data.len().saturating_mul(SPEND_TIMES);
        let taken = self
            .spent
            .0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                Some(t.saturating_add(bytes))
            })
            .unwrap_or(usize::MAX)
            .saturating_add(bytes);
        if taken > allowance {
            return Err(invalid(
                "RSC7",
                pointer,
                format!(
                    "the reads copy more than {SPEND_TIMES} times the resource's {} bytes (its lists share their objects)",
                    self.data.len()
                ),
            ));
        }
        Ok(())
    }

    /// The system pages, then the graphics pages: `data` holds exactly both, as [`inflate`] makes it; a resource
    /// whose data is another size (one built by hand) is an error, never read as all system pages.
    fn pages(&self) -> Result<(&[u8], &[u8])> {
        let (system, graphics) = (self.system.size(), self.graphics.size());
        let split = usize::try_from(system)
            .ok()
            .filter(|_| u64::try_from(self.data.len()).ok() == system.checked_add(graphics))
            .and_then(|n| self.data.split_at_checked(n));
        split.ok_or_else(|| {
            invalid(
                "RSC7",
                0,
                format!(
                    "{} bytes of pages, not the {system} system and {graphics} graphics bytes its flags state",
                    self.data.len()
                ),
            )
        })
    }

    /// The system (virtual) pages.
    pub fn system_data(&self) -> Result<&[u8]> {
        Ok(self.pages()?.0)
    }

    /// The graphics (physical) pages.
    pub fn graphics_data(&self) -> Result<&[u8]> {
        Ok(self.pages()?.1)
    }

    /// The `len` bytes a pointer points at (`0x5000_0000 + offset` in the system pages, `0x6000_0000 + offset` in
    /// the graphics pages, whose offsets may pass 256 MiB and so reach `0x7...`); an error when the pointer is of
    /// neither kind or the bytes run past its pages.
    pub fn at(&self, pointer: u64, len: usize) -> Result<&[u8]> {
        let (pages, offset) = match pointer {
            0x5000_0000..0x6000_0000 => (self.system_data()?, pointer.wrapping_sub(0x5000_0000)),
            0x6000_0000.. => (self.graphics_data()?, pointer.wrapping_sub(0x6000_0000)),
            _ => return Err(invalid("RSC7", pointer, "not a resource pointer")),
        };
        usize::try_from(offset)
            .ok()
            .and_then(|o| pages.get(o..o.checked_add(len)?))
            .ok_or_else(|| invalid("RSC7", pointer, format!("{len} bytes run past the pages")))
    }
}

impl Resource {
    /// The `N` bytes at `pointer`, as an array: a record of known size, read by constant offsets.
    pub fn block<const N: usize>(&self, pointer: u64) -> Result<&[u8; N]> {
        <&[u8; N]>::try_from(self.at(pointer, N)?)
            .map_err(|_| invalid("RSC7", pointer, format!("a block of {N} bytes")))
    }

    /// `n` records of `each` bytes at `pointer`, their total size checked.
    pub fn records(&self, pointer: u64, n: usize, each: usize) -> Result<&[u8]> {
        let len = n
            .checked_mul(each)
            .ok_or_else(|| invalid("RSC7", pointer, format!("{n} records of {each} bytes")))?;
        self.at(pointer, len)
    }
}

/// The 16-byte RSC7 header of a loose resource file: version, system flags, graphics flags.
pub fn header(stored: &[u8]) -> Result<(u32, PageFlags, PageFlags)> {
    let h = stored
        .first_chunk::<16>()
        .ok_or_else(|| invalid("RSC7", 0, "shorter than its header"))?;
    let [magic, version, system, graphics] = crate::rage::le::u32x4_of(h);
    if magic != MAGIC {
        return Err(invalid("RSC7", 0, format!("magic {magic:#010x}")));
    }
    Ok((version, PageFlags(system), PageFlags(graphics)))
}

/// The most the pages may reach past what the body can inflate to (the zero-filled end of the last page). The flags are
/// the file's word and a body of a few bytes can name pages of gigabytes; what the body cannot account for is
/// allocated only up to this, so a header cannot cost memory the body does not back. A judgement, not a measured
/// bound: the census (`tests/census.rs`) lists every resource whose body ends inside its last page.
const ZERO_FILL_LIMIT: usize = 64 << 20;

/// Inflate a resource body (already decrypted) laid out by `system` and `graphics`. The body must be one complete
/// DEFLATE stream consumed to its end that fills the pages exactly, or ends inside the last page (the rest of that
/// page is zero-filled and counted in `zero_filled`); anything else is an error. The pages are checked against the
/// body before anything is allocated for them: they may not need more than the body inflates to (a DEFLATE stream
/// expands at most [`DEFLATE_MAX_RATIO`] to one) plus [`ZERO_FILL_LIMIT`], and the output grows with the stream.
pub fn inflate(body: &[u8], system: PageFlags, graphics: PageFlags) -> Result<Resource> {
    let size = system
        .size()
        .checked_add(graphics.size())
        .and_then(|s| usize::try_from(s).ok())
        .ok_or_else(|| invalid("RSC7", 0, "page size overflows"))?;
    let last = if graphics.size() > 0 {
        graphics.last_page_size()
    } else {
        system.last_page_size()
    };
    let reach = body.len().saturating_mul(DEFLATE_MAX_RATIO);
    if size > reach.saturating_add(ZERO_FILL_LIMIT) {
        return Err(invalid(
            "RSC7",
            0,
            format!(
                "pages of {size} bytes from a body of {} bytes, which inflates to at most {reach} (and {ZERO_FILL_LIMIT} \
                 may be left zero)",
                body.len()
            ),
        ));
    }
    let mut data = Deflate.decompress_up_to(body, size)?;
    let zero_filled = size.abs_diff(data.len());
    if zero_filled > 0 {
        // the body ends inside the last page only
        if u64::try_from(zero_filled).map_or(true, |short| short >= last) {
            return Err(primase_foundations::compress::Error::Size {
                expected: size,
                actual: data.len(),
            }
            .into());
        }
        data.resize(size, 0);
    }
    Ok(Resource {
        version: system.version_bits() << 4 | graphics.version_bits(),
        system,
        graphics,
        data,
        zero_filled: zero_filled as u64,
        spent: Spent::default(),
    })
}

/// A loose resource file (RSC7 header + body).
pub fn inflate_file(stored: &[u8]) -> Result<Resource> {
    let (version, system, graphics) = header(stored)?;
    let body = stored
        .get(16..)
        .ok_or_else(|| invalid("RSC7", 0, "shorter than its header"))?;
    let mut r = inflate(body, system, graphics)?;
    r.version = version;
    Ok(r)
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

    /// A pointer reads the pages its flags state; data of another size is an error, not all system pages.
    #[test]
    fn the_pages_are_the_data_the_flags_state() {
        let resource = |len: usize| Resource {
            version: 0,
            // one system page of 0x2000 bytes, one graphics page of 0x1000
            system: PageFlags(4 | 1 << 27),
            graphics: PageFlags(3 | 1 << 27),
            data: (0..len).map(|i| (i / 0x1000) as u8).collect(),
            zero_filled: 0,
            spent: Spent::default(),
        };
        let r = resource(0x3000);
        assert_eq!(r.at(0x5000_1FFF, 1).unwrap(), [1]);
        assert_eq!(r.at(0x6000_0000, 1).unwrap(), [2]);
        assert!(r.at(0x6000_1000, 1).is_err());
        for len in [0x2000, 0x3001] {
            let e = resource(len).at(0x5000_0000, 1).unwrap_err().to_string();
            assert!(
                e.contains("system and 4096 graphics bytes its flags state"),
                "{e}"
            );
        }
    }
}
