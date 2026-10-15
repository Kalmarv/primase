use crate::types::{Cursor, read_types, required};
use crate::{Compendium, Result, Type, invalid, sections};

#[derive(Debug, Clone, Copy)]
pub struct Item {
    pub type_index: usize,
    pub flags: u8,
    pub offset: usize,
    pub count: usize,
}

#[derive(Debug)]
pub struct Tagfile<'a> {
    pub version: [u8; 8],
    pub types: Vec<Type>,
    pub items: Vec<Item>,
    pub patches: Vec<(usize, Vec<usize>)>,
    data: &'a [u8],
    data_offset: usize,
}

impl<'a> Tagfile<'a> {
    pub fn read(bytes: &'a [u8], compendium: Option<&Compendium>) -> Result<Self> {
        let roots = sections(bytes)?;
        if roots.len() != 1 || roots[0].signature != *b"TAG0" {
            return Err(invalid(0, "expected one TAG0 root"));
        }
        let children = roots[0].children()?;
        for s in &children {
            if ![*b"SDKV", *b"DATA", *b"TYPE", *b"TCRF", *b"INDX"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown tagfile section"));
            }
        }
        let sdk = required(&children, &[*b"SDKV"])?;
        let version: [u8; 8] = sdk
            .payload
            .try_into()
            .map_err(|_| invalid(sdk.offset, "invalid SDK version"))?;
        if version != *b"20160200" {
            return Err(invalid(sdk.offset, "unsupported SDK version"));
        }
        let ts = required(&children, &[*b"TYPE", *b"TCRF"])?;
        let types = if ts.signature == *b"TYPE" {
            read_types(ts)?
        } else {
            let mut c = Cursor::new(ts);
            let id: [u8; 8] = c.take(8)?.try_into().expect("eight bytes");
            c.padding()?;
            let cmp = compendium.ok_or_else(|| invalid(ts.offset, "external compendium required"))?;
            if !cmp.ids.contains(&id) {
                return Err(invalid(ts.offset, "compendium ID mismatch"));
            }
            cmp.types.clone()
        };
        let data = required(&children, &[*b"DATA"])?;
        let indices = required(&children, &[*b"INDX"])?.children()?;
        for s in &indices {
            if ![*b"ITEM", *b"PTCH"].contains(&s.signature) {
                return Err(invalid(s.offset, "unknown index section"));
            }
        }
        let table = required(&indices, &[*b"ITEM"])?;
        if table.payload.len() % 12 != 0 {
            return Err(invalid(table.offset, "partial item record"));
        }
        let mut items = Vec::new();
        for b in table.payload.as_chunks::<12>().0 {
            let info = u32::from_le_bytes(b[..4].try_into().expect("four bytes"));
            let type_index = (info & 0xffffff) as usize;
            if type_index >= types.len() {
                return Err(invalid(table.offset, "item type out of bounds"));
            }
            items.push(Item {
                type_index,
                flags: (info >> 24) as u8,
                offset: u32::from_le_bytes(b[4..8].try_into().expect("four bytes")) as usize,
                count: u32::from_le_bytes(b[8..].try_into().expect("four bytes")) as usize,
            });
        }
        if items.len() < 2 || items[0].type_index != 0 || items[0].count != 0 || items[0].offset != 0 {
            return Err(invalid(table.offset, "missing null/root item"));
        }
        let mut patches = Vec::new();
        let patch_sections = indices
            .iter()
            .filter(|s| s.signature == *b"PTCH")
            .collect::<Vec<_>>();
        if patch_sections.len() > 1 {
            return Err(invalid(table.offset, "duplicate patch section"));
        }
        if let Some(&ps) = patch_sections.first() {
            let mut c = Cursor::new(*ps);
            while c.position < c.bytes.len() {
                let ti = u32::from_le_bytes(c.take(4)?.try_into().expect("four bytes")) as usize;
                let count = u32::from_le_bytes(c.take(4)?.try_into().expect("four bytes")) as usize;
                if ti == 0 || ti >= types.len() {
                    return Err(c.error("patch type out of bounds"));
                }
                let mut offsets = Vec::new();
                for _ in 0..count {
                    let offset = u32::from_le_bytes(c.take(4)?.try_into().expect("four bytes")) as usize;
                    let bytes = data
                        .payload
                        .get(offset..offset.saturating_add(4))
                        .ok_or_else(|| c.error("patch exceeds DATA"))?;
                    let index = u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize;
                    if index >= items.len() {
                        return Err(c.error("patched item reference out of bounds"));
                    }
                    offsets.push(offset);
                }
                patches.push((ti, offsets));
            }
        }
        let result = Self {
            version,
            types,
            items,
            patches,
            data: data.payload,
            data_offset: data.offset + 8,
        };
        for (i, item) in result.items.iter().enumerate().skip(1) {
            if item.type_index == 0 {
                return Err(invalid(table.offset, format!("null type in item {i}")));
            }
            let stride = result.stride(item.type_index)?;
            let size = stride
                .checked_mul(item.count)
                .ok_or_else(|| invalid(table.offset, "item length overflow"))?;
            result.bytes(item.offset, size)?;
        }
        Ok(result)
    }
    pub fn inherited(&self, mut index: usize, f: impl Fn(&Type) -> Option<usize>) -> Result<usize> {
        for _ in 0..self.types.len() {
            if index == 0 {
                return Ok(0);
            }
            let t = self
                .types
                .get(index)
                .ok_or_else(|| invalid(0, "type index out of bounds"))?;
            if let Some(value) = f(t) {
                return Ok(value);
            }
            index = t.parent;
        }
        Err(invalid(0, "cyclic inherited property"))
    }
    pub fn format(&self, index: usize) -> Result<usize> {
        self.inherited(index, |t| t.format.map(|n| n as usize))
    }
    pub fn stride(&self, index: usize) -> Result<usize> {
        let f = self.format(index)?;
        // Serialized reference arrays contain 32-bit ITEM indices. Their type's
        // byte size describes the runtime pointer, not the serialized stride.
        if f & 15 == 6 || (f & 15 == 3 && f & 32 == 0) {
            Ok(4)
        } else {
            self.inherited(index, |t| t.size)
        }
    }
    pub fn bytes(&self, offset: usize, size: usize) -> Result<&'a [u8]> {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| invalid(self.data_offset + offset, "DATA range overflow"))?;
        self.data
            .get(offset..end)
            .ok_or_else(|| invalid(self.data_offset + offset, "object exceeds DATA"))
    }
}
