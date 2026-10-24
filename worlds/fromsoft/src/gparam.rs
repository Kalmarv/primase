//! GPARAM, the graphics parameter file (`/param/drawparam/<map>_<id>.gparam.dcx`): groups of fields, each field a list
//! of values keyed by a value ID (and, from version 5, a time of day). Sekiro and Elden Ring store version 5; version 6
//! (Armored Core 6 and later) and 3 (Dark Souls 3, Bloodborne) share the layout with the differences below, and are read
//! the same way. Version 2 (Dark Souls 2: narrow magic, Shift-JIS strings) is refused by name.
//!
//! The layout follows SoulsFormats' `GPARAM` (TKGP's current version, as Smithbox carries it); its names for the levels are
//! used here: a *group* (SoulsFormats: param; the older SoulsFormats: group) holds *fields* (param). All offsets are from
//! the file's start:
//!
//! - `"f\0i\0l\0t\0"`, then the version (u32), a zero byte, a flag byte (kept), two zero bytes;
//! - the group count, `count14` (kept), the offsets of the group offsets, the group records, the field offsets, the field
//!   records, the value arrays and the value ID arrays; the offset of an opaque block (`data30`), the count of the opaque
//!   extras, the offsets of those and of their IDs; a float (`unk40`); the offsets of the comment tables (versions 3 and
//!   later); a float (`unk50`, version 5 and later);
//! - a group record: the field count, the offset of its field offsets (from the field-offsets base), its key and name
//!   (NUL-terminated UTF-16, one after the other);
//! - a field record: the offset of its values (from the values base), the offset of its IDs (from the IDs base), then
//!   the type byte, the value count and an unknown byte (version 5: type, count as a byte, unknown as a short; version 6:
//!   count as a short, type, unknown as a byte), then its key and name;
//! - a value is stored by its type (`Values`); a vector of two or three floats takes 16 bytes; the IDs are an `i32` each,
//!   with a time of day as `f32` after it from version 5.
//!
//! What this reads is everything but the three blocks it does not interpret, which [`Gparam::unread`] counts: the comment
//! tables, the opaque `data30` block and the extras (SoulsFormats: `UnkParamExtra`, which tie value IDs of groups
//! together).
use crate::{
    Result,
    bytes::{Budget, View},
    invalid,
};
use serde::Serialize;

const FORMAT: &str = "GPARAM";
/// The wide magic; the narrow `filt` of version 2 is refused.
const MAGIC: &[u8] = b"f\0i\0l\0t\0";

/// A GPARAM file, every group and field in file order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gparam {
    /// The stored version: 3, 5 or 6.
    pub version: u32,
    pub groups: Vec<Group>,
    /// What the file stores that is not read.
    pub unread: Unread,
}

/// The parts of the file [`Gparam::read`] does not interpret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Unread {
    /// The opaque extras: records that tie value IDs of a group together (their meaning is not established).
    pub extras: usize,
    /// The bytes of the opaque block before them.
    pub data_bytes: usize,
}

/// A group of fields (a lighting set, fog, a tone map): its key (the editor identity, `LightSet ParamEditor`) and name.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub fields: Vec<Field>,
}

/// One field: the values it holds for each value ID. `ids[i]` and `times[i]` (empty before version 5) belong to value
/// `i` of `values`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Field {
    pub key: String,
    pub name: String,
    pub ids: Vec<i32>,
    pub times: Vec<f32>,
    pub values: Values,
}

/// A field's values, by the type the file states.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Values {
    I8(Vec<i8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Bool(Vec<bool>),
    Vec2(Vec<[f32; 2]>),
    Vec3(Vec<[f32; 3]>),
    Vec4(Vec<[f32; 4]>),
    /// Four bytes, red first.
    Rgba(Vec<[u8; 4]>),
    Text(Vec<String>),
}

/// One value, as stored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Value {
    Int(i64),
    Uint(u64),
    Float(f64),
    Bool(bool),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Rgba([u8; 4]),
    Text(String),
}

impl Values {
    pub fn len(&self) -> usize {
        match self {
            Values::I8(v) => v.len(),
            Values::I16(v) => v.len(),
            Values::I32(v) => v.len(),
            Values::I64(v) => v.len(),
            Values::U8(v) => v.len(),
            Values::U16(v) => v.len(),
            Values::U32(v) => v.len(),
            Values::U64(v) => v.len(),
            Values::F32(v) => v.len(),
            Values::F64(v) => v.len(),
            Values::Bool(v) => v.len(),
            Values::Vec2(v) => v.len(),
            Values::Vec3(v) => v.len(),
            Values::Vec4(v) => v.len(),
            Values::Rgba(v) => v.len(),
            Values::Text(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Value `i`; `None` past the end.
    pub fn get(&self, i: usize) -> Option<Value> {
        Some(match self {
            Values::I8(v) => Value::Int(i64::from(*v.get(i)?)),
            Values::I16(v) => Value::Int(i64::from(*v.get(i)?)),
            Values::I32(v) => Value::Int(i64::from(*v.get(i)?)),
            Values::I64(v) => Value::Int(*v.get(i)?),
            Values::U8(v) => Value::Uint(u64::from(*v.get(i)?)),
            Values::U16(v) => Value::Uint(u64::from(*v.get(i)?)),
            Values::U32(v) => Value::Uint(u64::from(*v.get(i)?)),
            Values::U64(v) => Value::Uint(*v.get(i)?),
            Values::F32(v) => Value::Float(f64::from(*v.get(i)?)),
            Values::F64(v) => Value::Float(*v.get(i)?),
            Values::Bool(v) => Value::Bool(*v.get(i)?),
            Values::Vec2(v) => Value::Vec2(*v.get(i)?),
            Values::Vec3(v) => Value::Vec3(*v.get(i)?),
            Values::Vec4(v) => Value::Vec4(*v.get(i)?),
            Values::Rgba(v) => Value::Rgba(*v.get(i)?),
            Values::Text(v) => Value::Text(v.get(i)?.clone()),
        })
    }
}

impl Field {
    /// The values this field holds for value ID `id`, in file order (a version 5 field may hold several for one ID, at
    /// different times of day).
    pub fn indices_of(&self, id: i32) -> impl Iterator<Item = usize> + '_ {
        self.ids
            .iter()
            .enumerate()
            .filter(move |(_, v)| **v == id)
            .map(|(i, _)| i)
    }

    /// The first value stored for value ID `id`.
    pub fn value_of(&self, id: i32) -> Option<Value> {
        self.values.get(self.indices_of(id).next()?)
    }
}

impl Group {
    /// The field with key `key`.
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Every value ID some field of the group stores, ascending.
    pub fn ids(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self.fields.iter().flat_map(|f| f.ids.iter().copied()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

impl Gparam {
    /// The group with key `key`.
    pub fn group(&self, key: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.key == key)
    }

    /// Read a file (already decompressed). A version other than 3, 5 and 6, or the narrow magic, is refused by name;
    /// every count and offset is checked against the file before it is followed.
    pub fn read(data: &[u8]) -> Result<Self> {
        let v = View {
            data,
            format: FORMAT,
            be: false,
        };
        if data.starts_with(b"filt") {
            return Err(invalid(FORMAT, 0, "version 2 (narrow strings) is not read"));
        }
        v.expect(0, MAGIC)?;
        let version = v.u32(8)?;
        if !matches!(version, 3 | 5 | 6) {
            return Err(invalid(FORMAT, 8, format!("version {version} is not read (3, 5 and 6 are)")));
        }
        // a zero byte, the flag byte (not read), two zero bytes
        v.expect(12, &[0])?;
        v.expect(14, &[0, 0])?;
        let int = |o: usize| -> Result<usize> {
            usize::try_from(v.u32(o)?).map_err(|_| invalid(FORMAT, o, "size overflow"))
        };
        let group_count = int(16)?;
        let group_offsets = int(24)?;
        let groups_at = int(28)?;
        let field_offsets = int(32)?;
        let fields_at = int(36)?;
        let values_at = int(40)?;
        let ids_at = int(44)?;
        let data_at = int(48)?;
        let extras = int(52)?;
        let extras_at = int(56)?;
        let wide_ids = version >= 5;
        let header = if wide_ids { 84 } else { 80 };
        v.slice(0, header)?;
        // every record named is charged to the allowance, so records that name one range over and over are refused
        let budget = Budget::new(FORMAT, data.len());
        let group_table = v.slice(
            group_offsets,
            group_count
                .checked_mul(4)
                .ok_or_else(|| invalid(FORMAT, 16, "count overflow"))?,
        )?;
        budget.charge(group_offsets, group_table.len())?;
        let mut groups = Vec::with_capacity(group_count);
        for g in 0..group_count {
            let at = groups_at
                .checked_add(int(group_offsets + g * 4)?)
                .ok_or_else(|| invalid(FORMAT, group_offsets, "size overflow"))?;
            let field_count = int(at)?;
            let field_table = field_offsets
                .checked_add(int(at + 4)?)
                .ok_or_else(|| invalid(FORMAT, at, "size overflow"))?;
            let (key, end) = v.utf16z(at + 8, &budget)?;
            let (name, _) = v.utf16z(end, &budget)?;
            v.slice(
                field_table,
                field_count
                    .checked_mul(4)
                    .ok_or_else(|| invalid(FORMAT, at, "count overflow"))?,
            )?;
            budget.charge(field_table, field_count * 4)?;
            let mut fields = Vec::with_capacity(field_count);
            for f in 0..field_count {
                let record = fields_at
                    .checked_add(int(field_table + f * 4)?)
                    .ok_or_else(|| invalid(FORMAT, field_table, "size overflow"))?;
                fields.push(read_field(&v, version, record, values_at, ids_at, &budget)?);
            }
            groups.push(Group { key, name, fields });
        }
        let unread = Unread {
            extras,
            data_bytes: extras_at.saturating_sub(data_at),
        };
        Ok(Gparam {
            version,
            groups,
            unread,
        })
    }
}

/// The type byte's name and size, by the file's type codes: SoulsFormats' `FieldType`.
fn read_field(
    v: &View<'_>,
    version: u32,
    record: usize,
    values_at: usize,
    ids_at: usize,
    budget: &Budget,
) -> Result<Field> {
    let values_offset = usize::try_from(v.u32(record)?).map_err(|_| invalid(FORMAT, record, "size overflow"))?;
    let ids_offset = usize::try_from(v.u32(record + 4)?).map_err(|_| invalid(FORMAT, record + 4, "size overflow"))?;
    let (kind, capacity) = if version < 6 {
        (v.u8(record + 8)?, usize::from(v.u8(record + 9)?))
    } else {
        (v.u8(record + 10)?, usize::from(v.u16(record + 8)?))
    };
    let (key, end) = v.utf16z(record + 12, budget)?;
    let (name, _) = v.utf16z(end, budget)?;
    let mut at = values_at
        .checked_add(values_offset)
        .ok_or_else(|| invalid(FORMAT, record, "size overflow"))?;
    let values = read_values(v, kind, capacity, &mut at, budget)?;
    let mut ids = Vec::with_capacity(capacity);
    let mut times = Vec::new();
    let mut id_at = ids_at
        .checked_add(ids_offset)
        .ok_or_else(|| invalid(FORMAT, record + 4, "size overflow"))?;
    let stride = if version >= 5 { 8 } else { 4 };
    v.slice(id_at, capacity * stride)?;
    budget.charge(id_at, capacity * stride)?;
    for _ in 0..capacity {
        ids.push(v.u32(id_at)? as i32);
        if version >= 5 {
            times.push(f32::from_bits(v.u32(id_at + 4)?));
        }
        id_at += stride;
    }
    Ok(Field {
        key,
        name,
        ids,
        times,
        values,
    })
}

fn read_values(v: &View<'_>, kind: u8, n: usize, at: &mut usize, budget: &Budget) -> Result<Values> {
    // bytes one value takes in the file
    let size = match kind {
        1 | 5 | 11 => 1,
        2 | 6 => 2,
        3 | 7 | 9 | 15 => 4,
        4 | 8 | 10 => 8,
        12..=14 => 16,
        16 => 0,
        other => return Err(invalid(FORMAT, *at, format!("field type {other}"))),
    };
    if size > 0 {
        v.slice(*at, n * size)?;
        budget.charge(*at, n * size)?;
    }
    let start = *at;
    let mut each = |width: usize| -> Result<usize> {
        let o = *at;
        *at += width;
        Ok(o)
    };
    // `take` reads n values of `width` bytes with `f`
    macro_rules! take {
        ($width:expr, $f:expr) => {{
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                let o = each($width)?;
                out.push($f(o)?);
            }
            out
        }};
    }
    let f32_at = |o: usize| -> Result<f32> { Ok(f32::from_bits(v.u32(o)?)) };
    let result = match kind {
        1 => Values::I8(take!(1, |o| v.u8(o).map(|b| b as i8))),
        2 => Values::I16(take!(2, |o| v.u16(o).map(|b| b as i16))),
        3 => Values::I32(take!(4, |o| v.u32(o).map(|b| b as i32))),
        4 => Values::I64(take!(8, |o| v.u64(o).map(|b| b as i64))),
        5 => Values::U8(take!(1, |o| v.u8(o))),
        6 => Values::U16(take!(2, |o| v.u16(o))),
        7 => Values::U32(take!(4, |o| v.u32(o))),
        8 => Values::U64(take!(8, |o| v.u64(o))),
        9 => Values::F32(take!(4, f32_at)),
        10 => Values::F64(take!(8, |o| v.u64(o).map(f64::from_bits))),
        11 => Values::Bool(take!(1, |o| v.boolean(o))),
        12 => Values::Vec2(take!(16, |o| -> Result<[f32; 2]> {
            Ok([f32_at(o)?, f32_at(o + 4)?])
        })),
        13 => Values::Vec3(take!(16, |o| -> Result<[f32; 3]> {
            Ok([f32_at(o)?, f32_at(o + 4)?, f32_at(o + 8)?])
        })),
        14 => Values::Vec4(take!(16, |o| -> Result<[f32; 4]> {
            Ok([f32_at(o)?, f32_at(o + 4)?, f32_at(o + 8)?, f32_at(o + 12)?])
        })),
        15 => Values::Rgba(take!(4, |o| -> Result<[u8; 4]> {
            Ok([v.u8(o)?, v.u8(o + 1)?, v.u8(o + 2)?, v.u8(o + 3)?])
        })),
        _ => {
            let mut out = Vec::with_capacity(n.min(v.data.len()));
            let mut o = start;
            for _ in 0..n {
                let (text, end) = v.utf16z(o, budget)?;
                out.push(text);
                o = end;
            }
            *at = o;
            Values::Text(out)
        }
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16z(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    fn pad4(b: &mut Vec<u8>) {
        while b.len() % 4 != 0 {
            b.push(0);
        }
    }

    /// One field to build: its type code, its encoded values and the IDs (and times of day) of them.
    struct F {
        key: &'static str,
        name: &'static str,
        kind: u8,
        count: u16,
        values: Vec<u8>,
        ids: Vec<(i32, f32)>,
    }

    fn floats(fields: &[f32]) -> Vec<u8> {
        fields.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn float_field(key: &'static str, ids: &[(i32, f32)], v: &[f32]) -> F {
        F {
            key,
            name: "name",
            kind: 9,
            count: v.len() as u16,
            values: floats(v),
            ids: ids.to_vec(),
        }
    }

    fn vec4_field(key: &'static str, ids: &[(i32, f32)], v: &[[f32; 4]]) -> F {
        F {
            key,
            name: "name",
            kind: 14,
            count: v.len() as u16,
            values: v.iter().flat_map(|c| floats(c)).collect(),
            ids: ids.to_vec(),
        }
    }

    /// A file of `version` laid out as SoulsFormats writes it: the header, the group offsets, the group records, the field
    /// offsets, the field records, the values, the IDs, the opaque block, the extras (two, with one ID each) and the
    /// comment tables (empty, at the end).
    fn build(version: u32, groups: &[(&str, &str, Vec<F>)]) -> Vec<u8> {
        let header = if version >= 5 { 84 } else { 80 };
        let mut values = Vec::new();
        let mut ids = Vec::new();
        let mut fields = Vec::new();
        let mut table: Vec<Vec<usize>> = Vec::new();
        for (_, _, group_fields) in groups {
            let mut offsets = Vec::new();
            for f in group_fields {
                offsets.push(fields.len());
                fields.extend((values.len() as u32).to_le_bytes());
                fields.extend((ids.len() as u32).to_le_bytes());
                if version < 6 {
                    fields.extend([f.kind, f.count as u8, 0, 0]);
                } else {
                    fields.extend(f.count.to_le_bytes());
                    fields.extend([f.kind, 0]);
                }
                fields.extend(utf16z(f.key));
                fields.extend(utf16z(f.name));
                pad4(&mut fields);
                values.extend(&f.values);
                pad4(&mut values);
                for (id, time) in &f.ids {
                    ids.extend(id.to_le_bytes());
                    if version >= 5 {
                        ids.extend(time.to_le_bytes());
                    }
                }
            }
            table.push(offsets);
        }
        let mut field_offsets = Vec::new();
        let mut table_at = Vec::new();
        for offsets in &table {
            table_at.push(field_offsets.len());
            for o in offsets {
                field_offsets.extend((*o as u32).to_le_bytes());
            }
        }
        let mut records = Vec::new();
        let mut record_at = Vec::new();
        for (g, (key, name, group_fields)) in groups.iter().enumerate() {
            record_at.push(records.len());
            records.extend((group_fields.len() as u32).to_le_bytes());
            records.extend((table_at[g] as u32).to_le_bytes());
            records.extend(utf16z(key));
            records.extend(utf16z(name));
            pad4(&mut records);
        }
        let group_offsets_at = header;
        let groups_at = group_offsets_at + groups.len() * 4;
        let field_offsets_at = groups_at + records.len();
        let fields_at = field_offsets_at + field_offsets.len();
        let values_at = fields_at + fields.len();
        let ids_at = values_at + values.len();
        let data_at = ids_at + ids.len();
        let data = [7u8; 8];
        let extras_at = data_at + data.len();
        let extra_ids_at = extras_at + 2 * if version >= 5 { 16 } else { 12 };
        let end = extra_ids_at + 8;
        let mut out = Vec::new();
        out.extend(MAGIC);
        out.extend(version.to_le_bytes());
        out.extend([0, 1, 0, 0]);
        for word in [
            groups.len() as u32,
            0,
            group_offsets_at as u32,
            groups_at as u32,
            field_offsets_at as u32,
            fields_at as u32,
            values_at as u32,
            ids_at as u32,
            data_at as u32,
            2,
            extras_at as u32,
            extra_ids_at as u32,
        ] {
            out.extend(word.to_le_bytes());
        }
        out.extend(0.5f32.to_le_bytes());
        for word in [end as u32; 3] {
            out.extend(word.to_le_bytes());
        }
        if version >= 5 {
            out.extend(0.25f32.to_le_bytes());
        }
        assert_eq!(out.len(), header);
        for at in &record_at {
            out.extend((*at as u32).to_le_bytes());
        }
        out.extend(&records);
        out.extend(&field_offsets);
        out.extend(&fields);
        out.extend(&values);
        out.extend(&ids);
        out.extend(data);
        for extra in 0..2u32 {
            out.extend(0u32.to_le_bytes());
            out.extend(1u32.to_le_bytes());
            out.extend((extra * 4).to_le_bytes());
            if version >= 5 {
                out.extend(0u32.to_le_bytes());
            }
        }
        out.extend([1u8, 0, 0, 0, 2, 0, 0, 0]);
        out
    }

    fn sample(version: u32) -> Vec<u8> {
        let light = vec![
            float_field("Sharpness", &[(0, 0.0), (7, 0.5), (7, 12.0)], &[1.0, 2.0, 3.0]),
            vec4_field("Colour", &[(7, 0.0)], &[[0.25, 0.5, 0.75, 1.0]]),
            F {
                key: "Angle",
                name: "n",
                kind: 12,
                count: 1,
                values: floats(&[10.0, -20.0, 0.0, 0.0]),
                ids: vec![(7, 0.0)],
            },
            F {
                key: "Axis",
                name: "n",
                kind: 13,
                count: 1,
                values: floats(&[1.0, 2.0, 3.0, 0.0]),
                ids: vec![(7, 0.0)],
            },
        ];
        let misc = vec![
            F {
                key: "Enable",
                name: "n",
                kind: 11,
                count: 1,
                values: vec![1],
                ids: vec![(0, 0.0)],
            },
            F {
                key: "Count",
                name: "n",
                kind: 3,
                count: 2,
                values: [5i32, -6].iter().flat_map(|i| i.to_le_bytes()).collect(),
                ids: vec![(0, 0.0), (1, 0.0)],
            },
            F {
                key: "Tint",
                name: "n",
                kind: 15,
                count: 1,
                values: vec![255, 128, 0, 255],
                ids: vec![(3, 0.0)],
            },
            F {
                key: "Label",
                name: "n",
                kind: 16,
                count: 2,
                values: [utf16z("a"), utf16z("bc")].concat(),
                ids: vec![(0, 0.0), (1, 0.0)],
            },
        ];
        build(
            version,
            &[("LightSet ParamEditor", "Light Set", light), ("Misc", "Misc", misc)],
        )
    }

    #[test]
    fn version_five_reads_groups_fields_values_ids_and_times() {
        let g = Gparam::read(&sample(5)).unwrap();
        assert_eq!(g.version, 5);
        assert_eq!(g.groups.len(), 2);
        let light = g.group("LightSet ParamEditor").unwrap();
        assert_eq!(light.name, "Light Set");
        assert_eq!(light.ids(), [0, 7]);
        let sharp = light.field("Sharpness").unwrap();
        assert_eq!(sharp.ids, [0, 7, 7]);
        assert_eq!(sharp.times, [0.0, 0.5, 12.0]);
        assert_eq!(sharp.values, Values::F32(vec![1.0, 2.0, 3.0]));
        // the first of the values stored for an ID, and every index of it
        assert_eq!(sharp.value_of(7), Some(Value::Float(2.0)));
        assert_eq!(sharp.indices_of(7).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(sharp.value_of(9), None);
        assert_eq!(
            light.field("Colour").unwrap().value_of(7),
            Some(Value::Vec4([0.25, 0.5, 0.75, 1.0]))
        );
        assert_eq!(light.field("Angle").unwrap().value_of(7), Some(Value::Vec2([10.0, -20.0])));
        assert_eq!(light.field("Axis").unwrap().value_of(7), Some(Value::Vec3([1.0, 2.0, 3.0])));
        let misc = g.group("Misc").unwrap();
        assert_eq!(misc.field("Enable").unwrap().value_of(0), Some(Value::Bool(true)));
        assert_eq!(misc.field("Count").unwrap().values, Values::I32(vec![5, -6]));
        assert_eq!(misc.field("Tint").unwrap().value_of(3), Some(Value::Rgba([255, 128, 0, 255])));
        assert_eq!(
            misc.field("Label").unwrap().values,
            Values::Text(vec!["a".into(), "bc".into()])
        );
        // the blocks it does not interpret are counted
        assert_eq!(g.unread, Unread { extras: 2, data_bytes: 8 });
        assert!(light.field("Missing").is_none() && g.group("Missing").is_none());
    }

    #[test]
    fn versions_three_and_six_differ_in_the_header_and_the_field_record() {
        // version 3 stores no times of day and a shorter header; version 6 a field's count as a short
        let three = Gparam::read(&sample(3)).unwrap();
        assert_eq!(three.version, 3);
        let sharp = three.group("LightSet ParamEditor").unwrap().field("Sharpness").unwrap();
        assert_eq!(sharp.ids, [0, 7, 7]);
        assert!(sharp.times.is_empty());
        assert_eq!(sharp.values, Values::F32(vec![1.0, 2.0, 3.0]));
        let six = Gparam::read(&sample(6)).unwrap();
        assert_eq!(six.version, 6);
        let sharp = six.group("LightSet ParamEditor").unwrap().field("Sharpness").unwrap();
        assert_eq!(sharp.times, [0.0, 0.5, 12.0]);
        assert_eq!(sharp.value_of(7), Some(Value::Float(2.0)));
        // a field with more than 255 values only a version 6 record can count
        let many: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let ids: Vec<(i32, f32)> = (0..300).map(|i| (i, 0.0)).collect();
        let big = build(6, &[("G", "G", vec![float_field("Many", &ids, &many)])]);
        let field = &Gparam::read(&big).unwrap().groups[0].fields[0];
        assert_eq!(field.values.len(), 300);
        assert_eq!(field.value_of(299), Some(Value::Float(299.0)));
    }

    #[test]
    fn what_is_not_a_readable_version_is_refused_by_name() {
        let mut two = sample(5);
        two[8] = 2;
        assert!(Gparam::read(&two).unwrap_err().to_string().contains("version 2"));
        let mut four = sample(5);
        four[8] = 4;
        assert!(Gparam::read(&four).unwrap_err().to_string().contains("version 4"));
        assert!(
            Gparam::read(b"filt\x02\0\0\0")
                .unwrap_err()
                .to_string()
                .contains("narrow")
        );
        assert!(Gparam::read(b"nope").is_err());
        assert!(Gparam::read(&[]).is_err());
        // an unknown field type is named
        let mut bad = sample(5);
        let at = bad
            .windows(utf16z("Sharpness").len())
            .position(|w| w == utf16z("Sharpness"))
            .unwrap();
        bad[at - 4] = 99;
        assert!(Gparam::read(&bad).unwrap_err().to_string().contains("field type 99"));
    }

    #[test]
    fn a_truncated_file_or_a_count_beyond_it_is_refused_not_followed() {
        let whole = sample(5);
        for cut in [0, 7, 40, 84, 100, whole.len() / 2] {
            assert!(Gparam::read(&whole[..cut]).is_err(), "cut at {cut}");
        }
        let mut huge = sample(5);
        // a group count that its table cannot hold
        huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Gparam::read(&huge).is_err());
    }
}
