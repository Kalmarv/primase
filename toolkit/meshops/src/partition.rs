//! `partition-grid`: divide a world's content into output parts by a ground grid, as a [`PartitionPlan`] the
//! conversion loop takes its parts from ([`primase_core::convert::PlannedParts`]).
//!
//! What is placed (placements, markers, curves, lights) is filed by where it stands: a placement, marker or light by
//! its position, a curve by the middle of its points' bounds, in the ground frame (`east`, `north`:
//! [`primase_core::frame::ground`]). Cells are half-open, `[origin + k * cell, origin + (k + 1) * cell)` on each axis,
//! so a point on a border belongs to the cell above it. Content with no position (a world soup) is its own last part.
//! A part's context is what other cells place within `halo` metres of its cell: loaded with it, written by its owner.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{PartitionGroup, PartitionPlan};
use primase_core::frame::ground;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Curve, Entity, Instance, Light, Marker, Resource, Transform, World};

/// `partition-grid{cell=C[,origin=E,N][,halo=H][,region=E0,N0,E1,N1]}`: see the module documentation. `cell` has no
/// default: how big a part is is the program's choice. `region` keeps only what stands in it (half-open), the rest
/// left out and counted.
#[derive(Clone, Debug, PartialEq)]
pub struct PartitionGrid {
    pub cell: f64,
    pub origin: [f64; 2],
    pub halo: f64,
    pub region: Option<[f64; 4]>,
}

impl PartitionGrid {
    pub fn from_options(o: &Options) -> Result<PartitionGrid, String> {
        let number = |k: &str, v: &str| -> Result<f64, String> {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| format!("{k}={v} is not a finite number"))
        };
        let numbers = |k: &str, v: &str, n: usize| -> Result<Vec<f64>, String> {
            let xs: Vec<f64> = v
                .split(',')
                .map(|x| number(k, x.trim()))
                .collect::<Result<_, _>>()?;
            if xs.len() == n {
                Ok(xs)
            } else {
                Err(format!("{k}={v}: {n} numbers expected"))
            }
        };
        let mut grid = PartitionGrid {
            cell: f64::NAN,
            origin: [0.0; 2],
            halo: 0.0,
            region: None,
        };
        for (k, v) in o {
            match k.as_str() {
                "cell" => grid.cell = number(k, v)?,
                "origin" => {
                    let xs = numbers(k, v, 2)?;
                    grid.origin = [xs[0], xs[1]];
                }
                "halo" => grid.halo = number(k, v)?,
                "region" => {
                    let xs = numbers(k, v, 4)?;
                    if !(xs[0] < xs[2] && xs[1] < xs[3]) {
                        return Err(format!(
                            "region={v}: not west,south,east,north with west < east, south < north"
                        ));
                    }
                    grid.region = Some([xs[0], xs[1], xs[2], xs[3]]);
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        // NaN (not given) or not above 0
        if grid.cell.is_nan() || grid.cell <= 0.0 {
            return Err("cell=C (metres, above 0) is required".into());
        }
        if grid.halo < 0.0 {
            return Err(format!("halo={} is negative", grid.halo));
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
        // placed content: what stands somewhere on its own (placements, markers, curves, lights)
        let placed: BTreeSet<Entity> = w
            .iter::<Instance>()
            .map(|(e, _)| e)
            .chain(w.iter::<Marker>().map(|(e, _)| e))
            .chain(w.iter::<Curve>().map(|(e, _)| e))
            .chain(w.iter::<Light>().map(|(e, _)| e))
            .collect();
        let mut cells: BTreeMap<(i64, i64), BTreeSet<Entity>> = BTreeMap::new();
        let mut at: Vec<(Entity, [f64; 2])> = Vec::new();
        let mut unplaced = BTreeSet::new();
        for &e in &placed {
            let Some(p) = position(w, e) else {
                unplaced.insert(e);
                continue;
            };
            if let Some(r) = self.region
                && !(p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3])
            {
                log.count("placed content outside the region, left out", 1);
                continue;
            }
            cells.entry(self.cell_of(p)).or_default().insert(e);
            at.push((e, p));
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
            groups.push(PartitionGroup {
                key: format!("cell {},{}", c.0, c.1),
                roots: roots.clone(),
                context,
            });
        }
        if !unplaced.is_empty() {
            log.count(
                "placed content with no position, in its own part",
                unplaced.len() as u64,
            );
            groups.push(PartitionGroup {
                key: "unplaced".into(),
                roots: unplaced,
                context: BTreeSet::new(),
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
        assert_eq!(log.counts["placed content outside the region, left out"], 1);
    }

    #[test]
    fn the_cell_size_is_required() {
        assert!(PartitionGrid::from_options(&Options::new()).is_err());
        assert!(PartitionGrid::from_options(&Options::from([("cell".to_owned(), "0".to_owned())])).is_err());
    }
}
