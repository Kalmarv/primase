//! `convert`: one reader, the passes, one writer. Readers and writers are registered once, here.
//!
//! Convert owns the streaming: it reads the world and runs the selection on it, lets the writer plan from that world
//! (geometry not loaded yet), then loads each part the writer asks for through the reader, runs the passes on it,
//! checks it against the writer's target and hands it to the writer read-only, one part in memory at a time.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use primase_core::pass::{Mode, PassManager, PassOptions, PassRegistry, Pipeline, RunReport, Target};
use primase_core::{Entity, World};

pub type Error = Box<dyn std::error::Error>;
pub type Result<T> = std::result::Result<T, Error>;

pub const USAGE: &str = "usage: primase convert --from <reader> [reader options] [--select \"<pipeline>\"] [--passes \"<pipeline>\"] --to <writer> \
<out> [writer options]
readers:
  gta   --gta <install> --keys <key folder> --region <legion|vespucci|x0,y0,x1,y1> [--map online|story] [--hour H]
        [--min-size M] [--max-priority N] [--interiors none|all]
writers:
  web   [--ktx2] [--budget-mib N | --no-budget] [--plain]";

/// A source of worlds: the world to plan from, then the parts a writer asks for, loaded.
pub trait Reader {
    /// The world to plan from: placements, prototypes and their state; geometry and textures not loaded yet.
    fn read(&mut self) -> Result<World>;
    /// The subset of `world` that `roots` reach, with everything a writer needs loaded (meshes, textures); entity ids
    /// stay `world`'s, and what loading spawns is reached through the roots.
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<World>;
    /// The passes the read world goes through before a writer plans (which state and placements to keep) unless
    /// `--select` says otherwise.
    fn default_select(&self) -> String;
    /// The passes a loaded world goes through unless `--passes` says otherwise.
    fn default_passes(&self) -> &'static str;
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

/// What a writer did, for the log.
pub type WriteReport = Vec<(String, String)>;

/// A destination. It plans from the selected world, then asks for its parts one at a time: convert loads each (the
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
    fn finish(self: Box<Self>) -> Result<WriteReport>;
}

/// The pass pipeline every loaded world goes through, and the manager that runs it.
pub struct Passes {
    pub manager: PassManager,
    pub pipeline: Pipeline,
}

/// Every pass by name: core's and each world crate's.
pub fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    r.register("select-lod", primase_core::select_lod);
    r.register("drop-optional", primase_core::drop_optional);
    r.register("resolve-state", primase_core::state::resolve_state);
    primase_gta::register(&mut r);
    r
}

type ReaderFactory = fn(&[String]) -> Result<Box<dyn Reader>>;
type WriterFactory = fn(&Path, &[String]) -> Result<Box<dyn Writer>>;

/// The readers, by `--from` name.
pub const READERS: &[(&str, ReaderFactory)] = &[("gta", crate::gta::reader)];
/// The writers, by `--to` name.
pub const WRITERS: &[(&str, WriterFactory)] = &[("web", crate::web::writer)];

/// The entry of `table` named `name`.
fn find<T: Copy>(table: &[(&str, T)], name: &str, kind: &str) -> Result<T> {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
        .ok_or_else(|| format!("no {kind} {name}; {USAGE}").into())
}

/// Adds each step's counts to `counts` under `<pass>: <what>`.
fn tally(counts: &mut BTreeMap<String, u64>, report: RunReport) {
    for step in report.steps {
        for (k, v) in step.log.counts {
            *counts.entry(format!("{}: {k}", step.pass)).or_default() += v;
        }
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let at = |flag: &str| args.iter().position(|a| a == flag);
    let (Some(from), Some(to)) = (at("--from"), at("--to")) else {
        return Err(USAGE.into());
    };
    if from != 0 || to < from + 2 || to + 2 > args.len() {
        return Err(USAGE.into());
    }
    let reader_name = &args[from + 1];
    let mut reader_args: Vec<String> = args[from + 2..to].to_vec();
    let mut take = |flag: &str| -> Result<Option<String>> {
        let Some(p) = reader_args.iter().position(|a| a == flag) else {
            return Ok(None);
        };
        let text = reader_args.get(p + 1).ok_or(USAGE)?.clone();
        reader_args.drain(p..p + 2);
        Ok(Some(text))
    };
    let passes_text = take("--passes")?;
    let select_text = take("--select")?;
    let writer_name = &args[to + 1];
    let out = Path::new(args.get(to + 2).ok_or(USAGE)?);
    let mut reader = find(READERS, reader_name, "reader")?(&reader_args)?;
    let mut writer = find(WRITERS, writer_name, "writer")?(out, &args[to + 3..])?;
    let registry = registry();
    let select = registry.parse(&select_text.unwrap_or_else(|| reader.default_select()))?;
    let pipeline = registry.parse(&passes_text.unwrap_or_else(|| reader.default_passes().to_string()))?;
    println!("select: {select}");
    println!("passes: {pipeline}");
    let passes = Passes {
        manager: PassManager::new(registry),
        pipeline,
    };
    let mut world = reader.read()?;
    check_emits(&world, reader.emits(), "read")?;
    let mut report = RunReport::default();
    passes.manager.run(&mut world, &select, &mut report)?;
    for step in &report.steps {
        println!("select: {}: {:?}", step.pass, step.log.counts);
    }
    let target = writer.target();
    writer.plan(&world, &|p| reader.prototype_box(&world, p))?;
    let (mut parts, mut counts) = (0usize, BTreeMap::<String, u64>::new());
    while let Some(roots) = writer.next_part()? {
        let mut part = reader.load(&world, &roots)?;
        check_emits(&part, reader.emits(), "load")?;
        let mut run = RunReport::default();
        passes.manager.run(&mut part, &passes.pipeline, &mut run)?;
        // what the passes left unreferenced (materials of dropped geometry, of a discarded level of detail) is not
        // part of the part
        let keep = part.closure(roots.iter().copied());
        part.retain(&keep);
        let checked = passes
            .manager
            .convert(&mut part, &target, Mode::Full, &PassOptions::new())?;
        tally(&mut counts, run);
        tally(&mut counts, checked);
        writer.write(&part)?;
        parts += 1;
    }
    println!("parts loaded: {parts}");
    println!("passes over all parts: {counts:?}");
    for (k, v) in writer.finish()? {
        println!("{k}: {v}");
    }
    Ok(())
}
