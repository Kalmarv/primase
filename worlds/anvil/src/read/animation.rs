//! An Animation resource's body (the clips of Riders Republic: 9,087 in the six forges' census), read as stored: its
//! header, which bones' tracks it holds and the tracks' framing. What a track's values mean is not read here
//! ([`StoredTrack::values`] are bytes); where the clip's key times fall in seconds isn't either.
//!
//! The resource's own header is longer than other types': its file-header byte is 1 and a table follows it
//! ([`crate::read::data::Resource::header_rows`]). The body (little-endian; the registry's field order, from the
//! `Animation` class, checked on every clip):
//! - a byte (1 on all); `Length` (f32); `F4F597FF` and `322B96B8` (u32 each: the first varies by clip, the second is 0
//!   on all);
//! - the two u16 arrays `556E9166` and `251905D9` (a u32 count and its u16s: empty on all);
//! - the four booleans, a byte each (`1F4DA15F`, `93847F6A`, `ABC2D4B8`, `0D7D1D50`: 0 on all but 390 clips, the
//!   streamed ones' (names ending `_Streamed`) fourth is 1);
//! - the `AnimTrackEvent` array (a u32 count; each element an object, `00` a u64 id and the type 0x41682213, then its
//!   fields: `EventSeed` objects with nested audio events, whose layout isn't read): kept as the bytes between the
//!   count and the next field, which is found by its anchor (below);
//! - the `AnimTrackData` pointer: `00`, a u64 id (0 on all), the type 0x0181EFE8, `567530C5` (u32), the
//!   `AnimTrackDataMapping` array (a u32 count; each element a u64 id (0), the type 0x653CAA76 and a u32: **the CRC-32
//!   of a bone's name**, the key of a Skeleton's bones; 0 for the root's tracks), and `66A6DC91` (u16, 0 on all);
//! - the `ECLAnimation` pointer: `03` (none) or `00`, a u64 id and the type 0xAC692A57 (`TTECLAnimation`), then its
//!   fields, a byte array (the notes: "tt-animation" strings), a u32 array, a u16 array and a byte array;
//! - `B832D26F`, a byte buffer (a u32 length), and `CB4E5F18` (u32).
//!
//! Then the tracks: a u32 count (the mappings' count, one track to each, in order) and that many blocks. A block is
//! a tag byte, a u32 `a` (its in-memory size: kept, within 8 of the payload's length on all), a u32 key count `b` and
//! the payload. **The tag is `3 * kind + w`**: `kind` selects the track class (each has a fixed value width,
//! [`value_width`]) and `w` how its key times are stored: 0 a u8 for each key after the first (the first is at 0), 1 a
//! u16 for each, 2 a marker byte (`0x40`, `0x80` or `0xC0` or'd with the number of bytes of the mask after it: the top
//! bits are kept, [`Times::Mask`]), those bytes, a bit mask of which frames hold a key (its set bits number `b`), no
//! times. Then `b` values of the kind's width. Measured on the 9,087 clips: every block of every clip reads so, each
//! clip's blocks ending at its body's last byte.
//!
//! Where the `AnimTrackEvent` array isn't empty its elements' length isn't read: the `AnimTrackData` object's start
//! is found as the first place after the count where a pointer's header with the type 0x0181EFE8 is followed by
//! mappings of the type 0x653CAA76 for the count they state.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

pub use crate::components::{StoredAnimation, StoredEcl, StoredEvents, StoredTrack, Times};

/// A record type: the CRC-32 of `Animation`.
pub const ANIMATION: u32 = 0x0FA3_067F;
/// An `AnimTrackData`'s type, an `AnimTrackDataMapping`'s, an `AnimTrackEvent`'s and a `TTECLAnimation`'s.
const ANIM_TRACK_DATA: u32 = 0x0181_EFE8;
const MAPPING: u32 = 0x653C_AA76;
const TTECL_ANIMATION: u32 = 0xAC69_2A57;

/// Why an Animation body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("animation body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("animation {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("animation track {track}: tag {tag:#04x} is kind {kind}, whose value width isn't read")]
    Kind { track: usize, tag: u8, kind: u8 },
    #[error("animation track {track}: {why}")]
    Track { track: usize, why: String },
    #[error("animation body: {0} bytes after its last track")]
    Trailing(usize),
    #[error("animation events: no AnimTrackData after the {0} events")]
    NoTrackData(u32),
}

/// The value width in bytes of a track class (`kind`, a tag over 3): the widths the census of Riders Republic's clips
/// settles (each by blocks that read to the body's last byte); `None` for a kind not seen.
pub fn value_width(kind: u8) -> Option<usize> {
    Some(match kind {
        3 | 21 | 22 => 4,
        4 => 6,
        5 | 25 => 8,
        7 | 17 => 2,
        8 | 9 => 12,
        16 | 18 => 1,
        _ => return None,
    })
}

impl StoredTrack {
    /// The track class: the tag over 3.
    pub fn kind(&self) -> u8 {
        self.tag.checked_div(3).unwrap_or(0)
    }

    /// How its key times are stored: the tag's remainder over 3 (0 a u8 each, 1 a u16 each, 2 a mask).
    pub fn time_form(&self) -> u8 {
        self.tag.checked_rem(3).unwrap_or(0)
    }
}

/// Little-endian fields read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(n).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s = self.b.get(self.at..end).ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let at = self.at;
        self.take(N, what)?.try_into().map_err(|_| Error::Short {
            what,
            at,
            have: self.b.len(),
        })
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        let [b] = self.array::<1>(what)?;
        Ok(b)
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array(what)?))
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
    fn expect(&mut self, what: &'static str, want: u64, width: usize) -> Result<(), Error> {
        let at = self.at;
        let found = match width {
            1 => u64::from(self.u8(what)?),
            4 => u64::from(self.u32(what)?),
            _ => self.u64(what)?,
        };
        if found == want {
            Ok(())
        } else {
            Err(Error::Value {
                what,
                at,
                found,
                read: "the grammar's value",
            })
        }
    }
    /// A count of items of `each` bytes the rest of the body can hold.
    fn count(&mut self, what: &'static str, each: usize) -> Result<usize, Error> {
        let at = self.at;
        let n = usize::try_from(self.u32(what)?).unwrap_or(usize::MAX);
        let left = self.b.len().saturating_sub(self.at);
        if n.checked_mul(each.max(1)).is_none_or(|bytes| bytes > left) {
            return Err(Error::Value {
                what,
                at,
                found: u64::try_from(n).unwrap_or(u64::MAX),
                read: "a count the body can hold",
            });
        }
        Ok(n)
    }
    /// A counted array of `each`-byte items, as the bytes.
    fn counted(&mut self, what: &'static str, each: usize) -> Result<&'a [u8], Error> {
        let n = self.count(what, each)?;
        self.take(n.saturating_mul(each), what)
    }
    /// Where an `AnimTrackData` pointer lies at or after the cursor: `00`, a u64, the type, a u32 and a count of
    /// mappings of the type [`MAPPING`] for as many as the count says.
    fn find_track_data(&self) -> Option<usize> {
        let needle = ANIM_TRACK_DATA.to_le_bytes();
        let mut from = self.at;
        loop {
            let rest = self.b.get(from..)?;
            let found = rest.windows(4).position(|w| w == needle)?.checked_add(from)?;
            from = found.checked_add(1)?;
            let Some(start) = found.checked_sub(9).filter(|&s| s >= self.at) else {
                continue;
            };
            if self.b.get(start) != Some(&0) {
                continue;
            }
            let mut probe = Cursor {
                b: self.b,
                at: found.checked_add(4)?,
            };
            let valid = (|| -> Result<bool, Error> {
                probe.u32("track data")?;
                let n = probe.count("mapping count", 16)?;
                for _ in 0..n {
                    probe.u64("mapping")?;
                    if probe.u32("mapping type")? != MAPPING {
                        return Ok(false);
                    }
                    probe.u32("mapping")?;
                }
                Ok(true)
            })();
            if valid.unwrap_or(false) {
                return Some(start);
            }
        }
    }
}

impl StoredAnimation {
    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredAnimation, Error> {
        let mut c = Cursor { b: body, at: 0 };
        c.expect("first byte", 1, 1)?;
        let length = c.f32("Length")?;
        let id_hash = c.u32("F4F597FF")?;
        let f2 = c.u32("322B96B8")?;
        let mut arrays: [Vec<u16>; 2] = Default::default();
        for a in &mut arrays {
            *a = c
                .counted("u16 array", 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|w| u16::from_le_bytes(*w))
                .collect();
        }
        let flags = c.array::<4>("booleans")?;
        let count = c.u32("event count")?;
        let events = if count == 0 {
            StoredEvents {
                count,
                bytes: Vec::new(),
            }
        } else {
            let start = c.at;
            let at = c.find_track_data().ok_or(Error::NoTrackData(count))?;
            c.at = at;
            StoredEvents {
                count,
                bytes: body.get(start..at).unwrap_or_default().to_vec(),
            }
        };
        c.expect("AnimTrackData pointer", 0, 1)?;
        let track_data_id = c.u64("AnimTrackData")?;
        c.expect("AnimTrackData type", u64::from(ANIM_TRACK_DATA), 4)?;
        let track_data_hash = c.u32("567530C5")?;
        let n = c.count("mapping count", 16)?;
        let mut mappings = Vec::with_capacity(n);
        for _ in 0..n {
            c.u64("mapping id")?;
            c.expect("mapping type", u64::from(MAPPING), 4)?;
            mappings.push(c.u32("mapping")?);
        }
        let track_data_tail = c.u16("66A6DC91")?;
        let at = c.at;
        let ecl = match c.u8("ECLAnimation pointer")? {
            3 => None,
            0 => {
                let id = c.u64("TTECLAnimation")?;
                c.expect("TTECLAnimation type", u64::from(TTECL_ANIMATION), 4)?;
                Some(StoredEcl {
                    id,
                    animation: c.counted("TTECLAnimation bytes", 1)?.to_vec(),
                    words: c
                        .counted("TTECLAnimation words", 4)?
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|w| u32::from_le_bytes(*w))
                        .collect(),
                    halves: c
                        .counted("TTECLAnimation halves", 2)?
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|w| u16::from_le_bytes(*w))
                        .collect(),
                    bytes: c.counted("TTECLAnimation bytes", 1)?.to_vec(),
                })
            }
            found => {
                return Err(Error::Value {
                    what: "ECLAnimation pointer",
                    at,
                    found: u64::from(found),
                    read: "3 (none) or 0 (an object)",
                });
            }
        };
        let buffer = c.counted("B832D26F", 1)?.to_vec();
        let cb4e = c.u32("CB4E5F18")?;
        let tracks_at = c.at;
        let tracks_n = usize::try_from(c.u32("track count")?).unwrap_or(usize::MAX);
        if tracks_n != mappings.len() {
            return Err(Error::Value {
                what: "track count",
                at: tracks_at,
                found: u64::try_from(tracks_n).unwrap_or(u64::MAX),
                read: "the number of mappings",
            });
        }
        let mut tracks = Vec::with_capacity(tracks_n);
        for track in 0..tracks_n {
            tracks.push(Self::track(&mut c, track)?);
        }
        match body.len().saturating_sub(c.at) {
            0 => {}
            left => return Err(Error::Trailing(left)),
        }
        Ok(StoredAnimation {
            length,
            id_hash,
            f2,
            arrays,
            flags,
            events,
            track_data_id,
            track_data_hash,
            mappings,
            track_data_tail,
            ecl,
            buffer,
            cb4e,
            tracks,
        })
    }

    fn track(c: &mut Cursor<'_>, track: usize) -> Result<StoredTrack, Error> {
        let why = |why: String| Error::Track { track, why };
        let tag = c.u8("track tag")?;
        let a = c.u32("track size")?;
        let keys = c.u32("track key count")?;
        let kind = tag.checked_div(3).unwrap_or(0);
        let width = value_width(kind).ok_or(Error::Kind { track, tag, kind })?;
        if keys == 0 {
            return Err(why("no keys".into()));
        }
        let n = usize::try_from(keys).map_err(|_| why("key count past this machine".into()))?;
        let times = match tag.checked_rem(3).unwrap_or(0) {
            0 => Times::U8(c.take(n.saturating_sub(1), "u8 key times")?.to_vec()),
            1 => Times::U16(
                c.take(n.saturating_sub(1).saturating_mul(2), "u16 key times")?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|w| u16::from_le_bytes(*w))
                    .collect(),
            ),
            _ => {
                let marker = c.u8("key mask marker")?;
                let bytes = usize::from(marker & 0x3F);
                if marker < 0x40 || !(1..=8).contains(&bytes) {
                    return Err(why(format!("key mask marker {marker:#04x}")));
                }
                let mask = c.take(bytes, "key mask")?;
                let set: u32 = mask.iter().map(|b| b.count_ones()).sum();
                if set != keys {
                    return Err(why(format!("a key mask of {set} frames for {keys} keys")));
                }
                Times::Mask {
                    mode: marker >> 6,
                    mask: mask.to_vec(),
                }
            }
        };
        let values = c.take(n.saturating_mul(width), "track values")?.to_vec();
        let length = times.stored_bytes().saturating_add(values.len());
        let near = usize::try_from(a).unwrap_or(usize::MAX);
        if near.abs_diff(length) > 8 {
            return Err(why(format!("size {a} for a payload of {length} bytes")));
        }
        Ok(StoredTrack {
            tag,
            size: a,
            keys,
            times,
            values,
        })
    }
}

impl Times {
    /// The bytes the times take in the block (a mask's marker byte included).
    fn stored_bytes(&self) -> usize {
        match self {
            Times::U8(t) => t.len(),
            Times::U16(t) => t.len().saturating_mul(2),
            Times::Mask { mask, .. } => mask.len().saturating_add(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(id: u64, kind: u32) -> Vec<u8> {
        let mut b = vec![0];
        b.extend(id.to_le_bytes());
        b.extend(kind.to_le_bytes());
        b
    }

    /// A body of the given tracks (blocks as bytes), mappings and ECLAnimation pointer bytes.
    fn body(events: Option<&[u8]>, mappings: &[u32], ecl: &[u8], blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut b = vec![1];
        b.extend(1.5f32.to_le_bytes());
        b.extend(0xAABB_CCDDu32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend([0u8; 8]);
        b.extend([0, 0, 0, 1]);
        match events {
            None => b.extend(0u32.to_le_bytes()),
            Some(e) => {
                b.extend(2u32.to_le_bytes());
                b.extend(e);
            }
        }
        b.extend(object(0, ANIM_TRACK_DATA));
        b.extend(0x1234u32.to_le_bytes());
        b.extend(u32::try_from(mappings.len()).expect("count").to_le_bytes());
        for m in mappings {
            b.extend(0u64.to_le_bytes());
            b.extend(MAPPING.to_le_bytes());
            b.extend(m.to_le_bytes());
        }
        b.extend(0u16.to_le_bytes());
        b.extend(ecl);
        b.extend(0u32.to_le_bytes());
        b.extend(42u32.to_le_bytes());
        b.extend(u32::try_from(blocks.len()).expect("count").to_le_bytes());
        for block in blocks {
            b.extend(block);
        }
        b
    }

    fn block(tag: u8, a: u32, keys: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = vec![tag];
        b.extend(a.to_le_bytes());
        b.extend(keys.to_le_bytes());
        b.extend(payload);
        b
    }

    #[test]
    fn tracks_of_each_time_form_read_by_their_kinds_widths() {
        // kind 9 (3 f32), one key, no times: 12 bytes, size 16
        let translation = block(27, 16, 1, &[0; 12]);
        // kind 5 (8 bytes), u8 times: 3 keys, 2 times
        let rotation = block(
            15,
            26,
            3,
            &[
                2, 4, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3,
            ],
        );
        // kind 5, u16 times: 2 keys, 1 time (tag 16)
        let two = block(16, 20, 2, &[5, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]);
        // kind 7 (2 bytes), mask of 2 bytes with 3 set bits (tag 23 = 3 * 7 + 2)
        let masked = block(23, 9, 3, &[0x42, 0b0000_0101, 0b0000_0010, 1, 0, 2, 0, 3, 0]);
        let b = body(None, &[0, 0, 7, 7], &[3], &[translation, rotation, two, masked]);
        let a = StoredAnimation::parse(&b).expect("animation");
        assert_eq!(
            (a.length, a.id_hash, a.mappings.clone()),
            (1.5, 0xAABB_CCDD, vec![0, 0, 7, 7])
        );
        assert_eq!(a.flags, [0, 0, 0, 1]);
        assert!(a.ecl.is_none() && a.events.count == 0);
        let [t0, t1, t2, t3] = &a.tracks[..] else {
            panic!("{} tracks", a.tracks.len())
        };
        assert_eq!((t0.kind(), t0.time_form(), t0.values.len()), (9, 0, 12));
        assert_eq!(t1.times, Times::U8(vec![2, 4]));
        assert_eq!(t2.times, Times::U16(vec![5]));
        assert_eq!(
            t3.times,
            Times::Mask {
                mode: 1,
                mask: vec![5, 2]
            }
        );
        assert_eq!(t3.values, [1, 0, 2, 0, 3, 0]);
    }

    #[test]
    fn a_mask_with_the_wrong_number_of_frames_or_a_kind_not_seen_is_refused() {
        let wrong = block(23, 9, 3, &[0x42, 1, 1, 1, 0, 2, 0, 3, 0]);
        let e = StoredAnimation::parse(&body(None, &[1], &[3], &[wrong])).expect_err("mask");
        assert!(e.to_string().contains("2 frames for 3 keys"), "{e}");
        let unseen = block(30, 16, 1, &[0; 12]);
        let e = StoredAnimation::parse(&body(None, &[1], &[3], &[unseen])).expect_err("kind");
        assert!(matches!(e, Error::Kind { kind: 10, .. }), "{e}");
    }

    #[test]
    fn events_are_kept_as_the_bytes_before_the_track_data_and_an_ecl_object_reads() {
        let events = vec![0xEE; 40];
        let mut ecl = object(9, TTECL_ANIMATION);
        ecl.extend(3u32.to_le_bytes());
        ecl.extend([7, 8, 9]);
        ecl.extend(1u32.to_le_bytes());
        ecl.extend(0xDEADu32.to_le_bytes());
        ecl.extend(0u32.to_le_bytes());
        ecl.extend(1u32.to_le_bytes());
        ecl.push(5);
        let b = body(Some(&events), &[0], &ecl, &[block(27, 16, 1, &[0; 12])]);
        let a = StoredAnimation::parse(&b).expect("animation");
        assert_eq!((a.events.count, a.events.bytes.clone()), (2, events));
        let ecl = a.ecl.expect("ecl");
        assert_eq!(
            (ecl.id, ecl.animation, ecl.words, ecl.halves, ecl.bytes),
            (9, vec![7, 8, 9], vec![0xDEAD], vec![], vec![5])
        );
    }

    #[test]
    fn bytes_after_the_last_track_or_a_track_count_off_the_mappings_are_refused() {
        let mut b = body(None, &[1], &[3], &[block(27, 16, 1, &[0; 12])]);
        b.push(0);
        assert!(matches!(StoredAnimation::parse(&b), Err(Error::Trailing(1))));
        let b = body(None, &[1, 2], &[3], &[block(27, 16, 1, &[0; 12])]);
        assert!(StoredAnimation::parse(&b).is_err());
    }
}
