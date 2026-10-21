//! A LODSelector resource's body: its five levels (each a reference to a Mesh or none) and its dependencies, read as
//! stored. The dependencies are the leading levels without a reference, in order ([`StoredLodSelector::level_ids`]):
//! on every LODSelector of Riders Republic (24,430 by ClassID, a patch's copy replacing its base's) the dependency
//! count equals the number of leading levels without a reference, and each of the 19,623 dependencies i is a Mesh
//! whose name ends `_LODi` (the census; RRAssetStudio's claim, which this measures). A selector whose count differs is
//! refused there.
//!
//! The body (little-endian, after the resource's class id and extension): a byte (1), a u32, 7 bytes; five levels of
//! 48 + 4n bytes, each a u64, 0xA360319A (equal to the CRC-32 of `LODDescriptor`, the name a label), a 10-byte reference
//! (`01`, a byte, a u64 ClassID; or `03` and nine zero bytes, none), three f32, 5 bytes, a u32, a u32 count n, n u32
//! and a byte; the dependency count d (u32) and d dependencies (a byte 0 and a u64 ClassID); d again (u32) and d u32; 6
//! bytes, ending the body.
//!
//! Established on two LODSelectors of the base WhiteRoom forge (Riders Republic build 25394987: entries 24 and 159,
//! one direct reference and none, two direct references and two dependencies; every byte consumed to the body's end,
//! and every reference and dependency resolving to a Mesh record; games/riders-republic
//! docs/recon/placement-darkroom-handoff.md, b8dae7b). Refused, naming it: another first byte or level marker, a
//! reference [`crate::read::reference`] refuses, a dependency byte other than 0, a second count other than d, or
//! bytes after the last 6. A level's count at +43 counts u32s kept as stored before its last byte (48 + 4n bytes):
//! every LODSelector of Riders Republic (48,862) reads so to its body's end in the census; RRAssetStudio reads the same.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::reference::{self, Reference};

/// A record type: equals the CRC-32 of the ASCII string `LODSelector` (the name a label).
pub const LOD_SELECTOR: u32 = 0x51DC_6B80;
/// A level's marker: equals the CRC-32 of `LODDescriptor` (the name a label).
pub const LOD_DESCRIPTOR: u32 = 0xA360_319A;
/// The levels a LODSelector stores.
pub const LEVELS: usize = 5;

/// Why a LODSelector body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("LODSelector body: {have} bytes, shorter than its {what}")]
    Short { what: &'static str, have: usize },
    #[error("LODSelector {what}: {found}, only {read} is read")]
    Value {
        what: &'static str,
        found: u64,
        read: &'static str,
    },
    #[error("LODSelector level reference: {0}")]
    Reference(#[source] crate::read::reference::Error),
    #[error("LODSelector body: {0} bytes after its end")]
    Trailing(usize),
}

/// One level as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredLevel {
    /// Its first u64 (on both fixtures the selector's ClassID + 13 + the level's index; meaning not established).
    pub head: u64,
    pub reference: Option<Reference>,
    /// The three f32 after the reference, as stored: `SwitchDistance`, `TransitionZoneSize` and
    /// `FadeTimeMultiplier` (registry-named-fields §2).
    pub floats: [f32; 3],
    /// The 5 bytes after them.
    pub bytes_34: [u8; 5],
    /// The u32 after those (0, 1 and 2 seen).
    pub u32_39: u32,
    /// The level's last byte.
    /// The u32s the count at +43 counts, as stored (RRAssetStudio calls them bone hashes: its claim).
    pub words: Vec<u32>,
    /// The level's last byte.
    pub last: u8,
}

/// A LODSelector body as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredLodSelector {
    /// The u32 after the first byte.
    pub head: u32,
    /// The 7 bytes after it.
    pub head_rest: [u8; 7],
    pub levels: Vec<StoredLevel>,
    /// The dependencies' ClassIDs (Meshes on every fixture), in order.
    pub dependencies: Vec<u64>,
    /// The u32s after the second count, one per dependency (not the dependencies' stored sizes in the forge index).
    pub dependency_words: Vec<u32>,
    /// The body's last 6 bytes.
    pub trailer: [u8; 6],
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
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        let [b] = self.array::<1>(what)?;
        Ok(b)
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array(what)?))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array(what)?))
    }
}

impl StoredLevel {
    /// Its `SwitchDistance`: the level's far limit.
    pub fn switch_distance(&self) -> f32 {
        self.floats[0]
    }
}

impl StoredLodSelector {
    /// Each level's ClassID, none where it has none: its reference, or for the leading levels without one the
    /// dependencies in order. Refused where the dependency count isn't the number of those leading levels.
    pub fn level_ids(&self) -> Result<Vec<Option<u64>>, Error> {
        let leading = self.levels.iter().take_while(|l| l.reference.is_none()).count();
        if self.dependencies.len() != leading {
            return Err(Error::Value {
                what: "dependency count",
                found: u64::try_from(self.dependencies.len()).unwrap_or(u64::MAX),
                read: "the number of leading levels without a reference",
            });
        }
        let mut dependencies = self.dependencies.iter();
        Ok(self
            .levels
            .iter()
            .map(|l| l.reference.map(|r| r.id).or_else(|| dependencies.next().copied()))
            .collect())
    }

    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredLodSelector, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let first = c.u8("first byte")?;
        if first != 1 {
            return Err(Error::Value {
                what: "first byte",
                found: u64::from(first),
                read: "1",
            });
        }
        let head = c.u32("head")?;
        let head_rest = c.array::<7>("head")?;
        let mut levels = Vec::with_capacity(LEVELS);
        for _ in 0..LEVELS {
            let level_head = c.u64("level")?;
            let marker = c.u32("level marker")?;
            if marker != LOD_DESCRIPTOR {
                return Err(Error::Value {
                    what: "level marker",
                    found: u64::from(marker),
                    read: "0xA360319A",
                });
            }
            let reference =
                reference::parse(c.array::<{ reference::LENGTH }>("reference")?).map_err(Error::Reference)?;
            let floats = [c.f32("level")?, c.f32("level")?, c.f32("level")?];
            let bytes_34 = c.array::<5>("level")?;
            let u32_39 = c.u32("level")?;
            let count = c.u32("level count")?;
            let count = usize::try_from(count).unwrap_or(usize::MAX);
            if count > body.len() / 4 {
                return Err(Error::Short {
                    what: "level's counted u32s",
                    have: body.len(),
                });
            }
            let words = (0..count)
                .map(|_| c.u32("level's counted u32"))
                .collect::<Result<_, _>>()?;
            levels.push(StoredLevel {
                head: level_head,
                reference,
                floats,
                bytes_34,
                u32_39,
                words,
                last: c.u8("level")?,
            });
        }
        let d = c.u32("dependency count")?;
        let mut dependencies = Vec::new();
        for _ in 0..d {
            let tag = c.u8("dependency")?;
            if tag != 0 {
                return Err(Error::Value {
                    what: "dependency tag",
                    found: u64::from(tag),
                    read: "0",
                });
            }
            dependencies.push(c.u64("dependency")?);
        }
        let again = c.u32("second dependency count")?;
        if again != d {
            return Err(Error::Value {
                what: "second dependency count",
                found: u64::from(again),
                read: "the dependency count",
            });
        }
        let dependency_words = (0..d)
            .map(|_| c.u32("dependency word"))
            .collect::<Result<_, _>>()?;
        let trailer = c.array::<6>("last bytes")?;
        let left = body.len().saturating_sub(c.at);
        if left != 0 {
            return Err(Error::Trailing(left));
        }
        Ok(StoredLodSelector {
            head,
            head_rest,
            levels,
            dependencies,
            dependency_words,
            trailer,
        })
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test fixtures built by hand")]
mod tests {
    use super::*;

    /// A body whose levels have these references (None: empty), these dependencies, and `count` at the first level.
    fn body(references: [Option<u64>; LEVELS], dependencies: &[u64], count: u32) -> Vec<u8> {
        let mut b = vec![1, 0, 0, 0, 0];
        b.extend([0; 7]);
        for (k, r) in references.iter().enumerate() {
            b.extend(7u64.to_le_bytes());
            b.extend(LOD_DESCRIPTOR.to_le_bytes());
            match r {
                Some(id) => {
                    b.extend([1, 1]);
                    b.extend(id.to_le_bytes());
                }
                None => b.extend([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            }
            for f in [20.0f32, 0.0, 1.0] {
                b.extend(f.to_le_bytes());
            }
            b.extend([0, 0, 0, 0, 1]);
            b.extend(0u32.to_le_bytes());
            let n = if k == 0 { count } else { 0 };
            b.extend(n.to_le_bytes());
            for _ in 0..n {
                b.extend(7u32.to_le_bytes());
            }
            b.push(0);
        }
        let d = u32::try_from(dependencies.len()).expect("few");
        b.extend(d.to_le_bytes());
        for id in dependencies {
            b.push(0);
            b.extend(id.to_le_bytes());
        }
        b.extend(d.to_le_bytes());
        for _ in dependencies {
            b.extend(9u32.to_le_bytes());
        }
        b.extend([0, 0, 0, 0, 1, 0]);
        b
    }

    #[test]
    fn a_body_reads_its_levels_and_dependencies() {
        let s = StoredLodSelector::parse(&body([None, Some(5), None, None, None], &[8], 0)).expect("parse");
        assert_eq!(s.levels.len(), LEVELS);
        assert_eq!(s.levels[1].reference, Some(Reference { mode: 1, id: 5 }));
        assert_eq!(s.levels[0].reference, None);
        assert_eq!(
            (s.dependencies.as_slice(), s.dependency_words.as_slice()),
            (&[8u64][..], &[9u32][..])
        );
    }

    #[test]
    fn another_value_or_a_trailing_byte_is_refused() {
        let good = body([Some(5), None, None, None, None], &[], 0);
        let mut b = good.clone();
        b[0] = 2;
        assert!(matches!(
            StoredLodSelector::parse(&b),
            Err(Error::Value {
                what: "first byte",
                ..
            })
        ));
        let mut b = good.clone();
        b[12 + 12] = 2;
        assert!(matches!(StoredLodSelector::parse(&b), Err(Error::Reference(_))));
        let mut b = good.clone();
        b[60 + 13] = 1;
        assert!(matches!(StoredLodSelector::parse(&b), Err(Error::Reference(_))));
        // a level's counted u32s are read; a count past the body is refused
        let s = StoredLodSelector::parse(&body([Some(5), None, None, None, None], &[], 2)).expect("parse");
        assert_eq!(s.levels[0].words, [7, 7]);
        let mut b = good.clone();
        b[12 + 43] = 0xFF;
        b[12 + 44] = 0xFF;
        assert!(StoredLodSelector::parse(&b).is_err());
        let mut b = good.clone();
        b.push(0);
        assert!(matches!(StoredLodSelector::parse(&b), Err(Error::Trailing(1))));
    }
}
