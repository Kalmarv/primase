//! The channel data of a clip dictionary's sequences ([`Sequence`]): decoded into the values each bone track takes on
//! each frame of the sequence's span.
//!
//! A sequence's bytes hold, in this order (offsets from the start of the data):
//! - **Channel parameters**, from byte 0 to the sequence's frame offset: for every channel, in the order of its type
//!   (0 to 8) and then of its list, what the type needs to decode it.
//! - **Frame data**, `frames` items of `frame_length` bytes: item `f` holds, in the same channel order, the bits the
//!   channels that vary a frame take (least significant bit first).
//! - **Channel lists**: nine `u16` channel counts (one per type), then for each type its channels' words, padded to a
//!   multiple of four words. A word is `track << 2 | index`: the bone track (of the animation's bone ids) the channel
//!   belongs to, and the channel's place in the track (0 to 3, the component it supplies).
//! - **Root motion references** (six bytes each: the counts are in the sequence's `root_motion_ref_counts`, position
//!   references in the high nibble, rotation in the low), kept as a count.
//!
//! The nine channel types and their parameters:
//!
//! | type | meaning | parameters | frame bits |
//! |---|---|---|---|
//! | 0 static quaternion | x, y, z of a unit quaternion, w the root of what they leave of 1 | 3 floats | 0 |
//! | 1 static vector | three floats | 3 floats | 0 |
//! | 2 static float | one float | 1 float | 0 |
//! | 3 raw float | a float a frame | none | 32 |
//! | 4 quantised float | `bits * quantum + offset` a frame | bits (i32), quantum, offset | `bits` |
//! | 5 indirect quantised | a frame's index into a table of `bits * quantum + offset` | frame bits, value bits, ints, quantum, offset, then the table's `ints` words | frame bits |
//! | 6 linear | `value * quantum + offset`, a value a frame by second-order deltas in chunks (below) | ints (header included), counts, quantum, offset, then bit fields | 0 |
//! | 7 cached quaternion (1) | the component of a quaternion a track does not store, w-like: the root of what the track's first three channels leave of 1 | none | 0 |
//! | 8 cached quaternion (2) | as 7, in a track that stores all four components too | none | 0 |
//!
//! A linear channel's bit fields: per chunk of `chunk_size` frames (the sequence's) a start offset (`counts & 0xFF`
//! bits, from the end of the start values), then per chunk a start value (`counts >> 8 & 0xFF` bits), then each
//! chunk's deltas: per frame after the first of a chunk, a delta of `counts >> 16 & 0xFF` low bits, then the high part
//! as the number of 0 bits before a 1, then (when the delta is not zero) a sign bit (1: negative). A frame's value is
//! the last plus a running increment, to which each frame's delta is added.
//!
//! A track's value at a frame ([`Track::sample`]) is its channels' values in order of index, up to four components
//! (a static vector supplies three, a static quaternion four); a track with a type 7 channel at index 3 stores three
//! components of a quaternion, the cached channel supplies the fourth at the place its word's index names, and the
//! result is normalised.
//!
//! The layout is CodeWalker's (its `Sequence.ParseData` and `AnimChannel` classes, GPL-licensed: read as a description
//! of the data and run as the oracle the tests compare with, not copied); this decoder is written from that description
//! and held to the file: every offset must land inside the sequence's data, the parameter bytes must end where the
//! frame data begins, the channel lists where the root motion references begin, and the references at the data's end.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::le;
use crate::le::LE;
use crate::ycd::{Animation, Sequence};
use crate::{Result, invalid};
use primase_foundations::bytes::View;

fn err(at: usize, detail: impl ToString) -> crate::Error {
    invalid("ycd channels", u64::try_from(at).unwrap_or(u64::MAX), detail)
}

fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b).ok_or_else(|| err(a, "an offset overflows"))
}

fn mul(a: usize, b: usize) -> Result<usize> {
    a.checked_mul(b).ok_or_else(|| err(a, "an offset overflows"))
}

fn as_usize(v: u32) -> Result<usize> {
    usize::try_from(v).map_err(|_| err(0, "a value that does not fit an offset"))
}

/// The nine channel types, in the order the lists are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    StaticQuaternion,
    StaticVector3,
    StaticFloat,
    RawFloat,
    QuantizeFloat,
    IndirectQuantizeFloat,
    LinearFloat,
    CachedQuaternion1,
    CachedQuaternion2,
}

const KINDS: [ChannelKind; 9] = [
    ChannelKind::StaticQuaternion,
    ChannelKind::StaticVector3,
    ChannelKind::StaticFloat,
    ChannelKind::RawFloat,
    ChannelKind::QuantizeFloat,
    ChannelKind::IndirectQuantizeFloat,
    ChannelKind::LinearFloat,
    ChannelKind::CachedQuaternion1,
    ChannelKind::CachedQuaternion2,
];

impl ChannelKind {
    /// The type number the lists are ordered by (0 to 8).
    pub fn code(self) -> u8 {
        match self {
            ChannelKind::StaticQuaternion => 0,
            ChannelKind::StaticVector3 => 1,
            ChannelKind::StaticFloat => 2,
            ChannelKind::RawFloat => 3,
            ChannelKind::QuantizeFloat => 4,
            ChannelKind::IndirectQuantizeFloat => 5,
            ChannelKind::LinearFloat => 6,
            ChannelKind::CachedQuaternion1 => 7,
            ChannelKind::CachedQuaternion2 => 8,
        }
    }
}

/// One channel of a track.
#[derive(Clone, Debug, PartialEq)]
pub enum Channel {
    /// A quaternion's x, y, z as stored; w is [`root_of_rest`] of them.
    StaticQuaternion([f32; 3]),
    StaticVector3([f32; 3]),
    StaticFloat(f32),
    /// A float a frame of the sequence's span, decoded from a raw, quantised, indirect or linear channel (`kind`).
    Frames {
        kind: ChannelKind,
        values: Vec<f32>,
    },
    /// The component (0 to 3: x, y, z, w) of a quaternion the track derives from its first three channels; `kind` is
    /// [`ChannelKind::CachedQuaternion1`] or [`ChannelKind::CachedQuaternion2`].
    Cached {
        kind: ChannelKind,
        component: u8,
    },
}

/// `sqrt(max(1 - x² - y² - z², 0))`: the component a unit quaternion's other three leave.
pub fn root_of_rest(v: [f32; 3]) -> f32 {
    let [x, y, z] = v;
    (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt()
}

impl Channel {
    /// The channel's type.
    pub fn kind(&self) -> ChannelKind {
        match self {
            Channel::StaticQuaternion(_) => ChannelKind::StaticQuaternion,
            Channel::StaticVector3(_) => ChannelKind::StaticVector3,
            Channel::StaticFloat(_) => ChannelKind::StaticFloat,
            Channel::Frames { kind, .. } | Channel::Cached { kind, .. } => *kind,
        }
    }

    /// The value of a channel that supplies one float, at `frame`.
    fn scalar(&self, frame: usize) -> Option<f32> {
        match self {
            Channel::StaticFloat(v) => Some(*v),
            Channel::Frames { values, .. } => values.get(frame).copied(),
            _ => None,
        }
    }
}

/// The channels of one bone track of a sequence, by index.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub channels: Vec<Channel>,
}

impl Track {
    /// A quaternion stored as three components and a cached one: the cached channel (index 3) and its component.
    fn cached_quaternion(&self) -> Option<u8> {
        match self.channels.get(3) {
            Some(Channel::Cached {
                kind: ChannelKind::CachedQuaternion1,
                component,
            }) => Some(*component),
            _ => None,
        }
    }

    /// How many components [`Track::sample`] returns (at most four): a quaternion stored as three components and a
    /// cached one has four, otherwise the channels' own (a static vector three, a static quaternion four, a
    /// cached component of the second kind none).
    pub fn width(&self) -> usize {
        if self.cached_quaternion().is_some() {
            return 4;
        }
        self.channels
            .iter()
            .map(|c| match c {
                Channel::StaticQuaternion(_) => 4,
                Channel::StaticVector3(_) => 3,
                Channel::Cached { .. } => 0,
                _ => 1,
            })
            .fold(0usize, usize::saturating_add)
            .min(4)
    }

    /// The track's value at `frame` of the sequence's span: [`Track::width`] components in an array of four (the rest
    /// zero). A quaternion is in x, y, z, w order and normalised.
    pub fn sample(&self, frame: usize) -> Result<[f32; 4]> {
        let scalar = |i: usize| {
            self.channels.get(i).and_then(|c| c.scalar(frame)).ok_or_else(|| {
                err(
                    frame,
                    format!("channel {i} of a track has no value at frame {frame}"),
                )
            })
        };
        if let Some(component) = self.cached_quaternion() {
            let stored = [scalar(0)?, scalar(1)?, scalar(2)?];
            let mut q = [0.0f32; 4];
            let mut rest = stored.iter();
            for (i, slot) in q.iter_mut().enumerate() {
                *slot = if i == usize::from(component) {
                    root_of_rest(stored)
                } else {
                    rest.next().copied().unwrap_or(0.0)
                };
            }
            let [x, y, z, w] = q;
            let length = (x * x + y * y + z * z + w * w).sqrt();
            if length > 1e-6 {
                let inverse = 1.0 / length;
                return Ok([x * inverse, y * inverse, z * inverse, w * inverse]);
            }
            return Ok(q);
        }
        let mut out = [0.0f32; 4];
        let mut n = 0usize;
        for (i, c) in self.channels.iter().enumerate() {
            // the values the channel supplies, at most four, without an allocation (this runs for every frame)
            let (part, count): ([f32; 4], usize) = match c {
                Channel::StaticQuaternion(v) => ([v[0], v[1], v[2], root_of_rest(*v)], 4),
                Channel::StaticVector3(v) => ([v[0], v[1], v[2], 0.0], 3),
                Channel::Cached { .. } => {
                    if n < 4 {
                        return Err(err(
                            i,
                            "a cached component in a track of fewer than four stored ones",
                        ));
                    }
                    ([0.0; 4], 0)
                }
                _ => ([scalar(i)?, 0.0, 0.0, 0.0], 1),
            };
            for v in part.into_iter().take(count) {
                if let Some(slot) = out.get_mut(n) {
                    *slot = v;
                    n = n.saturating_add(1);
                }
            }
        }
        Ok(out)
    }
}

/// A sequence decoded: its span of frames and, per bone track of the animation, the track (none when the sequence
/// holds no channel for it).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub frames: usize,
    pub tracks: Vec<Option<Track>>,
    /// How many root motion references the sequence ends with (not decoded).
    pub root_motion_refs: usize,
}

/// `n` (at most 32) bits of `data` from bit `start`, the first bit the least significant.
fn bits(data: &[u8], start: usize, n: u32) -> Result<u32> {
    let end = add(start, as_usize(n)?)?;
    if end > mul(data.len(), 8)? {
        return Err(err(
            start / 8,
            format!("{n} bits at bit {start}, past the {} bytes there are", data.len()),
        ));
    }
    if n == 0 {
        return Ok(0);
    }
    let mut buf = [0u8; 8];
    for (d, s) in buf.iter_mut().zip(data.get(start / 8..).unwrap_or_default()) {
        *d = *s;
    }
    let shift = u32::try_from(start % 8).unwrap_or(0);
    let mask = u64::MAX.checked_shr(64u32.saturating_sub(n)).unwrap_or(0);
    let v = u64::from_le_bytes(buf).checked_shr(shift).unwrap_or(0) & mask;
    u32::try_from(v).map_err(|_| err(start / 8, "more than 32 bits"))
}

/// A stream of bits read in order.
struct BitStream<'a> {
    data: &'a [u8],
    at: usize,
}

impl BitStream<'_> {
    fn read(&mut self, n: u32) -> Result<u32> {
        let v = bits(self.data, self.at, n)?;
        self.at = add(self.at, as_usize(n)?)?;
        Ok(v)
    }
}

/// What a channel's parameters and frame bits decode from, kept until the frame data is read.
enum Body {
    Done(Channel),
    Raw,
    Quantize { bits: u32, quantum: f32, offset: f32 },
    Indirect { frame_bits: u32, table: Vec<f32> },
}

struct Pending {
    kind: ChannelKind,
    track: usize,
    index: usize,
    body: Body,
}

impl Pending {
    /// The bits the channel takes in a frame.
    fn frame_bits(&self) -> u32 {
        match &self.body {
            Body::Raw => 32,
            Body::Quantize { bits, .. } => *bits,
            Body::Indirect { frame_bits, .. } => *frame_bits,
            Body::Done(_) => 0,
        }
    }
}

fn i32_at(data: &[u8], at: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(View::new(data).u32(at, LE)?.to_le_bytes()))
}

/// A bit count a file stores as an `i32`: within `1..=32`.
fn count_bits(v: i32, at: usize, what: &str) -> Result<u32> {
    u32::try_from(v)
        .ok()
        .filter(|b| (1..=32).contains(b))
        .ok_or_else(|| err(at, format!("{what} of {v} bits")))
}

impl Sequence {
    /// The sequence's channels decoded, for an animation of `tracks` bone tracks (see the module documentation).
    pub fn decode(&self, tracks: usize) -> Result<Block> {
        let data = self.data.as_slice();
        let frames = usize::from(self.frames);
        let frame_offset = as_usize(self.frame_offset)?;
        let frame_length = usize::from(self.frame_length);
        let list_at = add(frame_offset, mul(frame_length, frames)?)?;
        let mut info_at = add(list_at, 18)?;
        let mut at = 0usize;
        let mut pending: Vec<Pending> = Vec::new();
        for (type_index, kind) in KINDS.iter().copied().enumerate() {
            let count = usize::from(View::new(data).u16(add(list_at, mul(type_index, 2)?)?, LE)?);
            for _ in 0..count {
                let word = View::new(data).u16(info_at, LE)?;
                info_at = add(info_at, 2)?;
                let track = usize::from(word >> 2);
                let mut index = usize::from(word & 3);
                let body = match kind {
                    ChannelKind::StaticQuaternion => {
                        let v = le::vec3_of(&View::new(data).array::<12>(at)?);
                        at = add(at, 12)?;
                        Body::Done(Channel::StaticQuaternion(v))
                    }
                    ChannelKind::StaticVector3 => {
                        let v = le::vec3_of(&View::new(data).array::<12>(at)?);
                        at = add(at, 12)?;
                        Body::Done(Channel::StaticVector3(v))
                    }
                    ChannelKind::StaticFloat => {
                        let v = View::new(data).f32(at, LE)?;
                        at = add(at, 4)?;
                        Body::Done(Channel::StaticFloat(v))
                    }
                    ChannelKind::RawFloat => Body::Raw,
                    ChannelKind::QuantizeFloat => {
                        let b = count_bits(i32_at(data, at)?, at, "a quantised channel")?;
                        let quantum = View::new(data).f32(add(at, 4)?, LE)?;
                        let offset = View::new(data).f32(add(at, 8)?, LE)?;
                        at = add(at, 12)?;
                        Body::Quantize {
                            bits: b,
                            quantum,
                            offset,
                        }
                    }
                    ChannelKind::IndirectQuantizeFloat => {
                        let (table, frame_bits, used) = self.indirect(at)?;
                        at = add(at, used)?;
                        Body::Indirect { frame_bits, table }
                    }
                    ChannelKind::LinearFloat => {
                        let (values, used) = self.linear(at)?;
                        at = add(at, used)?;
                        Body::Done(Channel::Frames {
                            kind: ChannelKind::LinearFloat,
                            values,
                        })
                    }
                    ChannelKind::CachedQuaternion1 | ChannelKind::CachedQuaternion2 => {
                        let component = u8::try_from(index).unwrap_or(0);
                        index = if kind == ChannelKind::CachedQuaternion1 {
                            3
                        } else {
                            4
                        };
                        Body::Done(Channel::Cached { kind, component })
                    }
                };
                pending.push(Pending {
                    kind,
                    track,
                    index,
                    body,
                });
            }
            let rest = count % 4;
            if rest > 0 {
                info_at = add(info_at, mul(4usize.saturating_sub(rest), 2)?)?;
            }
        }
        if at != frame_offset {
            return Err(err(
                at,
                format!("channel parameters end at byte {at}, frame data begins at {frame_offset}"),
            ));
        }
        let refs = usize::from(self.root_motion_ref_counts >> 4)
            .saturating_add(usize::from(self.root_motion_ref_counts & 0xF));
        let root_motion = as_usize(self.root_motion_refs_offset)?.saturating_sub(0x20);
        if info_at != root_motion || add(info_at, mul(refs, 6)?)? != data.len() {
            return Err(err(
                info_at,
                format!(
                    "channel lists end at byte {info_at}, {refs} root motion references at {root_motion}, of {} bytes",
                    data.len()
                ),
            ));
        }

        // the frame data
        let frame_bits = pending
            .iter()
            .map(|p| as_usize(p.frame_bits()))
            .try_fold(0usize, |sum, b| add(sum, b?))?;
        if frame_bits > mul(frame_length, 8)? {
            return Err(err(
                frame_offset,
                format!("{frame_bits} bits a frame in items of {frame_length} bytes"),
            ));
        }
        // room for the frames of the channels that have any (a static or cached channel stores none per frame)
        let mut values: Vec<Vec<f32>> = (pending.iter())
            .map(|p| Vec::with_capacity(if p.frame_bits() > 0 { frames } else { 0 }))
            .collect();
        for f in 0..frames {
            let mut stream = BitStream {
                data,
                at: mul(add(frame_offset, mul(frame_length, f)?)?, 8)?,
            };
            for (p, out) in pending.iter().zip(values.iter_mut()) {
                match &p.body {
                    Body::Raw => out.push(f32::from_bits(stream.read(32)?)),
                    Body::Quantize {
                        bits: b,
                        quantum,
                        offset,
                    } => {
                        let q = stream.read(*b)?;
                        out.push(q as f32 * quantum + offset);
                    }
                    Body::Indirect { frame_bits: b, table } => {
                        let i = stream.read(*b)?;
                        let v = table.get(as_usize(i)?).ok_or_else(|| {
                            err(
                                f,
                                format!(
                                    "frame {f} takes value {i} of an indirect table of {}",
                                    table.len()
                                ),
                            )
                        })?;
                        out.push(*v);
                    }
                    Body::Done(_) => {}
                }
            }
        }

        // the tracks
        let mut by_track: Vec<Vec<(usize, Channel)>> = vec![Vec::new(); tracks];
        for (p, v) in pending.into_iter().zip(values) {
            let channel = match p.body {
                Body::Done(c) => c,
                Body::Raw | Body::Quantize { .. } | Body::Indirect { .. } => Channel::Frames {
                    kind: p.kind,
                    values: v,
                },
            };
            by_track
                .get_mut(p.track)
                .ok_or_else(|| {
                    err(
                        p.track,
                        format!("a channel of track {p}, the animation has {tracks}", p = p.track),
                    )
                })?
                .push((p.index, channel));
        }
        let mut out = Vec::with_capacity(tracks);
        for (t, mut channels) in by_track.into_iter().enumerate() {
            if channels.is_empty() {
                out.push(None);
                continue;
            }
            channels.sort_by_key(|(i, _)| *i);
            for (expect, (i, _)) in channels.iter().enumerate() {
                if *i != expect {
                    return Err(err(
                        t,
                        format!("track {t}'s channels have indices that are not 0, 1, 2..."),
                    ));
                }
            }
            out.push(Some(Track {
                channels: channels.into_iter().map(|(_, c)| c).collect(),
            }));
        }
        Ok(Block {
            frames,
            tracks: out,
            root_motion_refs: refs,
        })
    }

    /// An indirect quantised channel's table at parameter byte `at`, the bits of a frame's index into it and the
    /// parameter bytes the channel takes.
    fn indirect(&self, at: usize) -> Result<(Vec<f32>, u32, usize)> {
        let data = self.data.as_slice();
        let frame_bits = count_bits(i32_at(data, at)?, at, "an indirect channel's frames")?;
        let value_bits = count_bits(i32_at(data, add(at, 4)?)?, at, "an indirect channel's values")?;
        let ints =
            usize::try_from(i32_at(data, add(at, 8)?)?).map_err(|_| err(at, "a negative table length"))?;
        let quantum = View::new(data).f32(add(at, 12)?, LE)?;
        let offset = View::new(data).f32(add(at, 16)?, LE)?;
        let table_at = add(at, 20)?;
        let used = add(20, mul(ints, 4)?)?;
        if add(at, used)? > self.frame_offset_usize()? {
            return Err(err(at, "an indirect table past the parameter bytes"));
        }
        // the table holds as many values as its words do, or as the frame indices can name
        let by_words = mul(ints, 32)?.checked_div(as_usize(value_bits)?).unwrap_or(0);
        let by_index = (1usize.checked_shl(frame_bits).unwrap_or(usize::MAX)).saturating_sub(1);
        let n = by_words.min(by_index);
        let mut stream = BitStream {
            data,
            at: mul(table_at, 8)?,
        };
        let mut table = Vec::with_capacity(n);
        for _ in 0..n {
            table.push(stream.read(value_bits)? as f32 * quantum + offset);
        }
        Ok((table, frame_bits, used))
    }

    /// A linear channel's frame values at parameter byte `at`: the values and the parameter bytes it takes.
    fn linear(&self, at: usize) -> Result<(Vec<f32>, usize)> {
        let data = self.data.as_slice();
        let ints = as_usize(View::new(data).u32(at, LE)?)?;
        let counts = View::new(data).u32(add(at, 4)?, LE)?;
        let quantum = View::new(data).f32(add(at, 8)?, LE)?;
        let offset = View::new(data).f32(add(at, 12)?, LE)?;
        let used = mul(ints, 4)?;
        if used < 16 || add(at, used)? > self.frame_offset_usize()? {
            return Err(err(at, format!("a linear channel of {ints} words")));
        }
        let (offset_bits, value_bits, delta_bits) =
            (counts & 0xFF, (counts >> 8) & 0xFF, (counts >> 16) & 0xFF);
        let frames = usize::from(self.frames);
        let chunk_size = usize::from(self.chunk_size);
        if chunk_size == 0 {
            return Err(err(at, "a linear channel in a sequence without chunks"));
        }
        let chunks = add(frames, chunk_size.saturating_sub(1))?
            .checked_div(chunk_size)
            .unwrap_or(0);
        let start = mul(add(at, 16)?, 8)?;
        let mut header = BitStream { data, at: start };
        let mut starts = Vec::with_capacity(chunks);
        for _ in 0..chunks {
            starts.push(as_usize(header.read(offset_bits)?)?);
        }
        let mut firsts = Vec::with_capacity(chunks);
        for _ in 0..chunks {
            firsts.push(i64::from(header.read(value_bits)?));
        }
        let deltas = add(
            start,
            mul(chunks, add(as_usize(offset_bits)?, as_usize(value_bits)?)?)?,
        )?;
        let mut values = Vec::with_capacity(frames);
        for (c, (&begin, &first)) in starts.iter().zip(&firsts).enumerate() {
            let mut stream = BitStream {
                data,
                at: add(deltas, begin)?,
            };
            let mut value = first;
            let mut increment = 0i64;
            for j in 0..chunk_size {
                let frame = add(mul(c, chunk_size)?, j)?;
                if frame >= frames {
                    break;
                }
                values.push(value as f32 * quantum + offset);
                if add(j, 1)? >= chunk_size || add(frame, 1)? >= frames {
                    break;
                }
                let mut delta = i64::from(stream.read(delta_bits)?);
                let mut zeros = 0u32;
                while stream.read(1)? == 0 {
                    zeros = zeros.saturating_add(1);
                    if zeros > 64 {
                        return Err(err(at, "a delta's high part does not end"));
                    }
                }
                delta |= i64::from(zeros)
                    .checked_shl(delta_bits)
                    .ok_or_else(|| err(at, "a delta overflows"))?;
                if delta != 0 && stream.read(1)? == 1 {
                    delta = delta.saturating_neg();
                }
                increment = increment
                    .checked_add(delta)
                    .ok_or_else(|| err(at, "an increment overflows"))?;
                value = value
                    .checked_add(increment)
                    .ok_or_else(|| err(at, "a value overflows"))?;
            }
        }
        Ok((values, used))
    }

    fn frame_offset_usize(&self) -> Result<usize> {
        as_usize(self.frame_offset)
    }
}

impl Animation {
    /// Every sequence of the animation decoded, for the animation's bone tracks.
    pub fn decode(&self) -> Result<Vec<Block>> {
        self.sequences
            .iter()
            .map(|s| s.decode(self.bone_ids.len()))
            .collect()
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

    fn words(w: &[u16]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A sequence of `frames` frames whose bytes are `params`, `frame_data`, then the channel lists: `counts` (per type)
    /// and one word per channel (`infos`, each type's padded to a multiple of four words by the caller).
    fn sequence(
        params: &[u8],
        frame_data: &[u8],
        frame_length: u16,
        frames: u16,
        counts: [u16; 9],
        infos: &[u16],
    ) -> Sequence {
        let mut data = params.to_vec();
        data.extend_from_slice(frame_data);
        data.extend(words(&counts));
        data.extend(words(infos));
        Sequence {
            id: 0,
            frame_offset: params.len() as u32,
            root_motion_refs_offset: data.len() as u32 + 0x20,
            frames,
            frame_length,
            indirect_quantize_ints: 0,
            quantize_value_bits: 0,
            chunk_size: 64,
            root_motion_ref_counts: 0,
            data,
        }
    }

    #[test]
    fn a_static_vector_is_a_track_of_three_values() {
        let params: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        let mut counts = [0u16; 9];
        counts[1] = 1;
        // the channel's word: track 0, index 0; padded to four words
        let s = sequence(&params, &[], 0, 2, counts, &[0, 0, 0, 0]);
        let block = s.decode(1).unwrap();
        assert_eq!(block.frames, 2);
        let track = block.tracks[0].as_ref().unwrap();
        assert_eq!(track.channels, [Channel::StaticVector3([1.0, 2.0, 3.0])]);
        assert_eq!(track.width(), 3);
        assert_eq!(track.sample(1).unwrap(), [1.0, 2.0, 3.0, 0.0]);
    }

    #[test]
    fn a_quantised_channel_takes_its_bits_from_each_frame() {
        let mut params = Vec::new();
        params.extend(4i32.to_le_bytes());
        params.extend(0.5f32.to_le_bytes());
        params.extend(1.0f32.to_le_bytes());
        // three frames of four bytes: the low four bits hold 1, 2, 3
        let frames = [1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0];
        let mut counts = [0u16; 9];
        counts[4] = 1;
        let s = sequence(&params, &frames, 4, 3, counts, &[0, 0, 0, 0]);
        let block = s.decode(1).unwrap();
        let track = block.tracks[0].as_ref().unwrap();
        assert_eq!(
            track.channels,
            [Channel::Frames {
                kind: ChannelKind::QuantizeFloat,
                values: vec![1.5, 2.0, 2.5]
            }]
        );
    }

    #[test]
    fn a_sequence_that_does_not_end_where_it_says_is_refused() {
        let params: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        let mut counts = [0u16; 9];
        counts[1] = 1;
        let mut s = sequence(&params, &[], 0, 2, counts, &[0, 0, 0, 0]);
        s.frame_offset += 4;
        assert!(s.decode(1).is_err());
        let mut s = sequence(&params, &[], 0, 2, counts, &[0, 0, 0, 0]);
        s.data.truncate(20);
        assert!(s.decode(1).is_err());
        // a channel of a track the animation does not have
        let s = sequence(&params, &[], 0, 2, counts, &[8, 0, 0, 0]);
        assert!(s.decode(1).is_err());
    }

    #[test]
    fn bits_are_read_least_significant_first() {
        let d = [0b1010_0110u8, 0b0000_0011];
        assert_eq!(bits(&d, 0, 3).unwrap(), 0b110);
        assert_eq!(bits(&d, 4, 6).unwrap(), 0b11_1010);
        assert_eq!(bits(&d, 0, 16).unwrap(), 0x03A6);
        assert!(bits(&d, 10, 7).is_err());
    }
}
