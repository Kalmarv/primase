//! `primase_core::convert`: the loop's order, what it keeps of each part (the roots' closure and what the passes
//! made, not the context), the reader's namespaces, the target check that changes nothing, the reports kept per part
//! and on failure, and parts from a partition plan, with a toy reader, part source and writer.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    ConvertError, Notice, NoticeKind, Part, PartSize, PartSource, PartitionGroup, PartitionPlan,
    PlannedParts, Reader, Report, Result, Writer, convert, convert_parts,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::{Component, Entity, Instance, Name, Resource, World};

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

/// A raw form the target doesn't write; `toy-lower-raw` would lower it, if the program ran it.
#[derive(Clone, Debug, PartialEq)]
struct ToyRaw;
impl Component for ToyRaw {
    const NAME: &'static str = "toy.raw";
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

/// `placements` placements of their own prototypes; loading gives each prototype a mesh.
struct ToyReader {
    placements: usize,
    /// Put a `pbr.*` component into the world on reading / on loading.
    bad_read: bool,
    bad_load: bool,
    /// The roots of every load, in order.
    loads: Vec<BTreeSet<Entity>>,
}

fn reader(placements: usize) -> ToyReader {
    ToyReader {
        placements,
        bad_read: false,
        bad_load: false,
        loads: Vec::new(),
    }
}

impl Reader for ToyReader {
    fn read(&mut self) -> Result<(World, Report)> {
        event("read");
        let mut w = World::new();
        for k in 0..self.placements {
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
        Ok((
            w,
            vec![("toy".into(), format!("read {} placements", self.placements))],
        ))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        event("load");
        self.loads.push(roots.clone());
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
    fn emits(&self) -> &'static [&'static str] {
        &["core", "toy"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some([[0.0; 3], [1.0; 3]])
    }
}

/// One part per placement, planned from the selected world before its geometry is loaded.
#[derive(Default)]
struct ToyParts {
    placements: Vec<Entity>,
}

impl PartSource for ToyParts {
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        event("plan");
        assert!(world.iter::<ToyMesh>().next().is_none());
        self.placements = world.iter::<Instance>().map(|(e, _)| e).collect();
        assert!(
            world
                .iter::<Instance>()
                .all(|(_, i)| boxes(i.prototype).is_some())
        );
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        Ok(self.placements.pop().map(|e| Part {
            roots: BTreeSet::from([e]),
            context: BTreeSet::new(),
        }))
    }
}

/// Per written part: its placements and named entities, and whether it holds a mesh and a raw form.
#[derive(Clone, Debug, PartialEq)]
struct Got {
    placements: BTreeSet<Entity>,
    named: usize,
    mesh: bool,
    raw: bool,
}
type Written = Rc<RefCell<Vec<Got>>>;

/// Records what it was given; fails on the part numbered `fail_at`; reports one omission per part and at the end.
struct ToyWriter {
    target: Target,
    written: Written,
    fail_at: Option<usize>,
    seen: usize,
}

impl Writer for ToyWriter {
    fn target(&self) -> Target {
        self.target.clone()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        event("write");
        let i = self.seen;
        self.seen += 1;
        if self.fail_at == Some(i) {
            return Err("the toy writer failed".into());
        }
        self.written.borrow_mut().push(Got {
            placements: part.iter::<Instance>().map(|(e, _)| e).collect(),
            named: part.iter::<Name>().count(),
            mesh: part.iter::<ToyMesh>().next().is_some(),
            raw: part.iter::<ToyRaw>().next().is_some(),
        });
        Ok(())
    }
    fn notices(&mut self) -> Vec<Notice> {
        vec![Notice::new(
            "toy-writer",
            NoticeKind::Unsupported,
            "toy capes not written",
            1,
        )]
    }
    fn finish(self: Box<Self>) -> Result<Report> {
        event("finish");
        Ok(vec![(
            "toy".into(),
            format!("{} parts", self.written.borrow().len()),
        )])
    }
}

fn writer(target: Target) -> (Box<ToyWriter>, Written) {
    let written = Rc::new(RefCell::new(Vec::new()));
    (
        Box::new(ToyWriter {
            target,
            written: written.clone(),
            fail_at: None,
            seen: 0,
        }),
        written,
    )
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

/// The passes: record that they ran, place an entity nothing references (as a pass placing a marker does) with a raw
/// form the target doesn't write, make another that nothing references and they don't place (a stray, as a texture
/// derived for a discarded form), and approximate the same thing in every part.
struct ToyPass;
impl Pass for ToyPass {
    fn name(&self) -> &'static str {
        "toy-pass"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name", "toy.mesh", "toy.raw"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        event("passes");
        let marker = w.spawn();
        w.insert(marker, Name("placed, unreferenced".into()));
        w.insert(marker, ToyMesh(5));
        w.insert(marker, ToyRaw);
        log.place(marker);
        let stray = w.spawn();
        w.insert(stray, ToyMesh(3));
        log.count("orphans", 1);
        log.approximate("toy meshes rounded", 2);
        Ok(())
    }
}

/// Would lower the raw form; registered, so a planner could choose it, but the loop never runs a pass on its own.
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
    fn legalizes(&self) -> &[&'static str] {
        &["toy.raw"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        event("lower-raw");
        let raw: Vec<Entity> = w.iter::<ToyRaw>().map(|(e, _)| e).collect();
        for e in raw {
            w.remove::<ToyRaw>(e);
        }
        Ok(())
    }
}

/// A partition: the first two placements in one part with the third as its context, the third in another part.
struct ToyPartition;
impl Pass for ToyPartition {
    fn name(&self) -> &'static str {
        "toy-partition"
    }
    fn provides(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PartitionPlan::NAME]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let p: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        w.insert_resource(PartitionPlan {
            by: "toy-partition".into(),
            groups: vec![
                PartitionGroup {
                    key: "west".into(),
                    roots: BTreeSet::from([p[0], p[1]]),
                    context: BTreeSet::from([p[2]]),
                },
                PartitionGroup {
                    key: "east".into(),
                    roots: BTreeSet::from([p[2]]),
                    context: BTreeSet::new(),
                },
            ],
        });
        Ok(())
    }
}

fn manager(select: &str) -> (PassManager, Pipeline, Pipeline) {
    let mut r = PassRegistry::new();
    r.register("toy-select", |_| Ok(Box::new(ToySelect)));
    r.register("toy-pass", |_| Ok(Box::new(ToyPass)));
    r.register("toy-lower-raw", |_| Ok(Box::new(ToyLowerRaw)));
    r.register("toy-partition", |_| Ok(Box::new(ToyPartition)));
    let (select, passes) = (r.parse(select).unwrap(), r.parse("toy-pass").unwrap());
    (PassManager::new(r), select, passes)
}

/// What the toy writer writes; `toy.raw` it doesn't.
fn target() -> Target {
    Target::new("toy")
        .legal("core.instance")
        .legal("core.name")
        .legal("toy.mesh")
        .illegal("toy.raw")
}

#[test]
fn reads_selects_plans_then_loads_passes_checks_and_writes_each_part() {
    let (m, select, passes) = manager("toy-select");
    let (w, written) = writer(target());
    let mut reader = reader(2);
    let report = convert_parts(&mut reader, &mut ToyParts::default(), w, &m, &select, &passes).unwrap();
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
            .get("convert: entities the passes placed (core.name + toy.mesh + toy.raw)"),
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
    for g in written.borrow().iter() {
        assert_eq!((g.placements.len(), g.named, g.mesh), (1, 2, true));
    }
    // each part's payload as loaded and as written (its one root, the placement, the prototype it reaches and the
    // placed entity with its 5 bytes; not the stray's 3)
    let size = PartSize {
        roots: 1,
        loaded: BTreeMap::from([("toy.mesh".to_string(), 7)]),
        written: BTreeMap::from([("toy.mesh".to_string(), 12)]),
    };
    assert_eq!(report.part_sizes, vec![size.clone(), size]);
}

/// The loop runs the program's passes once and adds none: a raw form the target doesn't write is not lowered (though
/// a pass that lowers it is registered) but reported, and the writer gets the part as the passes left it.
#[test]
fn the_programs_passes_run_once_and_nothing_is_lowered_for_the_writer() {
    let (m, select, passes) = manager("toy-select");
    let (w, written) = writer(target());
    let report = convert_parts(&mut reader(2), &mut ToyParts::default(), w, &m, &select, &passes).unwrap();
    let ev = events();
    assert_eq!(ev.iter().filter(|e| *e == "passes").count(), 2);
    assert!(!ev.contains(&"lower-raw".to_string()), "{ev:?}");
    assert!(written.borrow().iter().all(|g| g.raw));
    for p in &report.part_reports {
        assert_eq!(p.explicit.steps.len(), 1);
        assert_eq!(p.unsupported, vec![("toy.raw".to_string(), 1)]);
    }
    let unsupported = report
        .notices()
        .into_iter()
        .find(|n| n.stage == "convert" && n.kind == NoticeKind::Unsupported)
        .unwrap();
    assert_eq!(
        (unsupported.reason.as_str(), unsupported.count),
        ("toy.raw: the writer doesn't write it", 2)
    );
}

/// A value of a name the writer writes that its rule refuses is an error before anything is written; a world with
/// nothing in it is written as nothing.
#[test]
fn values_the_writer_cant_encode_fail_and_an_empty_world_is_fine() {
    let (m, select, passes) = manager("toy-select");
    // the loaded meshes (7) are refused
    let (w, written) = writer(target().dynamic_typed::<ToyMesh>(|m| m.0 < 6));
    let e = convert_parts(&mut reader(2), &mut ToyParts::default(), w, &m, &select, &passes).unwrap_err();
    assert!(e.to_string().contains("toy.mesh the writer can't write"), "{e}");
    assert!(written.borrow().is_empty());
    events();
    let (w, written) = writer(target());
    let report = convert_parts(&mut reader(0), &mut ToyParts::default(), w, &m, &select, &passes).unwrap();
    assert_eq!(report.parts, 0);
    assert!(written.borrow().is_empty());
    assert!(events().contains(&"finish".to_string()));
}

#[test]
fn a_reader_outside_its_namespaces_fails_on_reading_and_on_loading() {
    let (m, select, passes) = manager("toy-select");
    let mut bad = reader(2);
    bad.bad_read = true;
    let e = convert(&mut bad, writer(target()).0, &m, &select, &passes)
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("read: the reader put pbr.toy in the world"), "{e}");
    events();
    let mut bad = reader(2);
    bad.bad_load = true;
    let e = convert(&mut bad, writer(target()).0, &m, &select, &passes)
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("load: the reader put pbr.toy in the world"), "{e}");
    assert!(!events().contains(&"write".to_string()));
}

/// Parts from a partition plan the selection made: a part's context is loaded with it but written by its owner, so
/// every placement is written once, as by the whole world in one part.
#[test]
fn planned_parts_load_their_context_and_write_only_what_they_own() {
    let (m, select, passes) = manager("toy-select, toy-partition");
    let (w, written) = writer(target());
    let mut toy = reader(3);
    let report = convert_parts(&mut toy, &mut PlannedParts::default(), w, &m, &select, &passes).unwrap();
    events();
    assert_eq!(report.parts, 2);
    let got = written.borrow().clone();
    // the first part loaded its two placements and the third as context, and wrote only its own two
    assert_eq!(toy.loads[0].len(), 3);
    assert_eq!(got[0].placements.len(), 2);
    assert_eq!(report.part_reports[0].context.len(), 1);
    assert_eq!(got[1].placements.len(), 1);
    assert!(got[0].placements.is_disjoint(&got[1].placements));
    // the same placements as the whole world in one part, none twice
    let whole_select = manager("toy-select").1;
    let (w, whole) = writer(target());
    convert(&mut reader(3), w, &m, &whole_select, &passes).unwrap();
    events();
    let all: BTreeSet<Entity> = got.iter().flat_map(|g| g.placements.iter().copied()).collect();
    assert_eq!(all, whole.borrow()[0].placements);
    // a selection without a partition pass leaves the planned parts nothing to take
    let (w, _) = writer(target());
    let e = convert_parts(
        &mut reader(3),
        &mut PlannedParts::default(),
        w,
        &m,
        &whole_select,
        &passes,
    )
    .unwrap_err();
    assert!(e.to_string().contains("no core.partition_plan"), "{e}");
    events();
}

/// Every part's reports are kept whole: the same approximation in two parts is two records with the parts' ordinals,
/// summed once; what a part dropped is a selection notice; the writer's omissions come back after each part and
/// before finishing.
#[test]
fn each_parts_reports_are_kept_and_summed_once() {
    let (m, select, passes) = manager("toy-select");
    let (w, _) = writer(target());
    let report = convert_parts(&mut reader(2), &mut ToyParts::default(), w, &m, &select, &passes).unwrap();
    events();
    assert_eq!(report.part_reports.len(), 2);
    for (i, p) in report.part_reports.iter().enumerate() {
        assert_eq!(p.ordinal, i);
        assert_eq!(p.roots.len(), 1);
        assert_eq!(p.load, vec![("toy".to_string(), "loaded".to_string())]);
        assert_eq!(
            p.explicit.steps[0].log.approximations.get("toy meshes rounded"),
            Some(&2)
        );
        assert_eq!(
            p.kept.get("made entities nothing keeps, dropped (toy.mesh)"),
            Some(&1)
        );
    }
    assert_eq!(
        report.approximations(),
        BTreeMap::from([("toy-pass: toy meshes rounded".to_string(), 4)])
    );
    let notices = report.notices();
    let find = |stage: &str, reason: &str| {
        notices
            .iter()
            .find(|n| n.stage == stage && n.reason == reason)
            .unwrap_or_else(|| panic!("no notice {stage}: {reason} in {notices:?}"))
            .clone()
    };
    let rounded = find("toy-pass", "toy meshes rounded");
    assert_eq!(
        (rounded.kind, rounded.count, rounded.examples),
        (
            NoticeKind::Approximation,
            4,
            vec!["part 0".to_string(), "part 1".to_string()]
        )
    );
    let dropped = find("convert", "made entities nothing keeps, dropped (toy.mesh)");
    assert_eq!((dropped.kind, dropped.count), (NoticeKind::Selection, 2));
    assert_eq!(notices.iter().filter(|n| n.stage == "toy-writer").count(), 3);
}

/// A conversion that fails on its second part keeps the first part's reports, says where it failed, and the error
/// reads as the writer's.
#[test]
fn a_failed_conversion_keeps_what_it_had_reported() {
    let (m, select, passes) = manager("toy-select");
    let (mut w, _) = writer(target());
    w.fail_at = Some(1);
    let e = convert_parts(&mut reader(2), &mut ToyParts::default(), w, &m, &select, &passes).unwrap_err();
    events();
    assert_eq!(e.to_string(), "the toy writer failed");
    let e = e.downcast::<ConvertError>().unwrap();
    assert_eq!(e.during, "part 1: write");
    assert_eq!(e.partial.parts, 1);
    assert_eq!(e.partial.part_reports.len(), 2);
    assert_eq!(
        e.partial.approximations().get("toy-pass: toy meshes rounded"),
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
