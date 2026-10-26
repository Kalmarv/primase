//! An Entity's skate guidance: the grindable edges the game's SWSkateGuidanceSystem component (`0x5CDCA222`) stores,
//! as chains of points in the Entity's local frame.
//!
//! The component, inline in the Entity's body, is `[pointer 0][managed][id u32][class][ResourceRef: id u32, class
//! 0x3C6C77C8, kind u8]`, then its GuidanceObjects: a u32 count, then per object three zero bytes, a pointer byte 0
//! and the GuidanceObject class (`0x344FA659`), then Valid (u8), SubType (u32, [`SUB_TYPES`]), Index0 and Index1
//! (u32: the edge's start and end in the point list), two face normals (each 3 f32 and a 4-byte word, 0 in every
//! object of level 01) and an inline SWSkateGuidanceObjectData (`[pointer 0][id u32][class 0x98A53374]`, a u32 that
//! is 0 in every object of level 01, the object linked at the edge's end, the object linked at its start (u32 each,
//! [`NO_LINK`]: none) and the edge's length, f32). The points follow the objects: a u32 count of u16 values, three
//! per point, each coordinate `(value - 32767) / 200` metres. That decoding is checked by the stored lengths: in level
//! 01 every edge's length is within 2 cm of the distance between its decoded points. An edge filter (`[pointer 0][id]
//! [class 0xDF7FF220]`, three f32, a u16, three f32 and a u8, every value finite (1/sqrt 2, pi/8, pi/4, 0, 46, 60,
//! 0.2, 0 in most of level 01's systems) and a partitioner (`[pointer 0][id][class 0x4661AAEF]`: root index u16, the leaf edge indices as a u32
//! count of u16, the bounds' minimum and maximum as 3 f32 each, and a u32 count of nodes, each three zero bytes, a
//! pointer byte 0, the PartNode class `0x5374195C`, a u32 type, two u16 and an f32) follow the points: a spatial index
//! of the edges, read to find where the system ends and not kept.
//!
//! The objects' links say which edges continue each other: an object's end link joins an edge that has the object's
//! end point as one of its two points, its start link one that has its start point (an edge may run either way).
//! [`GuidanceSystem::chains`] follows them into polylines; a link to an edge that holds neither point is refused.
//!
//! The shaping mechanic's three further classes (SWSkateFreeFormGuidanceSystem `0x359BE54E`,
//! SWSkateScaleGuidanceSystem `0x6A1CFF2F`, SWSkateMorphGuidanceSystem `0x081AF61A`) have the same layout up to the
//! partitioner. In level 01 the free-form system (95, one 5 m edge each, the `ShP_FFGrind5m` rails) has one u32
//! after it, the id of an object in the shared cells (class `0xB5403F20`, a single 0.25 m edge: `0x901687FC` in all of
//! level 01's systems; levels 02 to 05 name two more ids, `0x766A4B87` and `0x7CD6EC02`, of objects with the same
//! content), kept as [`Shaping::Template`]; the scale system (25, the `InfGP_Kicker` ramps and a few benches) nothing; the
//! morph system (5, `01_B_Inf_WaveFlat` and the `Q03_SQ03_Park_Object` shapes) a u32 count of states, each `[id]
//! [class 0xCB13AC37]`, a GuidanceObject array and a point array (a u32 count, three f32 each) of the same length as
//! the system's own. The system's own objects and points are the first state's (the same edges, points within one
//! 16-bit step), the further states are the shape's morph targets: in `01_B_Inf_WaveFlat` each of the 220 points lies
//! on one of the Mesh's 226 vertices and state 1's lie on its second vertex stream's, to 1e-5. A morph system's links
//! name nothing in its own object list (3 of its 5 systems hold links past it or to an edge that does not hold the
//! point), so its edges chain by the points they share ([`GuidanceSystem::chains`]).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::collections::BTreeMap;

use primase_bytes::{Cursor, Endian, Field, FieldError};

use super::scene::RESOURCE_REF;

const LE: Endian = Endian::Little;
pub const SW_GUIDANCE_SYSTEM: u32 = 0x5CDC_A222;
pub const GUIDANCE_OBJECT: u32 = 0x344F_A659;
pub const SW_GUIDANCE_OBJECT_DATA: u32 = 0x98A5_3374;
pub const FREE_FORM_GUIDANCE_SYSTEM: u32 = 0x359B_E54E;
pub const SCALE_GUIDANCE_SYSTEM: u32 = 0x6A1C_FF2F;
pub const MORPH_GUIDANCE_SYSTEM: u32 = 0x081A_F61A;
/// The edge filter and partitioner after the points, a partitioner's nodes, and a morph system's states.
pub const EDGE_FILTER: u32 = 0xDF7F_F220;
pub const PARTITIONER: u32 = 0x4661_AAEF;
pub const PART_NODE: u32 = 0x5374_195C;
pub const MORPH_TARGET_DATA: u32 = 0xCB13_AC37;
/// A link that joins nothing.
pub const NO_LINK: u32 = 0x1FFF;
/// GuidanceObjectSubType's names, by value: the enum's names stand in the executable last value first
/// (`GuidanceObjectSubTypeCurvedVert` ... `GuidanceObjectSubTypeNone`), as the engine's enum tables are written.
/// Level 01 holds 0 (`none`) and 9 (`curved_vert`: the lips of its vert ramps and quarter pipes).
pub const SUB_TYPES: [&str; 10] = [
    "none",
    "ledge_grab",
    "beam",
    "ladder",
    "pole",
    "rope",
    "surface",
    "quadruped",
    "kiosk",
    "curved_vert",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("guidance {field}: {why}")]
    Invalid { field: &'static str, why: String },
}

fn invalid(field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        why: why.into(),
    }
}

fn word(c: &mut Cursor<'_>, field: &'static str) -> Result<u32, Error> {
    Ok(c.u32(LE).field(field)?)
}

fn number(c: &mut Cursor<'_>, field: &'static str) -> Result<f32, Error> {
    let v = c.f32(LE).field(field)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(invalid(field, "a non-finite float"))
    }
}

fn expect(c: &mut Cursor<'_>, field: &'static str, want: &[u8]) -> Result<(), Error> {
    let got = c.take(want.len()).field(field)?;
    if got == want {
        Ok(())
    } else {
        Err(invalid(field, format!("{got:02x?}, not {want:02x?}")))
    }
}

/// One GuidanceObject: an edge between two points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub valid: u8,
    pub sub_type: u32,
    /// Index0 and Index1: its start and end in [`GuidanceSystem::points`].
    pub points: [u32; 2],
    /// The two face normals, local frame.
    pub normals: [[f32; 3]; 2],
    /// The objects linked at its start and at its end ([`NO_LINK`]: none).
    pub links: [u32; 2],
    /// Its stored length, metres.
    pub length: f32,
}

pub use crate::components::GuidanceClass as Class;

impl Class {
    /// The class hash of the component.
    pub const fn hash(self) -> u32 {
        match self {
            Class::Plain => SW_GUIDANCE_SYSTEM,
            Class::FreeForm => FREE_FORM_GUIDANCE_SYSTEM,
            Class::Scale => SCALE_GUIDANCE_SYSTEM,
            Class::Morph => MORPH_GUIDANCE_SYSTEM,
        }
    }

    pub fn of(hash: u32) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.hash() == hash)
    }
}

/// One state of a morph system: the edges and points of the shape in one of its morph targets (the first is the
/// system's own, the shape at rest). Its points are as many as the system's, one per point of the system.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub id: u32,
    pub edges: Vec<Edge>,
    /// Local frame, metres.
    pub points: Vec<[f32; 3]>,
}

/// What follows a system's partitioner (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub enum Shaping {
    /// A plain or scale system: nothing.
    None,
    /// A free-form system: the id of the shared object it names.
    Template(u32),
    /// A morph system: its states.
    States(Vec<State>),
}

/// A guidance system as read (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct GuidanceSystem {
    pub class: Class,
    pub edges: Vec<Edge>,
    /// Local frame, metres.
    pub points: Vec<[f32; 3]>,
    pub shaping: Shaping,
    /// Where the component ends in the body: the next component's first byte.
    pub end: usize,
}

/// A run of linked edges: the points in order, the edges between them, and whether the last point joins the first.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub points: Vec<u32>,
    pub edges: Vec<usize>,
    pub closed: bool,
}

/// A GuidanceObject array: a u32 count, then each object (the module documentation).
fn read_edges(c: &mut Cursor<'_>) -> Result<Vec<Edge>, Error> {
    let count = word(c, "GuidanceObjects count")?;
    let mut edges = Vec::new();
    for _ in 0..count {
        expect(c, "GuidanceObject header", &[0, 0, 0, 0])?;
        expect(c, "GuidanceObject class", &GUIDANCE_OBJECT.to_le_bytes())?;
        let valid = c.u8().field("GuidanceObject.Valid")?;
        let sub_type = word(c, "GuidanceObject.SubType")?;
        let points = [
            word(c, "GuidanceObject.Index0")?,
            word(c, "GuidanceObject.Index1")?,
        ];
        let mut normals = [[0f32; 3]; 2];
        for n in &mut normals {
            for v in n.iter_mut() {
                *v = number(c, "GuidanceObject normal")?;
            }
            word(c, "GuidanceObject normal word")?;
        }
        expect(c, "guidance object data pointer", &[0])?;
        word(c, "guidance object data id")?;
        expect(
            c,
            "guidance object data class",
            &SW_GUIDANCE_OBJECT_DATA.to_le_bytes(),
        )?;
        word(c, "guidance object data word")?;
        let end = word(c, "guidance object data end link")?;
        let start = word(c, "guidance object data start link")?;
        let length = number(c, "guidance object data length")?;
        edges.push(Edge {
            valid,
            sub_type,
            points,
            normals,
            links: [start, end],
            length,
        });
    }
    Ok(edges)
}

/// The edge filter and the partitioner, read and dropped; the largest leaf edge index the partitioner names.
fn read_index(c: &mut Cursor<'_>) -> Result<Option<u16>, Error> {
    expect(c, "edge filter pointer", &[0])?;
    word(c, "edge filter id")?;
    expect(c, "edge filter class", &EDGE_FILTER.to_le_bytes())?;
    for _ in 0..3 {
        number(c, "edge filter value")?;
    }
    c.u16(LE).field("edge filter word")?;
    for _ in 0..3 {
        number(c, "edge filter value")?;
    }
    c.u8().field("edge filter flag")?;
    expect(c, "partitioner pointer", &[0])?;
    word(c, "partitioner id")?;
    expect(c, "partitioner class", &PARTITIONER.to_le_bytes())?;
    c.u16(LE).field("partitioner root")?;
    let leaves = word(c, "partitioner leaf count")?;
    let mut last = None;
    for _ in 0..leaves {
        let leaf = c.u16(LE).field("partitioner leaf")?;
        last = last.max(Some(leaf));
    }
    for _ in 0..6 {
        number(c, "partitioner bounds")?;
    }
    let nodes = word(c, "partitioner node count")?;
    for _ in 0..nodes {
        expect(c, "PartNode header", &[0, 0, 0, 0])?;
        expect(c, "PartNode class", &PART_NODE.to_le_bytes())?;
        word(c, "PartNode type")?;
        c.u16(LE).field("PartNode index")?;
        c.u16(LE).field("PartNode index")?;
        number(c, "PartNode middle")?;
    }
    Ok(last)
}

/// A morph system's states: a u32 count, then per state its id, the TargetData class, its edges, and `points` points
/// (a u32 count, which must be the system's own, then three f32 each).
fn read_states(c: &mut Cursor<'_>, points: usize) -> Result<Vec<State>, Error> {
    let count = word(c, "morph state count")?;
    let mut states = Vec::new();
    for _ in 0..count {
        let id = word(c, "morph state id")?;
        expect(c, "morph state class", &MORPH_TARGET_DATA.to_le_bytes())?;
        let edges = read_edges(c)?;
        let n = word(c, "morph state point count")?;
        if usize::try_from(n).ok() != Some(points) {
            return Err(invalid(
                "morph state points",
                format!("{n} points, the system has {points}"),
            ));
        }
        let mut list = Vec::new();
        for _ in 0..n {
            let mut p = [0f32; 3];
            for v in &mut p {
                *v = number(c, "morph state point")?;
            }
            list.push(p);
        }
        states.push(State {
            id,
            edges,
            points: list,
        });
    }
    Ok(states)
}

/// Every index of `edges` in range of `points`; where `links`, every link to an edge that holds the point it joins at.
fn check_edges(edges: &[Edge], points: usize, links: bool) -> Result<(), Error> {
    for (i, e) in edges.iter().enumerate() {
        if let Some(p) = e
            .points
            .iter()
            .find(|&&p| usize::try_from(p).map_or(true, |p| p >= points))
        {
            return Err(invalid("Index", format!("edge {i}: point {p} of {points}")));
        }
        if !links {
            continue;
        }
        for (side, &link) in e.links.iter().enumerate() {
            if link == NO_LINK {
                continue;
            }
            let at = e.points.get(side).copied().unwrap_or_default();
            let other = usize::try_from(link).ok().and_then(|l| edges.get(l));
            match other {
                Some(o) if o.points.contains(&at) => {}
                Some(_) => {
                    return Err(invalid(
                        "link",
                        format!("edge {i}: edge {link} does not hold point {at}"),
                    ));
                }
                None => {
                    return Err(invalid(
                        "link",
                        format!("edge {i}: edge {link} of {}", edges.len()),
                    ));
                }
            }
        }
    }
    Ok(())
}

impl GuidanceSystem {
    /// The component whose class starts at `body[at..]` (the class, then its ResourceRef).
    pub fn parse(body: &[u8], at: usize) -> Result<GuidanceSystem, Error> {
        let mut c = Cursor::new(body);
        c.seek(at).field("guidance system")?;
        let hash = word(&mut c, "class")?;
        let class =
            Class::of(hash).ok_or_else(|| invalid("class", format!("{hash:#010x} is no guidance class")))?;
        word(&mut c, "ResourceRef id")?;
        expect(&mut c, "ResourceRef class", &RESOURCE_REF.to_le_bytes())?;
        c.u8().field("ResourceRef kind")?;
        let edges = read_edges(&mut c)?;
        let values = word(&mut c, "point value count")?;
        if values % 3 != 0 {
            return Err(invalid(
                "points",
                format!("{values} u16 values, not three per point"),
            ));
        }
        let mut points = Vec::new();
        for _ in 0..values / 3 {
            let mut p = [0f32; 3];
            for v in &mut p {
                let q = c.u16(LE).field("point coordinate")?;
                *v = (f32::from(q) - 32767.) / 200.;
            }
            points.push(p);
        }
        let leaf = read_index(&mut c)?;
        let shaping = match class {
            Class::Plain | Class::Scale => Shaping::None,
            Class::FreeForm => Shaping::Template(word(&mut c, "free-form template")?),
            Class::Morph => Shaping::States(read_states(&mut c, points.len())?),
        };
        let system = GuidanceSystem {
            class,
            edges,
            points,
            shaping,
            end: c.position(),
        };
        system.check(leaf)?;
        Ok(system)
    }

    /// Whether another component follows the system in `body`: its header, `[pointer type 4 (inline, managed)][0]` or
    /// `[pointer type 0 (inline)][managed byte 1]`, is at [`Self::end`]. Every system of level 01 is followed by one.
    pub fn ends_at_component(&self, body: &[u8]) -> bool {
        matches!(
            body.get(self.end..self.end.saturating_add(2)),
            Some([4, 0] | [0, 1])
        )
    }

    /// The edge lists a system holds: its own and each state's.
    fn edge_lists(&self) -> impl Iterator<Item = &[Edge]> {
        let states = match &self.shaping {
            Shaping::States(s) => s.as_slice(),
            _ => &[],
        };
        std::iter::once(self.edges.as_slice()).chain(states.iter().map(|s| s.edges.as_slice()))
    }

    /// Every index in range; every link, but a morph system's, to an edge that holds the point it joins at; the
    /// partitioner's leaves among the edges of the system or of one of its states.
    fn check(&self, leaf: Option<u16>) -> Result<(), Error> {
        let n = self.points.len();
        for edges in self.edge_lists() {
            check_edges(edges, n, self.class != Class::Morph)?;
        }
        let most = self.edge_lists().map(<[Edge]>::len).max().unwrap_or(0);
        if let Some(l) = leaf
            && usize::from(l) >= most
        {
            return Err(invalid("partitioner leaf", format!("edge {l} of {most}")));
        }
        Ok(())
    }

    /// The edges as chains. A morph system's edges chain by the points they share: a chain starts at a point one edge
    /// has, walks through points two edges share, and ends at a point that one edge or three or more have. The
    /// others follow the objects' links: a chain starts at an edge with a side linked to no edge of its sub-type, at
    /// that side's point, and follows the links through edges of the same sub-type until a side links to none, or to
    /// an edge already taken (a junction: the chain ends there, and the edge joins another chain). In both, edges left
    /// over form loops; each starts at its lowest edge and is closed when it comes back to its first point.
    pub fn chains(&self) -> Vec<Chain> {
        if self.class == Class::Morph {
            return self.point_chains();
        }
        let n = self.edges.len();
        let mut taken = vec![false; n];
        let mut out = Vec::new();
        let open = (0..n).filter_map(|i| (0..2).find(|&s| self.linked(i, s).is_none()).map(|s| (i, s)));
        let loops = (0..n).map(|i| (i, 0));
        let starts: Vec<(usize, usize)> = open.chain(loops).collect();
        for (start, side) in starts {
            let Some(first) = self.edges.get(start) else {
                continue;
            };
            if taken.get(start).copied().unwrap_or(true) {
                continue;
            }
            let (from, mut point) = if side == 0 {
                (first.points[0], first.points[1])
            } else {
                (first.points[1], first.points[0])
            };
            let mut chain = Chain {
                points: vec![from, point],
                edges: vec![start],
                closed: false,
            };
            if let Some(t) = taken.get_mut(start) {
                *t = true;
            }
            let mut at = start;
            while let Some((to, far)) = self.step(at, point, &taken) {
                if let Some(t) = taken.get_mut(to) {
                    *t = true;
                }
                chain.edges.push(to);
                at = to;
                point = far;
                if chain.points.first() == Some(&far) {
                    chain.closed = true;
                    break;
                }
                chain.points.push(far);
            }
            out.push(chain);
        }
        out
    }

    /// The edge linked to edge `from` at its `side` (0 start, 1 end), when it has the same sub-type.
    fn linked(&self, from: usize, side: usize) -> Option<usize> {
        let e = self.edges.get(from)?;
        let to = usize::try_from(*e.links.get(side)?).ok()?;
        (self.edges.get(to)?.sub_type == e.sub_type).then_some(to)
    }

    /// The edge not yet taken that edge `at` links to at its point `point`, and that edge's other point.
    fn step(&self, at: usize, point: u32, taken: &[bool]) -> Option<(usize, u32)> {
        let s = self.edges.get(at)?.points.iter().position(|&p| p == point)?;
        let to = self.linked(at, s)?;
        if taken.get(to).copied().unwrap_or(true) {
            return None;
        }
        let o = self.edges.get(to)?;
        let far = if o.points[0] == point {
            o.points[1]
        } else {
            o.points[0]
        };
        Some((to, far))
    }

    /// [`Self::chains`] by shared points.
    fn point_chains(&self) -> Vec<Chain> {
        let mut at: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        for (i, e) in self.edges.iter().enumerate() {
            for p in e.points {
                at.entry(p).or_default().push(i);
            }
        }
        let mut taken = vec![false; self.edges.len()];
        let mut out = Vec::new();
        let ends = at
            .iter()
            .filter_map(|(&p, v)| (v.len() == 1).then(|| v.first().map(|&e| (e, p))).flatten());
        let loops = self.edges.iter().enumerate().map(|(i, e)| (i, e.points[0]));
        let starts: Vec<(usize, u32)> = ends.chain(loops).collect();
        for (start, from) in starts {
            if taken.get(start).copied().unwrap_or(true) {
                continue;
            }
            let mut chain = Chain {
                points: vec![from],
                edges: Vec::new(),
                closed: false,
            };
            let (mut edge, mut point) = (start, from);
            loop {
                if let Some(t) = taken.get_mut(edge) {
                    *t = true;
                }
                let Some(e) = self.edges.get(edge) else { break };
                let far = if e.points[0] == point {
                    e.points[1]
                } else {
                    e.points[0]
                };
                chain.edges.push(edge);
                chain.points.push(far);
                if far == from {
                    chain.closed = true;
                    chain.points.pop();
                    break;
                }
                let here = at.get(&far).map(Vec::as_slice).unwrap_or_default();
                let next: Vec<usize> = here
                    .iter()
                    .copied()
                    .filter(|&j| !taken.get(j).copied().unwrap_or(true))
                    .collect();
                match (here.len(), next.as_slice()) {
                    (2, &[j]) => (edge, point) = (j, far),
                    _ => break,
                }
            }
            out.push(chain);
        }
        out
    }
}

/// The guidance components in an Entity's body: (class, position of the class) of each component of a guidance class,
/// found by its signature (the class, an id, the ResourceRef class), in order.
pub fn classes(body: &[u8]) -> Vec<(u32, usize)> {
    let rref = RESOURCE_REF.to_le_bytes();
    let mut out = Vec::new();
    for class in [
        SW_GUIDANCE_SYSTEM,
        FREE_FORM_GUIDANCE_SYSTEM,
        SCALE_GUIDANCE_SYSTEM,
        MORPH_GUIDANCE_SYSTEM,
    ] {
        let tag = class.to_le_bytes();
        let mut at = 0usize;
        while let Some(p) = body
            .get(at..)
            .and_then(|rest| rest.windows(4).position(|w| w == tag))
        {
            let p = at.saturating_add(p);
            if body.get(p.saturating_add(8)..p.saturating_add(12)) == Some(&rref[..]) {
                out.push((class, p));
            }
            at = p.saturating_add(4);
        }
    }
    out.sort_by_key(|&(_, p)| p);
    out
}
