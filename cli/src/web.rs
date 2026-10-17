//! `--to web`: web tiles for the viewer ([`primase_web::TilePlan`]): planned from the selected world, then the
//! placements filed by triangle centre (their prototype loaded, to file their triangles), then cell by cell, each
//! cell's world written by every tile set. Writes `<out>` (compressed, instanced) and, with `--plain`, `<out>/plain`
//! (the plain comparison set, for gates).
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use primase_core::pass::Target;
use primase_core::{Entity, Instance, World};
use primase_web::{Options, TilePlan};

use crate::convert::{Result, WriteReport, Writer};

/// The part [`Writer::next_part`] last gave.
enum Asked {
    /// A placement filed by triangle centre, its prototype loaded.
    Split(Entity),
    Cell((i32, i32)),
}

struct Web {
    /// Each tile set's options and folder: the first is the viewer's.
    sets: Vec<(Options, PathBuf)>,
    plans: Vec<TilePlan>,
    /// The placements filed by triangle centre, with their prototypes, still to split.
    unsplit: Vec<(Entity, Entity)>,
    cells: Option<std::vec::IntoIter<(i32, i32)>>,
    asked: Option<Asked>,
    bytes: Vec<usize>,
    report: WriteReport,
}

/// `[--ktx2] [--budget-mib N | --no-budget] [--plain]`
pub fn writer(out: &Path, args: &[String]) -> Result<Box<dyn Writer>> {
    let (mut options, mut plain) = (Options::default(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("web: {a} needs a value"));
        match a.as_str() {
            "--ktx2" => {
                #[cfg(feature = "ktx2")]
                {
                    options.ktx2 = true;
                }
                #[cfg(not(feature = "ktx2"))]
                return Err("web: --ktx2 needs primase built with the ktx2 feature".into());
            }
            "--budget-mib" => {
                let mib: u64 = value()?.parse().map_err(|_| "web: --budget-mib is a number")?;
                options.tile_byte_budget = Some(mib * 1024 * 1024);
            }
            "--no-budget" => options.tile_byte_budget = None,
            "--plain" => plain = true,
            _ => return Err(format!("web: unknown option {a}").into()),
        }
    }
    let mut sets = vec![(options.clone(), out.to_path_buf())];
    if plain {
        let plain = Options {
            compress: false,
            gpu_instancing: false,
            #[cfg(feature = "ktx2")]
            ktx2: false,
            ..options
        };
        sets.push((plain, out.join("plain")));
    }
    Ok(Box::new(Web {
        bytes: vec![0; sets.len()],
        sets,
        plans: Vec::new(),
        unsplit: Vec::new(),
        cells: None,
        asked: None,
        report: WriteReport::new(),
    }))
}

impl Writer for Web {
    /// The tile writer writes what glTF expresses and carries every other component with a JSON form in the
    /// extras, so it takes any name; the check is the verifier's.
    fn target(&self) -> Target {
        Target::new("web").carry_unknown()
    }
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        let spans = primase_web::spanning(world, self.sets[0].0.tile_size, |p| Ok(boxes(p)))?;
        // the prototypes with geometry before it is loaded: those with a box
        let prototypes: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let boxed: BTreeSet<Entity> = prototypes.into_iter().filter(|&p| boxes(p).is_some()).collect();
        for (options, _) in &mut self.sets {
            options.split_by_tile.extend(spans.marked.iter().copied());
            self.plans
                .push(primase_web::plan_with_geometry(world, options, &|p| {
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
    fn finish(self: Box<Self>) -> Result<WriteReport> {
        let Web {
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
