//! FMG: the text files of the games' message binders (`msg/<language>/item.msgbnd.dcx`), a table of ids to UTF-16
//! strings. Sekiro's and Elden Ring's are the 64-bit-offset layout (version 2), the Dark Souls family's the 32-bit one
//! (version 1); both are read, little-endian and Unicode only (the Demon's Souls layout is big-endian Shift-JIS, which
//! no game this crate reads uses).
//!
//! ```text
//! 0x00  u8 0, u8 big-endian (0), u8 version, u8 0
//! 0x04  u32 file size
//! 0x08  u8 unicode (1), u8 0, u8 0, u8 0
//! 0x0c  u32 group count          0x10  u32 string count
//! version 2: 0x14 u32 0xff, 0x18 u64 offset of the string offsets, 0x20 u64 0, groups from 0x28 (16 bytes each)
//! version 1: 0x14 u32 offset of the string offsets, 0x18 u32 0, groups from 0x1c (12 bytes each)
//! group: u32 index of its first string offset, u32 first id, u32 last id (, u32 0 in version 2)
//! string offset: u64 (u32 in version 1) from the file's start to a NUL-terminated UTF-16 string, 0 for none
//! ```
//!
//! A group holds the ids `first..=last`, its strings the offsets from `index` on. The layout is SoulsFormats' `FMG`
//! (as SoulsFormatsNEXT states it); a file that departs from it is refused, naming the byte.
use std::collections::BTreeMap;

use crate::{Result, invalid};

const FORMAT: &str = "FromSoftware FMG";

/// The strings of one FMG by id. An id with no string (offset 0) is not in the map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fmg {
    pub entries: BTreeMap<i32, String>,
}

impl Fmg {
    /// Reads `data`, a whole FMG file (not a DCX envelope).
    pub fn read(data: &[u8]) -> Result<Fmg> {
        let u8_at = |at: usize| -> Result<u8> {
            data.get(at)
                .copied()
                .ok_or_else(|| invalid(FORMAT, at, "ends before its header does"))
        };
        let u32_at = |at: usize| -> Result<u32> {
            data.get(at..at + 4)
                .and_then(|b| <[u8; 4]>::try_from(b).ok())
                .map(u32::from_le_bytes)
                .ok_or_else(|| invalid(FORMAT, at, "ends inside a 32-bit value"))
        };
        let u64_at = |at: usize| -> Result<u64> {
            data.get(at..at + 8)
                .and_then(|b| <[u8; 8]>::try_from(b).ok())
                .map(u64::from_le_bytes)
                .ok_or_else(|| invalid(FORMAT, at, "ends inside a 64-bit value"))
        };
        if u8_at(0)? != 0 || u8_at(3)? != 0 {
            return Err(invalid(FORMAT, 0, "not an FMG (the first and fourth bytes are 0 in the games' files)"));
        }
        if u8_at(1)? != 0 {
            return Err(invalid(FORMAT, 1, "a big-endian FMG: no game this reader serves stores one"));
        }
        let wide = match u8_at(2)? {
            1 => false,
            2 => true,
            other => return Err(invalid(FORMAT, 2, format!("version {other}; the readable ones are 1 and 2"))),
        };
        if u8_at(8)? != 1 {
            return Err(invalid(FORMAT, 8, "a Shift-JIS FMG: no game this reader serves stores one"));
        }
        let groups = u32_at(0x0c)? as usize;
        let strings = u32_at(0x10)? as usize;
        let (offsets, first_group, group_size) = if wide {
            (
                usize::try_from(u64_at(0x18)?)
                    .map_err(|_| invalid(FORMAT, 0x18, "an offset past the address space"))?,
                0x28,
                16,
            )
        } else {
            (u32_at(0x14)? as usize, 0x1c, 12)
        };
        let slot = if wide { 8 } else { 4 };
        // every string has an offset slot, so a count the file cannot hold is refused before anything is allocated
        if strings > data.len() / slot || groups > data.len() / group_size {
            return Err(invalid(
                FORMAT,
                0x0c,
                format!("{groups} groups and {strings} strings in {} bytes", data.len()),
            ));
        }
        let mut entries = BTreeMap::new();
        let mut counted = 0usize;
        for g in 0..groups {
            let at = first_group + g * group_size;
            let (index, first, last) = (u32_at(at)? as usize, u32_at(at + 4)? as i32, u32_at(at + 8)? as i32);
            if last < first {
                return Err(invalid(FORMAT, at, format!("a group of ids {first} to {last}")));
            }
            let count = (i64::from(last) - i64::from(first) + 1) as usize;
            counted += count;
            if counted > strings {
                return Err(invalid(FORMAT, at, format!("groups hold more than the {strings} strings it states")));
            }
            for j in 0..count {
                let slot_at = offsets + (index + j) * slot;
                let offset = if wide {
                    usize::try_from(u64_at(slot_at)?)
                        .map_err(|_| invalid(FORMAT, slot_at, "an offset past the address space"))?
                } else {
                    u32_at(slot_at)? as usize
                };
                if offset == 0 {
                    continue;
                }
                entries.insert(first + j as i32, utf16_at(data, offset)?);
            }
        }
        Ok(Fmg { entries })
    }
}

/// The NUL-terminated UTF-16LE string at `at`. Unpaired surrogates are refused, not replaced.
fn utf16_at(data: &[u8], at: usize) -> Result<String> {
    let rest = data
        .get(at..)
        .ok_or_else(|| invalid(FORMAT, at, "a string offset past the end of the file"))?;
    let units: Vec<u16> = rest
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| u16::from_le_bytes(b))
        .take_while(|&unit| unit != 0)
        .collect();
    // a string that runs to the end of the file with no terminator is cut off, not a string
    if (units.len() + 1) * 2 > rest.len() {
        return Err(invalid(FORMAT, at, "a string with no terminator before the end of the file"));
    }
    String::from_utf16(&units).map_err(|_| invalid(FORMAT, at, "a string with an unpaired surrogate"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian FMG of `groups` (first id and a string, or none, for each id of the group), in the 64-bit layout
    /// when `wide`.
    pub(crate) fn build(groups: &[(i32, &[Option<&str>])], wide: bool) -> Vec<u8> {
        let (slot, group_size, first_group) = if wide { (8, 16, 0x28) } else { (4, 12, 0x1c) };
        let strings: usize = groups.iter().map(|(_, s)| s.len()).sum();
        let offsets = first_group + groups.len() * group_size;
        let mut text = offsets + strings * slot;
        let mut slots = Vec::new();
        let mut body = Vec::new();
        for (_, group) in groups {
            for string in *group {
                match string {
                    None => slots.push(0usize),
                    Some(s) => {
                        slots.push(text + body.len());
                        body.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
                        body.extend([0, 0]);
                    }
                }
            }
        }
        text += body.len();
        let mut out = vec![0, 0, if wide { 2 } else { 1 }, 0];
        out.extend((text as u32).to_le_bytes());
        out.extend([1, 0, 0, 0]);
        out.extend((groups.len() as u32).to_le_bytes());
        out.extend((strings as u32).to_le_bytes());
        if wide {
            out.extend(0xffu32.to_le_bytes());
            out.extend((offsets as u64).to_le_bytes());
            out.extend(0u64.to_le_bytes());
        } else {
            out.extend((offsets as u32).to_le_bytes());
            out.extend(0u32.to_le_bytes());
        }
        let mut index = 0u32;
        for (first, group) in groups {
            out.extend(index.to_le_bytes());
            out.extend(first.to_le_bytes());
            out.extend((first + group.len() as i32 - 1).to_le_bytes());
            if wide {
                out.extend(0u32.to_le_bytes());
            }
            index += group.len() as u32;
        }
        assert_eq!(out.len(), offsets);
        for slot_value in slots {
            if wide {
                out.extend((slot_value as u64).to_le_bytes());
            } else {
                out.extend((slot_value as u32).to_le_bytes());
            }
        }
        out.extend(body);
        out
    }

    #[test]
    fn both_layouts_read_their_groups_strings_and_missing_ids() {
        let groups: &[(i32, &[Option<&str>])] = &[
            (10, &[Some("Genichiro Ashina"), None, Some("Isshin")] as &[Option<&str>]),
            (700_000, &[Some("Ōkami 狼")] as &[Option<&str>]),
        ];
        for wide in [true, false] {
            let fmg = Fmg::read(&build(groups, wide)).unwrap();
            assert_eq!(
                fmg.entries.into_iter().collect::<Vec<_>>(),
                [
                    (10, "Genichiro Ashina".to_owned()),
                    (12, "Isshin".to_owned()),
                    (700_000, "Ōkami 狼".to_owned()),
                ],
                "wide {wide}: an id with no string is not listed, and the text is Unicode"
            );
        }
    }

    #[test]
    fn a_file_that_departs_from_the_layout_is_refused_naming_the_byte() {
        let good = build(&[(1, &[Some("a")] as &[Option<&str>])], true);
        let refused = |edit: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = good.clone();
            edit(&mut bytes);
            Fmg::read(&bytes).unwrap_err().to_string()
        };
        assert!(refused(&|b| b[1] = 1).contains("big-endian"));
        assert!(refused(&|b| b[2] = 0).contains("version 0"));
        assert!(refused(&|b| b[8] = 0).contains("Shift-JIS"));
        // a string count short of what the groups hold
        assert!(refused(&|b| b[0x10] = 0).contains("groups hold more than the 0 strings"));
        assert!(refused(&|b| b.truncate(0x2a)).contains("ends"));
        // the last string loses its terminator
        assert!(refused(&|b| b.truncate(b.len() - 2)).contains("terminator"));
        // a group whose last id precedes its first
        assert!(refused(&|b| b[0x30..0x34].copy_from_slice(&0i32.to_le_bytes())).contains("ids 1 to 0"));
        // a count the file cannot hold
        assert!(refused(&|b| b[0x0c..0x10].copy_from_slice(&u32::MAX.to_le_bytes())).contains("groups and"));
    }
}
