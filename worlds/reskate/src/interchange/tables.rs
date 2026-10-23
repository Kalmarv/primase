//! The binary tables of scene-interchange v1: a 16-byte header (`SKST`, version 1, kind, record count), fixed-size
//! little-endian records, arrays after them for some kinds, nothing after the end.
//!
//! These are the file records exactly as stored; the reader turns them into components. Readers check magic,
//! version, kind, the exact size, every float (finite) and the indices a table can check on its own.

pub const MAGIC: [u8; 4] = *b"SKST";
pub const VERSION: u32 = 1;
const PLACEMENT_SIZE: usize = 76;
const LIGHT_SIZE: usize = 92;
const CURVE_SIZE: usize = 20;

/// Placement flags.
pub mod flags {
    pub const TILE: u32 = 1;
    pub const RADIUS: u32 = 2;
    pub const CLIP_GIVEN: u32 = 4;
    pub const CLIP: u32 = 8;
    pub const NO_COLLISION: u32 = 16;
    pub const SPLIT: u32 = 32;
    /// Light flags.
    pub const CAST_SHADOW: u32 = 1;
    pub const PRIORITY: u32 = 2;
    /// Curve flags.
    pub const CLOSED: u32 = 1;
}

/// A `PLAC` record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacementRec {
    pub proto: u32,
    pub variant: u32,
    /// 3x4 column by column.
    pub m: [f32; 12],
    pub tile: [i32; 2],
    pub radius: f32,
    pub flags: u32,
    /// Index into `placement_sources`, `u32::MAX` none.
    pub source: u32,
}

/// An `LGHT` record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightRec {
    /// 0 point, 1 spot.
    pub kind: u32,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub tangent: [f32; 3],
    pub color: [f32; 3],
    pub energy: f32,
    pub range: f32,
    pub radius: f32,
    pub spot_size: f32,
    pub spot_blend: f32,
    pub time_of_day: u32,
    pub flags: u32,
    pub priority: f32,
    pub group: u32,
    pub source: u32,
}

/// A `CURV` record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveRec {
    pub first: u32,
    pub count: u32,
    pub flags: u32,
    pub radius: f32,
    pub surface: u32,
}

/// A `CURV` table: the curves and the points they index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveTable {
    pub curves: Vec<CurveRec>,
    pub points: Vec<[f32; 3]>,
    /// The u32 after the point count (written 0).
    pub reserved: u32,
}

/// A `SOUP` table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoupTable {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub surfaces: Vec<u8>,
    /// The u32 after the vertex count (written 0).
    pub reserved: u32,
}

/// What broke, with the byte offset or record where it did.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TableError(pub String);

type Result<T> = std::result::Result<T, TableError>;

fn header(kind: &[u8; 4], count: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(kind);
    b.extend_from_slice(
        &u32::try_from(count)
            .expect("table over u32::MAX records")
            .to_le_bytes(),
    );
    b
}

/// The record count after checking magic, version and kind.
fn read_header(b: &[u8], kind: &[u8; 4]) -> Result<u32> {
    if b.len() < 16 || b[0..4] != MAGIC {
        return Err(TableError("not an SKST table".into()));
    }
    let v = u32_at(b, 4);
    if v != VERSION {
        return Err(TableError(format!(
            "table version {v}, this reader takes {VERSION}"
        )));
    }
    if &b[8..12] != kind {
        return Err(TableError(format!(
            "a {} table where a {} table was expected",
            String::from_utf8_lossy(&b[8..12]),
            String::from_utf8_lossy(kind)
        )));
    }
    Ok(u32_at(b, 12))
}

fn size(b: &[u8], want: u64) -> Result<()> {
    if b.len() as u64 != want {
        return Err(TableError(format!("{} bytes, the header says {want}", b.len())));
    }
    Ok(())
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    let f = f32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"));
    if !f.is_finite() {
        return Err(TableError(format!("a non-finite float at byte {o}")));
    }
    Ok(f)
}

fn v3_at(b: &[u8], o: usize) -> Result<[f32; 3]> {
    Ok([f32_at(b, o)?, f32_at(b, o + 4)?, f32_at(b, o + 8)?])
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(b: &mut Vec<u8>, v: f32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_v3(b: &mut Vec<u8>, v: [f32; 3]) {
    for x in v {
        put_f32(b, x);
    }
}

pub fn read_placements(b: &[u8]) -> Result<Vec<PlacementRec>> {
    let n = read_header(b, b"PLAC")?;
    size(b, 16 + u64::from(n) * PLACEMENT_SIZE as u64)?;
    (0..n as usize)
        .map(|i| {
            let o = 16 + i * PLACEMENT_SIZE;
            let mut m = [0.0; 12];
            for (k, x) in m.iter_mut().enumerate() {
                *x = f32_at(b, o + 8 + k * 4)?;
            }
            Ok(PlacementRec {
                proto: u32_at(b, o),
                variant: u32_at(b, o + 4),
                m,
                tile: [u32_at(b, o + 56) as i32, u32_at(b, o + 60) as i32],
                radius: f32_at(b, o + 64)?,
                flags: u32_at(b, o + 68),
                source: u32_at(b, o + 72),
            })
        })
        .collect()
}

pub fn write_placements(recs: &[PlacementRec]) -> Vec<u8> {
    let mut b = header(b"PLAC", recs.len());
    b.reserve(recs.len() * PLACEMENT_SIZE);
    for r in recs {
        put_u32(&mut b, r.proto);
        put_u32(&mut b, r.variant);
        for x in r.m {
            put_f32(&mut b, x);
        }
        put_u32(&mut b, r.tile[0] as u32);
        put_u32(&mut b, r.tile[1] as u32);
        put_f32(&mut b, r.radius);
        put_u32(&mut b, r.flags);
        put_u32(&mut b, r.source);
    }
    b
}

pub fn read_lights(b: &[u8]) -> Result<Vec<LightRec>> {
    let n = read_header(b, b"LGHT")?;
    size(b, 16 + u64::from(n) * LIGHT_SIZE as u64)?;
    (0..n as usize)
        .map(|i| {
            let o = 16 + i * LIGHT_SIZE;
            Ok(LightRec {
                kind: u32_at(b, o),
                position: v3_at(b, o + 4)?,
                direction: v3_at(b, o + 16)?,
                tangent: v3_at(b, o + 28)?,
                color: v3_at(b, o + 40)?,
                energy: f32_at(b, o + 52)?,
                range: f32_at(b, o + 56)?,
                radius: f32_at(b, o + 60)?,
                spot_size: f32_at(b, o + 64)?,
                spot_blend: f32_at(b, o + 68)?,
                time_of_day: u32_at(b, o + 72),
                flags: u32_at(b, o + 76),
                priority: f32_at(b, o + 80)?,
                group: u32_at(b, o + 84),
                source: u32_at(b, o + 88),
            })
        })
        .collect()
}

pub fn write_lights(recs: &[LightRec]) -> Vec<u8> {
    let mut b = header(b"LGHT", recs.len());
    for r in recs {
        put_u32(&mut b, r.kind);
        for v in [r.position, r.direction, r.tangent, r.color] {
            put_v3(&mut b, v);
        }
        for x in [r.energy, r.range, r.radius, r.spot_size, r.spot_blend] {
            put_f32(&mut b, x);
        }
        put_u32(&mut b, r.time_of_day);
        put_u32(&mut b, r.flags);
        put_f32(&mut b, r.priority);
        put_u32(&mut b, r.group);
        put_u32(&mut b, r.source);
    }
    b
}

/// Curves with at least 2 points inside the point array and a surface 0-4.
pub fn read_curves(b: &[u8]) -> Result<CurveTable> {
    let n = read_header(b, b"CURV")?;
    if b.len() < 24 {
        return Err(TableError("too short".into()));
    }
    let p = u32_at(b, 16);
    size(b, 24 + u64::from(n) * CURVE_SIZE as u64 + u64::from(p) * 12)?;
    let mut curves = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let o = 24 + i * CURVE_SIZE;
        let c = CurveRec {
            first: u32_at(b, o),
            count: u32_at(b, o + 4),
            flags: u32_at(b, o + 8),
            radius: f32_at(b, o + 12)?,
            surface: u32_at(b, o + 16),
        };
        if c.count < 2 || u64::from(c.first) + u64::from(c.count) > u64::from(p) {
            return Err(TableError(format!(
                "curve {i} indexes points {}+{} of {p}",
                c.first, c.count
            )));
        }
        if c.surface > 4 {
            return Err(TableError(format!("curve {i} has surface {}", c.surface)));
        }
        curves.push(c);
    }
    let po = 24 + n as usize * CURVE_SIZE;
    let points = (0..p as usize)
        .map(|k| v3_at(b, po + k * 12))
        .collect::<Result<_>>()?;
    Ok(CurveTable {
        curves,
        points,
        reserved: u32_at(b, 20),
    })
}

pub fn write_curves(t: &CurveTable) -> Vec<u8> {
    let mut b = header(b"CURV", t.curves.len());
    put_u32(&mut b, u32::try_from(t.points.len()).expect("points fit u32"));
    put_u32(&mut b, t.reserved);
    for c in &t.curves {
        put_u32(&mut b, c.first);
        put_u32(&mut b, c.count);
        put_u32(&mut b, c.flags);
        put_f32(&mut b, c.radius);
        put_u32(&mut b, c.surface);
    }
    for p in &t.points {
        put_v3(&mut b, *p);
    }
    b
}

/// A soup whose triangles index its vertices and whose surface bytes (bits 0-6) index `surfaces` entries.
pub fn read_soup(b: &[u8], surfaces: usize) -> Result<SoupTable> {
    let t = read_header(b, b"SOUP")?;
    if b.len() < 24 {
        return Err(TableError("too short".into()));
    }
    let v = u32_at(b, 16);
    size(b, 24 + u64::from(v) * 12 + u64::from(t) * 13)?;
    let positions = (0..v as usize)
        .map(|k| v3_at(b, 24 + k * 12))
        .collect::<Result<Vec<_>>>()?;
    let io = 24 + v as usize * 12;
    let mut triangles = Vec::with_capacity(t as usize);
    for k in 0..t as usize {
        let tri = [0, 1, 2].map(|j| u32_at(b, io + (k * 3 + j) * 4));
        if let Some(&bad) = tri.iter().find(|&&i| i >= v) {
            return Err(TableError(format!("triangle {k} indexes vertex {bad} of {v}")));
        }
        triangles.push(tri);
    }
    let so = io + t as usize * 12;
    let surf = b[so..so + t as usize].to_vec();
    if let Some((k, s)) = surf
        .iter()
        .enumerate()
        .find(|(_, s)| usize::from(**s & 0x7F) >= surfaces)
    {
        return Err(TableError(format!(
            "triangle {k} has surface {} of {surfaces}",
            s & 0x7F
        )));
    }
    Ok(SoupTable {
        positions,
        triangles,
        surfaces: surf,
        reserved: u32_at(b, 20),
    })
}

pub fn write_soup(s: &SoupTable) -> Vec<u8> {
    assert_eq!(
        s.triangles.len(),
        s.surfaces.len(),
        "one surface byte per triangle"
    );
    let mut b = header(b"SOUP", s.triangles.len());
    b.reserve(8 + s.positions.len() * 12 + s.triangles.len() * 13);
    put_u32(
        &mut b,
        u32::try_from(s.positions.len()).expect("vertices fit u32"),
    );
    put_u32(&mut b, s.reserved);
    for p in &s.positions {
        put_v3(&mut b, *p);
    }
    for t in &s.triangles {
        for i in t {
            put_u32(&mut b, *i);
        }
    }
    b.extend_from_slice(&s.surfaces);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plac() -> PlacementRec {
        PlacementRec {
            proto: 2,
            variant: 1,
            m: [1., 0., 0., 0., 1., 0., 0., 0., 1., 12., -0.0, -40.],
            tile: [-3, 7],
            radius: 4.5,
            flags: flags::TILE | flags::RADIUS,
            source: u32::MAX,
        }
    }

    #[test]
    fn placements_round_trip_and_layout() {
        let b = write_placements(&[plac(), plac()]);
        assert_eq!(b.len(), 16 + 2 * 76);
        assert_eq!(&b[..16], b"SKST\x01\0\0\0PLAC\x02\0\0\0");
        assert_eq!(read_placements(&b).unwrap(), [plac(), plac()]);
        // byte 56: the tile, signed
        assert_eq!(&b[16 + 56..16 + 60], (-3i32).to_le_bytes());
        // -0.0 survives
        assert_eq!(
            read_placements(&b).unwrap()[0].m[10].to_bits(),
            (-0.0f32).to_bits()
        );
    }

    #[test]
    fn bad_tables_fail_with_their_reason() {
        let good = write_placements(&[plac()]);
        let mut b = good.clone();
        b[0] = b'X';
        assert_eq!(read_placements(&b).unwrap_err().0, "not an SKST table");
        let mut b = good.clone();
        b[4] = 2;
        assert_eq!(
            read_placements(&b).unwrap_err().0,
            "table version 2, this reader takes 1"
        );
        assert_eq!(
            read_lights(&good).unwrap_err().0,
            "a PLAC table where a LGHT table was expected"
        );
        let mut b = good.clone();
        b.push(0);
        assert_eq!(read_placements(&b).unwrap_err().0, "93 bytes, the header says 92");
        let mut b = good.clone();
        b[16 + 8..16 + 12].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(
            read_placements(&b).unwrap_err().0,
            "a non-finite float at byte 24"
        );
        assert_eq!(read_placements(b"SKST").unwrap_err().0, "not an SKST table");
    }

    #[test]
    fn lights_round_trip() {
        let l = LightRec {
            kind: 1,
            position: [1., 2., 3.],
            direction: [0., -1., 0.],
            tangent: [1., 0., 0.],
            color: [1., 0.5, 0.25],
            energy: 100.,
            range: 20.,
            radius: 0.1,
            spot_size: 0.8,
            spot_blend: 0.2,
            time_of_day: 127,
            flags: flags::PRIORITY,
            priority: 2000.,
            group: 0,
            source: 3,
        };
        let b = write_lights(&[l]);
        assert_eq!(b.len(), 16 + 92);
        assert_eq!(read_lights(&b).unwrap(), [l]);
    }

    #[test]
    fn curves_check_ranges() {
        let t = CurveTable {
            curves: vec![CurveRec {
                first: 0,
                count: 2,
                flags: flags::CLOSED | (1 << 4),
                radius: 0.03,
                surface: 4,
            }],
            points: vec![[0.; 3], [1., 0., 0.]],
            reserved: 0,
        };
        let b = write_curves(&t);
        assert_eq!(b.len(), 24 + 20 + 24);
        assert_eq!(read_curves(&b).unwrap(), t);
        let mut bad = t.clone();
        bad.curves[0].count = 3;
        assert_eq!(
            read_curves(&write_curves(&bad)).unwrap_err().0,
            "curve 0 indexes points 0+3 of 2"
        );
        let mut bad = t.clone();
        bad.curves[0].count = 1;
        assert!(read_curves(&write_curves(&bad)).is_err());
        let mut bad = t;
        bad.curves[0].surface = 5;
        assert_eq!(
            read_curves(&write_curves(&bad)).unwrap_err().0,
            "curve 0 has surface 5"
        );
    }

    #[test]
    fn soups_check_indices_and_surfaces() {
        let s = SoupTable {
            positions: vec![[0.; 3], [1., 0., 0.], [0., 0., 1.]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0x80 | 2],
            reserved: 0,
        };
        let b = write_soup(&s);
        assert_eq!(b.len(), 24 + 36 + 13);
        assert_eq!(read_soup(&b, 3).unwrap(), s);
        assert_eq!(read_soup(&b, 2).unwrap_err().0, "triangle 0 has surface 2 of 2");
        let mut bad = s;
        bad.triangles[0][2] = 3;
        assert_eq!(
            read_soup(&write_soup(&bad), 3).unwrap_err().0,
            "triangle 0 indexes vertex 3 of 3"
        );
    }
}
