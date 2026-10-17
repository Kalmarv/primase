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
/// closure of its roots, through the passes) and hands it back to [`Writer::write`] before asking for the next.
pub trait Writer {
    /// What it accepts: every loaded part is checked against it ([`PassManager::convert`], [`Mode::Full`]) before
    /// it is written.
    fn target(&self) -> Target;
    /// Plan from the selected world, its geometry not loaded yet: `boxes` gives a prototype's box (core frame)
    /// where the reader knows it.
    fn plan(&mut self, world: &World, boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()>;
    /// The roots of the next part to load, `None` when every part is written.
    fn next_part(&mut self) -> Result<Option<BTreeSet<Entity>>>;
    /// Write the part last asked for, loaded: exactly the closure of its roots.
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
    /// Every pass's counts over all parts, as `<pass>: <what>`.
    pub counts: BTreeMap<String, u64>,
    /// The writer's report.
    pub written: Report,
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
        check_emits(&part, reader.emits(), "load")?;
        let mut run = RunReport::default();
        manager.run(&mut part, passes, &mut run)?;
        // what the passes left unreferenced (materials of dropped geometry, of a discarded level of detail) is not
        // part of the part
        let keep = part.closure(roots.iter().copied());
        part.retain(&keep);
        let checked = manager.convert(&mut part, &target, Mode::Full, &PassOptions::new())?;
        tally(&mut report.counts, run);
        tally(&mut report.counts, checked);
        writer.write(&part)?;
        report.parts += 1;
    }
    report.written = writer.finish()?;
    Ok(report)
}
