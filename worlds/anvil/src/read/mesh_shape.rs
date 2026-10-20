//! A MeshShape resource's body as a run of sections, each kept as stored; nothing in it is interpreted (no geometry).
//!
//! The body (little-endian; offsets from the body's start, after the resource's class id and extension):
//! - a prefix: a u32 (observed 1), a u8 (observed 0), the section run's length (u32);
//! - the section run, from byte 9 for that length: consecutive sections, each a type (u16), the marker 0xECD7 (u16),
//!   its size (i32, its 8 header bytes included), then its bytes; the sections close the run exactly;
//! - a tail: whatever follows the run, kept as stored (not sections).
//!
//! Established on two MeshShape resources of different sizes (Riders Republic build 25394987: ten sections each, in
//! the same type order).
//!
//! Its triangle geometry ([`MeshShape::geometry`]) is five of those sections, read as stored: 0x1EEE the counts (V
//! positions, F faces, X edges, u32 each), 0x1EEF V positions (three f32), 0x1EF0 F faces (three u16 position
//! indices), 0x1EF2 F faces' edges (three u32 edge ids), 0x1EF3 X edges' faces (two u16 face ids, 0xFFFF where an
//! edge has one face). Not established, so not read: units and frame, surfaces or materials, the other sections and
//! the tail, and which two positions a boundary edge joins (not every boundary edge's can be told from its face). A prefix with other values, a section without the marker, a size under its header or past
//! the run, or a run past the body is refused, naming it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// MeshShape's type id (CRC-32 of its type name).
pub const MESH_SHAPE: u32 = 0xB22B_3E61;
/// The marker in every section header.
pub const SECTION_MARKER: u16 = 0xECD7;
/// The prefix's first u32, the one value read.
pub const VERSION: u32 = 1;
/// The prefix's byte, the one value read.
pub const FLAG: u8 = 0;
/// The prefix's length: where the section run starts.
pub const PREFIX: usize = 9;
/// A section header's length (type, marker, size).
pub const SECTION_HEADER: usize = 8;

/// Why a MeshShape body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MeshShape body: {have} bytes, shorter than its {need}-byte {what}")]
    Short {
        what: &'static str,
        have: usize,
        need: usize,
    },
    #[error("MeshShape prefix: version {0}: only version 1 is read")]
    Version(u32),
    #[error("MeshShape prefix: byte {0}: only 0 is read (another value isn't established)")]
    Flag(u8),
    #[error("MeshShape section run: {length} bytes from {PREFIX} past the body's {body} bytes")]
    Run { length: u32, body: usize },
    #[error("MeshShape section {index} at {at}: marker {marker:#06x}, not 0xECD7")]
    Marker { index: usize, at: usize, marker: u16 },
    #[error("MeshShape section {index} at {at}: size {size}: {why}")]
    Size {
        index: usize,
        at: usize,
        size: i32,
        why: &'static str,
    },
    #[error("MeshShape geometry: section {kind:#06x}: {why}")]
    Geometry { kind: u16, why: String },
}

/// The counts section.
pub const COUNTS: u16 = 0x1EEE;
/// The positions section.
pub const POSITIONS: u16 = 0x1EEF;
/// The faces' position indices section.
pub const FACES: u16 = 0x1EF0;
/// The faces' edge ids section.
pub const FACE_EDGES: u16 = 0x1EF2;
/// The edges' face ids section.
pub const EDGE_FACES: u16 = 0x1EF3;
/// An edge's missing second face.
pub const NO_FACE: u16 = 0xFFFF;

/// A MeshShape's triangle geometry as stored (in its own units and frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    pub positions: Vec<[f32; 3]>,
    /// Each face's three position indices.
    pub faces: Vec<[u16; 3]>,
    /// Each face's three edge ids.
    pub face_edges: Vec<[u32; 3]>,
    /// Each edge's two face ids ([`NO_FACE`] for a boundary edge's missing one).
    pub edge_faces: Vec<[u16; 2]>,
}

impl Geometry {
    /// The edges with one face.
    pub fn boundary_edges(&self) -> usize {
        self.edge_faces.iter().filter(|f| f.contains(&NO_FACE)).count()
    }
}

/// One section as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Where its header starts in the body.
    pub at: usize,
    pub kind: u16,
    /// Its size as stored (header included).
    pub size: u32,
    /// Its bytes after the header.
    pub bytes: Vec<u8>,
}

/// A MeshShape body: its prefix, its sections in order and its tail, all as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshShape {
    pub version: u32,
    pub flag: u8,
    /// The section run's length as stored.
    pub run_length: u32,
    pub sections: Vec<Section>,
    /// The bytes after the run, as stored.
    pub tail: Vec<u8>,
}

fn field<const N: usize>(body: &[u8], at: usize, what: &'static str) -> Result<[u8; N], Error> {
    let short = || Error::Short {
        what,
        have: body.len(),
        need: at.saturating_add(N),
    };
    body.get(at..at.checked_add(N).ok_or_else(short)?)
        .ok_or_else(short)?
        .try_into()
        .map_err(|_| short())
}

impl MeshShape {
    /// A MeshShape resource's body (its [`crate::read::data::Resource::body`]).
    pub fn parse(body: &[u8]) -> Result<MeshShape, Error> {
        let version = u32::from_le_bytes(field(body, 0, "prefix")?);
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let [flag] = field::<1>(body, 4, "prefix")?;
        if flag != FLAG {
            return Err(Error::Flag(flag));
        }
        let run_length = u32::from_le_bytes(field(body, 5, "prefix")?);
        let past = || Error::Run {
            length: run_length,
            body: body.len(),
        };
        let run_end = usize::try_from(run_length)
            .ok()
            .and_then(|n| PREFIX.checked_add(n))
            .filter(|&end| end <= body.len())
            .ok_or_else(past)?;
        let mut sections = Vec::new();
        let mut at = PREFIX;
        while at < run_end {
            let index = sections.len();
            let header: [u8; SECTION_HEADER] = field(body, at, "section header")?;
            let [k0, k1, m0, m1, s0, s1, s2, s3] = header;
            let kind = u16::from_le_bytes([k0, k1]);
            let marker = u16::from_le_bytes([m0, m1]);
            if marker != SECTION_MARKER {
                return Err(Error::Marker { index, at, marker });
            }
            let size = i32::from_le_bytes([s0, s1, s2, s3]);
            let bad = |why| Error::Size { index, at, size, why };
            let len = usize::try_from(size).map_err(|_| bad("negative"))?;
            if len < SECTION_HEADER {
                return Err(bad("under its 8-byte header"));
            }
            let end = at.checked_add(len).ok_or(bad("past the run"))?;
            if end > run_end {
                return Err(bad("past the run"));
            }
            let bytes = body
                .get(at.checked_add(SECTION_HEADER).ok_or(bad("past the run"))?..end)
                .ok_or(bad("past the run"))?
                .to_vec();
            sections.push(Section {
                at,
                kind,
                size: u32::try_from(size).map_err(|_| bad("negative"))?,
                bytes,
            });
            at = end;
        }
        let tail = body.get(run_end..).ok_or_else(past)?.to_vec();
        Ok(MeshShape {
            version,
            flag,
            run_length,
            sections,
            tail,
        })
    }

    /// The section of type `kind`: exactly one.
    fn section(&self, kind: u16) -> Result<&[u8], Error> {
        let bad = |why: &str| Error::Geometry {
            kind,
            why: why.to_owned(),
        };
        let mut found = self.sections.iter().filter(|s| s.kind == kind);
        let one = found.next().ok_or(bad("missing"))?;
        if found.next().is_some() {
            return Err(bad("more than one"));
        }
        Ok(&one.bytes)
    }

    /// Its triangle geometry as stored: each section's length exact for its count, every index in range, and each
    /// edge's face ids the faces that name it (two for an edge between faces, one and [`NO_FACE`] for a boundary
    /// edge). Nothing is converted: positions are as stored, in the shape's own units and frame.
    pub fn geometry(&self) -> Result<Geometry, Error> {
        let bad = |kind: u16, why: String| Error::Geometry { kind, why };
        let counts = self.section(COUNTS)?;
        if counts.len() != 12 {
            return Err(bad(COUNTS, format!("{} bytes, not 12", counts.len())));
        }
        let word = |b: &[u8], at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(
                b.get(at..at.checked_add(4)?)?.try_into().ok()?,
            ))
        };
        let count = |at| {
            word(counts, at)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(bad(COUNTS, "a count past this machine".into()))
        };
        let (v, f, x) = (count(0)?, count(4)?, count(8)?);
        // a section holds `n` records of `size` bytes, exactly
        let records = |kind: u16, n: usize, size: usize| -> Result<&[u8], Error> {
            let b = self.section(kind)?;
            let want = n
                .checked_mul(size)
                .ok_or(bad(kind, "its count overflows".into()))?;
            if b.len() != want {
                return Err(bad(kind, format!("{} bytes, not {n} x {size} = {want}", b.len())));
            }
            Ok(b)
        };
        let le16 = |b: &[u8; 2]| u16::from_le_bytes(*b);
        let le32 = |b: &[u8; 4]| u32::from_le_bytes(*b);
        let positions: Vec<[f32; 3]> = records(POSITIONS, v, 12)?
            .as_chunks::<4>()
            .0
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f32::from_le_bytes))
            .collect();
        let mut faces = Vec::with_capacity(f);
        for (k, c) in records(FACES, f, 6)?
            .as_chunks::<2>()
            .0
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let face = c.map(|w| le16(&w));
            if let Some(p) = face.iter().find(|&&p| usize::from(p) >= v) {
                return Err(bad(FACES, format!("face {k}: position {p} of {v}")));
            }
            faces.push(face);
        }
        let mut face_edges = Vec::with_capacity(f);
        for (k, c) in records(FACE_EDGES, f, 12)?
            .as_chunks::<4>()
            .0
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let e = c.map(|w| le32(&w));
            if let Some(id) = e
                .iter()
                .find(|&&id| usize::try_from(id).map_or(true, |id| id >= x))
            {
                return Err(bad(FACE_EDGES, format!("face {k}: edge {id} of {x}")));
            }
            face_edges.push(e);
        }
        let mut edge_faces = Vec::with_capacity(x);
        for (k, c) in records(EDGE_FACES, x, 4)?
            .as_chunks::<2>()
            .0
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
        {
            let pair = c.map(|w| le16(&w));
            if let Some(id) = pair.iter().find(|&&id| id != NO_FACE && usize::from(id) >= f) {
                return Err(bad(EDGE_FACES, format!("edge {k}: face {id} of {f}")));
            }
            edge_faces.push(pair);
        }
        // each edge's face ids are the faces naming it
        let mut named: Vec<Vec<u16>> = vec![Vec::new(); x];
        for (k, e) in face_edges.iter().enumerate() {
            let face =
                u16::try_from(k).map_err(|_| bad(FACE_EDGES, format!("face {k} past a u16 face id")))?;
            for &id in e {
                if let Some(list) = usize::try_from(id).ok().and_then(|id| named.get_mut(id)) {
                    list.push(face);
                }
            }
        }
        for (k, (stored, named)) in edge_faces.iter().zip(&named).enumerate() {
            let mut stored: Vec<u16> = stored.iter().copied().filter(|&id| id != NO_FACE).collect();
            let mut named = named.clone();
            stored.sort_unstable();
            named.sort_unstable();
            if stored != named {
                return Err(bad(
                    EDGE_FACES,
                    format!("edge {k}: stored faces {stored:?}, named by faces {named:?}"),
                ));
            }
        }
        Ok(Geometry {
            positions,
            faces,
            face_edges,
            edge_faces,
        })
    }
}
