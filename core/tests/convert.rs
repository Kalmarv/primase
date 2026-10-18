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

/// The passes: record that they ran, place an entity nothing references (as a pass placing a marker does) and make
/// another that nothing references and they don't place (a stray, as a texture derived for a discarded form).
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
        let marker = w.spawn();
        w.insert(marker, Name("placed, unreferenced".into()));
        w.insert(marker, ToyMesh(5));
        log.place(marker);
        let stray = w.spawn();
        w.insert(stray, ToyMesh(3));
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
    // what the passes placed is kept, what they made that nothing keeps is dropped: both named by their components
    assert_eq!(
        report
            .counts
            .get("convert: entities the passes placed (core.name + toy.mesh)"),
        Some(&2)
    );
    assert_eq!(
        report
            .counts
            .get("convert: made entities nothing keeps, dropped (toy.mesh)"),
        Some(&2)
    );
    assert_eq!(report.written, vec![("toy".to_string(), "2 parts".to_string())]);
    // each part is its placement and prototype, loaded, and the entity the pass placed though nothing references it
    assert_eq!(*written.borrow(), vec![(3, true), (3, true)]);
    // each part's payload as loaded and as written (its one root, the placement, the prototype it reaches and the
    // placed entity with its 5 bytes; not the stray's 3)
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

/// A raw form the target doesn't take; `toy-lower-raw` lowers it, approximately.
#[derive(Clone, Debug, PartialEq)]
struct ToyRaw;
impl Component for ToyRaw {
    const NAME: &'static str = "toy.raw";
}

/// The passes of the report test: approximate the same thing in every part and leave a raw form for the target check.
struct ToyApprox;
impl Pass for ToyApprox {
    fn name(&self) -> &'static str {
        "toy-approx"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name", "toy.mesh", "toy.raw"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let marker = w.spawn();
        w.insert(marker, Name("placed".into()));
        w.insert(marker, ToyRaw);
        log.place(marker);
        let stray = w.spawn();
        w.insert(stray, ToyMesh(3));
        log.approximate("toy meshes rounded", 2);
        Ok(())
    }
}

struct ToyLowerRaw;
impl Pass for ToyLowerRaw {
    fn name(&self) -> &'static str {
        "toy-lower-raw"
    }
    fn requires(&self) -> &[&'static str] {
        &["toy.raw"]
    }
    fn removes(&self) -> &[&'static str] {
        &["toy.raw"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let raw: Vec<Entity> = w.iter::<ToyRaw>().map(|(e, _)| e).collect();
        for e in &raw {
            w.remove::<ToyRaw>(*e);
        }
        log.approximate("raw forms lowered", raw.len() as u64);
        Ok(())
    }
}

/// A writer that fails on the part numbered `fail_at` (from 0), reporting one omission per part it writes.
struct FailingWriter {
    inner: ToyWriter,
    fail_at: usize,
    seen: usize,
}
impl Writer for FailingWriter {
    fn target(&self) -> Target {
        self.inner.target()
    }
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.inner.plan(world, boxes)
    }
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>> {
        self.inner.next_part()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let i = self.seen;
        self.seen += 1;
        if i == self.fail_at {
            return Err("the toy writer failed".into());
        }
        self.inner.write(part)
    }
    fn notices(&mut self) -> Vec<primase_core::convert::Notice> {
        vec![primase_core::convert::Notice::new(
            "toy-writer",
            primase_core::convert::NoticeKind::Unsupported,
            "toy capes not written",
            1,
        )]
    }
    fn finish(self: Box<Self>) -> Result<Report> {
        Box::new(self.inner).finish()
    }
}

fn report_manager() -> (
    PassManager,
    primase_core::pass::Pipeline,
    primase_core::pass::Pipeline,
) {
    let mut r = PassRegistry::new();
    r.register("toy-select", |_| Ok(Box::new(ToySelect)));
    r.register("toy-approx", |_| Ok(Box::new(ToyApprox)));
    r.register("toy-lower-raw", |_| Ok(Box::new(ToyLowerRaw)));
    let (select, passes) = (r.parse("toy-select").unwrap(), r.parse("toy-approx").unwrap());
    (PassManager::new(r), select, passes)
}

fn failing(fail_at: usize) -> Box<FailingWriter> {
    Box::new(FailingWriter {
        inner: *writer(target().illegal("toy.raw")).0,
        fail_at,
        seen: 0,
    })
}

/// Every part's reports are kept whole: the same approximation in two parts is two records with the parts' ordinals,
/// summed once; the target check's lowering is the part's automatic report; what a part dropped is a selection
/// notice; the writer's omissions come back as notices.
#[test]
fn each_parts_reports_are_kept_and_summed_once() {
    use primase_core::convert::NoticeKind;
    let (m, select, passes) = report_manager();
    let report = convert(
        &mut ToyReader {
            bad_read: false,
            bad_load: false,
        },
        failing(usize::MAX),
        &m,
        &select,
        &passes,
    )
    .unwrap();
    events();
    assert_eq!(report.part_reports.len(), 2);
    for (i, p) in report.part_reports.iter().enumerate() {
        assert_eq!(p.ordinal, i);
        assert_eq!(p.roots.len(), 1);
        assert_eq!(p.load, vec![("toy".to_string(), "loaded".to_string())]);
        assert_eq!(p.explicit.steps.len(), 1);
        assert_eq!(
            p.explicit.steps[0].log.approximations.get("toy meshes rounded"),
            Some(&2)
        );
        assert_eq!(p.automatic.steps.len(), 1);
        assert_eq!(p.automatic.steps[0].pass, "toy-lower-raw");
        assert_eq!(
            p.automatic.steps[0].log.approximations.get("raw forms lowered"),
            Some(&1)
        );
        assert_eq!(
            p.kept.get("made entities nothing keeps, dropped (toy.mesh)"),
            Some(&1)
        );
        assert_eq!(
            p.kept.get("entities the passes placed (core.name + toy.raw)"),
            Some(&1)
        );
    }
    assert_eq!(
        report.approximations(),
        BTreeMap::from([
            ("toy-approx: toy meshes rounded".to_string(), 4),
            ("toy-lower-raw: raw forms lowered".to_string(), 2),
        ])
    );
    // the totals are the parts' sums, not counted twice
    assert_eq!(
        report
            .counts
            .get("convert: made entities nothing keeps, dropped (toy.mesh)"),
        Some(&2)
    );
    let notices = report.notices();
    let find = |stage: &str, reason: &str| {
        notices
            .iter()
            .find(|n| n.stage == stage && n.reason == reason)
            .unwrap_or_else(|| panic!("no notice {stage}: {reason} in {notices:?}"))
    };
    let rounded = find("toy-approx", "toy meshes rounded");
    assert_eq!(
        (rounded.kind, rounded.count, rounded.examples.clone()),
        (
            NoticeKind::Approximation,
            4,
            vec!["part 0".to_string(), "part 1".to_string()]
        )
    );
    let dropped = find("convert", "made entities nothing keeps, dropped (toy.mesh)");
    assert_eq!((dropped.kind, dropped.count), (NoticeKind::Selection, 2));
    // the writer was asked after each part and before finishing
    let omitted: Vec<_> = notices
        .iter()
        .filter(|n| n.stage == "toy-writer" && n.kind == NoticeKind::Unsupported)
        .collect();
    assert_eq!(omitted.len(), 3);
}

/// A conversion that fails on its second part keeps the first part's reports, says where it failed, and the error
/// reads as the writer's.
#[test]
fn a_failed_conversion_keeps_what_it_had_reported() {
    use primase_core::convert::{ConvertError, NoticeKind};
    let (m, select, passes) = report_manager();
    let e = convert(
        &mut ToyReader {
            bad_read: false,
            bad_load: false,
        },
        failing(1),
        &m,
        &select,
        &passes,
    )
    .unwrap_err();
    events();
    assert_eq!(e.to_string(), "the toy writer failed");
    let e = e.downcast::<ConvertError>().unwrap();
    assert_eq!(e.during, "part 1: write");
    assert_eq!(e.partial.parts, 1);
    assert_eq!(e.partial.part_reports.len(), 2);
    assert_eq!(
        e.partial.approximations().get("toy-approx: toy meshes rounded"),
        Some(&4)
    );
    let failure = e
        .partial
        .reported
        .iter()
        .find(|n| n.kind == NoticeKind::FailureContext)
        .unwrap();
    assert_eq!(
        failure.reason,
        "failed during part 1: write: the toy writer failed"
    );
}
