//! `primase_core::convert::convert_parts_gathered`: after the decisions, a gather traversal hands each part every
//! summary and decision, its contributions go to the spool, and nothing of it is written; each group the spool gives
//! is prepared on its own with the summaries and decisions, checked and written before any part; the finalizing
//! decision sees the parts' and the groups' summaries; the write traversal gets every decision. The writer's input does
//! not depend on the order the parts come in, the spool holds contributions apart from the parts (the most it holds is
//! counted), what the group and finalizing passes approximate reaches the notices, and a group summary two groups
//! make is refused.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, Gathered, NoticeKind, Part, PartScope, PartSource, Reader, Report, Result, Spool, Summarized,
    Writer, convert_parts_gathered,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::summary::{Summary, SummaryRegistry};
use primase_core::{Component, Entity, Instance, Name, Resource, Transform, World};

/// The placements a part owns, summed over the parts.
#[derive(Clone, Debug, PartialEq)]
struct Owned(u64);
impl Resource for Owned {
    const NAME: &'static str = "toy.owned";
}
impl Summary for Owned {
    fn merge(&mut self, other: Self) -> std::result::Result<(), String> {
        self.0 += other.0;
        Ok(())
    }
    fn retained_bytes(&self) -> usize {
        8
    }
}

/// The first decision: how many placements the whole holds.
#[derive(Clone, Debug, PartialEq)]
struct Total(u64);
impl Resource for Total {
    const NAME: &'static str = "toy.total";
}

/// A part's contribution to a cell: its value, made from the placement it owns.
#[derive(Clone, Debug, PartialEq)]
struct Bit {
    cell: u32,
    value: u64,
}
impl Component for Bit {
    const NAME: &'static str = "toy.bit";
}

/// A group: one cell's values, in the spool's order.
#[derive(Clone, Debug, PartialEq)]
struct Cell {
    cell: u32,
    values: Vec<u64>,
}
impl Component for Cell {
    const NAME: &'static str = "toy.cell";
}

/// Each cell's sum, one entry per group: merged as a union, a cell from two groups refused.
#[derive(Clone, Debug, PartialEq)]
struct Sums(BTreeMap<u32, u64>);
impl Resource for Sums {
    const NAME: &'static str = "toy.sums";
}
impl Summary for Sums {
    fn merge(&mut self, other: Self) -> std::result::Result<(), String> {
        for (k, v) in other.0 {
            if self.0.insert(k, v).is_some() {
                return Err(format!("cell {k} from two groups"));
            }
        }
        Ok(())
    }
    fn retained_bytes(&self) -> usize {
        16 * self.0.len()
    }
}

/// The finalizing decision: the cells' sums and the total.
#[derive(Clone, Debug, PartialEq)]
struct Final(u64);
impl Resource for Final {
    const NAME: &'static str = "toy.final";
}

/// `n` placements of their own prototypes.
struct ToyReader(usize);
impl Reader for ToyReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut w = World::new();
        for k in 0..self.0 {
            let proto = w.spawn();
            w.insert(proto, Name(format!("proto {k}")));
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(p, Transform::IDENTITY);
        }
        Ok((w, Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "toy"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// One part per placement, in the order given, as often as asked.
struct OrderedParts {
    order: Vec<usize>,
    placements: Vec<Entity>,
    next: usize,
}
impl PartSource for OrderedParts {
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.placements = world.iter::<Instance>().map(|(e, _)| e).collect();
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        let Some(&i) = self.order.get(self.next) else {
            return Ok(None);
        };
        self.next += 1;
        Ok(Some(Part {
            roots: BTreeSet::from([self.placements[i]]),
            context: BTreeSet::new(),
            whole: false,
            area: None,
        }))
    }
    fn restart(&mut self) -> Result<()> {
        self.next = 0;
        Ok(())
    }
}

/// The summary pass: what the part owns.
struct Count;
impl Pass for Count {
    fn name(&self) -> &'static str {
        "toy-count"
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.owned"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let roots = w.resource::<PartScope>().ok_or("no part scope")?.roots.clone();
        let owned = roots.into_iter().filter(|&e| w.has::<Instance>(e)).count();
        w.insert_resource(Owned(owned as u64));
        Ok(())
    }
}

/// The first decision, from the merged summary.
struct Decide;
impl Pass for Decide {
    fn name(&self) -> &'static str {
        "toy-decide"
    }
    fn requires(&self) -> &[&'static str] {
        &["toy.owned"]
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.total"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let n = w.resource::<Owned>().ok_or("no summary")?.0;
        w.insert_resource(Total(n));
        Ok(())
    }
}

/// The gather pass: given the first decision, each owned placement contributes a bit to cell `index % cells`, its
/// value the placement's index plus the total: declared as the part's output, for the spool to take.
struct Share {
    cells: u32,
}
impl Pass for Share {
    fn name(&self) -> &'static str {
        "toy-share"
    }
    fn requires(&self) -> &[&'static str] {
        &["toy.total"]
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.bit"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let total = w
            .resource::<Total>()
            .ok_or("the gather traversal is handed the decision")?
            .0;
        let roots = w.resource::<PartScope>().ok_or("no part scope")?.roots.clone();
        let owned: Vec<Entity> = roots.into_iter().filter(|&e| w.has::<Instance>(e)).collect();
        for r in owned {
            let bit = w.spawn();
            w.insert(
                bit,
                Bit {
                    cell: r.index() % self.cells,
                    value: u64::from(r.index()) + total,
                },
            );
            log.place(bit);
        }
        log.approximate("shared", 1);
        Ok(())
    }
}

/// Holds the bits apart from the parts, by cell; counts the most it held at once; gives one cell per group, in cell
/// order (with `twice`, the first cell a second time).
#[derive(Default)]
struct ToySpool {
    held: BTreeMap<u32, Vec<u64>>,
    peak: usize,
    twice: bool,
    given: Vec<u32>,
}
impl Spool for ToySpool {
    fn take(&mut self, part: &mut World) -> Result<()> {
        let bits: Vec<(Entity, Bit)> = part.iter::<Bit>().map(|(e, b)| (e, b.clone())).collect();
        for (e, b) in bits {
            self.held.entry(b.cell).or_default().push(b.value);
            part.despawn(e);
        }
        self.peak = self.peak.max(self.held.values().map(Vec::len).sum());
        Ok(())
    }
    fn next(&mut self) -> Result<Option<World>> {
        let cell = if self.twice && self.given.len() == 1 {
            self.given[0]
        } else {
            match self.held.keys().find(|k| !self.given.contains(k)) {
                Some(&k) => k,
                None => return Ok(None),
            }
        };
        self.given.push(cell);
        let mut values = self.held.get(&cell).cloned().unwrap_or_default();
        values.sort_unstable();
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Cell { cell, values });
        Ok(Some(w))
    }
}

/// The group pass: the cell's sum, as the group's summary; it is handed the first decision.
struct SumCell;
impl Pass for SumCell {
    fn name(&self) -> &'static str {
        "toy-sum-cell"
    }
    fn requires(&self) -> &[&'static str] {
        &["toy.cell", "toy.total"]
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.sums"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let (cell, sum) = w
            .iter::<Cell>()
            .map(|(_, c)| (c.cell, c.values.iter().sum::<u64>()))
            .next()
            .ok_or("no cell")?;
        w.insert_resource(Sums(BTreeMap::from([(cell, sum)])));
        log.approximate("cells summed", 1);
        Ok(())
    }
}

/// The finalizing pass: from every cell's sum and the first decision.
struct Finalize;
impl Pass for Finalize {
    fn name(&self) -> &'static str {
        "toy-finalize"
    }
    fn requires(&self) -> &[&'static str] {
        &["toy.sums", "toy.total"]
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.final"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let sums: u64 = w.resource::<Sums>().ok_or("no sums")?.0.values().sum();
        let total = w.resource::<Total>().ok_or("no total")?.0;
        w.insert_resource(Final(sums + total));
        log.approximate("finalized", 1);
        Ok(())
    }
}

/// What the writer was given, in order: a group (its cell and values) or a part (the final decision it saw, its
/// placements, and whether a contribution reached it).
#[derive(Clone, Debug, PartialEq)]
enum Got {
    Group(u32, Vec<u64>),
    Part(Option<u64>, usize, bool),
}

struct Seen(Rc<RefCell<Vec<Got>>>);
impl Writer for Seen {
    fn target(&self) -> Target {
        Target::new("toy")
            .legal("core.instance")
            .legal("core.name")
            .legal("toy.cell")
            .legal("toy.owned")
            .legal("toy.total")
            .legal("toy.sums")
            .legal("toy.final")
    }
    fn write(&mut self, w: &World) -> Result<()> {
        let got = match w.iter::<Cell>().next() {
            Some((_, c)) => Got::Group(c.cell, c.values.clone()),
            None => Got::Part(
                w.resource::<Final>().map(|f| f.0),
                w.iter::<Instance>().count(),
                w.iter::<Bit>().next().is_some(),
            ),
        };
        self.0.borrow_mut().push(got);
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn run(order: &[usize], twice: bool) -> Result<(Vec<Got>, primase_core::convert::ConvertReport, usize)> {
    let mut registry = SummaryRegistry::new();
    registry.summary::<Owned>();
    registry.summary::<Sums>();
    registry.decision::<Total>();
    registry.decision::<Final>();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let manager = PassManager::new(PassRegistry::new());
    let one = |p: Box<dyn Pass>| Pipeline { passes: vec![p] };
    let (summarize, decide) = (one(Box::new(Count)), one(Box::new(Decide)));
    let (gather, group, finalize) = (
        one(Box::new(Share { cells: 3 })),
        one(Box::new(SumCell)),
        one(Box::new(Finalize)),
    );
    let none = Pipeline { passes: Vec::new() };
    let mut spool = ToySpool {
        twice,
        ..ToySpool::default()
    };
    let report = convert_parts_gathered(
        &mut ToyReader(order.len()),
        &mut OrderedParts {
            order: order.to_vec(),
            placements: Vec::new(),
            next: 0,
        },
        Box::new(Seen(seen.clone())),
        &manager,
        &none,
        &Summarized {
            registry: &registry,
            summarize: &summarize,
            decide: &decide,
        },
        Gathered {
            spool: &mut spool,
            gather: &gather,
            group: &group,
            finalize: &finalize,
        },
        &none,
    )?;
    Ok((seen.take(), report, spool.peak))
}

#[test]
fn groups_are_written_first_and_every_part_gets_the_finalizing_decision_in_any_order() {
    let mut first = None;
    for order in [[0, 1, 2, 3], [3, 1, 0, 2], [2, 3, 1, 0]] {
        let (got, report, peak) = run(&order, false).unwrap();
        // placements 1, 3, 5, 7 (entity indices; prototypes are 0, 2, 4, 6), total 4, cells index % 3, in cell order
        let groups = [
            Got::Group(0, vec![7]),
            Got::Group(1, vec![5, 11]),
            Got::Group(2, vec![9]),
        ];
        assert_eq!(got[..3], groups[..], "{order:?}");
        // then the parts: each saw the final decision (32 + 4), wrote its one placement, and no bit
        assert_eq!(&got[3..], vec![Got::Part(Some(36), 1, false); 4], "{order:?}");
        assert_eq!(
            (report.gather_runs.len(), report.groups.len(), report.parts),
            (4, 3, 4)
        );
        assert_eq!(report.finalized.steps.len(), 1);
        assert_eq!(peak, 4, "the spool held the bits, the parts none");
        // what the group and finalizing passes approximated reaches the notices, the group as its example
        let notices = report.notices();
        let of = |reason: &str| {
            notices
                .iter()
                .find(|n| n.kind == NoticeKind::Approximation && n.reason == reason)
                .map(|n| (n.count, n.examples.clone()))
        };
        let groups_named = vec!["group 0".to_owned(), "group 1".to_owned(), "group 2".to_owned()];
        assert_eq!(of("cells summed"), Some((3, groups_named)));
        assert_eq!(of("finalized"), Some((1, Vec::new())));
        // the gather traversal's notices, named by traversal, one example per part
        let gathered = notices
            .iter()
            .find(|n| n.kind == NoticeKind::Approximation && n.reason == "shared")
            .unwrap();
        assert_eq!(
            (gathered.stage.as_str(), gathered.count, gathered.examples.len()),
            ("toy-share (gather traversal)", 4, 4)
        );
        assert_eq!(report.approximations()["toy-share (gather traversal): shared"], 4);
        match &first {
            None => first = Some(got),
            Some(f) => assert_eq!(&got, f, "the same writer input in every order"),
        }
    }
}

#[test]
fn a_group_summary_two_groups_make_is_refused() {
    let e = run(&[0, 1, 2, 3], true).unwrap_err().to_string();
    assert!(e.contains("cell 0 from two groups"), "{e}");
}
