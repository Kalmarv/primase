//! Frostbite EBX documents: RIFF `EBX\0`/`EBXS` files holding the EBXD payload, EFIX fixups, EBXX array and
//! boxed-value records, and a REFL or RFL2 reflection table. [`read`] turns one into typed values, [`write`] lays a
//! document out again, and [`carry`] moves values (and what they point at) from one document into another.
//!
//! This is a port of ReSkate's EBX reader, writer and merge (`Engine/Resource/ebx_document.cpp`,
//! `ebx_writer.cpp` and `ebx_merge.cpp`; github.com/Dingo-Shenanigans/ReSkate, GPL-3.0-or-later), which is the
//! reference for the choices the format leaves open: the donor reflection table is carried as stored, boxed values
//! travel as their bytes plus what those bytes point at, and a new array takes the hash of an array of its field. The
//! port adds bounds checks and strict validation where the C++ trusts its input. A byte comparison with that reference
//! writer therefore measures agreement with the port's source; independent evidence comes from stock documents, whose
//! values must survive a rewrite.
//!
//! Strings are bytes as stored (Frostbite strings are not guaranteed UTF-8). Offsets named `*_offset` are relative to
//! the start of the EBXD payload (`data_start`, 16-aligned) unless they say otherwise.
//!
//! A document read and written again keeps its values; the bytes come back exactly when the document already has the
//! writer's layout (a document the writer produced, or one laid out the same way).

pub mod carry;
mod decode;
mod encode;
mod json;

pub use decode::{read, read_file_guid};
pub use encode::write;

/// `FieldType`: bits 4..9 of an encoded type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FieldType {
    Inherited = 0x00,
    DbObject = 0x01,
    Structure = 0x02,
    Pointer = 0x03,
    Array = 0x04,
    FixedArray = 0x05,
    String = 0x06,
    CString = 0x07,
    Enumeration = 0x08,
    FileRef = 0x09,
    Boolean = 0x0A,
    Int8 = 0x0B,
    UInt8 = 0x0C,
    Int16 = 0x0D,
    UInt16 = 0x0E,
    Int32 = 0x0F,
    UInt32 = 0x10,
    UInt64 = 0x11,
    Int64 = 0x12,
    Float32 = 0x13,
    Float64 = 0x14,
    Guid = 0x15,
    Sha1 = 0x16,
    ResourceRef = 0x17,
    Function = 0x18,
    TypeRef = 0x19,
    BoxedValueRef = 0x1A,
    Interface = 0x1B,
    Delegate = 0x1C,
    /// 0x1D..0x1F: not a type ReSkate knows; read as nothing, written as zero bytes.
    Unknown = 0x1F,
}

impl FieldType {
    pub fn from_bits(v: u16) -> FieldType {
        use FieldType::*;
        const ALL: [FieldType; 29] = [
            Inherited,
            DbObject,
            Structure,
            Pointer,
            Array,
            FixedArray,
            String,
            CString,
            Enumeration,
            FileRef,
            Boolean,
            Int8,
            UInt8,
            Int16,
            UInt16,
            Int32,
            UInt32,
            UInt64,
            Int64,
            Float32,
            Float64,
            Guid,
            Sha1,
            ResourceRef,
            Function,
            TypeRef,
            BoxedValueRef,
            Interface,
            Delegate,
        ];
        ALL.get(usize::from(v)).copied().unwrap_or(Unknown)
    }
}

/// The category nibble of an encoded type (`FieldCategory`); only `Array` (4) changes how a field is read.
pub const CATEGORY_ARRAY: u16 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDescriptor {
    pub name: String,
    pub name_hash: u32,
    /// The encoded type as ReSkate keeps it: the stored u16 shifted right by one.
    pub encoded_type: u16,
    pub class_ref: u16,
    pub data_offset: u32,
}

impl FieldDescriptor {
    pub fn ty(&self) -> FieldType {
        FieldType::from_bits((self.encoded_type >> 4) & 0x1F)
    }
    pub fn is_array(&self) -> bool {
        self.encoded_type & 0x0F == CATEGORY_ARRAY
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDescriptor {
    pub name: String,
    pub name_hash: u32,
    pub field_index: i32,
    pub field_count: u16,
    pub encoded_type: u16,
    pub size: u16,
    pub alignment: u16,
    pub guid: [u8; 16],
    pub signature: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportReference {
    pub file_guid: [u8; 16],
    pub class_guid: [u8; 16],
}

/// An EBXX array record: where an array's data starts, its length, its hash and element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayRecord {
    pub offset: u32,
    pub count: u32,
    pub hash: u32,
    /// As stored (not shifted).
    pub encoded_type: u16,
    /// For struct elements, the EFIX type slot; else 0xFFFF.
    pub class_ref: u16,
}

/// One RFL2 group record: an array type the document uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReflectionGroup {
    pub hash: u32,
    pub count: u32,
    pub first: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxedValueRecord {
    pub offset: u32,
    pub count: u32,
    pub hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
    /// The record's bytes, up to the next record or the string section.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub fixup_type: u16,
    /// Index into [`Document::types`]; `None` when its EFIX type has no reflection entry (then it is not read).
    pub descriptor: Option<usize>,
    pub data_offset: u32,
    pub exported: bool,
    pub guid: [u8; 16],
    pub object: Option<Object>,
    /// The instance's bytes as read (its type's size): padding and unread fields are written back from here.
    pub raw_image: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Null,
    /// An instance of this document, by index.
    Internal(usize),
    /// An import, by index.
    External(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeRef {
    pub primitive: bool,
    pub primitive_type: Option<FieldType>,
    pub descriptor: Option<usize>,
    pub encoded: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxedRef {
    pub encoded_type: u32,
    /// Payload offset of the boxed value, or -1.
    pub data_offset: i64,
    pub relative_offset: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nothing,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    /// String, CString and FileRef values, as stored.
    Str(Vec<u8>),
    Guid([u8; 16]),
    Sha1([u8; 20]),
    Resource(u64),
    Pointer(Pointer),
    TypeRef(TypeRef),
    Boxed(BoxedRef),
    Object(Box<Object>),
    /// An array's elements, and the hash of the record it was read from (a new array has none).
    Array(Vec<Value>, Option<u32>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldValue {
    /// Index into [`Document::fields`].
    pub descriptor: usize,
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub descriptor: usize,
    pub fields: Vec<FieldValue>,
}

impl Object {
    /// The field named `name` (ASCII case-insensitive, as ReSkate looks fields up).
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
            .map(|f| &f.value)
    }
    pub fn field_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.fields
            .iter_mut()
            .find(|f| f.name.eq_ignore_ascii_case(name))
            .map(|f| &mut f.value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub file_guid: [u8; 16],
    pub root_type: String,
    /// Absolute offsets of the EBXD payload in the bytes read.
    pub data_start: usize,
    pub data_end: usize,
    pub arrays_offset: u32,
    pub boxed_values_offset: u32,
    pub strings_offset: u32,
    pub serialized_layout: bool,
    pub hashed_reflection_names: bool,
    pub reflection_data: Vec<u8>,
    pub fixup_type_guids: Vec<[u8; 16]>,
    pub fixup_type_signatures: Vec<u32>,
    pub boxed_reference_offsets: Vec<u32>,
    pub types: Vec<TypeDescriptor>,
    pub fields: Vec<FieldDescriptor>,
    pub instances: Vec<Instance>,
    pub imports: Vec<ImportReference>,
    pub arrays: Vec<ArrayRecord>,
    pub boxed_values: Vec<BoxedValueRecord>,
    pub pointer_offsets: Vec<u32>,
    pub resource_ref_offsets: Vec<u32>,
    pub import_offsets: Vec<u32>,
    pub type_info_offsets: Vec<u32>,
    pub boxed_value_slots: Vec<u32>,
    /// The EBXD bytes as read (from `data_start`): what boxed values point at is carried from here.
    pub payload: Vec<u8>,
    pub reflection_groups: Vec<ReflectionGroup>,
}

impl Document {
    /// The type name of instance `i`.
    pub fn type_name(&self, i: usize) -> Option<&str> {
        let d = self.instances.get(i)?.descriptor?;
        self.types.get(d).map(|t| t.name.as_str())
    }

    /// Indices of the instances whose type is named `name` (ASCII case-insensitive).
    pub fn instances_of(&self, name: &str) -> Vec<usize> {
        (0..self.instances.len())
            .filter(|&i| self.type_name(i).is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .collect()
    }
}
