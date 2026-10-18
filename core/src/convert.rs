//! Convert: one reader, the passes, one writer, and the loop that drives them ([`convert`]).
//!
//! The loop owns the streaming: it reads the world and runs the selection on it, lets the writer plan from that world
//! (geometry not loaded yet), then loads each part the writer asks for through the reader, runs the passes on it,
//! checks it against the writer's target and hands it to the writer read-only, one part in memory at a time. It does
//! no I/O of its own: reading and writing are the reader's and the writer's, and what they report comes back in the
//! [`ConvertReport`] for the caller to show.
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
/// closure of its roots, through the passes, with every entity the passes made) and hands it back to
/// [`Writer::write`] before asking for the next.
pub trait Writer {
    /// What it accepts: every loaded part is checked against it ([`PassManager::convert`], [`Mode::Full`]) before
    /// it is written.
    fn target(&self) -> Target;
    /// Plan from the selected world, its geometry not loaded yet: `boxes` gives a prototype's box (core frame)
    /// where the reader knows it.
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()>;
    /// The roots of the next part to load, `None` when every part is written.
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>>;
    /// Write the part last asked for, loaded: the closure of its roots and of every entity the passes made (a pass
    /// may make part content nothing references, such as a marker it places).
    fn write(&mut self, part: &World) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<Report>;
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
    /// Every pass's counts over all parts, as `<pass>: <what>`.
    pub counts: BTreeMap<String, u64>,
    /// The writer's report.
    pub written: Report,
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
fn tally(counts: &mut BTreeMap<String, u64>, report: RunReport) {
    for step in report.steps {
        for (k, v) in step.log.counts {
            *counts.entry(format!("{}: {k}", step.pass)).or_default() += v;
        }
    }
}

/// Reads with `reader`, runs `select` on the read world, lets `writer` plan, then loads, passes (`passes`), checks
/// and writes each part it asks for (see the module documentation).
pub fn convert(
    reader: &mut dyn Reader,
    mut writer: Box<dyn Writer>,
    manager: &PassManager,
    select: &Pipeline,
    passes: &Pipeline,
) -> Result<ConvertReport> {
    let mut report = ConvertReport::default();
    let (mut world, read) = reader.read()?;
    report.read = read;
    check_emits(&world, reader.emits(), "read")?;
    manager.run(&mut world, select, &mut report.select)?;
    let target = writer.target();
    writer.plan(&world, &|p| reader.prototype_box(&world, p))?;
    while let Some(roots) = writer.next_part()? {
        let (mut part, loaded) = reader.load(&world, &roots)?;
        report.loads.extend(loaded);
        let mut size = PartSize {
            roots: roots.len(),
            loaded: sizes(&part),
            ..PartSize::default()
        };
        check_emits(&part, reader.emits(), "load")?;
        let mut run = RunReport::default();
        let loaded: BTreeSet<Entity> = part.entities().collect();
        manager.run(&mut part, passes, &mut run)?;
        // what the passes left unreferenced of what was loaded (materials of dropped geometry, of a discarded level
        // of detail) is not part of the part; what they made is, referenced or not (a marker a pass places stands on
        // its own): kept with the roots' closure, and named in the counts by its components
        let made: Vec<Entity> = part.entities().filter(|e| !loaded.contains(e)).collect();
        let names: Vec<String> = part.component_names().into_iter().map(str::to_owned).collect();
        for &e in &made {
            let kinds: Vec<&str> = names
                .iter()
                .filter(|n| part.entity_has_named(e, n))
                .map(String::as_str)
                .collect();
            *report
                .counts
                .entry(format!(
                    "convert: entities the passes made ({})",
                    kinds.join(" + ")
                ))
                .or_default() += 1;
        }
        let keep = part.closure(roots.iter().copied().chain(made));
        part.retain(&keep);
        let checked = manager.convert(&mut part, &target, Mode::Full, &PassOptions::new())?;
        tally(&mut report.counts, run);
        tally(&mut report.counts, checked);
        size.written = sizes(&part);
        report.part_sizes.push(size);
        writer.write(&part)?;
        report.parts += 1;
    }
    report.written = writer.finish()?;
    Ok(report)
}
