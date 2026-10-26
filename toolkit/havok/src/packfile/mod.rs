//! Bounds-checked view of Havok binary *packfiles* (the pre-tagfile `0x57E0E057 0x10C0C010` container), version 7
//! with the 7.1.0-r1 contents version, 4-byte pointers, big endian: the layout the PS3 titles of that SDK ship.
//! Only that layout is read; any other version or layout rules are refused by name.
//!
//! Layout (offsets from the file start; every field checked on 490 real files):
//!
//! | offset | field | check |
//! |---|---|---|
//! | `0x00` | magic `57 E0 E0 57 10 C0 C0 10` | exact |
//! | `0x0C` | file version | 7 |
//! | `0x10` | layout rules: pointer size, little-endian flag, reuse-padding, empty-base-optimisation | `4, 0, 1, 1` |
//! | `0x14` | section count | headers fit the file |
//! | `0x18`, `0x1C` | contents section index and offset (the root object) | the data section; a virtual-fixup target |
//! | `0x20`, `0x24` | class-name section index and the root's class-name offset | the class-name section; a listed name |
//! | `0x28` | contents version, 16 bytes, NUL-terminated | `Havok-7.1.0-r1` |
//! | `0x38`, `0x3C` | two `0xFFFFFFFF` words, kept in [`Packfile::flags`] | read, not interpreted |
//! | `0x40 + 48 i` | section header: tag (20 bytes), then `start, local, global, virtual, exports, imports, end` | in order, inside the file |
//!
//! `__classnames__` holds `u32 signature, 0x09, name\0` entries (then `0xFF` padding). `__types__` is empty: there is no
//! reflection, so class layouts live in [`physics`], stated per SDK version and confirmed on the data. `__data__` holds the
//! objects; its fixups are local `{src,dst}`, global `{src,section,dst}` and virtual `{src,section,class-name offset}`,
//! each list ending at `0xFFFFFFFF` (the rest of its range is `0xFFFFFFFF` padding). An *object* is a virtual-fixup target;
//! it extends to the next one (arrays and strings that follow belong to it). A pointer slot is non-null iff it has a local
//! or global fixup; a slot with neither must hold zero.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use primase_bytes::{BoundsError, Endian::Big, View};

use crate::{Error, Result, invalid};

pub mod physics;

/// The contents version this reader knows.
pub const CONTENTS_VERSION: &str = "Havok-7.1.0-r1";
const MAGIC: [u8; 8] = [0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10];
const SECTION_HEADER: usize = 48;
const FIRST_SECTION: usize = 0x40;
const END_OF_FIXUPS: u32 = 0xFFFF_FFFF;

/// A section header; every offset but `start` is relative to `start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub tag: String,
    pub start: usize,
    pub local: usize,
    pub global: usize,
    pub virtual_fixups: usize,
    pub exports: usize,
    pub imports: usize,
    pub end: usize,
}

/// An object: a virtual-fixup target of the data section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    /// Offset in the data section.
    pub offset: usize,
    /// Bytes up to the next object (or the end of the data), including its trailing arrays and strings.
    pub size: usize,
    class: usize,
}

/// A hkArray header: element pointer (None when null) and element count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Array {
    pub data: Option<usize>,
    pub len: usize,
}

#[derive(Debug)]
pub struct Packfile<'a> {
    bytes: &'a [u8],
    pub sections: Vec<Section>,
    pub flags: [u32; 2],
    classes: Vec<(usize, String)>,
    data_start: usize,
    data_len: usize,
    local: Vec<(usize, usize)>,
    global: Vec<(usize, usize)>,
    objects: Vec<Object>,
    root: usize,
}

fn bounds(offset: usize, what: &str) -> impl FnOnce(BoundsError) -> Error + '_ {
    move |_| invalid(offset, format!("{what} runs past the file"))
}

fn sum(offset: usize, a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| invalid(offset, "offset overflows the address space"))
}

fn to_usize(w: u32) -> usize {
    usize::try_from(w).unwrap_or(usize::MAX)
}

impl<'a> Packfile<'a> {
    /// Parse and check the container (header, sections, class names, every fixup).
    pub fn read(bytes: &'a [u8]) -> Result<Self> {
        let v = View::new(bytes);
        let u32_at = |o: usize, what: &str| v.u32(o, Big).map_err(bounds(o, what));
        let usize_at = |o: usize, what: &str| -> Result<usize> {
            usize::try_from(u32_at(o, what)?).map_err(|_| invalid(o, "value exceeds the address space"))
        };
        let magic = v.array::<8>(0).map_err(bounds(0, "the magic"))?;
        if magic != MAGIC {
            return Err(invalid(0, "not a Havok packfile (magic)"));
        }
        let version = u32_at(0x0C, "the file version")?;
        if version != 7 {
            return Err(invalid(
                0x0C,
                format!("unsupported packfile version {version} (7 only)"),
            ));
        }
        let layout = v.array::<4>(0x10).map_err(bounds(0x10, "the layout rules"))?;
        let [ptr, little, reuse, empty_base] = layout;
        if ptr != 4 {
            return Err(invalid(0x10, format!("unsupported pointer size {ptr} (4 only)")));
        }
        if little != 0 {
            return Err(invalid(
                0x11,
                "unsupported byte order: little endian (big endian only)",
            ));
        }
        if reuse != 1 || empty_base != 1 {
            return Err(invalid(
                0x12,
                format!(
                    "unsupported layout rules: reuse padding {reuse}, empty base optimisation {empty_base} (1, 1 only)"
                ),
            ));
        }
        let version_text = v.slice(0x28, 16).map_err(bounds(0x28, "the contents version"))?;
        let version_text = version_text.split(|&b| b == 0).next().unwrap_or_default();
        if version_text != CONTENTS_VERSION.as_bytes() {
            return Err(invalid(
                0x28,
                format!(
                    "unsupported contents version {:?} ({CONTENTS_VERSION} only)",
                    String::from_utf8_lossy(version_text)
                ),
            ));
        }
        let flags = [u32_at(0x38, "the flags")?, u32_at(0x3C, "the predicate word")?];
        let count = usize_at(0x14, "the section count")?;
        let table_end = count
            .checked_mul(SECTION_HEADER)
            .and_then(|n| n.checked_add(FIRST_SECTION))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| invalid(0x14, "the section headers run past the file"))?;
        let mut sections = Vec::new();
        for i in 0..count {
            // in range: the table fits the file (checked above)
            let o = FIRST_SECTION.saturating_add(i.saturating_mul(SECTION_HEADER));
            let tag_bytes = v.slice(o, 19).map_err(bounds(o, "a section tag"))?;
            let tag_bytes = tag_bytes.split(|&b| b == 0).next().unwrap_or_default();
            let tag = std::str::from_utf8(tag_bytes)
                .map_err(|_| invalid(o, "section tag is not text"))?
                .to_string();
            let f = |k: usize| {
                usize_at(
                    o.saturating_add(20).saturating_add(k.saturating_mul(4)),
                    "a section field",
                )
            };
            let s = Section {
                tag,
                start: f(0)?,
                local: f(1)?,
                global: f(2)?,
                virtual_fixups: f(3)?,
                exports: f(4)?,
                imports: f(5)?,
                end: f(6)?,
            };
            let ordered = s.local <= s.global
                && s.global <= s.virtual_fixups
                && s.virtual_fixups <= s.exports
                && s.exports <= s.imports
                && s.imports <= s.end;
            if !ordered {
                return Err(invalid(
                    o,
                    format!("section {:?}: fixup offsets are not in order", s.tag),
                ));
            }
            if s.start < table_end || sum(o, s.start, s.end)? > bytes.len() {
                return Err(invalid(o, format!("section {:?} lies outside the file", s.tag)));
            }
            sections.push(s);
        }
        let section_named = |tag: &str| -> Result<usize> {
            let mut found = sections
                .iter()
                .enumerate()
                .filter(|(_, s)| s.tag == tag)
                .map(|(i, _)| i);
            match (found.next(), found.next()) {
                (Some(i), None) => Ok(i),
                _ => Err(invalid(
                    FIRST_SECTION,
                    format!("expected exactly one {tag} section"),
                )),
            }
        };
        let class_section = section_named("__classnames__")?;
        let data_section = section_named("__data__")?;
        section_named("__types__")?;
        // the level files' __types__ is empty; the ragdoll files carry reflection data, which is not read
        if usize_at(0x18, "the contents section")? != data_section {
            return Err(invalid(0x18, "the root object is not in the data section"));
        }
        if usize_at(0x20, "the class-name section")? != class_section {
            return Err(invalid(
                0x20,
                "the root class name is not in the class-name section",
            ));
        }
        let (Some(class_header), Some(data_header)) =
            (sections.get(class_section), sections.get(data_section))
        else {
            return Err(invalid(FIRST_SECTION, "section index out of range"));
        };
        // class names
        let class_bytes = v
            .slice(class_header.start, class_header.end)
            .map_err(bounds(class_header.start, "the class names"))?;
        let mut classes = Vec::new();
        let mut at = 0usize;
        // an entry is `u32 signature, 0x09, name `; the signature may start with any byte, so the separator decides
        while class_bytes.get(at..).unwrap_or_default().get(4) == Some(&9) {
            let here = sum(class_header.start, class_header.start, at)?;
            let name_at = sum(here, at, 5)?;
            let tail = class_bytes.get(name_at..).unwrap_or_default();
            let len = tail
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid(here, "class name is not terminated"))?;
            let name = std::str::from_utf8(tail.get(..len).unwrap_or_default())
                .map_err(|_| invalid(here, "class name is not text"))?;
            classes.push((name_at, name.to_string()));
            at = name_at.saturating_add(len).saturating_add(1);
        }
        if class_bytes
            .get(at..)
            .is_some_and(|rest| rest.iter().any(|&b| b != 0xFF))
        {
            return Err(invalid(
                class_header.start,
                "class names are followed by something but 0xFF padding",
            ));
        }
        let data_start = data_header.start;
        let data_len = data_header.local;
        let fixups = |from: usize, to: usize, words: usize| -> Result<Vec<Vec<usize>>> {
            let width = words.saturating_mul(4);
            let span = to
                .checked_sub(from)
                .ok_or_else(|| invalid(data_start, "fixup range is reversed"))?;
            let n = span.checked_div(width).unwrap_or(0);
            let mut out = Vec::new();
            let mut ended = false;
            for i in 0..n {
                let o = data_start
                    .saturating_add(from)
                    .saturating_add(i.saturating_mul(width));
                let mut e = Vec::new();
                for w in 0..words {
                    e.push(u32_at(o.saturating_add(w.saturating_mul(4)), "a fixup")?);
                }
                let first = e.first().copied().unwrap_or(END_OF_FIXUPS);
                if first == END_OF_FIXUPS {
                    ended = true;
                    if e.iter().any(|&w| w != END_OF_FIXUPS) {
                        return Err(invalid(o, "fixup list terminator is not 0xFFFFFFFF padding"));
                    }
                } else if ended {
                    return Err(invalid(o, "fixup after the list's terminator"));
                } else {
                    out.push(e.into_iter().map(to_usize).collect());
                }
            }
            // sections are 16-aligned, so a list may end in a partial record of 0xFF padding
            let tail = data_start
                .saturating_add(from)
                .saturating_add(n.saturating_mul(width));
            let rest = v
                .slice(tail, span.saturating_sub(n.saturating_mul(width)))
                .map_err(bounds(tail, "fixup padding"))?;
            if rest.iter().any(|&b| b != 0xFF) {
                return Err(invalid(tail, "partial fixup record that is not 0xFF padding"));
            }
            Ok(out)
        };
        let slot = |at: usize, dst: usize, what: &str| -> Result<()> {
            let in_range = sum(data_start, at, 4)? <= data_len;
            if in_range && dst <= data_len {
                Ok(())
            } else {
                Err(invalid(
                    data_start.saturating_add(at),
                    format!("{what} fixup {at:#x} -> {dst:#x} is outside the data"),
                ))
            }
        };
        let mut local = Vec::new();
        for e in fixups(data_header.local, data_header.global, 2)? {
            let (src, dst) = (e.first().copied().unwrap_or(0), e.get(1).copied().unwrap_or(0));
            slot(src, dst, "local")?;
            local.push((src, dst));
        }
        let mut global = Vec::new();
        for e in fixups(data_header.global, data_header.virtual_fixups, 3)? {
            let (src, sec, dst) = (
                e.first().copied().unwrap_or(0),
                e.get(1).copied().unwrap_or(0),
                e.get(2).copied().unwrap_or(0),
            );
            if sec != data_section {
                return Err(invalid(
                    data_start.saturating_add(src),
                    format!("global fixup into section {sec} (the data section only)"),
                ));
            }
            slot(src, dst, "global")?;
            global.push((src, dst));
        }
        let mut virtuals = Vec::new();
        for e in fixups(data_header.virtual_fixups, data_header.exports, 3)? {
            let (src, sec, name) = (
                e.first().copied().unwrap_or(0),
                e.get(1).copied().unwrap_or(0),
                e.get(2).copied().unwrap_or(0),
            );
            let here = data_start.saturating_add(src);
            if sec != class_section {
                return Err(invalid(
                    here,
                    format!("virtual fixup names section {sec}, not the class names"),
                ));
            }
            let class = classes.binary_search_by_key(&name, |(o, _)| *o).map_err(|_| {
                invalid(
                    here,
                    format!("virtual fixup class-name offset {name:#x} is not a name"),
                )
            })?;
            if src >= data_len {
                return Err(invalid(here, "object starts outside the data"));
            }
            virtuals.push((src, class));
        }
        if data_header.exports != data_header.end {
            return Err(invalid(data_start, "export/import tables are not supported"));
        }
        local.sort_unstable();
        global.sort_unstable();
        virtuals.sort_unstable();
        let repeated = |l: &[(usize, usize)]| {
            l.windows(2)
                .any(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
        };
        if repeated(&local) || repeated(&global) {
            return Err(invalid(data_start, "two fixups for one pointer slot"));
        }
        if virtuals
            .windows(2)
            .any(|w| w.first().map(|a| a.0) == w.get(1).map(|b| b.0))
        {
            return Err(invalid(data_start, "two virtual fixups for one object"));
        }
        if local
            .iter()
            .any(|(s, _)| global.binary_search_by_key(s, |(g, _)| *g).is_ok())
        {
            return Err(invalid(
                data_start,
                "a pointer slot has a local and a global fixup",
            ));
        }
        let mut objects = Vec::new();
        for (i, &(offset, class)) in virtuals.iter().enumerate() {
            let next = virtuals.get(i.saturating_add(1)).map_or(data_len, |n| n.0);
            objects.push(Object {
                offset,
                size: next.saturating_sub(offset),
                class,
            });
        }
        let root = usize_at(0x1C, "the root offset")?;
        let root_name = usize_at(0x24, "the root class name")?;
        let root_object = objects
            .binary_search_by_key(&root, |o| o.offset)
            .ok()
            .and_then(|i| objects.get(i));
        match root_object.and_then(|o| classes.get(o.class)) {
            Some((name_at, _)) if *name_at == root_name => {}
            _ => {
                return Err(invalid(
                    0x1C,
                    "the root offset/class is not a virtual-fixup object",
                ));
            }
        }
        Ok(Self {
            bytes,
            sections,
            flags,
            classes,
            data_start,
            data_len,
            local,
            global,
            objects,
            root,
        })
    }

    /// Every object, in data order.
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    /// Offset of the root object (`hkRootLevelContainer`).
    pub fn root(&self) -> usize {
        self.root
    }

    pub fn class_name(&self, object: &Object) -> &str {
        self.classes.get(object.class).map_or("", |(_, n)| n.as_str())
    }

    /// Offsets of the objects of class `name`, in data order.
    pub fn objects_of<'s>(&'s self, name: &'s str) -> impl Iterator<Item = usize> + 's {
        self.objects
            .iter()
            .filter(move |o| self.class_name(o) == name)
            .map(|o| o.offset)
    }

    /// Object count per class name.
    pub fn census(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for o in &self.objects {
            let n: &mut usize = out.entry(self.class_name(o)).or_default();
            *n = n.saturating_add(1);
        }
        out
    }

    /// The class name of the object that starts at `offset`.
    pub fn class_at(&self, offset: usize) -> Result<&str> {
        self.objects
            .binary_search_by_key(&offset, |o| o.offset)
            .ok()
            .and_then(|i| self.objects.get(i))
            .map(|o| self.class_name(o))
            .ok_or_else(|| self.err(offset, "no object starts here"))
    }

    /// An error at a data-section offset, reported at its file offset.
    pub(crate) fn err(&self, offset: usize, detail: impl Into<String>) -> Error {
        invalid(self.data_start.saturating_add(offset), detail)
    }

    /// `offset + add`, an error instead of wrapping.
    pub(crate) fn at(&self, offset: usize, add: usize) -> Result<usize> {
        offset
            .checked_add(add)
            .ok_or_else(|| self.err(offset, "offset overflows the address space"))
    }

    /// `len` bytes of the data section at `offset`.
    pub fn bytes(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = self.at(offset, len)?;
        if end > self.data_len {
            return Err(self.err(offset, format!("{len} bytes run past the data section")));
        }
        let start = self.at(self.data_start, offset)?;
        self.bytes
            .get(start..self.at(start, len)?)
            .ok_or_else(|| self.err(offset, "the data section runs past the file"))
    }

    pub fn u8(&self, offset: usize) -> Result<u8> {
        self.bytes(offset, 1)?
            .first()
            .copied()
            .ok_or_else(|| self.err(offset, "empty read"))
    }
    pub fn u16(&self, offset: usize) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array(offset)?))
    }
    pub fn u32(&self, offset: usize) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array(offset)?))
    }
    /// A `u32` as a size or count.
    pub fn count(&self, offset: usize) -> Result<usize> {
        usize::try_from(self.u32(offset)?).map_err(|_| self.err(offset, "count exceeds the address space"))
    }
    /// A finite `f32` (NaN and infinity are errors).
    pub fn f32(&self, offset: usize) -> Result<f32> {
        let x = f32::from_be_bytes(self.array(offset)?);
        if x.is_finite() {
            Ok(x)
        } else {
            Err(self.err(offset, "non-finite float"))
        }
    }
    /// `N` consecutive finite floats.
    pub fn floats<const N: usize>(&self, offset: usize) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.f32(self.at(offset, i.saturating_mul(4))?)?;
        }
        Ok(out)
    }
    pub(crate) fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        self.bytes(offset, N)?
            .try_into()
            .map_err(|_| self.err(offset, "short read"))
    }

    /// The pointer stored at `offset`: None when null. A slot with a fixup resolves to its target in the data section;
    /// one without must hold zero.
    pub fn pointer(&self, offset: usize) -> Result<Option<usize>> {
        let word = self.u32(offset)?;
        let local = self
            .local
            .binary_search_by_key(&offset, |(s, _)| *s)
            .ok()
            .and_then(|i| self.local.get(i));
        let global = self
            .global
            .binary_search_by_key(&offset, |(s, _)| *s)
            .ok()
            .and_then(|i| self.global.get(i));
        match local.or(global) {
            Some(&(_, dst)) => Ok(Some(dst)),
            None if word == 0 => Ok(None),
            None => Err(self.err(offset, format!("pointer slot holds {word:#x} but has no fixup"))),
        }
    }

    /// A pointer that must not be null.
    pub fn target(&self, offset: usize, what: &str) -> Result<usize> {
        self.pointer(offset)?
            .ok_or_else(|| self.err(offset, format!("{what} is null")))
    }

    /// A hkArray header at `offset` (`ptr, size, capacityAndFlags`): the size fits the capacity, a non-empty array has data.
    pub fn array_header(&self, offset: usize) -> Result<Array> {
        let data = self.pointer(offset)?;
        let len = self.count(self.at(offset, 4)?)?;
        let cap = self.u32(self.at(offset, 8)?)?;
        let capacity = to_usize(cap & 0x3FFF_FFFF);
        if len > capacity {
            return Err(self.err(offset, format!("array of {len} exceeds its capacity {capacity}")));
        }
        if len != 0 && data.is_none() {
            return Err(self.err(offset, "non-empty array has no data"));
        }
        Ok(Array { data, len })
    }

    /// The NUL-terminated text at `offset`, bytes mapped to chars one to one (Latin-1).
    pub fn c_string(&self, offset: usize) -> Result<String> {
        let tail = self.bytes(offset, self.data_len.saturating_sub(offset))?;
        let len = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.err(offset, "string is not terminated"))?;
        Ok(tail
            .get(..len)
            .unwrap_or_default()
            .iter()
            .map(|&b| char::from(b))
            .collect())
    }
}
