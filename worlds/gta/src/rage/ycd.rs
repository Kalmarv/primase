//! A clip dictionary (`.ycd`): animations, each a set of per-bone channel tracks, and the clips that play them, by name
//! hash. This is the container: the dictionary, its animation map and clip map (hash tables of entries chained on a
//! collision), each animation's frame count, duration, bone ids and sequences, and each clip's name, kind and the
//! animations it plays with their time windows and rates.
//!
//! The channel data of a sequence is kept as bytes (its header fields as stored): decoding it into values per bone and
//! frame is [`crate::rage`]'s next step, and the clips' tags and properties are not read (counted). The layout is
//! CodeWalker's (`ClipDictionary`, `Animation`, `Sequence`, `ClipBase`), which the reader holds the file to: every
//! pointer a block holds is followed and must land inside the resource, every count must fit its capacity, and a
//! clip's animation must be one of the dictionary's.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use crate::rage::le::LE;
use crate::rage::resource::Resource;
use crate::{Result, invalid};
use primase_bytes::View;

/// The resource version of Enhanced's clip dictionaries (ycd).
pub const VERSION: u32 = 46;

const ROOT: u64 = 0x5000_0000;

fn err(at: u64, detail: impl ToString) -> crate::Error {
    invalid("ycd", at, detail)
}

/// One bone track of an animation: the bone id, the track (what of the bone it animates: position, rotation, ...) and a
/// byte whose meaning is not known, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationBoneId {
    pub bone: u16,
    pub unknown: u8,
    pub track: u8,
}

/// One sequence of an animation: a span of its frames with the channel data of every track, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub id: u32,
    pub frame_offset: u32,
    pub root_motion_refs_offset: u32,
    pub frames: u16,
    pub frame_length: u16,
    pub indirect_quantize_ints: u16,
    pub quantize_value_bits: u16,
    pub chunk_size: u8,
    pub root_motion_ref_counts: u8,
    /// The channel data, `DataLength` bytes.
    pub data: Vec<u8>,
}

/// An animation.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    /// Bytes 0x10 and 0x11 and the word at 0x1C, as stored (their meaning is not known).
    pub flags: [u8; 2],
    pub unknown_1c: u32,
    pub frames: u16,
    pub sequence_frame_limit: u16,
    pub duration: f32,
    pub max_sequence_block: u32,
    pub usage_count: u32,
    pub sequences: Vec<Sequence>,
    pub bone_ids: Vec<AnimationBoneId>,
}

/// What a clip plays: an animation of the dictionary, between two times, at a rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipPlay {
    /// The animation's hash in the dictionary's animation map.
    pub animation: u32,
    pub start: f32,
    pub end: f32,
    pub rate: f32,
}

/// A clip: a name (its path as the game spells it) and what it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub hash: u32,
    pub name: String,
    /// One animation (type 1) or a list of them (type 2, with the list's duration).
    pub plays: Vec<ClipPlay>,
    pub list_duration: Option<f32>,
    /// Whether the clip has tags and properties (not read).
    pub has_tags: bool,
    pub has_properties: bool,
}

/// A clip dictionary.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipDictionary {
    /// Each animation with its hash, in the animation map's order.
    pub animations: Vec<(u32, Animation)>,
    pub clips: Vec<Clip>,
}

/// The entries of a hash table of `capacity` buckets at `table`: each bucket's chain of 32-byte entries (hash, pointer
/// to the item, pointer to the next entry), as `(hash, item pointer)`. The entry count stored beside the table must be
/// the number of entries found.
fn chains(r: &Resource, table: u64, capacity: usize, entries: usize) -> Result<Vec<(u32, u64)>> {
    let buckets = r.records(table, capacity, 8)?;
    let mut out = Vec::new();
    for bucket in buckets.as_chunks::<8>().0 {
        let mut at = u64::from_le_bytes(*bucket);
        let mut guard = 0usize;
        while at != 0 {
            guard = guard.saturating_add(1);
            if guard > entries.saturating_add(1) {
                return Err(err(table, "a hash table's chain does not end"));
            }
            // the chains may share their entries: stop at the first one past the count, not after all of them
            if out.len() >= entries {
                return Err(err(
                    table,
                    format!("more than the {entries} entries stored are in the chains"),
                ));
            }
            let e = r.at(at, 32)?;
            out.push((View::new(e).u32(0, LE)?, View::new(e).u64(8, LE)?));
            at = View::new(e).u64(0x10, LE)?;
        }
    }
    if out.len() != entries {
        return Err(err(
            table,
            format!("{} entries stored, {} in the chains", entries, out.len()),
        ));
    }
    Ok(out)
}

/// The bytes of a name at a pointer: `len` characters then a NUL, `capacity` the length plus one.
fn name_at(r: &Resource, pointer: u64, len: usize, capacity: usize) -> Result<String> {
    if capacity != len.saturating_add(1) {
        return Err(err(
            pointer,
            format!("a name of {len} characters with capacity {capacity}"),
        ));
    }
    let bytes = r.at(pointer, capacity)?;
    let (text, nul) = bytes.split_at(len);
    if nul != [0] {
        return Err(err(pointer, "a name without its terminating NUL"));
    }
    String::from_utf8(text.to_vec()).map_err(|e| err(pointer, e))
}

fn sequence(r: &Resource, pointer: u64) -> Result<Sequence> {
    let h = r.at(pointer, 0x20)?;
    let len = usize::try_from(View::new(h).u32(4, LE)?).map_err(|_| err(pointer, "a sequence length"))?;
    // the data is copied: a list naming one sequence over and over copies it every time
    r.charge(len.saturating_add(0x20), pointer)?;
    let data = r
        .at(
            pointer
                .checked_add(0x20)
                .ok_or_else(|| err(pointer, "a sequence past the address space"))?,
            len,
        )?
        .to_vec();
    let s = Sequence {
        id: View::new(h).u32(0, LE)?,
        frame_offset: View::new(h).u32(0x0C, LE)?,
        root_motion_refs_offset: View::new(h).u32(0x10, LE)?,
        frames: View::new(h).u16(0x16, LE)?,
        frame_length: View::new(h).u16(0x18, LE)?,
        indirect_quantize_ints: View::new(h).u16(0x1A, LE)?,
        quantize_value_bits: View::new(h).u16(0x1C, LE)?,
        chunk_size: View::new(h).u8(0x1E)?,
        root_motion_ref_counts: View::new(h).u8(0x1F)?,
        data,
    };
    if View::new(h).u32(8, LE)? != 0 || View::new(h).u16(0x14, LE)? != 0 {
        return Err(err(pointer, "a sequence's reserved words are not zero"));
    }
    // the root motion offset counts from 32 bytes before the data (the header's length), and is the data's length plus
    // 32 when the sequence has none; the frame data lies before it
    let root_motion = s.root_motion_refs_offset.checked_sub(0x20);
    if !root_motion.is_some_and(|rm| s.frame_offset <= rm && usize::try_from(rm).is_ok_and(|rm| rm <= len)) {
        return Err(err(
            pointer,
            format!(
                "frame data at {}, root motion at {}, of {len} bytes",
                s.frame_offset, s.root_motion_refs_offset
            ),
        ));
    }
    Ok(s)
}

fn animation(r: &Resource, pointer: u64) -> Result<Animation> {
    let a = r.at(pointer, 96)?;
    let list = |at: usize| -> Result<(u64, usize, usize)> {
        Ok((
            View::new(a).u64(at, LE)?,
            usize::from(View::new(a).u16(at.saturating_add(8), LE)?),
            usize::from(View::new(a).u16(at.saturating_add(10), LE)?),
        ))
    };
    let (seq_ptr, seq_count, seq_capacity) = list(0x40)?;
    let (bone_ptr, bone_count, bone_capacity) = list(0x50)?;
    if seq_count > seq_capacity || bone_count > bone_capacity {
        return Err(err(pointer, "a list with more entries than its capacity"));
    }
    r.charge(
        seq_capacity
            .saturating_mul(8)
            .saturating_add(bone_count.saturating_mul(4))
            .saturating_add(96),
        pointer,
    )?;
    let pointers = r.records(seq_ptr, seq_capacity, 8)?;
    let sequences = pointers
        .as_chunks::<8>()
        .0
        .iter()
        .take(seq_count)
        .map(|p| sequence(r, u64::from_le_bytes(*p)))
        .collect::<Result<Vec<_>>>()?;
    let bone_ids = r
        .records(bone_ptr, bone_count, 4)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| AnimationBoneId {
            bone: u16::from_le_bytes([b[0], b[1]]),
            unknown: b[2],
            track: b[3],
        })
        .collect();
    Ok(Animation {
        flags: [View::new(a).u8(0x10)?, View::new(a).u8(0x11)?],
        unknown_1c: View::new(a).u32(0x1C, LE)?,
        frames: View::new(a).u16(0x14, LE)?,
        sequence_frame_limit: View::new(a).u16(0x16, LE)?,
        duration: View::new(a).f32(0x18, LE)?,
        max_sequence_block: View::new(a).u32(0x38, LE)?,
        usage_count: View::new(a).u32(0x3C, LE)?,
        sequences,
        bone_ids,
    })
}

impl ClipDictionary {
    /// A ycd's dictionary, after checking the resource is in Enhanced's layout.
    pub fn read_resource(r: &Resource) -> Result<ClipDictionary> {
        if r.version != VERSION {
            return Err(err(
                0,
                format!(
                    "resource version {}, Enhanced's ycd layout is {VERSION}",
                    r.version
                ),
            ));
        }
        let d = r.at(ROOT, 0x40)?;
        // the animations: a map block holding the table
        let map = r.at(View::new(d).u64(0x18, LE)?, 48)?;
        let (table, capacity, entries) = (
            View::new(map).u64(0x18, LE)?,
            usize::from(View::new(map).u16(0x20, LE)?),
            usize::from(View::new(map).u16(0x22, LE)?),
        );
        let mut by_pointer: BTreeMap<u64, u32> = BTreeMap::new();
        let mut animations = Vec::new();
        for (hash, pointer) in chains(r, table, capacity, entries)? {
            if pointer == 0 {
                return Err(err(table, format!("animation {hash:#010x} without a body")));
            }
            by_pointer.insert(pointer, hash);
            animations.push((hash, animation(r, pointer)?));
        }
        // the clips
        let (clips_table, clips_capacity, clips_entries) = (
            View::new(d).u64(0x28, LE)?,
            usize::from(View::new(d).u16(0x30, LE)?),
            usize::from(View::new(d).u16(0x32, LE)?),
        );
        let mut clips = Vec::new();
        for (hash, pointer) in chains(r, clips_table, clips_capacity, clips_entries)? {
            let c = r.at(pointer, 112)?;
            let name = name_at(
                r,
                View::new(c).u64(0x18, LE)?,
                usize::from(View::new(c).u16(0x20, LE)?),
                usize::from(View::new(c).u16(0x22, LE)?),
            )?;
            let play = |start: usize, animation: u64| -> Result<ClipPlay> {
                Ok(ClipPlay {
                    animation: *by_pointer.get(&animation).ok_or_else(|| {
                        err(
                            pointer,
                            format!("clip {name} plays an animation that is not the dictionary's"),
                        )
                    })?,
                    start: View::new(c).f32(start, LE)?,
                    end: View::new(c).f32(start.saturating_add(4), LE)?,
                    rate: View::new(c).f32(start.saturating_add(8), LE)?,
                })
            };
            let (plays, list_duration) = match View::new(c).u32(0x10, LE)? {
                1 => (vec![play(0x54, View::new(c).u64(0x50, LE)?)?], None),
                2 => {
                    let count = usize::from(View::new(c).u16(0x58, LE)?);
                    if View::new(c).u16(0x5A, LE)? != View::new(c).u16(0x58, LE)? {
                        return Err(err(
                            pointer,
                            format!("clip {name}: two different counts of its animations"),
                        ));
                    }
                    let list = r.records(View::new(c).u64(0x50, LE)?, count, 24)?;
                    let plays = list
                        .as_chunks::<24>()
                        .0
                        .iter()
                        .map(|e| {
                            let animation = View::new(e).u64(0x10, LE)?;
                            Ok(ClipPlay {
                                animation: *by_pointer.get(&animation).ok_or_else(|| {
                                    err(
                                        pointer,
                                        format!(
                                            "clip {name} plays an animation that is not the dictionary's"
                                        ),
                                    )
                                })?,
                                start: View::new(e).f32(0, LE)?,
                                end: View::new(e).f32(4, LE)?,
                                rate: View::new(e).f32(8, LE)?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    (plays, Some(View::new(c).f32(0x60, LE)?))
                }
                t => return Err(err(pointer, format!("clip {name} of type {t}"))),
            };
            clips.push(Clip {
                hash,
                name,
                plays,
                list_duration,
                has_tags: View::new(c).u64(0x38, LE)? != 0,
                has_properties: View::new(c).u64(0x40, LE)? != 0,
            });
        }
        Ok(ClipDictionary { animations, clips })
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;
    use crate::rage::resource::PageFlags;

    fn pages(writes: &[(usize, &[u8])]) -> Resource {
        let mut data = vec![0u8; 0x2000];
        for (o, b) in writes {
            data[*o..*o + b.len()].copy_from_slice(b);
        }
        Resource {
            version: VERSION,
            system: PageFlags((1 << 27) | 4),
            graphics: PageFlags(0),
            data,
            zero_filled: 0,
            spent: Default::default(),
        }
    }

    /// Hash table buckets that all name one chain: the entries are read once for the count they state, not once per
    /// bucket.
    #[test]
    fn buckets_sharing_a_chain_are_stopped_at_the_stated_count() {
        let entry = (ROOT + 0x200).to_le_bytes();
        let table: Vec<u8> = (0..4).flat_map(|_| entry).collect();
        let r = pages(&[(0x100, &table)]);
        let e = chains(&r, ROOT + 0x100, 4, 1).unwrap_err().to_string();
        assert!(e.contains("more than the 1 entries stored"), "{e}");
        // one bucket, one entry: the count
        assert_eq!(chains(&r, ROOT + 0x100, 1, 1).unwrap(), [(0, 0)]);
    }

    /// A list naming one sequence again and again copies its data every time: the copies stop at a few times the
    /// resource.
    #[test]
    fn a_sequence_named_over_and_over_is_copied_only_a_few_times_the_resources_size() {
        let mut header = [0u8; 0x20];
        header[4..8].copy_from_slice(&0x200u32.to_le_bytes());
        header[0x10..0x14].copy_from_slice(&0x20u32.to_le_bytes());
        let r = pages(&[(0x100, &header)]);
        let refused = (0..200).position(|_| sequence(&r, ROOT + 0x100).is_err());
        let n = refused.expect("refused once the copies pass the allowance");
        assert!((50..70).contains(&n), "{n} copies of 544 bytes in 4 x 8 KiB");
    }
}
