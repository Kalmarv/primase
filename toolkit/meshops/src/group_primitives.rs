//! `group-primitives{max-groups=N,max-vertices=V}` (both required): every mesh's primitives joined into at most `N`
//! primitives of at most `V` vertices each, for a destination that draws a mesh in a few sections of limited size.
//!
//! The rule, per mesh:
//! 1. Primitives drawn with the same material (or with none) are joined, the vertices of the later ones after those of
//!    the earlier, indices offset: one group a material, however many primitives it had.
//! 2. A group over `V` vertices is split by whole triangles, in their order, into parts that each use at most `V`
//!    vertices (a part holds only the vertices its triangles use, in first-use order; a group within `V` is kept
//!    whole, unused vertices included).
//! 3. More than `N` primitives left is refused. With several materials that is because primitives of different
//!    materials cannot be joined without drawing one with the other's: the refusal names `pack-texture-atlas`, which
//!    gives them one. With one material the limits cannot be met (its vertices fill more than `N` parts of `V`).
//!    Joining the smallest groups with each other, once the groups are the materials, has nothing left to join: two
//!    groups of one material are already one, and a split's parts are filled to within two vertices of `V` (except
//!    the last), so no two of them fit in `V`.
//!
//! The result keeps the draw order of the first primitive each one holds. A primitive that is joined to nothing and
//! needs no split is kept exactly, and a mesh in which none changes is not touched. Primitives are joined only when
//! they carry the same streams (same semantics and types, refused naming the stream otherwise), and triangle lists
//! are the only topology (refused otherwise). Joined skinned primitives are given one palette: its slots are the
//! distinct (per binding: joint and inverse bind) slots of their palettes, in the order the palettes list them, the first
//! primitive's first, and every joint index they hold
//! is rewritten to it, so primitives whose palettes differ only in order are joined exactly; primitives bound
//! differently (per-vertex influences against one joint for the whole primitive, a different number of influence sets,
//! one unskinned) or wholly to different joints are refused. A joined primitive's other per-primitive components
//! (provenance, material variants) follow the primitive it is counted as coming from (the one contributing most
//! triangles, the earlier on a tie); a joined primitive with material variants is refused, and so is any mesh that
//! needs a join or a split while a component holds data per vertex (morph targets), since those follow one source
//! primitive's vertices.
//!
//! A mesh it changes is no longer its file's: the file link goes. Reported: meshes regrouped, primitives made, joined
//! by material, and parts split off. The primitives joined into others are declared as left
//! out of the mesh (their triangles are kept, in the primitive that holds them).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::ledger::Measure;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Component, Entity, Influences, MaterialVariants, Mesh, MeshFile, Primitive, Semantic, Skin, Slot, Stream,
    Topology, World,
};

use crate::subset_primitive_gathered;

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupPrimitives {
    max_groups: usize,
    max_vertices: usize,
}

impl GroupPrimitives {
    /// At most `max_groups` primitives (at least 1) of at most `max_vertices` vertices (at least 3, within `u32`).
    pub fn new(max_groups: usize, max_vertices: usize) -> Result<GroupPrimitives, String> {
        if max_groups == 0 {
            return Err("max-groups=0: a mesh keeps at least one primitive".to_owned());
        }
        if max_vertices < 3 || u32::try_from(max_vertices).is_err() {
            return Err(format!(
                "max-vertices={max_vertices}: a triangle needs 3 vertices, and indices are 32-bit"
            ));
        }
        Ok(GroupPrimitives {
            max_groups,
            max_vertices,
        })
    }

    pub fn max_groups(&self) -> usize {
        self.max_groups
    }

    pub fn max_vertices(&self) -> usize {
        self.max_vertices
    }

    /// From its options: `max-groups` and `max-vertices`, both required.
    pub fn from_options(o: &Options) -> Result<GroupPrimitives, String> {
        let (mut groups, mut vertices) = (None, None);
        for (k, v) in o {
            let n = || {
                v.parse::<usize>()
                    .map_err(|_| format!("{k}={v} is not a whole number"))
            };
            match k.as_str() {
                "max-groups" => groups = Some(n()?),
                "max-vertices" => vertices = Some(n()?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        GroupPrimitives::new(
            groups.ok_or("max-groups is required (primitives a mesh may have)")?,
            vertices.ok_or("max-vertices is required (vertices a primitive may have)")?,
        )
    }
}

/// The stream's type, for comparing streams and naming them.
fn kind(s: &Stream) -> &'static str {
    match s {
        Stream::F32x2(_) => "F32x2",
        Stream::F32x3(_) => "F32x3",
        Stream::F32x4(_) => "F32x4",
        Stream::U8x4(_) => "U8x4",
        Stream::U16x4(_) => "U16x4",
    }
}

/// How a primitive is skinned, without the slot of a primitive bound wholly to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    Unskinned,
    Streams(u8),
    SingleIndex,
    Whole,
}

/// What two primitives must share to be joined: their streams and how they are skinned.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shape {
    streams: BTreeMap<Semantic, &'static str>,
    binding: Binding,
}

fn shape(p: &Primitive, skin: Option<&Skin>, i: usize) -> Shape {
    let binding = match skin.and_then(|s| s.influences.get(i).copied().flatten()) {
        None => Binding::Unskinned,
        Some(Influences::Streams { sets }) => Binding::Streams(sets),
        Some(Influences::SingleIndex) => Binding::SingleIndex,
        Some(Influences::Whole { .. }) => Binding::Whole,
    };
    Shape {
        streams: p.streams.iter().map(|(s, st)| (*s, kind(st))).collect(),
        binding,
    }
}

/// Why primitives `i` (shape `a`) and `j` (shape `b`) cannot be joined, if they cannot.
fn mismatch(a: &Shape, b: &Shape, i: usize, j: usize) -> Option<String> {
    for sem in a.streams.keys().chain(b.streams.keys()).collect::<BTreeSet<_>>() {
        let (x, y) = (a.streams.get(sem), b.streams.get(sem));
        if x != y {
            let say = |k: Option<&&str>| k.map_or("absent".to_owned(), |k| (*k).to_owned());
            return Some(format!(
                "stream {sem:?} is {} in primitive {i} and {} in primitive {j}",
                say(x),
                say(y)
            ));
        }
    }
    (a.binding != b.binding).then(|| {
        format!(
            "primitive {i} is skinned as {:?} and primitive {j} as {:?}",
            a.binding, b.binding
        )
    })
}

/// A primitive being built: all its streams, triangle indices, and the source primitive each vertex comes from.
struct Built {
    prim: Primitive,
    origin: Vec<u32>,
}

impl Built {
    fn of(p: &Primitive, i: usize) -> Built {
        let n = p.vertex_count();
        Built {
            prim: Primitive {
                topology: p.topology,
                streams: p.streams.clone(),
                indices: Some(p.indices.clone().unwrap_or_else(|| (0..n as u32).collect())),
                material: p.material,
            },
            origin: vec![i as u32; n],
        }
    }

    /// `other` after this one: the streams are the same (checked by the caller).
    fn append(&mut self, other: Built) -> Result<(), String> {
        let offset = self.origin.len();
        if offset + other.origin.len() > u32::MAX as usize {
            return Err("more than 2^32 vertices".to_owned());
        }
        macro_rules! join {
            ($a:expr, $b:expr) => {
                $a.extend_from_slice(&$b)
            };
        }
        for (sem, st) in other.prim.streams {
            match (self.prim.streams.get_mut(&sem), st) {
                (Some(Stream::F32x2(a)), Stream::F32x2(b)) => join!(a, b),
                (Some(Stream::F32x3(a)), Stream::F32x3(b)) => join!(a, b),
                (Some(Stream::F32x4(a)), Stream::F32x4(b)) => join!(a, b),
                (Some(Stream::U8x4(a)), Stream::U8x4(b)) => join!(a, b),
                (Some(Stream::U16x4(a)), Stream::U16x4(b)) => join!(a, b),
                _ => unreachable!("shapes checked equal"),
            }
        }
        let offset = offset as u32;
        let own = self.prim.indices.get_or_insert_with(Vec::new);
        own.extend(other.prim.indices.into_iter().flatten().map(|i| i + offset));
        self.origin.extend(other.origin);
        Ok(())
    }

    /// Split by whole triangles into parts of at most `max` vertices; one part (this) when it already fits.
    fn split(self, max: usize) -> Result<Vec<Built>, String> {
        if self.origin.len() <= max {
            return Ok(vec![self]);
        }
        let indices = self.prim.indices.as_deref().unwrap_or(&[]);
        let mut stamp = vec![usize::MAX; self.origin.len()];
        let mut parts: Vec<Vec<u32>> = vec![Vec::new()];
        let mut used = 0usize;
        for tri in indices.as_chunks::<3>().0 {
            let fresh = |part: usize, stamp: &[usize]| {
                let mut n = 0;
                for (k, &v) in tri.iter().enumerate() {
                    if stamp[v as usize] != part && !tri[..k].contains(&v) {
                        n += 1;
                    }
                }
                n
            };
            let mut part = parts.len() - 1;
            if used + fresh(part, &stamp) > max {
                parts.push(Vec::new());
                part += 1;
                used = 0;
            }
            used += fresh(part, &stamp);
            for &v in tri {
                stamp[v as usize] = part;
            }
            parts[part].extend_from_slice(tri);
        }
        parts
            .into_iter()
            .map(|selection| {
                let (prim, gather) =
                    subset_primitive_gathered(&self.prim, &selection).map_err(|e| e.to_string())?;
                let origin = gather.iter().map(|&g| self.origin[g as usize]).collect();
                Ok(Built { prim, origin })
            })
            .collect()
    }
}

/// One primitive of the grouped mesh.
struct Piece {
    /// Where it stands: the first primitive it holds, then which part of a split.
    rank: (usize, usize),
    /// The source primitives contributing vertices, ascending; the first stands for the shape.
    origins: Vec<usize>,
    body: Body,
}

enum Body {
    /// Source primitive `i`, as it is.
    Kept(usize),
    Built(Built),
}

/// What grouping a mesh makes.
struct Plan {
    primitives: Vec<Primitive>,
    from: Vec<usize>,
    skin: Vec<SkinPatch>,
    by_material: u64,
    split_off: u64,
    lost_triangles: u64,
}

/// A joined primitive's palette and how it is bound: `piece` is its index in the new primitive list.
struct SkinPatch {
    piece: usize,
    influences: Influences,
    /// Per binding.
    palettes: Vec<Vec<Slot>>,
}

fn triangle_count(p: &Primitive) -> u64 {
    (p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3) as u64
}

impl GroupPrimitives {
    /// The refusals that need no other primitive: topology, streams, indices.
    fn check(&self, mesh: &Mesh) -> Result<(), String> {
        for (i, p) in mesh.primitives().iter().enumerate() {
            if p.topology != Topology::Triangles {
                return Err(format!(
                    "primitive {i} is {:?}: only triangle lists are grouped",
                    p.topology
                ));
            }
            let n = p.vertex_count();
            if p.positions().is_none() {
                return Err(format!("primitive {i} has no float3 positions"));
            }
            if let Some((sem, _)) = p.streams.iter().find(|(_, s)| s.len() != n) {
                return Err(format!("primitive {i}: stream {sem:?} is not {n} vertices long"));
            }
            let count = p.indices.as_ref().map_or(n, Vec::len);
            if count % 3 != 0 {
                return Err(format!("primitive {i}: {count} indices are not whole triangles"));
            }
            if p.indices.iter().flatten().any(|&v| v as usize >= n) {
                return Err(format!("primitive {i}: an index past its {n} vertices"));
            }
        }
        Ok(())
    }

    /// The new primitives of one mesh, or `None` when it stays as it is.
    fn plan(&self, w: &World, e: Entity) -> Result<Option<Plan>, String> {
        let mesh = w.get::<Mesh>(e).expect("listed");
        let skin = w.get::<Skin>(e);
        let prims = mesh.primitives();
        if prims.is_empty() {
            return Ok(None);
        }
        self.check(mesh)?;
        let shapes: Vec<Shape> = prims.iter().enumerate().map(|(i, p)| shape(p, skin, i)).collect();
        let join_check = |i: usize, j: usize| match mismatch(&shapes[i], &shapes[j], i, j) {
            Some(why) => Err(why),
            None => Ok(()),
        };
        // 1. one group a material, in order of first use
        let mut groups: Vec<(Option<Entity>, Vec<usize>)> = Vec::new();
        for (i, p) in prims.iter().enumerate() {
            match groups.iter_mut().find(|(m, _)| *m == p.material) {
                Some((_, members)) => members.push(i),
                None => groups.push((p.material, vec![i])),
            }
        }
        let (mut by_material, mut split_off) = (0u64, 0u64);
        let mut pieces: Vec<Piece> = Vec::new();
        for (_, members) in &groups {
            let first = members[0];
            if members.len() == 1 && prims[first].vertex_count() <= self.max_vertices {
                pieces.push(Piece {
                    rank: (first, 0),
                    origins: vec![first],
                    body: Body::Kept(first),
                });
                continue;
            }
            let mut built = Built::of(&prims[first], first);
            for &j in &members[1..] {
                join_check(first, j)?;
                built.append(Built::of(&prims[j], j))?;
                by_material += 1;
            }
            // 2. split by triangles
            let parts = built.split(self.max_vertices)?;
            split_off += parts.len() as u64 - 1;
            for (k, b) in parts.into_iter().enumerate() {
                let origins: Vec<usize> = b
                    .origin
                    .iter()
                    .map(|&o| o as usize)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                pieces.push(Piece {
                    rank: (first, k),
                    origins: if origins.is_empty() { vec![first] } else { origins },
                    body: Body::Built(b),
                });
            }
        }
        if pieces.len() > self.max_groups {
            return Err(if groups.len() > 1 {
                format!(
                    "{} primitives drawn with {} materials remain, more than max-groups={}: joining primitives of                      different materials would draw one with the other's; run pack-texture-atlas to give them one",
                    pieces.len(),
                    groups.len(),
                    self.max_groups
                )
            } else {
                format!(
                    "its vertices fill {} primitives of at most max-vertices={}, more than max-groups={}",
                    pieces.len(),
                    self.max_vertices,
                    self.max_groups
                )
            });
        }
        pieces.sort_by_key(|p| p.rank);
        let unchanged = pieces.len() == prims.len()
            && pieces
                .iter()
                .enumerate()
                .all(|(k, p)| matches!(p.body, Body::Kept(i) if i == k));
        if unchanged {
            return Ok(None);
        }
        // what a join or split needs of the rest of the entity
        let per_vertex = w.per_vertex_components(e);
        let variants = w.get::<MaterialVariants>(e);
        let mut primitives = Vec::with_capacity(pieces.len());
        let mut from = Vec::with_capacity(pieces.len());
        let mut patches = Vec::new();
        for (k, piece) in pieces.into_iter().enumerate() {
            let origins = piece.origins;
            match piece.body {
                Body::Kept(i) => {
                    primitives.push(prims[i].clone());
                    from.push(i);
                }
                Body::Built(mut b) => {
                    if let Some(name) = per_vertex.first() {
                        return Err(format!(
                            "{name} holds data per vertex, which a join or split of primitives cannot carry"
                        ));
                    }
                    if origins.len() > 1
                        && let Some(&o) = origins
                            .iter()
                            .find(|&&o| variants.and_then(|v| v.0.get(o)).is_some_and(|m| !m.is_empty()))
                    {
                        return Err(format!(
                            "primitive {o} has material variants and would be joined to others"
                        ));
                    }
                    // the primitive it is counted as coming from: most triangles, the earlier on a tie
                    let mut tris: BTreeMap<u32, u64> = BTreeMap::new();
                    let indices = b.prim.indices.as_deref().unwrap_or(&[]);
                    for t in indices.as_chunks::<3>().0 {
                        *tris.entry(b.origin[t[0] as usize]).or_default() += 1;
                    }
                    let source = tris
                        .iter()
                        .max_by_key(|(o, n)| (**n, std::cmp::Reverse(**o)))
                        .map_or(origins[0], |(o, _)| *o as usize);
                    if origins.len() > 1
                        && let Some(skin) = skin
                        && let Some(patch) = unify_palettes(skin, &mut b, &origins)?
                    {
                        patches.push(SkinPatch { piece: k, ..patch });
                    }
                    primitives.push(b.prim);
                    from.push(source);
                }
            }
        }
        // what the conservation count sees as lost: a source counted by pieces holding fewer of its triangles
        let before: Vec<u64> = prims.iter().map(triangle_count).collect();
        let sources: BTreeSet<usize> = from.iter().copied().collect();
        let lost_triangles = sources
            .iter()
            .map(|&j| {
                let now: u64 = from
                    .iter()
                    .zip(&primitives)
                    .filter(|(f, _)| **f == j)
                    .map(|(_, p)| triangle_count(p))
                    .sum();
                before[j].saturating_sub(now)
            })
            .sum();
        Ok(Some(Plan {
            primitives,
            from,
            skin: patches,
            by_material,
            split_off,
            lost_triangles,
        }))
    }
}

/// Give `b` (made of the primitives `origins`) one palette, rewriting its joint indices to it. `None` when none of
/// them is skinned.
fn unify_palettes(skin: &Skin, b: &mut Built, origins: &[usize]) -> Result<Option<SkinPatch>, String> {
    let Some(influences) = skin.influences.get(origins[0]).copied().flatten() else {
        return Ok(None);
    };
    if skin.bindings.is_empty() {
        return Err(format!(
            "primitive {} is skinned, but the skin has no binding",
            origins[0]
        ));
    }
    // a slot is the same slot when every binding agrees
    let mut unified: Vec<Vec<Slot>> = Vec::new();
    let mut maps: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &o in origins {
        let palettes: Vec<&Vec<Slot>> = skin
            .bindings
            .iter()
            .map(|bd| {
                bd.palettes
                    .get(o)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| format!("primitive {o} has no palette in binding {:?}", bd.id))
            })
            .collect::<Result<_, _>>()?;
        let len = palettes[0].len();
        if palettes.iter().any(|p| p.len() != len) {
            return Err(format!(
                "primitive {o}'s palettes differ in size between bindings"
            ));
        }
        let mut map = Vec::with_capacity(len);
        for k in 0..len {
            let key: Vec<Slot> = palettes.iter().map(|p| p[k]).collect();
            let at = match unified.iter().position(|u| *u == key) {
                Some(at) => at,
                None => {
                    unified.push(key);
                    unified.len() - 1
                }
            };
            map.push(at as u32);
        }
        maps.insert(o as u32, map);
    }
    let remap = |o: u32, slot: u32| -> Result<u32, String> {
        maps[&o].get(slot as usize).copied().ok_or_else(|| {
            format!(
                "primitive {o} names slot {slot} of a palette of {}",
                maps[&o].len()
            )
        })
    };
    let new_influences = match influences {
        Influences::Whole { slot } => {
            let slots: BTreeSet<u32> = origins
                .iter()
                .map(|&o| match skin.influences.get(o).copied().flatten() {
                    Some(Influences::Whole { slot }) => remap(o as u32, slot),
                    _ => unreachable!("shapes checked equal"),
                })
                .collect::<Result<_, _>>()?;
            if slots.len() > 1 {
                return Err(format!(
                    "primitives {origins:?} are each bound wholly to a different joint (the first to slot {slot}) \
                     and cannot be one primitive"
                ));
            }
            Influences::Whole {
                slot: *slots.first().expect("an origin"),
            }
        }
        Influences::SingleIndex | Influences::Streams { .. } => {
            let sets = match influences {
                Influences::Streams { sets } => sets,
                _ => 1,
            };
            for k in 0..sets {
                let stream = b
                    .prim
                    .streams
                    .get_mut(&Semantic::Joints(k))
                    .ok_or_else(|| format!("Joints({k}) is missing"))?;
                match stream {
                    Stream::U8x4(v) => {
                        if unified.len() > 256 {
                            return Err(format!(
                                "the joined palette has {} slots, more than 8-bit joint indices hold",
                                unified.len()
                            ));
                        }
                        for (j, o) in v.iter_mut().zip(&b.origin) {
                            for s in j.iter_mut() {
                                *s = remap(*o, u32::from(*s))? as u8;
                            }
                        }
                    }
                    Stream::U16x4(v) => {
                        if unified.len() > 65536 {
                            return Err(format!(
                                "the joined palette has {} slots, more than 16-bit joint indices hold",
                                unified.len()
                            ));
                        }
                        for (j, o) in v.iter_mut().zip(&b.origin) {
                            for s in j.iter_mut() {
                                *s = remap(*o, u32::from(*s))? as u16;
                            }
                        }
                    }
                    _ => return Err(format!("Joints({k}) is not an integer stream")),
                }
            }
            influences
        }
    };
    let palettes = (0..skin.bindings.len())
        .map(|bd| unified.iter().map(|key| key[bd]).collect())
        .collect();
    Ok(Some(SkinPatch {
        piece: 0,
        influences: new_influences,
        palettes,
    }))
}

impl Pass for GroupPrimitives {
    fn name(&self) -> &'static str {
        "group-primitives"
    }
    fn options(&self) -> Options {
        Options::from([
            ("max-groups".to_owned(), self.max_groups.to_string()),
            ("max-vertices".to_owned(), self.max_vertices.to_string()),
        ])
    }
    // a world without meshes has nothing to group
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME]
    }
    /// A changed mesh's file link.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        // every mesh planned before any changes
        let mut plans = Vec::new();
        for e in meshes {
            if let Some(plan) = self.plan(w, e).map_err(|why| format!("{e:?}: {why}"))? {
                plans.push((e, plan));
            }
        }
        for (e, plan) in plans {
            let Plan {
                primitives,
                from,
                skin,
                by_material,
                split_off,
                lost_triangles,
            } = plan;
            let (before, made) = (
                w.get::<Mesh>(e).expect("planned").primitives().len(),
                primitives.len(),
            );
            let kept: BTreeSet<usize> = from.iter().copied().collect();
            w.rebuild_primitives(e, |_| Ok((primitives, from)))?;
            if !skin.is_empty() {
                let s = w.get_mut::<Skin>(e).expect("planned with a skin");
                for patch in skin {
                    s.influences[patch.piece] = Some(patch.influences);
                    for (b, palette) in s.bindings.iter_mut().zip(patch.palettes) {
                        b.palettes[patch.piece] = Some(palette);
                    }
                }
            }
            w.remove::<MeshFile>(e);
            log.count("meshes regrouped", 1);
            log.count("primitives made", made as u64);
            log.count("primitives joined by material", by_material);
            log.count("parts split off by triangles", split_off);
            log.dropped(
                Measure::Primitives,
                "joined into another primitive of the mesh, triangles kept",
                (before - kept.len()) as u64,
            );
            log.dropped(
                Measure::Triangles,
                "triangles counted with a primitive other than the one now holding them",
                lost_triangles,
            );
        }
        Ok(())
    }
}
