//! [`TileWriter`]: web tiles through the convert loop ([`primase_core::convert`]), one part in memory at a time.
use std::collections::BTreeSet;
use std::path::PathBuf;

use primase_core::convert::{Report, Result, Writer};
use primase_core::pass::Target;
use primase_core::{Entity, Instance, World};

use crate::{Options, TilePlan};

/// The part [`Writer::next_part`] last gave.
enum Asked {
    /// A placement filed by triangle centre, its prototype loaded.
    Split(Entity),
    Cell((i32, i32)),
}

/// The web tile writer for [`primase_core::convert`]: plans from the selected world, asks for the placements filed
/// by triangle centre first (each with its prototype, to file its triangles), then for each cell, writes every
/// cell's world into every tile set, and publishes each set's index at the end. The first set is the viewer's.
pub struct TileWriter {
    /// Each tile set's options and folder: the first is the viewer's.
    sets: Vec<(Options, PathBuf)>,
    plans: Vec<TilePlan>,
    /// The placements filed by triangle centre, with their prototypes, still to split.
    unsplit: Vec<(Entity, Entity)>,
    cells: Option<std::vec::IntoIter<(i32, i32)>>,
    asked: Option<Asked>,
    bytes: Vec<usize>,
    report: Report,
}

impl std::fmt::Debug for TileWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileWriter")
            .field("sets", &self.sets.len())
            .field("unsplit", &self.unsplit.len())
            .finish()
    }
}

impl TileWriter {
    /// Tile sets, each its options and folder.
    pub fn new(sets: Vec<(Options, PathBuf)>) -> Self {
        TileWriter {
            bytes: vec![0; sets.len()],
            sets,
            plans: Vec::new(),
            unsplit: Vec::new(),
            cells: None,
            asked: None,
            report: Report::new(),
        }
    }
}

impl Writer for TileWriter {
    /// [`crate::target`]: what a cell's world may hold.
    fn target(&self) -> Target {
        crate::target()
    }
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        let spans = crate::spanning(world, self.sets[0].0.tile_size, |p| Ok(boxes(p)))?;
        // the prototypes with geometry before it is loaded: those with a box
        let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let boxed: BTreeSet<Entity> = prototypes.into_iter().filter(|&p| boxes(p).is_some()).collect();
        for (options, _) in &mut self.sets {
            options.split_by_tile.extend(spans.marked.iter().copied());
            self.plans.push(crate::plan_with_geometry(world, options, &|p| {
                boxed.contains(&p)
            })?);
        }
        self.unsplit = self.plans[0]
            .unsplit()
            .into_iter()
            .map(|e| (e, world.get::<Instance>(e).map_or(e, |i| i.prototype)))
            .collect();
        self.unsplit.reverse();
        self.report.push((
            "spanning".into(),
            format!("{spans:?}, filed by triangle centre {}", self.unsplit.len()),
        ));
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>> {
        if let Some((e, proto)) = self.unsplit.pop() {
            self.asked = Some(Asked::Split(e));
            return Ok(Some(BTreeSet::from([e, proto])));
        }
        let cells = self
            .cells
            .get_or_insert_with(|| self.plans[0].cells().into_iter());
        Ok(cells.next().map(|c| {
            self.asked = Some(Asked::Cell(c));
            self.plans[0].roots(c)
        }))
    }
    fn write(&mut self, part: &World) -> Result<()> {
        match self.asked.take().ok_or("web: write without a part asked for")? {
            Asked::Split(e) => {
                for plan in &mut self.plans {
                    plan.split(part, e)?;
                }
            }
            Asked::Cell(c) => {
                for (k, plan) in self.plans.iter_mut().enumerate() {
                    for tile in plan.write_cell(c, part)? {
                        tile.write_to(&self.sets[k].1)?;
                        self.bytes[k] += tile.bytes.len();
                    }
                }
            }
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Report> {
        let TileWriter {
            sets,
            plans,
            bytes,
            mut report,
            ..
        } = *self;
        let mut indexes = Vec::new();
        for (plan, (_, out)) in plans.into_iter().zip(&sets) {
            for o in plan.over_budget() {
                report.push((
                    "over the budget".into(),
                    format!("{}/{} ({} bytes): {}", out.display(), o.file, o.bytes, o.reason),
                ));
            }
            let index = plan.finish()?;
            index.write_to(out)?;
            indexes.push(index);
        }
        let index = &indexes[0];
        report.push((
            "web".into(),
            format!(
                "tiles={} triangles={} placements={} instanced={} lights={} bytes={bytes:?}",
                index.tiles.len(),
                index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
                index.tiles.iter().map(|t| t.placements).sum::<u64>(),
                index.tiles.iter().map(|t| t.instanced_placements).sum::<u64>(),
                index.tiles.iter().map(|t| t.lights).sum::<u64>(),
            ),
        ));
        Ok(report)
    }
}
