//! Encoding an EBX document to bytes (ReSkate `ebx_writer.cpp`): instances laid out again from their values
//! (padding and unread bytes from each instance's image as read), then arrays, boxed values (carried as bytes, with
//! the arrays and strings they point at), strings (deduplicated, in first-use order) and the fixup tables.
use std::collections::BTreeMap;

use super::decode::align_up;
use super::{
    ArrayRecord, BoxedRef, BoxedValueRecord, Document, FieldDescriptor, FieldType, Object, Pointer, Value,
};
use crate::{Result, bad};

const W: &str = "EBX writer";

fn scalar_size(t: FieldType) -> usize {
    use FieldType::*;
    match t {
        Boolean | Int8 | UInt8 => 1,
        Int16 | UInt16 => 2,
        Int32 | UInt32 | Float32 | Enumeration => 4,
        Int64 | UInt64 | Float64 | ResourceRef | Pointer | CString | FileRef | TypeRef | Delegate => 8,
        Guid => 16,
        Sha1 => 20,
        String => 32,
        BoxedValueRef => 16,
        _ => 8,
    }
}

fn u32_of(v: usize, what: &'static str) -> Result<u32> {
    u32::try_from(v).map_err(|_| bad(W, v, format!("{what} overflows 32 bits")))
}

/// A value as an integer of the field's width (ReSkate's `number<T>`): out of range is an error.
fn int(v: Option<&Value>, lo: i128, hi: i128) -> Result<i128> {
    let x = match v {
        Some(Value::Int(i)) => i128::from(*i),
        Some(Value::UInt(u)) => i128::from(*u),
        Some(Value::Float(f)) => {
            if !f.is_finite() {
                return Err(bad(W, 0, "floating-point field must be finite"));
            }
            *f as i128
        }
        Some(Value::Bool(b)) => i128::from(*b),
        _ => 0,
    };
    if !(lo..=hi).contains(&x) {
        return Err(bad(W, 0, format!("integer {x} exceeds its field's width")));
    }
    Ok(x)
}

fn float(v: Option<&Value>, single: bool) -> Result<f64> {
    let x = match v {
        Some(Value::Float(f)) => *f,
        Some(Value::Int(i)) => *i as f64,
        Some(Value::UInt(u)) => *u as f64,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    };
    if !x.is_finite() || (single && x.abs() > f64::from(f32::MAX)) {
        return Err(bad(
            W,
            0,
            format!("floating-point value {x} does not fit its field"),
        ));
    }
    Ok(x)
}

struct PendingArray<'a> {
    slot: u32,
    values: &'a [Value],
    descriptor: usize,
    hash: Option<u32>,
    holder: usize,
    emitted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Array,
    Str,
    Instance,
    Boxed,
    Empty,
    Unknown,
}

struct Builder<'a> {
    doc: &'a Document,
    payload: Vec<u8>,
    instance_offsets: Vec<u32>,
    pointer_offsets: Vec<u32>,
    resource_offsets: Vec<u32>,
    import_offsets: Vec<u32>,
    type_info_offsets: Vec<u32>,
    arrays_pending: Vec<PendingArray<'a>>,
    strings_pending: Vec<(u32, Vec<u8>)>,
    pointers_pending: Vec<(u32, Pointer)>,
    boxes_pending: Vec<(u32, BoxedRef)>,
    arrays: Vec<ArrayRecord>,
    boxed: Vec<BoxedValueRecord>,
    arrays_offset: u32,
    boxed_offset: u32,
    empty_array_target: u32,
    old_arrays: BTreeMap<u32, ArrayRecord>,
    old_instances: BTreeMap<u32, usize>,
    carried: BTreeMap<u32, u32>,
    boxed_targets: Vec<(u32, u32)>,
    known_walked: bool,
    known_hashes: BTreeMap<(usize, usize), u32>,
}

impl<'a> Builder<'a> {
    fn run(mut self) -> Result<Vec<u8>> {
        self.validate()?;
        self.write_instances()?;
        self.write_arrays()?;
        self.write_boxed_values()?;
        let string_offset = u32_of(self.payload.len(), "string offset")?;
        self.write_strings()?;
        self.resolve_pointers()?;
        let payload = std::mem::take(&mut self.payload);
        let fixup = self.write_fixup(string_offset)?;
        let extra = self.write_extra()?;
        self.write_riff(&payload, &fixup, &extra)
    }

    fn validate(&self) -> Result<()> {
        let d = self.doc;
        if d.reflection_data.is_empty() {
            return Err(bad(W, 0, "no donor reflection table"));
        }
        if d.fixup_type_signatures.len() > d.fixup_type_guids.len() {
            return Err(bad(W, 0, "fixup type table is invalid"));
        }
        let mut internal_seen = false;
        for (i, inst) in d.instances.iter().enumerate() {
            internal_seen |= !inst.exported;
            if internal_seen && inst.exported {
                return Err(bad(W, i, "exported instances must precede internal ones"));
            }
            if inst.descriptor.is_none_or(|t| t >= d.types.len()) {
                return Err(bad(W, i, "instance has no valid type descriptor"));
            }
            if usize::from(inst.fixup_type) >= d.fixup_type_guids.len() {
                return Err(bad(W, i, "instance has no valid fixup type"));
            }
        }
        Ok(())
    }

    fn grow(&mut self, end: usize) {
        if end > self.payload.len() {
            self.payload.resize(end, 0);
        }
    }
    fn put(&mut self, at: usize, b: &[u8]) {
        self.grow(at + b.len());
        self.payload[at..at + b.len()].copy_from_slice(b);
    }
    fn put_u16(&mut self, at: usize, v: u16) {
        self.put(at, &v.to_le_bytes());
    }
    fn put_u32(&mut self, at: usize, v: u32) {
        self.put(at, &v.to_le_bytes());
    }
    fn put_u64(&mut self, at: usize, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    fn write_instances(&mut self) -> Result<()> {
        let doc = self.doc;
        let mut cursor = 0usize;
        for inst in &doc.instances {
            let ti = inst
                .descriptor
                .ok_or_else(|| bad(W, 0, "instance without a type"))?;
            let t = &doc.types[ti];
            let start = align_up(
                cursor + if inst.exported { 16 } else { 0 },
                usize::from(t.alignment).max(1),
            );
            if inst.exported {
                self.put(start - 16, &inst.guid);
            }
            self.grow(start + usize::from(t.size));
            if !inst.raw_image.is_empty() {
                let n = inst.raw_image.len().min(usize::from(t.size));
                self.put(start, &inst.raw_image[..n]);
            }
            self.put_u16(start, inst.fixup_type);
            self.write_type(ti, start, inst.object.as_ref())?;
            self.instance_offsets.push(u32_of(start, "instance offset")?);
            cursor = start + usize::from(t.size);
        }
        Ok(())
    }

    fn value_for(object: Option<&'a Object>, descriptor: usize) -> Option<&'a Value> {
        object?
            .fields
            .iter()
            .find(|f| f.descriptor == descriptor)
            .map(|f| &f.value)
    }

    fn write_type(&mut self, ti: usize, start: usize, object: Option<&'a Object>) -> Result<()> {
        let doc = self.doc;
        let t = &doc.types[ti];
        self.grow(start + usize::from(t.size));
        let first = usize::try_from(t.field_index).map_err(|_| bad(W, start, "field range is invalid"))?;
        for k in 0..usize::from(t.field_count) {
            let di = first + k;
            let f = doc
                .fields
                .get(di)
                .ok_or_else(|| bad(W, start, "field range is invalid"))?;
            if f.ty() == FieldType::Inherited {
                if usize::from(f.class_ref) >= doc.types.len() {
                    return Err(bad(W, start, "base type is invalid"));
                }
                self.write_type(usize::from(f.class_ref), start, object)?;
                continue;
            }
            let position = start + f.data_offset as usize;
            let value = Self::value_for(object, di);
            if f.is_array() {
                self.grow(position + 8);
                let slot = u32_of(position, "array slot")?;
                self.pointer_offsets.push(slot);
                let (values, hash): (&'a [Value], Option<u32>) = match value {
                    Some(Value::Array(v, h)) => (v.as_slice(), *h),
                    _ => (&[], None),
                };
                self.arrays_pending.push(PendingArray {
                    slot,
                    values,
                    descriptor: di,
                    hash,
                    holder: object.map_or(ti, |o| o.descriptor),
                    emitted: false,
                });
            } else if f.ty() == FieldType::Structure {
                let nt = doc
                    .types
                    .get(usize::from(f.class_ref))
                    .ok_or_else(|| bad(W, position, "struct type is invalid"))?;
                let nested_start = align_up(position, usize::from(nt.alignment).max(1));
                let nested = match value {
                    Some(Value::Object(o)) => Some(o.as_ref()),
                    _ => None,
                };
                self.write_type(usize::from(f.class_ref), nested_start, nested)?;
            } else {
                self.write_scalar(f, position, value)?;
            }
        }
        Ok(())
    }

    fn write_scalar(&mut self, f: &FieldDescriptor, position: usize, v: Option<&Value>) -> Result<()> {
        use FieldType::*;
        match f.ty() {
            Boolean => {
                let b = match v {
                    Some(Value::Bool(b)) => u8::from(*b),
                    other => u8::from(int(other, i128::MIN, i128::MAX)? != 0),
                };
                self.put(position, &[b]);
            }
            Int8 => self.put(position, &(int(v, -128, 127)? as i8).to_le_bytes()),
            UInt8 => self.put(position, &[int(v, 0, 255)? as u8]),
            Int16 => self.put(position, &(int(v, -32768, 32767)? as i16).to_le_bytes()),
            UInt16 => self.put_u16(position, int(v, 0, 65535)? as u16),
            Int32 | Enumeration => self.put(
                position,
                &(int(v, i128::from(i32::MIN), i128::from(i32::MAX))? as i32).to_le_bytes(),
            ),
            UInt32 => self.put_u32(position, int(v, 0, i128::from(u32::MAX))? as u32),
            Int64 => self.put(
                position,
                &(int(v, i128::from(i64::MIN), i128::from(i64::MAX))? as i64).to_le_bytes(),
            ),
            UInt64 => self.put_u64(position, int(v, 0, i128::from(u64::MAX))? as u64),
            Float32 => self.put(position, &(float(v, true)? as f32).to_le_bytes()),
            Float64 => self.put(position, &float(v, false)?.to_le_bytes()),
            Guid => {
                let g = match v {
                    Some(Value::Guid(g)) => *g,
                    _ => [0; 16],
                };
                self.put(position, &g);
            }
            Sha1 => {
                let s = match v {
                    Some(Value::Sha1(s)) => *s,
                    _ => [0; 20],
                };
                self.put(position, &s);
            }
            String => {
                let mut b = [0u8; 32];
                if let Some(Value::Str(s)) = v {
                    if s.len() > 31 {
                        return Err(bad(W, position, "fixed string exceeds 31 bytes"));
                    }
                    b[..s.len()].copy_from_slice(s);
                }
                self.put(position, &b);
            }
            CString | FileRef => {
                self.put_u64(position, 0);
                let slot = u32_of(position, "string slot")?;
                self.pointer_offsets.push(slot);
                let s = match v {
                    Some(Value::Str(s)) => s.clone(),
                    _ => Vec::new(),
                };
                if s.len() > 1 << 20 {
                    return Err(bad(W, position, "string exceeds the authoring limit"));
                }
                self.strings_pending.push((slot, s));
            }
            ResourceRef => {
                let id = match v {
                    Some(Value::Resource(r)) => *r,
                    _ => 0,
                };
                self.put_u64(position, id);
                self.resource_offsets.push(u32_of(position, "resource slot")?);
            }
            Pointer => {
                self.put_u64(position, 0);
                let p = match v {
                    Some(Value::Pointer(p)) => *p,
                    _ => super::Pointer::Null,
                };
                self.pointers_pending.push((u32_of(position, "pointer slot")?, p));
            }
            TypeRef | Delegate => {
                self.grow(position + 8);
                let encoded = match v {
                    Some(Value::TypeRef(t)) => t.encoded,
                    _ => 0,
                };
                self.put_u32(position, encoded);
                self.put_u32(position + 4, 0);
                if encoded != 0 && f.ty() == TypeRef {
                    self.type_info_offsets.push(u32_of(position, "type-info slot")?);
                }
            }
            BoxedValueRef => {
                self.grow(position + 16);
                match v {
                    Some(Value::Boxed(b)) if b.encoded_type != 0 => {
                        self.put_u32(position, b.encoded_type);
                        self.put_u32(position + 4, 0);
                        self.type_info_offsets.push(u32_of(position, "boxed type slot")?);
                        self.pointer_offsets
                            .push(u32_of(position + 8, "boxed pointer slot")?);
                        self.boxes_pending.push((u32_of(position, "boxed slot")?, *b));
                    }
                    _ => self.put(position, &[0; 16]),
                }
            }
            other => self.grow(position + scalar_size(other)),
        }
        Ok(())
    }

    fn element_stride(&self, f: &FieldDescriptor) -> Result<usize> {
        if f.ty() == FieldType::Structure {
            let t = self
                .doc
                .types
                .get(usize::from(f.class_ref))
                .ok_or_else(|| bad(W, 0, "array struct type is invalid"))?;
            return Ok(usize::from(t.size).max(1));
        }
        Ok(scalar_size(f.ty()))
    }

    fn element_alignment(&self, f: &FieldDescriptor) -> Result<usize> {
        if f.ty() == FieldType::Structure {
            return Ok(usize::from(self.doc.types[usize::from(f.class_ref)].alignment).max(1));
        }
        Ok(self.element_stride(f)?.clamp(1, 8))
    }

    fn fixup_slot_for_descriptor(&self, descriptor: u16) -> u16 {
        let d = self.doc;
        let Some(t) = d.types.get(usize::from(descriptor)) else {
            return 0xFFFF;
        };
        let n = d.fixup_type_guids.len().min(d.fixup_type_signatures.len());
        (0..n)
            .find(|&i| d.fixup_type_guids[i] == t.guid && d.fixup_type_signatures[i] == t.signature)
            .map_or(0xFFFF, |i| i as u16)
    }

    fn array_hash(&self, f: &FieldDescriptor) -> u32 {
        let raw = f.encoded_type << 1;
        let class_ref = if f.ty() == FieldType::Structure {
            self.fixup_slot_for_descriptor(f.class_ref)
        } else {
            0xFFFF
        };
        if let Some(a) = self
            .doc
            .arrays
            .iter()
            .find(|a| a.encoded_type == raw && a.class_ref == class_ref)
        {
            return a.hash;
        }
        self.doc.arrays.first().map_or(f.name_hash, |a| a.hash)
    }

    fn new_array_hash(&mut self, index: usize, f: &FieldDescriptor) -> u32 {
        if !self.known_walked {
            self.known_walked = true;
            let doc = self.doc;
            for inst in &doc.instances {
                if let Some(o) = &inst.object {
                    collect_hashes(o, &mut self.known_hashes);
                }
            }
        }
        let key = (
            self.arrays_pending[index].holder,
            self.arrays_pending[index].descriptor,
        );
        let found = self.known_hashes.get(&key).copied();
        found.unwrap_or_else(|| self.array_hash(f))
    }

    fn write_arrays(&mut self) -> Result<()> {
        self.arrays_offset = u32_of(align_up(self.payload.len(), 16), "array section")?;
        self.grow(self.arrays_offset as usize + 32);
        self.empty_array_target = self.arrays_offset + 16;
        let mut i = 0;
        while i < self.arrays_pending.len() {
            if !self.arrays_pending[i].emitted {
                self.emit_array(i)?;
            }
            i += 1;
        }
        self.carry_boxed_arrays()
    }

    fn old_target(&self, slot: u32) -> Result<(Target, u32)> {
        let old = &self.doc.payload;
        let s = slot as usize;
        let disp = old
            .get(s..s + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| bad(W, s, "carried slot lies outside the payload"))?;
        let target = (i64::from(slot) + i64::from(disp)) as u32;
        if self.old_arrays.contains_key(&target) {
            return Ok((Target::Array, target));
        }
        if self.doc.strings_offset <= target && (target as usize) < old.len() {
            return Ok((Target::Str, target));
        }
        if let Some(&i) = self.old_instances.get(&target) {
            return Ok((Target::Instance, i as u32));
        }
        if self.doc.boxed_values_offset <= target && target < self.doc.strings_offset {
            return Ok((Target::Boxed, target));
        }
        let t = target as usize;
        if t >= 4 && t <= old.len() && old[t - 4..t] == [0, 0, 0, 0] {
            return Ok((Target::Empty, target));
        }
        Ok((Target::Unknown, target))
    }

    fn old_slots(start: u32, length: usize, offsets: &[u32]) -> Vec<u32> {
        let mut v: Vec<u32> = offsets
            .iter()
            .copied()
            .filter(|&o| o >= start && ((o - start) as usize) < length)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn carry_boxed_arrays(&mut self) -> Result<()> {
        let doc = self.doc;
        if doc.payload.is_empty() {
            return Ok(());
        }
        for a in &doc.arrays {
            self.old_arrays.entry(a.offset).or_insert(*a);
        }
        for (i, inst) in doc.instances.iter().enumerate() {
            self.old_instances.entry(inst.data_offset).or_insert(i);
        }
        let mut records: Vec<&BoxedValueRecord> = doc.boxed_values.iter().collect();
        records.sort_by_key(|b| b.offset);
        for src in records {
            for slot in Self::old_slots(src.offset, src.raw.len(), &doc.pointer_offsets) {
                let (kind, key) = self.old_target(slot)?;
                if kind == Target::Array {
                    let rec = self.old_arrays[&key];
                    self.carry_array(rec)?;
                }
            }
        }
        Ok(())
    }

    fn array_element(&self, rec: &ArrayRecord) -> Result<(usize, usize)> {
        let ty = FieldType::from_bits(((rec.encoded_type >> 1) >> 4) & 0x1F);
        if ty == FieldType::Structure {
            let d = self.doc;
            let slot = usize::from(rec.class_ref);
            if slot < d.fixup_type_guids.len() && slot < d.fixup_type_signatures.len() {
                for t in &d.types {
                    if t.guid == d.fixup_type_guids[slot] && t.signature == d.fixup_type_signatures[slot] {
                        return Ok((usize::from(t.size).max(1), usize::from(t.alignment).max(1)));
                    }
                }
            }
            return Err(bad(
                W,
                rec.offset as usize,
                "array record has an unknown struct type",
            ));
        }
        let stride = scalar_size(ty);
        Ok((stride, stride.clamp(1, 8)))
    }

    fn carry_array(&mut self, rec: ArrayRecord) -> Result<()> {
        if self.carried.contains_key(&rec.offset) {
            return Ok(());
        }
        let (stride, alignment) = self.array_element(&rec)?;
        let length = stride * rec.count as usize;
        let from = rec.offset as usize;
        if from + length > self.doc.payload.len() {
            return Err(bad(W, from, "carried array exceeds EBXD"));
        }
        let data = align_up(self.payload.len() + 4, alignment);
        self.put_u32(data - 4, rec.count);
        let doc = self.doc;
        self.put(data, &doc.payload[from..from + length]);
        let placed = u32_of(data, "array data")?;
        self.carried.insert(rec.offset, placed);
        self.arrays.push(ArrayRecord {
            offset: placed,
            ..rec
        });
        self.relocate(rec.offset, length, data)
    }

    fn relocate(&mut self, old_start: u32, length: usize, new_start: usize) -> Result<()> {
        let doc = self.doc;
        let moved = |old: u32| u32_of(new_start + (old - old_start) as usize, "carried relocation");
        for o in Self::old_slots(old_start, length, &doc.resource_ref_offsets) {
            self.resource_offsets.push(moved(o)?);
        }
        for o in Self::old_slots(old_start, length, &doc.import_offsets) {
            self.import_offsets.push(moved(o)?);
        }
        for o in Self::old_slots(old_start, length, &doc.type_info_offsets) {
            self.type_info_offsets.push(moved(o)?);
        }
        for o in Self::old_slots(old_start, length, &doc.pointer_offsets) {
            let slot = moved(o)?;
            self.pointer_offsets.push(slot);
            if doc.payload.is_empty() {
                continue;
            }
            let (kind, key) = self.old_target(o)?;
            match kind {
                Target::Array => {
                    let rec = self.old_arrays[&key];
                    self.carry_array(rec)?;
                    let to = self.carried[&key];
                    self.put_u32(slot as usize, to.wrapping_sub(slot));
                }
                Target::Str => {
                    let src = &doc.payload;
                    let k = key as usize;
                    let end = src[k..].iter().position(|&b| b == 0).map_or(src.len(), |n| k + n);
                    self.strings_pending.push((slot, src[k..end].to_vec()));
                }
                Target::Instance => {
                    let to = self.instance_offsets[key as usize];
                    self.put_u32(slot as usize, to.wrapping_sub(slot));
                }
                Target::Empty => {
                    let to = self.empty_array_target;
                    self.put_u32(slot as usize, to.wrapping_sub(slot));
                }
                Target::Boxed => self.boxed_targets.push((slot, key)),
                Target::Unknown => {}
            }
        }
        Ok(())
    }

    fn emit_array(&mut self, index: usize) -> Result<()> {
        self.arrays_pending[index].emitted = true;
        let slot = self.arrays_pending[index].slot;
        let values = self.arrays_pending[index].values;
        let descriptor = self.arrays_pending[index].descriptor;
        let doc = self.doc;
        let f = &doc.fields[descriptor];
        if values.is_empty() {
            let to = self.empty_array_target;
            self.put_u32(slot as usize, to.wrapping_sub(slot));
            return Ok(());
        }
        let data = align_up(self.payload.len() + 4, self.element_alignment(f)?);
        let stride = self.element_stride(f)?;
        let count = u32_of(values.len(), "array count")?;
        self.put_u32(data - 4, count);
        self.grow(data + stride * values.len());
        self.put_u32(slot as usize, (data as u32).wrapping_sub(slot));
        let first_child = self.arrays_pending.len();
        for (e, value) in values.iter().enumerate() {
            let at = data + e * stride;
            if f.ty() == FieldType::Structure {
                let o = match value {
                    Value::Object(o) => Some(o.as_ref()),
                    _ => None,
                };
                self.write_type(usize::from(f.class_ref), at, o)?;
            } else {
                self.write_scalar(f, at, Some(value))?;
            }
        }
        let hash = match self.arrays_pending[index].hash {
            Some(h) => h,
            None => self.new_array_hash(index, f),
        };
        let class_ref = if f.ty() == FieldType::Structure {
            self.fixup_slot_for_descriptor(f.class_ref)
        } else {
            0xFFFF
        };
        self.arrays.push(ArrayRecord {
            offset: u32_of(data, "array data")?,
            count,
            hash,
            encoded_type: f.encoded_type << 1,
            class_ref,
        });
        let mut child = first_child;
        while child < self.arrays_pending.len() {
            if !self.arrays_pending[child].emitted {
                self.emit_array(child)?;
            }
            child += 1;
        }
        Ok(())
    }

    fn write_boxed_values(&mut self) -> Result<()> {
        let doc = self.doc;
        self.boxed_offset = u32_of(align_up(self.payload.len(), 16), "boxed section")?;
        self.grow(self.boxed_offset as usize);
        let mut moved: BTreeMap<u32, u32> = BTreeMap::new();
        let mut records: Vec<&BoxedValueRecord> = doc.boxed_values.iter().collect();
        records.sort_by_key(|b| b.offset);
        for src in records {
            if src.offset < doc.boxed_values_offset {
                return Err(bad(W, src.offset as usize, "boxed record precedes its section"));
            }
            let offset = self.boxed_offset as usize + (src.offset - doc.boxed_values_offset) as usize;
            self.put(offset, &src.raw);
            self.relocate(src.offset, src.raw.len(), offset)?;
            let at = u32_of(offset, "boxed value")?;
            moved.insert(src.offset, at);
            self.boxed.push(BoxedValueRecord {
                offset: at,
                ..src.clone()
            });
        }
        for (slot, target) in std::mem::take(&mut self.boxed_targets) {
            let to = self.boxed_offset + (target - doc.boxed_values_offset);
            self.put_u32(slot as usize, to.wrapping_sub(slot));
        }
        for (slot, b) in self.boxes_pending.clone() {
            self.put_u32(slot as usize, b.encoded_type);
            self.put_u32(slot as usize + 4, 0);
            let rel = match u32::try_from(b.data_offset).ok().and_then(|o| moved.get(&o)) {
                Some(&to) => i64::from(to) - i64::from(slot + 8),
                None => b.relative_offset,
            };
            self.put(slot as usize + 8, &rel.to_le_bytes());
        }
        Ok(())
    }

    fn write_strings(&mut self) -> Result<()> {
        let mut offsets: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        for (slot, s) in std::mem::take(&mut self.strings_pending) {
            let at = match offsets.get(&s) {
                Some(&o) => o,
                None => {
                    let o = u32_of(self.payload.len(), "string table")?;
                    let end = self.payload.len();
                    self.put(end, &s);
                    let end = self.payload.len();
                    self.grow(end + 1);
                    offsets.insert(s, o);
                    o
                }
            };
            self.put_u32(slot as usize, at.wrapping_sub(slot));
            self.put_u32(slot as usize + 4, 0);
        }
        Ok(())
    }

    fn resolve_pointers(&mut self) -> Result<()> {
        for (slot, p) in std::mem::take(&mut self.pointers_pending) {
            match p {
                Pointer::Null => {}
                Pointer::Internal(i) => {
                    let to = *self
                        .instance_offsets
                        .get(i)
                        .ok_or_else(|| bad(W, slot as usize, "internal pointer index is invalid"))?;
                    self.put_u32(slot as usize, to.wrapping_sub(slot));
                    self.pointer_offsets.push(slot);
                }
                Pointer::External(i) => {
                    if i >= self.doc.imports.len() {
                        return Err(bad(W, slot as usize, "import pointer index is invalid"));
                    }
                    self.put_u32(slot as usize, (u32_of(i, "import index")? << 1) | 1);
                    self.import_offsets.push(slot);
                }
            }
            self.put_u32(slot as usize + 4, 0);
        }
        Ok(())
    }

    fn table(out: &mut Vec<u8>, values: &[u32]) -> Result<()> {
        let mut v = values.to_vec();
        v.sort_unstable();
        v.dedup();
        out.extend(u32_of(v.len(), "fixup table count")?.to_le_bytes());
        for x in v {
            out.extend(x.to_le_bytes());
        }
        Ok(())
    }

    fn write_fixup(&self, string_offset: u32) -> Result<Vec<u8>> {
        let d = self.doc;
        let mut w = Vec::new();
        w.extend(d.file_guid);
        w.extend(u32_of(d.fixup_type_guids.len(), "type count")?.to_le_bytes());
        for g in &d.fixup_type_guids {
            w.extend(g);
        }
        w.extend(u32_of(d.fixup_type_signatures.len(), "signature count")?.to_le_bytes());
        for s in &d.fixup_type_signatures {
            w.extend(s.to_le_bytes());
        }
        let exported = d.instances.iter().filter(|i| i.exported).count();
        w.extend(u32_of(exported, "exported count")?.to_le_bytes());
        Self::table(&mut w, &self.instance_offsets)?;
        Self::table(&mut w, &self.pointer_offsets)?;
        Self::table(&mut w, &self.resource_offsets)?;
        w.extend(u32_of(d.imports.len(), "import count")?.to_le_bytes());
        for i in &d.imports {
            w.extend(i.file_guid);
            w.extend(i.class_guid);
        }
        Self::table(&mut w, &self.import_offsets)?;
        Self::table(&mut w, &self.type_info_offsets)?;
        w.extend(self.arrays_offset.to_le_bytes());
        w.extend(self.boxed_offset.to_le_bytes());
        w.extend(string_offset.to_le_bytes());
        let mut old_slots = d.boxed_value_slots.clone();
        old_slots.sort_unstable();
        let mut new_slots: Vec<u32> = self.boxes_pending.iter().map(|b| b.0).collect();
        new_slots.sort_unstable();
        let mut trailing = Vec::new();
        for &old in &d.boxed_reference_offsets {
            if let Ok(i) = old_slots.binary_search(&old) {
                // the first of equal slots, as lower_bound finds it
                let i = old_slots[..=i].partition_point(|&s| s < old);
                if let Some(&n) = new_slots.get(i) {
                    trailing.push(n);
                }
            }
        }
        Self::table(&mut w, &trailing)?;
        Ok(w)
    }

    fn write_extra(&self) -> Result<Vec<u8>> {
        let mut w = Vec::new();
        w.extend(u32_of(self.arrays.len(), "array descriptor count")?.to_le_bytes());
        w.extend(u32_of(self.boxed.len(), "boxed descriptor count")?.to_le_bytes());
        let mut arrays = self.arrays.clone();
        arrays.sort_by_key(|a| a.offset);
        for a in arrays {
            w.extend(a.offset.to_le_bytes());
            w.extend(a.count.to_le_bytes());
            w.extend(a.hash.to_le_bytes());
            w.extend(a.encoded_type.to_le_bytes());
            w.extend(a.class_ref.to_le_bytes());
        }
        let mut boxed = self.boxed.clone();
        boxed.sort_by_key(|b| b.offset);
        for b in boxed {
            w.extend(b.offset.to_le_bytes());
            w.extend(b.count.to_le_bytes());
            w.extend(b.hash.to_le_bytes());
            w.extend(b.encoded_type.to_le_bytes());
            w.extend(b.class_ref.to_le_bytes());
        }
        Ok(w)
    }

    fn write_riff(&self, payload: &[u8], fixup: &[u8], extra: &[u8]) -> Result<Vec<u8>> {
        let d = self.doc;
        let mut w =
            Vec::with_capacity(payload.len() + fixup.len() + extra.len() + d.reflection_data.len() + 64);
        w.extend(b"RIFF");
        w.extend([0; 4]);
        w.extend(if d.serialized_layout { b"EBXS" } else { b"EBX\0" });
        let chunk = |w: &mut Vec<u8>, name: &[u8; 4], bytes: &[u8]| -> Result<()> {
            w.extend(name);
            w.extend(u32_of(bytes.len(), "RIFF chunk size")?.to_le_bytes());
            w.extend(bytes);
            if bytes.len() & 1 != 0 {
                w.push(0);
            }
            Ok(())
        };
        let mut data = vec![0u8; 12];
        data.extend(payload);
        chunk(&mut w, b"EBXD", &data)?;
        chunk(&mut w, b"EFIX", fixup)?;
        chunk(&mut w, b"EBXX", extra)?;
        chunk(
            &mut w,
            if d.hashed_reflection_names {
                b"RFL2"
            } else {
                b"REFL"
            },
            &d.reflection_data,
        )?;
        let size = u32_of(w.len() - 8, "RIFF size")?;
        w[4..8].copy_from_slice(&size.to_le_bytes());
        Ok(w)
    }
}

fn collect_hashes(o: &Object, out: &mut BTreeMap<(usize, usize), u32>) {
    for f in &o.fields {
        if let Value::Array(_, Some(h)) = &f.value {
            out.entry((o.descriptor, f.descriptor)).or_insert(*h);
        }
        collect_value(&f.value, out);
    }
}

fn collect_value(v: &Value, out: &mut BTreeMap<(usize, usize), u32>) {
    match v {
        Value::Object(o) => collect_hashes(o, out),
        Value::Array(items, _) => items.iter().for_each(|x| collect_value(x, out)),
        _ => {}
    }
}

/// Writes `doc` with the layout the reference writer produces. A new array (one without a hash of its own) takes the
/// hash of an array of the same field, or else of an array of its element type.
pub fn write(doc: &Document) -> Result<Vec<u8>> {
    Builder {
        doc,
        payload: Vec::new(),
        instance_offsets: Vec::new(),
        pointer_offsets: Vec::new(),
        resource_offsets: Vec::new(),
        import_offsets: Vec::new(),
        type_info_offsets: Vec::new(),
        arrays_pending: Vec::new(),
        strings_pending: Vec::new(),
        pointers_pending: Vec::new(),
        boxes_pending: Vec::new(),
        arrays: Vec::new(),
        boxed: Vec::new(),
        arrays_offset: 0,
        boxed_offset: 0,
        empty_array_target: 0,
        old_arrays: BTreeMap::new(),
        old_instances: BTreeMap::new(),
        carried: BTreeMap::new(),
        boxed_targets: Vec::new(),
        known_walked: false,
        known_hashes: BTreeMap::new(),
    }
    .run()
}
