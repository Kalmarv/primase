//! TAE files (`.tae`): the time-action event tables of a character's animation binder. One file holds the animations of
//! a character by number (`3000`, `20010`) and, for each, the timed events the game runs while it plays: a type, a start
//! and an end in seconds, and the event's parameters as bytes. What a type or a byte means is the game's event
//! templates, which the file does not carry (community definitions name them; this reader names none): an event is its
//! type number, its times and its parameter bytes as stored.
//!
//! Read: the 64-bit little-endian layout of version `0x1000D`, Sekiro's and Elden Ring's (SoulsFormatsNEXT's `TAE` reads
//! it as format `SDT`); the other versions (Demon's Souls, Dark Souls, Bloodborne, Dark Souls III) are refused by their
//! version. Not read: the animation groups table (an index over the animations), and the event groups (which events the
//! file ties together).
//!
//! The parameter bytes of an event run to the start of the next event's record; the last event's to the animation's
//! event groups, else to the next animation's header block, else to the end of the file, so they include the zero padding
//! that aligns the records to 16 bytes.

use crate::bytes::{Budget, View};
use crate::{Result, invalid};

pub mod visibility;

const FORMAT: &str = "TAE";

/// The only version read: Sekiro's and Elden Ring's.
pub const VERSION: u32 = 0x1000D;

/// Seconds from which SoulsFormatsNEXT treats an event's end as "until the animation ends" (it reads a stored `f32::MAX`
/// as this placeholder, and Elden Ring stores it as such).
pub const OPEN_END: f32 = 100.0;

/// What an animation's mini header states (the block the animation record's file offset names).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Header {
    /// The block is empty: no file name offset.
    None,
    /// The standard header: the animation plays its own motion data.
    Standard {
        /// "Makes the animation loop by default" (SoulsFormatsNEXT).
        loops: bool,
        /// The animation takes its motion data from the animation numbered `import_hkx_source`.
        imports_hkx: bool,
        /// "Allow this animation to be loaded from delayload anibnds" (SoulsFormatsNEXT).
        allow_delay_load: bool,
        import_hkx_source: i32,
    },
    /// The animation takes both its motion data and its events from the animation numbered `from`.
    ImportOther { from: i32, unknown: i32 },
}

/// One timed event.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Event {
    /// The event's type number, as stored.
    pub kind: i32,
    /// The word after the type, as stored (SoulsFormatsNEXT: "unknown, used in SOTFS and SDT").
    pub unk04: i32,
    /// Seconds into the animation.
    pub start: f32,
    /// Seconds into the animation, as stored: `f32::MAX` or at least [`OPEN_END`] for an event with no end of its own
    /// ([`Event::open_ended`]).
    pub end: f32,
    /// The parameter bytes as stored, with the padding that follows them.
    pub parameters: Vec<u8>,
}

impl Event {
    /// Whether the event runs until its animation ends (an end of [`OPEN_END`] seconds or more).
    pub fn open_ended(&self) -> bool {
        self.end >= OPEN_END
    }
}

/// One animation's record.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Animation {
    /// The animation's number as stored (`3000`).
    pub id: i64,
    pub header: Header,
    /// The motion file name the mini header names, where it states one (not all do).
    pub file_name: Option<String>,
    pub events: Vec<Event>,
}

/// A TAE file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Tae {
    /// The file's own number.
    pub id: i32,
    /// "What set of events this TAE uses" (SoulsFormatsNEXT): the event bank the file's types are numbered in.
    pub event_bank: i64,
    pub skeleton_name: Option<String>,
    pub sib_name: Option<String>,
    pub animations: Vec<Animation>,
}

fn size(v: &View<'_>, o: usize) -> Result<usize> {
    v.size(o)
}

fn float(v: &View<'_>, o: usize) -> Result<f32> {
    let n = f32::from_bits(v.u32(o)?);
    if n.is_nan() {
        return Err(invalid(FORMAT, o, "a time that is not a number"));
    }
    Ok(n)
}

/// The number of the animation a clip named `a000_003000` stands for: its group (the digits after `a`) times a million
/// plus its number (the digits after the underscore), `3000` for that one and `50003000` for `a050_003000`. This is the
/// numbering community tools use for these files; a program that finds the numbers disagree with a file's counts the
/// animations that match none ([`crate::sekiro::AnimationReport`]).
pub fn clip_animation_id(stem: &str) -> Option<i64> {
    let rest = stem.strip_prefix('a')?;
    let (group, number) = rest.split_once('_')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(group) || !digits(number) {
        return None;
    }
    Some(group.parse::<i64>().ok()? * 1_000_000 + number.parse::<i64>().ok()?)
}

impl Tae {
    /// Read the file `data` (already decoded from its DCX envelope, if it had one).
    pub fn read(data: &[u8]) -> Result<Tae> {
        let v = View {
            data,
            format: FORMAT,
            be: false,
        };
        v.expect(0, b"TAE ")?;
        if v.u8(4)? != 0 {
            return Err(invalid(FORMAT, 4, "big-endian files are not read"));
        }
        if v.u8(7)? != 0xFF {
            return Err(invalid(FORMAT, 7, "32-bit files are not read"));
        }
        let version = v.u32(8)?;
        if version != VERSION {
            return Err(invalid(
                FORMAT,
                8,
                format!("version {version:#x}, the one read is {VERSION:#x} (Sekiro, Elden Ring)"),
            ));
        }
        let budget = Budget::new(FORMAT, data.len());
        let event_bank = v.u64(48)? as i64;
        let id = v.u32(80)? as i32;
        let count = v.u32(84)? as usize;
        let table = size(&v, 88)?;
        let name = |at: usize| -> Result<Option<String>> {
            match size(&v, at)? {
                0 => Ok(None),
                offset => Ok(Some(v.utf16z(offset, &budget)?.0)),
            }
        };
        let skeleton_name = name(176)?;
        let sib_name = name(184)?;
        // each record names its animation's body: a number and an offset
        let bytes = count
            .checked_mul(16)
            .ok_or_else(|| invalid(FORMAT, 84, "size overflow"))?;
        v.slice(table, bytes)?;
        let mut bodies = Vec::with_capacity(count);
        for i in 0..count {
            let at = table + i * 16;
            bodies.push((v.u64(at)? as i64, size(&v, at + 8)?));
        }
        let mut animations = Vec::with_capacity(count);
        for (i, &(animation_id, body)) in bodies.iter().enumerate() {
            // where this animation's event parameters stop at the latest: the next animation's header block
            let next_block = bodies.get(i + 1).map(|&(_, b)| size(&v, b + 24)).transpose()?;
            animations.push(read_animation(&v, &budget, animation_id, body, next_block)?);
        }
        Ok(Tae {
            id,
            event_bank,
            skeleton_name,
            sib_name,
            animations,
        })
    }

    /// The animation numbered `id`.
    pub fn animation(&self, id: i64) -> Option<&Animation> {
        self.animations.iter().find(|a| a.id == id)
    }

    /// Follow a TAE import's event record, detecting missing targets and cycles. Motion-only imports
    /// keep their own events; only ImportOther inherits events as well as motion.
    pub fn resolved_animation(&self,id:i64)->Result<Option<&Animation>> {
        let Some(mut record)=self.animation(id) else {return Ok(None);};
        let mut seen=std::collections::BTreeSet::new();
        while let Header::ImportOther {from,..}=&record.header {
            if !seen.insert(record.id) {return Err(invalid(FORMAT,0,format!("animation {id}: cyclic event import")));}
            record=self.animation(i64::from(*from)).ok_or_else(||invalid(FORMAT,0,format!("animation {id}: event import {from} absent")))?;
        }
        Ok(Some(record))
    }
}

fn read_animation(
    v: &View<'_>,
    budget: &Budget,
    id: i64,
    body: usize,
    next_block: Option<usize>,
) -> Result<Animation> {
    let headers = size(v, body)?;
    let groups = size(v, body + 8)?;
    let times = size(v, body + 16)?;
    let block = size(v, body + 24)?;
    let count = v.u32(body + 32)? as usize;
    let bytes = count
        .checked_mul(24)
        .ok_or_else(|| invalid(FORMAT, body + 32, "size overflow"))?;
    v.slice(headers, bytes)?;
    // each event: where its start and end times and its data are
    let mut records = Vec::with_capacity(count);
    for i in 0..count {
        let at = headers + i * 24;
        let (start, end, data) = (size(v, at)?, size(v, at + 8)?, size(v, at + 16)?);
        let parameters = data
            .checked_add(16)
            .ok_or_else(|| invalid(FORMAT, at + 16, "size overflow"))?;
        // the record's parameter offset is the next word after it, or 0
        let stored = size(v, data + 8)?;
        if stored != 0 && stored != parameters {
            return Err(invalid(
                FORMAT,
                data + 8,
                format!("event {i} of animation {id} names parameters at {stored:#x}, not after its record"),
            ));
        }
        records.push((start, end, data, parameters));
    }
    let limit = if groups != 0 {
        groups
    } else {
        next_block.unwrap_or(v.data.len())
    };
    let mut events = Vec::with_capacity(count);
    for (i, &(start, end, data, parameters)) in records.iter().enumerate() {
        // the parameters run to the next record's start, which follows them in the file
        let stop = match records.get(i + 1) {
            Some(&(_, _, next, _)) => next,
            None => limit,
        };
        let length = stop.checked_sub(parameters).ok_or_else(|| {
            invalid(
                FORMAT,
                data,
                format!("event {i} of animation {id}: its parameters would end before they start (events not in file order)"),
            )
        })?;
        budget.charge(parameters, length)?;
        events.push(Event {
            kind: v.u32(data)? as i32,
            unk04: v.u32(data + 4)? as i32,
            start: float(v, start)?,
            end: float(v, end)?,
            parameters: v.slice(parameters, length)?.to_vec(),
        });
    }
    let (header, file_name) = read_block(v, budget, block, times);
    Ok(Animation {
        id,
        header,
        file_name,
        events,
    })
}

/// The mini header block at `block` and the file name it names: `Header::None` and no name where the block states
/// neither or is malformed (the block is the part of the record SoulsFormatsNEXT itself reads heuristically).
fn read_block(v: &View<'_>, budget: &Budget, block: usize, times: usize) -> (Header, Option<String>) {
    let read = || -> Result<(Header, Option<String>)> {
        let kind = v.u32(block)?;
        let record = v.u32(block + 8)? as usize;
        if record == 0 {
            return Ok((Header::None, None));
        }
        let name_at = v.u32(record)? as usize;
        let header = match kind {
            0 => Header::Standard {
                loops: v.u8(record + 8)? != 0,
                imports_hkx: v.u8(record + 9)? != 0,
                allow_delay_load: v.u8(record + 10)? != 0,
                import_hkx_source: v.u32(record + 12)? as i32,
            },
            1 => Header::ImportOther {
                from: v.u32(record + 8)? as i32,
                unknown: v.u32(record + 12)? as i32,
            },
            other => return Err(invalid(FORMAT, block, format!("mini header type {other}"))),
        };
        // the name is the text at `name_at`, unless that is the times table or a value that reads as a time
        let name = if name_at < v.data.len() && name_at != times && v.u64(name_at).ok() != Some(1) {
            let as_time = f32::from_bits(v.u32(name_at)?);
            if (0.016667..=100.0).contains(&as_time) {
                None
            } else {
                Some(v.utf16z(name_at, budget)?.0)
            }
        } else {
            None
        };
        Ok((header, name))
    };
    read().unwrap_or((Header::None, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut Vec<u8>, at: usize, bytes: &[u8]) {
        if buf.len() < at + bytes.len() {
            buf.resize(at + bytes.len(), 0);
        }
        buf[at..at + bytes.len()].copy_from_slice(bytes);
    }
    fn u32le(buf: &mut Vec<u8>, at: usize, n: u32) {
        put(buf, at, &n.to_le_bytes());
    }
    fn u64le(buf: &mut Vec<u8>, at: usize, n: u64) {
        put(buf, at, &n.to_le_bytes());
    }
    fn f32le(buf: &mut Vec<u8>, at: usize, n: f32) {
        put(buf, at, &n.to_le_bytes());
    }

    /// A one-animation file: animation 3000, two events (type 0 from 0 to 0.5 s and type 5 from 0.5 s to the end), the
    /// mini header naming `a000_003000.hkt`.
    fn file() -> Vec<u8> {
        let mut b = vec![0u8; 0xd0];
        put(&mut b, 0, b"TAE ");
        b[7] = 0xff;
        u32le(&mut b, 8, VERSION);
        u64le(&mut b, 48, 7); // event bank
        u32le(&mut b, 80, 1010); // the file's number
        u32le(&mut b, 84, 1); // one animation
        u64le(&mut b, 88, 0xd0); // the table
        // the table: animation 3000, its body at 0xe0
        u64le(&mut b, 0xd0, 3000);
        u64le(&mut b, 0xd8, 0xe0);
        // the body: events at 0x170, no groups, times at 0x160, the block at 0x110
        u64le(&mut b, 0xe0, 0x170);
        u64le(&mut b, 0xe0 + 8, 0);
        u64le(&mut b, 0xe0 + 16, 0x160);
        u64le(&mut b, 0xe0 + 24, 0x110);
        u32le(&mut b, 0xe0 + 32, 2);
        u32le(&mut b, 0xe0 + 40, 3);
        // the block: standard type, its record at 0x120
        u32le(&mut b, 0x110, 0);
        u32le(&mut b, 0x118, 0x120);
        // the record: the name at 0x140, loops, no import, source -1
        u32le(&mut b, 0x120, 0x140);
        put(&mut b, 0x128, &[1]);
        u32le(&mut b, 0x12c, u32::MAX);
        let name: Vec<u8> = "a000_003000.hkt\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        put(&mut b, 0x140, &name);
        // the times: 0, 0.5 and the placeholder for no end
        f32le(&mut b, 0x160, 0.0);
        f32le(&mut b, 0x164, 0.5);
        f32le(&mut b, 0x168, f32::MAX);
        // the headers: (start, end, data) of each event
        for (i, (start, end, data)) in [(0x160u64, 0x164u64, 0x1a0u64), (0x164, 0x168, 0x1c0)]
            .into_iter()
            .enumerate()
        {
            u64le(&mut b, 0x170 + i * 24, start);
            u64le(&mut b, 0x170 + i * 24 + 8, end);
            u64le(&mut b, 0x170 + i * 24 + 16, data);
        }
        // the events: type, word, parameters offset, 16 parameter bytes
        for (data, kind, fill) in [(0x1a0usize, 0u32, 0xaau8), (0x1c0, 5, 0xbb)] {
            u32le(&mut b, data, kind);
            u32le(&mut b, data + 4, 9);
            u64le(&mut b, data + 8, data as u64 + 16);
            put(&mut b, data + 16, &[fill; 16]);
        }
        b
    }

    #[test]
    fn an_animations_events_come_with_their_times_and_parameter_bytes() {
        let tae = Tae::read(&file()).unwrap();
        assert_eq!((tae.id, tae.event_bank, tae.animations.len()), (1010, 7, 1));
        let a = tae.animation(3000).unwrap();
        assert_eq!(a.file_name.as_deref(), Some("a000_003000.hkt"));
        assert_eq!(
            a.header,
            Header::Standard {
                loops: true,
                imports_hkx: false,
                allow_delay_load: false,
                import_hkx_source: -1
            }
        );
        assert_eq!(a.events.len(), 2);
        assert_eq!((a.events[0].kind, a.events[0].start, a.events[0].end), (0, 0.0, 0.5));
        assert_eq!(a.events[0].parameters, vec![0xaa; 16]);
        assert_eq!(a.events[1].kind, 5);
        assert_eq!(a.events[1].unk04, 9);
        assert!(a.events[1].open_ended());
        assert!(!a.events[0].open_ended());
        // the last event's parameters run to the end of the file
        assert_eq!(a.events[1].parameters, vec![0xbb; 16]);
    }

    #[test]
    fn other_versions_and_widths_are_refused_by_what_they_are() {
        let mut other = file();
        u32le(&mut other, 8, 0x1000c);
        assert!(Tae::read(&other).unwrap_err().to_string().contains("0x1000c"));
        let mut narrow = file();
        narrow[7] = 0;
        assert!(Tae::read(&narrow).unwrap_err().to_string().contains("32-bit"));
        let mut big = file();
        big[4] = 1;
        assert!(Tae::read(&big).is_err());
        assert!(Tae::read(&file()[..0x100]).is_err());
    }

    #[test]
    fn events_out_of_file_order_are_refused() {
        let mut b = file();
        // the second event's record before the first's parameters
        u64le(&mut b, 0x170 + 24 + 16, 0x190);
        put(&mut b, 0x190 + 8, &(0x190u64 + 16).to_le_bytes());
        assert!(Tae::read(&b).is_err());
    }

    #[test]
    fn a_clip_is_the_animation_its_group_and_number_make() {
        assert_eq!(clip_animation_id("a000_003000"), Some(3000));
        assert_eq!(clip_animation_id("a050_003000"), Some(50_003_000));
        assert_eq!(clip_animation_id("skeleton"), None);
        assert_eq!(clip_animation_id("a00x_003000"), None);
        assert_eq!(clip_animation_id("a000_"), None);
    }
}
