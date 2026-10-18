//! Reading an EBX document (ReSkate `ebx_document.cpp`, `read_document`).
use std::collections::BTreeMap;

use super::{
    ArrayRecord, BoxedRef, BoxedValueRecord, Document, FieldDescriptor, FieldType, FieldValue,
    ImportReference, Instance, Object, Pointer, ReflectionGroup, TypeDescriptor, TypeRef, Value,
};
use crate::{Result, bad};

const RIFF: u32 = 0x4646_4952; // "RIFF" read little-endian
const EBX: u32 = 0x4542_5800;
const EBXS: u32 = 0x4542_5853;
const EBXD: u32 = 0x4542_5844;
const EFIX: u32 = 0x4546_4958;
const EBXX: u32 = 0x4542_5858;
const REFL: u32 = 0x5245_464C;
const RFL2: u32 = 0x5246_4C32;

/// How deep types may nest (a type naming itself as its base or member fails instead of recursing forever).
const MAX_DEPTH: u32 = 128;

pub(crate) fn align_up(v: usize, a: usize) -> usize {
    if a <= 1 { v } else { v.div_ceil(a) * a }
}

/// A bounds-checked little-endian cursor over the document's bytes.
struct Cursor<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.d.len())
            .ok_or_else(|| {
                bad(
                    "read",
                    self.pos,
                    format!("{n} bytes past the end ({})", self.d.len()),
                )
            })?;
        let s = &self.d[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.arr::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u32be(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.arr()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.arr()?))
    }
    fn seek(&mut self, p: usize) -> Result<()> {
        if p > self.d.len() {
            return Err(bad("seek", p, format!("past the end ({})", self.d.len())));
        }
        self.pos = p;
        Ok(())
    }
    fn nonnegative(&mut self, what: &'static str) -> Result<usize> {
        let at = self.pos;
        let v = self.i32()?;
        usize::try_from(v).map_err(|_| bad(what, at, "count cannot be negative"))
    }
    /// That `count` records of `stride` bytes fit before `end`.
    fn ensure(&self, end: usize, count: usize, stride: usize, what: &'static str) -> Result<()> {
        if self.pos > end || count > (end - self.pos) / stride {
            return Err(bad(what, self.pos, format!("{count} records exceed their chunk")));
        }
        Ok(())
    }
    fn c_string(&mut self) -> Result<Vec<u8>> {
        let rest = &self.d[self.pos..];
        let n = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("string", self.pos, "unterminated"))?;
        let s = rest[..n].to_vec();
        self.pos += n + 1;
        Ok(s)
    }
}

#[derive(Clone, Copy)]
struct Chunk {
    id: u32,
    begin: usize,
    end: usize,
}

fn read_chunks(r: &mut Cursor) -> Result<Vec<Chunk>> {
    if r.u32()? != RIFF {
        return Err(bad("RIFF", 0, "not a RIFF document"));
    }
    let riff_end = r.u32()? as usize + 8;
    if riff_end != r.d.len() {
        return Err(bad(
            "RIFF",
            4,
            format!("size {riff_end} does not match the stream ({})", r.d.len()),
        ));
    }
    let form = r.u32be()?;
    if form != EBX && form != EBXS {
        return Err(bad("RIFF", 8, format!("form {form:#x} is not EBX or EBXS")));
    }
    let mut chunks = Vec::new();
    while r.pos < riff_end {
        if r.d.len() - r.pos < 8 {
            return Err(bad("RIFF", r.pos, "truncated chunk header"));
        }
        let id = r.u32be()?;
        let size = r.u32()? as usize;
        let begin = r.pos;
        if size > riff_end - begin {
            return Err(bad("RIFF", begin, "chunk exceeds its document"));
        }
        let end = begin + size;
        chunks.push(Chunk { id, begin, end });
        let next = end + (size & 1);
        if next > riff_end {
            return Err(bad("RIFF", end, "chunk padding is truncated"));
        }
        r.seek(next)?;
    }
    Ok(chunks)
}

fn one_chunk(chunks: &[Chunk], id: u32, name: &'static str, required: bool) -> Result<Option<Chunk>> {
    let mut found = None;
    for c in chunks.iter().filter(|c| c.id == id) {
        if found.is_some() {
            return Err(bad(name, c.begin, "duplicate chunk"));
        }
        found = Some(*c);
    }
    if required && found.is_none() {
        return Err(bad(name, 0, "missing chunk"));
    }
    Ok(found)
}

fn validate_data_offset(doc: &Document, offset: u32, required: usize, what: &'static str) -> Result<()> {
    let size = doc.data_end - doc.data_start;
    let o = offset as usize;
    if o > size || required > size - o {
        return Err(bad(what, doc.data_start + o, "offset exceeds EBXD"));
    }
    Ok(())
}

fn read_fixup(r: &mut Cursor, c: Chunk, doc: &mut Document) -> Result<()> {
    r.seek(c.begin)?;
    doc.file_guid = r.arr()?;
    let types = r.nonnegative("EFIX type")?;
    r.ensure(c.end, types, 16, "EFIX type")?;
    for _ in 0..types {
        doc.fixup_type_guids.push(r.arr()?);
    }
    let sigs = r.nonnegative("EFIX signature")?;
    if sigs > types {
        return Err(bad("EFIX signature", r.pos, "more signatures than types"));
    }
    r.ensure(c.end, sigs, 4, "EFIX signature")?;
    for _ in 0..sigs {
        doc.fixup_type_signatures.push(r.u32()?);
    }
    let exported = r.nonnegative("EFIX exported instance")?;
    let instances = r.nonnegative("EFIX instance")?;
    if exported > instances {
        return Err(bad(
            "EFIX instance",
            r.pos,
            "exported count exceeds the instances",
        ));
    }
    r.ensure(c.end, instances, 4, "EFIX instance")?;
    for i in 0..instances {
        let offset = r.u32()?;
        validate_data_offset(doc, offset, 2, "EFIX instance")?;
        let at = doc.data_start + offset as usize;
        let class_ref = u16::from_le_bytes([r.d[at], r.d[at + 1]]);
        if usize::from(class_ref) >= doc.fixup_type_guids.len() {
            return Err(bad(
                "EFIX instance",
                at,
                format!("type slot {class_ref} is invalid"),
            ));
        }
        doc.instances.push(Instance {
            fixup_type: class_ref,
            descriptor: None,
            data_offset: offset,
            exported: i < exported,
            guid: [0; 16],
            object: None,
            raw_image: Vec::new(),
        });
    }
    let offsets = |r: &mut Cursor, doc: &Document, what: &'static str, width: usize| -> Result<Vec<u32>> {
        let n = r.nonnegative(what)?;
        r.ensure(c.end, n, 4, what)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let o = r.u32()?;
            validate_data_offset(doc, o, width, what)?;
            out.push(o);
        }
        Ok(out)
    };
    doc.pointer_offsets = offsets(r, doc, "EFIX pointer", 8)?;
    doc.resource_ref_offsets = offsets(r, doc, "EFIX resource reference", 8)?;
    let imports = r.nonnegative("EFIX import")?;
    r.ensure(c.end, imports, 32, "EFIX import")?;
    for _ in 0..imports {
        doc.imports.push(ImportReference {
            file_guid: r.arr()?,
            class_guid: r.arr()?,
        });
    }
    doc.import_offsets = offsets(r, doc, "EFIX import relocation", 8)?;
    doc.type_info_offsets = offsets(r, doc, "EFIX type-info reference", 8)?;
    if c.end - r.pos < 12 {
        return Err(bad("EFIX", r.pos, "truncated section offsets"));
    }
    doc.arrays_offset = r.u32()?;
    doc.boxed_values_offset = r.u32()?;
    doc.strings_offset = r.u32()?;
    validate_data_offset(doc, doc.arrays_offset, 0, "EFIX array section")?;
    validate_data_offset(doc, doc.boxed_values_offset, 0, "EFIX boxed-value section")?;
    validate_data_offset(doc, doc.strings_offset, 0, "EFIX string section")?;
    if r.pos < c.end {
        doc.boxed_reference_offsets = offsets(r, doc, "EFIX boxed reference", 16)?;
    }
    if r.pos != c.end {
        return Err(bad("EFIX", r.pos, "unexpected bytes at its end"));
    }
    Ok(())
}

fn read_extra(r: &mut Cursor, c: Chunk, doc: &mut Document) -> Result<()> {
    r.seek(c.begin)?;
    let arrays = r.nonnegative("EBXX array")?;
    let boxed = r.nonnegative("EBXX boxed value")?;
    r.ensure(c.end, arrays.saturating_add(boxed), 16, "EBXX")?;
    let record = |r: &mut Cursor, what: &'static str| -> Result<(u32, u32, u32, u16, u16)> {
        let offset = r.u32()?;
        let at = r.pos;
        let count = u32::try_from(r.i32()?).map_err(|_| bad(what, at, "negative count"))?;
        let v = (offset, count, r.u32()?, r.u16()?, r.u16()?);
        validate_data_offset(doc, offset, 0, what)?;
        Ok(v)
    };
    let mut array_records = Vec::with_capacity(arrays);
    for _ in 0..arrays {
        let (offset, count, hash, encoded_type, class_ref) = record(r, "EBXX array")?;
        array_records.push(ArrayRecord {
            offset,
            count,
            hash,
            encoded_type,
            class_ref,
        });
    }
    let mut boxed_records = Vec::with_capacity(boxed);
    for _ in 0..boxed {
        let (offset, count, hash, encoded_type, class_ref) = record(r, "EBXX boxed value")?;
        boxed_records.push(BoxedValueRecord {
            offset,
            count,
            hash,
            encoded_type,
            class_ref,
            raw: Vec::new(),
        });
    }
    if r.pos != c.end {
        return Err(bad("EBXX", r.pos, "unexpected bytes at its end"));
    }
    doc.arrays = array_records;
    doc.boxed_values = boxed_records;
    Ok(())
}

fn string_at(d: &[u8], offset: usize, end: usize) -> Result<String> {
    if offset >= end {
        return Err(bad("reflection string", offset, "offset is invalid"));
    }
    let s = &d[offset..end];
    let n = s
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| bad("reflection string", offset, "unterminated"))?;
    String::from_utf8(s[..n].to_vec()).map_err(|_| bad("reflection string", offset, "is not UTF-8"))
}

/// The reflection table; returns each type's (guid, signature) for mapping instances.
fn read_reflection(
    r: &mut Cursor,
    c: Chunk,
    hashed: bool,
    doc: &mut Document,
) -> Result<Vec<([u8; 16], u32)>> {
    r.seek(c.begin)?;
    let nsig = r.nonnegative("reflection signature")?;
    r.ensure(c.end, nsig, 20, "reflection signature")?;
    let mut ids = Vec::with_capacity(nsig);
    for _ in 0..nsig {
        ids.push((r.arr::<16>()?, r.u32()?));
    }
    let ntypes = r.nonnegative("reflection type")?;
    r.ensure(c.end, ntypes, 16, "reflection type")?;
    for i in 0..ntypes {
        let at = r.pos;
        let name_hash = r.u32()?;
        let field_index = r.i32()?;
        let field_count = r.u16()?;
        let encoded_type = r.u16()? >> 1;
        let size = r.u16()?;
        let alignment = r.u16()?;
        if alignment > 255 {
            return Err(bad("reflection type", at, "alignment is unsupported"));
        }
        let (guid, signature) = ids.get(i).copied().unwrap_or(([0; 16], 0));
        doc.types.push(TypeDescriptor {
            name: String::new(),
            name_hash,
            field_index,
            field_count,
            encoded_type,
            size,
            alignment,
            guid,
            signature,
        });
    }
    let nfields = r.nonnegative("reflection field")?;
    r.ensure(c.end, nfields, 12, "reflection field")?;
    for _ in 0..nfields {
        let name_hash = r.u32()?;
        let data_offset = r.u32()?;
        let encoded_type = r.u16()? >> 1;
        let class_ref = r.u16()?;
        doc.fields.push(FieldDescriptor {
            name: String::new(),
            name_hash,
            encoded_type,
            class_ref,
            data_offset,
        });
    }
    for t in &doc.types {
        let ok = usize::try_from(t.field_index)
            .is_ok_and(|fi| fi <= doc.fields.len() && usize::from(t.field_count) <= doc.fields.len() - fi);
        if !ok {
            return Err(bad(
                "reflection type",
                c.begin,
                format!("{:#x}: field range is invalid", t.name_hash),
            ));
        }
    }
    let ngroups = r.nonnegative("reflection group")?;
    r.ensure(c.end, ngroups, 12, "reflection group")?;
    for _ in 0..ngroups {
        doc.reflection_groups.push(ReflectionGroup {
            hash: r.u32()?,
            count: r.u32()?,
            first: r.u32()?,
        });
    }
    let nmap = r.nonnegative("reflection mapping")?;
    r.ensure(c.end, nmap, 8, "reflection mapping")?;
    r.seek(r.pos + nmap * 8)?;
    if hashed {
        let nnames = r.nonnegative("reflection name")?;
        r.ensure(c.end, nnames, 8, "reflection name")?;
        // a later record of the same hash replaces an earlier one, as in ReSkate's map
        let mut names = BTreeMap::new();
        for _ in 0..nnames {
            let h = r.u32()?;
            names.insert(h, r.u32()? as usize);
        }
        let strings = r.pos;
        let lookup = |h: u32| -> Result<String> {
            let o = names.get(&h).ok_or_else(|| {
                bad(
                    "RFL2 name",
                    c.begin,
                    format!("{h:#x} is missing from {} names", names.len()),
                )
            })?;
            string_at(r.d, strings + o, c.end)
        };
        for t in &mut doc.types {
            t.name = lookup(t.name_hash)?;
        }
        for f in &mut doc.fields {
            f.name = lookup(f.name_hash)?;
        }
    } else {
        let strings = r.pos;
        for t in &mut doc.types {
            t.name = string_at(r.d, strings + t.name_hash as usize, c.end)?;
        }
        for f in &mut doc.fields {
            f.name = string_at(r.d, strings + f.name_hash as usize, c.end)?;
        }
    }
    Ok(ids)
}

/// Reads the values of the document's instances (ReSkate's `ValueReader`).
struct Values<'a> {
    d: &'a [u8],
    r: Cursor<'a>,
    doc: &'a Document,
    /// array record offset -> its hash (the first record of an offset)
    array_hashes: BTreeMap<i64, u32>,
    /// instance data offset -> index (the first instance of an offset)
    instance_at: BTreeMap<i64, usize>,
    depth: u32,
    boxed_slots: Vec<u32>,
}

impl<'a> Values<'a> {
    fn object(&mut self, descriptor: usize, start: usize) -> Result<Object> {
        self.nest(start)?;
        let t = self
            .doc
            .types
            .get(descriptor)
            .ok_or_else(|| bad("EBX object", start, "type reference is invalid"))?;
        if start < self.doc.data_start || usize::from(t.size) > self.doc.data_end.saturating_sub(start) {
            return Err(bad("EBX object", start, "exceeds EBXD"));
        }
        let mut o = Object {
            descriptor,
            fields: Vec::new(),
        };
        self.fields(&mut o, descriptor, start)?;
        self.depth -= 1;
        Ok(o)
    }

    fn nest(&mut self, at: usize) -> Result<()> {
        if self.depth >= MAX_DEPTH {
            return Err(bad("EBX type", at, "types nest too deeply"));
        }
        self.depth += 1;
        Ok(())
    }

    fn fields(&mut self, out: &mut Object, type_index: usize, start: usize) -> Result<()> {
        let t = &self.doc.types[type_index];
        let first = usize::try_from(t.field_index).unwrap_or(0);
        for k in 0..usize::from(t.field_count) {
            let di = first + k;
            let f = &self.doc.fields[di];
            if f.ty() == FieldType::Inherited {
                let base = usize::from(f.class_ref);
                if base >= self.doc.types.len() {
                    return Err(bad("EBX base type", start, "is invalid"));
                }
                self.nest(start)?;
                self.fields(out, base, start)?;
                self.depth -= 1;
                continue;
            }
            let position = start + f.data_offset as usize;
            if position > self.doc.data_end {
                return Err(bad("EBX field", position, "offset exceeds EBXD"));
            }
            let mut value = self.field(f, position)?;
            if f.is_array()
                && let Value::Array(items, hash) = &mut value
                && !items.is_empty()
            {
                let disp = i32::from_le_bytes([
                    self.d[position],
                    self.d[position + 1],
                    self.d[position + 2],
                    self.d[position + 3],
                ]);
                let target = position as i64 + i64::from(disp) - self.doc.data_start as i64;
                *hash = self.array_hashes.get(&target).copied();
            }
            out.fields.push(FieldValue {
                descriptor: di,
                name: f.name.clone(),
                value,
            });
        }
        Ok(())
    }

    fn field(&mut self, f: &FieldDescriptor, position: usize) -> Result<Value> {
        if f.is_array() {
            self.r.seek(position)?;
            let disp = self.r.i32()?;
            let target = position as i64 + i64::from(disp);
            let low = (self.doc.data_start + 4) as i64;
            let high = self.doc.data_end as i64;
            if !(low..=high).contains(&target) {
                return Err(bad("EBX array", position, "displacement exceeds EBXD"));
            }
            let target = target as usize;
            self.r.seek(target - 4)?;
            let count =
                usize::try_from(self.r.i32()?).map_err(|_| bad("EBX array", target - 4, "negative count"))?;
            if count > self.doc.data_end - target {
                return Err(bad("EBX array", target, format!("{count} elements exceed EBXD")));
            }
            self.r.seek(target)?;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let element = self.r.pos;
                items.push(self.value(f.ty(), f.class_ref, element)?);
                match f.ty() {
                    FieldType::Structure => {
                        let t = self
                            .doc
                            .types
                            .get(usize::from(f.class_ref))
                            .ok_or_else(|| bad("EBX array", element, "struct type is invalid"))?;
                        self.r.seek(element + usize::from(t.size))?;
                    }
                    FieldType::Pointer | FieldType::CString => {
                        let p = align_up(self.r.pos, 8);
                        self.r.seek(p)?;
                    }
                    _ => {}
                }
            }
            return Ok(Value::Array(items, None));
        }
        self.r.seek(position)?;
        self.value(f.ty(), f.class_ref, position)
    }

    fn value(&mut self, ty: FieldType, class_ref: u16, position: usize) -> Result<Value> {
        let r = &mut self.r;
        Ok(match ty {
            FieldType::Boolean => Value::Bool(r.u8()? != 0),
            FieldType::Int8 => Value::Int(i64::from(r.u8()? as i8)),
            FieldType::UInt8 => Value::UInt(u64::from(r.u8()?)),
            FieldType::Int16 => Value::Int(i64::from(r.u16()? as i16)),
            FieldType::UInt16 => Value::UInt(u64::from(r.u16()?)),
            FieldType::Int32 | FieldType::Enumeration => Value::Int(i64::from(r.i32()?)),
            FieldType::UInt32 => Value::UInt(u64::from(r.u32()?)),
            FieldType::Int64 => Value::Int(r.i64()?),
            FieldType::UInt64 => Value::UInt(r.u64()?),
            FieldType::Float32 => Value::Float(f64::from(f32::from_le_bytes(r.arr()?))),
            FieldType::Float64 => Value::Float(f64::from_le_bytes(r.arr()?)),
            FieldType::Guid => Value::Guid(r.arr()?),
            FieldType::Sha1 => Value::Sha1(r.arr()?),
            FieldType::ResourceRef => Value::Resource(r.u64()?),
            FieldType::String => {
                let raw = r.take(32)?;
                let n = raw.iter().position(|&b| b == 0).unwrap_or(32);
                Value::Str(raw[..n].to_vec())
            }
            FieldType::CString => {
                let disp = r.u32()?;
                Value::Str(self.relative_string(position, disp)?)
            }
            FieldType::FileRef => {
                let disp = r.u32()?;
                r.take(4)?;
                Value::Str(self.relative_string(position, disp)?)
            }
            FieldType::Pointer => {
                let encoded = r.i32()?;
                if encoded == 0 {
                    Value::Pointer(Pointer::Null)
                } else if encoded & 1 != 0 {
                    let index = usize::try_from(encoded >> 1)
                        .ok()
                        .filter(|&i| i < self.doc.imports.len())
                        .ok_or_else(|| bad("EBX pointer", position, "import is invalid"))?;
                    Value::Pointer(Pointer::External(index))
                } else {
                    let target = position as i64 + i64::from(encoded) - self.doc.data_start as i64;
                    let i = *self
                        .instance_at
                        .get(&target)
                        .ok_or_else(|| bad("EBX pointer", position, "internal target is invalid"))?;
                    Value::Pointer(Pointer::Internal(i))
                }
            }
            FieldType::TypeRef | FieldType::Delegate => {
                let encoded = r.u32()?;
                r.take(4)?;
                if encoded == 0 {
                    Value::TypeRef(TypeRef {
                        primitive: false,
                        primitive_type: None,
                        descriptor: None,
                        encoded: 0,
                    })
                } else if encoded & 0x8000_0000 != 0 {
                    let bits = ((encoded & !0x8000_0000) >> 5) & 0x1F;
                    Value::TypeRef(TypeRef {
                        primitive: true,
                        primitive_type: Some(FieldType::from_bits(bits as u16)),
                        descriptor: None,
                        encoded,
                    })
                } else {
                    Value::TypeRef(TypeRef {
                        primitive: false,
                        primitive_type: None,
                        descriptor: Some((encoded >> 2) as usize),
                        encoded,
                    })
                }
            }
            FieldType::BoxedValueRef => {
                let encoded_type = r.u32()?;
                r.take(4)?;
                let disp = r.i64()?;
                let target = (position + 8) as i64 + disp - self.doc.data_start as i64;
                self.boxed_slots.push((position - self.doc.data_start) as u32);
                Value::Boxed(BoxedRef {
                    encoded_type,
                    data_offset: target,
                    relative_offset: disp,
                })
            }
            FieldType::Structure => Value::Object(Box::new(self.object(usize::from(class_ref), position)?)),
            _ => Value::Nothing,
        })
    }

    fn relative_string(&mut self, base: usize, disp: u32) -> Result<Vec<u8>> {
        if disp == u32::MAX {
            return Ok(Vec::new());
        }
        let target = base as u64 + u64::from(disp);
        if !(self.doc.data_start as u64..self.doc.data_end as u64).contains(&target) {
            return Err(bad("EBX string", base, "displacement exceeds EBXD"));
        }
        let saved = self.r.pos;
        self.r.seek(target as usize)?;
        let s = self.r.c_string()?;
        if self.r.pos > self.doc.data_end {
            return Err(bad("EBX string", target as usize, "exceeds EBXD"));
        }
        self.r.pos = saved;
        Ok(s)
    }
}

/// Reads an EBX document into typed values.
pub fn read(bytes: &[u8]) -> Result<Document> {
    let mut r = Cursor { d: bytes, pos: 0 };
    let chunks = read_chunks(&mut r)?;
    let data = one_chunk(&chunks, EBXD, "EBXD", true)?.ok_or_else(|| bad("EBXD", 0, "missing"))?;
    let fixup = one_chunk(&chunks, EFIX, "EFIX", true)?.ok_or_else(|| bad("EFIX", 0, "missing"))?;
    let extra = one_chunk(&chunks, EBXX, "EBXX", true)?.ok_or_else(|| bad("EBXX", 0, "missing"))?;
    let refl = one_chunk(&chunks, REFL, "REFL", false)?;
    let rfl2 = one_chunk(&chunks, RFL2, "RFL2", false)?;
    let (reflection, hashed) = match (refl, rfl2) {
        (Some(c), None) => (c, false),
        (None, Some(c)) => (c, true),
        _ => return Err(bad("RIFF", 0, "must hold exactly one REFL or RFL2 chunk")),
    };
    let data_start = align_up(data.begin, 16);
    if data_start > data.end {
        return Err(bad("EBXD", data.begin, "alignment exceeds the chunk"));
    }
    r.seek(8)?;
    let serialized_layout = r.u32be()? == EBXS;
    let mut doc = Document {
        file_guid: [0; 16],
        root_type: String::new(),
        data_start,
        data_end: data.end,
        arrays_offset: 0,
        boxed_values_offset: 0,
        strings_offset: 0,
        serialized_layout,
        hashed_reflection_names: hashed,
        reflection_data: bytes[reflection.begin..reflection.end].to_vec(),
        fixup_type_guids: Vec::new(),
        fixup_type_signatures: Vec::new(),
        boxed_reference_offsets: Vec::new(),
        types: Vec::new(),
        fields: Vec::new(),
        instances: Vec::new(),
        imports: Vec::new(),
        arrays: Vec::new(),
        boxed_values: Vec::new(),
        pointer_offsets: Vec::new(),
        resource_ref_offsets: Vec::new(),
        import_offsets: Vec::new(),
        type_info_offsets: Vec::new(),
        boxed_value_slots: Vec::new(),
        payload: bytes[data_start..data.end].to_vec(),
        reflection_groups: Vec::new(),
    };
    read_fixup(&mut r, fixup, &mut doc)?;
    read_extra(&mut r, extra, &mut doc)?;
    let ids = read_reflection(&mut r, reflection, hashed, &mut doc)?;
    // map instances to reflection types by (guid, signature)
    for inst in &mut doc.instances {
        let slot = usize::from(inst.fixup_type);
        if slot >= doc.fixup_type_guids.len() || slot >= doc.fixup_type_signatures.len() {
            continue;
        }
        inst.descriptor = ids
            .iter()
            .position(|&(g, s)| g == doc.fixup_type_guids[slot] && s == doc.fixup_type_signatures[slot]);
    }
    if let Some(d) = doc.instances.first().and_then(|i| i.descriptor)
        && let Some(t) = doc.types.get(d)
    {
        doc.root_type = t.name.clone();
    }
    // values
    let mut instances = std::mem::take(&mut doc.instances);
    {
        let mut array_hashes = BTreeMap::new();
        for a in &doc.arrays {
            array_hashes.entry(i64::from(a.offset)).or_insert(a.hash);
        }
        let mut instance_at = BTreeMap::new();
        for (i, inst) in instances.iter().enumerate() {
            instance_at.entry(i64::from(inst.data_offset)).or_insert(i);
        }
        let mut v = Values {
            d: bytes,
            r: Cursor { d: bytes, pos: 0 },
            doc: &doc,
            array_hashes,
            instance_at,
            depth: 0,
            boxed_slots: Vec::new(),
        };
        for inst in &mut instances {
            let Some(d) = inst.descriptor else { continue };
            let Some(t) = doc.types.get(d) else { continue };
            let start = doc.data_start + inst.data_offset as usize;
            if inst.exported {
                if start < doc.data_start + 16 {
                    return Err(bad("EBX instance", start, "exported instance has no GUID"));
                }
                inst.guid.copy_from_slice(&bytes[start - 16..start]);
            }
            let size = usize::from(t.size);
            if size > doc.data_end - start {
                return Err(bad("EBX instance", start, "exceeds EBXD"));
            }
            inst.raw_image = bytes[start..start + size].to_vec();
            inst.object = Some(v.object(d, start)?);
        }
        let slots = std::mem::take(&mut v.boxed_slots);
        doc.boxed_value_slots = slots;
    }
    doc.instances = instances;
    // boxed value bytes: up to the next record, the last up to the strings
    doc.boxed_values.sort_by_key(|b| b.offset);
    for i in 0..doc.boxed_values.len() {
        let start = doc.boxed_values[i].offset;
        let end = doc
            .boxed_values
            .get(i + 1)
            .map_or(doc.strings_offset, |b| b.offset);
        if end < start {
            return Err(bad("EBXX boxed value", start as usize, "records are not ordered"));
        }
        validate_data_offset(&doc, start, (end - start) as usize, "EBXX boxed value bytes")?;
        let raw = doc.payload[start as usize..end as usize].to_vec();
        doc.boxed_values[i].raw = raw;
    }
    Ok(doc)
}

/// The document's own GUID (what another document's import names it by), without reading the rest.
pub fn read_file_guid(bytes: &[u8]) -> Result<[u8; 16]> {
    let mut r = Cursor { d: bytes, pos: 0 };
    let chunks = read_chunks(&mut r)?;
    let c = one_chunk(&chunks, EFIX, "EFIX", true)?.ok_or_else(|| bad("EFIX", 0, "missing"))?;
    r.seek(c.begin)?;
    r.arr()
}
