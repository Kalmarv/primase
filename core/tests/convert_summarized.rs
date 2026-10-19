//! `primase_core::convert::convert_parts_summarized`: every part's summary is made, merged over the parts and decided on
//! once before any part is written; each part then gets the decision, whatever order the parts arrive in; a summary
//! whose merge depends on that order is refused, and so is a part source that cannot give its parts twice.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use primase_core::convert::{
    Finished, Part, PartScope, PartSource, Reader, Report, Result, Summarized, Writer,
    convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::summary::{Summary, SummaryRegistry};
use primase_core::{Entity, Instance, Name, Resource, World};

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
}

/// The decision: how many placements the whole holds.
#[derive(Clone, Debug, PartialEq)]
struct Total(u64);
impl Resource for Total {
    const NAME: &'static str = "toy.total";
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
            context: BTreeSet::new(),
            whole: false,
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
    let manager = PassManager::new(PassRegistry::new());
    let summarize = Pipeline {
        passes: vec![Box::new(Count { firsts }) as Box<dyn Pass>],
    };
    let decide = Pipeline {
        passes: vec![Box::new(Decide) as Box<dyn Pass>],
    };
    let none = Pipeline { passes: Vec::new() };
    let report = convert_parts_summarized(
        &mut ToyReader(3),
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
