//! An Animation resource's body (the clips of Riders Republic: 9,087 in the six forges' census), read as stored: its
//! header, which bones' tracks it holds and the tracks' framing. What a track's values mean is not read here
//! ([`StoredTrack::values`] are bytes); where the clip's key times fall in seconds isn't either.
//!
//! The resource's own header is longer than other types': its file-header byte is 1 and a table follows it
//! ([`crate::data::Resource::header_rows`]). The body (little-endian; the registry's field order, from the
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
//! **Key times** ([`StoredTrack::times`]): a u8 or u16 time is a whole number of 1/60 s from the clip's start (the last
//! is the clip's `Length` times 60 on 4,617 of 5,361 u8 tracks and 1,451 of 5,011 u16 tracks of a sample of clips; the
//! others end early); a mask's set bits are frames of 1/30 s (bit 0 is the first key's, on every one of 3,683 sampled
//! tracks, and the last bit is `Length` times 30 on 3,063 of them).
//!
//! **Values** ([`StoredTrack::values`]) of the kinds read. Each decode is checked by the spike of exact matches
//! (`1 - |dot| < 1e-6`) of first keys against the mapped bone's stored local rotation, which no other reading of the
//! same bytes gives:
//! - kind 9: three f32, a translation (a first key equals the mapped bone's stored local position on 25,940 of 38,122
//!   sampled tracks, the others being animated away from rest);
//! - kind 8: the quaternion's x, y and z as f32, w the positive root that makes it unit;
//! - kind 3: a little-endian u32, three 10-bit fields, from the top after the index: bits 30 and 31 the index of the
//!   component left out, bits 20 to 29, 10 to 19 and 0 to 9 the three others in the order of their components, each
//!   `(v / 1023 * 2 - 1) / sqrt(2)`; the left-out component is the positive root that makes it unit;
//! - kind 4: three u16 words, the low 15 bits of each a component, `(v / 32767 * 2 - 1) / sqrt(2)`, in the order of
//!   their components; the index of the one left out is bit 15 of the first word (high) and the second (low), bit 15 of
//!   the third its sign (0 on all 84,886 keys of the sample: a clip that sets it is refused);
//! - kind 7: a u16 whose low 15 bits are z of a rotation about z alone, the same scale, w the positive root (bit 15 is
//!   set on 9% of keys and makes no difference to the match: left unread);
//! - kind 5: a little-endian u64 of three 20-bit components and four more bits, on a ring: bits 8 to 27 the first
//!   component kept, bits 32 to 51 the third, bits 52 to 63 and then bits 0 to 7 (twenty bits, the low ones first) the
//!   second, bits 30 and 31 the index of the component left out, bits 28 and 29 zero on every key (a key that sets
//!   them is refused), the scale as above. Told by every key's three components having squares that sum to at most 1
//!   (all 53,647 keys of 600 sampled tracks: a layout with a wrong bit does not), the keys of a track moving
//!   smoothly (the median of the median dot of consecutive keys 0.999996), and first keys equal to the mapped bone's
//!   stored rotation (1 - |dot| < 1e-3 on 28% of 4,000 sampled).
//!
//! Kind 25 (8 bytes), 21 and 22 (4), 16, 17 and 18 (1 or 2 bytes) are not read.
//!
//! Where the `AnimTrackEvent` array isn't empty its elements' length isn't read: the `AnimTrackData` object's start
//! is found as the first place after the count where a pointer's header with the type 0x0181EFE8 is followed by
//! mappings of the type 0x653CAA76 for the count they state.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::fields::{BodyError, LE, count, expect};
use primase_foundations::bytes::Field;

/// Why an Animation body or a track's values can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("animation body: {0}")]
    Short(#[from] primase_foundations::bytes::FieldError),
    #[error("animation {what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("animation reference at {at}: {why}")]
    Reference { at: usize, why: String },
    #[error("animation track {track}: tag {tag:#04x} is kind {kind}, whose value width isn't read")]
    Kind { track: usize, tag: u8, kind: u8 },
    #[error("animation track {track}: {why}")]
    Track { track: usize, why: String },
    #[error("animation tracks of kind {kind}: their values aren't read")]
    Unread { kind: u8 },
    #[error("animation track values: {0}")]
    Values(String),
    #[error("animation body: {0} bytes after its last track")]
    Trailing(usize),
    #[error("animation events: no AnimTrackData after the {0} events")]
    NoTrackData(u32),
}

/// An Animation resource's body as stored ([`crate::animation`] for what each field is).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredAnimation {
    /// `Length` (f32).
    pub length: f32,
    /// `F4F597FF`: varies by clip.
    pub id_hash: u32,
    /// `322B96B8`: 0 on every clip.
    pub f2: u32,
    /// `556E9166` and `251905D9`: empty on every clip.
    pub arrays: [Vec<u16>; 2],
    /// The four booleans (`1F4DA15F`, `93847F6A`, `ABC2D4B8`, `0D7D1D50`).
    pub flags: [u8; 4],
    pub events: StoredEvents,
    /// The `AnimTrackData` object's id (0 on every clip) and its `567530C5`.
    pub track_data_id: u64,
    pub track_data_hash: u32,
    /// Each track's bone: the CRC-32 of its name (0 for the root's), the mappings in the tracks' order.
    pub mappings: Vec<u32>,
    /// `66A6DC91`.
    pub track_data_tail: u16,
    pub ecl: Option<StoredEcl>,
    /// `B832D26F`, a byte buffer (empty on every clip).
    pub buffer: Vec<u8>,
    /// `CB4E5F18`.
    pub cb4e: u32,
    pub tracks: Vec<StoredTrack>,
}

/// An Animation's `TTECLAnimation` object, its four arrays as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEcl {
    pub id: u64,
    pub animation: Vec<u8>,
    pub words: Vec<u32>,
    pub halves: Vec<u16>,
    pub bytes: Vec<u8>,
}

/// An Animation's `AnimTrackEvent` array as stored: the count and the elements' bytes (not read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvents {
    pub count: u32,
    pub bytes: Vec<u8>,
}

/// One track of an Animation as stored: its block's tag (`3 * kind + w`), its size field, the key count, the times and
/// the values (`keys` of the kind's width, as bytes). The bone it drives is the clip's mapping at its position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTrack {
    pub tag: u8,
    /// The block's `a` field, its in-memory size (within 8 of the stored payload's length on every clip).
    pub size: u32,
    pub keys: u32,
    pub times: Times,
    pub values: Vec<u8>,
}

/// How a track's key times are stored ([`crate::animation`]: the tag's remainder over 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Times {
    /// A u8 for each key after the first (the first is at 0).
    U8(Vec<u8>),
    /// A u16 for each key after the first.
    U16(Vec<u16>),
    /// A bit mask of the frames that hold a key, with its marker byte's top two bits (`mode`: 1, 2 or 3).
    Mask { mode: u8, mask: Vec<u8> },
}

/// A track's values decoded ([`StoredTrack::values`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    /// Kind 9: translations.
    Vec3(Vec<[f32; 3]>),
    /// Kinds 3, 4, 5, 7 and 8: unit quaternions `[x, y, z, w]`, the component the format leaves out the positive root.
    Quaternions(Vec<[f32; 4]>),
}

/// The grammar's refusals as the animation error names them.
impl From<BodyError> for Error {
    fn from(e: BodyError) -> Self {
        match e {
            BodyError::Short(f) => Error::Short(f),
            BodyError::Value {
                what,
                at,
                found,
                read,
            } => Error::Value {
                what,
                at,
                found,
                read,
            },
            BodyError::Reference { at, source } => Error::Reference {
                at,
                why: source.to_string(),
            },
            BodyError::Trailing(n) => Error::Trailing(n),
        }
    }
}

/// A record type: the CRC-32 of `Animation`.
pub const ANIMATION: u32 = 0x0FA3_067F;
/// An `AnimTrackData`'s type, an `AnimTrackDataMapping`'s, an `AnimTrackEvent`'s and a `TTECLAnimation`'s.
const ANIM_TRACK_DATA: u32 = 0x0181_EFE8;
const MAPPING: u32 = 0x653C_AA76;
const TTECL_ANIMATION: u32 = 0xAC69_2A57;

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

/// A u8 or u16 key time's unit: 60 to the second.
pub const TIME_TICKS_PER_SECOND: f32 = 60.0;
/// A mask frame's unit: 30 to the second.
pub const MASK_FRAMES_PER_SECOND: f32 = 30.0;

/// The unit quaternion whose components other than `omitted` are `kept` (in the order of their indices) and whose
/// `omitted` one is the positive root that makes it unit.
fn smallest_three(omitted: u8, kept: [f32; 3]) -> [f32; 4] {
    let root = (1.0 - kept.iter().map(|c| c * c).sum::<f32>()).max(0.0).sqrt();
    let [a, b, c] = kept;
    match omitted {
        0 => [root, a, b, c],
        1 => [a, root, b, c],
        2 => [a, b, root, c],
        _ => [a, b, c, root],
    }
}

/// A packed component of `bits` bits: `(v / (2^bits - 1) * 2 - 1) / sqrt(2)`.
fn component(v: u16, max: f32) -> f32 {
    (f32::from(v) / max * 2.0 - 1.0) * std::f32::consts::FRAC_1_SQRT_2
}

impl StoredTrack {
    /// The key times in seconds, one for each key: the first key of a u8 or u16 track is at 0, the others at their
    /// stored time over [`TIME_TICKS_PER_SECOND`]; a mask's keys are at the frames of its set bits over
    /// [`MASK_FRAMES_PER_SECOND`].
    pub fn times(&self) -> Vec<f32> {
        match &self.times {
            Times::U8(t) => std::iter::once(0.0)
                .chain(t.iter().map(|&t| f32::from(t) / TIME_TICKS_PER_SECOND))
                .collect(),
            Times::U16(t) => std::iter::once(0.0)
                .chain(t.iter().map(|&t| f32::from(t) / TIME_TICKS_PER_SECOND))
                .collect(),
            Times::Mask { mask, .. } => mask
                .iter()
                .enumerate()
                .flat_map(|(i, byte)| (0..8usize).map(move |bit| (i, bit, *byte)))
                .filter(|&(_, bit, byte)| (byte >> bit) & 1 == 1)
                .filter_map(|(i, bit, _)| u16::try_from(i.saturating_mul(8).saturating_add(bit)).ok())
                .map(|frame| f32::from(frame) / MASK_FRAMES_PER_SECOND)
                .collect(),
        }
    }

    /// The values of the kinds read (see the module documentation); [`Error::Unread`] for another kind.
    pub fn values(&self) -> Result<Values, Error> {
        let keys = usize::try_from(self.keys).unwrap_or(usize::MAX);
        let counted = |n: usize| {
            if n == keys {
                Ok(())
            } else {
                Err(Error::Values(format!("{n} values for {keys} keys")))
            }
        };
        match self.kind() {
            9 => {
                let v = self.values.as_chunks::<12>().0;
                counted(v.len())?;
                Ok(Values::Vec3(
                    v.iter()
                        .map(|b| {
                            let [x0, x1, x2, x3, y0, y1, y2, y3, z0, z1, z2, z3] = *b;
                            [
                                f32::from_le_bytes([x0, x1, x2, x3]),
                                f32::from_le_bytes([y0, y1, y2, y3]),
                                f32::from_le_bytes([z0, z1, z2, z3]),
                            ]
                        })
                        .collect(),
                ))
            }
            8 => {
                let v = self.values.as_chunks::<12>().0;
                counted(v.len())?;
                Ok(Values::Quaternions(
                    v.iter()
                        .map(|b| {
                            let [x0, x1, x2, x3, y0, y1, y2, y3, z0, z1, z2, z3] = *b;
                            let kept = [
                                f32::from_le_bytes([x0, x1, x2, x3]),
                                f32::from_le_bytes([y0, y1, y2, y3]),
                                f32::from_le_bytes([z0, z1, z2, z3]),
                            ];
                            smallest_three(3, kept)
                        })
                        .collect(),
                ))
            }
            3 => {
                let v = self.values.as_chunks::<4>().0;
                counted(v.len())?;
                Ok(Values::Quaternions(
                    v.iter()
                        .map(|b| {
                            let u = u32::from_le_bytes(*b);
                            let field = |shift: u32| u16::try_from((u >> shift) & 1023).unwrap_or(0);
                            let kept = [20, 10, 0].map(|s| component(field(s), 1023.0));
                            smallest_three(u8::try_from(u >> 30).unwrap_or(3), kept)
                        })
                        .collect(),
                ))
            }
            4 => {
                let v = self.values.as_chunks::<6>().0;
                counted(v.len())?;
                v.iter()
                    .map(|b| {
                        let [a0, a1, b0, b1, c0, c1] = *b;
                        let words = [
                            u16::from_le_bytes([a0, a1]),
                            u16::from_le_bytes([b0, b1]),
                            u16::from_le_bytes([c0, c1]),
                        ];
                        let [w0, w1, w2] = words;
                        if w2 >> 15 != 0 {
                            return Err(Error::Values("a kind 4 value with its sign bit set".into()));
                        }
                        let omitted = u8::try_from(((w0 >> 15) << 1) | (w1 >> 15)).unwrap_or(3);
                        let kept = words.map(|w| component(w & 0x7FFF, 32767.0));
                        Ok(smallest_three(omitted, kept))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Values::Quaternions)
            }
            5 => {
                let v = self.values.as_chunks::<8>().0;
                counted(v.len())?;
                v.iter()
                    .map(|b| {
                        let u = u64::from_le_bytes(*b);
                        if (u >> 28) & 3 != 0 {
                            return Err(Error::Values("a kind 5 value with its spare bits set".into()));
                        }
                        let field = |v: u64| {
                            let v = u32::try_from(v & 0xF_FFFF).unwrap_or(0);
                            (v as f32 / 1_048_575.0 * 2.0 - 1.0) * std::f32::consts::FRAC_1_SQRT_2
                        };
                        let first = field(u >> 8);
                        let third = field(u >> 32);
                        let second = field((u >> 52) | ((u & 0xFF) << 12));
                        let omitted = u8::try_from((u >> 30) & 3).unwrap_or(3);
                        Ok(smallest_three(omitted, [first, second, third]))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Values::Quaternions)
            }
            7 => {
                let v = self.values.as_chunks::<2>().0;
                counted(v.len())?;
                Ok(Values::Quaternions(
                    v.iter()
                        .map(|b| {
                            let z = component(u16::from_le_bytes(*b) & 0x7FFF, 32767.0);
                            [0.0, 0.0, z, (1.0 - z * z).max(0.0).sqrt()]
                        })
                        .collect(),
                ))
            }
            kind => Err(Error::Unread { kind }),
        }
    }

    /// The track class: the tag over 3.
    pub fn kind(&self) -> u8 {
        self.tag.checked_div(3).unwrap_or(0)
    }

    /// How its key times are stored: the tag's remainder over 3 (0 a u8 each, 1 a u16 each, 2 a mask).
    pub fn time_form(&self) -> u8 {
        self.tag.checked_rem(3).unwrap_or(0)
    }
}

/// The body's cursor ([`primase_foundations::bytes::Cursor`]), with this record's grammar as methods.
struct Cursor<'a>(primase_foundations::bytes::Cursor<'a>);

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cursor(primase_foundations::bytes::Cursor::new(b))
    }
}

impl<'a> std::ops::Deref for Cursor<'a> {
    type Target = primase_foundations::bytes::Cursor<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Cursor<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> Cursor<'a> {
    /// A counted array of `each`-byte items, as the bytes.
    fn counted(&mut self, what: &'static str, each: usize) -> Result<&'a [u8], Error> {
        let n = count(self, what, each)?;
        Ok(self.take(n.saturating_mul(each)).field(what)?)
    }
    /// Where an `AnimTrackData` pointer lies at or after the cursor: `00`, a u64, the type, a u32 and a count of
    /// mappings of the type [`MAPPING`] for as many as the count says.
    fn find_track_data(&self) -> Option<usize> {
        let needle = ANIM_TRACK_DATA.to_le_bytes();
        let mut from = self.position();
        loop {
            let rest = self.bytes().get(from..)?;
            let found = rest.windows(4).position(|w| w == needle)?.checked_add(from)?;
            from = found.checked_add(1)?;
            let Some(start) = found.checked_sub(9).filter(|&s| s >= self.position()) else {
                continue;
            };
            if self.bytes().get(start) != Some(&0) {
                continue;
            }
            let mut probe = primase_foundations::bytes::Cursor::new(self.bytes());
            probe.seek(found.checked_add(4)?).ok()?;
            let valid = (|| -> Result<bool, BodyError> {
                probe.u32(LE).field("track data")?;
                let n = count(&mut probe, "mapping count", 16)?;
                for _ in 0..n {
                    probe.u64(LE).field("mapping")?;
                    if probe.u32(LE).field("mapping type")? != MAPPING {
                        return Ok(false);
                    }
                    probe.u32(LE).field("mapping")?;
                }
                Ok(true)
            })();
            if valid.unwrap_or(false) {
                return Some(start);
            }
        }
    }
}

/// Whether a track of `kind` drives a joint: kind 9 a translation, kinds 3, 4, 5, 7 and 8 a rotation (kind 5's values
/// are decoded on their own; its first keys equal the mapped bones' rest rotations like the other rotation kinds').
/// The other kinds (16, 17, 18, 21, 22, 25) are not known to drive one (their values are 1 to 4 bytes, and kind 25's
/// are unverified), so they are not counted.
pub fn drives_a_joint(kind: u8) -> bool {
    matches!(kind, 3 | 4 | 5 | 7 | 8 | 9)
}

impl StoredAnimation {
    /// The bone hash of each track that drives a joint ([`drives_a_joint`]) of a bone (not the root's motion, hash 0),
    /// in the tracks' order.
    pub fn joint_tracks(&self) -> Vec<u32> {
        self.tracks
            .iter()
            .zip(&self.mappings)
            .filter(|(t, h)| **h != 0 && drives_a_joint(t.kind()))
            .map(|(_, &h)| h)
            .collect()
    }

    /// The body parsed, consumed to its last byte.
    pub fn parse(body: &[u8]) -> Result<StoredAnimation, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        let length = c.f32(LE).field("Length")?;
        let id_hash = c.u32(LE).field("F4F597FF")?;
        let f2 = c.u32(LE).field("322B96B8")?;
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
        let flags = c.array::<4>().field("booleans")?;
        let event_count = c.u32(LE).field("event count")?;
        let events = if event_count == 0 {
            StoredEvents {
                count: event_count,
                bytes: Vec::new(),
            }
        } else {
            let start = c.position();
            let at = c.find_track_data().ok_or(Error::NoTrackData(event_count))?;
            c.seek(at).field("AnimTrackData pointer")?;
            StoredEvents {
                count: event_count,
                bytes: body.get(start..at).unwrap_or_default().to_vec(),
            }
        };
        expect(&mut c, "AnimTrackData pointer", 0, 1)?;
        let track_data_id = c.u64(LE).field("AnimTrackData")?;
        expect(&mut c, "AnimTrackData type", u64::from(ANIM_TRACK_DATA), 4)?;
        let track_data_hash = c.u32(LE).field("567530C5")?;
        let n = count(&mut c, "mapping count", 16)?;
        let mut mappings = Vec::with_capacity(n);
        for _ in 0..n {
            c.u64(LE).field("mapping id")?;
            expect(&mut c, "mapping type", u64::from(MAPPING), 4)?;
            mappings.push(c.u32(LE).field("mapping")?);
        }
        let track_data_tail = c.u16(LE).field("66A6DC91")?;
        let at = c.position();
        let ecl = match c.u8().field("ECLAnimation pointer")? {
            3 => None,
            0 => {
                let id = c.u64(LE).field("TTECLAnimation")?;
                expect(&mut c, "TTECLAnimation type", u64::from(TTECL_ANIMATION), 4)?;
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
                return Err(BodyError::Value {
                    what: "ECLAnimation pointer",
                    at,
                    found: u64::from(found),
                    read: "3 (none) or 0 (an object)",
                }
                .into());
            }
        };
        let buffer = c.counted("B832D26F", 1)?.to_vec();
        let cb4e = c.u32(LE).field("CB4E5F18")?;
        let tracks_at = c.position();
        let tracks_n = usize::try_from(c.u32(LE).field("track count")?).unwrap_or(usize::MAX);
        if tracks_n != mappings.len() {
            return Err(BodyError::Value {
                what: "track count",
                at: tracks_at,
                found: u64::try_from(tracks_n).unwrap_or(u64::MAX),
                read: "the number of mappings",
            }
            .into());
        }
        let mut tracks = Vec::with_capacity(tracks_n);
        for track in 0..tracks_n {
            tracks.push(Self::track(&mut c, track)?);
        }
        match body.len().saturating_sub(c.position()) {
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
        let tag = c.u8().field("track tag")?;
        let a = c.u32(LE).field("track size")?;
        let keys = c.u32(LE).field("track key count")?;
        let kind = tag.checked_div(3).unwrap_or(0);
        let width = value_width(kind).ok_or(Error::Kind { track, tag, kind })?;
        if keys == 0 {
            return Err(why("no keys".into()));
        }
        let n = usize::try_from(keys).map_err(|_| why("key count past this machine".into()))?;
        let times = match tag.checked_rem(3).unwrap_or(0) {
            0 => Times::U8(c.take(n.saturating_sub(1)).field("u8 key times")?.to_vec()),
            1 => Times::U16(
                c.take(n.saturating_sub(1).saturating_mul(2))
                    .field("u16 key times")?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|w| u16::from_le_bytes(*w))
                    .collect(),
            ),
            _ => {
                let marker = c.u8().field("key mask marker")?;
                let bytes = usize::from(marker & 0x3F);
                if marker < 0x40 || !(1..=8).contains(&bytes) {
                    return Err(why(format!("key mask marker {marker:#04x}")));
                }
                let mask = c.take(bytes).field("key mask")?;
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
        let values = c.take(n.saturating_mul(width)).field("track values")?.to_vec();
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

    fn track(tag: u8, times: Times, values: &[u8]) -> StoredTrack {
        let width = value_width(tag / 3).expect("a kind read");
        StoredTrack {
            tag,
            size: 0,
            keys: u32::try_from(values.len().checked_div(width).expect("a width")).expect("count"),
            times,
            values: values.to_vec(),
        }
    }

    fn close(a: &[f32], b: &[f32], tolerance: f32) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    #[test]
    fn values_of_each_kind_read_decode_to_the_bones_stored_rotations() {
        // each fixture is the first key of a track against the stored local rotation of the bone it drives, where the
        // two agree (docs: the census of the clips)
        let kind =
            |tag: u8, bytes: &[u8]| match track(tag, Times::U8(vec![]), bytes).values().expect("values") {
                Values::Quaternions(q) => q.first().copied().expect("a key"),
                Values::Vec3(_) => panic!("vectors"),
            };
        // kind 4 (tag 12): the identity, and a rotation
        assert!(close(
            &kind(12, &[0xFF, 0xBF, 0xFF, 0xBF, 0xFF, 0x3F]),
            &[0.0, 0.0, 0.0, 1.0],
            1e-4
        ));
        assert!(close(
            &kind(12, &[0x21, 0xBB, 0xDD, 0xC0, 0xDF, 0x4F]),
            &[-0.053793, 0.009598, 0.175388, 0.982982],
            1e-4
        ));
        assert!(close(
            &kind(12, &[0xBF, 0x92, 0xBE, 0x92, 0xBE, 0x12]),
            &[-0.5, -0.5, -0.5, 0.5],
            1e-4
        ));
        // kind 3 (tag 9): a 10-bit rotation
        assert!(close(
            &kind(9, &[0x01, 0x3A, 0xA6, 0xD8]),
            &[-0.16247, -0.156476, 0.003176, 0.974222],
            2e-3
        ));
        // kind 5 (tag 15): the identity, and a rotation the stored rest of its bone gives to 1e-4
        assert!(close(
            &kind(15, &[0x7F, 0xFF, 0xFF, 0xC7, 0xFF, 0xFF, 0xF7, 0xFF]),
            &[0.0, 0.0, 0.0, 1.0],
            1e-4
        ));
        assert!(close(
            &kind(15, &[0x7F, 0x67, 0xFD, 0xC7, 0x79, 0xF2, 0xC9, 0xFA]),
            &[-0.00085, -0.00009, 0.17211, 0.98508],
            2e-4
        ));
        // kind 7 (tag 21): a rotation about z
        assert!(close(
            &kind(21, &[0x4A, 0x36]),
            &[0.0, 0.0, -0.107253, 0.994232],
            1e-4
        ));
        // kind 8 (tag 24): x, y, z as f32
        let half: Vec<u8> = [0.5f32, 0.5, 0.5].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert!(close(&kind(24, &half), &[0.5, 0.5, 0.5, 0.5], 1e-6));
        // kind 9 (tag 27): a translation
        let t: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            track(27, Times::U8(vec![]), &t).values().expect("values"),
            Values::Vec3(vec![[1.0, 2.0, 3.0]])
        );
        // a kind not read, and a sign bit set
        assert!(matches!(
            track(75, Times::U8(vec![]), &[0; 8]).values(),
            Err(Error::Unread { kind: 25 })
        ));
        // kind 5's spare bits (28 and 29) must be clear
        assert!(
            track(15, Times::U8(vec![]), &[0, 0, 0, 0x10, 0, 0, 0, 0])
                .values()
                .is_err()
        );
        assert!(
            track(12, Times::U8(vec![]), &[0, 0, 0, 0, 0, 0x80])
                .values()
                .is_err()
        );
    }

    #[test]
    fn key_times_are_sixtieths_of_a_second_or_frames_of_a_thirtieth() {
        let u8s = track(27, Times::U8(vec![30, 60]), &[0; 36]);
        assert_eq!(u8s.times(), [0.0, 0.5, 1.0]);
        let u16s = track(28, Times::U16(vec![120]), &[0; 24]);
        assert_eq!(u16s.times(), [0.0, 2.0]);
        // frames 0, 2 and 9
        let mask = track(
            29,
            Times::Mask {
                mode: 1,
                mask: vec![0b0000_0101, 0b0000_0010],
            },
            &[0; 36],
        );
        assert_eq!(mask.times(), [0.0, 2.0 / 30.0, 9.0 / 30.0]);
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
