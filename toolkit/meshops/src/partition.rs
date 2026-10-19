//! `partition-grid`: divide a world's content into output parts by a ground grid, as a [`PartitionPlan`] the
//! conversion loop takes its parts from ([`primase_core::convert::PlannedParts`]).
//!
//! Every piece of independent content (an entity nothing else refers to: a placement, a marker, a curve, a light, a
//! standalone collision soup or mesh) is in exactly one part; what it reaches (prototypes, materials, textures) comes
//! with it. Content is filed by where it stands: a marker by its ground position, a curve by the middle of its points'
//! bounds, anything else with a transform by its position, in the ground frame (`east`, `north`:
//! [`primase_core::frame::ground`]). Cells are half-open, `[origin + k * cell, origin + (k + 1) * cell)` on each axis,
//! so a point on a border belongs to the cell above it. Content with no position (a world soup with no transform) is
//! its own last part, `unplaced`. A part's context is the content of other cells standing within `halo` metres of its
//! cell, outside the region too (an operation may need to read a neighbour the region leaves out); the part's area
//! (its cell and halo) lets [`primase_core::convert::PlannedParts`] add what reaches it by its authored box.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{PartitionGroup, PartitionPlan};
use primase_core::frame::ground;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Curve, Entity, Marker, Resource, Transform, World};

/// `partition-grid{cell=C[,origin=E,N][,halo=H][,region=E0,N0,E1,N1]}`: see the module documentation. `cell` has no
/// default: how big a part is is the program's choice. `region` keeps only what stands in it (half-open), the rest
/// left out and counted. Built by [`PartitionGrid::new`] and its setters, which check every value; the pipeline text
/// goes through the same constructor ([`PartitionGrid::from_options`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionGrid {
    cell: f64,
    origin: [f64; 2],
    halo: f64,
    region: Option<[f64; 4]>,
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
        })
    }

    /// The cells' origin `[east, north]`, metres.
    pub fn with_origin(mut self, origin: [f64; 2]) -> Result<PartitionGrid, String> {
        self.origin = [
            finite("origin east", origin[0])?,
            finite("origin north", origin[1])?,
        ];
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

    /// From the pipeline text's options, through [`PartitionGrid::new`] and the setters.
    pub fn from_options(o: &Options) -> Result<PartitionGrid, String> {
        let numbers = |k: &str, v: &str, n: usize| -> Result<Vec<f64>, String> {
            let xs: Vec<f64> = v
                .split(',')
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
        if let Some(k) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "cell" | "origin" | "halo" | "region"))
        {
            return Err(format!("unknown option {k}"));
        }
        let cell = o.get("cell").ok_or("cell=C (metres, above 0) is required")?;
        let mut grid = PartitionGrid::new(numbers("cell", cell, 1)?[0])?;
        if let Some(v) = o.get("origin") {
            let xs = numbers("origin", v, 2)?;
            grid = grid.with_origin([xs[0], xs[1]])?;
        }
        if let Some(v) = o.get("halo") {
            grid = grid.with_halo(numbers("halo", v, 1)?[0])?;
        }
        if let Some(v) = o.get("region") {
            let xs = numbers("region", v, 4)?;
            grid = grid.with_region([xs[0], xs[1], xs[2], xs[3]])?;
        }
        Ok(grid)
    }

    /// The cell `[east, north]` stands in.
    fn cell_of(&self, p: [f64; 2]) -> (i64, i64) {
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

impl Pass for PartitionGrid {
    fn name(&self) -> &'static str {
        "partition-grid"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([("cell".to_owned(), self.cell.to_string())]);
        if self.origin != [0.0; 2] {
            o.insert("origin".into(), format!("{},{}", self.origin[0], self.origin[1]));
        }
        if self.halo > 0.0 {
            o.insert("halo".into(), self.halo.to_string());
        }
        if let Some(r) = self.region {
            o.insert("region".into(), format!("{},{},{},{}", r[0], r[1], r[2], r[3]));
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
        let content: Vec<Entity> = w
            .unreferenced()
            .into_iter()
            .filter(|&e| names.iter().any(|n| w.entity_has_named(e, n)))
            .collect();
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
            cells.entry(self.cell_of(p)).or_default().insert(e);
        }
        let mut groups = Vec::new();
        let mut shared = 0;
        for (&c, roots) in &cells {
            let context: BTreeSet<Entity> = at
                .iter()
                .filter(|(e, p)| !roots.contains(e) && self.near(c, *p))
                .map(|(e, _)| *e)
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

    fn grid(o: &[(&str, &str)]) -> PartitionGrid {
        PartitionGrid::from_options(
            &o.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
        .unwrap()
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
        grid(&[("cell", "100"), ("region", "0,0,100,100")])
            .run(&mut w2, &mut log)
            .unwrap();
        assert_eq!(w2.resource::<PartitionPlan>().unwrap().groups.len(), 1);
        assert_eq!(log.counts["content outside the region, left out"], 1);
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
        assert_eq!(
            PartitionGrid::from_options(&Options::from([("cell".to_owned(), "100".to_owned())])).unwrap(),
            g
        );
        assert!(
            PartitionGrid::from_options(&Options::from([("cell".to_owned(), "NaN".to_owned())])).is_err()
        );
    }

    #[test]
    fn the_cell_size_is_required() {
        assert!(PartitionGrid::from_options(&Options::new()).is_err());
        assert!(PartitionGrid::from_options(&Options::from([("cell".to_owned(), "0".to_owned())])).is_err());
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
        grid(&[("cell", "100"), ("halo", "5"), ("region", "0,0,100,100")])
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
}
