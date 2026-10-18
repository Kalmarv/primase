//! Printing the world around passes (DESIGN.md §4 `print-world-after`): [`WorldPrinter`] is an [`Instrument`] that
//! makes one text per pass: what the world holds after it (entities, components and resources by name), what the
//! pass spawned, despawned and changed, the pass's own counts, and optionally the components of chosen entities.
//!
//! Core does no I/O: each print goes to the [`PrintSink`] the caller supplies (a command writes files, a test keeps
//! them), with the run's label ([`WorldPrinter::set_label`]) and the print's name: `00-input` (the world before the
//! first pass), `NN-<pass>` after pass NN (from 01), and `NN-<pass>.failed` instead when the pass failed (the world
//! as the failure left it; the print before it is the pass's input). Labels and names are safe as file names. The
//! same world, pipeline and options give the same text.
//!
//! Changes are found by comparing a digest of each component's `Debug` text before and after the pass (64-bit
//! FNV-1a): JSON views summarise bulk data (a mesh shows its counts, not its vertices), so a pass that only moves
//! vertices would not show in them. The digest is a debugging aid and nothing more: `Debug` text is not a format and
//! may change between builds, so the digest is never written out, gated on or used as a cache key.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::components::{Name, Source};
use crate::pass::{Error, Instrument, Step, StepReport};
use crate::world::{Entity, World};

/// What a [`WorldPrinter`] writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintOptions {
    /// Print after these passes only (by pass name); empty: the input and after every pass.
    pub only: BTreeSet<String>,
    /// Entities listed under `changed`, `spawned` and `despawned` before `... N more`.
    pub limit: usize,
    /// The entities whose components are written out; `None`: none.
    pub dump: Option<EntityFilter>,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            only: BTreeSet::new(),
            limit: 200,
            dump: None,
        }
    }
}

/// Which entities a print writes out in full. An entity matches when it meets every kind of criterion given, and
/// within a kind any one value; no criteria at all match every entity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityFilter {
    pub entities: BTreeSet<Entity>,
    /// Component names (typed or opaque), any of which the entity has.
    pub has: BTreeSet<String>,
    /// Text found in the entity's `core.source`.
    pub source: Vec<String>,
    /// Text found in the entity's `core.name`.
    pub name: Vec<String>,
    /// Only entities the pass spawned or changed (none in the input print).
    pub changed: bool,
}

impl EntityFilter {
    /// `all`, or criteria separated by commas: `entity=12` (also `e12`), `has=<component name>`,
    /// `source=<text>`, `name=<text>`, `changed`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut f = EntityFilter::default();
        let text = text.trim();
        if text == "all" {
            return Ok(f);
        }
        if text.is_empty() {
            return Err("empty entity filter (`all` prints every entity)".to_owned());
        }
        for item in text.split(',').map(str::trim) {
            if item == "changed" {
                f.changed = true;
                continue;
            }
            if let Some(index) = item.strip_prefix('e').and_then(|n| n.parse::<u32>().ok()) {
                f.entities.insert(Entity::from_index(index));
                continue;
            }
            let Some((k, v)) = item.split_once('=') else {
                return Err(format!(
                    "entity filter item {item:?} is not key=value or `changed`"
                ));
            };
            let v = v.trim();
            if v.is_empty() {
                return Err(format!("entity filter item {item:?} has no value"));
            }
            match k.trim() {
                "entity" => {
                    let n = v.strip_prefix('e').unwrap_or(v);
                    let index = n
                        .parse::<u32>()
                        .map_err(|_| format!("entity filter: {v:?} is not an entity"))?;
                    f.entities.insert(Entity::from_index(index));
                }
                "has" => {
                    f.has.insert(v.to_owned());
                }
                "source" => f.source.push(v.to_owned()),
                "name" => f.name.push(v.to_owned()),
                k => return Err(format!("entity filter: unknown key {k:?}")),
            }
        }
        Ok(f)
    }

    fn matches(&self, w: &World, e: Entity, changed: &BTreeSet<Entity>) -> bool {
        let contains = |texts: &[String], value: Option<&String>| {
            texts.is_empty() || value.is_some_and(|v| texts.iter().any(|t| v.contains(t.as_str())))
        };
        (self.entities.is_empty() || self.entities.contains(&e))
            && (self.has.is_empty() || self.has.iter().any(|n| w.entity_has_named(e, n)))
            && contains(&self.source, w.get::<Source>(e).map(|s| &s.0))
            && contains(&self.name, w.get::<Name>(e).map(|n| &n.0))
            && (!self.changed || changed.contains(&e))
    }
}

/// Where a [`WorldPrinter`]'s prints go: `label` is the run's label when one is set, `name` the print's name, unique
/// within a run (see the module docs). An error stops the run as [`Error::Instrument`]. Closures
/// `FnMut(Option<&str>, &str, &str) -> Result<(), String>` are sinks.
pub trait PrintSink {
    fn print(&mut self, label: Option<&str>, name: &str, text: &str) -> Result<(), String>;
}

impl<F: FnMut(Option<&str>, &str, &str) -> Result<(), String>> PrintSink for F {
    fn print(&mut self, label: Option<&str>, name: &str, text: &str) -> Result<(), String> {
        self(label, name, text)
    }
}

/// Prints the world around every pass of the runs it instruments, into `sink` (see the module docs).
pub struct WorldPrinter<S: PrintSink> {
    sink: S,
    label: Option<String>,
    options: PrintOptions,
    /// The world before the current pass, when a print needs it.
    before: Option<Snapshot>,
}

impl<S: PrintSink> fmt::Debug for WorldPrinter<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldPrinter")
            .field("label", &self.label)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl<S: PrintSink> WorldPrinter<S> {
    pub fn new(sink: S, options: PrintOptions) -> Self {
        WorldPrinter {
            sink,
            label: None,
            options,
            before: None,
        }
    }

    /// Label the next runs' prints (a streamed conversion's cell): the sink gets it with each print. ASCII letters,
    /// digits, `_`, `-` and `.`, not starting with `.`, so it is safe as a folder name.
    pub fn set_label(&mut self, label: &str) -> Result<(), String> {
        let ok = !label.is_empty()
            && !label.starts_with('.')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
        if !ok {
            return Err(format!("print label {label:?}: use letters, digits, _, - and ."));
        }
        self.label = Some(label.to_owned());
        self.before = None;
        Ok(())
    }

    fn selected(&self, pass: &str) -> bool {
        self.options.only.is_empty() || self.options.only.contains(pass)
    }

    fn write(&mut self, name: &str, text: &str) -> Result<(), String> {
        self.sink.print(self.label.as_deref(), name, text)
    }

    /// The text of one print: `head`, the world as `after` holds it, the change from `before`, `report`'s counts and
    /// the dump.
    fn render(
        &self,
        head: &str,
        w: &World,
        before: Option<&Snapshot>,
        after: &Snapshot,
        report: Option<&StepReport>,
    ) -> String {
        let mut out = String::new();
        // writing into a String cannot fail
        let _ = render(&mut out, head, w, before, after, report, &self.options);
        out
    }
}

impl<S: PrintSink> Instrument for WorldPrinter<S> {
    fn before_pass(&mut self, step: &Step<'_>, w: &World) -> Result<(), String> {
        if step.index == 0 {
            self.before = None;
        }
        if !self.selected(step.name) || self.before.is_some() {
            return Ok(());
        }
        let snap = Snapshot::take(w);
        if step.index == 0 && self.options.only.is_empty() {
            let text = self.render("input", w, None, &snap, None);
            self.write("00-input", &text)?;
        }
        self.before = Some(snap);
        Ok(())
    }

    fn after_pass(&mut self, step: &Step<'_>, w: &World, report: &StepReport) -> Result<(), String> {
        if !self.selected(step.name) {
            self.before = None;
            return Ok(());
        }
        let after = Snapshot::take(w);
        let head = format!("after step {}: {}", step.index + 1, step.text);
        let text = self.render(&head, w, self.before.as_ref(), &after, Some(report));
        self.write(&format!("{:02}-{}", step.index + 1, file_name(step.name)), &text)?;
        self.before = Some(after);
        Ok(())
    }

    fn pass_failed(&mut self, step: &Step<'_>, w: &World, error: &Error) -> Result<(), String> {
        let before = self.before.take();
        let after = Snapshot::take(w);
        let head = format!("step {} failed: {}\nerror: {error}", step.index + 1, step.text);
        let text = self.render(&head, w, before.as_ref(), &after, None);
        self.write(
            &format!("{:02}-{}.failed", step.index + 1, file_name(step.name)),
            &text,
        )
    }
}

/// What a print compares: who is alive, and a digest of every component and resource.
#[derive(Debug, Default)]
struct Snapshot {
    alive: BTreeSet<Entity>,
    /// Component name -> (schema version, entity -> digest).
    components: BTreeMap<String, (u32, BTreeMap<Entity, u64>)>,
    /// Resource name -> (schema version, digest).
    resources: BTreeMap<String, (u32, u64)>,
}

impl Snapshot {
    fn take(w: &World) -> Self {
        let mut s = Snapshot {
            alive: w.entities().collect(),
            ..Snapshot::default()
        };
        w.visit_components_debug(&mut |name, version, e, value| {
            let digest = digest(value);
            match s.components.get_mut(name) {
                Some((_, col)) => {
                    col.insert(e, digest);
                }
                None => {
                    s.components
                        .insert(name.to_owned(), (version, BTreeMap::from([(e, digest)])));
                }
            }
        });
        w.visit_resources_debug(&mut |name, version, value| {
            s.resources.insert(name.to_owned(), (version, digest(value)));
        });
        s
    }
}

/// 64-bit FNV-1a over text written into it.
struct Fnv(u64);

impl fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        Ok(())
    }
}

/// The digest of a value's `Debug` text, streamed (no allocation). Debugging only, see the module docs.
fn digest(value: &dyn fmt::Debug) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    // a Debug impl that fails leaves what it wrote so far: still deterministic
    let _ = write!(h, "{value:?}");
    h.0
}

/// How an entity's components changed in a pass: `+` gained, `-` lost, `~` changed, each with the name.
type Changes<'a> = BTreeMap<Entity, Vec<(char, &'a str)>>;

fn render(
    out: &mut String,
    head: &str,
    w: &World,
    before: Option<&Snapshot>,
    after: &Snapshot,
    report: Option<&StepReport>,
    options: &PrintOptions,
) -> fmt::Result {
    writeln!(out, "{head}")?;
    let empty = Snapshot::default();
    let b = before.unwrap_or(&empty);
    let spawned: Vec<Entity> = after.alive.difference(&b.alive).copied().collect();
    let despawned: Vec<Entity> = b.alive.difference(&after.alive).copied().collect();
    write!(out, "entities {}", after.alive.len())?;
    if before.is_some() {
        write!(out, " spawned {} despawned {}", spawned.len(), despawned.len())?;
    }
    writeln!(out)?;

    // components by name: count, and gained (spawned entities included) / lost / changed
    let mut changes: Changes<'_> = BTreeMap::new();
    let names: BTreeSet<&str> = after
        .components
        .keys()
        .chain(b.components.keys())
        .map(String::as_str)
        .collect();
    let none = (0, BTreeMap::new());
    for name in names {
        let (va, now) = after.components.get(name).unwrap_or(&none);
        let (vb, was) = b.components.get(name).unwrap_or(&none);
        let version = if after.components.contains_key(name) {
            va
        } else {
            vb
        };
        write!(out, "component {name}@{version} {}", now.len())?;
        if before.is_some() {
            let (mut gained, mut lost, mut changed) = (0, 0, 0);
            for (e, d) in now {
                match was.get(e) {
                    None => {
                        gained += 1;
                        if b.alive.contains(e) {
                            changes.entry(*e).or_default().push(('+', name));
                        }
                    }
                    Some(old) if old != d => {
                        changed += 1;
                        changes.entry(*e).or_default().push(('~', name));
                    }
                    Some(_) => {}
                }
            }
            for e in was.keys().filter(|e| !now.contains_key(e)) {
                lost += 1;
                if after.alive.contains(e) {
                    changes.entry(*e).or_default().push(('-', name));
                }
            }
            write!(out, " gained {gained} lost {lost} changed {changed}")?;
        }
        writeln!(out)?;
    }

    let res: BTreeSet<&str> = after
        .resources
        .keys()
        .chain(b.resources.keys())
        .map(String::as_str)
        .collect();
    for name in res {
        match (after.resources.get(name), b.resources.get(name)) {
            (Some((v, _)), _) if before.is_none() => writeln!(out, "resource {name}@{v}")?,
            (Some((v, d)), Some((_, old))) => writeln!(
                out,
                "resource {name}@{v}{}",
                if d == old { "" } else { " changed" }
            )?,
            (Some((v, _)), None) => writeln!(out, "resource {name}@{v} added")?,
            (None, Some((v, _))) => writeln!(out, "resource {name}@{v} removed")?,
            (None, None) => {}
        }
    }

    if let Some(r) = report {
        for (k, n) in &r.log.counts {
            writeln!(out, "count {k} {n}")?;
        }
        for (k, n) in &r.log.approximations {
            writeln!(out, "approximated {k} {n}")?;
        }
    }

    if before.is_some() {
        writeln!(out, "changed {}", changes.len())?;
        list(out, options.limit, changes.iter(), |out, (e, cs)| {
            write!(out, "  {e:?}")?;
            for (c, n) in cs {
                write!(out, " {c}{n}")?;
            }
            Ok(())
        })?;
        for (what, ids, snap) in [("spawned", &spawned, after), ("despawned", &despawned, b)] {
            writeln!(out, "{what} {}", ids.len())?;
            list(out, options.limit, ids.iter(), |out, e| {
                write!(out, "  {e:?}")?;
                for (name, (_, col)) in &snap.components {
                    if col.contains_key(e) {
                        write!(out, " {name}")?;
                    }
                }
                Ok(())
            })?;
        }
    }

    if let Some(filter) = &options.dump {
        let changed: BTreeSet<Entity> = changes.keys().chain(&spawned).copied().collect();
        let picked: Vec<Entity> = w.entities().filter(|&e| filter.matches(w, e, &changed)).collect();
        writeln!(out, "dump {}", picked.len())?;
        for e in picked {
            writeln!(out, "{e:?}")?;
            match w.components_json(e) {
                Ok(cs) => {
                    for c in cs {
                        match c.value {
                            Some(v) => writeln!(out, "  {}@{} {v}", c.name, c.version)?,
                            None => writeln!(out, "  {}@{} (no view)", c.name, c.version)?,
                        }
                    }
                }
                Err(err) => writeln!(out, "  (no faithful view: {err})")?,
            }
        }
    }
    Ok(())
}

/// Up to `limit` lines from `items`, then `  ... N more`.
fn list<T>(
    out: &mut String,
    limit: usize,
    items: impl ExactSizeIterator<Item = T>,
    mut line: impl FnMut(&mut String, T) -> fmt::Result,
) -> fmt::Result {
    let total = items.len();
    for item in items.take(limit) {
        line(out, item)?;
        writeln!(out)?;
    }
    if total > limit {
        writeln!(out, "  ... {} more", total - limit)?;
    }
    Ok(())
}

/// A pass name as a file name: characters other than ASCII letters, digits, `_`, `-` and `.` become `_`.
fn file_name(pass: &str) -> String {
    pass.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test I/O: unit tests write and read a sink file"
)]
mod tests {
    use super::*;
    use crate::components::{Mesh, Primitive, Semantic, Stream, Topology};
    use crate::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
    use crate::world::{Opaque, OpaqueData};
    use std::sync::{Arc, Mutex};

    fn mesh(x: f32) -> Mesh {
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[x, 0., 0.], [0., 1., 0.], [0., 0., 1.]]),
            )]),
            indices: None,
            material: None,
        }])
    }

    /// e0 a named mesh from `m11:wall`, e1 a named light-ish opaque thing, e2 a bare mesh.
    fn world() -> World {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Name("wall".into()));
        w.insert(a, Source("m11:wall".into()));
        w.insert(a, mesh(0.));
        let b = w.spawn();
        w.insert(b, Name("lamp".into()));
        w.insert_opaque(
            b,
            "game.lamp",
            Opaque {
                version: 2,
                data: OpaqueData::Json(serde_json::json!({"on": true})),
            },
        );
        let c = w.spawn();
        w.insert(c, mesh(5.));
        w
    }

    /// Moves the first vertex of every mesh (the JSON view stays the same), despawns `game.lamp` holders, spawns one
    /// named entity and renames e0.
    struct Edit;
    impl Pass for Edit {
        fn name(&self) -> &'static str {
            "edit"
        }
        fn provides(&self) -> &[&'static str] {
            &["core.name"]
        }
        fn removes(&self) -> &[&'static str] {
            &["game.lamp", "core.name"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["core.mesh"]
        }
        fn despawns_entities(&self) -> bool {
            true
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
            let meshes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
            for e in meshes {
                if let Some(Stream::F32x3(v)) = w
                    .get_mut::<Mesh>(e)
                    .and_then(|m| m.primitives_mut()[0].streams.get_mut(&Semantic::Position))
                {
                    v[0][0] += 1.;
                    log.count("moved", 1);
                }
            }
            for e in w.entities_named("game.lamp") {
                w.despawn(e);
            }
            let n = w.spawn();
            w.insert(n, Name("new".into()));
            let first = w.entities().next().expect("e0");
            w.insert(first, Name("wall2".into()));
            Ok(())
        }
    }

    /// Fails after removing e2's mesh.
    struct Break;
    impl Pass for Break {
        fn name(&self) -> &'static str {
            "break"
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> Result<(), String> {
            w.remove::<Mesh>(Entity::from_index(2));
            Err("broke on purpose".into())
        }
    }

    fn pipeline(passes: Vec<Box<dyn Pass>>) -> Pipeline {
        Pipeline { passes }
    }

    /// Prints by (label, name), as a sink keeps them.
    type Prints = Arc<Mutex<BTreeMap<(Option<String>, String), String>>>;

    fn keeper() -> (Prints, impl PrintSink) {
        let prints = Prints::default();
        let kept = prints.clone();
        let sink = move |label: Option<&str>, name: &str, text: &str| {
            let key = (label.map(str::to_owned), name.to_owned());
            match kept.lock().unwrap().insert(key, text.to_owned()) {
                None => Ok(()),
                Some(_) => Err(format!("{name} printed twice")),
            }
        };
        (prints, sink)
    }

    /// Run `passes` on [`world`], printing; the prints of the unlabelled run by name.
    fn run(
        options: PrintOptions,
        passes: Vec<Box<dyn Pass>>,
    ) -> (crate::pass::Result<()>, BTreeMap<String, String>) {
        let (prints, sink) = keeper();
        let mut printer = WorldPrinter::new(sink, options);
        let result = PassManager::new(PassRegistry::new()).run_instrumented(
            &mut world(),
            &pipeline(passes),
            &mut RunReport::default(),
            &mut printer,
        );
        let by_name = prints
            .lock()
            .unwrap()
            .iter()
            .map(|((_, n), t)| (n.clone(), t.clone()))
            .collect();
        (result, by_name)
    }

    #[test]
    fn prints_counts_changes_and_dumps() {
        let options = PrintOptions {
            dump: Some(EntityFilter::parse("source=m11,has=core.mesh").unwrap()),
            ..PrintOptions::default()
        };
        let (result, prints) = run(options, vec![Box::new(Edit)]);
        result.unwrap();
        assert_eq!(prints.keys().collect::<Vec<_>>(), ["00-input", "01-edit"]);
        let input = "input
entities 3
component core.mesh@1 2
component core.name@1 2
component core.source@1 1
component game.lamp@2 1
dump 1
e0
  core.mesh@1 {\"primitives\":[{\"topology\":\"Triangles\",\"vertices\":3,\"indices\":null,\"streams\":[\"Position\"],\"material\":null}]}
  core.name@1 \"wall\"
  core.source@1 \"m11:wall\"
";
        assert_eq!(prints["00-input"], input);
        // the meshes' JSON views did not change; their vertices did
        let after = "after step 1: edit
entities 3 spawned 1 despawned 1
component core.mesh@1 2 gained 0 lost 0 changed 2
component core.name@1 2 gained 1 lost 1 changed 1
component core.source@1 1 gained 0 lost 0 changed 0
component game.lamp@2 0 gained 0 lost 1 changed 0
count moved 2
changed 2
  e0 ~core.mesh ~core.name
  e2 ~core.mesh
spawned 1
  e3 core.name
despawned 1
  e1 core.name game.lamp
dump 1
e0
  core.mesh@1 {\"primitives\":[{\"topology\":\"Triangles\",\"vertices\":3,\"indices\":null,\"streams\":[\"Position\"],\"material\":null}]}
  core.name@1 \"wall2\"
  core.source@1 \"m11:wall\"
";
        assert_eq!(prints["01-edit"], after);
    }

    #[test]
    fn a_failing_pass_is_printed_as_the_failure_left_the_world() {
        let options = PrintOptions {
            limit: 1,
            ..PrintOptions::default()
        };
        let (result, prints) = run(options, vec![Box::new(Edit), Box::new(Break)]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "pass break failed: broke on purpose"
        );
        assert_eq!(
            prints.keys().collect::<Vec<_>>(),
            ["00-input", "01-edit", "02-break.failed"]
        );
        assert!(prints["01-edit"].contains("changed 2\n  e0 ~core.mesh ~core.name\n  ... 1 more\n"));
        assert_eq!(
            prints["02-break.failed"],
            "step 2 failed: break
error: pass break failed: broke on purpose
entities 3 spawned 0 despawned 0
component core.mesh@1 1 gained 0 lost 1 changed 0
component core.name@1 2 gained 0 lost 0 changed 0
component core.source@1 1 gained 0 lost 0 changed 0
changed 1
  e2 -core.mesh
spawned 0
despawned 0
"
        );
    }

    #[test]
    fn only_labels_and_sink_errors() {
        let m = PassManager::new(PassRegistry::new());
        let edit = || pipeline(vec![Box::new(Edit)]);
        // not selected: nothing printed
        let (prints, sink) = keeper();
        let options = PrintOptions {
            only: BTreeSet::from(["break".to_owned()]),
            ..PrintOptions::default()
        };
        let mut printer = WorldPrinter::new(sink, options);
        m.run_instrumented(&mut world(), &edit(), &mut RunReport::default(), &mut printer)
            .unwrap();
        assert!(prints.lock().unwrap().is_empty());

        // each run under its label; a sink's refusal stops the run, naming the step
        let (prints, sink) = keeper();
        let mut printer = WorldPrinter::new(sink, PrintOptions::default());
        printer.set_label("cell_0_-1").unwrap();
        m.run_instrumented(&mut world(), &edit(), &mut RunReport::default(), &mut printer)
            .unwrap();
        let err = m
            .run_instrumented(&mut world(), &edit(), &mut RunReport::default(), &mut printer)
            .unwrap_err();
        assert!(
            matches!(&err, crate::pass::Error::Instrument { step, message }
                if step == "edit" && message == "00-input printed twice"),
            "{err}"
        );
        printer.set_label("cell_0_0").unwrap();
        m.run_instrumented(&mut world(), &edit(), &mut RunReport::default(), &mut printer)
            .unwrap();
        let prints = prints.lock().unwrap();
        let of = |label: &str| -> Vec<(&String, &String)> {
            prints
                .iter()
                .filter(|((l, _), _)| l.as_deref() == Some(label))
                .map(|((_, n), t)| (n, t))
                .collect()
        };
        assert_eq!(of("cell_0_-1").len(), 2);
        assert_eq!(of("cell_0_-1"), of("cell_0_0"));
        for bad in ["", "..", "a/b", ".x", "a b"] {
            assert!(printer.set_label(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn filters_parse() {
        let f = EntityFilter::parse("e3, entity=7, has=core.light, source=m11, name=wall, changed").unwrap();
        assert_eq!(
            f.entities,
            BTreeSet::from([Entity::from_index(3), Entity::from_index(7)])
        );
        assert_eq!(f.has, BTreeSet::from(["core.light".to_owned()]));
        assert_eq!(
            (f.source, f.name, f.changed),
            (vec!["m11".into()], vec!["wall".into()], true)
        );
        assert_eq!(EntityFilter::parse("all").unwrap(), EntityFilter::default());
        for bad in ["", "e", "e3x", "entity=x", "has=", "colour=red"] {
            assert!(EntityFilter::parse(bad).is_err(), "{bad:?}");
        }
    }

    /// The child half of [`prints_are_identical_across_processes`]: writes its prints, one after another, to the file
    /// the variable names.
    #[test]
    #[ignore = "run by prints_are_identical_across_processes"]
    fn print_child() {
        let path = std::env::var_os("PRIMASE_PRINT_CHILD_FILE").expect("set by its parent test");
        let options = PrintOptions {
            dump: Some(EntityFilter::parse("all").unwrap()),
            ..PrintOptions::default()
        };
        let (result, prints) = run(options, vec![Box::new(Edit), Box::new(Break)]);
        result.unwrap_err();
        let all: String = prints
            .iter()
            .map(|(n, t)| {
                format!(
                    "== {n}
{t}"
                )
            })
            .collect();
        std::fs::write(path, all).unwrap();
    }

    /// Each process seeds its hash maps (the world's column table among them) differently: the prints must not show it.
    #[test]
    fn prints_are_identical_across_processes() {
        let exe = std::env::current_exe().unwrap();
        let texts = ["a", "b"].map(|run| {
            let path = std::env::temp_dir().join(format!("primase-print-{run}-{}.txt", std::process::id()));
            let status = std::process::Command::new(&exe)
                .args([
                    "--ignored",
                    "--exact",
                    "print::tests::print_child",
                    "--test-threads=1",
                ])
                .env("PRIMASE_PRINT_CHILD_FILE", &path)
                .status()
                .unwrap();
            assert!(status.success());
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            text
        });
        assert_eq!(
            texts[0]
                .matches(
                    "
== "
                )
                .count()
                + 1,
            3
        );
        assert_eq!(texts[0], texts[1]);
    }
}
