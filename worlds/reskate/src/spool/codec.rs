//! The cell spool's record of a share (`skate.cell_share`), little-endian: the cell (water `u8`, column `i32`, row
//! `i32`); its soups (`u32` count; each: raw `u8`, planning id `u32`, ground `u8`, triangles `u32` n, then 9n `f32`
//! coordinates, n surface bytes, rock count `u32` and as many rock bytes); its props (`u32` count; each: tile column and
//! row `i32`, triangles `u32` n, 9n `f32` coordinates, n surface bytes, ground `u8` flag and, when set, n ground bytes).
//! A record is length-prefixed (`u32`), so a cell's file is its records one after another.

use crate::components::{CellProps, CellSoup, SkateCellShare};

/// `share` appended to `out` as one length-prefixed record.
pub fn encode(share: &SkateCellShare, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&0u32.to_le_bytes());
    let (water, x, y) = share.cell;
    out.push(u8::from(water));
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out.extend_from_slice(&(share.soups.len() as u32).to_le_bytes());
    for g in &share.soups {
        out.push(u8::from(g.order.0));
        out.extend_from_slice(&g.order.1.to_le_bytes());
        out.push(u8::from(g.ground));
        out.extend_from_slice(&(g.s.len() as u32).to_le_bytes());
        for v in &g.v {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&g.s);
        out.extend_from_slice(&(g.rock.len() as u32).to_le_bytes());
        out.extend(g.rock.iter().map(|&r| u8::from(r)));
    }
    out.extend_from_slice(&(share.props.len() as u32).to_le_bytes());
    for p in &share.props {
        out.extend_from_slice(&p.tile.0.to_le_bytes());
        out.extend_from_slice(&p.tile.1.to_le_bytes());
        out.extend_from_slice(&(p.surf.len() as u32).to_le_bytes());
        for v in &p.pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&p.surf);
        match &p.ground {
            Some(g) => {
                out.push(1);
                out.extend(g.iter().map(|&b| u8::from(b)));
            }
            None => out.push(0),
        }
    }
    let len = (out.len() - start - 4) as u32;
    out[start..start + 4].copy_from_slice(&len.to_le_bytes());
}

/// A reader over one record's bytes.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| {
                format!(
                    "a spooled share ends early, at byte {} of {}",
                    self.at,
                    self.b.len()
                )
            })?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn flag(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(format!("a spooled share has the flag {v}, not 0 or 1")),
        }
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    /// A count of items of at least `each` bytes, refused when the record cannot hold them.
    fn count(&mut self, each: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n.saturating_mul(each) > self.b.len() - self.at {
            return Err(format!("a spooled share counts {n} items its record cannot hold"));
        }
        Ok(n)
    }
    fn f32s(&mut self, n: usize) -> Result<Vec<f32>, String> {
        Ok(self
            .take(n.checked_mul(4).ok_or("a spooled share's size overflows")?)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c))
            .collect())
    }
    fn flags(&mut self, n: usize) -> Result<Vec<bool>, String> {
        self.take(n)?
            .iter()
            .map(|&v| match v {
                0 => Ok(false),
                1 => Ok(true),
                v => Err(format!("a spooled share has the flag {v}, not 0 or 1")),
            })
            .collect()
    }
}

/// Every record of `bytes` (a cell's file), in order.
pub fn decode(bytes: &[u8]) -> Result<Vec<SkateCellShare>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let len = bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
            .ok_or("a spooled record's length is cut short")?;
        let body = bytes
            .get(at + 4..at + 4 + len)
            .ok_or("a spooled record is cut short")?;
        out.push(record(body)?);
        at += 4 + len;
    }
    Ok(out)
}

fn record(b: &[u8]) -> Result<SkateCellShare, String> {
    let mut r = Rd { b, at: 0 };
    let cell = (r.flag()?, r.i32()?, r.i32()?);
    let n = r.count(10)?;
    let mut soups = Vec::with_capacity(n);
    for _ in 0..n {
        let order = (r.flag()?, r.u32()?);
        let ground = r.flag()?;
        let t = r.count(37)?;
        let v = r.f32s(t * 9)?;
        let s = r.take(t)?.to_vec();
        let nr = r.count(1)?;
        let rock = r.flags(nr)?;
        soups.push(CellSoup {
            order,
            ground,
            v,
            s,
            rock,
        });
    }
    let n = r.count(13)?;
    let mut props = Vec::with_capacity(n);
    for _ in 0..n {
        let tile = (r.i32()?, r.i32()?);
        let t = r.count(37)?;
        let pos = r.f32s(t * 9)?;
        let surf = r.take(t)?.to_vec();
        let ground = if r.flag()? { Some(r.flags(t)?) } else { None };
        props.push(CellProps {
            tile,
            pos,
            surf,
            ground,
        });
    }
    if r.at != b.len() {
        return Err(format!(
            "a spooled record has {} bytes past its share",
            b.len() - r.at
        ));
    }
    Ok(SkateCellShare { cell, soups, props })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share() -> SkateCellShare {
        SkateCellShare {
            cell: (true, -3, 7),
            soups: vec![CellSoup {
                order: (true, 41),
                ground: true,
                v: (0..18).map(|i| i as f32 * -0.5).collect(),
                s: vec![3, 0x85],
                rock: vec![false, true],
            }],
            props: vec![
                CellProps {
                    tile: (-1, 2),
                    pos: vec![f32::MIN_POSITIVE; 9],
                    surf: vec![7],
                    ground: Some(vec![true]),
                },
                CellProps {
                    tile: (4, -5),
                    pos: vec![-0.0; 9],
                    surf: vec![1],
                    ground: None,
                },
            ],
        }
    }

    /// Records round-trip bit for bit (negative zero and the smallest normal included), one after another in a file.
    #[test]
    fn records_round_trip_one_after_another() {
        let (a, mut b) = (share(), share());
        b.cell = (false, 0, 0);
        b.soups.clear();
        let mut bytes = Vec::new();
        encode(&a, &mut bytes);
        encode(&b, &mut bytes);
        let back = decode(&bytes).unwrap();
        assert_eq!(back, vec![a.clone(), b]);
        let bits = |s: &SkateCellShare| -> Vec<u32> { s.props[1].pos.iter().map(|v| v.to_bits()).collect() };
        assert_eq!(bits(&back[0]), bits(&a));
    }

    /// A record cut short, or with a count its bytes cannot hold, is refused, not read past.
    #[test]
    fn a_cut_or_overcounted_record_is_refused() {
        let mut bytes = Vec::new();
        encode(&share(), &mut bytes);
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        // the soups' count (after the length, the cell's 9 bytes) made huge
        let mut big = bytes.clone();
        big[13..17].copy_from_slice(&u32::MAX.to_le_bytes());
        let e = decode(&big).unwrap_err();
        assert!(e.contains("cannot hold"), "{e}");
    }
}
