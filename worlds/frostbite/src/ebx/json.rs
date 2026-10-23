//! A document's typed values as JSON, for showing and golden tests: every instance with its type and the values of its
//! fields in stored order, each value as the document holds it. Nothing is dropped: a string that is not UTF-8 is its
//! bytes, a non-finite float is an error (JSON has none), a pointer is the instance or import it names.

use primase_core::JsonError;
use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq, Serializer};

use super::{Document, Object, Pointer, Value};
use crate::hex;

impl Document {
    /// The document as JSON: `file_guid`, `root_type`, `imports` and `instances` (each its `type`, whether it is
    /// `exported` with its `guid`, and its `values`). Hexadecimal text is the bytes as stored.
    pub fn to_json(&self) -> Result<serde_json::Value, JsonError> {
        primase_core::json::to_value(&DocumentView(self))
    }
}

struct DocumentView<'a>(&'a Document);
struct ImportsView<'a>(&'a Document);
struct InstancesView<'a>(&'a Document);
struct ObjectView<'a>(&'a Document, &'a Object);
struct ValueView<'a>(&'a Document, &'a Value);
struct Text<'a>(&'a [u8]);

impl Serialize for DocumentView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0;
        let mut m = s.serialize_map(Some(4))?;
        m.serialize_entry("file_guid", &hex(&d.file_guid))?;
        m.serialize_entry("root_type", &d.root_type)?;
        m.serialize_entry("imports", &ImportsView(d))?;
        m.serialize_entry("instances", &InstancesView(d))?;
        m.end()
    }
}

impl Serialize for ImportsView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.imports.len()))?;
        for i in &self.0.imports {
            seq.serialize_element(&serde_json::json!({
                "file_guid": hex(&i.file_guid),
                "class_guid": hex(&i.class_guid),
            }))?;
        }
        seq.end()
    }
}

impl Serialize for InstancesView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = self.0;
        let mut seq = s.serialize_seq(Some(d.instances.len()))?;
        for (i, inst) in d.instances.iter().enumerate() {
            seq.serialize_element(&InstanceView(d, i, inst))?;
        }
        seq.end()
    }
}

struct InstanceView<'a>(&'a Document, usize, &'a super::Instance);

impl Serialize for InstanceView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (d, i, inst) = (self.0, self.1, self.2);
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("type", &d.type_name(i))?;
        m.serialize_entry("exported", &inst.exported)?;
        if inst.exported {
            m.serialize_entry("guid", &hex(&inst.guid))?;
        }
        match &inst.object {
            Some(o) => m.serialize_entry("values", &ObjectView(d, o))?,
            None => m.serialize_entry("values", &None::<()>)?,
        }
        m.end()
    }
}

impl Serialize for ObjectView<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (d, o) = (self.0, self.1);
        let mut m = s.serialize_map(Some(o.fields.len()))?;
        for f in &o.fields {
            m.serialize_entry(&f.name, &ValueView(d, &f.value))?;
        }
        m.end()
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
        let d = self.0;
        match self.1 {
            Value::Nothing => s.serialize_none(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(v) => s.serialize_i64(*v),
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
                match t.descriptor.and_then(|i| d.types.get(i)) {
                    Some(ty) => m.serialize_entry("type", &ty.name)?,
                    None => m.serialize_entry("encoded_type", &t.encoded)?,
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
                m.serialize_entry("values", &ObjectView(d, o))?;
                m.end()
            }
            Value::Array(items, _) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items {
                    seq.serialize_element(&ValueView(d, v))?;
                }
                seq.end()
            }
        }
    }
}
