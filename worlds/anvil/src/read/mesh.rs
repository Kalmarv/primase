//! A Mesh resource's body: its compiled mesh's vertex and index buffers, read as stored. Nothing about a vertex's
//! fields is decided here beyond their layout; [`StoredVertex::position`] is the one decode, its evidence the bounds.
//!
//! The body (little-endian; offsets from the body's start, after the resource's class id and extension):
//! - a 61-byte header, kept as stored; its bounds (two f32 triples at 20 and 36) are read;
//! - at 61, the compiled mesh: its marker 0xFC9E1595 (u32), the vertex stride (u32 at marker+22), the range record
//!   count N (u32 at marker+63), then from marker+75 N range records, each the record id 0x07B19A87 (u32), a version
//!   byte (1), three bytes, and two u32 (the terminal record's: the index count of the first range and the second
//!   range's vertex base); every record but the last carries 8 more bytes;
//! - the vertex buffer's byte length (u32), then the vertices (stride bytes each);
//! - 8 bytes, kept as stored; the index buffer's byte length (u32), then the indices (u16);
//! - a tail, kept as stored.
//!
//! Established on four Mesh resources (Riders Republic build 25394987: one, two and three range records, strides 24).
//! A header without the marker at 61, another stride, record id or version, a buffer past the body, or an index
//! buffer not of whole triangles is refused, naming it. Which buffer ranges the non-terminal records describe, their 8
//! extra bytes and the 8 bytes before the indices are not established and are kept as stored.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// Mesh's type id (CRC-32 of its type name).
pub const MESH: u32 = 0x415D_9568;
/// The compiled mesh's marker.
pub const COMPILED_MESH: u32 = 0xFC9E_1595;
/// Where the compiled mesh starts in the body.
pub const HEADER: usize = 61;
/// A range record's id.
pub const RANGE_RECORD: u32 = 0x07B1_9A87;
/// The one vertex stride read.
pub const STRIDE: u32 = 24;

/// Why a Mesh body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Mesh body: {have} bytes, shorter than its {what} at {at}")]
    Short {
        what: &'static str,
        at: usize,
        have: usize,
    },
    #[error("Mesh body: {found:#010x} at 61, not the compiled mesh marker 0xFC9E1595")]
    Marker { found: u32 },
    #[error("Mesh vertex stride {0}: only 24 is read")]
    Stride(u32),
    #[error("Mesh range record {index}: {why}")]
    Record { index: usize, why: String },
    #[error("Mesh {what}: {why}")]
    Buffer { what: &'static str, why: String },
}

/// A range record as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeRecord {
    /// The three bytes after the version byte.
    pub pad: [u8; 3],
    pub fields: [u32; 2],
    /// The 8 bytes every record but the last carries (meaning not established).
    pub extension: Option<[u32; 2]>,
}

/// One vertex of stride 24, every field as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredVertex {
    pub position: [i16; 3],
    /// The position's scale (signed).
    pub scale: i16,
    /// Bytes 8..12 (a packed direction: the normal, where its geometry agrees).
    pub direction_a: [u8; 4],
    /// Bytes 12..16 (a packed direction: the tangent, where its geometry agrees).
    pub direction_b: [u8; 4],
    /// Bytes 16..20 (observed 0xFFFF, 0xFFFF).
    pub unknown: [u16; 2],
    /// Bytes 20..24 (texture coordinates; their divisor isn't stored).
    pub uv: [u16; 2],
}

impl StoredVertex {
    /// The position: each component times the scale over 32768. The evidence is the extrema only: on the four
    /// fixtures (tests/meshes.rs) the decoded positions' least and greatest coordinates lie within one quantization
    /// step of the header's stored bounds. No single vertex between them is checked against anything.
    pub fn position(&self) -> [f32; 3] {
        let s = f32::from(self.scale) / 32768.0;
        self.position.map(|v| f32::from(v) * s)
    }
}

/// A Mesh body's compiled buffers, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMesh {
    /// The 61 header bytes.
    pub header: Vec<u8>,
    /// The header's bounds: its minimum and maximum corners.
    pub bounds: [[f32; 3]; 2],
    /// The compiled mesh's bytes from its marker to its first range record (75), as stored.
    pub compiled_head: Vec<u8>,
    pub stride: u32,
    pub records: Vec<RangeRecord>,
    pub vertices: Vec<StoredVertex>,
    /// The 8 bytes between the vertices and the index buffer's length.
    pub gap: [u8; 8],
    pub indices: Vec<u16>,
    pub tail: Vec<u8>,
}

/// Little-endian fields read in order, each bounded.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let short = Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        };
        let end = self.at.checked_add(n).ok_or(Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })?;
        let s = self.b.get(self.at..end).ok_or(short)?;
        self.at = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], Error> {
        let s = self.take(N, what)?;
        s.try_into().map_err(|_| Error::Short {
            what,
            at: self.at,
            have: self.b.len(),
        })
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array(what)?))
    }
}

fn f32s(b: &[u8; 12]) -> [f32; 3] {
    let [x0, x1, x2, x3, y0, y1, y2, y3, z0, z1, z2, z3] = *b;
    [
        f32::from_le_bytes([x0, x1, x2, x3]),
        f32::from_le_bytes([y0, y1, y2, y3]),
        f32::from_le_bytes([z0, z1, z2, z3]),
    ]
}

impl StoredMesh {
    /// A Mesh resource's body (its [`crate::read::data::Resource::body`]).
    pub fn parse(body: &[u8]) -> Result<StoredMesh, Error> {
        let mut c = Cursor { b: body, at: 0 };
        let header = c.take(HEADER, "header")?.to_vec();
        let bound = |at: usize| -> Result<[f32; 3], Error> {
            let s = header.get(at..at.saturating_add(12)).ok_or(Error::Short {
                what: "header bounds",
                at,
                have: body.len(),
            })?;
            Ok(f32s(s.try_into().map_err(|_| Error::Short {
                what: "header bounds",
                at,
                have: body.len(),
            })?))
        };
        let bounds = [bound(20)?, bound(36)?];
        let marker = HEADER;
        let compiled_head = c.take(75, "compiled mesh head")?.to_vec();
        let word = |at: usize| -> u32 {
            compiled_head
                .get(at..at.saturating_add(4))
                .and_then(|w| w.try_into().ok())
                .map_or(0, u32::from_le_bytes)
        };
        let found = word(0);
        if found != COMPILED_MESH {
            return Err(Error::Marker { found });
        }
        let stride = word(22);
        if stride != STRIDE {
            return Err(Error::Stride(stride));
        }
        let count = word(63);
        let n = usize::try_from(count).map_err(|_| Error::Record {
            index: 0,
            why: format!("{count} records past this machine"),
        })?;
        if n == 0 || n > body.len() / 16 {
            return Err(Error::Record {
                index: 0,
                why: format!("count {count} at marker+63: not a record count the body can hold"),
            });
        }
        debug_assert_eq!(c.at, marker.saturating_add(75));
        let mut records = Vec::with_capacity(n);
        for index in 0..n {
            let id = c.u32("range record id")?;
            if id != RANGE_RECORD {
                return Err(Error::Record {
                    index,
                    why: format!("id {id:#010x}, not 0x07B19A87"),
                });
            }
            let [version, p0, p1, p2] = c.array::<4>("range record version")?;
            if version != 1 {
                return Err(Error::Record {
                    index,
                    why: format!("version {version}, not 1"),
                });
            }
            let fields = [c.u32("range record field")?, c.u32("range record field")?];
            let last = index.checked_add(1) == Some(n);
            let extension = if last {
                None
            } else {
                Some([c.u32("range record extension")?, c.u32("range record extension")?])
            };
            records.push(RangeRecord {
                pad: [p0, p1, p2],
                fields,
                extension,
            });
        }
        let vertex_bytes = usize::try_from(c.u32("vertex buffer length")?).map_err(|_| Error::Buffer {
            what: "vertex buffer",
            why: "length past this machine".into(),
        })?;
        let stride_len = usize::try_from(stride).map_err(|_| Error::Stride(stride))?;
        if vertex_bytes.checked_rem(stride_len) != Some(0) {
            return Err(Error::Buffer {
                what: "vertex buffer",
                why: format!("{vertex_bytes} bytes, not whole {stride}-byte vertices"),
            });
        }
        let raw = c.take(vertex_bytes, "vertex buffer")?;
        let vertices = raw
            .as_chunks::<24>()
            .0
            .iter()
            .map(|v| {
                let [
                    p0,
                    p1,
                    p2,
                    p3,
                    p4,
                    p5,
                    s0,
                    s1,
                    a0,
                    a1,
                    a2,
                    a3,
                    b0,
                    b1,
                    b2,
                    b3,
                    u0,
                    u1,
                    u2,
                    u3,
                    t0,
                    t1,
                    t2,
                    t3,
                ] = *v;
                StoredVertex {
                    position: [
                        i16::from_le_bytes([p0, p1]),
                        i16::from_le_bytes([p2, p3]),
                        i16::from_le_bytes([p4, p5]),
                    ],
                    scale: i16::from_le_bytes([s0, s1]),
                    direction_a: [a0, a1, a2, a3],
                    direction_b: [b0, b1, b2, b3],
                    unknown: [u16::from_le_bytes([u0, u1]), u16::from_le_bytes([u2, u3])],
                    uv: [u16::from_le_bytes([t0, t1]), u16::from_le_bytes([t2, t3])],
                }
            })
            .collect();
        let gap = c.array::<8>("bytes before the index buffer")?;
        let index_bytes = usize::try_from(c.u32("index buffer length")?).map_err(|_| Error::Buffer {
            what: "index buffer",
            why: "length past this machine".into(),
        })?;
        if index_bytes.checked_rem(6) != Some(0) {
            return Err(Error::Buffer {
                what: "index buffer",
                why: format!("{index_bytes} bytes, not whole triangles of u16"),
            });
        }
        let indices = c
            .take(index_bytes, "index buffer")?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|w| u16::from_le_bytes(*w))
            .collect();
        let tail = body.get(c.at..).map_or_else(Vec::new, <[u8]>::to_vec);
        Ok(StoredMesh {
            header,
            bounds,
            compiled_head,
            stride,
            records,
            vertices,
            gap,
            indices,
            tail,
        })
    }

    /// The terminal range record's fields: the first range's index count and the second range's vertex base
    /// (`(0, 0)`: one range).
    pub fn split(&self) -> [u32; 2] {
        self.records.last().map_or([0, 0], |r| r.fields)
    }

    /// The indices as vertex indices: the first range's as stored, the rest rebased by the second range's vertex base
    /// (the terminal record's split); refused where one falls past the vertices or the split past the indices.
    pub fn triangles(&self) -> Result<Vec<[u32; 3]>, Error> {
        let [first, base] = self.split();
        let first = usize::try_from(first).map_err(|_| Error::Buffer {
            what: "split",
            why: "first range past this machine".into(),
        })?;
        if first > self.indices.len() {
            return Err(Error::Buffer {
                what: "split",
                why: format!("first range of {first} indices, {} stored", self.indices.len()),
            });
        }
        let n = self.vertices.len();
        let mut out = Vec::with_capacity(self.indices.len());
        for (k, &i) in self.indices.iter().enumerate() {
            let v = if k < first {
                u32::from(i)
            } else {
                u32::from(i).checked_add(base).ok_or(Error::Buffer {
                    what: "split",
                    why: "rebased index overflows".into(),
                })?
            };
            if usize::try_from(v).map_or(true, |v| v >= n) {
                return Err(Error::Buffer {
                    what: "index buffer",
                    why: format!("index {k} is vertex {v}, of {n}"),
                });
            }
            out.push(v);
        }
        Ok(out.as_chunks::<3>().0.to_vec())
    }
}
