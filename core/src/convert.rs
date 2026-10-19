//! Convert: one reader, the program's passes, one writer, and the loop that drives them ([`convert_parts`],
//! [`convert`]).
//!
//! The loop owns the streaming: it reads the world and runs the program's selection on it, lets a [`PartSource`] plan
//! the parts from that world (geometry not loaded yet), then loads each part through the reader (its roots and the
//! context its passes need), runs the program's passes on it once, checks the part against what the writer writes
//! without changing it, and hands the part's own output to the writer read-only, one part in memory at a time. It
//! adds no pass of its own: what the writer doesn't write is reported as a notice, and values it can't encode are an
//! error. It does no I/O of its own: reading and writing are the reader's and the writer's, and what they report comes
//! back in the [`ConvertReport`]: every part's pass reports whole ([`PartReport`]), and the notices the reader and
//! writer give ([`Notice`]). A conversion that fails keeps what it had reported ([`ConvertError`]).
//!
//! How the content is divided is a choice: [`WholeWorld`] is one part; [`PlannedParts`] takes the parts a partition
//! pass (as `partition-grid`) put in the selected world's [`PartitionPlan`]; a destination may plan its own.
//!
//! Every part states whether it is the whole of what is written ([`Part::whole`]): the source says so, as it alone
//! knows how it divides the content, and the part's passes read it in [`PartScope::whole`]; an operation that needs
//! the whole (a map's floor, its spawn, a budget over all of it) refuses a part that is not. A world composed
//! directly, outside this loop (no `core.part_scope` resource), is by definition the whole of what is written.
use std::collections::{BTreeMap, BTreeSet};

use crate::pass::{PassManager, Pipeline, RunReport, Target};
use crate::{Entity, Resource, World};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Labelled lines for the log: what a reader read or loaded, what a writer wrote.
pub type Report = Vec<(String, String)>;

/// A source of worlds: the world to plan from, then the parts asked for, loaded.
pub trait Reader {
    /// The world to plan from: placements, prototypes and their state; geometry and textures not loaded yet.
    fn read(&mut self) -> Result<(World, Report)>;
    /// The subset of `world` that `roots` reach, with everything a writer needs loaded (meshes, textures); entity ids
    /// stay `world`'s, and what loading spawns is reached through the roots.
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)>;
    /// The component namespaces it may put in a world (`core`, its own dialect): a reader reads, it lowers nothing
    /// (no `pbr.*` unless its source format is PBR). [`check_emits`] holds it to that after reading, and
    /// [`check_load_emits`] holds what each load adds to it.
    fn emits(&self) -> &'static [&'static str];
    /// A prototype's box (core frame) before its geometry is loaded, where the reader knows it.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]>;
    /// What it noticed since it was last asked (source it can't read, what it tolerated), taken after reading and
    /// after each load. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
}

/// Whether `name` (a component or resource name) is in one of the namespaces in `emits`.
fn emitted(name: &str, emits: &[&str]) -> bool {
    emits.contains(&name.split('.').next().unwrap_or(""))
}

/// An error naming the first component or resource of `w` outside the reader's namespaces (`when`: after what).
pub fn check_emits(w: &World, emits: &[&str], when: &str) -> Result<()> {
    if let Some(name) = w.component_names().into_iter().find(|n| !emitted(n, emits)) {
        return Err(format!("{when}: the reader put {name} in the world (it emits {emits:?})").into());
    }
    if let Some(name) = w.resource_names().into_iter().find(|n| !emitted(n, emits)) {
        return Err(
            format!("{when}: the reader put the resource {name} in the world (it emits {emits:?})").into(),
        );
    }
    Ok(())
}

/// An error naming the first component a load added outside the reader's namespaces: a component of the loaded `part`
/// on an entity the `selected` world didn't have it on (an entity the load spawned, or one the selection hadn't given
/// it). What the program's selection put there is the program's, not the reader's, so a selection that lowers is let
/// through and a reader whose load adds the same name to other entities is not. It compares which entities carry each
/// name, not values: a load that overwrites a component the selection already put on that entity is not seen.
pub fn check_load_emits(selected: &World, part: &World, emits: &[&str]) -> Result<()> {
    let before: BTreeSet<&str> = selected.resource_names().into_iter().collect();
    if let Some(name) = part
        .resource_names()
        .into_iter()
        .find(|n| !emitted(n, emits) && !before.contains(n))
    {
        return Err(format!(
            "load: the reader put the resource {name} in the part, which the selection had not (it emits {emits:?})"
        )
        .into());
    }
    for name in part.component_names() {
        if emitted(name, emits) {
            continue;
        }
        let before: BTreeSet<Entity> = selected.entities_named(name).into_iter().collect();
        if let Some(e) = part
            .entities_named(name)
            .into_iter()
            .find(|e| !before.contains(e))
        {
            return Err(format!(
                "load: the reader put {name} on {e:?}, which the selection had not (it emits {emits:?})"
            )
            .into());
        }
    }
    Ok(())
}

/// One part to load: the roots it outputs, and the context its passes need around them (neighbouring placements a
/// collision or normal operation reads). The reader loads both; only the roots' closure, with what the passes placed
/// for it, goes to the writer, so content a part only reads is written by the part that owns it. The passes see which
/// is which in the part's [`PartScope`]; content they make from a context entity ([`crate::pass::PassLog::place_from`])
/// is left to the part that owns that entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub roots: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// Whether this part is the whole of what is written: every selected entity, nothing in another part. Stated by
    /// the source (no default): an operation that needs the whole refuses a part that is not.
    pub whole: bool,
}

/// How a conversion divides the selected world into parts.
pub trait PartSource {
    /// Plan from the selected world, its geometry not loaded yet: `boxes` gives a prototype's box (core frame) where
    /// the reader knows it.
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()>;
    /// The next part to load, `None` when every part is done.
    fn next_part(&mut self) -> Result<Option<Part>>;
    /// The part just asked for, as the writer got it: for a source that refines its plan from loaded geometry.
    fn written(&mut self, _part: &World) -> Result<()> {
        Ok(())
    }
}

/// One part: every entity of the selected world.
#[derive(Debug, Default)]
pub struct WholeWorld {
    roots: Option<BTreeSet<Entity>>,
}

impl PartSource for WholeWorld {
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.roots = Some(world.entities().collect());
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        Ok(self.roots.take().map(|roots| Part {
            roots,
            context: BTreeSet::new(),
            whole: true,
        }))
    }
}

/// `core.partition_plan`: how a pass divided the content into output parts, in order. Each group owns its roots
/// (every root in one group) and names the context its passes need (roots of other groups, loaded but written by
/// their owner).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartitionPlan {
    /// What made it (`partition-grid{cell=200}`).
    pub by: String,
    pub groups: Vec<PartitionGroup>,
}

/// One part of a [`PartitionPlan`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartitionGroup {
    /// A name for reports (`cell 3,-2`).
    pub key: String,
    pub roots: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// The ground area `[west, south, east, north]` (metres, half-open) whose content the group's operations read:
    /// what it owns and its halo. [`PlannedParts`] adds to the context every placement whose authored box reaches it.
    pub area: Option<[f64; 4]>,
}

/// `core.part_scope`: which entities of a part the conversion writes (its roots, and what they reach) and which it
/// only loaded as context, for passes that make content per source entity. Put in the part's world before its passes
/// and taken out before writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartScope {
    pub roots: BTreeSet<Entity>,
    pub context: BTreeSet<Entity>,
    /// The part's [`Part::whole`]: whether it is the whole of what is written.
    pub whole: bool,
}

impl Resource for PartScope {
    const NAME: &'static str = "core.part_scope";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.roots.iter().chain(&self.context).copied());
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        self.roots.retain(|&e| keep(e));
        self.context.retain(|&e| keep(e));
    }
}

/// The resources a conversion makes for its own work, never content a writer is to write.
pub const OPERATIONAL_RESOURCES: &[&str] = &[PartitionPlan::NAME, PartScope::NAME];

impl Resource for PartitionPlan {
    const NAME: &'static str = "core.partition_plan";
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, crate::JsonError> {
        let groups: Vec<serde_json::Value> = self
            .groups
            .iter()
            .map(|g| {
                serde_json::json!({
                    "key": g.key,
                    "roots": g.roots.iter().map(|e| e.index()).collect::<Vec<_>>(),
                    "context": g.context.iter().map(|e| e.index()).collect::<Vec<_>>(),
                    "area": g.area,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "by": self.by, "groups": groups })))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for g in &self.groups {
            out.extend(g.roots.iter().chain(&g.context).copied());
        }
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        for g in &mut self.groups {
            g.roots.retain(|&e| keep(e));
            g.context.retain(|&e| keep(e));
        }
    }
}

/// The parts of the selected world's [`PartitionPlan`], in its order. A world without one is an error: the program
/// runs the partition pass in its selection. A group with an area also reads, as context, every other placement whose
/// prototype's authored box (the reader's, before geometry is loaded) reaches that area, so a long object whose pivot
/// stands far off is still in its neighbours' operations.
#[derive(Debug, Default)]
pub struct PlannedParts {
    groups: std::vec::IntoIter<PartitionGroup>,
    /// Whether the plan has one group (a part is then the whole when it needs no context).
    single: bool,
}

/// The ground box `[west, south, east, north]` of a placement's prototype box.
fn ground_box(t: &crate::Transform, b: [[f64; 3]; 2]) -> [f64; 4] {
    let mut out = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for k in 0..8 {
        let c = [b[k & 1][0], b[(k >> 1) & 1][1], b[(k >> 2) & 1][2]];
        let p: [f64; 3] = std::array::from_fn(|i| {
            (0..3).map(|j| f64::from(t.cols[j][i]) * c[j]).sum::<f64>() + f64::from(t.cols[3][i])
        });
        let (east, north) = (p[0], -p[2]);
        out = [
            out[0].min(east),
            out[1].min(north),
            out[2].max(east),
            out[3].max(north),
        ];
    }
    out
}

impl PartSource for PlannedParts {
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        let plan = world
            .resource::<PartitionPlan>()
            .ok_or("no core.partition_plan in the selected world: run a partition pass in the selection")?;
        let placed: Vec<(Entity, [f64; 4])> = world
            .iter::<crate::Instance>()
            .filter_map(|(e, i)| {
                let t = world.get::<crate::Transform>(e)?;
                Some((e, ground_box(t, boxes(i.prototype)?)))
            })
            .collect();
        let mut groups = plan.groups.clone();
        for g in &mut groups {
            let Some(a) = g.area else { continue };
            for &(e, b) in &placed {
                if !g.roots.contains(&e) && b[0] < a[2] && b[2] >= a[0] && b[1] < a[3] && b[3] >= a[1] {
                    g.context.insert(e);
                }
            }
        }
        self.single = groups.len() == 1;
        self.groups = groups.into_iter();
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        // the whole only when the plan has one group and it needs no context
        let single = self.single;
        Ok(self.groups.next().map(|g| Part {
            whole: single && g.context.is_empty(),
            roots: g.roots,
            context: g.context,
        }))
    }
}

/// A destination. It is handed each part, loaded and through the program's passes: the closure of its roots and of
/// every entity a pass placed (content nothing references, such as a marker; [`crate::pass::PassLog::place`]).
/// Other entities the passes made are in it only where something in it references them.
pub trait Writer {
    /// What it writes: every part is checked against it before it is written ([`Target::check_prepared`]), without
    /// changing it. A name it doesn't write is reported; a value of a name it writes that its rule refuses is an error.
    fn target(&self) -> Target;
    fn write(&mut self, part: &World) -> Result<()>;
    /// What it noticed since it was last asked (what it omitted because it doesn't support it), taken after each
    /// part and before [`Writer::finish`]. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
    /// Finish writing (indexes, manifests): its report, and what it noticed while finishing.
    fn finish(self: Box<Self>) -> Result<Finished>;
}

/// What a writer's [`Writer::finish`] returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finished {
    pub report: Report,
    /// What it noticed while finishing (an omission it can only tell once every part is in).
    pub notices: Vec<Notice>,
}

impl From<Report> for Finished {
    fn from(report: Report) -> Finished {
        Finished {
            report,
            notices: Vec::new(),
        }
    }
}

/// What a [`Notice`] tells the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeKind {
    /// Something in the world or source the stage doesn't support, left out.
    Unsupported,
    /// Something kept, but not exactly.
    Approximation,
    /// Something left out or chosen deliberately, by a selection or by what a part keeps.
    Selection,
    /// What a stage was doing when it failed.
    FailureContext,
    /// What a diagnostic the program turned on found (conservation: a pass's losses differ from what it declared).
    Diagnostic,
}

/// The most examples a notice keeps; its count still counts every case.
pub const NOTICE_EXAMPLES: usize = 8;

/// One thing a conversion stage tells the program: what stage, what kind, why, how many, and a few examples (source
/// names or ids) of what it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The reader, a pass (as in its pipeline), `convert` or the writer.
    pub stage: String,
    pub kind: NoticeKind,
    pub reason: String,
    pub count: u64,
    /// At most [`NOTICE_EXAMPLES`].
    pub examples: Vec<String>,
}

impl Notice {
    pub fn new(stage: impl Into<String>, kind: NoticeKind, reason: impl Into<String>, count: u64) -> Notice {
        Notice {
            stage: stage.into(),
            kind,
            reason: reason.into(),
            count,
            examples: Vec::new(),
        }
    }

    /// Add an example, unless it has [`NOTICE_EXAMPLES`] already.
    pub fn example(mut self, e: impl Into<String>) -> Notice {
        if self.examples.len() < NOTICE_EXAMPLES {
            self.examples.push(e.into());
        }
        self
    }
}

impl std::fmt::Display for Notice {
    /// `approximation lower-gta-pbr: specular folded into roughness x 12 (part 0, part 3)`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            NoticeKind::Unsupported => "unsupported",
            NoticeKind::Approximation => "approximation",
            NoticeKind::Selection => "selection",
            NoticeKind::FailureContext => "failure",
            NoticeKind::Diagnostic => "diagnostic",
        };
        write!(f, "{kind} {}: {} x {}", self.stage, self.reason, self.count)?;
        if !self.examples.is_empty() {
            write!(f, " ({})", self.examples.join(", "))?;
        }
        Ok(())
    }
}

/// What one part of a conversion went through, in full.
#[derive(Debug, Default)]
pub struct PartReport {
    /// Its place in the order the writer asked for the parts, from 0.
    pub ordinal: usize,
    /// The roots asked for. Entity ids are the planning world's; a pass's own entities are the part's.
    pub roots: BTreeSet<Entity>,
    /// The context loaded with them, not written by this part.
    pub context: BTreeSet<Entity>,
    /// The reader's report on loading it.
    pub load: Report,
    /// The program's passes.
    pub explicit: RunReport,
    /// Names the writer doesn't write, with how many entities held them ([`Target::check_prepared`]).
    pub unsupported: Vec<(String, u64)>,
    /// Resources the writer doesn't write.
    pub unsupported_resources: Vec<String>,
    /// What the part kept and left out of what the passes made, by its components: `entities the passes placed (..)`,
    /// `made entities nothing keeps, dropped (..)`.
    pub kept: BTreeMap<String, u64>,
}

/// What a conversion did.
#[derive(Debug, Default)]
pub struct ConvertReport {
    /// The reader's report on reading.
    pub read: Report,
    /// The selection passes.
    pub select: RunReport,
    /// The reader's reports on loading, part by part.
    pub loads: Report,
    pub parts: usize,
    /// The bulk payload of every part, in the order written.
    pub part_sizes: Vec<PartSize>,
    /// Every pass's counts over all parts, as `<pass>: <what>`, and what the parts kept of what the passes made
    /// (`convert: <what>`). A sum of [`ConvertReport::part_reports`], kept for callers that print totals.
    pub counts: BTreeMap<String, u64>,
    /// Each part's reports, whole, in the order written.
    pub part_reports: Vec<PartReport>,
    /// What the reader and the writer noticed, in the order they reported it.
    pub reported: Vec<Notice>,
    /// The writer's report.
    pub written: Report,
}

impl ConvertReport {
    /// Every approximation, as `<pass>: <what>`, over the selection and every part, from the reports kept.
    pub fn approximations(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        let runs = std::iter::once(&self.select).chain(self.part_reports.iter().map(|p| &p.explicit));
        for run in runs {
            for step in &run.steps {
                for (k, v) in &step.log.approximations {
                    *out.entry(format!("{}: {k}", step.pass)).or_default() += v;
                }
            }
        }
        out
    }

    /// Every notice: the reader's and the writer's, then the passes' approximations, omissions and drops (what they
    /// left out on purpose: a selection, `<measure>: <why>`), what the parts left out and what the writer doesn't write,
    /// one notice per stage and reason over all parts, with the parts as examples.
    pub fn notices(&self) -> Vec<Notice> {
        let mut out = self.reported.clone();
        let mut derived: BTreeMap<(String, NoticeKind, String), (u64, Vec<String>)> = BTreeMap::new();
        let mut add = |stage: &str, kind: NoticeKind, reason: &str, n: u64, part: Option<usize>| {
            let (count, examples) = derived
                .entry((stage.to_owned(), kind, reason.to_owned()))
                .or_default();
            *count += n;
            if let Some(i) = part
                && examples.len() < NOTICE_EXAMPLES
            {
                examples.push(format!("part {i}"));
            }
        };
        for step in &self.select.steps {
            for (k, v) in &step.log.approximations {
                add(&step.pass, NoticeKind::Approximation, k, *v, None);
            }
            for (k, v) in &step.log.omitted {
                add(&step.pass, NoticeKind::Unsupported, k, *v, None);
            }
            for ((m, why), v) in &step.log.drops {
                add(
                    &step.pass,
                    NoticeKind::Selection,
                    &format!("{m}: {why}"),
                    *v,
                    None,
                );
            }
            for g in &step.conservation {
                add(&step.pass, NoticeKind::Diagnostic, &g.to_string(), 1, None);
            }
        }
        for p in &self.part_reports {
            for step in &p.explicit.steps {
                for (k, v) in &step.log.approximations {
                    add(&step.pass, NoticeKind::Approximation, k, *v, Some(p.ordinal));
                }
                for (k, v) in &step.log.omitted {
                    add(&step.pass, NoticeKind::Unsupported, k, *v, Some(p.ordinal));
                }
                for ((m, why), v) in &step.log.drops {
                    add(
                        &step.pass,
                        NoticeKind::Selection,
                        &format!("{m}: {why}"),
                        *v,
                        Some(p.ordinal),
                    );
                }
                for g in &step.conservation {
                    add(
                        &step.pass,
                        NoticeKind::Diagnostic,
                        &g.to_string(),
                        1,
                        Some(p.ordinal),
                    );
                }
            }
            for (k, v) in &p.kept {
                if k.starts_with("made entities nothing keeps") {
                    add("convert", NoticeKind::Selection, k, *v, Some(p.ordinal));
                }
            }
            for (name, n) in &p.unsupported {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("{name}: the writer doesn't write it"),
                    *n,
                    Some(p.ordinal),
                );
            }
            for name in &p.unsupported_resources {
                add(
                    "convert",
                    NoticeKind::Unsupported,
                    &format!("resource {name}: the writer doesn't write it"),
                    1,
                    Some(p.ordinal),
                );
            }
        }
        out.extend(
            derived
                .into_iter()
                .map(|((stage, kind, reason), (count, examples))| Notice {
                    stage,
                    kind,
                    reason,
                    count,
                    examples,
                }),
        );
        out
    }
}

/// A conversion that failed: the error, where, and everything reported before it: the parts written, the failing
/// part's report as far as it got (the passes that ran, the target check), and every notice the reader and writer
/// gave, those of the failing call included.
#[derive(Debug)]
pub struct ConvertError {
    pub error: Error,
    /// What was being done: `read`, `select`, `plan`, `part N: load`, `part N: passes`, `part N: write`, `finish`.
    pub during: String,
    pub partial: Box<ConvertReport>,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

/// The bulk payload one part held ([`World::payload_bytes`]), by component name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartSize {
    /// The roots the writer asked for.
    pub roots: usize,
    /// As the reader loaded it.
    pub loaded: BTreeMap<String, u64>,
    /// After the passes, as the writer got it.
    pub written: BTreeMap<String, u64>,
}

impl PartSize {
    pub fn loaded_total(&self) -> u64 {
        self.loaded.values().sum()
    }
    pub fn written_total(&self) -> u64 {
        self.written.values().sum()
    }
}

fn sizes(w: &World) -> BTreeMap<String, u64> {
    w.payload_bytes()
        .into_iter()
        .map(|(n, b)| (n.to_owned(), b))
        .collect()
}

/// Adds each step's counts to `counts` under `<pass>: <what>`.
fn tally(counts: &mut BTreeMap<String, u64>, report: &RunReport) {
    for step in &report.steps {
        for (k, v) in &step.log.counts {
            *counts.entry(format!("{}: {k}", step.pass)).or_default() += v;
        }
    }
}

/// Reads with `reader`, runs the program's `select` on the read world, lets `parts` plan, then loads each part, runs
/// the program's `passes` on it once, checks it against `writer`'s target and writes it (see the module
/// documentation). On failure the error is a [`ConvertError`] holding what was reported until then.
pub fn convert_parts(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let mut during = String::new();
    match run(
        reader,
        parts,
        writer,
        manager,
        select,
        passes,
        &mut report,
        &mut during,
    ) {
        Ok(()) => Ok(report),
        Err(error) => {
            report.reported.push(Notice::new(
                "convert",
                NoticeKind::FailureContext,
                format!("failed during {during}: {error}"),
                1,
            ));
            Err(Box::new(ConvertError {
                error,
                during,
                partial: Box::new(report),
            }))
        }
    }
}

/// [`convert_parts`] in one part: every entity of the selected world ([`WholeWorld`]).
pub fn convert(
    reader: &mut dyn Reader,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    convert_parts(
        reader,
        &mut WholeWorld::default(),
        writer,
        manager,
        select,
        passes,
    )
}

#[allow(clippy::too_many_arguments)]
fn run(
    reader: &mut dyn Reader,
    parts: &mut dyn PartSource,
    mut writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
    report: &mut ConvertReport,
    during: &mut String,
) -> Result<()> {
    "read".clone_into(during);
    let read = reader.read();
    report.reported.extend(reader.notices());
    let (mut world, read) = read?;
    report.read = read;
    check_emits(&world, reader.emits(), "read")?;
    // what was read is checked before anything uses it: the selection, the plan, an empty pipeline too
    manager.verify(&world, "read")?;
    "select".clone_into(during);
    manager.run(&mut world, select, &mut report.select)?;
    let target = writer.target();
    "plan".clone_into(during);
    parts.plan(&world, &|p| reader.prototype_box(&world, p))?;
    while let Some(Part {
        roots,
        context,
        whole,
    }) = parts.next_part()?
    {
        let ordinal = report.part_reports.len();
        *during = format!("part {ordinal}: load");
        let asked: BTreeSet<Entity> = roots.union(&context).copied().collect();
        let loaded = reader.load(&world, &asked);
        report.reported.extend(reader.notices());
        let (mut part, loaded) = loaded?;
        report.loads.extend(loaded.iter().cloned());
        let mut size = PartSize {
            roots: roots.len(),
            loaded: sizes(&part),
            ..PartSize::default()
        };
        check_load_emits(&world, &part, reader.emits())?;
        // what was loaded is checked before the part's passes use it
        manager.verify(&part, "load")?;
        *during = format!("part {ordinal}: passes");
        // the part's report is published before its passes run, so a failure keeps what ran
        report.part_reports.push(PartReport {
            ordinal,
            roots: roots.clone(),
            context: context.clone(),
            load: loaded,
            ..PartReport::default()
        });
        let part_report = report.part_reports.last_mut().expect("pushed");
        let loaded: BTreeSet<Entity> = part.entities().collect();
        part.insert_resource(PartScope {
            roots: roots.clone(),
            context: context.clone(),
            whole,
        });
        manager.run(&mut part, passes, &mut part_report.explicit)?;
        part.remove_resource::<PartScope>();
        // the part's output is the closure of its roots and of what the passes placed for it: content of its own
        // nothing need reference (as a marker; PassLog::place), and content made from an entity the part owns
        // (PassLog::place_from; made from its context, it is the owner's). Its context is not output, unless the
        // output reaches it. What the passes left unreferenced of what was loaded (materials of dropped geometry, of a
        // discarded level of detail) is not part of it, nor is what they made that nothing kept reaches: dropped, and
        // named in the counts by its components, as are the placed entities
        let owned = part.closure(roots.iter().copied());
        let mut from_context = BTreeSet::new();
        let mut placed = BTreeSet::new();
        for s in &part_report.explicit.steps {
            for &e in s.log.placed.iter().filter(|&&e| part.is_alive(e)) {
                match s.log.placed_from.get(&e) {
                    Some(source) if !owned.contains(source) => {
                        from_context.insert(e);
                    }
                    _ => {
                        placed.insert(e);
                    }
                }
            }
        }
        let keep = part.closure(roots.iter().copied().chain(placed.iter().copied()));
        let names: Vec<String> = part.component_names().into_iter().map(str::to_owned).collect();
        let kinds = |e: Entity| {
            names
                .iter()
                .filter(|n| part.entity_has_named(e, n))
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" + ")
        };
        for e in part.entities().filter(|e| !loaded.contains(e)) {
            let what = if placed.contains(&e) {
                "entities the passes placed"
            } else if from_context.contains(&e) {
                "entities placed from context, left to the part that owns their source"
            } else if keep.contains(&e) {
                continue;
            } else {
                "made entities nothing keeps, dropped"
            };
            *part_report
                .kept
                .entry(format!("{what} ({})", kinds(e)))
                .or_default() += 1;
        }
        for (k, v) in &part_report.kept {
            *report.counts.entry(format!("convert: {k}")).or_default() += v;
        }
        part.retain(&keep);
        *during = format!("part {ordinal}: target check");
        manager.verify(&part, "the passes")?;
        let checked = target.check_prepared(&part);
        part_report.unsupported = checked.unsupported;
        part_report.unsupported_resources = checked.unsupported_resources;
        if let Some((name, words, bad)) = checked.invalid.first() {
            return Err(format!(
                "{}: {} of {name} the writer can't write ({words}), the first {:?}",
                target.name,
                bad.len(),
                bad[0]
            )
            .into());
        }
        if let Some(name) = checked.invalid_resources.first() {
            return Err(format!(
                "{}: resource {name} refused ({})",
                target.name,
                target.rule_words(name).unwrap_or_default()
            )
            .into());
        }
        let explicit = part_report.explicit.clone();
        tally(&mut report.counts, &explicit);
        size.written = sizes(&part);
        report.part_sizes.push(size);
        *during = format!("part {ordinal}: write");
        let written = writer.write(&part);
        report.reported.extend(writer.notices());
        written?;
        parts.written(&part)?;
        report.parts += 1;
    }
    "finish".clone_into(during);
    report.reported.extend(writer.notices());
    let finished = writer.finish()?;
    report.reported.extend(finished.notices);
    report.written = finished.report;
    Ok(())
}
