#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::{Error, Result, Section, invalid, sections};

#[derive(Debug, Clone, Default)]
pub struct Type {
    pub name: String,
    pub parent: usize,
    pub flags: u64,
    pub format: Option<u64>,
    pub subtype: Option<usize>,
    pub version: Option<u64>,
    pub size: Option<usize>,
    pub alignment: Option<usize>,
    pub abstract_value: Option<u64>,
    pub parameters: Vec<(String, u64)>,
    pub fields: Vec<Field>,
    /// Reflection properties the type declares beside its fields: counted, their records are not serialized.
    pub properties: usize,
    pub interfaces: Vec<(usize, u64)>,
    pub hash: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub flags: u64,
    pub offset: usize,
    pub type_index: usize,
}

#[derive(Debug, Clone)]
pub struct Compendium {
    pub ids: Vec<[u8; 8]>,
    /// Index zero is a null placeholder.
    pub types: Vec<Type>,
}

pub(crate) struct Cursor<'a> {
    pub bytes: &'a [u8],
    pub position: usize,
    pub base: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(section: Section<'a>) -> Self {
        Self {
            bytes: section.payload,
            position: 0,
            base: section.offset.saturating_add(8),
        }
    }
    pub fn error(&self, detail: impl Into<String>) -> Error {
        invalid(self.base.saturating_add(self.position), detail)
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(n)
            .ok_or_else(|| self.error("length overflow"))?;
        let b = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| self.error("short input"))?;
        self.position = end;
        Ok(b)
    }
    /// The next `N` bytes.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.take(N)?;
        b.try_into().map_err(|_| self.error("short input"))
    }
    pub fn u32_le(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }
    pub fn packed(&mut self) -> Result<u64> {
        let [first] = self.array()?;
        let (remaining, mask) = match first {
            0..=0x7f => (0, 0x7f),
            0x80..=0xbf => (1, 0x3f),
            0xc0..=0xdf => (2, 0x1f),
            0xe0..=0xe7 => (3, 7),
            0xe8..=0xef => (4, 7),
            0xf0..=0xf7 => (7, 7),
            0xf8 => (5, 0),
            0xf9 => (8, 0),
            _ => return Err(self.error("reserved packed integer prefix")),
        };
        let mut value = (first & mask) as u64;
        for &byte in self.take(remaining)? {
            value = (value << 8) | u64::from(byte);
        }
        Ok(value)
    }
    pub fn index(&mut self) -> Result<usize> {
        usize::try_from(self.packed()?).map_err(|_| self.error("index exceeds address space"))
    }
    pub fn padding(&mut self) -> Result<()> {
        let rest = self
            .bytes
            .get(self.position..)
            .ok_or_else(|| self.error("position past the end"))?;
        if rest.iter().any(|&b| b != 0) {
            return Err(self.error("nonzero trailing bytes"));
        }
        self.position = self.bytes.len();
        Ok(())
    }
}

pub(crate) fn required<'a>(children: &[Section<'a>], names: &[[u8; 4]]) -> Result<Section<'a>> {
    let mut found = children.iter().filter(|s| names.contains(&s.signature));
    let result = *found
        .next()
        .ok_or_else(|| invalid(0, format!("missing section {:?}", names)))?;
    if found.next().is_some() {
        return Err(invalid(result.offset, "duplicate section"));
    }
    Ok(result)
}
fn strings(section: Section<'_>) -> Result<Vec<String>> {
    // the table's strings are the NUL-terminated runs: the empty run after the last NUL is no string, so an index
    // equal to the table's length is out of bounds, not an empty name
    let Some((0, strings)) = section.payload.split_last() else {
        return Err(invalid(section.offset, "unterminated string table"));
    };
    strings
        .split(|&b| b == 0)
        .map(|b| {
            String::from_utf8(b.to_vec()).map_err(|_| invalid(section.offset, "invalid UTF-8 string table"))
        })
        .collect()
}
fn string(table: &[String], c: &mut Cursor<'_>) -> Result<String> {
    let index = c.index()?;
    table
        .get(index)
        .cloned()
        .ok_or_else(|| c.error("string index out of bounds"))
}

impl Compendium {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let roots = sections(bytes)?;
        let &[root] = roots.as_slice() else {
            return Err(invalid(0, "expected one TCM0 root"));
        };
        if root.signature != *b"TCM0" {
            return Err(invalid(0, "expected one TCM0 root"));
        }
        let children = root.children()?;
        for s in &children {
            if ![*b"TCID", *b"TYPE"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown compendium section"));
            }
        }
        let ids = required(&children, &[*b"TCID"])?;
        if ids.payload.is_empty() || ids.payload.len() % 8 != 0 {
            return Err(invalid(ids.offset, "invalid compendium IDs"));
        }
        let ids = ids.payload.as_chunks::<8>().0.to_vec();
        let types = read_types(required(&children, &[*b"TYPE"])?)?;
        Ok(Self { ids, types })
    }
}

pub(crate) fn read_types(section: Section<'_>) -> Result<Vec<Type>> {
    let children = section.children()?;
    for s in &children {
        if ![
            *b"TPTR", *b"TSTR", *b"TST1", *b"TNAM", *b"TNA1", *b"FSTR", *b"FST1", *b"TBOD", *b"TBDY",
            *b"THSH", *b"TPAD",
        ]
        .contains(&s.signature)
        {
            return Err(invalid(s.offset, "unknown type section"));
        }
        if [*b"TPTR", *b"TPAD"].contains(&s.signature) && s.payload.iter().any(|&b| b != 0) {
            return Err(invalid(s.offset, "nonzero type padding or pointer table"));
        }
    }
    let names = strings(required(&children, &[*b"TSTR", *b"TST1"])?)?;
    let field_names = strings(required(&children, &[*b"FSTR", *b"FST1"])?)?;
    let mut c = Cursor::new(required(&children, &[*b"TNAM", *b"TNA1"])?);
    let count = c.index()?;
    if count == 0 || count > c.bytes.len() {
        return Err(c.error("invalid type count"));
    }
    let mut types = vec![Type::default(); count];
    for ty in types.iter_mut().skip(1) {
        ty.name = string(&names, &mut c)?;
        let n = c.index()?;
        if n > c.bytes.len() / 2 {
            return Err(c.error("invalid template count"));
        }
        for _ in 0..n {
            ty.parameters.push((string(&names, &mut c)?, c.packed()?));
        }
    }
    c.padding()?;
    let mut c = Cursor::new(required(&children, &[*b"TBOD", *b"TBDY"])?);
    let mut seen = vec![false; count];
    while c.position < c.bytes.len() {
        let index = c.index()?;
        if index == 0 {
            continue;
        }
        let (Some(seen), Some(t)) = (seen.get_mut(index), types.get_mut(index)) else {
            return Err(c.error("invalid or duplicate type body"));
        };
        if std::mem::replace(seen, true) {
            return Err(c.error("invalid or duplicate type body"));
        }
        t.parent = c.index()?;
        t.flags = c.packed()?;
        if t.flags & !127 != 0 {
            return Err(c.error("unknown type flags"));
        }
        if t.flags & 1 != 0 {
            t.format = Some(c.packed()?);
        }
        if t.flags & 2 != 0 {
            t.subtype = Some(c.index()?);
        }
        if t.flags & 4 != 0 {
            t.version = Some(c.packed()?);
        }
        if t.flags & 8 != 0 {
            t.size = Some(c.index()?);
            t.alignment = Some(c.index()?);
        }
        if t.flags & 16 != 0 {
            t.abstract_value = Some(c.packed()?);
        }
        if t.flags & 32 != 0 {
            // The low 16 bits count the field records that follow; the high 16 bits count reflection properties,
            // which store no records here (as HKLib's 2018 reader reads the same word).
            let counts = c.packed()?;
            t.properties = usize::try_from(counts >> 16).map_err(|_| c.error("property count overflow"))?;
            let n = (counts & 0xffff) as usize;
            if n > c.bytes.len() / 4 {
                return Err(c.error("unsupported or invalid member count"));
            }
            for _ in 0..n {
                t.fields.push(Field {
                    name: string(&field_names, &mut c)?,
                    flags: c.packed()?,
                    offset: c.index()?,
                    type_index: c.index()?,
                });
            }
        }
        if t.flags & 64 != 0 {
            let n = c.index()?;
            if n > c.bytes.len() / 2 {
                return Err(c.error("invalid interface count"));
            }
            for _ in 0..n {
                t.interfaces.push((c.index()?, c.packed()?));
            }
        }
    }
    if let Some(&hashes) = children.iter().find(|s| s.signature == *b"THSH") {
        let mut c = Cursor::new(hashes);
        let n = c.index()?;
        for _ in 0..n {
            let i = c.index()?;
            let hash = c.u32_le()?;
            match types.get_mut(i) {
                Some(t) if i != 0 && t.hash.is_none() => t.hash = Some(hash),
                _ => return Err(c.error("invalid type hash index")),
            }
        }
        c.padding()?;
    }
    for (i, t) in types.iter().enumerate().skip(1) {
        if t.parameters
            .iter()
            .any(|(name, value)| name.starts_with('t') && *value >= count as u64)
            || t.parent >= count
            || t.subtype.is_some_and(|s| s >= count)
            || t.fields
                .iter()
                .any(|f| f.type_index == 0 || f.type_index >= count)
            || t.interfaces.iter().any(|&(j, _)| j == 0 || j >= count)
        {
            return Err(invalid(section.offset, format!("invalid type reference in {i}")));
        }
    }
    if let Some(i) = parent_cycle(&types) {
        return Err(invalid(section.offset, format!("type {i} inherits from itself")));
    }
    Ok(types)
}

/// A type on a cycle of parents, if there is one: every type's parent chain must end at the null type 0. Linear in the
/// types: a type is visited once, however many chains pass through it. Every `parent` must be below `types.len()`.
fn parent_cycle(types: &[Type]) -> Option<usize> {
    // 0: not visited, 1: on the chain being followed, 2: known to end at the null type
    let mut state = vec![0u8; types.len()];
    if let Some(null) = state.first_mut() {
        *null = 2;
    }
    for start in 1..types.len() {
        let mut chain = Vec::new();
        let mut at = start;
        // the state the chain ends on (a parent past the types, which the caller excludes, ends it as the null type)
        let end = loop {
            let (Some(s), Some(t)) = (state.get_mut(at), types.get(at)) else {
                break 2;
            };
            if *s != 0 {
                break *s;
            }
            *s = 1;
            chain.push(at);
            at = t.parent;
        };
        if end == 1 {
            return Some(at);
        }
        for t in chain {
            if let Some(s) = state.get_mut(t) {
                *s = 2;
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, reason = "test fixtures built by hand")]
mod tests {
    use super::*;
    #[test]
    fn packed_integer_widths_preserve_high_bits() {
        for (bytes, expected) in [
            (vec![0x7f], 127u64),
            (vec![0xbf, 0xff], 0x3fff),
            (vec![0xdf, 0xff, 0xff], 0x1fffff),
            (vec![0xe7, 0xff, 0xff, 0xff], 0x7ffffff),
            (vec![0xef, 0xff, 0xff, 0xff, 0xff], 0x7ffffffff),
            (
                vec![0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                0x7ffffffffffffff,
            ),
            (vec![0xf8, 0xff, 0xff, 0xff, 0xff, 0xff], 0xffffffffff),
            (
                vec![0xf9, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                u64::MAX,
            ),
        ] {
            let mut c = Cursor {
                bytes: &bytes,
                position: 0,
                base: 0,
            };
            assert_eq!(c.packed().expect("integer"), expected);
            assert_eq!(c.position, bytes.len());
            for n in 0..bytes.len() {
                let mut c = Cursor {
                    bytes: &bytes[..n],
                    position: 0,
                    base: 0,
                };
                assert!(c.packed().is_err());
            }
        }
    }
    /// Parent chains that end at the null type pass, however long and however many share a tail (a type visited once,
    /// not once per chain through it: a chain of this many was ~5e9 steps); a cycle is found, a self-parent too.
    #[test]
    fn parent_chains_are_checked_in_one_pass() {
        let typed = |parents: &[usize]| -> Vec<Type> {
            parents
                .iter()
                .map(|&parent| Type {
                    parent,
                    ..Type::default()
                })
                .collect()
        };
        let n = 100_000;
        let chain: Vec<usize> = (0..n).map(|i: usize| i.saturating_sub(1)).collect();
        assert_eq!(parent_cycle(&typed(&chain)), None);
        // 0 null, 1 and 2 hang from 0, 3 from 2, 4 from 3
        assert_eq!(parent_cycle(&typed(&[0, 0, 0, 2, 3])), None);
        assert!(
            parent_cycle(&typed(&[0, 2, 1])).is_some(),
            "1 and 2 are each other's parent"
        );
        assert!(
            parent_cycle(&typed(&[0, 1])).is_some(),
            "a type that is its own parent"
        );
        assert!(
            parent_cycle(&typed(&[0, 0, 3, 4, 2])).is_some(),
            "a cycle beyond the first type"
        );
    }

    #[test]
    fn reserved_integer_markers_are_errors() {
        for first in 0xfa..=0xff {
            let bytes = [first; 9];
            let mut c = Cursor {
                bytes: &bytes,
                position: 0,
                base: 0,
            };
            assert!(c.packed().is_err());
        }
    }
}
