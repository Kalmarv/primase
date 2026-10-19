//! Web tiles through the convert loop ([`primase_core::convert`]), one part in memory at a time: [`tiles`] gives the
//! part source that plans the tiles from the selected world ([`TileParts`]) and the writer that writes them
//! ([`TileWriter`]), sharing the plan.
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use primase_core::convert::{Finished, Part, PartSource, Report, Result, Writer};
use primase_core::pass::Target;
use primase_core::{Entity, Instance, World};

use crate::{Options, TilePlan};

/// The part [`TileParts::next_part`] last gave.
enum Asked {
    /// A placement filed by triangle centre, its prototype loaded.
    Split(Entity),
    Cell((i32, i32)),
}

/// What the part source and the writer share: each tile set's options, folder and plan.
struct Tiles {
    /// Each tile set's options and folder: the first is the viewer's.
    sets: Vec<(Options, PathBuf)>,
    plans: Vec<TilePlan>,
    asked: Option<Asked>,
    report: Report,
}

/// The web tiles' part source: plans every tile set from the selected world, then asks for the placements filed by
/// triangle centre first (each with its prototype, to file its triangles), then for each cell of the first set.
pub struct TileParts {
    tiles: Rc<RefCell<Tiles>>,
    /// The placements filed by triangle centre, with their prototypes, still to split.
    unsplit: Vec<(Entity, Entity)>,
    cells: Option<std::vec::IntoIter<(i32, i32)>>,
}

/// The web tile writer: writes every cell's world into every tile set, and publishes each set's index at the end.
pub struct TileWriter {
    tiles: Rc<RefCell<Tiles>>,
    bytes: Vec<usize>,
}

impl std::fmt::Debug for TileWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileWriter")
            .field("sets", &self.tiles.borrow().sets.len())
            .finish()
    }
}

impl std::fmt::Debug for TileParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileParts")
            .field("unsplit", &self.unsplit.len())
            .finish()
    }
}

/// The part source and the writer of tile sets, each its options and folder.
pub fn tiles(sets: Vec<(Options, PathBuf)>) -> (TileParts, TileWriter) {
    let bytes = vec![0; sets.len()];
    let tiles = Rc::new(RefCell::new(Tiles {
        sets,
        plans: Vec::new(),
        asked: None,
        report: Report::new(),
    }));
    (
        TileParts {
            tiles: tiles.clone(),
            unsplit: Vec::new(),
            cells: None,
        },
        TileWriter { tiles, bytes },
    )
}

impl PartSource for TileParts {
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        let mut tiles = self.tiles.borrow_mut();
        let spans = crate::spanning(world, tiles.sets[0].0.tile_size.get(), |p| Ok(boxes(p)))?;
        // the prototypes with geometry before it is loaded: those with a box
        let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let boxed: BTreeSet<Entity> = prototypes.into_iter().filter(|&p| boxes(p).is_some()).collect();
        let mut plans = Vec::new();
        for (options, _) in &mut tiles.sets {
            options.split_by_tile.extend(spans.marked.iter().copied());
            plans.push(crate::plan_with_geometry(world, options, &|p| {
                boxed.contains(&p)
            })?);
        }
        tiles.plans = plans;
        self.unsplit = tiles.plans[0]
            .unsplit()
            .into_iter()
            .map(|e| (e, world.get::<Instance>(e).map_or(e, |i| i.prototype)))
            .collect();
        self.unsplit.reverse();
        let line = format!("{spans:?}, filed by triangle centre {}", self.unsplit.len());
        tiles.report.push(("spanning".into(), line));
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        let mut tiles = self.tiles.borrow_mut();
        if let Some((e, proto)) = self.unsplit.pop() {
            tiles.asked = Some(Asked::Split(e));
            return Ok(Some(Part {
                roots: BTreeSet::from([e, proto]),
                context: BTreeSet::new(),
                whole: false,
                // no context loaded: it states no area
                area: None,
            }));
        }
        let cells = self
            .cells
            .get_or_insert_with(|| tiles.plans[0].cells().into_iter());
        Ok(match cells.next() {
            Some(c) => {
                tiles.asked = Some(Asked::Cell(c));
                Some(Part {
                    roots: tiles.plans[0].roots(c),
                    context: BTreeSet::new(),
                    whole: false,
                    area: None,
                })
            }
            None => None,
        })
    }
    fn written(&mut self, part: &World) -> Result<()> {
        self.refine(part)
    }
}

impl TileParts {
    /// After a part: a placement's triangles filed into the plans' cells, the plan refined from its loaded geometry.
    fn refine(&mut self, part: &World) -> Result<()> {
        let mut tiles = self.tiles.borrow_mut();
        if let Some(Asked::Split(e)) = tiles.asked.take() {
            for plan in tiles.plans.iter_mut() {
                plan.split(part, e)?;
            }
        }
        Ok(())
    }
}

impl Writer for TileWriter {
    /// [`crate::target`]: what a cell's world may hold.
    fn target(&self) -> Target {
        crate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut tiles = self.tiles.borrow_mut();
        let Tiles {
            sets, plans, asked, ..
        } = &mut *tiles;
        match asked.as_ref().ok_or("web: write without a part asked for")? {
            // a placement loaded to file its triangles: planning, nothing to write ([`TileParts::written`])
            Asked::Split(_) => {}
            &Asked::Cell(c) => {
                for (k, plan) in plans.iter_mut().enumerate() {
                    for tile in plan.write_cell(c, part)? {
                        tile.write_to(&sets[k].1)?;
                        self.bytes[k] += tile.bytes.len();
                    }
                }
            }
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        let TileWriter { tiles, bytes } = *self;
        let Tiles {
            sets,
            plans,
            mut report,
            ..
        } = tiles.replace(Tiles {
            sets: Vec::new(),
            plans: Vec::new(),
            asked: None,
            report: Report::new(),
        });
        let mut indexes = Vec::new();
        let mut notices = Vec::new();
        for (plan, (_, out)) in plans.into_iter().zip(&sets) {
            // a file-size choice: the content is all written, in more tiles
            if !plan.splits().is_empty() {
                let mut n = primase_core::convert::Notice::new(
                    "web",
                    primase_core::convert::NoticeKind::Selection,
                    format!(
                        "cells split into subcells to fit the tile byte budget ({})",
                        out.display()
                    ),
                    plan.splits().len() as u64,
                );
                for split in plan.splits() {
                    n = n.example(match split.bytes {
                        Some(b) => format!("{} ({b} bytes)", split.cell),
                        None => split.cell.clone(),
                    });
                }
                notices.push(n);
            }
            #[cfg(feature = "ktx2")]
            if plan.encodings_dropped() > 0 {
                report.push((
                    "KTX2 encodings dropped from memory (made again or read from the disk cache)".into(),
                    format!("{}: {}", out.display(), plan.encodings_dropped()),
                ));
            }
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
        let index = indexes.first().ok_or("web: no tile set")?;
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
        Ok(Finished { report, notices })
    }
}
