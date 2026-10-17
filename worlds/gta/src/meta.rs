//! META resources (ymap, ytyp and other `.ym*` data): self-describing structures.
//!
//! A META resource holds structure descriptions (name, size, fields: name, offset, kind, referenced type), enum
//! descriptions and data blocks, each block an array of one structure or primitive type. Values inside blocks point
//! at other blocks by `(block id, byte offset)` packed in 32 bits (`id` 1-based in the low 12 bits, offset in the
//! next 20); an array of structures that does not fit in one block continues at the start of the next. Names are
//! case-sensitive Jenkins hashes (`joaat_exact`) of the identifiers the game's parser uses (`CMapData`, `entities`,
//! `archetypeName`).
//!
//! [`Meta::read`] checks the container; [`Meta::root`] decodes the root block's structure into a [`Struct`] tree,
//! following every array and pointer by the descriptions, and fails on anything they do not account for.
use std::collections::{BTreeMap, BTreeSet};

use crate::resource::Resource;
use crate::{Result, invalid};

/// Built-in type names of primitive data blocks (the field kind codes of their element types).
const BLOCK_STRING: u32 = 0x10;
const BLOCK_POINTER: u32 = 0x07;
/// The name of the field description that gives an array's element type.
const ARRAY_INFO: u32 = 0x100;

/// A field's kind, as the descriptions code it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    /// Three floats (a field reads 12 bytes; array elements are padded to 16).
    Vec3,
    Vec4,
    Hash,
    /// An enum stored in a byte, an int, or flags in a short or an int (the referenced enum by name).
    ByteEnum,
    IntEnum,
    ShortFlags,
    IntFlags1,
    IntFlags2,
    /// Pointer, count, capacity: elements of the type the array-info field gives.
    Array,
    /// A fixed-size inline character array (length in the field's reference).
    Chars,
    /// A fixed-size inline array (length in the field's reference, element type in the array-info field).
    Inline,
    /// A pointer to a whole data block.
    DataBlock,
    /// Pointer and length of a string in a string block.
    CharPointer,
    /// A pointer to a structure in a block whose name is the structure's.
    StructurePointer,
    /// An inline structure (the referenced structure by name).
    Structure,
}

impl Kind {
    fn from_code(c: u8) -> Option<Kind> {
        Some(match c {
            0x01 => Kind::Bool,
            0x10 => Kind::I8,
            0x11 => Kind::U8,
            0x12 => Kind::I16,
            0x13 => Kind::U16,
            0x14 => Kind::I32,
            0x15 => Kind::U32,
            0x21 => Kind::F32,
            0x33 => Kind::Vec3,
            0x34 => Kind::Vec4,
            0x4A => Kind::Hash,
            0x60 => Kind::ByteEnum,
            0x62 => Kind::IntEnum,
            0x63 => Kind::IntFlags1,
            0x64 => Kind::ShortFlags,
            0x65 => Kind::IntFlags2,
            0x52 => Kind::Array,
            0x40 => Kind::Chars,
            0x50 => Kind::Inline,
            0x59 => Kind::DataBlock,
            0x44 => Kind::CharPointer,
            0x07 => Kind::StructurePointer,
            0x05 => Kind::Structure,
            _ => return None,
        })
    }

    /// The size of one value inside a structure (`None` for an inline structure, sized by its description).
    fn size(self) -> Option<usize> {
        Some(match self {
            Kind::Bool | Kind::I8 | Kind::U8 | Kind::ByteEnum => 1,
            Kind::I16 | Kind::U16 | Kind::ShortFlags => 2,
            Kind::I32
            | Kind::U32
            | Kind::F32
            | Kind::Hash
            | Kind::IntEnum
            | Kind::IntFlags1
            | Kind::IntFlags2 => 4,
            Kind::Vec3 => 12,
            Kind::Vec4 => 16,
            Kind::Array | Kind::CharPointer => 16,
            Kind::DataBlock | Kind::StructurePointer => 8,
            Kind::Chars | Kind::Inline | Kind::Structure => return None,
        })
    }
}

/// A field description.
#[derive(Clone, Debug)]
pub struct Field {
    pub name: u32,
    pub offset: u32,
    pub kind: Kind,
    /// The description's byte 9, kept as stored (its meaning is not known).
    pub byte9: u8,
    /// The index of the array-info field among the structure's fields (arrays and inline arrays), else -1 or 0.
    pub ref_index: i16,
    /// The referenced structure or enum's name, or an inline array's length.
    pub ref_key: u32,
}

/// A structure description.
#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: u32,
    pub key: u32,
    pub size: u32,
    pub fields: Vec<Field>,
}

/// An enum description: value names and values.
#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: u32,
    pub key: u32,
    pub values: Vec<(u32, i32)>,
}

/// A data block: an array of one structure or primitive type.
#[derive(Clone, Debug)]
pub struct Block {
    pub name: u32,
    pub data: Vec<u8>,
}

/// A META resource's container.
#[derive(Clone, Debug)]
pub struct Meta {
    /// The resource's own name, when it carries one.
    pub name: Option<String>,
    /// The index of the root block.
    pub root: usize,
    pub structs: BTreeMap<u32, StructInfo>,
    pub enums: BTreeMap<u32, EnumInfo>,
    pub blocks: Vec<Block>,
}

/// A decoded value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    F32(f32),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Hash(u32),
    /// An enum or flags value with the enum's name.
    Enum {
        enum_name: u32,
        value: i32,
    },
    /// A fixed-size character array up to its first NUL.
    Chars(Vec<u8>),
    /// A whole data block's bytes.
    Data(Vec<u8>),
    /// A string (`None` for a null pointer).
    Str(Option<Vec<u8>>),
    Struct(Struct),
    /// A pointed-to structure (`None` for a null pointer).
    Pointer(Option<Struct>),
    Array(Vec<Value>),
}

/// A decoded structure: its name and its fields in description order (array-info fields left out).
#[derive(Clone, Debug, PartialEq)]
pub struct Struct {
    pub name: u32,
    pub fields: Vec<(u32, Value)>,
}

fn err(offset: u64, detail: impl ToString) -> crate::Error {
    invalid("META", offset, detail)
}

use crate::le::{f32 as f32_at, u16 as u16_at, u32 as u32_at, u64 as u64_at};

/// The header's fixed values (`0x10`, `0x14`).
const HEADER_MAGIC: u32 = 0x5052_4430;
const HEADER_VERSION: u16 = 0x0079;

impl Meta {
    /// Reads the container: header, structure and enum descriptions, data blocks.
    pub fn read(r: &Resource) -> Result<Meta> {
        let h = r.at(0x5000_0000, 0x70)?;
        if u32_at(h, 0x10)? != HEADER_MAGIC || u16_at(h, 0x14)? != HEADER_VERSION {
            return Err(err(
                0x10,
                format!("header {:#x} {:#x}", u32_at(h, 0x10)?, u16_at(h, 0x14)?),
            ));
        }
        if h[0x16] > 1 || h[0x17] != 0 || u32_at(h, 0x18)? != 0 || h[0x4E..0x70].iter().any(|&b| b != 0) {
            return Err(err(0x16, "reserved header bytes are not zero"));
        }
        let root = u32_at(h, 0x1C)? as usize;
        let (struct_ptr, enum_ptr, block_ptr, name_ptr) = (
            u64_at(h, 0x20)?,
            u64_at(h, 0x28)?,
            u64_at(h, 0x30)?,
            u64_at(h, 0x38)?,
        );
        let (n_structs, n_enums, n_blocks) = (
            u16_at(h, 0x48)? as usize,
            u16_at(h, 0x4A)? as usize,
            u16_at(h, 0x4C)? as usize,
        );

        let mut structs = BTreeMap::new();
        if n_structs > 0 {
            let s = r.at(struct_ptr, 32 * n_structs)?;
            for i in 0..n_structs {
                let d = &s[32 * i..32 * i + 32];
                let (name, key, size) = (u32_at(d, 0)?, u32_at(d, 4)?, u32_at(d, 0x18)?);
                let n = u16_at(d, 0x1E)? as usize;
                if u32_at(d, 0xC)? != 0 || u16_at(d, 0x1C)? != 0 {
                    return Err(err(struct_ptr, format!("structure {name:#010x}: reserved bytes")));
                }
                let e = if n > 0 {
                    r.at(u64_at(d, 0x10)?, 16 * n)?
                } else {
                    &[][..]
                };
                let mut fields = Vec::with_capacity(n);
                for j in 0..n {
                    let f = &e[16 * j..16 * j + 16];
                    let kind = Kind::from_code(f[8]).ok_or_else(|| {
                        err(
                            struct_ptr,
                            format!("structure {name:#010x}: field kind {:#x}", f[8]),
                        )
                    })?;
                    fields.push(Field {
                        name: u32_at(f, 0)?,
                        offset: u32_at(f, 4)?,
                        kind,
                        byte9: f[9],
                        ref_index: u16_at(f, 10)? as i16,
                        ref_key: u32_at(f, 12)?,
                    });
                }
                if structs
                    .insert(
                        name,
                        StructInfo {
                            name,
                            key,
                            size,
                            fields,
                        },
                    )
                    .is_some()
                {
                    return Err(err(struct_ptr, format!("structure {name:#010x} described twice")));
                }
            }
        }

        let mut enums = BTreeMap::new();
        if n_enums > 0 {
            let s = r.at(enum_ptr, 24 * n_enums)?;
            for i in 0..n_enums {
                let d = &s[24 * i..24 * i + 24];
                let (name, key, n) = (u32_at(d, 0)?, u32_at(d, 4)?, u32_at(d, 0x10)? as usize);
                if u32_at(d, 0x14)? != 0 {
                    return Err(err(enum_ptr, format!("enum {name:#010x}: reserved bytes")));
                }
                let e = if n > 0 {
                    r.at(u64_at(d, 8)?, 8 * n)?
                } else {
                    &[][..]
                };
                let values = e
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| {
                        let [k, v] = crate::le::u32x2_of(c);
                        (k, v as i32)
                    })
                    .collect();
                if enums.insert(name, EnumInfo { name, key, values }).is_some() {
                    return Err(err(enum_ptr, format!("enum {name:#010x} described twice")));
                }
            }
        }

        let s = r.at(block_ptr, 16 * n_blocks)?;
        let mut blocks = Vec::with_capacity(n_blocks);
        for i in 0..n_blocks {
            let d = &s[16 * i..16 * i + 16];
            let len = u32_at(d, 4)? as usize;
            let data = if len > 0 {
                r.at(u64_at(d, 8)?, len)?.to_vec()
            } else {
                Vec::new()
            };
            blocks.push(Block {
                name: u32_at(d, 0)?,
                data,
            });
        }
        if root == 0 || root > blocks.len() {
            return Err(err(0x1C, format!("root block {root} of {}", blocks.len())));
        }

        let name = match name_ptr {
            0 => None,
            p => Some(read_c_string(r, p)?),
        };
        Ok(Meta {
            name,
            root: root - 1,
            structs,
            enums,
            blocks,
        })
    }

    /// The structure description of this name.
    pub fn struct_info(&self, name: u32) -> Result<&StructInfo> {
        self.structs
            .get(&name)
            .ok_or_else(|| err(0, format!("no description of structure {name:#010x}")))
    }

    /// Decodes the root block's first structure.
    pub fn root(&self) -> Result<Struct> {
        self.root_covered().map(|(s, _)| s)
    }

    /// Decodes the root block's first structure, and counts per block the bytes the decoding never reached (a
    /// structure covers its whole extent, padding included; a string covers its terminating NUL).
    pub fn root_covered(&self) -> Result<(Struct, Vec<usize>)> {
        let block = &self.blocks[self.root];
        let info = self.struct_info(block.name)?;
        if (info.size as usize) > block.data.len() {
            return Err(err(0, "root block shorter than its structure"));
        }
        let bytes: usize = self.blocks.iter().map(|b| b.data.len()).sum();
        let mut walk = Walk {
            seen: self.blocks.iter().map(|b| vec![false; b.data.len()]).collect(),
            reached: BTreeSet::new(),
            budget: (MAX_DEPTH + 1).saturating_mul(bytes.saturating_add(1)),
        };
        let root = self.decode_struct(self.root, 0, info, 0, &mut walk)?;
        Ok((
            root,
            walk.seen
                .iter()
                .map(|b| b.iter().filter(|&&x| !x).count())
                .collect(),
        ))
    }

    fn block(&self, id: usize) -> Result<&Block> {
        id.checked_sub(1)
            .and_then(|i| self.blocks.get(i))
            .ok_or_else(|| err(0, format!("block id {id} of {}", self.blocks.len())))
    }

    /// `(block id, offset)` of a packed pointer; `None` for null. The high 32 bits must be zero.
    fn unpack(pointer: u64) -> Result<Option<(usize, usize)>> {
        if pointer >> 32 != 0 {
            return Err(err(0, format!("pointer {pointer:#x} has high bits")));
        }
        if pointer == 0 {
            return Ok(None);
        }
        Ok(Some((
            (pointer & 0xFFF) as usize,
            ((pointer >> 12) & 0xF_FFFF) as usize,
        )))
    }

    fn decode_struct(
        &self,
        block: usize,
        offset: usize,
        info: &StructInfo,
        depth: usize,
        walk: &mut Walk,
    ) -> Result<Struct> {
        if depth > MAX_DEPTH {
            return Err(err(0, format!("structures nest deeper than {MAX_DEPTH}")));
        }
        walk.budget = walk
            .budget
            .checked_sub(1)
            .ok_or_else(|| err(offset as u64, "more structures than the blocks can hold"))?;
        let data = &self.blocks[block].data;
        let end = offset + info.size as usize;
        if end > data.len() {
            return Err(err(
                offset as u64,
                format!("structure {:#010x} runs past its block", info.name),
            ));
        }
        let d = &data[offset..end];
        walk.seen[block][offset..end].fill(true);
        let mut fields = Vec::with_capacity(info.fields.len());
        for f in &info.fields {
            if f.name == ARRAY_INFO {
                continue;
            }
            let o = f.offset as usize;
            let size = match f.kind {
                Kind::Structure => self.struct_info(f.ref_key)?.size as usize,
                Kind::Chars => f.ref_key as usize,
                Kind::Inline => (f.ref_key as usize)
                    .checked_mul(self.element_size(self.array_info(info, f)?)?)
                    .ok_or_else(|| err((offset + o) as u64, "inline array size overflows"))?,
                k => k.size().expect("sized kind"),
            };
            if o.checked_add(size).is_none_or(|end| end > d.len()) {
                return Err(err(
                    (offset + o) as u64,
                    format!(
                        "field {:#010x} of {:#010x} runs past the structure",
                        f.name, info.name
                    ),
                ));
            }
            let v = &d[o..o + size];
            let value = match f.kind {
                Kind::Bool => match v[0] {
                    0 => Value::Bool(false),
                    1 => Value::Bool(true),
                    b => return Err(err((offset + o) as u64, format!("bool {b}"))),
                },
                Kind::I8 => Value::I8(v[0] as i8),
                Kind::U8 => Value::U8(v[0]),
                Kind::I16 => Value::I16(u16_at(v, 0)? as i16),
                Kind::U16 => Value::U16(u16_at(v, 0)?),
                Kind::I32 => Value::I32(u32_at(v, 0)? as i32),
                Kind::U32 => Value::U32(u32_at(v, 0)?),
                Kind::F32 => Value::F32(f32_at(v, 0)?),
                Kind::Vec3 => Value::Vec3([f32_at(v, 0)?, f32_at(v, 4)?, f32_at(v, 8)?]),
                Kind::Vec4 => Value::Vec4([f32_at(v, 0)?, f32_at(v, 4)?, f32_at(v, 8)?, f32_at(v, 12)?]),
                Kind::Hash => Value::Hash(u32_at(v, 0)?),
                Kind::ByteEnum => Value::Enum {
                    enum_name: f.ref_key,
                    value: i32::from(v[0]),
                },
                Kind::ShortFlags => Value::Enum {
                    enum_name: f.ref_key,
                    value: i32::from(u16_at(v, 0)?),
                },
                Kind::IntEnum | Kind::IntFlags1 | Kind::IntFlags2 => Value::Enum {
                    enum_name: f.ref_key,
                    value: u32_at(v, 0)? as i32,
                },
                Kind::Chars => Value::Chars(v.iter().copied().take_while(|&b| b != 0).collect()),
                Kind::Inline => {
                    let el = self.array_info(info, f)?;
                    let n = f.ref_key as usize;
                    let es = self.element_size(el)?;
                    let mut items = Vec::with_capacity(n);
                    for i in 0..n {
                        items.push(self.decode_inline(el, &v[i * es..(i + 1) * es])?);
                    }
                    Value::Array(items)
                }
                Kind::DataBlock => match Self::unpack(u64_at(v, 0)?)? {
                    None => Value::Data(Vec::new()),
                    Some((id, 0)) => {
                        let data = self.block(id)?.data.clone();
                        walk.seen[id - 1].fill(true);
                        Value::Data(data)
                    }
                    Some((_, off)) => {
                        return Err(err(
                            (offset + o) as u64,
                            format!("data block pointer at offset {off}"),
                        ));
                    }
                },
                Kind::CharPointer => {
                    let (len, cap) = (u16_at(v, 8)? as usize, u16_at(v, 10)? as usize);
                    if u32_at(v, 12)? != 0 || cap < len {
                        return Err(err((offset + o) as u64, "string length, capacity"));
                    }
                    match Self::unpack(u64_at(v, 0)?)? {
                        None if len == 0 => Value::Str(None),
                        None => return Err(err((offset + o) as u64, "null string with a length")),
                        Some((id, off)) => {
                            let b = self.block(id)?;
                            if b.name != BLOCK_STRING {
                                return Err(err(
                                    (offset + o) as u64,
                                    format!("string in a {:#x} block", b.name),
                                ));
                            }
                            let s = b
                                .data
                                .get(off..off + len)
                                .ok_or_else(|| err((offset + o) as u64, "string runs past its block"))?;
                            let nul = usize::from(b.data.get(off + len) == Some(&0));
                            walk.seen[id - 1][off..off + len + nul].fill(true);
                            Value::Str(Some(s.to_vec()))
                        }
                    }
                }
                Kind::StructurePointer => Value::Pointer(self.follow(u64_at(v, 0)?, depth, walk)?),
                Kind::Structure => {
                    let si = self.struct_info(f.ref_key)?;
                    Value::Struct(self.decode_struct(block, offset + o, si, depth + 1, walk)?)
                }
                Kind::Array => {
                    let el = self.array_info(info, f)?;
                    let (count, cap) = (u16_at(v, 8)? as usize, u16_at(v, 10)? as usize);
                    if u32_at(v, 12)? != 0 || cap < count {
                        return Err(err(
                            (offset + o) as u64,
                            format!("array count {count}, capacity {cap}"),
                        ));
                    }
                    Value::Array(self.decode_array(el, u64_at(v, 0)?, count, depth, walk)?)
                }
            };
            fields.push((f.name, value));
        }
        Ok(Struct {
            name: info.name,
            fields,
        })
    }

    /// The array-info field an array or inline-array field refers to.
    fn array_info<'a>(&self, info: &'a StructInfo, f: &Field) -> Result<&'a Field> {
        usize::try_from(f.ref_index)
            .ok()
            .and_then(|i| info.fields.get(i))
            .filter(|a| a.name == ARRAY_INFO)
            .ok_or_else(|| {
                err(
                    0,
                    format!(
                        "field {:#010x} of {:#010x}: no array-info field",
                        f.name, info.name
                    ),
                )
            })
    }

    /// The size of one element of an array whose array-info field is `el`; never 0 (a structure described with size
    /// 0 is no array element).
    fn element_size(&self, el: &Field) -> Result<usize> {
        Ok(match el.kind {
            Kind::Structure => match self.struct_info(el.ref_key)?.size {
                0 => {
                    return Err(err(
                        0,
                        format!("array of {:#010x}, a structure of size 0", el.ref_key),
                    ));
                }
                size => size as usize,
            },
            Kind::Vec3 => 16,
            k => k.size().ok_or_else(|| err(0, format!("array of {k:?}")))?,
        })
    }

    /// One element of an inline array.
    fn decode_inline(&self, el: &Field, v: &[u8]) -> Result<Value> {
        Ok(match el.kind {
            Kind::U8 => Value::U8(v[0]),
            Kind::I8 => Value::I8(v[0] as i8),
            Kind::U16 => Value::U16(u16_at(v, 0)?),
            Kind::I16 => Value::I16(u16_at(v, 0)? as i16),
            Kind::U32 => Value::U32(u32_at(v, 0)?),
            Kind::I32 => Value::I32(u32_at(v, 0)? as i32),
            Kind::F32 => Value::F32(f32_at(v, 0)?),
            Kind::Hash => Value::Hash(u32_at(v, 0)?),
            Kind::Vec3 => Value::Vec3([f32_at(v, 0)?, f32_at(v, 4)?, f32_at(v, 8)?]),
            Kind::Vec4 => Value::Vec4([f32_at(v, 0)?, f32_at(v, 4)?, f32_at(v, 8)?, f32_at(v, 12)?]),
            k => return Err(err(0, format!("inline array of {k:?}"))),
        })
    }

    /// A structure pointer: the structure is the one its block holds.
    fn follow(&self, pointer: u64, depth: usize, walk: &mut Walk) -> Result<Option<Struct>> {
        let Some((id, off)) = Self::unpack(pointer)? else {
            return Ok(None);
        };
        let b = self.block(id)?;
        let info = self.struct_info(b.name)?;
        if off % (info.size as usize).max(1) != 0 {
            return Err(err(
                off as u64,
                format!("pointer into the middle of a {:#010x}", info.name),
            ));
        }
        walk.reach(id, off)?;
        Ok(Some(self.decode_struct(id - 1, off, info, depth + 1, walk)?))
    }

    /// `count` elements starting at a packed pointer, continuing into following blocks of the same name.
    fn decode_array(
        &self,
        el: &Field,
        pointer: u64,
        count: usize,
        depth: usize,
        walk: &mut Walk,
    ) -> Result<Vec<Value>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let Some((mut id, mut off)) = Self::unpack(pointer)? else {
            return Err(err(0, format!("null array of {count}")));
        };
        let es = self.element_size(el)?;
        // the name of the blocks holding the elements
        let block_name = match el.kind {
            Kind::Structure => el.ref_key,
            Kind::StructurePointer => BLOCK_POINTER,
            Kind::CharPointer | Kind::DataBlock | Kind::Array => {
                return Err(err(0, format!("array of {:?}", el.kind)));
            }
            _ => u32::from(el_code(el.kind)),
        };
        let mut items = Vec::with_capacity(count);
        while items.len() < count {
            let b = self.block(id)?;
            if b.name != block_name {
                return Err(err(
                    off as u64,
                    format!("array of {block_name:#x} in a {:#x} block", b.name),
                ));
            }
            if off % es != 0 || b.data.len() % es != 0 || off >= b.data.len() {
                return Err(err(
                    off as u64,
                    format!("array elements of {es} bytes misaligned in block {id}"),
                ));
            }
            while off < b.data.len() && items.len() < count {
                let v = &b.data[off..off + es];
                if el.kind != Kind::Structure {
                    walk.seen[id - 1][off..off + es].fill(true);
                }
                items.push(match el.kind {
                    Kind::Structure => {
                        walk.reach(id, off)?;
                        Value::Struct(self.decode_struct(
                            id - 1,
                            off,
                            self.struct_info(el.ref_key)?,
                            depth + 1,
                            walk,
                        )?)
                    }
                    Kind::StructurePointer => Value::Pointer(self.follow(u64_at(v, 0)?, depth, walk)?),
                    _ => self.decode_inline(el, v)?,
                });
                off += es;
            }
            id += 1;
            off = 0;
        }
        Ok(items)
    }
}

/// What a decoding reached: per block, which bytes; and where the structures that pointers and arrays lead to start.
struct Walk {
    seen: Vec<Vec<bool>>,
    reached: BTreeSet<(usize, usize)>,
    /// Structures left to decode. An embedded structure lies within its parent's bytes and pointers and arrays reach
    /// a structure once, so a tree of non-empty structures decodes at most `MAX_DEPTH + 1` per byte of its blocks;
    /// embedded fields that overlap could otherwise decode one exponentially often.
    budget: usize,
}

/// How deep structures may nest (embedded, pointed at or array elements).
const MAX_DEPTH: usize = 64;

impl Walk {
    /// Marks the structure a pointer or array element leads to (block id, offset). A structure is reached once: the
    /// containers are trees, and one reached twice would be decoded again for every path to it (exponentially many
    /// with nesting).
    fn reach(&mut self, id: usize, offset: usize) -> Result<()> {
        if !self.reached.insert((id, offset)) {
            return Err(err(
                offset as u64,
                format!("the structure at offset {offset} of block {id} is reached twice"),
            ));
        }
        Ok(())
    }
}

/// The kind code of a primitive kind (the name of blocks of that primitive).
fn el_code(k: Kind) -> u8 {
    match k {
        Kind::U8 => 0x11,
        Kind::I8 => 0x10,
        Kind::U16 => 0x13,
        Kind::I16 => 0x12,
        Kind::U32 => 0x15,
        Kind::I32 => 0x14,
        Kind::F32 => 0x21,
        Kind::Vec3 => 0x33,
        Kind::Vec4 => 0x34,
        Kind::Hash => 0x4A,
        _ => 0,
    }
}

/// A NUL-terminated string at a resource pointer.
fn read_c_string(r: &Resource, pointer: u64) -> Result<String> {
    let mut s = Vec::new();
    loop {
        let b = r.at(pointer + s.len() as u64, 1)?[0];
        if b == 0 {
            break;
        }
        s.push(b);
    }
    String::from_utf8(s).map_err(|e| err(pointer, e))
}

impl Struct {
    /// The field of this name (the identifier, hashed).
    pub fn get(&self, name: &str) -> Result<&Value> {
        let h = primase_hash::joaat_exact(name.as_bytes());
        self.fields
            .iter()
            .find(|(n, _)| *n == h)
            .map(|(_, v)| v)
            .ok_or_else(|| err(0, format!("structure {:#010x} has no field {name}", self.name)))
    }

    fn kind_err(&self, name: &str, want: &str) -> crate::Error {
        err(
            0,
            format!("structure {:#010x}: field {name} is not {want}", self.name),
        )
    }

    pub fn u32(&self, name: &str) -> Result<u32> {
        match self.get(name)? {
            Value::U32(v) => Ok(*v),
            _ => Err(self.kind_err(name, "a u32")),
        }
    }

    pub fn i32(&self, name: &str) -> Result<i32> {
        match self.get(name)? {
            Value::I32(v) => Ok(*v),
            _ => Err(self.kind_err(name, "an i32")),
        }
    }

    pub fn f32(&self, name: &str) -> Result<f32> {
        match self.get(name)? {
            Value::F32(v) => Ok(*v),
            _ => Err(self.kind_err(name, "an f32")),
        }
    }

    pub fn hash(&self, name: &str) -> Result<u32> {
        match self.get(name)? {
            Value::Hash(v) => Ok(*v),
            _ => Err(self.kind_err(name, "a hash")),
        }
    }

    pub fn vec3(&self, name: &str) -> Result<[f32; 3]> {
        match self.get(name)? {
            Value::Vec3(v) => Ok(*v),
            _ => Err(self.kind_err(name, "a vector of three")),
        }
    }

    pub fn vec4(&self, name: &str) -> Result<[f32; 4]> {
        match self.get(name)? {
            Value::Vec4(v) => Ok(*v),
            _ => Err(self.kind_err(name, "a vector of four")),
        }
    }

    /// An enum or flags value.
    pub fn enum_value(&self, name: &str) -> Result<i32> {
        match self.get(name)? {
            Value::Enum { value, .. } => Ok(*value),
            _ => Err(self.kind_err(name, "an enum")),
        }
    }

    pub fn array(&self, name: &str) -> Result<&[Value]> {
        match self.get(name)? {
            Value::Array(v) => Ok(v),
            _ => Err(self.kind_err(name, "an array")),
        }
    }

    /// An array of hashes.
    pub fn hashes(&self, name: &str) -> Result<Vec<u32>> {
        self.array(name)?
            .iter()
            .map(|v| match v {
                Value::Hash(h) => Ok(*h),
                _ => Err(self.kind_err(name, "an array of hashes")),
            })
            .collect()
    }

    /// An array of structure pointers, none null.
    pub fn pointed(&self, name: &str) -> Result<Vec<&Struct>> {
        self.array(name)?
            .iter()
            .map(|v| match v {
                Value::Pointer(Some(s)) => Ok(s),
                _ => Err(self.kind_err(name, "an array of structure pointers")),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::PageFlags;
    use primase_hash::joaat_exact as joaat;

    fn put(d: &mut [u8], o: usize, b: &[u8]) {
        d[o..o + b.len()].copy_from_slice(b);
    }

    /// A root structure (`count`, an array of `Item` split over two blocks, a string) and an `Item` (f32, hash).
    fn sample(items: u16, string: &[u8]) -> Resource {
        let mut d = vec![0u8; 0x2000];
        put(&mut d, 0x10, &HEADER_MAGIC.to_le_bytes());
        put(&mut d, 0x14, &HEADER_VERSION.to_le_bytes());
        put(&mut d, 0x1C, &1u32.to_le_bytes()); // root block id
        for (o, p) in [(0x20, 0x100u64), (0x30, 0x400), (0x38, 0x900)] {
            put(&mut d, o, &(0x5000_0000 + p).to_le_bytes());
        }
        put(&mut d, 0x48, &2u16.to_le_bytes());
        put(&mut d, 0x4C, &4u16.to_le_bytes());
        let (root, item) = (joaat(b"Root"), joaat(b"Item"));
        // structure descriptions: name, key, -, -, fields pointer, size, -, field count
        for (i, (name, fields, size, n)) in [(root, 0x200u64, 40u32, 4u16), (item, 0x300, 8, 2)]
            .into_iter()
            .enumerate()
        {
            let o = 0x100 + 32 * i;
            put(&mut d, o, &name.to_le_bytes());
            put(&mut d, o + 0x10, &(0x5000_0000 + fields).to_le_bytes());
            put(&mut d, o + 0x18, &size.to_le_bytes());
            put(&mut d, o + 0x1E, &n.to_le_bytes());
        }
        // field descriptions: name, offset, kind, byte 9, reference index, reference key
        let field = |d: &mut [u8], o: usize, name: u32, off: u32, kind: u8, index: i16, key: u32| {
            put(d, o, &name.to_le_bytes());
            put(d, o + 4, &off.to_le_bytes());
            d[o + 8] = kind;
            put(d, o + 10, &index.to_le_bytes());
            put(d, o + 12, &key.to_le_bytes());
        };
        field(&mut d, 0x200, joaat(b"count"), 0, 0x15, 0, 0);
        field(&mut d, 0x210, ARRAY_INFO, 0, 0x05, 0, item);
        field(&mut d, 0x220, joaat(b"items"), 8, 0x52, 1, 0);
        field(&mut d, 0x230, joaat(b"label"), 24, 0x44, 0, 0);
        field(&mut d, 0x300, joaat(b"v"), 0, 0x21, 0, 0);
        field(&mut d, 0x310, joaat(b"h"), 4, 0x4A, 0, 0);
        // blocks: the root, two items, one item, a string
        for (i, (name, len, at)) in [
            (root, 40u32, 0x500u64),
            (item, 16, 0x600),
            (item, 8, 0x700),
            (BLOCK_STRING, string.len() as u32, 0x800),
        ]
        .into_iter()
        .enumerate()
        {
            let o = 0x400 + 16 * i;
            put(&mut d, o, &name.to_le_bytes());
            put(&mut d, o + 4, &len.to_le_bytes());
            put(&mut d, o + 8, &(0x5000_0000 + at).to_le_bytes());
        }
        put(&mut d, 0x500, &3u32.to_le_bytes());
        put(&mut d, 0x508, &2u64.to_le_bytes()); // block id 2, offset 0
        put(&mut d, 0x510, &items.to_le_bytes());
        put(&mut d, 0x512, &items.to_le_bytes());
        put(&mut d, 0x518, &4u64.to_le_bytes()); // block id 4, offset 0
        put(&mut d, 0x520, &5u16.to_le_bytes());
        put(&mut d, 0x522, &6u16.to_le_bytes());
        for (i, v) in [1.0f32, 2.0, 3.0].iter().enumerate() {
            let o = if i < 2 { 0x600 + 8 * i } else { 0x700 };
            put(&mut d, o, &v.to_le_bytes());
            put(&mut d, o + 4, &(i as u32 + 10).to_le_bytes());
        }
        put(&mut d, 0x800, string);
        put(&mut d, 0x900, b"sample\0");
        Resource {
            version: 2,
            system: PageFlags((1 << 27) | 4),
            graphics: PageFlags(0),
            data: d,
            zero_filled: 0,
        }
    }

    #[test]
    fn arrays_continue_into_the_next_block_and_every_byte_is_reached() {
        let m = Meta::read(&sample(3, b"hello\0")).expect("container");
        assert_eq!(m.name.as_deref(), Some("sample"));
        let (root, unreached) = m.root_covered().expect("decodes");
        assert_eq!(unreached, vec![0, 0, 0, 0]);
        assert_eq!(root.get("count").expect("count"), &Value::U32(3));
        assert_eq!(
            root.get("label").expect("label"),
            &Value::Str(Some(b"hello".to_vec()))
        );
        let Value::Array(items) = root.get("items").expect("items") else {
            panic!("not an array")
        };
        let vs: Vec<_> = items
            .iter()
            .map(|i| match i {
                Value::Struct(s) => (s.get("v").expect("v").clone(), s.get("h").expect("h").clone()),
                _ => panic!("not a structure"),
            })
            .collect();
        assert_eq!(vs[2], (Value::F32(3.0), Value::Hash(12)));
        assert_eq!(vs.len(), 3);
    }

    #[test]
    fn unreached_bytes_are_counted_and_overruns_refused() {
        let m = Meta::read(&sample(3, b"hello\0x")).expect("container");
        assert_eq!(m.root_covered().expect("decodes").1, vec![0, 0, 0, 1]);
        // a fourth item would continue into the string block
        let m = Meta::read(&sample(4, b"hello\0")).expect("container");
        assert!(m.root().is_err());
    }

    /// A field description: name, offset, kind, reference index, reference key.
    type F = (u32, u32, u8, i16, u32);

    /// A container of these structure descriptions (name, size, fields) and blocks (name, bytes), the first block
    /// the root.
    fn container(structs: &[(u32, u32, Vec<F>)], blocks: &[(u32, Vec<u8>)]) -> Resource {
        let mut d = vec![0u8; 0x100];
        let at = |d: &mut Vec<u8>, bytes: &[u8]| -> u64 {
            let o = d.len();
            d.extend_from_slice(bytes);
            d.resize(d.len().next_multiple_of(16), 0);
            0x5000_0000 + o as u64
        };
        let mut descriptions = Vec::new();
        for (name, size, fields) in structs {
            let mut f = Vec::new();
            for &(n, off, kind, index, key) in fields {
                f.extend(n.to_le_bytes());
                f.extend(off.to_le_bytes());
                f.extend([kind, 0]);
                f.extend(index.to_le_bytes());
                f.extend(key.to_le_bytes());
            }
            let fp = if f.is_empty() { 0 } else { at(&mut d, &f) };
            let mut s = [0u8; 32];
            put(&mut s, 0, &name.to_le_bytes());
            put(&mut s, 0x10, &fp.to_le_bytes());
            put(&mut s, 0x18, &size.to_le_bytes());
            put(&mut s, 0x1E, &(fields.len() as u16).to_le_bytes());
            descriptions.extend(s);
        }
        let sp = at(&mut d, &descriptions);
        let mut table = Vec::new();
        for (name, bytes) in blocks {
            let bp = at(&mut d, bytes);
            table.extend(name.to_le_bytes());
            table.extend((bytes.len() as u32).to_le_bytes());
            table.extend(bp.to_le_bytes());
        }
        let bp = at(&mut d, &table);
        put(&mut d, 0x10, &HEADER_MAGIC.to_le_bytes());
        put(&mut d, 0x14, &HEADER_VERSION.to_le_bytes());
        put(&mut d, 0x1C, &1u32.to_le_bytes());
        put(&mut d, 0x20, &sp.to_le_bytes());
        put(&mut d, 0x30, &bp.to_le_bytes());
        put(&mut d, 0x48, &(structs.len() as u16).to_le_bytes());
        put(&mut d, 0x4C, &(blocks.len() as u16).to_le_bytes());
        d.resize(0x2000, 0);
        Resource {
            version: 2,
            system: PageFlags((1 << 27) | 4),
            graphics: PageFlags(0),
            data: d,
            zero_filled: 0,
        }
    }

    /// An array field's 16 bytes: pointer, count, capacity.
    fn array(pointer: u64, count: u16) -> Vec<u8> {
        let mut v = pointer.to_le_bytes().to_vec();
        v.extend(count.to_le_bytes());
        v.extend(count.to_le_bytes());
        v.extend([0; 4]);
        v
    }

    #[test]
    fn an_array_of_structures_of_size_zero_is_refused() {
        let (root, item) = (joaat(b"Root"), joaat(b"Item"));
        let fields = vec![(ARRAY_INFO, 0, 0x05, 0, item), (joaat(b"items"), 0, 0x52, 0, 0)];
        let r = container(
            &[(root, 16, fields), (item, 0, Vec::new())],
            &[(root, array(2, 1)), (item, vec![0; 8])],
        );
        let m = Meta::read(&r).expect("container");
        let e = m.root().expect_err("size 0");
        assert!(e.to_string().contains("size 0"), "{e}");
    }

    #[test]
    fn a_structure_reached_twice_is_refused() {
        let (root, item) = (joaat(b"Root"), joaat(b"Item"));
        let fields = |n: u32| vec![(n, 0, 0x21, 0, 0)];
        // two arrays starting at the same element
        let arrays = vec![
            (ARRAY_INFO, 0, 0x05, 0, item),
            (joaat(b"a"), 0, 0x52, 0, 0),
            (joaat(b"b"), 16, 0x52, 0, 0),
        ];
        let r = container(
            &[(root, 32, arrays), (item, 4, fields(joaat(b"v")))],
            &[(root, [array(2, 1), array(2, 1)].concat()), (item, vec![0; 4])],
        );
        let e = Meta::read(&r).expect("container").root().expect_err("twice");
        assert!(e.to_string().contains("reached twice"), "{e}");
        // two pointers at the same structure
        let pointers = vec![(joaat(b"p"), 0, 0x07, 0, 0), (joaat(b"q"), 8, 0x07, 0, 0)];
        let r = container(
            &[(root, 16, pointers), (item, 4, fields(joaat(b"v")))],
            &[
                (root, [2u64.to_le_bytes(), 2u64.to_le_bytes()].concat()),
                (item, vec![0; 4]),
            ],
        );
        let e = Meta::read(&r).expect("container").root().expect_err("twice");
        assert!(e.to_string().contains("reached twice"), "{e}");
        // one pointer decodes
        let pointers = vec![(joaat(b"p"), 0, 0x07, 0, 0), (joaat(b"q"), 8, 0x07, 0, 0)];
        let r = container(
            &[(root, 16, pointers), (item, 4, fields(joaat(b"v")))],
            &[
                (root, [2u64.to_le_bytes(), 0u64.to_le_bytes()].concat()),
                (item, vec![0; 4]),
            ],
        );
        Meta::read(&r).expect("container").root().expect("one pointer");
    }

    #[test]
    fn overlapping_embedded_structures_stop_at_the_budget() {
        // S0 has a u32; each S(k+1) embeds S(k) twice at offset 0, so S40 would decode 2^40 structures
        let name = |k: u32| joaat(format!("S{k}").as_bytes());
        let mut structs = vec![(name(0), 4, vec![(joaat(b"v"), 0, 0x21, 0, 0)])];
        for k in 1..=40 {
            let f = vec![
                (joaat(b"a"), 0, 0x05, 0, name(k - 1)),
                (joaat(b"b"), 0, 0x05, 0, name(k - 1)),
            ];
            structs.push((name(k), 4, f));
        }
        let r = container(&structs, &[(name(40), vec![0; 4])]);
        let e = Meta::read(&r).expect("container").root().expect_err("budget");
        assert!(e.to_string().contains("more structures"), "{e}");
    }
}
