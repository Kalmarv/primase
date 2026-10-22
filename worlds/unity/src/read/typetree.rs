//! Type trees: a class's serialized layout, and decoding an object's bytes by it.
//!
//! The rules: primitives by their type name; `string` is a 32-bit length, the UTF-8 bytes, then 4-byte alignment;
//! `TypelessData` a 32-bit length and the bytes; `pair` its two children; a node whose first child is `Array` is a
//! vector (a 32-bit count, then the elements, each by the array's second child); anything else is a struct of its
//! children in order. After any node whose meta flag has `ALIGN`, the position is aligned to 4 bytes (from the
//! object's start); an `Array` child's flag aligns its vector.

use std::sync::Arc;

use super::Reader;
use crate::read::value::{Array, Value};
use crate::{Error, Result, invalid};

/// The meta flag that aligns the position to 4 bytes after the node.
pub const ALIGN: u32 = 0x4000;

/// One node: its type and field name, its byte size (-1: variable), version, meta flags and children (indices into
/// the tree's node list).
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub ty: Arc<str>,
    pub name: Arc<str>,
    pub byte_size: i32,
    pub version: i32,
    pub meta_flag: u32,
    pub children: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prim {
    Bool,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Prim {
    fn of(ty: &str) -> Option<Prim> {
        Some(match ty {
            "bool" => Prim::Bool,
            "SInt8" => Prim::I8,
            "UInt8" | "char" => Prim::U8,
            "short" | "SInt16" => Prim::I16,
            "unsigned short" | "UInt16" => Prim::U16,
            "int" | "SInt32" => Prim::I32,
            "unsigned int" | "UInt32" | "Type*" => Prim::U32,
            "long long" | "SInt64" => Prim::I64,
            "unsigned long long" | "UInt64" | "FileSize" => Prim::U64,
            "float" => Prim::F32,
            "double" => Prim::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Prim::Bool | Prim::I8 | Prim::U8 => 1,
            Prim::I16 | Prim::U16 => 2,
            Prim::I32 | Prim::U32 | Prim::F32 => 4,
            Prim::I64 | Prim::U64 | Prim::F64 => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Prim(Prim),
    String,
    Typeless,
    Pair,
    /// The element node; whether the `Array` child aligns.
    Vector {
        element: usize,
        aligned: bool,
    },
    /// Managed references (`[SerializeReference]`), which need the file's own reference type trees.
    Referenced,
    Struct,
}

/// A class's type tree; node 0 is the root.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeTree {
    pub nodes: Vec<Node>,
    kinds: Vec<Kind>,
}

impl TypeTree {
    pub fn new(nodes: Vec<Node>) -> Result<Self> {
        if nodes.is_empty() {
            return Err(invalid("empty type tree"));
        }
        let mut kinds = Vec::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            if n.children.iter().any(|&c| c <= i || c >= nodes.len()) {
                return Err(invalid(format!("type tree node {i} has a bad child index")));
            }
            let first = n.children.first().map(|&c| &nodes[c]);
            let kind = if let Some(p) = Prim::of(&n.ty) {
                Kind::Prim(p)
            } else if &*n.ty == "string" {
                Kind::String
            } else if &*n.ty == "TypelessData" {
                Kind::Typeless
            } else if &*n.ty == "pair" {
                if n.children.len() != 2 {
                    return Err(invalid(format!(
                        "pair node {i} has {} children",
                        n.children.len()
                    )));
                }
                Kind::Pair
            } else if &*n.ty == "ReferencedObject" {
                Kind::Referenced
            } else if let Some(a) = first.filter(|f| &*f.ty == "Array") {
                let ac = n.children[0];
                if a.children.len() != 2 {
                    return Err(invalid(format!(
                        "Array node {ac} has {} children",
                        a.children.len()
                    )));
                }
                Kind::Vector {
                    element: a.children[1],
                    aligned: a.meta_flag & ALIGN != 0,
                }
            } else {
                Kind::Struct
            };
            kinds.push(kind);
        }
        Ok(TypeTree { nodes, kinds })
    }

    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    /// Decode `data` (an object's bytes) from its start; returns the value and the bytes consumed.
    pub fn decode(&self, data: &[u8], big_endian: bool) -> Result<(Value, usize)> {
        let mut r = Reader::new(data, big_endian);
        let v = self.read(0, &mut r, 0)?;
        Ok((v, r.pos))
    }

    /// The root struct's field `name` alone: the fields before it are walked (and so must decode), the ones after it
    /// are not read, so a name stored first costs a few bytes whatever the object holds. `None` when the root has no
    /// such field; the value is what [`TypeTree::decode`] gives it. A root that is not a struct has no fields.
    pub fn decode_field(&self, data: &[u8], big_endian: bool, name: &str) -> Result<Option<Value>> {
        if !matches!(self.kinds[0], Kind::Struct) {
            return Ok(None);
        }
        let mut r = Reader::new(data, big_endian);
        for &c in &self.nodes[0].children {
            let v = self.read(c, &mut r, 1)?;
            if &*self.nodes[c].name == name {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    fn read(&self, i: usize, r: &mut Reader, depth: u32) -> Result<Value> {
        if depth > 128 {
            return Err(invalid("type tree recursion deeper than 128"));
        }
        let n = &self.nodes[i];
        let v = match &self.kinds[i] {
            Kind::Prim(p) => prim(*p, r)?,
            Kind::String => {
                let len = length(r, 1)?;
                let b = r.take(len)?;
                let s = std::str::from_utf8(b)
                    .map_err(|_| invalid(format!("{}: string is not UTF-8", n.name)))?
                    .to_owned();
                r.align(4)?;
                Value::String(s)
            }
            Kind::Typeless => {
                let len = length(r, 1)?;
                Value::Array(Array::U8(r.take(len)?.to_vec()))
            }
            Kind::Pair => {
                let a = self.read(n.children[0], r, depth + 1)?;
                let b = self.read(n.children[1], r, depth + 1)?;
                Value::Pair(Box::new((a, b)))
            }
            Kind::Vector { element, aligned } => {
                let v = self.vector(*element, r, depth)?;
                if *aligned {
                    r.align(4)?;
                }
                v
            }
            Kind::Referenced => {
                return Err(Error::Unsupported(format!(
                    "{}: managed reference (needs the file's reference type trees)",
                    n.name
                )));
            }
            Kind::Struct => {
                let mut fields = Vec::with_capacity(n.children.len());
                for &c in &n.children {
                    fields.push((self.nodes[c].name.clone(), self.read(c, r, depth + 1)?));
                }
                Value::Struct(fields)
            }
        };
        if n.meta_flag & ALIGN != 0 {
            r.align(4)?;
        }
        Ok(v)
    }

    fn vector(&self, e: usize, r: &mut Reader, depth: u32) -> Result<Value> {
        let en = &self.nodes[e];
        // a primitive element without its own alignment: the elements packed
        if let Kind::Prim(p) = self.kinds[e]
            && en.meta_flag & ALIGN == 0
        {
            let n = length(r, p.size())?;
            return Ok(Value::Array(prim_array(p, n, r)?));
        }
        // the shortest an element can be, to bound the count by the bytes left
        let min = match self.kinds[e] {
            Kind::Prim(p) => p.size(),
            Kind::String | Kind::Typeless | Kind::Vector { .. } => 4,
            _ => 0,
        };
        let n = length(r, min)?;
        let mut out = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            out.push(self.read(e, r, depth + 1)?);
        }
        Ok(Value::Array(Array::Values(out)))
    }
}

/// A 32-bit length or count, checked against the bytes left at `unit` bytes per element.
fn length(r: &mut Reader, unit: usize) -> Result<usize> {
    let at = r.pos;
    let n = r.i32()?;
    let n = usize::try_from(n).map_err(|_| invalid(format!("negative length {n} at offset {at}")))?;
    if n.saturating_mul(unit) > r.remaining() {
        return Err(invalid(format!(
            "length {n} at offset {at} runs past the object ({} bytes left)",
            r.remaining()
        )));
    }
    Ok(n)
}

fn prim(p: Prim, r: &mut Reader) -> Result<Value> {
    Ok(match p {
        Prim::Bool => Value::Bool(r.bool()?),
        Prim::I8 => Value::Int(i64::from(r.i8()?)),
        Prim::U8 => Value::UInt(u64::from(r.u8()?)),
        Prim::I16 => Value::Int(i64::from(r.i16()?)),
        Prim::U16 => Value::UInt(u64::from(r.u16()?)),
        Prim::I32 => Value::Int(i64::from(r.i32()?)),
        Prim::U32 => Value::UInt(u64::from(r.u32()?)),
        Prim::I64 => Value::Int(r.i64()?),
        Prim::U64 => Value::UInt(r.u64()?),
        Prim::F32 => Value::Float(r.f32()?),
        Prim::F64 => Value::Double(r.f64()?),
    })
}

fn prim_array(p: Prim, n: usize, r: &mut Reader) -> Result<Array> {
    macro_rules! many {
        ($f:ident, $v:ident) => {
            Array::$v((0..n).map(|_| r.$f()).collect::<Result<Vec<_>>>()?)
        };
    }
    Ok(match p {
        Prim::U8 => Array::U8(r.take(n)?.to_vec()),
        Prim::Bool => many!(bool, Bool),
        Prim::I8 => many!(i8, I8),
        Prim::I16 => many!(i16, I16),
        Prim::U16 => many!(u16, U16),
        Prim::I32 => many!(i32, I32),
        Prim::U32 => many!(u32, U32),
        Prim::I64 => many!(i64, I64),
        Prim::U64 => many!(u64, U64),
        Prim::F32 => many!(f32, F32),
        Prim::F64 => many!(f64, F64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(ty: &str, name: &str, meta: u32, children: Vec<usize>) -> Node {
        Node {
            ty: ty.into(),
            name: name.into(),
            byte_size: -1,
            version: 1,
            meta_flag: meta,
            children,
        }
    }

    /// A struct of a bool, an aligned string, a vector of floats and a pair, decoded and aligned as the rules say.
    #[test]
    fn decodes_by_the_rules() {
        let t = TypeTree::new(vec![
            node("Thing", "Base", 0, vec![1, 2, 6, 10]),
            node("bool", "m_On", ALIGN, vec![]),
            node("string", "m_Name", 0, vec![3]),
            node("Array", "Array", ALIGN, vec![4, 5]),
            node("int", "size", 0, vec![]),
            node("char", "data", 0, vec![]),
            node("vector", "m_V", 0, vec![7]),
            node("Array", "Array", 0, vec![8, 9]),
            node("int", "size", 0, vec![]),
            node("float", "data", 0, vec![]),
            node("pair", "m_P", 0, vec![11, 12]),
            node("int", "first", 0, vec![]),
            node("UInt8", "second", ALIGN, vec![]),
        ])
        .unwrap();
        let mut d = vec![1, 0, 0, 0];
        d.extend_from_slice(&3i32.to_le_bytes());
        d.extend_from_slice(b"abc\0");
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&1.5f32.to_le_bytes());
        d.extend_from_slice(&(-2.0f32).to_le_bytes());
        d.extend_from_slice(&7i32.to_le_bytes());
        d.extend_from_slice(&[9, 0, 0, 0]);
        let (v, used) = t.decode(&d, false).unwrap();
        assert_eq!(used, d.len());
        assert_eq!(v.get("m_On"), Some(&Value::Bool(true)));
        assert_eq!(v.get("m_Name").and_then(Value::as_str), Some("abc"));
        assert_eq!(v.get("m_V"), Some(&Value::Array(Array::F32(vec![1.5, -2.0]))));
        assert_eq!(
            v.get("m_P"),
            Some(&Value::Pair(Box::new((Value::Int(7), Value::UInt(9)))))
        );
        // one field alone: what decode gives it, the fields after it unread (a broken tail does not matter), none for a
        // name the root lacks
        for name in ["m_On", "m_Name", "m_V", "m_P"] {
            assert_eq!(
                t.decode_field(&d, false, name).unwrap().as_ref(),
                v.get(name),
                "{name}"
            );
        }
        assert_eq!(t.decode_field(&d, false, "m_Missing").unwrap(), None);
        assert_eq!(
            t.decode_field(&d[..12], false, "m_Name")
                .unwrap()
                .as_ref()
                .and_then(Value::as_str),
            Some("abc"),
            "the name decodes from the bytes up to it"
        );
        assert!(t.decode_field(&d[..10], false, "m_Name").is_err());
        // a count past the data and a bad bool are errors
        let mut short = d.clone();
        short[12..16].copy_from_slice(&1000i32.to_le_bytes());
        assert!(t.decode(&short, false).is_err());
        let mut b = d.clone();
        b[0] = 2;
        assert!(t.decode(&b, false).is_err());
    }
}
