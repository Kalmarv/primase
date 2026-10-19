//! Moving values between documents and tidying a document after an edit (ReSkate's `ebx_merge.cpp` does the same
//! when it merges two documents' edits of one asset).
use std::collections::{BTreeMap, BTreeSet};

use super::{Document, FieldType, FieldValue, Instance, Object, Pointer, Value};
use crate::{Result, bad};

const W: &str = "EBX carry";
/// The object id bits of an instance's `Flags` (the low bits of its GUID).
pub const OBJECT_ID_MASK: u32 = 0x01FF_FFFF;

/// The EFIX type slot of `doc`'s type `descriptor`, added when the document has none (ReSkate's `ensure_fixup_type`).
pub fn ensure_fixup_type(doc: &mut Document, descriptor: usize) -> Result<u16> {
    let t = doc
        .types
        .get(descriptor)
        .ok_or_else(|| bad(W, 0, "type descriptor is invalid"))?;
    let (g, s) = (t.guid, t.signature);
    let n = doc.fixup_type_guids.len().min(doc.fixup_type_signatures.len());
    if let Some(i) = (0..n).find(|&i| doc.fixup_type_guids[i] == g && doc.fixup_type_signatures[i] == s) {
        return u16::try_from(i).map_err(|_| bad(W, 0, "type slot overflow"));
    }
    doc.fixup_type_guids.insert(n, g);
    doc.fixup_type_signatures.push(s);
    u16::try_from(n).map_err(|_| bad(W, 0, "type slot overflow"))
}

/// Every pointer inside `v`, mutably.
pub fn walk_pointers(v: &mut Value, f: &mut dyn FnMut(&mut Pointer)) {
    match v {
        Value::Pointer(p) => f(p),
        Value::Object(o) => o.fields.iter_mut().for_each(|x| walk_pointers(&mut x.value, f)),
        Value::Array(items, _) => items.iter_mut().for_each(|x| walk_pointers(x, f)),
        _ => {}
    }
}

/// Drops the instances nothing reachable from the root (instance 0) points at, and the imports nothing kept uses;
/// remaps every pointer. -> (instances kept, imports kept)
pub fn compact(doc: &mut Document) -> (usize, usize) {
    let mut reach = BTreeSet::new();
    let mut stack = vec![0usize];
    while let Some(k) = stack.pop() {
        if k >= doc.instances.len() || !reach.insert(k) {
            continue;
        }
        if let Some(o) = doc.instances[k].object.as_mut() {
            for fv in &mut o.fields {
                walk_pointers(&mut fv.value, &mut |p| {
                    if let Pointer::Internal(i) = p {
                        stack.push(*i);
                    }
                });
            }
        }
    }
    let keep: Vec<usize> = (0..doc.instances.len()).filter(|k| reach.contains(k)).collect();
    let mut used = BTreeSet::new();
    for &k in &keep {
        if let Some(o) = doc.instances[k].object.as_mut() {
            for fv in &mut o.fields {
                walk_pointers(&mut fv.value, &mut |p| {
                    if let Pointer::External(i) = p {
                        used.insert(*i);
                    }
                });
            }
        }
    }
    let ikeep: Vec<usize> = (0..doc.imports.len()).filter(|i| used.contains(i)).collect();
    let mmap: BTreeMap<usize, usize> = ikeep.iter().enumerate().map(|(n, &o)| (o, n)).collect();
    for &k in &keep {
        if let Some(o) = doc.instances[k].object.as_mut() {
            for fv in &mut o.fields {
                walk_pointers(&mut fv.value, &mut |p| {
                    if let Pointer::External(i) = p {
                        *i = mmap[i];
                    }
                });
            }
        }
    }
    rearrange(doc, &keep);
    let imports = std::mem::take(&mut doc.imports);
    doc.imports = ikeep.iter().map(|&i| imports[i]).collect();
    (keep.len(), ikeep.len())
}

/// Puts the exported instances before the internal ones, as the format requires (instances added to a document go at
/// its end), each group in its own order; remaps every pointer. -> whether any instance moved
pub fn order_exported_first(doc: &mut Document) -> bool {
    let n = doc.instances.len();
    let order: Vec<usize> = (0..n)
        .filter(|&k| doc.instances[k].exported)
        .chain((0..n).filter(|&k| !doc.instances[k].exported))
        .collect();
    if order.iter().enumerate().all(|(n, &k)| n == k) {
        return false;
    }
    rearrange(doc, &order);
    true
}

/// Keeps the instances `order` lists, in that order, and remaps the internal pointers of the kept ones (each must
/// point at a kept instance).
fn rearrange(doc: &mut Document, order: &[usize]) {
    let imap: BTreeMap<usize, usize> = order.iter().enumerate().map(|(n, &o)| (o, n)).collect();
    for &k in order {
        if let Some(o) = doc.instances[k].object.as_mut() {
            for fv in &mut o.fields {
                walk_pointers(&mut fv.value, &mut |p| {
                    if let Pointer::Internal(i) = p {
                        *i = imap[i];
                    }
                });
            }
        }
    }
    let mut old = std::mem::take(&mut doc.instances);
    doc.instances = order
        .iter()
        .map(|&k| std::mem::replace(&mut old[k], placeholder()))
        .collect();
}

fn placeholder() -> Instance {
    Instance {
        fixup_type: 0,
        descriptor: None,
        data_offset: 0,
        exported: false,
        guid: [0; 16],
        object: None,
        raw_image: Vec::new(),
    }
}

/// The field descriptors of `doc`'s type `t` by name, its bases' included.
fn fields_by_name(doc: &Document, t: usize) -> Result<BTreeMap<String, usize>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![t];
    let mut depth = 0;
    while let Some(t) = stack.pop() {
        depth += 1;
        if depth > 128 {
            return Err(bad(W, 0, "types nest too deeply"));
        }
        let ty = doc
            .types
            .get(t)
            .ok_or_else(|| bad(W, 0, "type descriptor is invalid"))?;
        let first = usize::try_from(ty.field_index).map_err(|_| bad(W, 0, "field range is invalid"))?;
        for di in first..first + usize::from(ty.field_count) {
            let f = &doc.fields[di];
            if f.ty() == FieldType::Inherited {
                stack.push(usize::from(f.class_ref));
            } else {
                out.entry(f.name.clone()).or_insert(di);
            }
        }
    }
    Ok(out)
}

/// Carries values from document `from` into document `to`: the instances they point at come along (an exported one
/// only once, by GUID), imports are shared, and types and fields are found in `to` by name.
#[derive(Debug)]
pub struct Carrier<'a> {
    from: &'a Document,
    pub to: &'a mut Document,
    type_by_name: BTreeMap<String, usize>,
    instance_by_guid: BTreeMap<[u8; 16], usize>,
    carried_internal: BTreeMap<usize, usize>,
    fields: BTreeMap<usize, BTreeMap<String, usize>>,
    pub instances: usize,
    pub renumbered: usize,
}

impl<'a> Carrier<'a> {
    pub fn new(from: &'a Document, to: &'a mut Document) -> Self {
        let mut type_by_name = BTreeMap::new();
        for (i, t) in to.types.iter().enumerate() {
            type_by_name.entry(t.name.clone()).or_insert(i);
        }
        let mut instance_by_guid = BTreeMap::new();
        for (i, inst) in to.instances.iter().enumerate() {
            instance_by_guid.entry(inst.guid).or_insert(i);
        }
        Carrier {
            from,
            to,
            type_by_name,
            instance_by_guid,
            carried_internal: BTreeMap::new(),
            fields: BTreeMap::new(),
            instances: 0,
            renumbered: 0,
        }
    }

    fn ty(&self, descriptor: usize) -> Result<usize> {
        let name = &self
            .from
            .types
            .get(descriptor)
            .ok_or_else(|| bad(W, 0, "type descriptor is out of range"))?
            .name;
        self.type_by_name
            .get(name)
            .copied()
            .ok_or_else(|| bad(W, 0, format!("needs a type the target does not define: {name}")))
    }

    fn instance(&mut self, index: usize) -> Result<usize> {
        let from = self.from;
        let src = from
            .instances
            .get(index)
            .ok_or_else(|| bad(W, 0, "pointer is out of range"))?;
        if src.exported {
            if let Some(&at) = self.instance_by_guid.get(&src.guid) {
                return Ok(at);
            }
        } else if let Some(&at) = self.carried_internal.get(&index) {
            return Ok(at);
        }
        let obj = src
            .object
            .as_ref()
            .ok_or_else(|| bad(W, 0, "instance has no parsed object"))?;
        let d = self.ty(src.descriptor.ok_or_else(|| bad(W, 0, "instance has no type"))?)?;
        let fixup = ensure_fixup_type(self.to, d)?;
        self.to.instances.push(Instance {
            fixup_type: fixup,
            descriptor: Some(d),
            data_offset: 0,
            exported: src.exported,
            guid: src.guid,
            object: Some(Object {
                descriptor: d,
                fields: Vec::new(),
            }),
            raw_image: src.raw_image.clone(),
        });
        let at = self.to.instances.len() - 1;
        if src.exported {
            self.instance_by_guid.entry(src.guid).or_insert(at);
        } else {
            self.carried_internal.insert(index, at);
        }
        let mut copied = self.object(obj)?;
        if src.exported && renumber(&mut copied, &src.guid, from) {
            self.renumbered += 1;
        }
        if let Some(o) = self.to.instances[at].object.as_mut() {
            o.fields = copied.fields;
        }
        self.instances += 1;
        Ok(at)
    }

    fn imported(&mut self, index: usize) -> Result<usize> {
        let r = *self
            .from
            .imports
            .get(index)
            .ok_or_else(|| bad(W, 0, "import pointer is out of range"))?;
        if let Some(at) = self.to.imports.iter().position(|x| *x == r) {
            return Ok(at);
        }
        self.to.imports.push(r);
        Ok(self.to.imports.len() - 1)
    }

    pub fn object(&mut self, src: &Object) -> Result<Object> {
        let t = self.ty(src.descriptor)?;
        if !self.fields.contains_key(&t) {
            let m = fields_by_name(self.to, t)?;
            self.fields.insert(t, m);
        }
        let mut out = Object {
            descriptor: t,
            fields: Vec::with_capacity(src.fields.len()),
        };
        for f in &src.fields {
            let descriptor = *self.fields[&t].get(&f.name).ok_or_else(|| {
                bad(
                    W,
                    0,
                    format!("the target's {} has no field {}", self.to.types[t].name, f.name),
                )
            })?;
            out.fields.push(FieldValue {
                descriptor,
                name: f.name.clone(),
                value: self.value(&f.value)?,
            });
        }
        Ok(out)
    }

    pub fn value(&mut self, v: &Value) -> Result<Value> {
        Ok(match v {
            Value::Pointer(Pointer::Internal(i)) => Value::Pointer(Pointer::Internal(self.instance(*i)?)),
            Value::Pointer(Pointer::External(i)) => Value::Pointer(Pointer::External(self.imported(*i)?)),
            Value::Object(o) => Value::Object(Box::new(self.object(o)?)),
            Value::Array(items, h) => {
                Value::Array(items.iter().map(|x| self.value(x)).collect::<Result<_>>()?, *h)
            }
            Value::TypeRef(t) if !t.primitive => {
                let mut t = *t;
                if let Some(d) = t.descriptor {
                    t.descriptor = Some(self.ty(d)?);
                }
                Value::TypeRef(t)
            }
            Value::Boxed(_) => return Err(bad(W, 0, "cannot carry a boxed value yet")),
            other => other.clone(),
        })
    }
}

fn object_id(guid: &[u8; 16]) -> u32 {
    u32::from_le_bytes([guid[0], guid[1], guid[2], guid[3]])
}

/// An exported instance carried from another document gets the object id of its own GUID in its `Flags`, when the
/// id it had named one of the source's exported instances. -> whether it changed
fn renumber(obj: &mut Object, guid: &[u8; 16], source: &Document) -> bool {
    let Some(f) = obj.fields.iter_mut().find(|f| f.name == "Flags") else {
        return false;
    };
    let current = match f.value {
        Value::Int(i) => i as u32,
        Value::UInt(u) => u as u32,
        _ => return false,
    };
    let new = (current & !OBJECT_ID_MASK) | (object_id(guid) & OBJECT_ID_MASK);
    if new == current {
        return false;
    }
    let cloned = source
        .instances
        .iter()
        .any(|i| i.exported && (object_id(&i.guid) & OBJECT_ID_MASK) == (current & OBJECT_ID_MASK));
    if !cloned {
        return false;
    }
    f.value = match f.value {
        Value::Int(_) => Value::Int(i64::from(new as i32)),
        _ => Value::UInt(u64::from(new)),
    };
    true
}
