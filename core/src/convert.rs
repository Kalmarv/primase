//! Convert: one reader, the passes, one writer, and the loop that drives them ([`convert`]).
//!
//! The loop owns the streaming: it reads the world and runs the selection on it, lets the writer plan from that world
//! (geometry not loaded yet), then loads each part the writer asks for through the reader, runs the passes on it,
//! checks it against the writer's target and hands it to the writer read-only, one part in memory at a time. It does
//! no I/O of its own: reading and writing are the reader's and the writer's, and what they report comes back in the
//! [`ConvertReport`] for the caller to show: every part's pass reports whole ([`PartReport`]), and the notices the
//! reader and writer give ([`Notice`]). A conversion that fails keeps what it had reported ([`ConvertError`]).
use std::collections::{BTreeMap, BTreeSet};

use crate::pass::{Mode, PassManager, PassOptions, Pipeline, RunReport, Target};
use crate::{Entity, World};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Labelled lines for the log: what a reader read or loaded, what a writer wrote.
pub type Report = Vec<(String, String)>;

/// A source of worlds: the world to plan from, then the parts a writer asks for, loaded.
pub trait Reader {
    /// The world to plan from: placements, prototypes and their state; geometry and textures not loaded yet.
    fn read(&mut self) -> Result<(World, Report)>;
    /// The subset of `world` that `roots` reach, with everything a writer needs loaded (meshes, textures); entity ids
    /// stay `world`'s, and what loading spawns is reached through the roots.
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)>;
    /// The passes the read world goes through before a writer plans (which state and placements to keep), unless the
    /// caller gives others.
    fn default_select(&self) -> String;
    /// The passes a loaded world goes through, unless the caller gives others.
    fn default_passes(&self) -> String;
    /// The component namespaces it may put in a world (`core`, its own dialect): a reader reads, it lowers nothing
    /// (no `pbr.*` unless its source format is PBR). [`check_emits`] holds it to that after reading and loading.
    fn emits(&self) -> &'static [&'static str];
    /// A prototype's box (core frame) before its geometry is loaded, where the reader knows it.
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]>;
    /// What it noticed since it was last asked (source it can't read, what it tolerated), taken after reading and
    /// after each load. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
}

/// An error naming the first component of `w` outside the reader's namespaces (`when`: after what).
pub fn check_emits(w: &World, emits: &[&str], when: &str) -> Result<()> {
    if let Some(name) = w
        .component_names()
        .into_iter()
        .find(|n| !emits.contains(&n.split('.').next().unwrap_or("")))
    {
        return Err(format!("{when}: the reader put {name} in the world (it emits {emits:?})").into());
    }
    Ok(())
}

/// A destination. It plans from the selected world, then asks for its parts one at a time: the loop loads each (the
/// closure of its roots, through the passes, with what the passes placed) and hands it back to [`Writer::write`]
/// before asking for the next.
pub trait Writer {
    /// What it accepts: every loaded part is checked against it ([`PassManager::convert`], [`Mode::Full`]) before
    /// it is written.
    fn target(&self) -> Target;
    /// Plan from the selected world, its geometry not loaded yet: `boxes` gives a prototype's box (core frame)
    /// where the reader knows it.
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()>;
    /// The roots of the next part to load, `None` when every part is written.
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>>;
    /// Write the part last asked for, loaded: the closure of its roots and of every entity a pass placed (content
    /// nothing references, such as a marker; [`crate::pass::PassLog::place`]). Other entities the passes made are in
    /// it only where something in it references them.
    fn write(&mut self, part: &World) -> Result<()>;
    /// What it noticed since it was last asked (what it omitted because it doesn't support it), taken after each
    /// part and before [`Writer::finish`]. None by default.
    fn notices(&mut self) -> Vec<Notice> {
        Vec::new()
    }
    fn finish(self: Box<Self>) -> Result<Report>;
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
    /// The roots the writer asked for. Entity ids are the planning world's; a pass's own entities are the part's.
    pub roots: BTreeSet<Entity>,
    /// The reader's report on loading it.
    pub load: Report,
    /// The passes the caller gave.
    pub explicit: RunReport,
    /// The check against the writer's target, with what it lowered to pass it.
    pub automatic: RunReport,
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
    /// Every approximation, as `<pass>: <what>`, over the selection and every part (explicit passes and the target
    /// check), from the reports kept.
    pub fn approximations(&self) -> BTreeMap<String, u64> {
        let mut out = BTreeMap::new();
        let runs = std::iter::once(&self.select)
            .chain(self.part_reports.iter().flat_map(|p| [&p.explicit, &p.automatic]));
        for run in runs {
            for step in &run.steps {
                for (k, v) in &step.log.approximations {
                    *out.entry(format!("{}: {k}", step.pass)).or_default() += v;
                }
            }
        }
        out
    }

    /// Every notice: the reader's and the writer's, then the passes' approximations and what the parts left out, one
    /// notice per stage and reason over all parts, with the ordinals of the parts as examples.
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
        }
        for p in &self.part_reports {
            for step in p.explicit.steps.iter().chain(&p.automatic.steps) {
                for (k, v) in &step.log.approximations {
                    add(&step.pass, NoticeKind::Approximation, k, *v, Some(p.ordinal));
                }
            }
            for (k, v) in &p.kept {
                if k.starts_with("made entities nothing keeps") {
                    add("convert", NoticeKind::Selection, k, *v, Some(p.ordinal));
                }
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

/// A conversion that failed: the error, where, and everything reported before it (the parts written, the notices).
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

/// Reads with `reader`, runs `select` on the read world, lets `writer` plan, then loads, passes (`passes`), checks
/// and writes each part it asks for (see the module documentation). On failure the error is a [`ConvertError`]
/// holding what was reported until then.
pub fn convert(
    reader: &mut dyn Reader,
    writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let mut during = String::new();
    match run(reader, writer, manager, select, passes, &mut report, &mut during) {
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

fn run(
    reader: &mut dyn Reader,
    mut writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
    report: &mut ConvertReport,
    during: &mut String,
) -> Result<()> {
    "read".clone_into(during);
    let (mut world, read) = reader.read()?;
    report.read = read;
    report.reported.extend(reader.notices());
    check_emits(&world, reader.emits(), "read")?;
    "select".clone_into(during);
    manager.run(&mut world, select, &mut report.select)?;
    let target = writer.target();
    "plan".clone_into(during);
    writer.plan(&world, &|p| reader.prototype_box(&world, p))?;
    while let Some(roots) = writer.next_part()? {
        let ordinal = report.part_reports.len();
        *during = format!("part {ordinal}: load");
        let (mut part, loaded) = reader.load(&world, &roots)?;
        report.loads.extend(loaded.iter().cloned());
        report.reported.extend(reader.notices());
        let mut size = PartSize {
            roots: roots.len(),
            loaded: sizes(&part),
            ..PartSize::default()
        };
        check_emits(&part, reader.emits(), "load")?;
        *during = format!("part {ordinal}: passes");
        let mut part_report = PartReport {
            ordinal,
            roots: roots.clone(),
            load: loaded,
            ..PartReport::default()
        };
        let loaded: BTreeSet<Entity> = part.entities().collect();
        manager.run(&mut part, passes, &mut part_report.explicit)?;
        // the part is the closure of the roots and of what the passes placed (content of its own nothing need
        // reference, as a marker; PassLog::place). What the passes left unreferenced of what was loaded (materials of
        // dropped geometry, of a discarded level of detail) is not part of it, nor is what they made that nothing kept
        // reaches: dropped, and named in the counts by its components, as are the placed entities
        let placed: BTreeSet<Entity> = part_report
            .explicit
            .steps
            .iter()
            .flat_map(|s| s.log.placed.iter().copied())
            .filter(|&e| part.is_alive(e))
            .collect();
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
        part_report.automatic = manager.convert(&mut part, &target, Mode::Full, &PassOptions::new())?;
        tally(&mut report.counts, &part_report.explicit);
        tally(&mut report.counts, &part_report.automatic);
        size.written = sizes(&part);
        report.part_sizes.push(size);
        report.part_reports.push(part_report);
        *during = format!("part {ordinal}: write");
        writer.write(&part)?;
        report.reported.extend(writer.notices());
        report.parts += 1;
    }
    "finish".clone_into(during);
    report.reported.extend(writer.notices());
    report.written = writer.finish()?;
    Ok(())
}
