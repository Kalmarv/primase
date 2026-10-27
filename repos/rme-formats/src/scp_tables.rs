//! The SCP's node table, world-matrix table and blob section chain, read as authored.
//!
//! Layout (big-endian, offsets in the decoded SCP), from the game's files and checked over every level SCP:
//! header `+0x18/+0x1C` node count/offset (0x70-byte records), `+0x38/+0x3C` world-matrix table count/offset
//! (8-byte `{u32 hash, u32 matrix offset}` per node, same count), `+0x44` blob offset (0 or `0xFFFFFFFF`: none),
//! `+0x10/+0x14` placement count/offset (0x30-byte records, only to resolve a node's links).
//!
//! The blob is a chain of sections `{flags, size, count, data offset}`: `size` runs to the next section, the data
//! starts `data offset` bytes in (0x10, except 0x20 to 0x40 for type 0xE), the type is `flags >> 16` (the low half
//! varies and is kept), and the chain ends in a 16-byte record `{junk, 8, 0, 0}`. Type 2 is the spot chunks.
//! Types 0, 1, 9, 0xA and 0xC are splines (`scp_splines`; the low half of their flags is junk), type 3 is spot
//! cameras (`scp_cameras`), type 4 lights (`scp_lights`), type 5 locators (`scp_locators`) and type 0xB
//! occluders (`scp_occluders`).
//! Every other type is listed with its flags, size and count and reported unread.
use crate::Affine;
use crate::scene::Unread;
use crate::scp_cameras::{self, SpotCamera};
use crate::scp_lights::{self, SceneLight};
use crate::scp_locators::{self, Locator};
use crate::scp_occluders::{self, Occluder};
use crate::scp_splines::{self, Spline, SplineKind};
use crate::{Result, invalid};
use primase_foundations::bytes::{Endian, Field, View};

/// A node's links are `0x70`-byte records.
const NODE: usize = 0x70;
const PLACEMENT: usize = 0x30;
const SECTION_SPOTS: u32 = 2;
const SECTION_CAMERAS: u32 = 3;
const SECTION_LIGHTS: u32 = 4;
const SECTION_LOCATORS: u32 = 5;
const SECTION_OCCLUDERS: u32 = 0xb;
const CHUNK: usize = 0x200;
const CHUNK_MATRIX: usize = 64;

/// One node record. Links are indices into the same file's node and placement tables.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The record's id word (`+0x10`): a name hash, shared by spot chunks whose `id` it equals.
    pub id: u32,
    /// The world-matrix table entry's hash: a group name's hash for group nodes, a model's for single-mesh nodes,
    /// else 0.
    pub matrix_hash: u32,
    pub world: Affine,
    /// Words `+0x00..+0x0F` as floats when any is non-zero: centre and radius of a bounding sphere. Unproven: the
    /// centre lies 0.57 to 73 from the node's translation in one checked file.
    pub sphere: Option<[f32; 4]>,
    pub children: Vec<u32>,
    pub placements: Vec<u32>,
}

/// One blob section header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub offset: usize,
    pub flags: u32,
    pub size: usize,
    pub count: u32,
    pub data: usize,
}
impl Section {
    /// `flags >> 16`.
    pub fn kind(&self) -> u32 {
        self.flags >> 16
    }
}

/// How the section chain ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chain {
    /// The SCP has no blob.
    #[default]
    None,
    /// The walk reached the 16-byte end record `{junk, 8, 0, 0}` exactly at the end of the file.
    Complete,
    /// A header at `offset` is not a section and the file does not end there: the rest is unread.
    Stopped { offset: usize },
}

/// A spot chunk: a `0x200`-byte record and its own 64-byte world matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotChunk {
    /// Offset of the section holding it, and its index in the section.
    pub section: usize,
    pub index: u32,
    /// `+0x10`: hash of the chunk's name (a DB spot name, lower-cased or as written).
    pub name: u32,
    /// `+0x14`: its own id; a node has this id when the chunk is also a node.
    pub id: u32,
    /// `+0x18`: id of the parent group node.
    pub parent: u32,
    /// `+0x1C`: the class: sdbm of the spot class name ([`class_name`]).
    pub class: u32,
    /// `+0x20`: the shape: `Rectangle` or `Grid` hash ([`shape_name`]).
    pub shape: u32,
    /// `+0x24`, `+0x28`: two ids that pair chunks (Rectangle chunks carry consecutive ones). Unproven.
    pub pair: [u32; 2],
    /// The node whose `id` equals `id`, when there is one.
    pub node: Option<u32>,
    pub world: Affine,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    pub nodes: Vec<Node>,
    pub sections: Vec<Section>,
    pub chain: Chain,
    pub spots: Vec<SpotChunk>,
    pub splines: Vec<Spline>,
    pub lights: Vec<SceneLight>,
    pub cameras: Vec<SpotCamera>,
    pub locators: Vec<Locator>,
    pub occluders: Vec<Occluder>,
}

/// The spot classes whose names are proven: each hash is sdbm of the name, and the engine's `AddSpotChunk_*` list
/// names the classes. The game mixes names with and without a `Spot` suffix (`Attractor`, `BailSpot`). Spot classes in
/// the files whose hash matches no name here are kept by hash.
pub const CLASS_NAMES: [&str; 14] = [
    "Teleport",
    "Attractor",
    "AIJunction",
    "Spot",
    "SafeRespawn",
    "GenericTrigger",
    "AudioVolume",
    "NavJunction",
    "AudioTrigger",
    "BailSpot",
    "ChallengeArea",
    "ChallengeObj",
    "AITriggerSpot",
    "VFXSpot",
];

/// The name of a class hash among [`CLASS_NAMES`].
pub fn class_name(hash: u32) -> Option<&'static str> {
    CLASS_NAMES
        .into_iter()
        .find(|n| primase_foundations::hash::sdbm(n.as_bytes()) == hash)
}

/// `Rectangle` (a centred unit box in the chunk's frame) or `Grid`.
pub fn shape_name(hash: u32) -> Option<&'static str> {
    ["Rectangle", "Grid"]
        .into_iter()
        .find(|n| primase_foundations::hash::sdbm(n.as_bytes()) == hash)
}

pub(crate) fn matrix(v: View<'_>, at: usize) -> Result<Affine> {
    let mut cols = [[0.; 3]; 4];
    for (j, col) in cols.iter_mut().enumerate() {
        for (k, x) in col.iter_mut().enumerate() {
            *x = v.f32(at + j * 16 + k * 4, Endian::Big).field("matrix")?;
            if !x.is_finite() {
                return Err(invalid(at + j * 16 + k * 4, "scp: nonfinite matrix element"));
            }
        }
        let w = v.f32(at + j * 16 + 12, Endian::Big).field("matrix")?;
        if w != if j == 3 { 1. } else { 0. } {
            return Err(invalid(at + j * 16 + 12, "scp: nonaffine matrix"));
        }
    }
    Ok(Affine { cols })
}

fn table(total: usize, count: usize, at: usize, stride: usize, what: &str) -> Result<()> {
    primase_foundations::bytes::checked_span(at, count, stride, total)
        .map(|_| ())
        .map_err(|e| invalid(at, format!("scp: {what} table: {e}")))
}

/// Decode the node table, the world-matrix table and the section chain of one SCP. Spot chunks come out of the
/// chain; every other section is returned in `sections` and its `Unread` entry is in the second result.
pub fn decode(scp: &[u8]) -> Result<(Tables, Vec<Unread>)> {
    let v = View::new(scp);
    let u = |at: usize| -> Result<usize> { Ok(v.u32(at, Endian::Big).field("scp header")? as usize) };
    let (n_place, o_place) = (u(0x10)?, u(0x14)?);
    let (n_node, o_node) = (u(0x18)?, u(0x1c)?);
    let (n_xf, o_xf) = (u(0x38)?, u(0x3c)?);
    let blob = u(0x44)?;
    let mut tables = Tables::default();
    let mut unread = Vec::new();
    if n_node != n_xf {
        return Err(invalid(
            0x38,
            format!("scp: {n_xf} world matrices for {n_node} nodes"),
        ));
    }
    for (count, at, stride, what) in [
        (n_node, o_node, NODE, "node"),
        (n_xf, o_xf, 8, "world matrix"),
        (n_place, o_place, PLACEMENT, "placement"),
    ] {
        if count > 0 {
            table(scp.len(), count, at, stride, what)?;
        }
    }
    let link = |ptr: usize, base: usize, stride: usize, count: usize, what: &str| -> Result<u32> {
        let rel = ptr
            .checked_sub(base)
            .filter(|r| r % stride == 0 && r / stride < count);
        rel.map(|r| (r / stride) as u32)
            .ok_or_else(|| invalid(ptr, format!("scp: {what} link {ptr:#x} is not a record start")))
    };
    for i in 0..n_node {
        let at = o_node + i * NODE;
        let word = |k: usize| u(at + 4 * k);
        let (hash, matrix_at) = (u(o_xf + 8 * i)? as u32, u(o_xf + 8 * i + 4)?);
        let mut lists = [(); 2].map(|()| Vec::new());
        for (set, (counts, arrays)) in [(8, 13), (18, 23)].into_iter().enumerate() {
            for k in 0..5 {
                let (count, array) = (word(counts + k)?, word(arrays + k)?);
                if count == 0 {
                    continue;
                }
                if k != 0 {
                    return Err(invalid(at + 4 * (counts + k), "scp: node link kind other than 0"));
                }
                if array == u32::MAX as usize {
                    return Err(invalid(
                        at + 4 * (arrays + k),
                        "scp: node links counted but no array",
                    ));
                }
                table(scp.len(), count, array, 4, "node link")?;
                for j in 0..count {
                    let ptr = u(array + 4 * j)?;
                    lists[set].push(if set == 0 {
                        link(ptr, o_node, NODE, n_node, "child")?
                    } else {
                        link(ptr, o_place, PLACEMENT, n_place, "placement")?
                    });
                }
            }
        }
        let [children, placements] = lists;
        let sphere = if (0..4).all(|k| word(k).is_ok_and(|w| w == 0)) {
            None
        } else {
            let mut s = [0.; 4];
            for (k, x) in s.iter_mut().enumerate() {
                *x = v.f32(at + 4 * k, Endian::Big).field("node sphere")?;
                if !x.is_finite() {
                    return Err(invalid(at + 4 * k, "scp: nonfinite node sphere"));
                }
            }
            Some(s)
        };
        tables.nodes.push(Node {
            id: word(4)? as u32,
            matrix_hash: hash,
            world: matrix(v, matrix_at)?,
            sphere,
            children,
            placements,
        });
    }
    let by_id: std::collections::BTreeMap<u32, u32> = {
        let mut m = std::collections::BTreeMap::new();
        for (i, n) in tables.nodes.iter().enumerate() {
            m.entry(n.id).or_insert(i as u32);
        }
        m
    };
    if blob == 0 || blob == u32::MAX as usize {
        tables.chain = Chain::None;
        return Ok((tables, unread));
    }
    let mut o = blob;
    let chain = loop {
        if o + 16 == scp.len() {
            let tail = |k: usize| v.u32(o + 4 * k, Endian::Big).field("chain end");
            break if (tail(1)?, tail(2)?, tail(3)?) == (8, 0, 0) {
                Chain::Complete
            } else {
                Chain::Stopped { offset: o }
            };
        }
        let head = |k: usize| v.u32(o + 4 * k, Endian::Big).ok();
        let (Some(flags), Some(size), Some(count), Some(data)) = (head(0), head(1), head(2), head(3)) else {
            break Chain::Stopped { offset: o };
        };
        let (size, data) = (size as usize, data as usize);
        if data < 0x10 || data % 0x10 != 0 || data > size || o.checked_add(size).is_none_or(|e| e > scp.len())
        {
            break Chain::Stopped { offset: o };
        }
        let s = Section {
            offset: o,
            flags,
            size,
            count,
            data,
        };
        tables.sections.push(s);
        o += size;
    };
    for s in &tables.sections {
        if s.kind() == SECTION_SPOTS {
            let n = s.count as usize;
            let fits = n
                .checked_mul(CHUNK + CHUNK_MATRIX)
                .and_then(|b| b.checked_add(s.data))
                .is_some_and(|b| b == s.size);
            if fits && s.flags & 0xffff == 0 {
                for k in 0..n {
                    let at = s.offset + s.data + k * CHUNK;
                    let word = |w: usize| v.u32(at + w, Endian::Big).field("spot chunk");
                    let id = word(0x14)?;
                    tables.spots.push(SpotChunk {
                        section: s.offset,
                        index: k as u32,
                        name: word(0x10)?,
                        id,
                        parent: word(0x18)?,
                        class: word(0x1c)?,
                        shape: word(0x20)?,
                        pair: [word(0x24)?, word(0x28)?],
                        node: by_id.get(&id).copied(),
                        world: matrix(v, s.offset + s.data + n * CHUNK + k * CHUNK_MATRIX)?,
                    });
                }
                continue;
            }
            unread.push(Unread {
                file: "scp",
                offset: s.offset,
                bytes: s.size,
                reason: "spot section whose size is not 0x10 + count * 576 or whose flags carry low bits",
            });
        } else if let Some(kind) = SplineKind::of_section(s.flags) {
            tables
                .splines
                .extend(scp_splines::decode(scp, s, kind, &mut unread)?);
        } else if s.kind() == SECTION_CAMERAS {
            tables.cameras.extend(scp_cameras::decode(scp, s, &mut unread)?);
        } else if s.kind() == SECTION_LOCATORS {
            tables.locators.extend(scp_locators::decode(scp, s, &mut unread)?);
        } else if s.kind() == SECTION_OCCLUDERS {
            tables
                .occluders
                .extend(scp_occluders::decode(scp, s, &mut unread)?);
        } else if s.kind() == SECTION_LIGHTS {
            tables.lights.extend(scp_lights::decode(scp, s, &mut unread)?);
        } else {
            unread.push(Unread {
                file: "scp",
                offset: s.offset,
                bytes: s.size,
                reason: "blob section of a type not decoded by scp_tables (see Tables::sections for its flags)",
            });
        }
    }
    if let Chain::Stopped { offset } = chain {
        unread.push(Unread {
            file: "scp",
            offset,
            bytes: scp.len() - offset,
            reason: "section chain stops: the next header is not a section",
        });
    }
    tables.chain = chain;
    Ok((tables, unread))
}
