//! The game's own files derived from a level part's glTF, as its pipeline wrote them: the collision model
//! (`Level_<L>_<N>Collision.xnb`, `SE.Pipeline.CollisionModelReader`: the processor's triangles, grippable edges, ramps
//! and grind ledges) and the shortcuts (`Level_<L>_<N>Cuts.xnb`, a reflective `SE.Pipeline.ShortcutContent`). Read as
//! stored, in the game's units and frame; [`crate::read::level`] places them.
//!
//! The collision model, after the object's reader index (every number little-endian, a vector three f32):
//! the solids' and the triggers' grid cell sizes (vectors); the mesh count (i32, one per glTF node); per mesh a byte
//! (1: present), then its aggregate (byte: 0 solid, 1 trigger, 2 none), its part count (i32) and per part the
//! triangle count (i32), each triangle three corners (vectors) and, for a solid mesh, its grippable-edge bits (byte)
//! and ramp id (i32, -1 none), then the part's grid cells (i32 count; each cell three i32, a count and that many
//! triangle indices: the game's lookup, derived from the triangles, checked and not kept); then the ledges: the corner
//! count (i32), each corner's position, each corner's near corners (i32 count, indices), the ledge count (i32), each
//! ledge (min and max angle f32, normal, tangent, two corner indices, length, the recursive lengths in each direction,
//! group id i32, two lerp cut-offs f32, type byte: 0 edge, 1 rail without boost, 2 rail with boost), then each
//! corner's ledge directions (i32 count, each a ledge index and a bool, forward). The binormal is not stored: the game
//! derives it as normalize(cross(tangent, normal)), as [`binormal`] does.

use std::collections::BTreeMap;

use monogame_formats::xnb::{Content, Xnb};

use crate::{Error, Result};

fn bad(what: impl std::fmt::Display) -> Error {
    Error::Format(format!("collision model: {what}"))
}

/// A collision mesh as stored: its aggregate (0 solid, 1 trigger, 2 none) and its parts.
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionMesh {
    pub aggregate: u8,
    pub parts: Vec<CollisionPart>,
}

/// A collision mesh part: its triangles (corners in the game's units), and for a solid mesh each triangle's
/// grippable-edge bits and ramp id (-1: none); empty for a trigger.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionPart {
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub grippable: Vec<u8>,
    pub ramps: Vec<i32>,
}

/// A ledge corner: its position, the corners near it (indices) and the ledges leaving it (ledge index, forward).
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    pub position: [f32; 3],
    pub near: Vec<u32>,
    pub ledges: Vec<(u32, bool)>,
}

/// A grind ledge as stored (the binormal derived as the game derives it).
#[derive(Clone, Debug, PartialEq)]
pub struct Ledge {
    /// 0 edge, 1 rail without boost, 2 rail with boost.
    pub kind: u8,
    pub corners: [u32; 2],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub binormal: [f32; 3],
    pub min_angle: f32,
    pub max_angle: f32,
    pub length: f32,
    pub length_recursive: [f32; 2],
    pub group: i32,
    pub lerp_cut_off: [f32; 2],
}

/// A part's collision model as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionModel {
    pub solids_cell: [f32; 3],
    pub triggers_cell: [f32; 3],
    /// Per glTF node, its collision mesh (`None`: the node has none).
    pub meshes: Vec<Option<CollisionMesh>>,
    pub corners: Vec<Corner>,
    pub ledges: Vec<Ledge>,
}

/// normalize(cross(tangent, normal)) in f32, as the game's math library computes it (the reciprocal of the length,
/// multiplied).
pub fn binormal(tangent: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    let [ax, ay, az] = tangent;
    let [bx, by, bz] = normal;
    let c = [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx];
    let f = 1.0 / (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    [c[0] * f, c[1] * f, c[2] * f]
}

fn vector(c: &mut Content<'_>) -> Result<[f32; 3]> {
    Ok([c.f32()?, c.f32()?, c.f32()?])
}

fn count(c: &mut Content<'_>, what: &str) -> Result<usize> {
    let at = c.position();
    let n = c.i32()?;
    usize::try_from(n).map_err(|_| bad(format!("{what} count {n} at {at:#x}")))
}

fn index(c: &mut Content<'_>, below: usize, what: &str) -> Result<u32> {
    let at = c.position();
    let i = c.i32()?;
    match usize::try_from(i) {
        Ok(u) if u < below => Ok(u as u32),
        _ => Err(bad(format!("{what} {i} at {at:#x}, not below {below}"))),
    }
}

/// A collision model file's object, read to its end.
pub fn collision_model(bytes: &[u8]) -> Result<CollisionModel> {
    let xnb = Xnb::parse(bytes)?;
    let (r, mut c) = xnb.primary()?;
    if r.type_name() != "SE.Pipeline.CollisionModelReader" {
        return Err(bad(format!(
            "reader {}, not SE.Pipeline.CollisionModelReader",
            r.type_name()
        )));
    }
    let solids_cell = vector(&mut c)?;
    let triggers_cell = vector(&mut c)?;
    let mesh_count = count(&mut c, "mesh")?;
    let mut meshes = Vec::new();
    for _ in 0..mesh_count {
        match c.u8()? {
            0 => {
                meshes.push(None);
                continue;
            }
            1 => {}
            present => return Err(bad(format!("mesh presence {present} at {:#x}", c.position() - 1))),
        }
        let aggregate = c.u8()?;
        if aggregate > 2 {
            return Err(bad(format!("aggregate {aggregate} at {:#x}", c.position() - 1)));
        }
        let part_count = count(&mut c, "part")?;
        let mut parts = Vec::with_capacity(part_count.min(1 << 10));
        for _ in 0..part_count {
            let n = count(&mut c, "triangle")?;
            let mut part = CollisionPart::default();
            for _ in 0..n {
                part.triangles
                    .push([vector(&mut c)?, vector(&mut c)?, vector(&mut c)?]);
                if aggregate == 0 {
                    part.grippable.push(c.u8()?);
                    part.ramps.push(c.i32()?);
                }
            }
            // the lookup grid: every index a triangle of the part
            for _ in 0..count(&mut c, "grid cell")? {
                c.bytes(12)?;
                for _ in 0..count(&mut c, "cell triangle")? {
                    index(&mut c, n, "cell triangle")?;
                }
            }
            parts.push(part);
        }
        meshes.push(Some(CollisionMesh { aggregate, parts }));
    }
    let corner_count = count(&mut c, "ledge corner")?;
    let mut corners = Vec::new();
    for _ in 0..corner_count {
        corners.push(Corner {
            position: vector(&mut c)?,
            near: Vec::new(),
            ledges: Vec::new(),
        });
    }
    for i in 0..corner_count {
        let n = count(&mut c, "near corner")?;
        let near = (0..n)
            .map(|_| index(&mut c, corner_count, "near corner"))
            .collect::<Result<_>>()?;
        if let Some(corner) = corners.get_mut(i) {
            corner.near = near;
        }
    }
    let ledge_count = count(&mut c, "ledge")?;
    let mut ledges = Vec::new();
    for _ in 0..ledge_count {
        let (min_angle, max_angle) = (c.f32()?, c.f32()?);
        let (normal, tangent) = (vector(&mut c)?, vector(&mut c)?);
        let corners = [
            index(&mut c, corner_count, "ledge corner")?,
            index(&mut c, corner_count, "ledge corner")?,
        ];
        let length = c.f32()?;
        let length_recursive = [c.f32()?, c.f32()?];
        let group = c.i32()?;
        let lerp_cut_off = [c.f32()?, c.f32()?];
        let kind = c.u8()?;
        if kind > 2 {
            return Err(bad(format!("ledge type {kind} at {:#x}", c.position() - 1)));
        }
        ledges.push(Ledge {
            kind,
            corners,
            normal,
            tangent,
            binormal: binormal(tangent, normal),
            min_angle,
            max_angle,
            length,
            length_recursive,
            group,
            lerp_cut_off,
        });
    }
    for i in 0..corner_count {
        let n = count(&mut c, "corner ledge")?;
        let mut dirs = Vec::with_capacity(n.min(1 << 10));
        for _ in 0..n {
            let ledge = index(&mut c, ledge_count, "corner ledge")?;
            let forward = match c.u8()? {
                0 => false,
                1 => true,
                b => return Err(bad(format!("a bool byte {b} at {:#x}", c.position() - 1))),
            };
            dirs.push((ledge, forward));
        }
        if let Some(corner) = corners.get_mut(i) {
            corner.ledges = dirs;
        }
    }
    c.end()?;
    Ok(CollisionModel {
        solids_cell,
        triggers_cell,
        meshes,
        corners,
        ledges,
    })
}

/// A shortcuts file's `ShortcutsPerLevel` as stored: per key (a level's id), its integers (`None`: a null array). The
/// object is a reflective `SE.Pipeline.ShortcutContent` whose one property is a `Dictionary<uint, int[]>`: its reader
/// index, the entry count (i32), then per entry the key (u32) and the value (reader index; i32 count and the
/// integers).
pub fn shortcuts(bytes: &[u8]) -> Result<BTreeMap<u32, Option<Vec<i32>>>> {
    let xnb = Xnb::parse(bytes)?;
    let reader = |i: u32| (i as usize).checked_sub(1).and_then(|i| xnb.readers.get(i));
    let (r, mut c) = xnb.primary()?;
    let shortcut_bad = |what: String| Error::Format(format!("shortcuts: {what}"));
    if r.base_name() != "Microsoft.Xna.Framework.Content.ReflectiveReader`1"
        || r.generic_arguments() != ["SE.Pipeline.ShortcutContent"]
    {
        return Err(shortcut_bad(format!("reader {}", r.type_name())));
    }
    let mut out = BTreeMap::new();
    let dictionary = c.seven_bit()?;
    if dictionary == 0 {
        c.end()?;
        return Ok(out);
    }
    match reader(dictionary) {
        Some(d)
            if d.base_name() == "Microsoft.Xna.Framework.Content.DictionaryReader`2"
                && d.generic_arguments() == ["System.UInt32", "System.Int32[]"] => {}
        other => {
            return Err(shortcut_bad(format!(
                "property reader {:?}, not a Dictionary<uint, int[]>",
                other.map(|d| d.type_name())
            )));
        }
    }
    for _ in 0..count(&mut c, "shortcut entry")? {
        let key = c.u32()?;
        let array = c.seven_bit()?;
        let value = if array == 0 {
            None
        } else {
            match reader(array) {
                Some(a)
                    if a.base_name() == "Microsoft.Xna.Framework.Content.ArrayReader`1"
                        && a.generic_arguments() == ["System.Int32"] => {}
                other => {
                    return Err(shortcut_bad(format!(
                        "value reader {:?}, not an int array",
                        other.map(|a| a.type_name())
                    )));
                }
            }
            let n = count(&mut c, "shortcut")?;
            Some(
                (0..n)
                    .map(|_| c.i32())
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            )
        };
        out.insert(key, value);
    }
    c.end()?;
    Ok(out)
}
