//! A document's typed values as JSON, for showing and golden tests: every instance with its type and the values of its
//! fields in stored order, each value as the document holds it. Nothing is dropped: a string that is not UTF-8 is its
//! bytes, a non-finite float is an error (JSON has none), a pointer is the instance or import it names.
//!
//! [`to_json`] is that view. [`to_json_with`] adds, by [`JsonOptions`], what the document alone can say or a table of the
//! install can: the member an enumeration value names, the name of the document an import points at, and the file
//! offset of every field (for a hex editor beside the dump).

use primase_core::JsonError;
use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq, Serializer};

use frostbite_formats::ebx::{Document, FieldType, Instance, Object, Pointer, Value};
use frostbite_formats::hex;
use frostbite_formats::names::GuidNames;

/// What [`to_json_with`] adds to the view of [`to_json`]; the default adds nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonOptions<'a> {
    /// An enumeration value shows as `{"value": n, "member": name}` (`member` is null when no member has the value).
    pub enum_members: bool,
    /// An import shows the name of the document it points at, `name`, where the table has its file GUID.
    pub import_names: Option<&'a GuidNames>,
    /// The document shows its `data_start` (the file offset of the EBXD payload), every instance its `offset` and, beside
    /// `values`, `offsets`: the file offset of each field, by name; so does every nested object. An offset is where the
    /// field's slot lies; for an array it is the slot that holds the displacement to the elements.
    pub offsets: bool,
}

/// The document as JSON: `file_guid`, `root_type`, `imports` and `instances` (each its `type`, whether it is
/// `exported` with its `guid`, and its `values`). Hexadecimal text is the bytes as stored.
pub fn to_json(document: &Document) -> Result<serde_json::Value, JsonError> {
    to_json_with(document, &JsonOptions::default())
}

/// [`to_json`] with what `options` adds.
pub fn to_json_with(document: &Document, options: &JsonOptions<'_>) -> Result<serde_json::Value, JsonError> {
    primase_core::json::to_value(&DocumentView(Ctx {
        d: document,
        o: *options,
    }))
}

/// The document and the options a view is made with.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    d: &'a Document,
    o: JsonOptions<'a>,
}

impl Ctx<'_> {
    /// The file offset of the payload offset `payload`.
    fn file_offset(&self, payload: usize) -> usize {
        self.d.data_start.saturating_add(payload)
    }
}

struct DocumentView<'a>(Ctx<'a>);
struct ImportsView<'a>(Ctx<'a>);
struct InstancesView<'a>(Ctx<'a>);
struct InstanceView<'a>(Ctx<'a>, usize, &'a Instance);
/// An object's values.
struct ObjectView<'a>(Ctx<'a>, &'a Object);
/// The file offsets of an object's fields; the payload offset the object starts at.
struct OffsetsView<'a>(Ctx<'a>, &'a Object, usize);
/// The offsets of one field's value: the slot's payload offset, and the value.
struct SlotView<'a>(Ctx<'a>, &'a Value, usize);
/// A value and the document field it is a value of.
struct ValueView<'a>(Ctx<'a>, &'a Value, Option<usize>);
struct Text<'a>(&'a [u8]);

impl Serialize for DocumentView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0.d;
        let mut m = s.serialize_map(Some(4))?;
        m.serialize_entry("file_guid", &hex(&d.file_guid))?;
        m.serialize_entry("root_type", &d.root_type)?;
        if self.0.o.offsets {
            m.serialize_entry("data_start", &d.data_start)?;
        }
        m.serialize_entry("imports", &ImportsView(self.0))?;
        m.serialize_entry("instances", &InstancesView(self.0))?;
        m.end()
    }
}

impl Serialize for ImportsView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Ctx { d, o } = self.0;
        let mut seq = s.serialize_seq(Some(d.imports.len()))?;
        for i in &d.imports {
            let mut v = serde_json::json!({
                "file_guid": hex(&i.file_guid),
                "class_guid": hex(&i.class_guid),
            });
            if let (Some(names), Some(m)) = (o.import_names, v.as_object_mut())
                && let Some(name) = names.get(&i.file_guid)
            {
                m.insert("name".to_string(), name.into());
            }
            seq.serialize_element(&v)?;
        }
        seq.end()
    }
}

impl Serialize for InstancesView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0.d;
        let mut seq = s.serialize_seq(Some(d.instances.len()))?;
        for (i, inst) in d.instances.iter().enumerate() {
            seq.serialize_element(&InstanceView(self.0, i, inst))?;
        }
        seq.end()
    }
}

impl Serialize for InstanceView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (ctx, i, inst) = (self.0, self.1, self.2);
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("type", &ctx.d.type_name(i))?;
        m.serialize_entry("exported", &inst.exported)?;
        if inst.exported {
            m.serialize_entry("guid", &hex(&inst.guid))?;
        }
        let at = usize::try_from(inst.data_offset).ok();
        if ctx.o.offsets {
            m.serialize_entry("offset", &at.map(|a| ctx.file_offset(a)))?;
        }
        match &inst.object {
            Some(o) => {
                m.serialize_entry("values", &ObjectView(ctx, o))?;
                if let (true, Some(at)) = (ctx.o.offsets, at) {
                    m.serialize_entry("offsets", &OffsetsView(ctx, o, at))?;
                }
            }
            None => m.serialize_entry("values", &None::<()>)?,
        }
        m.end()
    }
}

impl Serialize for ObjectView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (ctx, o) = (self.0, self.1);
        let mut m = s.serialize_map(Some(o.fields.len()))?;
        for f in &o.fields {
            m.serialize_entry(&f.name, &ValueView(ctx, &f.value, Some(f.descriptor)))?;
        }
        m.end()
    }
}

impl Serialize for OffsetsView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (ctx, o, at) = (self.0, self.1, self.2);
        let mut m = s.serialize_map(Some(o.fields.len()))?;
        for f in &o.fields {
            let slot = ctx
                .d
                .fields
                .get(f.descriptor)
                .and_then(|fd| at.checked_add(usize::try_from(fd.data_offset).ok()?));
            match slot {
                Some(slot) => m.serialize_entry(&f.name, &SlotView(ctx, &f.value, slot))?,
                None => m.serialize_entry(&f.name, &None::<usize>)?,
            }
        }
        m.end()
    }
}

impl Serialize for SlotView<'_> {
    /// The slot's file offset; a structure or an array of structures shows `{offset, fields}` or `{offset, elements}`
    /// with the offsets of what it holds.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (ctx, v, slot) = (self.0, self.1, self.2);
        let offset = ctx.file_offset(slot);
        match v {
            Value::Object(o) => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("offset", &offset)?;
                m.serialize_entry("fields", &OffsetsView(ctx, o, slot))?;
                m.end()
            }
            Value::Array(items, _) if items.iter().any(|i| matches!(i, Value::Object(_))) => {
                // the elements lie one after another from where the slot's displacement points, a structure's size apart
                let start = u32::try_from(slot)
                    .ok()
                    .and_then(|slot| ctx.d.array_data_offset(slot))
                    .and_then(|p| usize::try_from(p).ok());
                let mut at = start;
                let elements: Vec<serde_json::Value> = items
                    .iter()
                    .map(|i| {
                        let Value::Object(o) = i else {
                            return serde_json::Value::Null;
                        };
                        let here = at;
                        at = at.and_then(|a| a.checked_add(usize::from(ctx.d.types.get(o.descriptor)?.size)));
                        match (
                            here,
                            primase_core::json::to_value(&OffsetsView(ctx, o, here.unwrap_or(0))),
                        ) {
                            (Some(_), Ok(v)) => v,
                            _ => serde_json::Value::Null,
                        }
                    })
                    .collect();
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("offset", &offset)?;
                m.serialize_entry("elements", &elements)?;
                m.end()
            }
            _ => s.serialize_u64(u64::try_from(offset).unwrap_or(u64::MAX)),
        }
    }
}

impl Serialize for Text<'_> {
    /// A string as stored: text where the bytes are UTF-8, else `{"bytes": hex}`.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match std::str::from_utf8(self.0) {
            Ok(t) => s.serialize_str(t),
            Err(_) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("bytes", &hex(self.0))?;
                m.end()
            }
        }
    }
}

impl Serialize for ValueView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (ctx, field) = (self.0, self.2);
        let d = ctx.d;
        match self.1 {
            Value::Nothing => s.serialize_none(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(v) => match field {
                Some(f)
                    if ctx.o.enum_members
                        && d.fields
                            .get(f)
                            .is_some_and(|fd| fd.ty() == FieldType::Enumeration) =>
                {
                    let mut m = s.serialize_map(Some(2))?;
                    m.serialize_entry("value", v)?;
                    m.serialize_entry("member", &d.enum_member(f, *v))?;
                    m.end()
                }
                _ => s.serialize_i64(*v),
            },
            Value::UInt(v) => s.serialize_u64(*v),
            Value::Float(v) => s.serialize_f64(*v),
            Value::Str(b) => Text(b).serialize(s),
            Value::Guid(g) => s.serialize_str(&hex(g)),
            Value::Sha1(h) => s.serialize_str(&hex(h)),
            Value::Resource(r) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("resource", &format!("{r:#018x}"))?;
                m.end()
            }
            Value::Pointer(Pointer::Null) => s.serialize_none(),
            Value::Pointer(Pointer::Internal(i)) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("instance", i)?;
                m.end()
            }
            Value::Pointer(Pointer::External(i)) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("import", i)?;
                m.end()
            }
            Value::TypeRef(t) => {
                let mut m = s.serialize_map(Some(1))?;
                // a type of the document's reflection table by name; else the GUID the EFIX type list holds at the
                // reference's slot (most references name types the document does not reflect); else as stored
                match (
                    d.type_ref_target(t),
                    t.fixup_type.and_then(|slot| d.fixup_type_guids.get(slot)),
                ) {
                    (Some(i), _) => m.serialize_entry("type", &d.types[i].name)?,
                    (None, Some(guid)) => m.serialize_entry("type_guid", &hex(guid))?,
                    (None, None) => m.serialize_entry("encoded_type", &t.encoded)?,
                }
                m.end()
            }
            Value::Boxed(b) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry(
                    "boxed",
                    &serde_json::json!({ "encoded_type": b.encoded_type, "data_offset": b.data_offset }),
                )?;
                m.end()
            }
            Value::Object(o) => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("type", &d.types.get(o.descriptor).map(|t| t.name.as_str()))?;
                m.serialize_entry("values", &ObjectView(ctx, o))?;
                m.end()
            }
            Value::Array(items, _) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items {
                    seq.serialize_element(&ValueView(ctx, v, field))?;
                }
                seq.end()
            }
        }
    }
}
