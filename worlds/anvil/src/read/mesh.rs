//! A Mesh resource's body: its compiled mesh's vertex and index buffers, read as stored. Nothing about a vertex's
//! fields is decided here beyond their layout; [`StoredVertices::positions`] is the one decode, its evidence the bounds.
//!
//! The body (little-endian; offsets from the body's start, after the resource's class id and extension) starts with a
//! 61-byte header, kept as stored, whose bounds (two f32 triples at 20 and 36) are read, then as many 81-byte records
//! as the u32 at 16 counts, kept as stored (on every Mesh of Riders Republic the compiled mesh's marker is at 61 + 81
//! times that count: 29,174 meshes with 1 to 212, the rest with 0). There the compiled mesh starts: its marker 0xFC9E1595 (CRC-32 of `CompiledMesh`), a byte at marker+4, 8 bytes, and at marker+13 a u32
//! that with that byte names its layout ([`Layout`]). Two layouts are read; another is refused:
//!
//! - **ranged** (0 at marker+4, 0xC351EE43 at marker+13): a u32 at marker+22 (24 on every ranged fixture, equal to
//!   the vertex size; only 24 is read), the range record count N (u32 at marker+63), then from marker+75 N range
//!   records, each 0x07B19A87 (u32), a byte (1), three bytes, and two u32: the range's first index and its vertex
//!   base; every record but the last carries 8 more bytes. Then the vertex buffer's byte length (u32) and the
//!   vertices; 8 bytes; the index buffer's byte length (u32) and the indices (u16). Then a u32 byte length (32 per
//!   range) and that many bytes; 8 bytes; `MeshData` (its CRC-32 0x0645ABB5 and 18 bytes); one
//!   `MeshPrimitive` entry per range ([`StoredPrimitive`], 8 bytes after each but the last); four runs, each a u32
//!   length and that many bytes ([`Ranged::runs`]); then the material list.
//! - **inline** (3 at marker+4, `MeshData`'s CRC-32 at marker+13): `MeshData`'s 18 bytes; one `MeshPrimitive` entry;
//!   the vertex buffer's byte length (u32, 24 bytes per vertex of the entry's count) and the vertices; 8 bytes; the
//!   index buffer's byte length (u32) and the indices, 2 bytes each when `MeshData`'s first byte is 0 and 4 when it
//!   is 1; then the material list.
//!
//! A `MeshPrimitive` entry: 0xA57387EF (equal to the CRC-32 of `MeshPrimitive`), vertex base, a u32, vertex count,
//! first index, triangle count and a u32 (1); the four counts are checked against the ranges, so they are
//! established. The material list, ending the body ([`MaterialList`]): the range count (u32) and 8 bytes; one material
//! descriptor per range (0x0B1D34C1, 4 bytes, a zero byte, a u16 vertex count, a zero byte, the u64 ClassID of a
//! Material, 8 bytes); the range count (u32) and 6 bytes; one 31-byte group per range, holding its descriptor's ClassID
//! at +8 and +17; 20 bytes.
//!
//! The indices are in ranges: a ranged mesh's one per record, running from its first index to the next record's (the
//! last to the buffer's end), its stored indices counting from its vertex base and its vertices running from its base
//! to the next record's; an inline mesh's one, its `MeshPrimitive` entry's, which must cover both buffers. Refused,
//! naming it: a header without the marker at 61, another layout or stride, record id, marker, version or range count,
//! a buffer past the body, an index buffer not of whole triangles, ranges that don't partition the buffers in order
//! (the first at index and vertex 0, each range at a whole triangle and at or after the one before, none past either
//! buffer), an index outside its range's vertices, a byte other than 0 either side of a descriptor's vertex count, a
//! group's ClassID not its descriptor's, or bytes left after the last 20. [`StoredMesh::materials`] refuses a
//! `MeshPrimitive` entry or descriptor that disagrees with its range. An empty range and vertices no triangle names are
//! read, not refused.
//!
//! Established on seven ranged meshes (Riders Republic build 25394987: one to six records, strides 24; every byte to
//! the body's end accounted for, games/riders-republic docs/recon/material-chain-binding.md, 2b3215e) and two inline
//! ones (the Favela BigTurn A and B LOD0s, 72,260 and 93,906 vertices; docs/recon/layoutb-favela-observation.md,
//! d099244). Observed, not required: no range is empty and every vertex is named. A descriptor's vertex count is its
//! range's low 16 bits (the two inline meshes' counts pass 65535 and are stored so). That descriptor *k* is range *k*'s
//! material is read from their order and the counts agreeing; how the engine binds it is not established. The bytes
//! kept as stored are not interpreted.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::ops::Range;

/// A record type: equals the CRC-32 of the ASCII string `Mesh` (the name a label).
pub const MESH: u32 = 0x415D_9568;
/// The compiled mesh's marker: equals the CRC-32 of `CompiledMesh` (the name a label).
pub const COMPILED_MESH: u32 = 0xFC9E_1595;
/// The ranged layout's id at marker+13 (its name unknown).
pub const RANGED: u32 = 0xC351_EE43;
/// Where the compiled mesh starts in the body.
pub const HEADER: usize = 61;
/// A range record's id.
pub const RANGE_RECORD: u32 = 0x07B1_9A87;
/// The vertex strides read: the ranged layout's u32s at marker+22, +26 and +30 are one of these, then 0 and 0.
pub const STRIDES: [u32; 3] = [16, 24, 28];
/// Equals the CRC-32 of `MeshData` (the name a label): the inline layout's id at marker+13, and in the ranged
/// layout's tail.
pub const MESH_DATA: u32 = 0x0645_ABB5;
/// A `MeshPrimitive` entry's marker: equals the CRC-32 of `MeshPrimitive` (the name a label).
pub const MESH_PRIMITIVE: u32 = 0xA573_87EF;
/// A material descriptor's marker (its name unknown).
pub const MATERIAL_DESCRIPTOR: u32 = 0x0B1D_34C1;
/// The record type of the entries material descriptors' ClassIDs resolve to on the fixtures: equals the CRC-32 of
/// `Material` (the name a label).
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
    #[error("Mesh body: {found:#010x} at {at}, not the compiled mesh marker 0xFC9E1595")]
    Marker { found: u32, at: usize },
    #[error(
        "Mesh compiled layout {id:#010x} at marker+13, byte {flags} at marker+4: only 0xC351EE43 with 0 (ranged) and \
         0x0645ABB5 with 3 (inline) are read"
    )]
    Layout { id: u32, flags: u8 },
    #[error("Mesh u32s at marker+22, +26 and +30: {0:?}, only 16, 24 or 28 then 0 and 0 are read")]
    Stride([u32; 3]),
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

/// A vertex buffer as stored: whole vertices of `stride` bytes (16, 24 or 28 in the ranged layout, 24 inline). Only
/// each vertex's first 8 bytes are decoded ([`StoredVertices::positions`]); what the rest hold is not established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredVertices {
    pub stride: usize,
    pub bytes: Vec<u8>,
}

impl StoredVertices {
    /// The vertex count.
    pub fn len(&self) -> usize {
        self.bytes.len().checked_div(self.stride).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Each vertex's bytes.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.bytes.chunks_exact(self.stride.max(1))
    }

    /// Each vertex's first 8 bytes: three i16 and an i16 `w`.
    fn heads(&self) -> impl Iterator<Item = ([i16; 3], i16)> {
        self.iter().map(|v| {
            let [p0, p1, p2, p3, p4, p5, w0, w1] = v.first_chunk::<8>().copied().unwrap_or([0; 8]);
            (
                [
                    i16::from_le_bytes([p0, p1]),
                    i16::from_le_bytes([p2, p3]),
                    i16::from_le_bytes([p4, p5]),
                ],
                i16::from_le_bytes([w0, w1]),
            )
        })
    }

    /// Each vertex's position: its first three i16 times its fourth over 32768. Evidence is the extrema alone: on
    /// every fixture in tests/meshes.rs each axis's least and greatest differs from the header's stored bound by at
    /// most one step (`|w| / 32768`; largest 1.14e-4), and the census measures the same on every mesh of strides 16
    /// (2,036) and 28 (86) in Riders Republic. No stored value checks a vertex between them.
    pub fn positions(&self) -> impl Iterator<Item = [f32; 3]> {
        self.heads().map(|(xyz, w)| {
            let s = f32::from(w) / 32768.0;
            xyz.map(|v| f32::from(v) * s)
        })
    }

    /// Each vertex's `w`, the i16 at 6.
    pub fn ws(&self) -> impl Iterator<Item = i16> {
        self.heads().map(|(_, w)| w)
    }

    /// Each vertex's bytes 20..24 as two little-endian u16, for stride 24 (a layout reading; their meaning is not
    /// established); none for another stride.
    pub fn words_20(&self) -> Vec<[u16; 2]> {
        if self.stride != 24 {
            return Vec::new();
        }
        self.iter()
            .map(|v| {
                let [a, b, c, d] = v.get(20..24).and_then(|w| w.try_into().ok()).unwrap_or([0; 4]);
                [u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d])]
            })
            .collect()
    }
}

/// One range of a Mesh's buffers: its stored indices count from `vertices.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshRange {
    pub indices: Range<usize>,
    pub vertices: Range<usize>,
}

/// One `MeshPrimitive` entry as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPrimitive {
    pub vertex_base: u32,
    /// The u32 after the vertex base (0 on every entry seen).
    pub reserved: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub triangle_count: u32,
    /// The 8 bytes after an entry in the ranged layout's tail (none after the inline layout's).
    pub rest: Option<[u8; 8]>,
}

/// One material descriptor as stored (28 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialDescriptor {
    /// The 4 bytes after its marker.
    pub head: [u8; 4],
    /// Its range's vertex count's low 16 bits, between two zero bytes.
    pub vertex_count: u16,
    /// The ClassID of a Material resource.
    pub material: u64,
    /// The 8 bytes after the ClassID.
    pub rest: [u8; 8],
}

/// The material list ending a Mesh body, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialList {
    /// The 8 bytes after the first range count.
    pub head: [u8; 8],
    pub descriptors: Vec<MaterialDescriptor>,
    /// The 6 bytes after the second range count.
    pub groups_head: [u8; 6],
    /// The 31-byte groups, one per range.
    pub groups: Vec<[u8; 31]>,
    /// The body's last 20 bytes.
    pub trailer: [u8; 20],
}

/// The ranged layout's fields, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranged {
    /// The compiled mesh's bytes from its marker to its first range record (75).
    pub compiled_head: Vec<u8>,
    pub records: Vec<RangeRecord>,
    /// The tail's 32-byte blocks, one per range.
    pub blocks: Vec<[u8; 32]>,
    /// The 8 bytes after them.
    pub blocks_gap: [u8; 8],
    /// `MeshData`'s 18 bytes after its marker.
    pub mesh_data: [u8; 18],
    pub primitives: Vec<StoredPrimitive>,
    /// The four runs after the last entry, each a u32 length and that many bytes, as stored (on 3,984 meshes of Riders
    /// Republic the first and fourth are as long as the vertex and index buffers, which RRAssetStudio calls CPU
    /// copies; empty on every other mesh read).
    pub runs: [Vec<u8>; 4],
}

/// The inline layout's fields, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    /// `MeshData`'s 18 bytes after its marker.
    pub mesh_data: [u8; 18],
    pub primitive: StoredPrimitive,
}

/// A compiled mesh's layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Range records before the buffers, u16 indices, `MeshData` and the `MeshPrimitive` entries after.
    Ranged(Ranged),
    /// `MeshData` and one `MeshPrimitive` entry before the buffers, 2- or 4-byte indices by `MeshData`'s first byte.
    Inline(Inline),
}

/// A Mesh body's compiled buffers, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMesh {
    /// The 61 header bytes.
    pub header: Vec<u8>,
    /// The 81-byte records after them, as many as the u32 at 16 counts (none on meshes without), as stored.
    pub records_81: Vec<[u8; 81]>,
    /// The header's two f32 triples at 20 and 36 (each fixture's decoded extrema lie within a step of them).
    pub bounds: [[f32; 3]; 2],
    /// The 8 bytes after the compiled mesh's layout byte (marker+5).
    pub compiled_flags: [u8; 8],
    pub layout: Layout,
    pub vertices: StoredVertices,
    /// The 8 bytes between the vertices and the index buffer's length.
    pub gap: [u8; 8],
    /// The indices (u16 in the ranged layout, u32 in the inline).
    pub indices: Vec<u32>,
    pub material_list: MaterialList,
}

/// The bytes before the compiled mesh.
struct Head {
    header: Vec<u8>,
    records_81: Vec<[u8; 81]>,
    bounds: [[f32; 3]; 2],
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
    /// A u32 that must be `want`, refused as a tail field otherwise.
    fn marker(&mut self, what: &'static str, want: u32) -> Result<(), Error> {
        let found = self.u32(what)?;
        if found == want {
            Ok(())
        } else {
            Err(Error::Tail {
                what,
                why: format!("{found:#010x}, not {want:#010x}"),
            })
        }
    }
    /// A u32 that must be the range count `n`.
    fn count(&mut self, what: &'static str, n: usize) -> Result<(), Error> {
        let found = self.u32(what)?;
        if usize::try_from(found).ok() == Some(n) {
            Ok(())
        } else {
            Err(Error::Tail {
                what,
                why: format!("{found}, not the {n} ranges"),
            })
        }
    }
    /// A byte length read as a usize.
    fn length(&mut self, what: &'static str) -> Result<usize, Error> {
        usize::try_from(self.u32(what)?).map_err(|_| Error::Buffer {
            what,
            why: "length past this machine".into(),
        })
    }
    /// A `MeshPrimitive` entry after its marker, with the 8 bytes after it when `rest`.
    fn primitive(&mut self, rest: bool) -> Result<StoredPrimitive, Error> {
        self.marker("MeshPrimitive marker", MESH_PRIMITIVE)?;
        let vertex_base = self.u32("MeshPrimitive vertex base")?;
        let reserved = self.u32("MeshPrimitive")?;
        let vertex_count = self.u32("MeshPrimitive vertex count")?;
        let first_index = self.u32("MeshPrimitive first index")?;
        let triangle_count = self.u32("MeshPrimitive triangle count")?;
        let version = self.u32("MeshPrimitive version")?;
        if version != 1 {
            return Err(Error::Tail {
                what: "MeshPrimitive version",
                why: format!("{version}, not 1"),
            });
        }
        Ok(StoredPrimitive {
            vertex_base,
            reserved,
            vertex_count,
            first_index,
            triangle_count,
            rest: if rest {
                Some(self.array::<8>("MeshPrimitive")?)
            } else {
                None
            },
        })
    }
    /// `vertex_bytes` of `stride`-byte vertices.
    fn vertices(&mut self, vertex_bytes: usize, stride: usize) -> Result<StoredVertices, Error> {
        if vertex_bytes.checked_rem(stride) != Some(0) {
            return Err(Error::Buffer {
                what: "vertex buffer",
                why: format!("{vertex_bytes} bytes, not whole {stride}-byte vertices"),
            });
        }
        Ok(StoredVertices {
            stride,
            bytes: self.take(vertex_bytes, "vertex buffer")?.to_vec(),
        })
    }
    /// The index buffer: its byte length, then indices of `width` bytes (2 or 4) in whole triangles.
    fn indices(&mut self, width: usize) -> Result<Vec<u32>, Error> {
        let index_bytes = self.length("index buffer length")?;
        if index_bytes.checked_rem(width.saturating_mul(3)) != Some(0) {
            return Err(Error::Buffer {
                what: "index buffer",
                why: format!("{index_bytes} bytes, not whole triangles of {width}-byte indices"),
            });
        }
        let raw = self.take(index_bytes, "index buffer")?;
        Ok(if width == 2 {
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|w| u32::from(u16::from_le_bytes(*w)))
                .collect()
        } else {
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|w| u32::from_le_bytes(*w))
                .collect()
        })
    }
    /// The material list for `n` ranges, which must end the body.
    fn material_list(&mut self, n: usize) -> Result<MaterialList, Error> {
        self.count("range count before the descriptors", n)?;
        let head = self.array::<8>("bytes before the descriptors")?;
        let mut descriptors = Vec::with_capacity(n);
        for _ in 0..n {
            self.marker("material descriptor marker", MATERIAL_DESCRIPTOR)?;
            let head = self.array::<4>("material descriptor")?;
            let [z0, v0, v1, z1] = self.array::<4>("material descriptor vertex count")?;
            if [z0, z1] != [0, 0] {
                return Err(Error::Tail {
                    what: "material descriptor vertex count",
                    why: format!("bytes {z0} and {z1} either side of it: only 0 is read"),
                });
            }
            let material = u64::from_le_bytes(self.array::<8>("material descriptor ClassID")?);
            descriptors.push(MaterialDescriptor {
                head,
                vertex_count: u16::from_le_bytes([v0, v1]),
                material,
                rest: self.array::<8>("material descriptor")?,
            });
        }
        self.count("range count after the descriptors", n)?;
        let groups_head = self.array::<6>("bytes before the groups")?;
        let mut groups = Vec::with_capacity(n);
        for d in &descriptors {
            let g = self.array::<31>("group")?;
            let id = |at: usize| {
                g.get(at..at.saturating_add(8))
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_le_bytes)
            };
            if id(8) != Some(d.material) || id(17) != Some(d.material) {
                return Err(Error::Tail {
                    what: "group",
                    why: format!(
                        "ClassIDs {:?} and {:?}, not its descriptor's {}",
                        id(8),
                        id(17),
                        d.material
                    ),
                });
            }
            groups.push(g);
        }
        let trailer = self.array::<20>("last bytes")?;
        if self.at != self.b.len() {
            return Err(Error::Tail {
                what: "end",
                why: format!("{} bytes after its last 20", self.b.len().saturating_sub(self.at)),
            });
        }
        Ok(MaterialList {
            head,
            descriptors,
            groups_head,
            groups,
            trailer,
        })
    }
}

/// An inline mesh's index width, by `MeshData`'s first byte: 0, 2 bytes; 1, 4 bytes. Established by the census
/// (worlds/anvil/tests/census.rs): with it every inline mesh's index buffer is whole triangles of its MeshPrimitive's
/// count. Not a ranged mesh's: one has 1 there and 2-byte indices its ranges and MeshPrimitive entries agree with.
/// Measured over every mesh of Riders Republic (110,779), the byte is 1 exactly when the mesh has more than 65,535
/// vertices; that relation is recorded, not read.
fn index_width(mesh_data: &[u8; 18]) -> Result<usize, Error> {
    match mesh_data.first() {
        Some(0) => Ok(2),
        Some(1) => Ok(4),
        found => Err(Error::Tail {
            what: "MeshData",
            why: format!("first byte {found:?}: only 0 (2-byte indices) and 1 (4-byte) are read"),
        }),
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
            header
                .get(at..at.saturating_add(12))
                .and_then(|s| s.try_into().ok())
                .map(f32s)
                .ok_or(Error::Short {
                    what: "header bounds",
                    at,
                    have: body.len(),
                })
        };
        let bounds = [bound(20)?, bound(36)?];
        let count = header
            .get(16..20)
            .and_then(|w| w.try_into().ok())
            .map_or(0, u32::from_le_bytes);
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count > body.len() / 81 {
            return Err(Error::Short {
                what: "81-byte records",
                at: HEADER,
                have: body.len(),
            });
        }
        let records_81 = (0..count)
            .map(|_| c.array::<81>("81-byte record"))
            .collect::<Result<Vec<_>, _>>()?;
        let marker = c.at;
        let found = c.u32("compiled mesh marker")?;
        if found != COMPILED_MESH {
            return Err(Error::Marker { found, at: marker });
        }
        let head = Head {
            header,
            records_81,
            bounds,
        };
        let [flags] = c.array::<1>("compiled mesh layout byte")?;
        let compiled_flags = c.array::<8>("compiled mesh")?;
        let id = c.u32("compiled mesh layout")?;
        match (id, flags) {
            (RANGED, 0) => Self::ranged(body, c, marker, head, compiled_flags),
            (MESH_DATA, 3) => Self::inline(c, head, compiled_flags),
            _ => Err(Error::Layout { id, flags }),
        }
    }

    fn ranged(
        body: &[u8],
        mut c: Cursor<'_>,
        marker: usize,
        head: Head,
        compiled_flags: [u8; 8],
    ) -> Result<StoredMesh, Error> {
        // the compiled head is kept whole, from its marker
        let compiled_head = body
            .get(marker..marker.saturating_add(75))
            .ok_or(Error::Short {
                what: "compiled mesh head",
                at: marker,
                have: body.len(),
            })?
            .to_vec();
        c.at = marker.saturating_add(75);
        let word = |at: usize| -> u32 {
            compiled_head
                .get(at..at.saturating_add(4))
                .and_then(|w| w.try_into().ok())
                .map_or(0, u32::from_le_bytes)
        };
        let strides = [word(22), word(26), word(30)];
        let [stride, 0, 0] = strides else {
            return Err(Error::Stride(strides));
        };
        if !STRIDES.contains(&stride) {
            return Err(Error::Stride(strides));
        }
        let stride = usize::try_from(stride).map_err(|_| Error::Stride(strides))?;
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
            let extension = if index.checked_add(1) == Some(n) {
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
        let vertex_bytes = c.length("vertex buffer length")?;
        let vertices = c.vertices(vertex_bytes, stride)?;
        let gap = c.array::<8>("bytes before the index buffer")?;
        let indices = c.indices(2)?;
        let block_bytes = c.u32("block length")?;
        if usize::try_from(block_bytes).ok() != n.checked_mul(32) {
            return Err(Error::Tail {
                what: "block length",
                why: format!("{block_bytes}, not 32 for each of {n} ranges"),
            });
        }
        let blocks = (0..n).map(|_| c.array::<32>("block")).collect::<Result<_, _>>()?;
        let blocks_gap = c.array::<8>("bytes before MeshData")?;
        c.marker("MeshData marker", MESH_DATA)?;
        let mesh_data = c.array::<18>("MeshData")?;
        let primitives = (0..n)
            .map(|k| c.primitive(k.saturating_add(1) < n))
            .collect::<Result<_, _>>()?;
        let mut run = || -> Result<Vec<u8>, Error> {
            let length = c.length("tail run length")?;
            Ok(c.take(length, "tail run")?.to_vec())
        };
        let runs = [run()?, run()?, run()?, run()?];
        let materials = c.material_list(n)?;
        Ok(StoredMesh {
            header: head.header,
            records_81: head.records_81,
            bounds: head.bounds,
            compiled_flags,
            layout: Layout::Ranged(Ranged {
                compiled_head,
                records,
                blocks,
                blocks_gap,
                mesh_data,
                primitives,
                runs,
            }),
            vertices,
            gap,
            indices,
            material_list: materials,
        })
    }

    fn inline(mut c: Cursor<'_>, head: Head, compiled_flags: [u8; 8]) -> Result<StoredMesh, Error> {
        let mesh_data = c.array::<18>("MeshData")?;
        let primitive = c.primitive(false)?;
        let vertex_bytes = c.length("vertex buffer length")?;
        let count = usize::try_from(primitive.vertex_count).unwrap_or(usize::MAX);
        if count.checked_mul(24) != Some(vertex_bytes) {
            return Err(Error::Buffer {
                what: "vertex buffer",
                why: format!("{vertex_bytes} bytes, not 24 for each of its MeshPrimitive's {count} vertices"),
            });
        }
        let vertices = c.vertices(vertex_bytes, 24)?;
        let gap = c.array::<8>("bytes before the index buffer")?;
        let indices = c.indices(index_width(&mesh_data)?)?;
        let materials = c.material_list(1)?;
        Ok(StoredMesh {
            header: head.header,
            records_81: head.records_81,
            bounds: head.bounds,
            compiled_flags,
            layout: Layout::Inline(Inline { mesh_data, primitive }),
            vertices,
            gap,
            indices,
            material_list: materials,
        })
    }

    /// The `MeshPrimitive` entries, one per range.
    pub fn primitives(&self) -> &[StoredPrimitive] {
        match &self.layout {
            Layout::Ranged(r) => &r.primitives,
            Layout::Inline(i) => std::slice::from_ref(&i.primitive),
        }
    }

    /// Each range's indices and vertices, refused unless they tile both buffers (the first at 0, each at a whole
    /// triangle and at or after the one before, the last ending each buffer).
    pub fn ranges(&self) -> Result<Vec<MeshRange>, Error> {
        let at = |x: u32| usize::try_from(x).unwrap_or(usize::MAX);
        let starts: Vec<(usize, usize)> = match &self.layout {
            Layout::Ranged(r) => r
                .records
                .iter()
                .map(|r| (at(r.first_index), at(r.vertex_base)))
                .collect(),
            Layout::Inline(i) => {
                // its one entry must cover both buffers
                let p = &i.primitive;
                let covers = at(p.triangle_count).checked_mul(3) == Some(self.indices.len())
                    && at(p.vertex_count) == self.vertices.len();
                if !covers {
                    return Err(Error::Record {
                        index: 0,
                        why: format!(
                            "MeshPrimitive of {} triangles and {} vertices, the buffers {} indices and {} vertices",
                            p.triangle_count,
                            p.vertex_count,
                            self.indices.len(),
                            self.vertices.len()
                        ),
                    });
                }
                vec![(at(p.first_index), at(p.vertex_base))]
            }
        };
        let refuse = |index: usize, why: String| Error::Record { index, why };
        let mut out = Vec::with_capacity(starts.len());
        for (index, &(first, base)) in starts.iter().enumerate() {
            let (end, vertices_end) = starts
                .get(index.saturating_add(1))
                .copied()
                .unwrap_or((self.indices.len(), self.vertices.len()));
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

    /// Each range's material: its descriptor's Material ClassID, refused unless the range's `MeshPrimitive` entry and
    /// descriptor agree with it (vertex base and count, first index, triangle count; the descriptor's count is the
    /// range's low 16 bits).
    pub fn materials(&self) -> Result<Vec<u64>, Error> {
        let ranges = self.ranges()?;
        let primitives = self.primitives();
        let mut out = Vec::with_capacity(ranges.len());
        for (k, r) in ranges.iter().enumerate() {
            let (Some(p), Some(d)) = (primitives.get(k), self.material_list.descriptors.get(k)) else {
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
                vertices & 0xFFFF,
            ];
            if stored != range {
                return Err(Error::Tail {
                    what: "ranges",
                    why: format!(
                        "range {k}: stored (vertex base, vertex count, first index, triangle count, descriptor's \
                         count) {stored:?}, the range's {range:?}"
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
                let v = vertices
                    .start
                    .saturating_add(usize::try_from(i).unwrap_or(usize::MAX));
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
    clippy::cast_possible_truncation,
    reason = "test fixtures built by hand"
)]
mod tests {
    use super::*;

    /// A material list's bytes for these descriptor counts and materials.
    fn material_list(counts: &[u16], materials: &[u64]) -> Vec<u8> {
        let n = counts.len() as u32;
        let mut b = Vec::new();
        b.extend(n.to_le_bytes());
        b.extend([0; 8]);
        for (c, m) in counts.iter().zip(materials) {
            b.extend(MATERIAL_DESCRIPTOR.to_le_bytes());
            b.extend([1, 0, 0, 0, 0]);
            b.extend(c.to_le_bytes());
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

    fn primitive(b: &mut Vec<u8>, [base, count, first, triangles]: [u32; 4]) {
        b.extend(MESH_PRIMITIVE.to_le_bytes());
        for x in [base, 0, count, first, triangles, 1] {
            b.extend(x.to_le_bytes());
        }
    }

    /// The header and the compiled mesh's first 17 bytes.
    fn head(flags: u8, layout: u32) -> Vec<u8> {
        let mut b = vec![0; HEADER];
        b.extend(COMPILED_MESH.to_le_bytes());
        b.push(flags);
        b.extend([0; 8]);
        b.extend(layout.to_le_bytes());
        b
    }

    /// A ranged body of `vertices` vertices, these u16 indices, records (first index, vertex base) and materials.
    fn ranged(vertices: usize, indices: &[u16], records: &[(u32, u32)], materials: &[u64]) -> Vec<u8> {
        let n = records.len();
        let mut b = head(0, RANGED);
        let mut rest = vec![0; 75 - 17];
        rest[22 - 17..26 - 17].copy_from_slice(&24u32.to_le_bytes());
        rest[63 - 17..67 - 17].copy_from_slice(&(n as u32).to_le_bytes());
        b.extend(rest);
        for (k, (first, base)) in records.iter().enumerate() {
            b.extend(RANGE_RECORD.to_le_bytes());
            b.extend([1, 0, 0, 0]);
            b.extend(first.to_le_bytes());
            b.extend(base.to_le_bytes());
            if k + 1 < n {
                b.extend([0; 8]);
            }
        }
        b.extend(((vertices * 24) as u32).to_le_bytes());
        b.extend(vec![0; vertices * 24]);
        b.extend([0; 8]);
        b.extend(((indices.len() * 2) as u32).to_le_bytes());
        indices.iter().for_each(|i| b.extend(i.to_le_bytes()));
        b.extend(((32 * n) as u32).to_le_bytes());
        b.extend(vec![7; 32 * n]);
        b.extend([0; 8]);
        b.extend(MESH_DATA.to_le_bytes());
        b.extend([0; 18]);
        let mut counts = Vec::new();
        for (k, &(first, base)) in records.iter().enumerate() {
            let (end, vend) = records
                .get(k + 1)
                .copied()
                .unwrap_or((indices.len() as u32, vertices as u32));
            primitive(&mut b, [base, vend - base, first, (end - first) / 3]);
            b.extend([0; 8]);
            counts.push((vend - base) as u16);
        }
        b.extend([0; 8]);
        b.extend(material_list(&counts, materials));
        b
    }

    /// An inline body of `vertices` vertices, these 4-byte indices and one material.
    fn inline(vertices: usize, indices: &[u32], material: u64) -> Vec<u8> {
        let mut b = head(3, MESH_DATA);
        b.push(1);
        b.extend([0; 17]);
        primitive(&mut b, [0, vertices as u32, 0, (indices.len() / 3) as u32]);
        b.extend(((vertices * 24) as u32).to_le_bytes());
        b.extend(vec![0; vertices * 24]);
        b.extend([0; 8]);
        b.extend(((indices.len() * 4) as u32).to_le_bytes());
        indices.iter().for_each(|i| b.extend(i.to_le_bytes()));
        b.extend(material_list(&[vertices as u16], &[material]));
        b
    }

    fn refused_as(body: &[u8]) -> &'static str {
        match StoredMesh::parse(body) {
            Err(Error::Tail { what, .. } | Error::Short { what, .. } | Error::Buffer { what, .. }) => what,
            other => panic!("not refused: {other:?}"),
        }
    }

    /// A ranged mesh of these records, parsed.
    fn mesh(vertices: usize, indices: &[u16], records: &[(u32, u32)]) -> StoredMesh {
        let mut m = StoredMesh::parse(&ranged(vertices, &[], &[(0, 0)], &[1])).expect("parse");
        m.indices = indices.iter().map(|&i| u32::from(i)).collect();
        m.vertices = StoredVertices {
            stride: 24,
            bytes: vec![0; 24 * vertices],
        };
        if let Layout::Ranged(r) = &mut m.layout {
            r.records = records
                .iter()
                .map(|&(first_index, vertex_base)| RangeRecord {
                    pad: [0; 3],
                    first_index,
                    vertex_base,
                    extension: None,
                })
                .collect();
        }
        m
    }

    #[test]
    fn a_ranged_body_reads_its_ranges_and_materials() {
        let m =
            StoredMesh::parse(&ranged(6, &[0, 1, 2, 0, 2, 1], &[(0, 0), (3, 3)], &[11, 22])).expect("parse");
        assert_eq!(m.triangles().expect("read"), [[0, 1, 2], [3, 5, 4]]);
        assert_eq!(m.materials().expect("materials"), [11, 22]);
        // a MeshPrimitive entry or a descriptor's count disagreeing with its range
        let mut wrong = m.clone();
        if let Layout::Ranged(r) = &mut wrong.layout {
            r.primitives[1].vertex_base = 2;
        }
        assert!(matches!(
            wrong.materials(),
            Err(Error::Tail { what: "ranges", .. })
        ));
        let mut wrong = m;
        wrong.material_list.descriptors[0].vertex_count = 4;
        assert!(matches!(
            wrong.materials(),
            Err(Error::Tail { what: "ranges", .. })
        ));
    }

    #[test]
    fn an_inline_body_reads_its_one_range_and_u32_indices() {
        // 70,000 vertices: an index past 65535, and a descriptor count of the low 16 bits (70,000 - 65,536)
        let n = 70_000;
        let body = inline(n, &[0, 1, 69_999, 69_999, 1, 2], 33);
        let m = StoredMesh::parse(&body).expect("parse");
        assert_eq!(m.triangles().expect("read"), [[0, 1, 69_999], [69_999, 1, 2]]);
        assert_eq!(m.material_list.descriptors[0].vertex_count, 4464);
        assert_eq!(m.materials().expect("materials"), [33]);
        // its entry not covering the buffers
        let mut wrong = m;
        if let Layout::Inline(i) = &mut wrong.layout {
            i.primitive.triangle_count = 1;
        }
        assert!(matches!(wrong.triangles(), Err(Error::Record { index: 0, .. })));
    }

    #[test]
    fn an_inline_bodys_index_width_is_meshdatas_first_byte() {
        // 2-byte indices when it is 0
        let mut b = head(3, MESH_DATA);
        b.push(0);
        b.extend([0; 17]);
        primitive(&mut b, [0, 3, 0, 1]);
        b.extend(72u32.to_le_bytes());
        b.extend([0; 72]);
        b.extend([0; 8]);
        b.extend(6u32.to_le_bytes());
        [0u16, 2, 1].iter().for_each(|i| b.extend(i.to_le_bytes()));
        b.extend(material_list(&[3], &[9]));
        let m = StoredMesh::parse(&b).expect("parse");
        assert_eq!(m.triangles().expect("read"), [[0, 2, 1]]);
        // another first byte is refused
        let at = HEADER + 17;
        b[at] = 2;
        assert!(matches!(
            StoredMesh::parse(&b),
            Err(Error::Tail { what: "MeshData", .. })
        ));
    }

    #[test]
    fn another_layout_is_refused() {
        for (flags, id) in [(3, RANGED), (0, MESH_DATA), (0, 1)] {
            let mut b = head(flags, id);
            b.extend([0; 200]);
            assert!(matches!(
                StoredMesh::parse(&b),
                Err(Error::Layout { id: i, flags: f }) if (i, f) == (id, flags)
            ));
        }
    }

    #[test]
    fn a_ranged_tails_runs_after_the_last_entry_are_read() {
        let mut b = ranged(6, &[0, 1, 2, 0, 2, 1], &[(0, 0), (3, 3)], &[11, 22]);
        // the first run, after the last entry (the second ends 28 bytes after the first's 36), holds 4 bytes
        let groups = b.len() - 20 - 2 * 31;
        let descriptors = groups - 6 - 4 - 2 * 28;
        let runs = descriptors - 12 - 16;
        b[runs] = 4;
        for (k, x) in [9, 8, 7, 6].into_iter().enumerate() {
            b.insert(runs + 4 + k, x);
        }
        let m = StoredMesh::parse(&b).expect("parse");
        let Layout::Ranged(r) = &m.layout else {
            panic!("ranged")
        };
        assert_eq!(r.runs, [vec![9, 8, 7, 6], vec![], vec![], vec![]]);
        assert_eq!(r.primitives[0].rest, Some([0; 8]));
        assert_eq!(r.primitives[1].rest, None);
        assert_eq!(m.materials().expect("materials"), [11, 22]);
    }

    #[test]
    fn records_81_counted_at_16_come_before_the_compiled_mesh() {
        let plain = ranged(3, &[0, 1, 2], &[(0, 0)], &[5]);
        let mut b = plain.clone();
        b[16] = 2;
        for _ in 0..2 * 81 {
            b.insert(HEADER, 0xAB);
        }
        let m = StoredMesh::parse(&b).expect("parse");
        assert_eq!(m.records_81, vec![[0xAB; 81]; 2]);
        assert_eq!(m.triangles().expect("read"), [[0, 1, 2]]);
        // a count the records don't fill moves the marker off the compiled mesh
        let mut short = plain;
        short[16] = 1;
        assert!(matches!(
            StoredMesh::parse(&short),
            Err(Error::Marker { at: 142, .. })
        ));
    }

    #[test]
    fn a_tail_off_its_grammar_is_refused() {
        let good = ranged(6, &[0, 1, 2, 0, 1, 2], &[(0, 0), (3, 3)], &[11, 22]);
        let groups = good.len() - 20 - 2 * 31;
        let descriptors = groups - 6 - 4 - 2 * 28;
        let after_primitives = descriptors - 12 - 8;
        let primitives = after_primitives - 2 * 36;
        let edit = |at: usize, x: u8| {
            let mut b = good.clone();
            b[at] = x;
            b
        };
        assert_eq!(refused_as(&edit(groups + 8, 0xFF)), "group");
        assert_eq!(
            refused_as(&edit(descriptors + 11, 1)),
            "material descriptor vertex count"
        );
        assert_eq!(refused_as(&edit(primitives + 24, 2)), "MeshPrimitive version");
        // the third run's length 1: its byte taken, the fourth's length then runs past the body
        assert_eq!(refused_as(&edit(after_primitives, 1)), "tail run");
        assert_eq!(
            refused_as(&edit(descriptors - 12, 3)),
            "range count before the descriptors"
        );
        let mut b = good.clone();
        b.push(0);
        assert_eq!(refused_as(&b), "end");
        assert_eq!(refused_as(&good[..good.len() - 1]), "last bytes");
        // the inline layout's: a vertex buffer not its entry's count, and the same list
        let good = inline(3, &[0, 1, 2], 5);
        let vertex_bytes = HEADER + 17 + 18 + 28;
        let mut b = good.clone();
        b[vertex_bytes] = 0;
        assert_eq!(refused_as(&b), "vertex buffer");
        let mut b = good.clone();
        b.push(0);
        assert_eq!(refused_as(&b), "end");
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
