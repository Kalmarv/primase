//! Sekiro EDGE records: grapple points, hang segments and wall-hug segments.
use crate::{Result, bytes::View, invalid};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Grapple,
    Hang,
    Hug,
}
#[derive(Debug, Clone)]
pub struct Edge {
    pub points: [[f32; 3]; 3],
    pub parameter: f32,
    pub id: i32,
    pub kind: Kind,
    pub variation: u8,
    pub flags: u8,
}
#[derive(Debug, Clone)]
pub struct Edges {
    pub id: i32,
    pub edges: Vec<Edge>,
}
impl Edges {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let v = View {
            data: bytes,
            format: "EDGE",
            be: false,
        };
        v.expect(0, &4u32.to_le_bytes())?;
        let count = v.u32(4)? as usize;
        let length = count
            .checked_mul(64)
            .and_then(|n| n.checked_add(16))
            .ok_or_else(|| invalid("EDGE", 4, "size overflow"))?;
        if length != bytes.len() {
            return Err(invalid("EDGE", 4, "record count does not cover file"));
        }
        v.expect(12, &[0; 4])?;
        let mut edges = Vec::with_capacity(count);
        for i in 0..count {
            let start = 16 + i * 64;
            let r = View {
                data: v.slice(start, 64)?,
                format: "EDGE record",
                be: false,
            };
            r.expect(12, &1f32.to_le_bytes())?;
            r.expect(28, &1f32.to_le_bytes())?;
            r.expect(55, &[0; 9])?;
            let kind = match r.u8(52)? {
                1 => Kind::Grapple,
                2 => Kind::Hang,
                3 => Kind::Hug,
                _ => return Err(invalid("EDGE", start + 52, "unknown edge type")),
            };
            edges.push(Edge {
                points: [vector(&r, 0)?, vector(&r, 16)?, vector(&r, 32)?],
                parameter: float(&r, 44)?,
                id: r.u32(48)? as i32,
                kind,
                variation: r.u8(53)?,
                flags: r.u8(54)?,
            });
        }
        Ok(Self {
            id: v.u32(8)? as i32,
            edges,
        })
    }
}
pub(crate) fn float(v: &View<'_>, offset: usize) -> Result<f32> {
    let value = f32::from_bits(v.u32(offset)?);
    if !value.is_finite() {
        return Err(invalid(v.format, offset, "nonfinite float"));
    }
    Ok(value)
}
pub(crate) fn vector(v: &View<'_>, offset: usize) -> Result<[f32; 3]> {
    Ok([float(v, offset)?, float(v, offset + 4)?, float(v, offset + 8)?])
}
