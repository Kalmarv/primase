//! `partition-grid`: divide a world's content into output parts by a ground grid, as a [`PartitionPlan`] the
//! conversion loop takes its parts from ([`primase_core::convert::PlannedParts`]).
//!
//! Every piece of independent content (an entity nothing else refers to: a placement, a marker, a curve, a light, a
//! standalone collision soup or mesh) is in exactly one part; what it reaches (prototypes, materials, textures) comes
//! with it. Content is filed by where it stands: a marker by its ground position, a curve by the middle of its points'
//! bounds, anything else with a transform by its position, in the ground frame (`east`, `north`:
//! [`primase_core::frame::ground`]). Cells are half-open, `[origin + k * cell, origin + (k + 1) * cell)` on each axis,
//! so a point on a border belongs to the cell above it. Content with no position (a world soup with no transform) is
//! its own last part, `unplaced`. With `tile=T`, the cells are aligned to a writer's `T`-metre tiles: a cell and the
//! origin whole multiples of `T` (refused otherwise), and content is filed by the tile it stands in, by the writers' own
//! rule ([`primase_core::Tile::of`], or the `core.tile` it states), so a tile's content is never split between parts.
//! A part's context is the content of other cells standing within `halo` metres of its
//! cell, outside the region too (an operation may need to read a neighbour the region leaves out); the part's area
//! (its cell and halo) lets [`primase_core::convert::PlannedParts`] add what reaches it by its authored box.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{PartitionGroup, PartitionPlan};
use primase_core::frame::ground;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Curve, Entity, Marker, Resource, Tile, Transform, World};

/// `partition-grid{cell=C[,origin=E;N][,halo=H][,region=E0;N0;E1;N1][,tile=T]}`: see the module documentation. `cell`
/// has no default: how big a part is is the program's choice. `region` keeps only what stands in it (half-open), the rest
/// left out and counted. Built by [`PartitionGrid::new`] and its setters, which check every value; the pipeline text
/// goes through the same constructor ([`PARTITION_GRID`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionGrid {
    cell: f64,
    origin: [f64; 2],
    halo: f64,
    region: Option<[f64; 4]>,
    /// The writer's tile the cells align to (`None`: not aligned).
    tile: Option<f64>,
}

fn finite(what: &str, x: f64) -> Result<f64, String> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(format!("{what} {x} is not a finite number"))
    }
}

impl PartitionGrid {
    /// A grid of `cell`-metre cells from the origin (0, 0), no halo, no region. `cell` is finite and above 0.
    pub fn new(cell: f64) -> Result<PartitionGrid, String> {
        // finite, so a plain comparison: not above 0
        if finite("cell", cell)? <= 0.0 {
            return Err(format!("cell {cell} is not above 0 metres"));
        }
        Ok(PartitionGrid {
            cell,
            origin: [0.0; 2],
            halo: 0.0,
            region: None,
            tile: None,
        })
    }

    /// Whether `x` is a whole multiple of the tile (when aligned).
    fn aligned(&self, what: &str, x: f64) -> Result<(), String> {
        match self.tile {
            Some(t) if (x / t).fract() != 0.0 || (x / t) * t != x => Err(format!(
                "{what} {x} is not a whole multiple of the tile {t}: an aligned grid's cells hold whole tiles"
            )),
            _ => Ok(()),
        }
    }

    /// The cells' origin `[east, north]`, metres.
    pub fn with_origin(mut self, origin: [f64; 2]) -> Result<PartitionGrid, String> {
        self.origin = [
            finite("origin east", origin[0])?,
            finite("origin north", origin[1])?,
        ];
        self.aligned("origin east", self.origin[0])?;
        self.aligned("origin north", self.origin[1])?;
        Ok(self)
    }

    /// Cells aligned to a writer's `tile`-metre tiles (finite, above 0, exact in f32 as the writers take it): the
    /// cell and the origin must be whole multiples of it, and content is filed by its tile.
    pub fn with_tile(mut self, tile: f64) -> Result<PartitionGrid, String> {
        if finite("tile", tile)? <= 0.0 || f64::from(tile as f32) != tile {
            return Err(format!(
                "tile {tile} is not above 0 metres and exact as a 32-bit float"
            ));
        }
        self.tile = Some(tile);
        self.aligned("cell", self.cell)?;
        self.aligned("origin east", self.origin[0])?;
        self.aligned("origin north", self.origin[1])?;
        Ok(self)
    }

    /// The halo, metres, finite and not negative: other cells' content standing within it is a part's context.
    pub fn with_halo(mut self, halo: f64) -> Result<PartitionGrid, String> {
        if finite("halo", halo)? < 0.0 {
            return Err(format!("halo {halo} is negative"));
        }
        self.halo = halo;
        Ok(self)
    }

    /// The region `[west, south, east, north]` (half-open, finite, west < east, south < north).
    pub fn with_region(mut self, r: [f64; 4]) -> Result<PartitionGrid, String> {
        for x in r {
            finite("region", x)?;
        }
        if !(r[0] < r[2] && r[1] < r[3]) {
            return Err(format!(
                "region {r:?}: not west,south,east,north with west < east, south < north"
            ));
        }
        self.region = Some(r);
        Ok(self)
    }

    pub fn cell(&self) -> f64 {
        self.cell
    }
    pub fn origin(&self) -> [f64; 2] {
        self.origin
    }
    pub fn halo(&self) -> f64 {
        self.halo
    }
    pub fn region(&self) -> Option<[f64; 4]> {
        self.region
    }
    pub fn tile(&self) -> Option<f64> {
        self.tile
    }

    /// From its declared options, through [`PartitionGrid::new`] and the setters.
    fn from_given(g: &Given) -> Result<PartitionGrid, String> {
        let numbers = |k: &str, v: &str, n: usize| -> Result<Vec<f64>, String> {
            let xs: Vec<f64> = v
                .split(';')
                .map(|x| {
                    x.trim()
                        .parse::<f64>()
                        .map_err(|_| format!("{k}={v}: not numbers"))
                })
                .collect::<Result<_, _>>()?;
            if xs.len() == n {
                Ok(xs)
            } else {
                Err(format!("{k}={v}: {n} numbers expected"))
            }
        };
        let mut grid = PartitionGrid::new(numbers("cell", g.required("cell"), 1)?[0])?;
        if let Some(v) = g.optional("tile") {
            grid = grid.with_tile(numbers("tile", v, 1)?[0])?;
        }
        if let Some(v) = g.optional("origin") {
            let xs = numbers("origin", v, 2)?;
            grid = grid.with_origin([xs[0], xs[1]])?;
        }
        if let Some(v) = g.optional("halo") {
            grid = grid.with_halo(numbers("halo", v, 1)?[0])?;
        }
        if let Some(v) = g.optional("region") {
            let xs = numbers("region", v, 4)?;
            grid = grid.with_region([xs[0], xs[1], xs[2], xs[3]])?;
        }
        Ok(grid)
    }

    /// The cell content stands in: by its position `p` `[east, north]`; aligned to tiles, by its tile (the one it
    /// states, `stated`, else the one `p` stands in), as a writer files it.
    fn cell_of(&self, p: [f64; 2], stated: Option<Tile>) -> Result<(i64, i64), String> {
        if let Some(t) = self.tile {
            // whole multiples (with_tile): the counts of tiles are exact
            let tile = match stated {
                Some(tile) => tile,
                None => Tile::of([p[0] as f32, p[1] as f32], t as f32).map_err(|e| e.to_string())?,
            };
            let per = (self.cell / t) as i64;
            let (o0, o1) = ((self.origin[0] / t) as i64, (self.origin[1] / t) as i64);
            return Ok((
                (i64::from(tile.col) - o0).div_euclid(per),
                (i64::from(tile.row) - o1).div_euclid(per),
            ));
        }
        Ok((
            ((p[0] - self.origin[0]) / self.cell).floor() as i64,
            ((p[1] - self.origin[1]) / self.cell).floor() as i64,
        ))
    }

    /// The cell `p` stands in by position alone.
    fn cell_at(&self, p: [f64; 2]) -> (i64, i64) {
        (
            ((p[0] - self.origin[0]) / self.cell).floor() as i64,
            ((p[1] - self.origin[1]) / self.cell).floor() as i64,
        )
    }

    /// Whether `p` is within `halo` of cell `c` (and not in it).
    fn near(&self, c: (i64, i64), p: [f64; 2]) -> bool {
        let lo = [
            self.origin[0] + c.0 as f64 * self.cell,
            self.origin[1] + c.1 as f64 * self.cell,
        ];
        let d = |x: f64, lo: f64| (lo - x).max(x - (lo + self.cell)).max(0.0);
        let (dx, dy) = (d(p[0], lo[0]), d(p[1], lo[1]));
        self.halo > 0.0 && (dx * dx + dy * dy).sqrt() <= self.halo
    }
}

/// Placed content by the grid cell it stands in (its position alone: tiles and regions do not move it), so a cell finds
/// the content within its halo from the cells around it, not from all of the content.
struct Buckets {
    by_cell: BTreeMap<(i64, i64), Vec<usize>>,
    /// How many cells a halo reaches past a cell's own (one more than the distance needs, for rounding).
    reach: i64,
}

impl Buckets {
    fn of(grid: &PartitionGrid, at: &[(Entity, [f64; 2])]) -> Buckets {
        let mut by_cell: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        if grid.halo > 0.0 {
            for (k, (_, p)) in at.iter().enumerate() {
                by_cell.entry(grid.cell_at(*p)).or_default().push(k);
            }
        }
        // saturating: a halo past the i64 cells is every cell
        let reach = (grid.halo / grid.cell).ceil() as i64;
        Buckets {
            by_cell,
            reach: reach.saturating_add(1),
        }
    }

    /// The indices of the content that may stand within the halo of cell `c`: everything in the cells `reach` around it
    /// (all of the content when that is more cells than there are occupied ones). None without a halo, which makes
    /// every cell's context empty.
    fn near<'a>(&'a self, grid: &PartitionGrid, c: (i64, i64)) -> Box<dyn Iterator<Item = usize> + 'a> {
        if grid.halo <= 0.0 {
            return Box::new(std::iter::empty());
        }
        let side = self.reach.saturating_mul(2).saturating_add(1);
        let square = u128::try_from(side)
            .unwrap_or(u128::MAX)
            .saturating_mul(u128::try_from(side).unwrap_or(u128::MAX));
        if square > self.by_cell.len() as u128 {
            let (reach, lo) = (
                self.reach,
                (c.0.saturating_sub(self.reach), c.1.saturating_sub(self.reach)),
            );
            let hi = (c.0.saturating_add(reach), c.1.saturating_add(reach));
            Box::new(
                self.by_cell
                    .range((lo.0, i64::MIN)..=(hi.0, i64::MAX))
                    .filter(move |((_, y), _)| (lo.1..=hi.1).contains(y))
                    .flat_map(|(_, ks)| ks.iter().copied()),
            )
        } else {
            let (x0, y0) = (c.0.saturating_sub(self.reach), c.1.saturating_sub(self.reach));
            Box::new((0..side).flat_map(move |dx| {
                (0..side).flat_map(move |dy| {
                    self.by_cell
                        .get(&(x0.saturating_add(dx), y0.saturating_add(dy)))
                        .into_iter()
                        .flatten()
                        .copied()
                })
            }))
        }
    }
}

/// Where an entity stands on the ground, if it is placed content.
fn position(w: &World, e: Entity) -> Option<[f64; 2]> {
    let at = |p: [f32; 3]| ground(p).map(f64::from);
    if let Some(m) = w.get::<Marker>(e)
        && let Some(g) = m.ground
    {
        return Some(g.map(f64::from));
    }
    if let Some(c) = w.get::<Curve>(e) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &c.points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        return c
            .points
            .first()
            .map(|_| at(std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0)));
    }
    w.get::<Transform>(e).map(|t| at(t.cols[3]))
}

/// The strongly connected components of the graph on `nodes` whose edges `next` gives (to nodes among them), each
/// node in exactly one (Tarjan's algorithm, iterative: no recursion however long a chain).
fn strongly_connected(nodes: &[Entity], next: &dyn Fn(Entity) -> Vec<Entity>) -> Vec<Vec<Entity>> {
    let mut s = Tarjan::default();
    for &start in nodes {
        if s.index.contains_key(&start) {
            continue;
        }
        s.visit(start, next);
        while let Some(top) = s.calls.len().checked_sub(1) {
            let v = s.calls[top].0;
            if let Some(&u) = s.calls[top].1.get(s.calls[top].2) {
                s.calls[top].2 += 1;
                if !s.index.contains_key(&u) {
                    s.visit(u, next);
                } else if s.on_stack.contains(&u) {
                    let m = s.low[&v].min(s.index[&u]);
                    s.low.insert(v, m);
                }
                continue;
            }
            s.calls.pop();
            if let Some(&(parent, _, _)) = s.calls.last() {
                let m = s.low[&parent].min(s.low[&v]);
                s.low.insert(parent, m);
            }
            if s.low[&v] == s.index[&v] {
                let mut component = Vec::new();
                while let Some(x) = s.stack.pop() {
                    s.on_stack.remove(&x);
                    component.push(x);
                    if x == v {
                        break;
                    }
                }
                s.out.push(component);
            }
        }
    }
    s.out
}

/// [`strongly_connected`]'s state: each node's visit order and lowest reachable order, the nodes on the component
/// stack, the search's own stack (each node, its successors and the next one to look at), the components found.
#[derive(Default)]
struct Tarjan {
    index: BTreeMap<Entity, usize>,
    low: BTreeMap<Entity, usize>,
    on_stack: BTreeSet<Entity>,
    stack: Vec<Entity>,
    calls: Vec<(Entity, Vec<Entity>, usize)>,
    out: Vec<Vec<Entity>>,
}

impl Tarjan {
    fn visit(&mut self, v: Entity, next: &dyn Fn(Entity) -> Vec<Entity>) {
        let n = self.index.len();
        self.index.insert(v, n);
        self.low.insert(v, n);
        self.stack.push(v);
        self.on_stack.insert(v);
        self.calls.push((v, next(v), 0));
    }
}

/// `partition-grid`'s declaration ([`primase_core::pass::PassRegistry::add`]): how big a part is is the program's,
/// `cell` has no default.
pub const PARTITION_GRID: PassSpec = PassSpec {
    name: "partition-grid",
    options: &[
        OptionSpec::required("cell", "100", "metres: a cell's side, above 0"),
        OptionSpec::optional(
            "origin",
            "0;0",
            "metres: the cells' origin, east;north (default 0;0)",
        ),
        OptionSpec::optional(
            "halo",
            "5",
            "metres, not negative: other cells' content within it is a part's context (default 0)",
        ),
        OptionSpec::optional(
            "region",
            "0;0;100;100",
            "metres: west;south;east;north, only what stands in it kept (default everything)",
        ),
        OptionSpec::optional(
            "tile",
            "100",
            "metres: cells aligned to a writer's tiles of this side, content filed by its tile",
        ),
    ],
    make: |g| Ok(Box::new(PartitionGrid::from_given(g)?)),
};

impl Pass for PartitionGrid {
    fn name(&self) -> &'static str {
        "partition-grid"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([("cell".to_owned(), self.cell.to_string())]);
        if self.origin != [0.0; 2] {
            o.insert("origin".into(), format!("{};{}", self.origin[0], self.origin[1]));
        }
        if self.halo > 0.0 {
            o.insert("halo".into(), self.halo.to_string());
        }
        if let Some(r) = self.region {
            o.insert("region".into(), format!("{};{};{};{}", r[0], r[1], r[2], r[3]));
        }
        if let Some(t) = self.tile {
            o.insert("tile".into(), t.to_string());
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // independent content: what nothing refers to (entities with no component are nothing)
        let names = w.component_names();
        let has_any = |e: Entity| names.iter().any(|n| w.entity_has_named(e, n));
        let mut content: Vec<Entity> = w.unreferenced().into_iter().filter(|&e| has_any(e)).collect();
        // content only reachable from itself (a reference cycle, a self-reference, and what only they reach) has no
        // unreferenced root. Among what no root covers (nothing covered refers to it, or it would be covered), its
        // strongly connected components condensed: a component nothing outside it refers to is a root of its own, at
        // its smallest entity with a component; every other uncovered entity is reached from one, so it is in a
        // root's closure, never a root itself (an entity a cycle reaches is not rooted before the cycle)
        let covered = w.closure(content.iter().copied());
        let uncovered: Vec<Entity> = w.entities().filter(|e| !covered.contains(e)).collect();
        let inside: BTreeSet<Entity> = uncovered.iter().copied().collect();
        let next = |e: Entity| -> Vec<Entity> {
            let mut r: Vec<Entity> = w
                .references(e)
                .into_iter()
                .filter(|r| inside.contains(r))
                .collect();
            r.sort_unstable();
            r.dedup();
            r
        };
        let components = strongly_connected(&uncovered, &next);
        let mut component_of: BTreeMap<Entity, usize> = BTreeMap::new();
        for (i, c) in components.iter().enumerate() {
            component_of.extend(c.iter().map(|&e| (e, i)));
        }
        let mut entered = vec![false; components.len()];
        for &e in &uncovered {
            for r in next(e) {
                if component_of[&r] != component_of[&e] {
                    entered[component_of[&r]] = true;
                }
            }
        }
        let mut cycles = 0u64;
        for (c, _) in components.iter().zip(&entered).filter(|(_, entered)| !**entered) {
            if let Some(&e) = c.iter().filter(|&&e| has_any(e)).min() {
                content.push(e);
                cycles += 1;
            }
        }
        if cycles > 0 {
            log.count(
                "content reached only from itself (reference cycles), rooted at its smallest entity",
                cycles,
            );
        }
        let mut cells: BTreeMap<(i64, i64), BTreeSet<Entity>> = BTreeMap::new();
        let mut at: Vec<(Entity, [f64; 2])> = Vec::new();
        let mut unplaced = BTreeSet::new();
        for e in content {
            let Some(p) = position(w, e) else {
                unplaced.insert(e);
                continue;
            };
            at.push((e, p));
            if let Some(r) = self.region
                && !(p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3])
            {
                log.count("content outside the region, left out", 1);
                continue;
            }
            cells
                .entry(
                    self.cell_of(p, w.get::<Tile>(e).copied())
                        .map_err(|m| format!("{e:?}: {m}"))?,
                )
                .or_default()
                .insert(e);
        }
        let mut groups = Vec::new();
        let mut shared = 0;
        let buckets = Buckets::of(self, &at);
        for (&c, roots) in &cells {
            let context: BTreeSet<Entity> = buckets
                .near(self, c)
                .filter(|&k| !roots.contains(&at[k].0) && self.near(c, at[k].1))
                .map(|k| at[k].0)
                .collect();
            shared += context.len();
            let (w0, s0) = (
                self.origin[0] + c.0 as f64 * self.cell,
                self.origin[1] + c.1 as f64 * self.cell,
            );
            groups.push(PartitionGroup {
                key: format!("cell {},{}", c.0, c.1),
                roots: roots.clone(),
                context,
                area: Some([
                    w0 - self.halo,
                    s0 - self.halo,
                    w0 + self.cell + self.halo,
                    s0 + self.cell + self.halo,
                ]),
            });
        }
        if !unplaced.is_empty() {
            log.count("content with no position, in its own part", unplaced.len() as u64);
            groups.push(PartitionGroup {
                key: "unplaced".into(),
                roots: unplaced,
                context: BTreeSet::new(),
                area: None,
            });
        }
        log.count("parts", groups.len() as u64);
        log.count("context entries loaded by a neighbouring part", shared as u64);
        w.insert_resource(PartitionPlan {
            by: format!("partition-grid{{cell={}}}", self.cell),
            groups,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::Instance;

    fn placement(w: &mut World, proto: Entity, east: f32, north: f32) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -north];
        w.insert(e, t);
        e
    }

    fn grid(o: &[(&str, &str)]) -> Box<dyn Pass> {
        PARTITION_GRID.instance(&options(o)).unwrap()
    }

    fn options(o: &[(&str, &str)]) -> Options {
        o.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Half-open cells: a placement on a border is the cell above's; the halo loads a neighbour's placement as
    /// context; a region leaves out what stands outside it, counted.
    #[test]
    fn cells_are_half_open_and_halos_load_neighbours() {
        let mut w = World::new();
        let proto = w.spawn();
        let a = placement(&mut w, proto, 10.0, 10.0);
        let border = placement(&mut w, proto, 100.0, 10.0);
        let b = placement(&mut w, proto, 101.0, 10.0);
        let far = placement(&mut w, proto, 500.0, 10.0);
        let mut log = PassLog::default();
        grid(&[("cell", "100"), ("halo", "5")])
            .run(&mut w, &mut log)
            .unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap();
        let keys: Vec<&str> = plan.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["cell 0,0", "cell 1,0", "cell 5,0"]);
        assert_eq!(plan.groups[0].roots, BTreeSet::from([a]));
        assert_eq!(plan.groups[1].roots, BTreeSet::from([border, b]));
        // within 5 m of cell 0,0's east border
        assert_eq!(plan.groups[0].context, BTreeSet::from([border, b]));
        assert_eq!(plan.groups[1].context, BTreeSet::new());
        assert_eq!(plan.groups[2].roots, BTreeSet::from([far]));
        let mut w2 = World::new();
        let proto = w2.spawn();
        placement(&mut w2, proto, 10.0, 10.0);
        placement(&mut w2, proto, 150.0, 10.0);
        let mut log = PassLog::default();
        grid(&[("cell", "100"), ("region", "0;0;100;100")])
            .run(&mut w2, &mut log)
            .unwrap();
        assert_eq!(w2.resource::<PartitionPlan>().unwrap().groups.len(), 1);
        assert_eq!(log.counts["content outside the region, left out"], 1);
    }

    /// Content no unreferenced entity reaches (an isolated two-entity cycle, a self-reference) is rooted once, at its
    /// smallest entity, so the parts cover exactly what the whole world holds, each entity in one part's roots'
    /// closure; a cycle a normal root already reaches gets no root of its own, and a shared prototype stays shared.
    #[test]
    fn reference_cycles_are_rooted_once_and_nothing_is_left_out() {
        let mut w = World::new();
        let proto = w.spawn();
        let a = placement(&mut w, proto, 10.0, 10.0);
        let b = placement(&mut w, proto, 150.0, 10.0);
        // an isolated two-entity cycle: each places the other
        let (c1, c2) = (w.spawn(), w.spawn());
        for (e, other, x) in [(c1, c2, 20.0f32), (c2, c1, 30.0)] {
            w.insert(
                e,
                Instance {
                    prototype: other,
                    variant: 0,
                },
            );
            let mut t = Transform::IDENTITY;
            t.cols[3] = [x, 0.0, -10.0];
            w.insert(e, t);
        }
        // a self-reference, unplaced
        let selfish = w.spawn();
        w.insert(
            selfish,
            Instance {
                prototype: selfish,
                variant: 0,
            },
        );
        // a cycle a normal placement already reaches: a places r1, r1 and r2 place each other
        let (r1, r2) = (w.spawn(), w.spawn());
        w.insert(
            r1,
            Instance {
                prototype: r2,
                variant: 0,
            },
        );
        w.insert(
            r2,
            Instance {
                prototype: r1,
                variant: 0,
            },
        );
        let reaching = placement(&mut w, r1, 12.0, 10.0);
        let mut log = PassLog::default();
        grid(&[("cell", "100")]).run(&mut w, &mut log).unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap().clone();
        // two cycles needed a root of their own: {c1, c2} and {selfish}
        assert_eq!(
            log.counts["content reached only from itself (reference cycles), rooted at its smallest entity"],
            2
        );
        let roots: Vec<Entity> = plan.groups.iter().flat_map(|g| g.roots.iter().copied()).collect();
        let unique: BTreeSet<Entity> = roots.iter().copied().collect();
        assert_eq!(unique.len(), roots.len(), "a root in two parts: {roots:?}");
        assert!(
            unique.contains(&c1) && !unique.contains(&c2),
            "the cycle rooted at its smallest entity"
        );
        assert!(unique.contains(&selfish));
        assert!(
            !unique.contains(&r1) && !unique.contains(&r2),
            "reached from a normal root"
        );
        // what the parts write together is what the whole world holds
        let covered: BTreeSet<Entity> = plan
            .groups
            .iter()
            .flat_map(|g| w.closure(g.roots.iter().copied()))
            .collect();
        // (the whole world's conversion writes the closure of every entity: all of them here)
        let whole: BTreeSet<Entity> = w.closure(w.entities());
        assert_eq!(covered, whole);
        for e in [a, b, c1, c2, selfish, r1, r2, reaching, proto] {
            assert!(covered.contains(&e), "{e:?}");
        }
        assert!(
            plan.groups
                .iter()
                .any(|g| g.key == "unplaced" && g.roots.contains(&selfish))
        );
    }

    /// The typed constructor and the text refuse the same values: no zero, negative or NaN cell, no negative halo,
    /// no empty region.
    #[test]
    fn typed_and_text_construction_check_the_same() {
        assert!(PartitionGrid::new(0.0).is_err());
        assert!(PartitionGrid::new(-5.0).is_err());
        assert!(PartitionGrid::new(f64::NAN).is_err());
        let g = PartitionGrid::new(100.0).unwrap();
        assert!(g.clone().with_halo(-1.0).is_err());
        assert!(g.clone().with_region([10.0, 0.0, 10.0, 5.0]).is_err());
        assert_eq!(grid(&[("cell", "100")]).options(), g.options());
        assert!(PARTITION_GRID.instance(&options(&[("cell", "NaN")])).is_err());
    }

    #[test]
    fn the_cell_size_is_required() {
        assert!(PARTITION_GRID.instance(&Options::new()).is_err());
        assert!(PARTITION_GRID.instance(&options(&[("cell", "0")])).is_err());
    }

    /// Standalone content (a collision soup nothing places, with or without a transform) is in exactly one part, as
    /// placements are: every piece of independent content of the world, none twice, none lost.
    #[test]
    fn every_piece_of_independent_content_is_in_one_part() {
        use primase_core::Collision;
        let soup = |w: &mut World, at: Option<[f32; 3]>| {
            let e = w.spawn();
            w.insert(
                e,
                Collision {
                    positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                    triangles: vec![[0, 1, 2]],
                    surfaces: vec![0],
                },
            );
            if let Some(p) = at {
                let mut t = Transform::IDENTITY;
                t.cols[3] = p;
                w.insert(e, t);
            }
            e
        };
        let parted = |w: &mut World| {
            let mut log = PassLog::default();
            grid(&[("cell", "100")]).run(w, &mut log).unwrap();
            let plan = w.resource::<PartitionPlan>().unwrap();
            let mut all: Vec<Entity> = plan.groups.iter().flat_map(|g| g.roots.iter().copied()).collect();
            all.sort();
            all
        };
        // collision only: one soup placed at a transform, one with none
        let mut w = World::new();
        let placed = soup(&mut w, Some([150.0, 0.0, -10.0]));
        let world_soup = soup(&mut w, None);
        assert_eq!(parted(&mut w), vec![placed, world_soup]);
        let plan = w.resource::<PartitionPlan>().unwrap();
        assert_eq!(plan.groups.last().unwrap().key, "unplaced");
        // mixed: placements with their prototype (reached, not a root of its own) and a soup
        let mut w = World::new();
        let proto = w.spawn();
        w.insert(proto, primase_core::Name("proto".into()));
        let a = placement(&mut w, proto, 10.0, 10.0);
        let b = placement(&mut w, proto, 210.0, 10.0);
        let s = soup(&mut w, None);
        assert_eq!(parted(&mut w), vec![a, b, s]);
        let whole: Vec<Entity> = w.unreferenced().into_iter().filter(|e| *e != proto).collect();
        assert!(whole.iter().all(|e| [a, b, s].contains(e)));
    }

    /// A part's context reads a neighbour the region leaves out of the output.
    #[test]
    fn context_reads_neighbours_outside_the_region() {
        let mut w = World::new();
        let proto = w.spawn();
        let inside = placement(&mut w, proto, 98.0, 10.0);
        let outside = placement(&mut w, proto, 102.0, 10.0);
        let mut log = PassLog::default();
        grid(&[("cell", "100"), ("halo", "5"), ("region", "0;0;100;100")])
            .run(&mut w, &mut log)
            .unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].roots, BTreeSet::from([inside]));
        assert_eq!(plan.groups[0].context, BTreeSet::from([outside]));
        assert_eq!(plan.groups[0].area, Some([-5.0, -5.0, 105.0, 105.0]));
        assert_eq!(log.counts["content outside the region, left out"], 1);
    }

    /// A raw form a target refuses, lowered by a pass that needs a partition plan.
    #[derive(Clone, Debug, PartialEq)]
    struct Raw;
    impl primase_core::Component for Raw {
        const NAME: &'static str = "test.raw";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    }
    struct LowerRaw;
    impl Pass for LowerRaw {
        fn name(&self) -> &'static str {
            "lower-raw"
        }
        fn requires(&self) -> &[&'static str] {
            &["test.raw", PartitionPlan::NAME]
        }
        fn removes(&self) -> &[&'static str] {
            &["test.raw"]
        }
        fn legalizes(&self) -> &[&'static str] {
            &["test.raw"]
        }
        fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
            let raw: Vec<Entity> = w.iter::<Raw>().map(|(e, _)| e).collect();
            for e in raw {
                w.remove::<Raw>(e);
            }
            Ok(())
        }
    }

    /// The planner never runs a required-option pass on its registered example: a plan that needs partition-grid is
    /// an error without the caller's cell size, and uses the caller's when given.
    #[test]
    fn the_planner_needs_the_callers_options_for_a_required_option_pass() {
        use primase_core::pass::{Mode, PassManager, PassOptions, PassRegistry, RunReport, Target};
        let mut reg = PassRegistry::new();
        crate::register(&mut reg);
        reg.register("lower-raw", |_| Ok(Box::new(LowerRaw)));
        let manager = PassManager::new(reg);
        let target = Target::new("t").illegal("test.raw").carry_unknown();
        let mut w = World::new();
        let proto = w.spawn();
        let e = placement(&mut w, proto, 10.0, 10.0);
        w.insert(e, Raw);
        let plan = |options: &PassOptions| {
            manager.plan(&w, &target, Mode::Partial, options, &mut RunReport::default())
        };
        let e = plan(&PassOptions::new()).map(|p| p.to_string()).unwrap_err();
        assert!(e.to_string().contains("partition-grid"), "{e}");
        let given = PassOptions::from([(
            "partition-grid".to_owned(),
            Options::from([("cell".to_owned(), "250".to_owned())]),
        )]);
        let p = plan(&given).unwrap().to_string();
        assert!(p.contains("partition-grid{cell=250}"), "{p}");
    }

    /// Every option, the lists included, survives the pipeline text: options are separated by commas, so a list's
    /// numbers by semicolons.
    #[test]
    fn every_option_round_trips_through_the_pipeline_text() {
        let g = PartitionGrid::new(400.0)
            .unwrap()
            .with_tile(200.0)
            .unwrap()
            .with_origin([-200.0, 200.0])
            .unwrap()
            .with_halo(37.5)
            .unwrap()
            .with_region([-1000.0, -600.0, 1400.0, 2000.0])
            .unwrap();
        let text = primase_core::pass::pass_text(&g);
        assert_eq!(
            text,
            "partition-grid{cell=400,halo=37.5,origin=-200;200,region=-1000;-600;1400;2000,tile=200}"
        );
        let mut reg = primase_core::pass::PassRegistry::new();
        crate::register(&mut reg);
        let parsed = reg.parse(&text).unwrap();
        assert_eq!(parsed.passes.len(), 1);
        assert_eq!(parsed.passes[0].options(), g.options());
    }

    /// Aligned to a writer's 200 m tiles: a cell or origin that is not a whole multiple of the tile is refused; content
    /// is filed by the tile it stands in, by the writers' own rule, so a cell holds whole tiles (on both sides of every
    /// border, in f32 as a writer computes it), and a placement stating its tile is filed by that tile.
    #[test]
    fn aligned_cells_hold_whole_tiles_by_the_writers_rule() {
        for bad in [
            &[("cell", "300"), ("tile", "200")][..],
            &[("cell", "400"), ("tile", "200"), ("origin", "100;0")],
            &[("cell", "400"), ("tile", "0.1")],
            &[("cell", "400"), ("tile", "0")],
        ] {
            assert!(PARTITION_GRID.instance(&options(bad)).is_err(), "{bad:?}");
        }
        assert!(PartitionGrid::new(300.0).unwrap().with_tile(200.0).is_err());
        assert!(
            PartitionGrid::new(400.0)
                .unwrap()
                .with_tile(200.0)
                .unwrap()
                .with_origin([100.0, 0.0])
                .is_err()
        );
        let g = PartitionGrid::new(400.0)
            .unwrap()
            .with_tile(200.0)
            .unwrap()
            .with_origin([-200.0, 200.0])
            .unwrap();
        assert_eq!(
            grid(&[("cell", "400"), ("tile", "200"), ("origin", "-200;200")]).options(),
            g.options()
        );
        // every f32 position within a few steps of a tile border, both sides, both signs
        let mut w = World::new();
        let proto = w.spawn();
        let mut placed = Vec::new();
        for border in [-600.0f32, -200.0, 0.0, 200.0, 600.0, 1000.0, 25_600.0] {
            let mut x = border;
            for _ in 0..4 {
                x = f32::from_bits(if x > 0.0 { x.to_bits() - 1 } else { x.to_bits() + 1 });
            }
            for _ in 0..8 {
                placed.push((placement(&mut w, proto, x, x), x));
                x = if x == 0.0 {
                    f32::from_bits(1)
                } else if x > 0.0 {
                    f32::from_bits(x.to_bits() + 1)
                } else {
                    f32::from_bits(x.to_bits() - 1)
                };
            }
        }
        // a placement standing in one tile and stating another
        let stated = placement(&mut w, proto, 10.0, 10.0);
        w.insert(stated, Tile { col: 5, row: -3 });
        g.run(&mut w, &mut PassLog::default()).unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap();
        let cell_of = |e: Entity| {
            plan.groups
                .iter()
                .find(|g| g.roots.contains(&e))
                .map(|g| g.key.clone())
                .unwrap()
        };
        // the cell of tile (c, r): tiles from the origin's (-1, 1), two to a cell
        let cell_of_tile = |t: Tile| {
            format!(
                "cell {},{}",
                (i64::from(t.col) + 1).div_euclid(2),
                (i64::from(t.row) - 1).div_euclid(2)
            )
        };
        for (e, x) in placed {
            assert_eq!(
                cell_of(e),
                cell_of_tile(Tile::of([x, x], 200.0).unwrap()),
                "east = north = {x:e}"
            );
        }
        assert_eq!(cell_of(stated), cell_of_tile(Tile { col: 5, row: -3 }));
        assert_eq!(cell_of(stated), "cell 3,-2");
    }

    /// Refers to the entities it holds.
    #[derive(Clone, Debug, PartialEq)]
    struct Refers(Vec<Entity>);
    impl primase_core::Component for Refers {
        const NAME: &'static str = "test.refers";
        const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
        fn references(&self, out: &mut Vec<Entity>) {
            out.extend(&self.0);
        }
    }

    /// A cycle that reaches more (e1 -> e2, e2 -> e1, e2 -> e0, e0 the smallest id, nothing else referring to them) is
    /// one root's: its component nothing outside refers to is rooted at its smallest entity (e1), and e0, reached from
    /// it, is no root of its own; no entity is in two roots' closures, and nothing is left out.
    #[test]
    fn content_a_cycle_reaches_is_not_rooted_before_the_cycle() {
        let mut w = World::new();
        let (e0, e1, e2) = (w.spawn(), w.spawn(), w.spawn());
        w.insert(e0, Refers(Vec::new()));
        w.insert(e1, Refers(vec![e2]));
        w.insert(e2, Refers(vec![e1, e0]));
        let mut log = PassLog::default();
        grid(&[("cell", "100")]).run(&mut w, &mut log).unwrap();
        let plan = w.resource::<PartitionPlan>().unwrap().clone();
        let roots: Vec<Entity> = plan.groups.iter().flat_map(|g| g.roots.iter().copied()).collect();
        assert_eq!(roots, [e1], "{roots:?}");
        assert_eq!(
            log.counts["content reached only from itself (reference cycles), rooted at its smallest entity"],
            1
        );
        // no entity owned by two parts, nothing left out
        let mut owned: BTreeMap<Entity, usize> = BTreeMap::new();
        for g in &plan.groups {
            for e in w.closure(g.roots.iter().copied()) {
                *owned.entry(e).or_default() += 1;
            }
        }
        assert_eq!(owned, BTreeMap::from([(e0, 1), (e1, 1), (e2, 1)]));
    }
    /// A cell finds the content within its halo from the cells around it: the same content a scan of all of it finds, for
    /// halos smaller than, equal to and many times a cell, and none without a halo.
    #[test]
    fn the_content_near_a_cell_is_found_from_the_cells_around_it() {
        let mut w = World::new();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 20_000) as f64 / 100.0 - 100.0
        };
        let at: Vec<(Entity, [f64; 2])> = (0..300).map(|_| (w.spawn(), [next(), next()])).collect();
        for (cell, halo) in [
            (10.0, 3.0),
            (10.0, 10.0),
            (10.0, 25.0),
            (3.0, 100.0),
            (7.5, 0.5),
            (50.0, 1e9),
            (10.0, 0.0),
        ] {
            let grid = PartitionGrid::new(cell).unwrap().with_halo(halo).unwrap();
            let buckets = Buckets::of(&grid, &at);
            let cells: BTreeSet<(i64, i64)> = at.iter().map(|(_, p)| grid.cell_at(*p)).collect();
            for c in cells {
                let brute: BTreeSet<usize> = (0..at.len()).filter(|&k| grid.near(c, at[k].1)).collect();
                let found: BTreeSet<usize> = buckets
                    .near(&grid, c)
                    .filter(|&k| grid.near(c, at[k].1))
                    .collect();
                assert_eq!(found, brute, "cell {cell}, halo {halo}, cell {c:?}");
            }
        }
    }
}
