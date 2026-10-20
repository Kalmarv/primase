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
//! - the tail ([`StoredTail`]): a u32 byte length (32 per range) and that many bytes; 8 bytes; `MeshData` (its
//!   CRC-32 0x0645ABB5 and 18 bytes); one `MeshPrimitive` entry per range (its CRC-32 0xA57387EF, vertex base, a u32,
//!   vertex count, first index, triangle count, version 1, 8 bytes) and 20 bytes holding the range count at +8 after
//!   the last; one material descriptor per range (0x0B1D34C1, 4 bytes, a zero byte, the range's vertex count as u16, a
//!   zero byte, the u64 ClassID of a Material, 8 bytes); the range count (u32) and 6 bytes; one 31-byte group per
//!   range, holding its descriptor's Material ClassID at +8 and +17; 20 bytes, ending the body.
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
//!
//! The tail's grammar is established on the same meshes (games/riders-republic docs/recon/material-chain-binding.md,
//! 2b3215e: every byte to the body's end accounted for, and the Material ClassIDs resolving to Material resources).
//! Refused: another block length, marker, version or range count, a byte other than 0 either side of a descriptor's
//! vertex count, a group's ClassID not its descriptor's, or bytes left after the last 20. [`StoredMesh::materials`]
//! refuses a `MeshPrimitive` entry or descriptor that disagrees with its range. That descriptor *k* is range *k*'s
//! material is read from their order and the vertex counts agreeing; how the engine binds it is not established. The
//! bytes kept as stored are not interpreted.
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
/// `MeshData`'s marker in the tail (its CRC-32).
pub const MESH_DATA: u32 = 0x0645_ABB5;
/// A `MeshPrimitive` entry's marker in the tail (its CRC-32).
pub const MESH_PRIMITIVE: u32 = 0xA573_87EF;
/// A material descriptor's marker in the tail (its name unknown).
pub const MATERIAL_DESCRIPTOR: u32 = 0x0B1D_34C1;
/// The type of the resources material descriptors refer to (CRC-32 of `Material`).
pub const MATERIAL: u32 = 0x85C8_17C3;

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
    #[error("Mesh tail {what}: {why}")]
    Tail { what: &'static str, why: String },
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
    pub tail: StoredTail,
}

/// One `MeshPrimitive` entry of the tail (36 bytes after its marker's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPrimitive {
    pub vertex_base: u32,
    /// The u32 after the vertex base (0 on every entry seen).
    pub reserved: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub triangle_count: u32,
    pub rest: [u8; 8],
}

/// One material descriptor of the tail (28 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialDescriptor {
    /// The 4 bytes after its marker.
    pub head: [u8; 4],
    /// Its range's vertex count, as a u16 between two zero bytes (a width beyond 16 bits isn't established).
    pub vertex_count: u16,
    /// The ClassID of a Material resource.
    pub material: u64,
    /// The 8 bytes after the ClassID.
    pub rest: [u8; 8],
}

/// A Mesh body's tail after the index buffer, as stored (its grammar in the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTail {
    /// The 32-byte blocks, one per range.
    pub blocks: Vec<[u8; 32]>,
    /// The 8 bytes after them.
    pub gap: [u8; 8],
    /// `MeshData`'s 18 bytes after its marker.
    pub mesh_data: [u8; 18],
    pub primitives: Vec<StoredPrimitive>,
    /// The 20 bytes after the last entry (the range count at +8).
    pub primitives_end: [u8; 20],
    pub descriptors: Vec<MaterialDescriptor>,
    /// The 6 bytes after the second range count.
    pub groups_head: [u8; 6],
    /// The 31-byte groups, one per range.
    pub groups: Vec<[u8; 31]>,
    /// The body's last 20 bytes.
    pub trailer: [u8; 20],
}

impl StoredTail {
    /// The tail of `body` from `at`, for `n` ranges; it must end the body.
    pub fn parse(body: &[u8], at: usize, n: usize) -> Result<StoredTail, Error> {
        let refuse = |what: &'static str, why: String| Error::Tail { what, why };
        let mut c = Cursor { b: body, at };
        let marker = |c: &mut Cursor<'_>, what: &'static str, want: u32| -> Result<(), Error> {
            let found = c.u32(what)?;
            if found == want {
                Ok(())
            } else {
                Err(refuse(what, format!("{found:#010x}, not {want:#010x}")))
            }
        };
        let block_bytes = c.u32("block length")?;
        if usize::try_from(block_bytes).ok() != n.checked_mul(32) {
            return Err(refuse(
                "block length",
                format!("{block_bytes}, not 32 for each of {n} ranges"),
            ));
        }
        let blocks = (0..n).map(|_| c.array::<32>("block")).collect::<Result<_, _>>()?;
        let gap = c.array::<8>("bytes before MeshData")?;
        marker(&mut c, "MeshData marker", MESH_DATA)?;
        let mesh_data = c.array::<18>("MeshData")?;
        let mut primitives = Vec::with_capacity(n);
        for _ in 0..n {
            marker(&mut c, "MeshPrimitive marker", MESH_PRIMITIVE)?;
            let vertex_base = c.u32("MeshPrimitive vertex base")?;
            let reserved = c.u32("MeshPrimitive")?;
            let vertex_count = c.u32("MeshPrimitive vertex count")?;
            let first_index = c.u32("MeshPrimitive first index")?;
            let triangle_count = c.u32("MeshPrimitive triangle count")?;
            let version = c.u32("MeshPrimitive version")?;
            if version != 1 {
                return Err(refuse("MeshPrimitive version", format!("{version}, not 1")));
            }
            primitives.push(StoredPrimitive {
                vertex_base,
                reserved,
                vertex_count,
                first_index,
                triangle_count,
                rest: c.array::<8>("MeshPrimitive")?,
            });
        }
        let primitives_end = c.array::<20>("bytes after the MeshPrimitive entries")?;
        let [m0, m1, m2, m3, _, _, _, _, n0, n1, n2, n3, ..] = primitives_end;
        let next = u32::from_le_bytes([m0, m1, m2, m3]);
        let ranges = u32::from_le_bytes([n0, n1, n2, n3]);
        if next != 0 || usize::try_from(ranges).ok() != Some(n) {
            return Err(refuse(
                "bytes after the MeshPrimitive entries",
                format!(
                    "{next:#010x} where another entry's marker would be and {ranges} at +8, for {n} ranges"
                ),
            ));
        }
        let mut descriptors = Vec::with_capacity(n);
        for _ in 0..n {
            marker(&mut c, "material descriptor marker", MATERIAL_DESCRIPTOR)?;
            let head = c.array::<4>("material descriptor")?;
            let [z0, v0, v1, z1] = c.array::<4>("material descriptor vertex count")?;
            if [z0, z1] != [0, 0] {
                return Err(refuse(
                    "material descriptor vertex count",
                    format!("bytes {z0} and {z1} either side of it: only 0 is read"),
                ));
            }
            let material = u64::from_le_bytes(c.array::<8>("material descriptor ClassID")?);
            descriptors.push(MaterialDescriptor {
                head,
                vertex_count: u16::from_le_bytes([v0, v1]),
                material,
                rest: c.array::<8>("material descriptor")?,
            });
        }
        let after = c.u32("range count after the descriptors")?;
        if usize::try_from(after).ok() != Some(n) {
            return Err(refuse(
                "range count after the descriptors",
                format!("{after}, not the {n} ranges"),
            ));
        }
        let groups_head = c.array::<6>("bytes before the groups")?;
        let mut groups = Vec::with_capacity(n);
        for d in &descriptors {
            let g = c.array::<31>("group")?;
            let id = |at: usize| {
                g.get(at..at.saturating_add(8))
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_le_bytes)
            };
            if id(8) != Some(d.material) || id(17) != Some(d.material) {
                return Err(refuse(
                    "group",
                    format!(
                        "ClassIDs {:?} and {:?}, not its descriptor's {}",
                        id(8),
                        id(17),
                        d.material
                    ),
                ));
            }
            groups.push(g);
        }
        let trailer = c.array::<20>("last bytes")?;
        if c.at != body.len() {
            return Err(refuse(
                "end",
                format!("{} bytes after its last 20", body.len().saturating_sub(c.at)),
            ));
        }
        Ok(StoredTail {
            blocks,
            gap,
            mesh_data,
            primitives,
            primitives_end,
            descriptors,
            groups_head,
            groups,
            trailer,
        })
    }
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
        let tail = StoredTail::parse(body, c.at, records.len())?;
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

    /// Each range's material: its descriptor's Material ClassID, refused unless the tail's `MeshPrimitive` entry and
    /// descriptor for each range agree with it (vertex base and count, first index, triangle count).
    pub fn materials(&self) -> Result<Vec<u64>, Error> {
        let ranges = self.ranges()?;
        let mut out = Vec::with_capacity(ranges.len());
        for (k, r) in ranges.iter().enumerate() {
            let (Some(p), Some(d)) = (self.tail.primitives.get(k), self.tail.descriptors.get(k)) else {
                return Err(Error::Tail {
                    what: "ranges",
                    why: format!("no entry for range {k}"),
                });
            };
            let stored = [
                p.vertex_base,
                p.vertex_count,
                p.first_index,
                p.triangle_count,
                u32::from(d.vertex_count),
            ]
            .map(|x| usize::try_from(x).unwrap_or(usize::MAX));
            let vertices = r.vertices.len();
            let range = [
                r.vertices.start,
                vertices,
                r.indices.start,
                r.indices.len() / 3,
                vertices,
            ];
            if stored != range {
                return Err(Error::Tail {
                    what: "ranges",
                    why: format!(
                        "range {k}: stored (vertex base, vertex count, first index, triangle count, descriptor vertex                          count) {stored:?}, the range's {range:?}"
                    ),
                });
            }
            out.push(d.material);
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
            tail: StoredTail {
                blocks: Vec::new(),
                gap: [0; 8],
                mesh_data: [0; 18],
                primitives: Vec::new(),
                primitives_end: [0; 20],
                descriptors: Vec::new(),
                groups_head: [0; 6],
                groups: Vec::new(),
                trailer: [0; 20],
            },
        }
    }

    /// A tail's bytes for these ranges (vertex base, vertex count, first index, triangle count) and their materials.
    fn tail(ranges: &[[u32; 4]], materials: &[u64]) -> Vec<u8> {
        let n = u32::try_from(ranges.len()).expect("few");
        let mut b = Vec::new();
        b.extend((32 * n).to_le_bytes());
        b.extend(vec![7; 32 * ranges.len()]);
        b.extend([0; 8]);
        b.extend(MESH_DATA.to_le_bytes());
        b.extend([0; 18]);
        for r in ranges {
            b.extend(MESH_PRIMITIVE.to_le_bytes());
            for x in [r[0], 0, r[1], r[2], r[3], 1] {
                b.extend(x.to_le_bytes());
            }
            b.extend([0; 8]);
        }
        b.extend([0; 8]);
        b.extend(n.to_le_bytes());
        b.extend([0; 8]);
        for (r, m) in ranges.iter().zip(materials) {
            b.extend(MATERIAL_DESCRIPTOR.to_le_bytes());
            b.extend([1, 0, 0, 0, 0]);
            b.extend(u16::try_from(r[1]).expect("small").to_le_bytes());
            b.push(0);
            b.extend(m.to_le_bytes());
            b.extend([0; 8]);
        }
        b.extend(n.to_le_bytes());
        b.extend([0; 6]);
        for m in materials {
            b.extend([0, 0, 0xAF, 0x40, 0x16, 0x9F, 1, 1]);
            b.extend(m.to_le_bytes());
            b.push(0);
            b.extend(m.to_le_bytes());
            b.extend([0; 6]);
        }
        b.extend([0; 16]);
        b.extend(0x21u32.to_le_bytes());
        b
    }

    #[test]
    fn a_tail_reads_each_ranges_material() {
        let ranges = [[0, 3, 0, 1], [3, 3, 3, 1]];
        let bytes = tail(&ranges, &[11, 22]);
        let t = StoredTail::parse(&bytes, 0, 2).expect("tail");
        assert_eq!(
            t.descriptors.iter().map(|d| d.material).collect::<Vec<_>>(),
            [11, 22]
        );
        let mut m = mesh(6, &[0, 1, 2, 0, 1, 2], &[(0, 0), (3, 3)]);
        m.tail = t;
        assert_eq!(m.materials().expect("materials"), [11, 22]);
        // a MeshPrimitive entry disagreeing with its range
        let mut wrong = m.clone();
        wrong.tail.primitives[1].vertex_base = 2;
        assert!(matches!(
            wrong.materials(),
            Err(Error::Tail { what: "ranges", .. })
        ));
        // a descriptor's vertex count disagreeing
        let mut wrong = m;
        wrong.tail.descriptors[0].vertex_count = 4;
        assert!(matches!(
            wrong.materials(),
            Err(Error::Tail { what: "ranges", .. })
        ));
    }

    #[test]
    fn a_tail_off_its_grammar_is_refused() {
        let good = tail(&[[0, 3, 0, 1], [3, 3, 3, 1]], &[11, 22]);
        let what = |bytes: &[u8], n: usize| match StoredTail::parse(bytes, 0, n) {
            Err(Error::Tail { what, .. }) => what,
            Err(Error::Short { what, .. }) => what,
            other => panic!("not refused: {other:?}"),
        };
        // another range count than the mesh's
        assert_eq!(what(&good, 3), "block length");
        // a group's ClassID flipped (the first group's first copy)
        let groups = good.len() - 20 - 2 * 31;
        let mut b = good.clone();
        b[groups + 8] ^= 0xFF;
        assert_eq!(what(&b, 2), "group");
        // a nonzero byte beside a descriptor's vertex count
        let descriptors = groups - 6 - 4 - 2 * 28;
        let mut b = good.clone();
        b[descriptors + 11] = 1;
        assert_eq!(what(&b, 2), "material descriptor vertex count");
        // a MeshPrimitive of another version
        let primitives = 4 + 64 + 8 + 22;
        let mut b = good.clone();
        b[primitives + 24] = 2;
        assert_eq!(what(&b, 2), "MeshPrimitive version");
        // a byte past the end, and one short
        let mut b = good.clone();
        b.push(0);
        assert_eq!(what(&b, 2), "end");
        assert_eq!(what(&good[..good.len() - 1], 2), "last bytes");
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
