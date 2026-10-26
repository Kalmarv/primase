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
//! 01 every edge's length is within 2 cm of the distance between its decoded points. What follows the points (an
//! edge filter, a partitioner) is not read.
//!
//! The objects' links say which edges continue each other: an object's end link joins an edge that has the object's
//! end point as one of its two points, its start link one that has its start point (an edge may run either way).
//! [`GuidanceSystem::chains`] follows them into polylines; a link to an edge that holds neither point is refused.
//!
//! Other guidance classes this game stores (SWSkateFreeFormGuidanceSystem, SWSkateScaleGuidanceSystem,
//! SWSkateMorphGuidanceSystem: the shaping mechanic's) are not read; [`classes`] names them so the reader counts them.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_bytes::{Cursor, Endian, Field, FieldError};

use super::scene::RESOURCE_REF;

const LE: Endian = Endian::Little;
pub const SW_GUIDANCE_SYSTEM: u32 = 0x5CDC_A222;
pub const GUIDANCE_OBJECT: u32 = 0x344F_A659;
pub const SW_GUIDANCE_OBJECT_DATA: u32 = 0x98A5_3374;
pub const FREE_FORM_GUIDANCE_SYSTEM: u32 = 0x359B_E54E;
pub const SCALE_GUIDANCE_SYSTEM: u32 = 0x6A1C_FF2F;
pub const MORPH_GUIDANCE_SYSTEM: u32 = 0x081A_F61A;
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

/// A SWSkateGuidanceSystem as read (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct GuidanceSystem {
    pub edges: Vec<Edge>,
    /// Local frame, metres.
    pub points: Vec<[f32; 3]>,
}

/// A run of linked edges: the points in order, the edges between them, and whether the last point joins the first.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub points: Vec<u32>,
    pub edges: Vec<usize>,
    pub closed: bool,
}

impl GuidanceSystem {
    /// The component whose class starts at `body[at..]` (the class, then its ResourceRef).
    pub fn parse(body: &[u8], at: usize) -> Result<GuidanceSystem, Error> {
        let mut c = Cursor::new(body);
        c.seek(at).field("guidance system")?;
        expect(&mut c, "class", &SW_GUIDANCE_SYSTEM.to_le_bytes())?;
        word(&mut c, "ResourceRef id")?;
        expect(&mut c, "ResourceRef class", &RESOURCE_REF.to_le_bytes())?;
        c.u8().field("ResourceRef kind")?;
        let count = word(&mut c, "GuidanceObjects count")?;
        let mut edges = Vec::new();
        for _ in 0..count {
            expect(&mut c, "GuidanceObject header", &[0, 0, 0, 0])?;
            expect(&mut c, "GuidanceObject class", &GUIDANCE_OBJECT.to_le_bytes())?;
            let valid = c.u8().field("GuidanceObject.Valid")?;
            let sub_type = word(&mut c, "GuidanceObject.SubType")?;
            let points = [
                word(&mut c, "GuidanceObject.Index0")?,
                word(&mut c, "GuidanceObject.Index1")?,
            ];
            let mut normals = [[0f32; 3]; 2];
            for n in &mut normals {
                for v in n.iter_mut() {
                    *v = number(&mut c, "GuidanceObject normal")?;
                }
                word(&mut c, "GuidanceObject normal word")?;
            }
            expect(&mut c, "guidance object data pointer", &[0])?;
            word(&mut c, "guidance object data id")?;
            expect(
                &mut c,
                "guidance object data class",
                &SW_GUIDANCE_OBJECT_DATA.to_le_bytes(),
            )?;
            word(&mut c, "guidance object data word")?;
            let end = word(&mut c, "guidance object data end link")?;
            let start = word(&mut c, "guidance object data start link")?;
            let length = number(&mut c, "guidance object data length")?;
            edges.push(Edge {
                valid,
                sub_type,
                points,
                normals,
                links: [start, end],
                length,
            });
        }
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
        let system = GuidanceSystem { edges, points };
        system.check()?;
        Ok(system)
    }

    /// Every index in range, every link to an edge that holds the point it joins at.
    fn check(&self) -> Result<(), Error> {
        let n = self.points.len();
        for (i, e) in self.edges.iter().enumerate() {
            if let Some(p) = e
                .points
                .iter()
                .find(|&&p| usize::try_from(p).map_or(true, |p| p >= n))
            {
                return Err(invalid("Index", format!("edge {i}: point {p} of {n}")));
            }
            for (side, &link) in e.links.iter().enumerate() {
                if link == NO_LINK {
                    continue;
                }
                let at = e.points.get(side).copied().unwrap_or_default();
                let other = usize::try_from(link).ok().and_then(|l| self.edges.get(l));
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
                            format!("edge {i}: edge {link} of {}", self.edges.len()),
                        ));
                    }
                }
            }
        }
        Ok(())
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

    /// The edges as chains. A chain starts at an edge with a side linked to no edge of its sub-type, at that side's
    /// point, and follows the links through edges of the same sub-type until a side links to none, or to an edge
    /// already taken (a junction: the chain ends there, and the edge joins another chain). Edges left over form loops;
    /// each starts at its lowest edge and is closed when it comes back to its first point.
    pub fn chains(&self) -> Vec<Chain> {
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
