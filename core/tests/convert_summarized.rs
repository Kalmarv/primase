//! `primase_core::convert::convert_parts_summarized`: every part's summary is made, merged over the parts and decided on
//! once before any part is written; each part then gets the decision, whatever order the parts arrive in; a summary
//! whose merge depends on that order is refused, and so is a part source that cannot give its parts twice.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use primase_core::convert::{
    Finished, Part, PartScope, PartSource, Reader, Report, Result, Summarized, Writer,
    convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::summary::{Summary, SummaryRegistry};
use primase_core::{Entity, Instance, Name, Resource, Transform, World};

/// The placements a part owns: summed over the parts.
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

/// The parts' first placements in the order they were merged: depends on the arrival order.
#[derive(Clone, Debug, PartialEq)]
struct Firsts(Vec<Entity>);
impl Resource for Firsts {
    const NAME: &'static str = "toy.firsts";
}
impl Summary for Firsts {
    fn merge(&mut self, other: Self) -> std::result::Result<(), String> {
        self.0.extend(other.0);
        Ok(())
    }
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Entity>() * self.0.len()
    }
}

/// The decision: how many placements the whole holds.
#[derive(Clone, Debug, PartialEq)]
struct Total(u64);
impl Resource for Total {
    const NAME: &'static str = "toy.total";
}

/// `n` placements of their own prototypes.
/// `n` placements of their own prototypes; `malformed`: a load drops the placements' transforms (a reader at fault).
struct ToyReader(usize, bool);
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
        let mut w = world.subset(&world.closure(roots.iter().copied()));
        if self.1 {
            let placed: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
            for e in placed {
                w.remove::<Transform>(e);
            }
        }
        Ok((w, Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "toy"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// One part per placement, in the order `order` gives (indices into the placements), the same each time; or once only.
struct OrderedParts {
    order: Vec<usize>,
    placements: Vec<Entity>,
    next: usize,
    restartable: bool,
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
            shared: BTreeSet::new(),
            context: BTreeSet::new(),
            whole: false,
            area: None,
        }))
    }
    fn restart(&mut self) -> Result<()> {
        if !self.restartable {
            return Err("these parts come once".into());
        }
        self.next = 0;
        Ok(())
    }
}

/// The summary pass: what the part owns (its roots that are placements), and its first placement when asked.
struct Count {
    firsts: bool,
}
impl Pass for Count {
    fn name(&self) -> &'static str {
        "toy-count"
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.owned", "toy.firsts"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let roots = w.resource::<PartScope>().ok_or("no part scope")?.roots.clone();
        let owned: Vec<Entity> = roots.into_iter().filter(|&e| w.has::<Instance>(e)).collect();
        w.insert_resource(Owned(owned.len() as u64));
        if self.firsts {
            w.insert_resource(Firsts(owned));
        }
        Ok(())
    }
}

/// The decision pass: the whole's total, from the merged summary only.
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
        assert!(
            w.iter::<Instance>().next().is_none(),
            "a decision sees no part content"
        );
        let n = w.resource::<Owned>().ok_or("no summary")?.0;
        w.insert_resource(Total(n));
        Ok(())
    }
}

/// Per written part: the decision it saw and its placements.
type Saw = Vec<(Option<Total>, usize)>;

/// What each written part saw of the decision.
struct Seen(Rc<RefCell<Saw>>);
impl Writer for Seen {
    fn target(&self) -> Target {
        Target::new("toy")
            .legal("core.instance")
            .legal("core.name")
            .legal("toy.owned")
            .legal("toy.firsts")
            .legal("toy.total")
    }
    fn write(&mut self, part: &World) -> Result<()> {
        self.0
            .borrow_mut()
            .push((part.resource::<Total>().cloned(), part.iter::<Instance>().count()));
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn run(order: &[usize], restartable: bool, firsts: bool) -> Result<Saw> {
    let mut registry = SummaryRegistry::new();
    registry.summary::<Owned>();
    registry.summary::<Firsts>();
    registry.decision::<Total>();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let summarize = Pipeline {
        passes: vec![Box::new(Count { firsts }) as Box<dyn Pass>],
    };
    let decide = Pipeline {
        passes: vec![Box::new(Decide) as Box<dyn Pass>],
    };
    let none = Pipeline { passes: Vec::new() };
    let report = convert_parts_summarized(
        &mut ToyReader(3, false),
        &mut OrderedParts {
            order: order.to_vec(),
            placements: Vec::new(),
            next: 0,
            restartable,
        },
        Box::new(Seen(seen.clone())),
        &manager,
        &none,
        &Summarized {
            registry: &registry,
            summarize: &summarize,
            decide: &decide,
        },
        &none,
    )?;
    assert_eq!(
        (report.summarized.len(), report.parts),
        (order.len(), order.len())
    );
    assert_eq!(report.decided.steps.len(), 1);
    // each part's summary (8 bytes) kept until the merge, and the merged one, counted
    if !firsts {
        assert_eq!(
            (report.summary_bytes, report.merged_summary_bytes),
            (8 * order.len(), 8)
        );
    }
    Ok(seen.take())
}

#[test]
fn every_part_gets_the_decision_over_all_parts_in_any_order() {
    for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
        let seen = run(&order, true, false).unwrap();
        assert_eq!(seen, vec![(Some(Total(3)), 1); 3], "{order:?}");
    }
}

#[test]
fn an_order_bound_summary_and_a_one_pass_source_are_refused() {
    let e = run(&[0, 1, 2], true, true).unwrap_err().to_string();
    assert!(e.contains("toy.firsts: its merge depends on the order"), "{e}");
    let e = run(&[0, 1, 2], false, false).unwrap_err().to_string();
    assert!(e.contains("these parts come once"), "{e}");
}

/// Counts the times a sentinel pass ran.
static RAN: AtomicUsize = AtomicUsize::new(0);

/// A pass that only notes that it ran.
struct Sentinel(&'static str);
impl Pass for Sentinel {
    fn name(&self) -> &'static str {
        self.0
    }
    fn run(&self, _w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        RAN.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A sum whose own check refuses more than 2: each part's is valid, their merge is not.
#[derive(Clone, Debug, PartialEq)]
struct Capped(u32);
impl Resource for Capped {
    const NAME: &'static str = "toy.capped";
    fn verify(&self, _w: &World) -> std::result::Result<(), String> {
        if self.0 > 2 {
            Err(format!("{} past 2", self.0))
        } else {
            Ok(())
        }
    }
}
impl Summary for Capped {
    fn merge(&mut self, other: Self) -> std::result::Result<(), String> {
        self.0 += other.0;
        Ok(())
    }
    fn retained_bytes(&self) -> usize {
        4
    }
}

/// Puts Capped(1) into each part.
struct One;
impl Pass for One {
    fn name(&self) -> &'static str {
        "toy-one"
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.capped"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        w.insert_resource(Capped(1));
        Ok(())
    }
}

/// The toy map converted with `summarize` and `decide`, the loads `malformed` or not; the error, if it failed.
fn summarize_with(
    malformed: bool,
    summarize: Vec<Box<dyn Pass>>,
    decide: Vec<Box<dyn Pass>>,
) -> Option<String> {
    let mut registry = SummaryRegistry::new();
    registry.summary::<Capped>();
    let none = Pipeline { passes: Vec::new() };
    convert_parts_summarized(
        &mut ToyReader(3, malformed),
        &mut OrderedParts {
            order: vec![0, 1, 2],
            placements: Vec::new(),
            next: 0,
            restartable: true,
        },
        Box::new(Seen(Rc::new(RefCell::new(Vec::new())))),
        &PassManager::new(PassRegistry::new()).with_pass_checks(true),
        &none,
        &Summarized {
            registry: &registry,
            summarize: &Pipeline { passes: summarize },
            decide: &Pipeline { passes: decide },
        },
        &none,
    )
    .err()
    .map(|e| e.to_string())
}

/// What a reader loads is checked before any summary pass reads it, an empty summary pipeline too; and the merged
/// summaries are checked before the first decision pass reads them (a merge makes input as a pass does).
#[test]
fn loads_and_merged_summaries_are_checked_before_they_are_read() {
    RAN.store(0, Ordering::SeqCst);
    let e = summarize_with(true, vec![Box::new(Sentinel("sentinel"))], Vec::new()).unwrap();
    assert!(
        e.contains("summary 0: load") && e.contains("core.transform"),
        "{e}"
    );
    assert_eq!(
        RAN.load(Ordering::SeqCst),
        0,
        "the summary pass did not run on the malformed load"
    );
    let e = summarize_with(true, Vec::new(), Vec::new()).unwrap();
    assert!(e.contains("summary 0: load"), "{e}");
    // three valid parts' Capped(1) merge into Capped(3), which its own check refuses: before any decision pass
    let e = summarize_with(
        false,
        vec![Box::new(One)],
        vec![Box::new(Sentinel("decide-sentinel"))],
    )
    .unwrap();
    assert!(
        e.contains("the merged summaries") && e.contains("3 past 2"),
        "{e}"
    );
    assert_eq!(
        RAN.load(Ordering::SeqCst),
        0,
        "the decision pass did not run on the invalid merge"
    );
}
