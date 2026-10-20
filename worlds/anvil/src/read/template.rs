//! A MaterialTemplate's descriptor run: its counted property descriptors, each with three strings and its metadata
//! lists, read as stored. The rest of the template is not read, and the run is found by search, not by parsing the
//! body from its start: the template's whole layout is not established.
//!
//! A descriptor (little-endian): eight zero bytes; 0xD933DB50; a byte (2 or 5 seen); a u64; a u32; three strings;
//! a u32 count and that many strings; a u32 count and that many strings. A string is a u32 byte length, that many
//! bytes, and one zero byte when the length isn't 0. The run is a u32 count n directly followed by n descriptors,
//! each starting where the one before ends. [`descriptors`] takes the one place in the body where such a run reads
//! whole; a body with none, or with more than one, is refused.
//!
//! Established by the census (worlds/anvil/tests/census.rs) on every MaterialTemplate of Riders Republic, and by the
//! join it measures: a Material's texture-property key equals the CRC-32 of the second string of a descriptor in its
//! own template. What the strings, the byte, the u64 and the u32 mean is not read (the second string reads as a
//! label: "2 - Normal Map").
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// A record type: equals the CRC-32 of the ASCII string `MaterialTemplate` (the name a label).
pub use crate::read::material::MATERIAL_TEMPLATE;

/// A descriptor's marker, after its eight zero bytes.
pub const DESCRIPTOR: u32 = 0xD933_DB50;

/// One descriptor as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// Where it starts in the body.
    pub at: usize,
    /// The byte after the marker.
    pub byte: u8,
    /// The u64 after it.
    pub link: u64,
    /// The u32 after that.
    pub word: u32,
    /// The three strings, as stored bytes.
    pub strings: [Vec<u8>; 3],
    /// The first counted list of strings.
    pub keys: Vec<Vec<u8>>,
    /// The second counted list of strings.
    pub values: Vec<Vec<u8>>,
}

impl Descriptor {
    /// The CRC-32 of its second string (equal to a Material texture-property key in the census's join).
    pub fn key(&self) -> u32 {
        let [_, label, _] = &self.strings;
        primase_hash::crc32(label)
    }
}

/// Why a template's descriptors can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MaterialTemplate: no descriptor run reads whole")]
    None,
    #[error(
        "MaterialTemplate: {0} descriptor runs read whole, at {1:?}: which is the template's isn't established"
    )]
    Several(usize, Vec<usize>),
}

/// Fields read in order, `None` past the body.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)?.try_into().ok().map(u64::from_le_bytes)
    }
    fn string(&mut self) -> Option<Vec<u8>> {
        let n = usize::try_from(self.u32()?).ok()?;
        let s = self.take(n)?.to_vec();
        if n > 0 && self.take(1)? != [0] {
            return None;
        }
        Some(s)
    }
    fn strings(&mut self) -> Option<Vec<Vec<u8>>> {
        let n = usize::try_from(self.u32()?).ok()?;
        if n > self.b.len() / 4 {
            return None;
        }
        (0..n).map(|_| self.string()).collect()
    }
    fn descriptor(&mut self) -> Option<Descriptor> {
        let at = self.at;
        if self.u64()? != 0 || self.u32()? != DESCRIPTOR {
            return None;
        }
        let [byte] = self.take(1)? else {
            return None;
        };
        let byte = *byte;
        let link = self.u64()?;
        let word = self.u32()?;
        let strings = [self.string()?, self.string()?, self.string()?];
        let keys = self.strings()?;
        let values = self.strings()?;
        Some(Descriptor {
            at,
            byte,
            link,
            word,
            strings,
            keys,
            values,
        })
    }
}

/// The run of n descriptors whose count is the u32 before `first`, if it reads whole.
fn run(body: &[u8], first: usize) -> Option<Vec<Descriptor>> {
    let count_at = first.checked_sub(4)?;
    let mut c = Cursor {
        b: body,
        at: count_at,
    };
    let n = usize::try_from(c.u32()?).ok()?;
    if n == 0 || n > body.len() / 37 {
        return None;
    }
    (0..n).map(|_| c.descriptor()).collect()
}

/// The template body's descriptor run: the one place where a u32 count is followed by that many descriptors read
/// whole. Refused when there is none, or more than one.
pub fn descriptors(body: &[u8]) -> Result<Vec<Descriptor>, Error> {
    let marker = DESCRIPTOR.to_le_bytes();
    let mut runs: Vec<Vec<Descriptor>> = Vec::new();
    let mut inside_a_run_up_to = 0usize;
    for first in 12..body.len() {
        // a descriptor of a run already found isn't a second run's first
        if first < inside_a_run_up_to {
            continue;
        }
        let signature = body.get(first..first.saturating_add(12));
        if signature.is_none_or(|s| s.get(..8) != Some(&[0; 8][..]) || s.get(8..) != Some(&marker[..])) {
            continue;
        }
        if let Some(r) = run(body, first) {
            inside_a_run_up_to = r.last().map_or(first, |d| d.at.saturating_add(1));
            runs.push(r);
        }
    }
    match runs.len() {
        0 => Err(Error::None),
        1 => Ok(runs.pop().unwrap_or_default()),
        n => Err(Error::Several(
            n,
            runs.iter().filter_map(|r| r.first().map(|d| d.at)).collect(),
        )),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test fixtures built by hand")]
mod tests {
    use super::*;

    fn string(b: &mut Vec<u8>, s: &[u8]) {
        b.extend(u32::try_from(s.len()).expect("short").to_le_bytes());
        b.extend(s);
        if !s.is_empty() {
            b.push(0);
        }
    }

    fn descriptor(b: &mut Vec<u8>, label: &[u8]) {
        b.extend([0; 8]);
        b.extend(DESCRIPTOR.to_le_bytes());
        b.push(2);
        b.extend(9u64.to_le_bytes());
        b.extend(0x4DDB_34EEu32.to_le_bytes());
        string(b, b"");
        string(b, label);
        string(b, b"shown");
        b.extend(1u32.to_le_bytes());
        string(b, b"Category");
        b.extend(1u32.to_le_bytes());
        string(b, b"MAD");
    }

    #[test]
    fn a_run_is_found_and_read_whole() {
        let mut b = vec![7; 40];
        b.extend(2u32.to_le_bytes());
        descriptor(&mut b, b"2 - Normal Map");
        descriptor(&mut b, b"1 - Diffuse Alpha");
        b.extend([5; 9]);
        let d = descriptors(&b).expect("run");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].key(), 0x443B_F980);
        assert_eq!(d[1].key(), 0xE4B6_CC48);
        assert_eq!(d[0].keys, [b"Category".to_vec()]);
    }

    #[test]
    fn none_or_two_runs_are_refused() {
        assert!(matches!(descriptors(&[0; 64]), Err(Error::None)));
        let mut b = vec![7; 20];
        b.extend(1u32.to_le_bytes());
        descriptor(&mut b, b"a");
        b.extend([7; 20]);
        b.extend(1u32.to_le_bytes());
        descriptor(&mut b, b"b");
        assert!(matches!(descriptors(&b), Err(Error::Several(2, _))));
    }
}
