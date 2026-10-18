//! `primase_core::convert`: the loop's order, what it keeps of each part (the roots' closure and what the passes
//! made), the reader's namespaces and the target check, with a toy reader and writer.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{PartSize, Reader, Report, Result, Writer, convert};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Target};
use primase_core::{Component, Entity, Instance, Name, World};

#[derive(Clone, Debug, PartialEq)]
struct ToyMesh(u32);
impl Component for ToyMesh {
    const NAME: &'static str = "toy.mesh";
    /// The toy's payload is its number, in bytes.
    fn payload_bytes(&self) -> u64 {
        u64::from(self.0)
    }
}

/// A component outside the toy reader's namespaces.
#[derive(Clone, Debug, PartialEq)]
struct Lowered;
impl Component for Lowered {
    const NAME: &'static str = "pbr.toy";
}

thread_local! {
    /// What happened, in order (the passes run on the test's thread).
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
fn event(e: impl Into<String>) {
    EVENTS.with(|v| v.borrow_mut().push(e.into()));
}
fn events() -> Vec<String> {
    EVENTS.with(|v| v.borrow_mut().drain(..).collect())
}

/// Two placements of their own prototypes; loading gives each prototype a mesh.
struct ToyReader {
    /// Put a `pbr.*` component into the world on reading / on loading.
    bad_read: bool,
    bad_load: bool,
}

impl Reader for ToyReader {
    fn read(&mut self) -> Result<(World, Report)> {
        event("read");
        let mut w = World::new();
        for k in 0..2 {
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
            if self.bad_read {
                w.insert(p, Lowered);
            }
        }
        Ok((w, vec![("toy".into(), "read 2 placements".into())]))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        event("load");
        let mut w = world.subset(&world.closure(roots.iter().copied()));
        let protos: Vec<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        for p in protos {
            w.insert(p, ToyMesh(7));
            if self.bad_load {
                w.insert(p, Lowered);
            }
        }
        Ok((w, vec![("toy".into(), "loaded".into())]))
    }
    fn default_select(&self) -> String {
        "toy-select".into()
    }
    fn default_passes(&self) -> String {
        "toy-pass".into()
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "toy"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some([[0.0; 3], [1.0; 3]])
    }
}

/// Per written part: its entity count and whether it holds a mesh.
type Written = Rc<RefCell<Vec<(usize, bool)>>>;

/// One part per placement; records what it was given.
struct ToyWriter {
    target: Target,
    placements: Vec<Entity>,
    written: Written,
}

impl Writer for ToyWriter {
    fn target(&self) -> Target {
        self.target.clone()
    }
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        event("plan");
        // planned from the selected world, geometry not loaded
        assert!(world.iter::<ToyMesh>().next().is_none());
        self.placements = world.iter::<Instance>().map(|(e, _)| e).collect();
        assert!(
            world
                .iter::<Instance>()
                .all(|(_, i)| boxes(i.prototype).is_some())
        );
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>> {
        Ok(self.placements.pop().map(|e| BTreeSet::from([e])))
    }
    fn write(&mut self, part: &World) -> Result<()> {
        event("write");
        let n = part.iter::<Name>().count() + part.iter::<Instance>().count();
        self.written
            .borrow_mut()
            .push((n, part.iter::<ToyMesh>().next().is_some()));
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Report> {
        event("finish");
        Ok(vec![(
            "toy".into(),
            format!("{} parts", self.written.borrow().len()),
        )])
    }
}

/// The selection: records that it ran.
struct ToySelect;
impl Pass for ToySelect {
    fn name(&self) -> &'static str {
        "toy-select"
    }
    fn run(&self, _w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        event("select");
        Ok(())
    }
}

/// The passes: record that they ran and make an entity nothing references (as a pass placing a marker does).
struct ToyPass;
impl Pass for ToyPass {
    fn name(&self) -> &'static str {
        "toy-pass"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name", "toy.mesh"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        event("passes");
        let orphan = w.spawn();
        w.insert(orphan, Name("made, unreferenced".into()));
        w.insert(orphan, ToyMesh(5));
        log.count("orphans", 1);
        Ok(())
    }
}

fn manager() -> (
    PassManager,
    primase_core::pass::Pipeline,
    primase_core::pass::Pipeline,
) {
    let mut r = PassRegistry::new();
    r.register("toy-select", |_| Ok(Box::new(ToySelect)));
    r.register("toy-pass", |_| Ok(Box::new(ToyPass)));
    let (select, passes) = (r.parse("toy-select").unwrap(), r.parse("toy-pass").unwrap());
    (PassManager::new(r), select, passes)
}

fn target() -> Target {
    Target::new("toy")
        .legal("core.instance")
        .legal("core.name")
        .legal("toy.mesh")
}

fn writer(target: Target) -> (Box<ToyWriter>, Written) {
    let written = Rc::new(RefCell::new(Vec::new()));
    (
        Box::new(ToyWriter {
            target,
            placements: Vec::new(),
            written: written.clone(),
        }),
        written,
    )
}

#[test]
fn reads_selects_plans_then_loads_passes_checks_and_writes_each_part() {
    let (m, select, passes) = manager();
    let (w, written) = writer(target());
    let mut reader = ToyReader {
        bad_read: false,
        bad_load: false,
    };
    let report = convert(&mut reader, w, &m, &select, &passes).unwrap();
    assert_eq!(
        events(),
        [
            "read", "select", "plan", "load", "passes", "write", "load", "passes", "write", "finish"
        ]
    );
    assert_eq!(report.parts, 2);
    assert_eq!(
        report.read,
        vec![("toy".to_string(), "read 2 placements".to_string())]
    );
    assert_eq!(report.loads.len(), 2);
    assert_eq!(report.counts.get("toy-pass: orphans"), Some(&2));
    // what the passes made is named in the counts by its components
    assert_eq!(
        report
            .counts
            .get("convert: entities the passes made (core.name + toy.mesh)"),
        Some(&2)
    );
    assert_eq!(report.written, vec![("toy".to_string(), "2 parts".to_string())]);
    // each part is its placement and prototype, loaded, and the entity the pass made though nothing references it
    assert_eq!(*written.borrow(), vec![(3, true), (3, true)]);
    // each part's payload as loaded and as written (its one root, the placement, the prototype it reaches and the
    // pass's entity with its 5 bytes)
    let size = PartSize {
        roots: 1,
        loaded: BTreeMap::from([("toy.mesh".to_string(), 7)]),
        written: BTreeMap::from([("toy.mesh".to_string(), 12)]),
    };
    assert_eq!(report.part_sizes, vec![size.clone(), size]);
}

#[test]
fn a_reader_outside_its_namespaces_fails_on_reading_and_on_loading() {
    let (m, select, passes) = manager();
    let e = convert(
        &mut ToyReader {
            bad_read: true,
            bad_load: false,
        },
        writer(target()).0,
        &m,
        &select,
        &passes,
    )
    .unwrap_err()
    .to_string();
    assert!(e.starts_with("read: the reader put pbr.toy in the world"), "{e}");
    events();
    let e = convert(
        &mut ToyReader {
            bad_read: false,
            bad_load: true,
        },
        writer(target()).0,
        &m,
        &select,
        &passes,
    )
    .unwrap_err()
    .to_string();
    assert!(e.starts_with("load: the reader put pbr.toy in the world"), "{e}");
    assert!(!events().contains(&"write".to_string()));
}

#[test]
fn a_part_the_target_refuses_fails_before_it_is_written() {
    let (m, select, passes) = manager();
    let (w, written) = writer(
        Target::new("toy")
            .legal("core.instance")
            .legal("core.name")
            .illegal("toy.mesh"),
    );
    let r = convert(
        &mut ToyReader {
            bad_read: false,
            bad_load: false,
        },
        w,
        &m,
        &select,
        &passes,
    );
    assert!(r.is_err());
    let ev = events();
    assert!(
        ev.contains(&"passes".to_string()) && !ev.contains(&"write".to_string()),
        "{ev:?}"
    );
    assert!(written.borrow().is_empty());
}
