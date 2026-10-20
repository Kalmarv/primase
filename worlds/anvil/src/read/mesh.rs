//! A Mesh resource's body: its compiled mesh's vertex and index buffers, read as stored. Nothing about a vertex's
//! fields is decided here beyond their layout; [`StoredVertex::position`] is the one decode, its evidence the bounds.
//!
//! The body (little-endian; offsets from the body's start, after the resource's class id and extension):
//! - a 61-byte header, kept as stored; its bounds (two f32 triples at 20 and 36) are read;
//! - at 61, the compiled mesh: its marker 0xFC9E1595 (u32), a zero byte at marker+4, its layout 0xC351EE43 (u32 at
//!   marker+13; the offsets after it are this layout's), the vertex stride (u32 at marker+22), the range record
//!   count N (u32 at marker+63), then from marker+75 N range records, each the record id 0x07B19A87 (u32), a version
//!   byte (1), three bytes, and two u32: the range's first index and its vertex base; every record but the last
//!   carries 8 more bytes;
//! - the vertex buffer's byte length (u32), then the vertices (stride bytes each);
//! - 8 bytes, kept as stored; the index buffer's byte length (u32), then the indices (u16);
//! - a tail, kept as stored.
//!
//! The indices are in ranges, one per record: a range runs from its first index to the next record's (the last to the
//! buffer's end), and its stored indices count from its vertex base; its vertices run from its base to the next
//! record's (the last to the buffer's end). Refused, naming it: a header without the marker at 61, another layout or
//! stride,
//! record id or version, a buffer past the body, an index buffer not of whole triangles, and ranges that don't
//! partition the buffers in order (the first at index and vertex 0, each range at a whole triangle and at or after the
//! one before, none past either buffer) or an index outside its range's vertices. An empty range (no indices, or no
//! vertices) and vertices no triangle names are read, not refused: nothing establishes that they can't be stored.
//! Observed, not required: on seven Mesh resources (Riders Republic build 25394987: one, two, three, five and six
//! records, strides 24) no range is empty and every vertex is named. The records' 8 extra bytes and the 8 bytes
//! before the indices are not established and are kept as stored.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::ops::Range;

/// Mesh's type id (CRC-32 of its type name).
pub const MESH: u32 = 0x415D_9568;
/// The compiled mesh's marker (CRC-32 of `CompiledMesh`).
pub const COMPILED_MESH: u32 = 0xFC9E_1595;
/// The one compiled mesh layout read: the u32 at marker+13 (its name unknown), with the byte at marker+4 0. Another
/// layout places its fields elsewhere: one with `MeshData`'s CRC-32 (0x0645ABB5) there and 3 at marker+4 is seen.
pub const LAYOUT: u32 = 0xC351_EE43;
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
    #[error(
        "Mesh compiled layout {id:#010x} at marker+13, byte {flags} at marker+4: only 0xC351EE43 and 0 are read"
    )]
    Layout { id: u32, flags: u8 },
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
    /// Where the range starts in the index buffer.
    pub first_index: u32,
    /// What the range's stored indices count from in the vertex buffer.
    pub vertex_base: u32,
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

/// One range of a Mesh's buffers: its stored indices count from `vertices.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshRange {
    pub indices: Range<usize>,
    pub vertices: Range<usize>,
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
        let (id, flags) = (word(13), compiled_head.get(4).copied().unwrap_or_default());
        if (id, flags) != (LAYOUT, 0) {
            return Err(Error::Layout { id, flags });
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
            let first_index = c.u32("range record first index")?;
            let vertex_base = c.u32("range record vertex base")?;
            let last = index.checked_add(1) == Some(n);
            let extension = if last {
                None
            } else {
                Some([c.u32("range record extension")?, c.u32("range record extension")?])
            };
            records.push(RangeRecord {
                pad: [p0, p1, p2],
                first_index,
                vertex_base,
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

    /// Each range's indices and vertices, refused unless they tile both
    /// buffers (the first at 0, each at a whole triangle and at or after the one before, the last ending each buffer).
    pub fn ranges(&self) -> Result<Vec<MeshRange>, Error> {
        let at = |x: u32| usize::try_from(x).unwrap_or(usize::MAX);
        let refuse = |index: usize, why: String| Error::Record { index, why };
        let mut out = Vec::with_capacity(self.records.len());
        for (index, r) in self.records.iter().enumerate() {
            let (first, base) = (at(r.first_index), at(r.vertex_base));
            let (end, vertices_end) = self
                .records
                .get(index.saturating_add(1))
                .map_or((self.indices.len(), self.vertices.len()), |next| {
                    (at(next.first_index), at(next.vertex_base))
                });
            if index == 0 && (first, base) != (0, 0) {
                return Err(refuse(
                    index,
                    format!("the first range starts at index {first}, vertex {base}, not 0, 0"),
                ));
            }
            if first.checked_rem(3) != Some(0) || first > end || end > self.indices.len() {
                return Err(refuse(
                    index,
                    format!(
                        "indices {first}..{end} of {}: not whole triangles in order",
                        self.indices.len()
                    ),
                ));
            }
            if base > vertices_end || vertices_end > self.vertices.len() {
                return Err(refuse(
                    index,
                    format!(
                        "vertices {base}..{vertices_end} of {}: not in order",
                        self.vertices.len()
                    ),
                ));
            }
            out.push(MeshRange {
                indices: first..end,
                vertices: base..vertices_end,
            });
        }
        Ok(out)
    }

    /// The indices as vertex indices: each range's counted from its vertex base; refused where one falls outside its
    /// range's vertices.
    pub fn triangles(&self) -> Result<Vec<[u32; 3]>, Error> {
        let mut out = Vec::with_capacity(self.indices.len());
        for (index, MeshRange { indices, vertices }) in self.ranges()?.into_iter().enumerate() {
            let stored = self.indices.get(indices.clone()).unwrap_or_default();
            for (k, &i) in stored.iter().enumerate() {
                let v = vertices.start.saturating_add(usize::from(i));
                if !vertices.contains(&v) {
                    return Err(Error::Record {
                        index,
                        why: format!(
                            "index {} is vertex {v}, outside the range's {vertices:?}",
                            indices.start.saturating_add(k)
                        ),
                    });
                }
                out.push(u32::try_from(v).map_err(|_| Error::Buffer {
                    what: "index buffer",
                    why: format!("vertex {v} past u32"),
                })?);
            }
        }
        Ok(out.as_chunks::<3>().0.to_vec())
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    /// A mesh of `vertices` vertices and these indices, its records these (first index, vertex base).
    fn mesh(vertices: usize, indices: &[u16], records: &[(u32, u32)]) -> StoredMesh {
        let vertex = StoredVertex {
            position: [0; 3],
            scale: 1,
            direction_a: [0; 4],
            direction_b: [0; 4],
            unknown: [0xFFFF; 2],
            uv: [0; 2],
        };
        StoredMesh {
            header: vec![0; HEADER],
            bounds: [[0.0; 3]; 2],
            compiled_head: vec![0; 75],
            stride: STRIDE,
            records: records
                .iter()
                .enumerate()
                .map(|(k, &(first_index, vertex_base))| RangeRecord {
                    pad: [0; 3],
                    first_index,
                    vertex_base,
                    extension: (k + 1 < records.len()).then_some([0, 0]),
                })
                .collect(),
            vertices: vec![vertex; vertices],
            gap: [0; 8],
            indices: indices.to_vec(),
            tail: Vec::new(),
        }
    }

    fn refused(m: &StoredMesh) -> usize {
        match m.triangles() {
            Err(Error::Record { index, .. }) => index,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn each_range_counts_from_its_base() {
        let m = mesh(6, &[0, 1, 2, 0, 2, 1, 1, 0, 2], &[(0, 0), (3, 3)]);
        assert_eq!(m.triangles().expect("read"), [[0, 1, 2], [3, 5, 4], [4, 3, 5]]);
    }

    #[test]
    fn a_first_range_not_at_zero_is_refused() {
        assert_eq!(refused(&mesh(3, &[0, 1, 2, 0, 1, 2], &[(3, 0)])), 0);
        assert_eq!(refused(&mesh(6, &[0, 1, 2], &[(0, 3)])), 0);
    }

    #[test]
    fn a_range_not_at_a_whole_triangle_is_refused() {
        assert_eq!(refused(&mesh(6, &[0, 1, 2, 0, 1, 2], &[(0, 0), (4, 3)])), 1);
    }

    #[test]
    fn ranges_out_of_order_or_past_the_buffers_are_refused() {
        // indices descending, vertices descending, indices past the buffer, vertices past the buffer
        let i = [0, 1, 2, 0, 1, 2, 0, 1, 2];
        assert_eq!(refused(&mesh(9, &i, &[(0, 0), (6, 3), (3, 6)])), 1);
        assert_eq!(refused(&mesh(9, &i, &[(0, 0), (3, 6), (6, 3)])), 1);
        assert_eq!(refused(&mesh(9, &i, &[(0, 0), (12, 3)])), 0);
        assert_eq!(refused(&mesh(9, &i, &[(0, 0), (3, 12)])), 0);
    }

    #[test]
    fn an_index_outside_its_ranges_vertices_is_refused() {
        // range 0 owns vertices 0..3; its index 3 would be range 1's
        assert_eq!(refused(&mesh(6, &[0, 1, 3, 0, 1, 2], &[(0, 0), (3, 3)])), 0);
        // the last range owns 3..6; 3 + 3 is past it
        assert_eq!(refused(&mesh(6, &[0, 1, 2, 0, 1, 3], &[(0, 0), (3, 3)])), 1);
    }

    #[test]
    fn empty_ranges_and_unnamed_vertices_are_read() {
        // an empty middle range (no indices, vertices 3..4), and vertex 4 named by no triangle
        let m = mesh(7, &[0, 1, 2, 0, 1, 2], &[(0, 0), (3, 3), (3, 4)]);
        assert_eq!(m.triangles().expect("read"), [[0, 1, 2], [4, 5, 6]]);
        let ranges = m.ranges().expect("ranges");
        assert_eq!(
            ranges[1],
            MeshRange {
                indices: 3..3,
                vertices: 3..4
            }
        );
        // a range with indices but no vertices of its own names none, so any index is outside it
        assert_eq!(refused(&mesh(3, &[0, 1, 2, 0, 1, 2], &[(0, 0), (3, 3)])), 1);
    }
}
