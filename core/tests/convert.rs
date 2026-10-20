//! `primase_core::convert`: the loop's order, what it keeps of each part (the roots' closure and what the passes
//! made, not the context), the reader's namespaces, the target check that changes nothing, the reports kept per part
//! and on failure, and parts from a partition plan, with a toy reader, part source and writer.
#![allow(clippy::unwrap_used)]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    ConvertError, Finished, Notice, NoticeKind, Part, PartScope, PartSize, PartSource, PartitionGroup,
    PartitionPlan, PlannedParts, Reader, Report, Result, Writer, convert, convert_parts,
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
            w.insert(p, primase_core::Transform::IDENTITY);
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
            whole: false,
            area: None,
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
    fn finish(self: Box<Self>) -> Result<Finished> {
        event("finish");
        Ok(Finished {
            report: vec![("toy".into(), format!("{} parts", self.written.borrow().len()))],
            notices: vec![Notice::new(
                "toy-writer",
                NoticeKind::Unsupported,
                "toy index fields not written",
                1,
            )],
        })
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
        log.omit("toy hats with no toy form, left out", 1);
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

/// A selection that lowers: every placement gets the `pbr.toy` form, which the program may make though the reader
/// may not.
struct ToyLower;
impl Pass for ToyLower {
    fn name(&self) -> &'static str {
        "toy-lower"
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.toy"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        let placements: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        for e in placements {
            w.insert(e, Lowered);
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
                    area: None,
                },
                PartitionGroup {
                    key: "east".into(),
                    roots: BTreeSet::from([p[2]]),
                    context: BTreeSet::new(),
                    area: None,
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
    r.register("toy-lower", |_| Ok(Box::new(ToyLower)));
    r.register("toy-partition", |_| Ok(Box::new(ToyPartition)));
    let (select, passes) = (r.parse(select).unwrap(), r.parse("toy-pass").unwrap());
    (PassManager::new(r), select, passes)
}

/// What the toy writer writes; `toy.raw` it doesn't.
fn target() -> Target {
    Target::new("toy")
        .legal("core.instance")
        .legal("core.transform")
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
    // the whole world is one part; e0 is the first prototype, which the load gave the form
    assert!(
        e.starts_with("load: the reader put pbr.toy on e0, which the selection had not"),
        "{e}"
    );
    assert!(!events().contains(&"write".to_string()));
}

/// What the program's selection lowered is in every loaded part, and it is the program's: the load check holds the
/// reader to what its load adds, not to what the selection made.
#[test]
fn a_selection_may_lower_outside_the_reader_s_namespaces() {
    let (m, select, passes) = manager("toy-select, toy-lower");
    let (w, written) = writer(target());
    let report = convert_parts(&mut reader(2), &mut ToyParts::default(), w, &m, &select, &passes).unwrap();
    assert_eq!(report.parts, 2);
    assert_eq!(written.borrow().len(), 2);
    events();
}

/// The check is per entity: a load that puts a name the selection already made on entities the selection didn't
/// give it (here the prototypes, where the selection lowered the placements) is still the reader's, and refused.
#[test]
fn a_load_adding_a_name_the_selection_made_elsewhere_is_refused() {
    let (m, select, passes) = manager("toy-select, toy-lower");
    let mut bad = reader(2);
    bad.bad_load = true;
    let e = convert(&mut bad, writer(target()).0, &m, &select, &passes)
        .unwrap_err()
        .to_string();
    // e0 is the first prototype; the placements (e1, e3) carry the form from the selection
    assert!(
        e.starts_with("load: the reader put pbr.toy on e0, which the selection had not"),
        "{e}"
    );
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
/// summed once; what a pass left out is an unsupported notice; what a part dropped is a selection notice; the writer's omissions come back after each part and
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
    // what a pass left out for want of a form is an unsupported notice, per part
    let hats = find("toy-pass", "toy hats with no toy form, left out");
    assert_eq!((hats.kind, hats.count), (NoticeKind::Unsupported, 2));
    let dropped = find("convert", "made entities nothing keeps, dropped (toy.mesh)");
    assert_eq!((dropped.kind, dropped.count), (NoticeKind::Selection, 2));
    // after each part, before finishing, and the one finish returns
    assert_eq!(notices.iter().filter(|n| n.stage == "toy-writer").count(), 4);
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

/// Makes a marker for every placement it sees, from that placement (as a pass deriving content per source would),
/// and checks it is told which placements its part owns.
struct ToyPerPlacement;
impl Pass for ToyPerPlacement {
    fn name(&self) -> &'static str {
        "toy-per-placement"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let scope = w.resource::<PartScope>().ok_or("no part scope")?.clone();
        let placements: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        for p in placements {
            assert!(scope.roots.contains(&p) || scope.context.contains(&p));
            let m = w.spawn();
            w.insert(m, Name(format!("marker of {}", p.index())));
            log.place_from(m, p);
        }
        Ok(())
    }
}

/// Fails, after the passes before it ran.
struct ToyFail;
impl Pass for ToyFail {
    fn name(&self) -> &'static str {
        "toy-fail"
    }
    fn run(&self, _w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        Err("the toy pass failed".into())
    }
}

/// Notes an approximation and an omission, then fails.
struct ToyNoteThenFail;
impl Pass for ToyNoteThenFail {
    fn name(&self) -> &'static str {
        "toy-note-then-fail"
    }
    fn run(&self, _w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        log.approximate("corners rounded", 2);
        log.omit("toy lights left out", 1);
        Err("the toy pass gave up".into())
    }
}

/// Notes an approximation, then takes a placement's transform away (the verifier refuses the result).
struct ToyUnplace;
impl Pass for ToyUnplace {
    fn name(&self) -> &'static str {
        "toy-unplace"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        log.approximate("placements unplaced", 1);
        let e = w
            .iter::<Instance>()
            .map(|(e, _)| e)
            .next()
            .ok_or("no placement")?;
        w.remove::<primase_core::Transform>(e);
        Ok(())
    }
}

/// A resource of a source, which the toy writer does not write unless its target lists it.
#[derive(Clone, Debug, PartialEq)]
struct ToyRes;
impl Resource for ToyRes {
    const NAME: &'static str = "toy.res";
}
struct ToyAddRes;
impl Pass for ToyAddRes {
    fn name(&self) -> &'static str {
        "toy-add-res"
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.res"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        w.insert_resource(ToyRes);
        Ok(())
    }
}

fn manager_with(select: &str, passes: &str) -> (PassManager, Pipeline, Pipeline) {
    let mut r = PassRegistry::new();
    r.register("toy-select", |_| Ok(Box::new(ToySelect)));
    r.register("toy-pass", |_| Ok(Box::new(ToyPass)));
    r.register("toy-partition", |_| Ok(Box::new(ToyPartition)));
    r.register("toy-per-placement", |_| Ok(Box::new(ToyPerPlacement)));
    r.register("toy-fail", |_| Ok(Box::new(ToyFail)));
    r.register("toy-note-then-fail", |_| Ok(Box::new(ToyNoteThenFail)));
    r.register("toy-unplace", |_| Ok(Box::new(ToyUnplace)));
    r.register("toy-add-res", |_| Ok(Box::new(ToyAddRes)));
    r.register("two-cells", |_| Ok(Box::new(TwoCells)));
    let (select, passes) = (r.parse(select).unwrap(), r.parse(passes).unwrap());
    (PassManager::new(r), select, passes)
}

/// Writes nothing; keeps the names of the markers it is given.
struct Names(Rc<RefCell<Vec<String>>>);
impl Writer for Names {
    fn target(&self) -> Target {
        Target::new("names").legal("core.instance").legal("core.name")
    }
    fn write(&mut self, part: &World) -> Result<()> {
        assert!(part.resource::<PartScope>().is_none());
        self.0.borrow_mut().extend(
            part.iter::<Name>()
                .filter(|(_, n)| n.0.starts_with("marker"))
                .map(|(_, n)| n.0.clone()),
        );
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

/// Content a pass makes from a placement loaded as context is the owner's: every placement's marker is written once,
/// as by the whole world in one part, and the part scope is gone before writing.
#[test]
fn content_made_from_context_is_written_by_its_owner() {
    let (m, select, passes) = manager_with("toy-select, toy-partition", "toy-per-placement");
    let markers = Rc::new(RefCell::new(Vec::<String>::new()));
    let report = convert_parts(
        &mut reader(3),
        &mut PlannedParts::default(),
        Box::new(Names(markers.clone())),
        &m,
        &select,
        &passes,
    )
    .unwrap();
    events();
    let mut parted = markers.borrow().clone();
    parted.sort();
    assert_eq!(parted.len(), 3, "{parted:?}");
    parted.dedup();
    assert_eq!(parted.len(), 3, "a marker written twice: {parted:?}");
    assert_eq!(
        report.part_reports[0]
            .kept
            .get("entities placed from context, left to the part that owns their source (core.name)"),
        Some(&1)
    );
    let whole = Rc::new(RefCell::new(Vec::<String>::new()));
    let (m, select, passes) = manager_with("toy-select", "toy-per-placement");
    convert(
        &mut reader(3),
        Box::new(Names(whole.clone())),
        &m,
        &select,
        &passes,
    )
    .unwrap();
    events();
    let mut whole = whole.borrow().clone();
    whole.sort();
    assert_eq!(parted, whole);
}

/// A failure keeps the failing part's report as far as it got: the passes that ran before a failing pass and its own
/// record, marked failed; the whole
/// pipeline when the target check refuses the part; and the notices a writer gave on the call that failed.
#[test]
fn a_failing_part_keeps_what_its_passes_and_writer_reported() {
    let (m, select, passes) = manager_with("toy-select", "toy-pass, toy-fail");
    let (w, _) = writer(target());
    let e = convert_parts(&mut reader(1), &mut ToyParts::default(), w, &m, &select, &passes).unwrap_err();
    events();
    let e = e.downcast::<ConvertError>().unwrap();
    assert_eq!(e.during, "part 0: passes");
    let part = &e.partial.part_reports[0];
    // the pass that ran, then the failing one's record, marked failed
    assert_eq!(part.explicit.steps.len(), 2);
    assert!(part.explicit.steps[0].failed.is_none());
    assert_eq!(
        part.explicit.steps[1].failed.as_deref(),
        Some("pass toy-fail failed: the toy pass failed")
    );
    assert_eq!(
        part.explicit.steps[0]
            .log
            .approximations
            .get("toy meshes rounded"),
        Some(&2)
    );
    // the target refuses the part after its passes ran
    let (m, select, passes) = manager_with("toy-select", "toy-pass");
    let (w, _) = writer(target().dynamic_typed::<ToyMesh>(|m| m.0 < 6));
    let e = convert_parts(&mut reader(1), &mut ToyParts::default(), w, &m, &select, &passes).unwrap_err();
    events();
    let e = e.downcast::<ConvertError>().unwrap();
    assert_eq!(e.during, "part 0: target check");
    assert_eq!(e.partial.part_reports[0].explicit.steps.len(), 1);
    // the writer reports an omission on the write that fails
    let (mut w, _) = writer(target());
    w.fail_at = Some(0);
    let e = convert_parts(&mut reader(1), &mut ToyParts::default(), w, &m, &select, &passes).unwrap_err();
    events();
    let e = e.downcast::<ConvertError>().unwrap();
    assert_eq!(
        e.partial
            .reported
            .iter()
            .filter(|n| n.stage == "toy-writer")
            .count(),
        1
    );
}

/// A resource the writer does not write is a notice; one its target lists is not; the conversion's own resources
/// (the partition plan a planned part carries) never are.
#[test]
fn resources_the_writer_doesnt_write_are_reported() {
    let (m, select, passes) = manager_with("toy-select, toy-partition", "toy-pass, toy-add-res");
    let (w, _) = writer(target());
    let report = convert_parts(
        &mut reader(3),
        &mut PlannedParts::default(),
        w,
        &m,
        &select,
        &passes,
    )
    .unwrap();
    events();
    for p in &report.part_reports {
        assert_eq!(p.unsupported_resources, vec!["toy.res".to_string()]);
    }
    assert!(
        report
            .notices()
            .iter()
            .any(|n| n.reason == "resource toy.res: the writer doesn't write it" && n.count == 2)
    );
    let (w, _) = writer(target().legal("toy.res"));
    let report = convert_parts(
        &mut reader(3),
        &mut PlannedParts::default(),
        w,
        &m,
        &select,
        &passes,
    )
    .unwrap();
    events();
    assert!(
        report
            .part_reports
            .iter()
            .all(|p| p.unsupported_resources.is_empty())
    );
}

/// Placements standing at given ground positions, each with a prototype of its own whose authored box is given.
struct PlacedReader {
    at: Vec<([f32; 2], [[f64; 3]; 2])>,
    loads: Vec<BTreeSet<Entity>>,
}
impl Reader for PlacedReader {
    fn read(&mut self) -> Result<(World, Report)> {
        let mut w = World::new();
        for (k, (p, _)) in self.at.iter().enumerate() {
            let proto = w.spawn();
            w.insert(proto, Name(format!("proto {k}")));
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            let mut t = primase_core::Transform::IDENTITY;
            t.cols[3] = [p[0], 0.0, -p[1]];
            w.insert(e, t);
        }
        Ok((w, Report::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        self.loads.push(roots.clone());
        Ok((world.subset(&world.closure(roots.iter().copied())), Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core"]
    }
    fn prototype_box(&self, _world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        // the prototypes are the even entities, in order
        self.at.get(prototype.index() as usize / 2).map(|(_, b)| *b)
    }
}

/// Two parts by area; the second owns a placement whose pivot stands far east and whose authored box reaches back
/// west into the first part's area.
struct TwoCells;
impl Pass for TwoCells {
    fn name(&self) -> &'static str {
        "two-cells"
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
            by: "two-cells".into(),
            groups: vec![
                PartitionGroup {
                    key: "west".into(),
                    roots: BTreeSet::from([p[0]]),
                    context: BTreeSet::new(),
                    area: Some([-5.0, -5.0, 105.0, 105.0]),
                },
                PartitionGroup {
                    key: "east".into(),
                    roots: BTreeSet::from([p[1], p[2]]),
                    context: BTreeSet::new(),
                    area: Some([395.0, -5.0, 605.0, 105.0]),
                },
            ],
        });
        Ok(())
    }
}

/// A part reads, as context, a placement whose authored box reaches its area though its pivot stands far off.
#[test]
fn planned_parts_read_what_reaches_them_by_its_authored_box() {
    let (m, select, _) = manager_with("two-cells", "toy-pass");
    let passes = Pipeline::default();
    let unit = [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
    // a 450 m long object standing at 500 east, reaching back to 50 east
    let long = [[-450.0, 0.0, -1.0], [1.0, 1.0, 1.0]];
    let mut reader = PlacedReader {
        at: vec![([10.0, 10.0], unit), ([500.0, 10.0], long), ([510.0, 10.0], unit)],
        loads: Vec::new(),
    };
    let (w, written) = writer(Target::new("t").carry_unknown());
    convert_parts(&mut reader, &mut PlannedParts::default(), w, &m, &select, &passes).unwrap();
    events();
    // the west part loaded its own placement and the long one, and wrote only its own
    assert_eq!(reader.loads[0].len(), 2);
    assert_eq!(written.borrow()[0].placements.len(), 1);
    assert_eq!(reader.loads[1].len(), 2);
}

/// What a malformed reader gets wrong: one fault per case.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// A prototype mesh without positions, read.
    MeshOnRead,
    /// A resource outside the reader's namespaces, read.
    ResourceOnRead,
    /// A texture whose level is the wrong size, loaded.
    TextureOnLoad,
    /// A resource outside its namespaces, loaded.
    ResourceOnLoad,
    /// Nothing wrong, and nothing read.
    Empty,
}

/// A toy resource of a namespace no reader here emits.
#[derive(Clone, Debug, PartialEq)]
struct Stray;
impl primase_core::Resource for Stray {
    const NAME: &'static str = "other.stray";
}

struct Malformed(Fault);

impl Reader for Malformed {
    fn read(&mut self) -> Result<(World, Report)> {
        event("read");
        let mut w = World::new();
        if self.0 == Fault::Empty {
            return Ok((w, Report::new()));
        }
        let proto = w.spawn();
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(p, primase_core::Transform::IDENTITY);
        match self.0 {
            Fault::MeshOnRead => {
                w.insert(
                    proto,
                    primase_core::Mesh::new(vec![primase_core::Primitive {
                        topology: primase_core::Topology::Triangles,
                        streams: Default::default(),
                        indices: None,
                        material: None,
                    }]),
                );
            }
            Fault::ResourceOnRead => {
                w.insert_resource(Stray);
            }
            _ => {}
        }
        Ok((w, Report::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        event("load");
        let mut w = world.subset(&world.closure(roots.iter().copied()));
        match self.0 {
            Fault::TextureOnLoad => {
                let t = w.spawn();
                w.insert(
                    t,
                    primase_core::Texture {
                        width: 4,
                        height: 4,
                        format: primase_core::TexFormat::Rgba8Unorm,
                        levels: vec![vec![0; 63]],
                        color_space: primase_core::ColorSpace::Srgb,
                        role: primase_core::TexRole::Color,
                        normal: None,
                    },
                );
            }
            Fault::ResourceOnLoad => {
                w.insert_resource(Stray);
            }
            _ => {}
        }
        Ok((w, Report::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some([[0.0; 3], [1.0; 3]])
    }
}

/// What was read and what was loaded are checked before anything uses them: malformed data or a resource outside
/// the reader's namespaces stops the conversion at the read or the load, before the selection, the plan or a pass
/// runs, with the partial report kept; an empty world passes the checks.
#[test]
fn read_and_loaded_data_are_checked_before_any_pass_runs() {
    let run = |fault| {
        let (m, select, passes) = manager("toy-select");
        let (w, _) = writer(target());
        let r = convert_parts(
            &mut Malformed(fault),
            &mut ToyParts::default(),
            w,
            &m,
            &select,
            &passes,
        );
        (r, events())
    };
    for (fault, during, words) in [
        (Fault::MeshOnRead, "read", "no positions"),
        (Fault::ResourceOnRead, "read", "other.stray"),
        (Fault::TextureOnLoad, "part 0: load", "bytes, not 64"),
        (Fault::ResourceOnLoad, "part 0: load", "other.stray"),
    ] {
        let (r, ev) = run(fault);
        let e = r.unwrap_err().downcast::<ConvertError>().unwrap();
        assert_eq!(e.during, during, "{words}");
        assert!(
            format!("{:?}", e.error).contains(words) || e.error.to_string().contains(words),
            "{words}: {e:?}"
        );
        // the sentinel: no pass ran on what the check refused
        if during == "read" {
            assert_eq!(ev, ["read"], "{words}");
        } else {
            assert!(!ev.contains(&"passes".to_owned()), "{words}: {ev:?}");
        }
        assert!(
            e.partial
                .reported
                .iter()
                .any(|n| n.reason.contains("failed during"))
        );
    }
    let (r, _) = run(Fault::Empty);
    r.unwrap();
}

/// A pass that fails, by its own error or by what the checks after it refuse, keeps its step: the record is marked
/// failed and holds what the pass had noted, so the conversion's partial report still names its omissions and
/// approximations, each once.
#[test]
fn a_failed_pass_keeps_what_it_noted() {
    for (passes, why, notes) in [
        (
            "toy-note-then-fail",
            "the toy pass gave up",
            vec![
                ("corners rounded", NoticeKind::Approximation, 2),
                ("toy lights left out", NoticeKind::Unsupported, 1),
            ],
        ),
        (
            "toy-unplace",
            "core.transform",
            vec![("placements unplaced", NoticeKind::Approximation, 1)],
        ),
    ] {
        // two placements in one part, so taking one's transform leaves the name in the world (the verifier's case)
        let (m, select, passes_p) = manager_with("toy-select", passes);
        let (w, _) = writer(target());
        let mut whole = primase_core::convert::WholeWorld::default();
        let e = convert_parts(&mut reader(2), &mut whole, w, &m, &select, &passes_p).unwrap_err();
        events();
        let e = e.downcast::<ConvertError>().unwrap();
        assert_eq!(e.during, "part 0: passes", "{passes}");
        let step = e.partial.part_reports[0].explicit.steps.last().unwrap();
        assert_eq!(step.pass, passes);
        assert!(
            step.failed.as_deref().is_some_and(|f| f.contains(why)),
            "{passes}: {:?}",
            step.failed
        );
        let notices = e.partial.notices();
        for (reason, kind, count) in notes {
            let found: Vec<_> = notices.iter().filter(|n| n.reason == reason).collect();
            assert_eq!(found.len(), 1, "{passes}: {reason}: {found:?}");
            assert_eq!(
                (found[0].kind, found[0].count),
                (kind, count),
                "{passes}: {reason}"
            );
        }
    }
}

/// A planned part states its group's area, and loads as context the world collision soups whose points reach it as
/// well as the placements (a part's operations reading the ground around its own content need both).
#[test]
fn planned_parts_state_their_area_and_load_the_soups_reaching_it() {
    let mut w = World::new();
    let proto = w.spawn();
    let placed = w.spawn();
    w.insert(
        placed,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = primase_core::Transform::IDENTITY;
    t.cols[3] = [10.0, 0.0, -10.0];
    w.insert(placed, t);
    // a world soup from east 200 to 210
    let soup = w.spawn();
    w.insert(
        soup,
        primase_core::Collision {
            positions: vec![[200.0, 0.0, -1.0], [210.0, 0.0, -1.0], [205.0, 0.0, -5.0]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        },
    );
    let (west, east) = ([-5.0, -5.0, 250.0, 105.0], [150.0, -5.0, 300.0, 105.0]);
    w.insert_resource(PartitionPlan {
        by: "test".into(),
        groups: vec![
            PartitionGroup {
                key: "west".into(),
                roots: BTreeSet::from([placed]),
                context: BTreeSet::new(),
                area: Some(west),
            },
            PartitionGroup {
                key: "east".into(),
                roots: BTreeSet::from([soup]),
                context: BTreeSet::new(),
                area: Some(east),
            },
        ],
    });
    let mut parts = PlannedParts::default();
    parts
        .plan(&w, &|_| Some([[-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]]))
        .unwrap();
    let a = parts.next_part().unwrap().unwrap();
    let b = parts.next_part().unwrap().unwrap();
    // the west area reaches the soup; the east area does not reach the placement
    assert_eq!((a.area, a.context), (Some(west), BTreeSet::from([soup])));
    assert_eq!((b.area, b.context), (Some(east), BTreeSet::new()));
    // the whole world states no area
    let mut whole = primase_core::convert::WholeWorld::default();
    whole.plan(&w, &|_| None).unwrap();
    assert_eq!(whole.next_part().unwrap().unwrap().area, None);
}

/// Requires the given names (none of which the toy reader or passes make, but `toy.mesh`) and records that it ran;
/// opts in to being skipped in a part without them when the flag says so.
struct ToyNeeds(&'static [&'static str], bool);
impl Pass for ToyNeeds {
    fn name(&self) -> &'static str {
        "toy-needs"
    }
    fn part_may_lack_inputs(&self) -> bool {
        self.1
    }
    fn requires(&self) -> &[&'static str] {
        self.0
    }
    fn run(&self, _w: &mut World, _log: &mut PassLog) -> std::result::Result<(), String> {
        event("needs ran");
        Ok(())
    }
}

/// A cape on the part's entity e1 (the toy reader's first placement): a name one part holds and the other not.
#[derive(Clone, Debug, PartialEq)]
struct ToyCape;
impl primase_core::Component for ToyCape {
    const NAME: &'static str = "toy.cape";
}
struct CapeOnFirst;
impl Pass for CapeOnFirst {
    fn name(&self) -> &'static str {
        "toy-cape-on-first"
    }
    fn provides(&self) -> &[&'static str] {
        &["toy.cape"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let first: Vec<Entity> = w
            .iter::<Instance>()
            .map(|(e, _)| e)
            .filter(|e| e.index() == 1)
            .collect();
        for e in first {
            w.insert(e, ToyCape);
        }
        log.count("capes", 1);
        Ok(())
    }
}

/// A pass that opts in (part_may_lack_inputs) and finds none of its required names in a part has nothing to do there:
/// the part run skips it, recorded on its step and counted per pass. A pass that does not opt in fails in the same
/// part; partial presence fails; an opted-in pass skipped in every part fails the conversion as a whole run would;
/// the selection (a whole-world run) stays strict.
#[test]
fn a_part_with_none_of_a_passs_required_names_skips_it_recorded() {
    let (m, select, _) = manager("toy-select");
    let with = |needs: ToyNeeds| Pipeline {
        passes: vec![Box::new(ToyPass), Box::new(CapeOnFirst), Box::new(needs)],
    };
    let failure = |select: &Pipeline, passes: &Pipeline| {
        let e = convert_parts(
            &mut reader(2),
            &mut ToyParts::default(),
            writer(target()).0,
            &m,
            select,
            passes,
        )
        .unwrap_err()
        .to_string();
        events();
        e
    };
    // opted in: one part holds toy.cape, the other not, so the pass runs once and is skipped once
    let (w, written) = writer(target());
    let report = convert_parts(
        &mut reader(2),
        &mut ToyParts::default(),
        w,
        &m,
        &select,
        &with(ToyNeeds(&["toy.cape"], true)),
    )
    .unwrap();
    assert_eq!(written.borrow().len(), 2);
    assert_eq!(events().iter().filter(|e| *e == "needs ran").count(), 1);
    let skipped: Vec<&Vec<String>> = report
        .part_reports
        .iter()
        .map(|p| {
            &p.explicit
                .steps
                .iter()
                .find(|s| s.pass == "toy-needs")
                .unwrap()
                .skipped
        })
        .collect();
    assert_eq!(skipped.iter().filter(|s| s.is_empty()).count(), 1);
    assert_eq!(skipped.iter().filter(|s| s.as_slice() == ["toy.cape"]).count(), 1);
    assert_eq!(
        report.counts[&format!("toy-needs: {}", primase_core::pass::SKIPPED)],
        1
    );

    // not opted in, the same parts: the part without toy.cape fails with the requires error
    let e = failure(&select, &with(ToyNeeds(&["toy.cape"], false)));
    assert!(
        e.contains("pass toy-needs requires `toy.cape`, which the world does not have"),
        "{e}"
    );

    // opted in but some of its names present and not all: fails
    let e = failure(&select, &with(ToyNeeds(&["toy.mesh", "toy.mantle"], true)));
    assert!(
        e.contains("requires `toy.mantle`, which the world does not have"),
        "{e}"
    );

    // opted in and skipped in every part: the conversion fails as a whole run would
    let passes = Pipeline {
        passes: vec![Box::new(ToyPass), Box::new(ToyNeeds(&["toy.mantle"], true))],
    };
    let e = failure(&select, &passes);
    assert!(
        e.contains("pass toy-needs requires `toy.mantle`, which the world does not have"),
        "{e}"
    );

    // the selection is a whole-world run: strict even for a pass that opts in
    let select = Pipeline {
        passes: vec![Box::new(ToyNeeds(&["toy.cape"], true))],
    };
    let passes = Pipeline {
        passes: vec![Box::new(ToyPass)],
    };
    let e = failure(&select, &passes);
    assert!(
        e.contains("requires `toy.cape`, which the world does not have"),
        "{e}"
    );
}

/// A part's declared output (`PartScope::declare`, `PartScope::output`): each placement classified when declared, while
/// its source is in the world: placed with no source or from an entity the roots reach is the part's, placed from its
/// context is the owner's; a source a later step removes does not change that, and what was made and not placed is not
/// output.
#[test]
fn a_parts_output_is_its_roots_and_what_it_declared() {
    let mut w = World::new();
    let root = w.spawn();
    w.insert(root, Name("root".into()));
    let halo = w.spawn();
    w.insert(halo, Name("halo".into()));
    let (from_root, from_halo, alone, temporary) = (w.spawn(), w.spawn(), w.spawn(), w.spawn());
    for e in [from_root, from_halo, alone, temporary] {
        w.insert(e, Name("made".into()));
    }
    w.insert_resource(PartScope {
        roots: BTreeSet::from([root]),
        context: BTreeSet::from([halo]),
        whole: false,
        area: None,
        first_made: Some(from_root.index()),
        placed: BTreeMap::new(),
    });
    PartScope::declare(
        &mut w,
        &BTreeSet::from([from_root, from_halo, alone]),
        &BTreeMap::from([(from_root, root), (from_halo, halo)]),
        &BTreeSet::new(),
    );
    // the root a later step removes: what was placed from it stays the part's
    w.despawn(root);
    let out = w.resource::<PartScope>().unwrap().output(&w);
    assert_eq!(out.placed, BTreeSet::from([from_root, alone]));
    assert_eq!(out.from_context, BTreeSet::from([from_halo]));
    assert!(!out.keep.contains(&temporary) && !out.keep.contains(&halo));
    assert!(out.keep.contains(&from_root) && out.keep.contains(&alone));
}

/// Declared output derived from declared output is the part's, whatever generation and order: a placement from one an
/// earlier pass placed, chains within one pass declared in any order, and one from a source the same pass consumed
/// (what the part owned before the pass). A chain from context stays context's.
#[test]
fn declared_output_from_declared_output_is_the_parts() {
    let mut w = World::new();
    let root = w.spawn();
    w.insert(root, Name("root".into()));
    let halo = w.spawn();
    w.insert(halo, Name("halo".into()));
    let made: Vec<Entity> = (0..9)
        .map(|_| {
            let e = w.spawn();
            w.insert(e, Name("made".into()));
            e
        })
        .collect();
    let [a, b, late, early, eaten_from, ctx1, ctx2, mid, tail] = made[..] else {
        unreachable!()
    };
    w.insert_resource(PartScope {
        roots: BTreeSet::from([root]),
        context: BTreeSet::from([halo]),
        whole: false,
        area: None,
        first_made: Some(a.index()),
        placed: BTreeMap::new(),
    });
    // pass 1 places a on its own
    PartScope::declare(&mut w, &BTreeSet::from([a]), &BTreeMap::new(), &BTreeSet::new());
    // pass 2 places b from a (a reaches no root)
    let before = w.resource::<PartScope>().unwrap().owned(&w);
    PartScope::declare(&mut w, &BTreeSet::from([b]), &BTreeMap::from([(b, a)]), &before);
    // pass 3: a chain within one pass, the later link first in declaration order (late < early is not needed: both
    // orders must hold), plus one from a root the pass consumed, and a chain from context
    let before = w.resource::<PartScope>().unwrap().owned(&w);
    // and an intermediate the pass made, placed, derived from, and consumed before it returned
    w.despawn(root);
    w.despawn(mid);
    PartScope::declare(
        &mut w,
        &BTreeSet::from([late, early, eaten_from, ctx1, ctx2, mid, tail]),
        &BTreeMap::from([
            (late, early),
            (early, b),
            (eaten_from, root),
            (ctx1, halo),
            (ctx2, ctx1),
            (mid, a),
            (tail, mid),
        ]),
        &before,
    );
    let out = w.resource::<PartScope>().unwrap().output(&w);
    assert_eq!(out.placed, BTreeSet::from([a, b, late, early, eaten_from, tail]));
    assert_eq!(out.from_context, BTreeSet::from([ctx1, ctx2]));
    for e in [a, b, late, early, eaten_from, tail] {
        assert!(out.keep.contains(&e), "{e:?} kept");
    }
    assert!(!out.keep.contains(&ctx1) && !out.keep.contains(&ctx2));
}

/// Per root placement: makes A placed from it, B placed from A, then consumes A, in one pass.
struct ToyChain;
impl Pass for ToyChain {
    fn name(&self) -> &'static str {
        "toy-chain"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.name"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let roots = w.resource::<PartScope>().ok_or("no part scope")?.roots.clone();
        let placed: Vec<Entity> = roots.into_iter().filter(|&e| w.has::<Instance>(e)).collect();
        for root in placed {
            let a = w.spawn();
            w.insert(a, Name("intermediate".into()));
            log.place_from(a, root);
            let b = w.spawn();
            w.insert(b, Name("derived".into()));
            log.place_from(b, a);
            w.despawn(a);
        }
        Ok(())
    }
}

/// Through the pass manager and a conversion in parts: output derived from an intermediate the same pass placed and
/// consumed reaches the writer (one more named entity per part than without the pass).
#[test]
fn output_from_a_consumed_intermediate_reaches_the_writer() {
    let (m, select, _) = manager("toy-select");
    let named = |chain: bool| {
        let mut passes: Vec<Box<dyn Pass>> = vec![Box::new(ToyPass)];
        if chain {
            passes.push(Box::new(ToyChain));
        }
        let (w, written) = writer(target());
        convert_parts(
            &mut reader(2),
            &mut ToyParts::default(),
            w,
            &m,
            &select,
            &Pipeline { passes },
        )
        .unwrap();
        events();
        let n: Vec<usize> = written.borrow().iter().map(|g| g.named).collect();
        n
    };
    let (without, with) = (named(false), named(true));
    assert_eq!(with.len(), 2);
    assert_eq!(with, without.iter().map(|n| n + 1).collect::<Vec<_>>());
}
