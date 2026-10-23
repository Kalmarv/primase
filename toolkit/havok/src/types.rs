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
            base: section.offset + 8,
        }
    }
    pub fn error(&self, detail: impl Into<String>) -> Error {
        invalid(self.base + self.position, detail)
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
    pub fn packed(&mut self) -> Result<u64> {
        let first = self.take(1)?[0];
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
        if self.bytes[self.position..].iter().any(|&b| b != 0) {
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
    if section.payload.last() != Some(&0) {
        return Err(invalid(section.offset, "unterminated string table"));
    }
    section
        .payload
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
        if roots.len() != 1 || roots[0].signature != *b"TCM0" {
            return Err(invalid(0, "expected one TCM0 root"));
        }
        let children = roots[0].children()?;
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
        if index >= count || seen[index] {
            return Err(c.error("invalid or duplicate type body"));
        }
        seen[index] = true;
        let t = &mut types[index];
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
            let hash = u32::from_le_bytes(c.take(4)?.try_into().expect("four bytes"));
            if i == 0 || i >= count || types[i].hash.is_some() {
                return Err(c.error("invalid type hash index"));
            }
            types[i].hash = Some(hash);
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
        let mut parent = t.parent;
        for _ in 0..count {
            if parent == 0 {
                break;
            }
            if parent == i {
                return Err(invalid(section.offset, "cyclic type inheritance"));
            }
            parent = types[parent].parent;
            if parent >= count {
                return Err(invalid(section.offset, "invalid parent type"));
            }
        }
        if parent != 0 {
            return Err(invalid(section.offset, "cyclic parent chain"));
        }
    }
    Ok(types)
}

#[cfg(test)]
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
