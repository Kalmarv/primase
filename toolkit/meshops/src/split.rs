//! `split-by-collision{placements=<prefix>;...,groups=<collision prefix>=<group>;...,tolerance=M,rest=<group>}`:
//! render placements whose geometry merges what a source collides with as separate named pieces (a bake that draws
//! a room's shell, its furniture and a skatepark as one mesh, beside collision kept per named node) split into one
//! placement per group of those pieces, so a later pass can treat each apart (a destination's selection by
//! placement name, `<placement>~<group>`: a room's shell casting no shadow, the park casting).
//!
//! The grouping is the program's, every option required:
//!
//! - `placements`: the placements to split, by name prefix (a placement's `core.name`, as its reader wrote it);
//! - `groups`: collision placements by name prefix, each to a group (`Skatepark_<=park;SkateparkWall_<=room`). A
//!   collision placement is an entity with a `core.instance` and a `core.transform` whose prototype has a
//!   `core.collision`, named by its own name, else its prototype's; or an entity with a `core.collision` and a
//!   `core.transform` that no placement places, named by its own. The longest prefix that starts its name chooses its
//!   group; a collision placement no prefix matches is no group's (counted). A prefix holds no `;`; a group name no
//!   `;` and no `=`;
//! - `tolerance`: metres, finite and above 0: a triangle belongs to a group when each of its three corners, placed,
//!   lies within `tolerance` of a triangle of that group's collision, placed. Of several such groups the nearest wins:
//!   the one whose farthest corner is nearest (ties: the group named first in name order);
//! - `rest`: the group of the triangles near no group's collision, and of the primitives that are not triangles.
//!
//! Each selected placement becomes one placement per group its triangles fall in, named `<placement>~<group>`: the
//! placement itself for the first group in name order, a copy of it ([`World::duplicate`]) for each other. Each
//! places a prototype of its own, named the same, made from the placement's prototype ([`World::copy_components`],
//! declared with [`PassLog::place_from`]): per primitive of the source, the triangles of that group (vertices
//! compacted, every stream kept, [`crate::subset_primitive`]), its material unchanged; per-primitive components follow
//! ([`World::remap_primitives`]), then per-vertex ones the compacted vertices ([`World::remap_vertices`]), and one
//! that can't follow (an opaque component) fails the pass naming it; the file link (`core.mesh_file`) does not follow
//! (the mesh is no longer the file's). A placement's own collision (`core.collision` on the placement) stays on the
//! first group's placement only, counted: copies of it would collide again. A prototype no placement places any
//! longer is despawned: its primitives all stand in the copies. Triangles are counted per group, and those near no
//! group's collision apart.
//!
//! Refused, before anything changes: a prefix that matches nothing (a placement prefix no placement's name starts
//! with, a group prefix no collision placement's), a selected placement without a transform or whose prototype has no
//! loaded mesh, levels of detail ([`Lods`]: each level would need splitting) or a collision of its own (a copy per
//! group would collide several times), a triangle primitive whose positions are not `F32x3` or whose indices run past
//! them.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    Collision, Component, Entity, Instance, Lods, Mesh, MeshFile, Name, Primitive, Topology, Transform, World,
};

use crate::{Triangle, TriangleGrid, subset_primitive_gathered};

/// See the module documentation. Made by [`SplitByCollision::new`]; from options ([`SPLIT_BY_COLLISION`]) by the same
/// call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitByCollision {
    placements: BTreeSet<String>,
    groups: BTreeMap<String, String>,
    /// Metres, finite and above 0, kept as their bits so the pass compares exactly.
    tolerance: u64,
    rest: String,
}

impl SplitByCollision {
    /// Split the placements named with a prefix in `placements` (at least one) by the collision placements named with
    /// a prefix in `groups` (prefix to group, at least one), corners within `tolerance` metres (finite, above 0), the
    /// rest to `rest`. No empty prefix or group name; no `;` in either, no `=` in a group name.
    pub fn new(
        placements: BTreeSet<String>,
        groups: BTreeMap<String, String>,
        tolerance: f64,
        rest: String,
    ) -> Result<SplitByCollision, String> {
        if placements.is_empty() {
            return Err("split-by-collision needs at least one placement prefix".into());
        }
        if groups.is_empty() {
            return Err("split-by-collision needs at least one collision prefix and its group".into());
        }
        if let Some(p) = placements
            .iter()
            .chain(groups.keys())
            .find(|p| p.is_empty() || p.contains(';'))
        {
            return Err(format!("prefix {p:?}: empty or holding ';'"));
        }
        if let Some(g) = groups
            .values()
            .chain([&rest])
            .find(|g| g.is_empty() || g.contains([';', '=']))
        {
            return Err(format!("group {g:?}: empty or holding ';' or '='"));
        }
        if !(tolerance.is_finite() && tolerance > 0.0) {
            return Err(format!("tolerance={tolerance} is not finite metres above 0"));
        }
        Ok(SplitByCollision {
            placements,
            groups,
            tolerance: tolerance.to_bits(),
            rest,
        })
    }

    pub fn placements(&self) -> &BTreeSet<String> {
        &self.placements
    }

    pub fn groups(&self) -> &BTreeMap<String, String> {
        &self.groups
    }

    pub fn tolerance(&self) -> f64 {
        f64::from_bits(self.tolerance)
    }

    pub fn rest(&self) -> &str {
        &self.rest
    }

    /// From its declared options, through [`SplitByCollision::new`]: `groups`' group follows its last `=`.
    fn from_given(g: &Given) -> Result<SplitByCollision, String> {
        let mut groups = BTreeMap::new();
        for pair in g.required("groups").split(';') {
            let (prefix, group) = pair
                .rsplit_once('=')
                .ok_or_else(|| format!("groups: {pair} is not <collision prefix>=<group>"))?;
            if groups.insert(prefix.to_owned(), group.to_owned()).is_some() {
                return Err(format!("groups: prefix {prefix} given twice"));
            }
        }
        let t = g.required("tolerance");
        let tolerance = t
            .parse::<f64>()
            .map_err(|_| format!("tolerance={t} is not metres"))?;
        SplitByCollision::new(
            g.required("placements").split(';').map(str::to_owned).collect(),
            groups,
            tolerance,
            g.required("rest").to_owned(),
        )
    }
}

/// `split-by-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): the grouping is the program's,
/// every option required.
pub const SPLIT_BY_COLLISION: PassSpec = PassSpec {
    name: "split-by-collision",
    options: &[
        OptionSpec::required(
            "placements",
            "level",
            "name prefixes of the placements to split, joined by ;",
        ),
        OptionSpec::required(
            "groups",
            "floor=ground",
            "<collision prefix>=<group>, joined by ;: the group of each collision placement's prefix",
        ),
        OptionSpec::required(
            "tolerance",
            "0.05",
            "metres, finite and above 0: how near a triangle's corners lie to a group's collision",
        ),
        OptionSpec::required(
            "rest",
            "other",
            "the group of the triangles near no group's collision",
        ),
    ],
    make: |g| Ok(Box::new(SplitByCollision::from_given(g)?)),
};

fn place(t: &Transform, p: [f32; 3]) -> [f64; 3] {
    std::array::from_fn(|r| {
        (0..3)
            .map(|c| f64::from(t.cols[c][r]) * f64::from(p[c]))
            .sum::<f64>()
            + f64::from(t.cols[3][r])
    })
}

/// A triangle primitive's corners: its indices, else its vertices in order.
fn corners(p: &Primitive) -> Vec<u32> {
    match &p.indices {
        Some(ix) => ix.clone(),
        None => (0..p.vertex_count() as u32).collect(),
    }
}

/// One group's share of a placement's prototype: per source primitive, the primitive drawn and, when its vertices
/// were compacted, which source vertex each of its vertices is.
type Share = Vec<(usize, Primitive, Option<Vec<u32>>)>;

impl Pass for SplitByCollision {
    fn name(&self) -> &'static str {
        "split-by-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "placements".to_owned(),
                self.placements.iter().cloned().collect::<Vec<_>>().join(";"),
            ),
            (
                "groups".to_owned(),
                self.groups
                    .iter()
                    .map(|(p, g)| format!("{p}={g}"))
                    .collect::<Vec<_>>()
                    .join(";"),
            ),
            ("tolerance".to_owned(), self.tolerance().to_string()),
            ("rest".to_owned(), self.rest.clone()),
        ])
    }
    fn modifies(&self) -> &[&'static str] {
        &[Instance::NAME, Name::NAME, primase_core::Asset::NAME]
    }
    /// A prototype no placement places once its placements are split.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let tolerance = self.tolerance();
        let name = |e: Entity| w.get::<Name>(e).map(|n| n.0.as_str());
        // the placements to split
        let selected: Vec<(Entity, Instance)> = w
            .iter::<Instance>()
            .filter(|(e, _)| {
                name(*e).is_some_and(|n| self.placements.iter().any(|p| n.starts_with(p.as_str())))
            })
            .map(|(e, i)| (e, *i))
            .collect();
        if let Some(p) = self.placements.iter().find(|p| {
            !w.iter::<Instance>()
                .any(|(e, _)| name(e).is_some_and(|n| n.starts_with(p.as_str())))
        }) {
            return Err(format!("split-by-collision: no placement's name starts with {p}"));
        }
        for (e, i) in &selected {
            let n = name(*e).unwrap_or_default();
            let why = if !w.has::<Transform>(*e) {
                Some("has no transform")
            } else if !w.has::<Mesh>(i.prototype) {
                Some("places no loaded mesh")
            } else if w.has::<Lods>(i.prototype) {
                Some("places levels of detail: select a level first")
            } else if w.has::<Collision>(i.prototype) {
                Some("places a prototype with a collision of its own")
            } else {
                None
            };
            if let Some(why) = why {
                return Err(format!("split-by-collision: placement {n} ({e:?}) {why}"));
            }
        }
        // the collision placements, each in the group of the longest prefix its name starts with
        let prototypes: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let mut colliders: Vec<(Entity, Entity, &str)> = Vec::new();
        for (e, i) in w.iter::<Instance>() {
            if w.has::<Collision>(i.prototype)
                && w.has::<Transform>(e)
                && let Some(n) = name(e).or_else(|| name(i.prototype))
            {
                colliders.push((e, i.prototype, n));
            }
        }
        for (e, _) in w.iter::<Collision>() {
            if !prototypes.contains(&e)
                && w.has::<Transform>(e)
                && let Some(n) = name(e)
            {
                colliders.push((e, e, n));
            }
        }
        let group_names: Vec<&String> = self
            .groups
            .values()
            .chain([&self.rest])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let index_of = |g: &String| group_names.iter().position(|n| *n == g).expect("listed") as u32;
        let mut triangles: Vec<(Triangle, u32)> = Vec::new();
        let mut matched: BTreeSet<&String> = BTreeSet::new();
        let mut ungrouped = 0u64;
        for &(placement, soup, n) in &colliders {
            let Some((prefix, group)) = self
                .groups
                .iter()
                .filter(|(p, _)| n.starts_with(p.as_str()))
                .max_by_key(|(p, _)| p.len())
            else {
                ungrouped += 1;
                continue;
            };
            matched.insert(prefix);
            let (t, c) = (
                w.get::<Transform>(placement).expect("listed"),
                w.get::<Collision>(soup).expect("listed"),
            );
            let label = index_of(group);
            for tri in &c.triangles {
                let corner = |k: u32| {
                    c.positions
                        .get(k as usize)
                        .map(|&p| place(t, p))
                        .ok_or_else(|| format!("split-by-collision: collision {n} indexes position {k}"))
                };
                triangles.push(([corner(tri[0])?, corner(tri[1])?, corner(tri[2])?], label));
            }
        }
        if let Some(p) = self.groups.keys().find(|p| !matched.contains(p)) {
            return Err(format!(
                "split-by-collision: no collision placement's name starts with {p}"
            ));
        }
        // the box of interest: the selected placements' corners, and the tolerance around them
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for (e, i) in &selected {
            let t = w.get::<Transform>(*e).expect("checked");
            for p in w.get::<Mesh>(i.prototype).expect("checked").primitives() {
                for &v in p.positions().unwrap_or_default() {
                    let v = place(t, v);
                    for a in 0..3 {
                        bounds[0][a] = bounds[0][a].min(v[a] - tolerance);
                        bounds[1][a] = bounds[1][a].max(v[a] + tolerance);
                    }
                }
            }
        }
        if bounds[0][0] > bounds[1][0] {
            bounds = [[0.0; 3]; 2];
        }
        // the cell sets what a query visits, never its answer: at least the tolerance, at most 128 across the box
        let extent = (0..3).map(|a| bounds[1][a] - bounds[0][a]).fold(0.0, f64::max);
        let grid = TriangleGrid::new(triangles, tolerance.max(extent / 128.0), bounds)?;
        log.count("collision placements in no group", ungrouped);

        // each selected placement's triangles, by group
        let rest = index_of(&self.rest) as usize;
        let mut splits: Vec<(Entity, Instance, String, BTreeMap<usize, Share>)> = Vec::new();
        for &(e, inst) in &selected {
            let t = w.get::<Transform>(e).expect("checked");
            let mesh = w.get::<Mesh>(inst.prototype).expect("checked");
            let pname = name(e).unwrap_or_default();
            let mut shares: BTreeMap<usize, BTreeMap<usize, Vec<u32>>> = BTreeMap::new();
            let mut whole: Vec<usize> = Vec::new();
            for (pi, p) in mesh.primitives().iter().enumerate() {
                if p.topology != Topology::Triangles {
                    whole.push(pi);
                    log.count("primitives that are not triangles, put in the rest group", 1);
                    continue;
                }
                let pos = p.positions().ok_or_else(|| {
                    format!("split-by-collision: placement {pname}: primitive {pi}'s positions are not F32x3")
                })?;
                let ix = corners(p);
                if !ix.len().is_multiple_of(3) {
                    return Err(format!(
                        "split-by-collision: placement {pname}: primitive {pi} has {} corners, not whole triangles",
                        ix.len()
                    ));
                }
                if ix.is_empty() {
                    whole.push(pi);
                    continue;
                }
                // each corner's nearest distance per group, once per vertex
                let mut near: BTreeMap<u32, BTreeMap<u32, f64>> = BTreeMap::new();
                for &k in &ix {
                    if let std::collections::btree_map::Entry::Vacant(slot) = near.entry(k) {
                        let v = pos.get(k as usize).ok_or_else(|| {
                            format!(
                                "split-by-collision: placement {pname}: primitive {pi} indexes vertex {k} of {}",
                                pos.len()
                            )
                        })?;
                        slot.insert(grid.nearest_by_label(place(t, *v), tolerance));
                    }
                }
                for tri in ix.as_chunks::<3>().0 {
                    let [a, b, c] = tri.map(|k| &near[&k]);
                    // the group whose farthest corner is nearest, each corner within the tolerance
                    let group = a
                        .iter()
                        .filter_map(|(g, &da)| Some((*g, da.max(*b.get(g)?).max(*c.get(g)?))))
                        .min_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)))
                        .map(|(g, _)| g as usize);
                    let g = group.unwrap_or_else(|| {
                        log.count("triangles near no group's collision, put in the rest group", 1);
                        rest
                    });
                    log.count(&format!("triangles in group {}", group_names[g]), 1);
                    shares
                        .entry(g)
                        .or_default()
                        .entry(pi)
                        .or_default()
                        .extend_from_slice(tri);
                }
            }
            let mut by_group: BTreeMap<usize, Share> = BTreeMap::new();
            for (g, prims) in shares {
                for (pi, ix) in prims {
                    let source = &mesh.primitives()[pi];
                    let (piece, gather) =
                        subset_primitive_gathered(source, &ix).map_err(|e| e.to_string())?;
                    // every vertex in its own order: nothing for a per-vertex component to follow
                    let moved = !(gather.len() == source.vertex_count()
                        && gather.iter().enumerate().all(|(i, &v)| v as usize == i));
                    by_group
                        .entry(g)
                        .or_default()
                        .push((pi, piece, moved.then_some(gather)));
                }
            }
            for pi in whole {
                by_group
                    .entry(rest)
                    .or_default()
                    .push((pi, mesh.primitives()[pi].clone(), None));
            }
            for share in by_group.values_mut() {
                share.sort_by_key(|(pi, ..)| *pi);
            }
            splits.push((e, inst, pname.to_owned(), by_group));
        }

        // each group's prototype and placement
        let group_names: Vec<String> = group_names.into_iter().cloned().collect();
        for (e, inst, pname, by_group) in splits {
            for (n, (g, share)) in by_group.into_iter().enumerate() {
                let label = format!("{pname}~{}", group_names[g]);
                let proto = w.spawn();
                w.copy_components(inst.prototype, proto);
                let mut from = Vec::new();
                let mut prims = Vec::new();
                let mut gathers = Vec::new();
                for (pi, p, gather) in share {
                    from.push(pi);
                    prims.push(p);
                    gathers.push(gather);
                }
                // per-primitive components follow the pieces, then per-vertex ones each piece's vertices; one that
                // can't follow fails the pass naming it
                w.remap_primitives(proto, &from)
                    .map_err(|e| format!("split-by-collision: placement {pname}: {e}"))?;
                w.insert(proto, Mesh::new(prims));
                for (k, gather) in gathers.iter().enumerate() {
                    if let Some(g) = gather {
                        w.remap_vertices(proto, k, g)
                            .map_err(|e| format!("split-by-collision: placement {pname}: {e}"))?;
                    }
                }
                w.remove::<MeshFile>(proto);
                w.insert(proto, Name(label.clone()));
                log.place_from(proto, inst.prototype);
                log.count("prototypes made, one per group of a split placement", 1);
                let placement = if n == 0 {
                    e
                } else {
                    let copy = w.duplicate(e).expect("alive");
                    // the placement's own collision stays on the first group's placement: once in the world
                    if w.remove::<Collision>(copy).is_some() {
                        log.count(
                            "placements' own collision kept on the first group's placement only (left off a copy)",
                            1,
                        );
                    }
                    log.place_from(copy, e);
                    log.count("placements added for the other groups of a split placement", 1);
                    copy
                };
                w.insert(
                    placement,
                    Instance {
                        prototype: proto,
                        variant: inst.variant,
                    },
                );
                w.insert(placement, Name(label));
            }
            log.count("placements split", 1);
        }
        // the prototypes no placement places any longer: their primitives stand in the copies
        let placed: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let sources: BTreeSet<Entity> = selected.iter().map(|(_, i)| i.prototype).collect();
        for p in sources.difference(&placed) {
            // its primitives stand in the copies: nothing is left out to declare, but no asset keeps listing it
            w.despawn(*p);
            primase_core::ledger::unlist(w, *p, log);
            log.count("prototypes left unplaced by the split, despawned", 1);
        }
        Ok(())
    }
}
