use crate::{Result, Tagfile, invalid};

/// A typed view into DATA. References remain indices, so cyclic object graphs
/// do not cause recursive allocations or stack overflow during parsing.
#[derive(Debug, Clone, Copy)]
pub struct Value<'t, 'a> {
    pub(crate) tag: &'t Tagfile<'a>,
    pub type_index: usize,
    pub offset: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct Array<'t, 'a> {
    tag: &'t Tagfile<'a>,
    type_index: usize,
    offset: usize,
    count: usize,
    stride: usize,
}
impl<'a> Tagfile<'a> {
    pub fn item(&self, index: usize) -> Result<Array<'_, 'a>> {
        let item = self
            .items
            .get(index)
            .ok_or_else(|| invalid(index, "item index out of bounds"))?;
        Ok(Array {
            tag: self,
            type_index: item.type_index,
            offset: item.offset,
            count: item.count,
            stride: self.stride(item.type_index)?,
        })
    }
    pub fn root(&self) -> Result<Value<'_, 'a>> {
        self.item(1)?.get(0)
    }
}
impl<'t, 'a> Array<'t, 'a> {
    pub fn len(self) -> usize {
        self.count
    }
    pub fn is_empty(self) -> bool {
        self.count == 0
    }
    pub fn get(self, index: usize) -> Result<Value<'t, 'a>> {
        if index >= self.count {
            return Err(invalid(self.offset, "array index out of bounds"));
        }
        let offset = index
            .checked_mul(self.stride)
            .and_then(|n| self.offset.checked_add(n))
            .ok_or_else(|| invalid(self.offset, "array offset overflow"))?;
        self.tag.bytes(offset, self.stride)?;
        Ok(Value {
            tag: self.tag,
            type_index: self.type_index,
            offset,
        })
    }
}
impl<'t, 'a> Value<'t, 'a> {
    pub fn type_name(self) -> &'t str {
        &self.tag.types[self.type_index].name
    }
    pub fn format(self) -> Result<usize> {
        self.tag.format(self.type_index)
    }
    pub fn field(self, name: &str) -> Result<Self> {
        if self.format()? & 15 != 7 {
            return Err(invalid(self.offset, "field access requires a class"));
        }
        let mut index = self.type_index;
        while index != 0 {
            let t = &self.tag.types[index];
            if let Some(f) = t.fields.iter().find(|f| f.name == name) {
                let offset = self
                    .offset
                    .checked_add(f.offset)
                    .ok_or_else(|| invalid(self.offset, "field offset overflow"))?;
                let size = self.tag.inherited(f.type_index, |t| t.size)?;
                let parent_size = self.tag.inherited(self.type_index, |t| t.size)?;
                if f.offset.checked_add(size).is_none_or(|n| n > parent_size) {
                    return Err(invalid(offset, "field exceeds class layout"));
                }
                self.tag.bytes(offset, size)?;
                return Ok(Self {
                    tag: self.tag,
                    type_index: f.type_index,
                    offset,
                });
            }
            index = t.parent;
        }
        Err(invalid(
            self.offset,
            format!("{} has no field {name}", self.type_name()),
        ))
    }
    fn reference_index(self) -> Result<usize> {
        Ok(u32::from_le_bytes(self.tag.bytes(self.offset, 4)?.try_into().expect("four bytes")) as usize)
    }
    pub fn pointer(self) -> Result<Option<Self>> {
        if self.format()? & 15 != 6 {
            return Err(invalid(self.offset, "expected pointer"));
        }
        let index = self.reference_index()?;
        if index == 0 {
            Ok(None)
        } else {
            self.tag.item(index)?.get(0).map(Some)
        }
    }
    pub fn dereference(self) -> Result<Self> {
        self.pointer()?
            .ok_or_else(|| invalid(self.offset, "null pointer"))
    }
    /// A tagfile string reference, bounded by its character ITEM.
    pub fn string(self) -> Result<String> {
        if self.format()? & 15 != 3 {
            return Err(invalid(self.offset, "expected string"));
        }
        let index = self.reference_index()?;
        if index == 0 {
            return Ok(String::new());
        }
        let item = self.tag.item(index)?;
        let mut bytes = Vec::new();
        for i in 0..item.len() {
            let byte = u8::try_from(item.get(i)?.unsigned()?)
                .map_err(|_| invalid(self.offset, "string character exceeds byte"))?;
            if byte == 0 {
                return String::from_utf8(bytes).map_err(|_| invalid(self.offset, "invalid UTF-8 string"));
            }
            bytes.push(byte);
        }
        Err(invalid(self.offset, "unterminated string"))
    }
    pub fn array(self) -> Result<Array<'t, 'a>> {
        let format = self.format()?;
        if format & 15 != 8 {
            return Err(invalid(self.offset, "expected array"));
        }
        if format & 32 == 0 {
            return self.tag.item(self.reference_index()?);
        }
        let type_index = self.tag.inherited(self.type_index, |t| t.subtype)?;
        if type_index == 0 {
            return Err(invalid(self.offset, "array has no element type"));
        }
        let count = format >> 8;
        let stride = self.tag.inherited(type_index, |t| t.size)?;
        let size = count
            .checked_mul(stride)
            .ok_or_else(|| invalid(self.offset, "array size overflow"))?;
        self.tag.bytes(self.offset, size)?;
        Ok(Array {
            tag: self.tag,
            type_index,
            offset: self.offset,
            count,
            stride,
        })
    }
    /// Integer bits without sign extension, preserving the stored representation.
    pub fn unsigned(self) -> Result<u64> {
        let format = self.format()?;
        if ![2, 4].contains(&(format & 15)) {
            return Err(invalid(self.offset, "expected integer"));
        }
        let bits = format >> 10;
        if ![8, 16, 32, 64].contains(&bits) {
            return Err(invalid(self.offset, "unsupported integer width"));
        }
        let bytes = self.tag.bytes(self.offset, bits / 8)?;
        Ok(bytes.iter().rev().fold(0u64, |n, &b| (n << 8) | u64::from(b)))
    }
    pub fn signed(self) -> Result<i64> {
        let format = self.format()?;
        let bits = format >> 10;
        let value = self.unsigned()?;
        if format & 512 == 0 {
            return i64::try_from(value).map_err(|_| invalid(self.offset, "unsigned value exceeds i64"));
        }
        Ok(((value << (64 - bits)) as i64) >> (64 - bits))
    }
    /// Stored float32, including nonfinite values in unused SIMD lanes.
    /// Geometry consumers must validate the components they actually use.
    pub fn float(self) -> Result<f32> {
        if self.format()? != 0x174605 {
            return Err(invalid(self.offset, "expected float32"));
        }
        let value = f32::from_le_bytes(self.tag.bytes(self.offset, 4)?.try_into().expect("four bytes"));
        Ok(value)
    }
    pub fn vector<const N: usize>(self) -> Result<[f32; N]> {
        let a = self.array()?;
        if a.len() != N {
            return Err(invalid(self.offset, "vector length mismatch"));
        }
        let mut result = [0.; N];
        for (i, x) in result.iter_mut().enumerate() {
            *x = a.get(i)?.float()?;
        }
        Ok(result)
    }
}
