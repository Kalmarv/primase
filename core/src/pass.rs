//! The pass manager (DESIGN.md §4): passes that transform the world, the pipelines a program states, the targets
//! writers declare, and the verifier that runs between every step.
//!
//! - A [`Pass`] has a stable name, text options, and declares the component and resource names it **requires**,
//!   **provides**, **removes** and **modifies** (in place), plus what it approximates (a name is a component's or a
//!   resource's, never both), and the run **settings** it reads. An input (required) is content a reader or an
//!   earlier pass makes, and a world may simply not hold it: a pass that opts in is skipped where it finds none of its
//!   inputs ([`Pass::may_lack_inputs`]). A setting ([`Setting`]) is how the program runs the pass, never made from
//!   the content: its thread count (`core.threads`). The program states its settings once, on the manager
//!   ([`PassManager::with_settings`], [`Settings`]), which states them in every world it runs a pipeline on, a
//!   conversion's parts included, as operational resources no writer writes. A pass that runs without a setting it
//!   declares is refused like one without an input ([`Error::Missing`]), but a setting never decides whether a pass is
//!   skipped, and the run report states each setting a step read with its value (`threads 16`,
//!   [`StepReport::settings`]). A value that changes what a pass makes is neither: it is the pass's option.
//!   After a pass runs, the names that actually appeared and disappeared must be among those it declared. A pass that
//!   declares it **despawns entities** (leaves placements out, discards forms) may also make a name disappear that
//!   only despawned entities held: no surviving entity lost it. A pass that **removes by log** may make a component name disappear that it recorded
//!   as removed ([`PassLog::removed`]: what a hook dropped, known only when it runs).
//! - A [`Pipeline`] is the program's: it states every pass, in order, with its options (`load-meshes,
//!   lower-pbr{normals=rebuild}`, [`PassRegistry::parse`]), and runs as written. The manager never chooses, adds or
//!   reorders a pass.
//! - A [`Target`] (a writer's declaration) gives each component name a [`Legality`]: legal, dynamic (a predicate on
//!   the entity's value), covered (written through another component) or illegal. A name it does not list is not
//!   written: [`Mode::Partial`] keeps it and reports it, [`Mode::Full`] refuses it. A target never carries a name it
//!   does not write (CONTRIBUTING.md, law 4).
//! - [`PassManager::convert`] runs a stated pipeline and checks the world it leaves against a target: a name the
//!   target still refuses is an error that says whether any pass of the pipeline removes or rewrites it.
//! - The verifier ([`Component::verify`] on every component, [`crate::Resource::verify`] on every resource, plus the
//!   manager's world [`Validator`]s) runs on a conversion's reader output and before its writer in every build, and
//!   after every pass only with the pass checks ([`PassManager::with_pass_checks`]): on in debug builds, off by
//!   default in release builds for speed (a program opts in); its errors name the step and the entity.
//! - An [`Instrument`] sees the world before and after every pass and when one fails (`print-world-after`, see
//!   [`crate::print`]). [`PassManager::run`] and [`PassManager::convert`] are [`PassManager::run_instrumented`] and
//!   [`PassManager::convert_instrumented`] with an instrument that does nothing.
//!
//! Everything is deterministic: passes and names are walked in sorted order.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod memo;
pub mod stages;

pub use crate::ledger::{Conservation, Gap, Measure};
pub use crate::pass_options::{Choice, Given, OptionSpec, PassSpec, TextForm};
use crate::world::{Component, Entity, Resource, World};
pub use stages::Stages;

/// A pass's text options (`name{key=value,...}`), sorted by key.
pub type Options = BTreeMap<String, String>;

/// Counts a pass logs while it runs: what it changed, what it approximated and what it left out, and the entities it
/// placed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassLog {
    pub counts: BTreeMap<String, u64>,
    pub approximations: BTreeMap<String, u64>,
    /// Of [`PassLog::approximations`], the entities each reason was recorded on ([`PassLog::approximate_on`]): the
    /// material or mesh the approximation concerns, by reason. An approximation recorded with [`PassLog::approximate`]
    /// names none. The ids are the world's the pass ran on; the manager refuses one that is not alive after the pass.
    pub approximated_on: BTreeMap<String, BTreeSet<Entity>>,
    /// What the pass left out because what it makes has no form for it, by reason (`collision triangles on a surface
    /// of another kind, left out: PHYS_CASTER`). A conversion reports each as an unsupported notice.
    pub omitted: BTreeMap<String, u64>,
    /// Component names the pass removed from some entity without declaring them in [`Pass::removes`] (a component a
    /// hook dropped, as [`World::apply_linear`] reports): accepted as removals only for a pass that opts in with
    /// [`Pass::removes_by_log`].
    pub removed: BTreeSet<String>,
    /// Entities the pass made that stand on their own: content nothing need reference (a marker it places). A
    /// conversion keeps them with the writer's roots ([`crate::convert`]); any other entity a pass makes is kept only
    /// while something kept references it. The manager refuses an entity here that the pass did not make.
    pub placed: BTreeSet<Entity>,
    /// Of [`PassLog::placed`], those made from one source entity, with it ([`PassLog::place_from`]).
    pub placed_from: BTreeMap<Entity, Entity>,
    /// What the pass left out on purpose, by measure and reason (`placements in an interior left out`); a conversion
    /// reports each as a selection notice ([`crate::ledger`]).
    pub drops: BTreeMap<(Measure, String), u64>,
    /// Display-only structured explanations that the typed selections/exclusions, approximations and counts cannot
    /// express; prefer those typed entries first. A program may show these values generically, but must never parse
    /// their fields or branch on their contents. A value needed for a decision belongs in a typed core field.
    /// Repeated entries under one label retain their order when logs join. JSON reporters omit this field when empty.
    pub details: BTreeMap<String, Vec<serde_json::Value>>,
}

impl PassLog {
    pub fn count(&mut self, what: &str, n: u64) {
        *self.counts.entry(what.to_owned()).or_default() += n;
    }

    /// Append a display-only explanation (see [`PassLog::details`]), without replacing an earlier explanation.
    pub fn detail(&mut self, label: &str, value: serde_json::Value) {
        self.details.entry(label.to_owned()).or_default().push(value);
    }

    /// Record that the pass removed component `name` from some entity (see [`PassLog::removed`]).
    pub fn record_removed(&mut self, name: &str) {
        self.removed.insert(name.to_owned());
    }

    /// Record `n` items lowered with an approximation (`"layer blend baked at 1024 px"`).
    pub fn approximate(&mut self, what: &str, n: u64) {
        *self.approximations.entry(what.to_owned()).or_default() += n;
    }

    /// Record `n` items lowered with an approximation, on the entity `e` they concern (a material whose base albedo
    /// could not be read): [`PassLog::approximate`] and the entity kept with the reason ([`PassLog::approximated_on`]).
    /// The count stays the number of items, not of entities: one call per item, or `n` items of one entity.
    pub fn approximate_on(&mut self, e: Entity, what: &str, n: u64) {
        self.approximate(what, n);
        self.approximated_on.entry(what.to_owned()).or_default().insert(e);
    }

    /// Record `n` items left out because what the pass makes has no form for them (see [`PassLog::omitted`]).
    pub fn omit(&mut self, what: &str, n: u64) {
        *self.omitted.entry(what.to_owned()).or_default() += n;
    }

    /// Record `n` of `measure` left out on purpose, and why (see [`PassLog::drops`]; an entity despawned whole:
    /// [`crate::ledger::despawn`]).
    pub fn dropped(&mut self, measure: Measure, why: &str, n: u64) {
        if n > 0 {
            *self.drops.entry((measure, why.to_owned())).or_default() += n;
        }
    }

    /// The drops it recorded in `measure`, over every reason.
    pub fn declared(&self, measure: Measure) -> u64 {
        self.drops
            .iter()
            .filter(|((m, _), _)| *m == measure)
            .map(|(_, n)| n)
            .sum()
    }

    /// Record that the pass made `e` as content of its own (see [`PassLog::placed`]): content of the whole world, as a
    /// spawn a selection places. Content made from one source entity is [`PassLog::place_from`].
    pub fn place(&mut self, e: Entity) {
        self.placed.insert(e);
    }

    /// Record that the pass made `e` as content of its own from `source` (a marker for a placement): a conversion in
    /// parts keeps it only in the part that owns `source`, so content made from a neighbour loaded as context is
    /// written once, by its owner.
    pub fn place_from(&mut self, e: Entity, source: Entity) {
        self.placed.insert(e);
        self.placed_from.insert(e, source);
    }

    /// Take in the log of work done on the pass's behalf (a function returning its own [`PassLog`]): every count,
    /// approximation and omission added to this log's, its recorded removals and its placed entities (with their
    /// sources) and drops, so nothing it reported is lost on the way to the pass's report.
    pub fn absorb(&mut self, other: PassLog) {
        let PassLog {
            counts,
            approximations,
            approximated_on,
            omitted,
            removed,
            placed,
            placed_from,
            drops,
            details,
        } = other;
        for ((m, why), n) in drops {
            self.dropped(m, &why, n);
        }
        for (k, n) in counts {
            self.count(&k, n);
        }
        for (k, n) in approximations {
            self.approximate(&k, n);
        }
        for (k, entities) in approximated_on {
            self.approximated_on.entry(k).or_default().extend(entities);
        }
        for (k, n) in omitted {
            self.omit(&k, n);
        }
        self.removed.extend(removed);
        self.placed.extend(placed);
        self.placed_from.extend(placed_from);
        for (label, entries) in details {
            self.details.entry(label).or_default().extend(entries);
        }
    }
}

/// The run settings a program states for every pipeline its manager runs ([`PassManager::with_settings`]): how the
/// passes run, never what they make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The threads a pass may use (`core.threads`, [`crate::parallel::THREADS`]).
    pub threads: primase_foundations::parallel::Threads,
}

/// A run setting a pass reads ([`Pass::settings`]): a resource the program states for the run, by name, with how a
/// run report states its value ([`RunSetting::text`]). Made by [`Setting::of`].
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    /// The resource's name (`core.threads`).
    pub name: &'static str,
    text: fn(&World) -> Option<String>,
}

impl Setting {
    /// The setting `T`.
    pub const fn of<T: RunSetting>() -> Self {
        Setting {
            name: T::NAME,
            text: setting_text::<T>,
        }
    }

    /// Its value in `w` as a report states it (`threads 16`); none where `w` does not state it.
    pub fn text(&self, w: &World) -> Option<String> {
        (self.text)(w)
    }
}

/// A resource a program states as a run setting ([`Setting`]): how it runs the passes, never what they make.
pub trait RunSetting: Resource {
    /// The value as a run report states it (`threads 16`).
    fn text(&self) -> String;
}

fn setting_text<T: RunSetting>(w: &World) -> Option<String> {
    w.resource::<T>().map(RunSetting::text)
}

/// What entities leaving made a pass's world drop to stay whole ([`crate::Dropped`]), counted in the pass's log.
fn count_dropped(log: &mut PassLog, d: &crate::Dropped) {
    let n = |x: usize| u64::try_from(x).unwrap_or(u64::MAX);
    for (what, count) in [
        (
            "render graph steps dropped: they drew with an entity that left the world",
            n(d.graph.steps.len()),
        ),
        (
            "render graph lighting pipelines dropped: they selected a dropped step",
            u64::from(d.graph.lighting),
        ),
        (
            "render graph pictures dropped: a dropped step wrote them",
            u64::from(d.graph.picture),
        ),
        (
            "step bindings dropped: only dropped render graph steps made their draws",
            n(d.step_bindings.len()),
        ),
        (
            "step draws dropped: only dropped render graph steps made their draws",
            n(d.step_draws.len()),
        ),
    ] {
        if count > 0 {
            log.count(what, count);
        }
    }
}

/// The entities the world itself took component `name` from to stay whole ([`crate::Dropped`]).
fn dropped_from(d: &crate::Dropped, name: &str) -> BTreeSet<Entity> {
    let list = if name == <crate::StepBindings as crate::Component>::NAME {
        &d.step_bindings
    } else if name == <crate::StepDraws as crate::Component>::NAME {
        &d.step_draws
    } else {
        return BTreeSet::new();
    };
    list.iter().map(|(e, _)| *e).collect()
}

/// A setting read from a world that does not state it ([`Setting`]): a pass that reads one without declaring it in
/// [`Pass::settings`], or a caller outside the pass manager. The program states its settings for the run
/// ([`PassManager::with_settings`]), and no default stands in for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{name}`, which the world does not have: the program states it for the run")]
pub struct Unstated {
    /// The setting's resource name.
    pub name: &'static str,
}

/// A transformation of the world. Declarations are component names (`core.mesh`, a destination's `material`).
pub trait Pass: Send + Sync {
    /// Stable name, used in pipelines and reports (`load-meshes`).
    fn name(&self) -> &'static str;
    /// The options this instance was made with (printed into the pipeline).
    fn options(&self) -> Options {
        Options::new()
    }
    /// Whether a run may skip it in a world (or a part) that holds none of its required names: a lowering whose input
    /// the world may simply not have (a part of only a light has no materials, a read region no lights) opts in;
    /// the step is recorded as skipped ([`StepReport::skipped`]). Required names are prerequisites, so this is false
    /// unless the pass says otherwise: an absent name may be a failed load or a wrong order, and a pass that
    /// summarizes, decides or plans never opts in.
    fn may_lack_inputs(&self) -> bool {
        false
    }
    /// Names that must be present before it runs.
    fn requires(&self) -> &[&'static str] {
        &[]
    }
    /// The run settings it reads, which the program states for the run on its manager (its thread count,
    /// [`crate::parallel::THREADS`], [`PassManager::with_settings`]): checked with [`Pass::requires`] before it runs and
    /// refused where absent, never counted in whether it is skipped ([`Pass::may_lack_inputs`]), and stated in its
    /// step's report.
    fn settings(&self) -> &[Setting] {
        &[]
    }
    /// Names it may add.
    fn provides(&self) -> &[&'static str] {
        &[]
    }
    /// Names it may take away (what it lowers).
    fn removes(&self) -> &[&'static str] {
        &[]
    }
    /// Names whose values it edits in place, and resources it writes (replaced, also by removing and inserting again,
    /// or borrowed with [`World::resource_mut`]). The pass manager checks the resources; an in-place edit of a
    /// component's value on an entity that keeps it (`get_mut`, or [`World::insert`] over the value) is declared here
    /// and not checked: seeing it would take a controlled editing interface, later work. What the gap costs: a pass that
    /// edits a value in place without declaring it here is never named, in any build, and its edit can make what the
    /// declarations say about that name wrong ([`Pipeline::unfed`], [`Pipeline::check`]); one that leaves an invalid value there is named at the pass in a build with the
    /// pass checks, and in a release build without them only by the verifier that runs before the writer, which names the
    /// entity but not the pass. Adding a name to an entity, taking one away and despawning are checked entity by entity
    /// against [`Pass::provides`], [`Pass::removes`] and [`Pass::despawns_entities`].
    fn modifies(&self) -> &[&'static str] {
        &[]
    }
    /// What it approximates, in words (empty when it is exact).
    fn approximates(&self) -> &'static str {
        ""
    }
    /// Whether it despawns entities. If so, a component name it does not declare in [`Pass::removes`] may disappear
    /// when every entity that held the name before the pass was despawned by it (a dropped placement's
    /// `core.name`, a discarded form's dialect data); a name still held by a surviving entity before the pass must
    /// be declared as ever. Added names are checked as for every pass.
    fn despawns_entities(&self) -> bool {
        false
    }
    /// Whether the components it removes are not all known in advance (a hook decides, as
    /// [`crate::Component::apply_linear`] does). If so, a component name that disappears is accepted when the pass
    /// recorded it in [`PassLog::removed`] as well as when it is declared in [`Pass::removes`]; a name that disappears
    /// unrecorded and undeclared is an error as ever, and a resource it removes must be declared as for any pass.
    fn removes_by_log(&self) -> bool {
        false
    }
    /// Names whose values it brings to their final form (a classifier: every surface classified). A later pass in a
    /// pipeline that provides one of them adds values that never went through this pass: unless a pass after it settles
    /// the name again, [`PassManager::run`] refuses such a pipeline before it runs ([`Error::SettledThenChanged`]).
    /// Modifying settled values later is allowed. A modifier must not add
    /// values without declaring them in [`Pass::provides`]; the target's value rules check the result.
    fn settles(&self) -> &[&'static str] {
        &[]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String>;
}

/// The passes a manager knows, by name: each by its declaration ([`PassSpec`]).
#[derive(Default)]
pub struct PassRegistry {
    specs: BTreeMap<&'static str, &'static PassSpec>,
}

impl fmt::Debug for PassRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.specs.keys()).finish()
    }
}

impl PassRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pass by its declaration: the registry checks every pipeline's options against it before the pass is
    /// made ([`PassSpec::check`]), makes it from the examples to read its declarations ([`PassSpec::example`]; a pass
    /// with a required option is never run so: a pipeline that leaves the option out fails to parse), and says what it
    /// takes ([`PassRegistry::describe`]). Panics when the name is taken, the declaration is wrong ([`PassSpec`]: a key
    /// twice, a group of one, a choice with no values, an optional option that says nothing of its absence), the pass
    /// is not made from its examples or is not the pass named, or what it prints back ([`Pass::options`]) holds an undeclared key, leaves out a required option or a
    /// group, or does not make a pass that prints the same.
    pub fn add(&mut self, spec: &'static PassSpec) {
        let name = spec.name;
        spec.validate();
        let example = spec.example();
        let made = spec
            .instance(&example)
            .unwrap_or_else(|e| panic!("pass {name} with its example options {example:?}: {e}"));
        assert_eq!(
            made.name(),
            name,
            "factory registered as {name} makes {}",
            made.name()
        );
        let printed = made.options();
        if let Err(e) = spec.check(&printed) {
            panic!("pass {name} prints {printed:?}, which its declaration refuses: {e}");
        }
        let again = spec
            .instance(&printed)
            .unwrap_or_else(|e| panic!("pass {name}: what it prints, {printed:?}, does not make it: {e}"));
        assert_eq!(
            again.options(),
            printed,
            "pass {name}: made from what it prints, it prints otherwise"
        );
        assert!(
            self.specs.insert(name, spec).is_none(),
            "pass {name} registered twice"
        );
    }

    /// What pass `name` takes, from its declaration ([`PassSpec::describe`]); `None` for a pass not registered.
    pub fn describe(&self, name: &str) -> Option<String> {
        self.specs.get(name).map(|spec| spec.describe())
    }

    /// Whether `name` was registered with required options ([`PassSpec::requires_options`]): made with its example only
    /// to read its declarations, never to run.
    pub fn requires_options(&self, name: &str) -> bool {
        self.specs.get(name).is_some_and(|spec| spec.requires_options())
    }

    /// Every registered pass's name, in order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.specs.keys().copied()
    }

    /// The pass `name` made with its declaration's example options (none, or a required-option pass's example).
    pub fn make_registered(&self, name: &str) -> Result<Box<dyn Pass>, Error> {
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| Error::UnknownPass(name.to_owned()))?;
        self.make(name, &spec.example())
    }

    pub fn make(&self, name: &str, options: &Options) -> Result<Box<dyn Pass>, Error> {
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| Error::UnknownPass(name.to_owned()))?;
        spec.instance(options).map_err(|message| Error::Options {
            pass: name.to_owned(),
            message,
        })
    }

    /// A pipeline from its text form: `pass, pass{key=value,key=value}, ...` (whitespace around items ignored).
    pub fn parse(&self, text: &str) -> Result<Pipeline, Error> {
        let mut passes = Vec::new();
        for (name, options) in parse_pipeline(text)? {
            passes.push(self.make(&name, &options)?);
        }
        Ok(Pipeline { passes })
    }

    /// Describe a preparation without discarding its unfed texts. `runnable` is the declaration-based
    /// projection, while `requested` retains every text for an explicitly optional run
    /// ([`PassManager::with_missing_inputs`]). Invalid texts remain requested and runnable so parsing
    /// still reports their error; they are never silently treated as absent source content.
    pub fn prepare_texts(&self, present: &BTreeSet<String>, texts: Vec<String>) -> TextPreparation {
        let mut held = present.clone();
        let mut runnable = Vec::new();
        let mut unfed = Vec::new();
        for text in &texts {
            let Ok(pipeline) = self.parse(text) else {
                runnable.push(text.clone());
                continue;
            };
            let inputs = pipeline.unfed(&held);
            if inputs.passes.is_empty() {
                held = inputs.held;
                runnable.push(text.clone());
            } else {
                let mut missing = Vec::new();
                for name in inputs.passes.iter().flat_map(|(_, names)| names) {
                    if !missing.iter().any(|held| held == name) {
                        missing.push((*name).to_owned());
                    }
                }
                unfed.push((text.clone(), missing));
            }
        }
        TextPreparation {
            requested: texts,
            runnable,
            unfed,
        }
    }
}

fn is_word(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub(crate) fn parse_pipeline(text: &str) -> Result<Vec<(String, Options)>, Error> {
    let bad = |m: String| Error::Parse(format!("{m} in pipeline {text:?}"));
    let mut out = Vec::new();
    let mut rest = text.trim();
    if rest.is_empty() {
        return Ok(out);
    }
    loop {
        let end = rest.find([',', '{']).unwrap_or(rest.len());
        let name = rest[..end].trim();
        if !is_word(name) {
            return Err(bad(format!("{name:?} is not a pass name")));
        }
        rest = &rest[end..];
        let mut options = Options::new();
        if let Some(r) = rest.strip_prefix('{') {
            let close = r
                .find('}')
                .ok_or_else(|| bad(format!("{name}: no closing brace")))?;
            // A value may hold commas (`rect=x0,y0,x1,y1`, as `Display` writes it): an item without `=` continues the
            // previous option's value.
            let mut last: Option<String> = None;
            for item in r[..close].split(',').filter(|s| !s.trim().is_empty()) {
                let Some((k, v)) = item.split_once('=') else {
                    let value = last
                        .as_ref()
                        .and_then(|k| options.get_mut(k))
                        .ok_or_else(|| bad(format!("{name}: option {item:?} is not key=value")))?;
                    value.push(',');
                    value.push_str(item.trim());
                    continue;
                };
                let (k, v) = (k.trim(), v.trim());
                if !is_word(k) || v.contains('{') {
                    return Err(bad(format!("{name}: bad option {item:?}")));
                }
                if options.insert(k.to_owned(), v.to_owned()).is_some() {
                    return Err(bad(format!("{name}: option {k} twice")));
                }
                last = Some(k.to_owned());
            }
            rest = r[close + 1..].trim_start();
        }
        out.push((name.to_owned(), options));
        if rest.is_empty() {
            return Ok(out);
        }
        rest = rest
            .strip_prefix(',')
            .ok_or_else(|| bad(format!("unexpected {rest:?}")))?
            .trim_start();
    }
}

/// Passes in the order they run.
#[derive(Default)]
pub struct Pipeline {
    pub passes: Vec<Box<dyn Pass>>,
}

/// A preparation's requested texts and the declaration-based projection on the source's names.
/// Consumers retain `unfed` in their report instead of losing the reason a text was not fed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TextPreparation {
    /// Every caller-requested text, in order, including ones whose inputs were absent.
    pub requested: Vec<String>,
    /// The texts whose declarations were fed by the initial names or preceding runnable texts.
    pub runnable: Vec<String>,
    /// Unfed texts and their absent names; no component or resource is synthesized for them.
    pub unfed: Vec<(String, Vec<String>)>,
}

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pipeline({self})")
    }
}

/// The text form, which [`PassRegistry::parse`] reads back.
impl Pipeline {
    /// The checks [`PassManager::run`] makes from the passes' declarations before it runs anything: today, that no
    /// pass provides a name an earlier pass settles ([`Pass::settles`]) unless a later one settles it again. It needs
    /// no world, so a program's tests run it on every pipeline it composes (data-free, unlike a run).
    pub fn check(&self) -> Result<()> {
        check_settled(&self.passes)
    }

    /// The passes a run would refuse for a required name absent, in a world holding the component and resource names
    /// `present` before it runs, and the names held after the passes that can run: each pass in order, with what the
    /// passes before it provide added. Settings ([`Pass::settings`]) are not inputs: a pass is never unfed for one, and a
    /// run without one refuses the pass rather than leaving it out. That is the declarations' answer, not a run's: a pass may add less than it
    /// provides and take away less than it removes, so every name provided is taken as added and every name removed as
    /// kept. A pass that may lack its inputs and finds none of them is skipped, as a run skips it, and provides nothing;
    /// one that finds some but not all is refused, as a run refuses it ([`Pass::may_lack_inputs`]). A refused pass
    /// provides nothing: the passes after it are checked as if it were left out, where a run would stop at it.
    pub fn unfed(&self, present: &BTreeSet<String>) -> Unfed {
        let mut held = present.clone();
        let mut passes = Vec::new();
        for p in &self.passes {
            let missing: Vec<&'static str> = p
                .requires()
                .iter()
                .copied()
                .filter(|&r| !held.contains(r))
                .collect();
            if p.may_lack_inputs() && !p.requires().is_empty() && missing.len() == p.requires().len() {
                continue;
            }
            if missing.is_empty() {
                held.extend(p.provides().iter().map(|&n| n.to_owned()));
            } else {
                passes.push((pass_text(p.as_ref()), missing));
            }
        }
        Unfed { passes, held }
    }
}

/// What [`Pipeline::unfed`] finds of a pipeline in a world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfed {
    /// Each pass a run would refuse for a required name absent: its text and the names it lacks, in pipeline order;
    /// empty when every pass can run or is skipped.
    pub passes: Vec<(String, Vec<&'static str>)>,
    /// The names held after the passes that can run: those present before, with what each such pass provides. The
    /// world a following pipeline is checked against.
    pub held: BTreeSet<String>,
}

impl fmt::Display for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, p) in self.passes.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(&pass_text(p.as_ref()))?;
        }
        Ok(())
    }
}

/// Whether the pipeline text form carries `value` as an option's value: [`PassRegistry::parse`] reads back exactly
/// what [`pass_text`] writes. It does not for a value with a brace, with a comma-separated part after the first that
/// is blank, holds `=` (it would start another option) or has whitespace at its ends, or with whitespace at the
/// value's own ends. A pass whose options take text checks its values with this, so its text form reads back as it.
pub fn carries(value: &str) -> bool {
    parse_pipeline(&format!("p{{k={value}}}"))
        .ok()
        .and_then(|mut passes| passes.pop())
        .and_then(|(_, options)| options.get("k").cloned())
        .is_some_and(|v| v == value)
}

/// A pass with its options as pipeline text (`bake{size=512}`), as [`PassRegistry::parse`] reads it; what
/// `core.provenance` records for the passes that derive an entity.
pub fn pass_text(p: &dyn Pass) -> String {
    stated(p.name(), &p.options())
}

/// `value` as text (`read-fromsoft-area{area=m10_00_00_00,margin=30}`), in a pass's form; [`parse_text_form`] reads it
/// back.
pub fn text_form<T: TextForm>(value: &T) -> String {
    stated(T::NAME, &value.options())
}

/// `name` with `options` as pipeline text: the name alone where there are none.
fn stated(name: &str, options: &Options) -> String {
    if options.is_empty() {
        return name.to_owned();
    }
    let items: Vec<String> = options.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{name}{{{}}}", items.join(","))
}

/// The `T` that `text` states: one item, named [`TextForm::NAME`], whose options the declaration lets through
/// ([`TextForm::OPTIONS`]; refused as a pass's would be) and `T` takes.
pub fn parse_text_form<T: TextForm>(text: &str) -> Result<T, Error> {
    let mut items = parse_pipeline(text)?;
    let (name, options) = match (items.pop(), items.is_empty()) {
        (Some(item), true) => item,
        _ => return Err(Error::Parse(format!("{text:?} is not one {}", T::NAME))),
    };
    if name != T::NAME {
        return Err(Error::Parse(format!("{name} is not {}", T::NAME)));
    }
    let refused = |message| Error::Stated {
        name: T::NAME,
        message,
    };
    crate::pass_options::check_options(T::NAME, T::OPTIONS, &options).map_err(refused)?;
    T::make(&Given::new(T::NAME, T::OPTIONS, &options)).map_err(refused)
}

/// The checks [`PassRegistry::add`] makes of a pass, made of `T`: panics where the declaration is wrong (a key twice,
/// a group of one, a choice with no values, an optional option that says nothing of its absence), `T` is not made from
/// the declaration's examples, or what it prints is refused by the declaration or does not make a `T` that prints the
/// same.
pub fn validate_text_form<T: TextForm>() {
    let name = T::NAME;
    assert!(is_word(name), "{name:?} is not a name a text can start with");
    crate::pass_options::validate_options(name, T::OPTIONS);
    let example = PassSpec {
        name,
        options: T::OPTIONS,
        make: |_| unreachable!("the example is only read"),
    }
    .example();
    let made = T::make(&Given::new(name, T::OPTIONS, &example))
        .unwrap_or_else(|e| panic!("{name} with its example options {example:?}: {e}"));
    let printed = made.options();
    if let Err(e) = crate::pass_options::check_options(name, T::OPTIONS, &printed) {
        panic!("{name} prints {printed:?}, which its declaration refuses: {e}");
    }
    let again = T::make(&Given::new(name, T::OPTIONS, &printed))
        .unwrap_or_else(|e| panic!("{name}: what it prints, {printed:?}, does not make it: {e}"));
    assert_eq!(
        again.options(),
        printed,
        "{name}: made from what it prints, it prints otherwise"
    );
}

/// A predicate deciding whether one entity's component is legal for a target (e.g. a texture format it writes).
pub type LegalityCheck = Arc<dyn Fn(&World, Entity) -> bool + Send + Sync>;

/// What a target does with one component name. A name it gives no rule is not written: kept and reported in
/// [`Mode::Partial`], refused in [`Mode::Full`]. A writer's target never carries a name through untouched: it writes its
/// format's meaning and reports the rest (CONTRIBUTING.md, law 4).
#[derive(Clone)]
pub enum Legality {
    /// The writer writes it.
    Legal,
    /// Legal for the entities the predicate accepts; the others must be lowered.
    Dynamic(LegalityCheck),
    /// Written through another component: legal on the entities the check accepts (a `core.light` with the
    /// destination's `light` it was raised to); on the rest as an unlisted name (kept and reported as not written in
    /// [`Mode::Partial`], illegal in [`Mode::Full`]). See [`Target::covered`].
    Covered(fn(&World, Entity) -> bool),
    /// Must be lowered before writing.
    Illegal,
}

/// The kind of a target's rule for a name, without its checks ([`Target::rules`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// [`Legality::Legal`].
    Legal,
    /// [`Legality::Dynamic`]: legal on the entities a check accepts.
    Dynamic,
    /// [`Legality::Covered`]: written through another component on the entities a check accepts.
    Covered,
    /// [`Legality::Illegal`].
    Illegal,
}

impl fmt::Debug for Legality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Legality::Legal => "Legal",
            Legality::Dynamic(_) => "Dynamic",
            Legality::Covered(_) => "Covered",
            Legality::Illegal => "Illegal",
        })
    }
}

/// How strict a conversion is about names the target does not list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Only illegal (and failing dynamic) names must go; unlisted names stay, not written.
    Partial,
    /// Every name must be legal; unlisted names are illegal.
    Full,
}

/// A value rule on a resource ([`Target::check_resource`]).
#[derive(Clone)]
struct ResourceRule {
    what: String,
    check: Arc<dyn Fn(&World) -> bool + Send + Sync>,
}

impl fmt::Debug for ResourceRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceRule({:?})", self.what)
    }
}

/// A writer's declaration of what it accepts.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    rules: BTreeMap<String, Legality>,
    /// The words naming a component's value rule ([`Target::check`]), by component name.
    rule_words: BTreeMap<String, String>,
    /// Value rules on resources ([`Target::check_resource`]), by resource name.
    resource_rules: BTreeMap<String, ResourceRule>,
}

impl Target {
    pub fn new(name: impl Into<String>) -> Self {
        Target {
            name: name.into(),
            rules: BTreeMap::new(),
            rule_words: BTreeMap::new(),
            resource_rules: BTreeMap::new(),
        }
    }

    pub fn rule(mut self, name: &str, legality: Legality) -> Self {
        self.rules.insert(name.to_owned(), legality);
        self
    }

    pub fn legal(self, name: &str) -> Self {
        self.rule(name, Legality::Legal)
    }

    pub fn illegal(self, name: &str) -> Self {
        self.rule(name, Legality::Illegal)
    }

    pub fn dynamic(self, name: &str, check: impl Fn(&World, Entity) -> bool + Send + Sync + 'static) -> Self {
        self.rule(name, Legality::Dynamic(Arc::new(check)))
    }

    /// A name the writer writes through another component: legal on the entities `check` accepts, `what` naming the
    /// cover (`"covered by <dialect>.light"`); the rest are reported as `<name> not <what>` ([`Legality::Covered`]).
    pub fn covered(mut self, name: &str, what: &str, check: fn(&World, Entity) -> bool) -> Self {
        self.rule_words.insert(name.to_owned(), what.to_owned());
        self.rule(name, Legality::Covered(check))
    }

    /// The legal checks of a typed component: legal when `check` accepts the value.
    pub fn dynamic_typed<T: Component>(self, check: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.dynamic(T::NAME, move |w, e| w.get::<T>(e).is_some_and(&check))
    }

    /// A named value rule on a typed component: as [`Target::dynamic_typed`], with `what` naming the rule in errors
    /// ("every placement regular").
    pub fn check<T: Component>(
        mut self,
        what: &str,
        check: impl Fn(&T) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.rule_words.insert(T::NAME.to_owned(), what.to_owned());
        self.dynamic_typed::<T>(check)
    }

    /// A named value rule on a resource: the target refuses the world while the resource is there and `check` refuses
    /// it (`"every surface classified"`). A pass of the stated pipeline that modifies or removes the resource fixes it.
    pub fn check_resource<R: Resource>(
        mut self,
        what: &str,
        check: impl Fn(&R) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.resource_rules.insert(
            R::NAME.to_owned(),
            ResourceRule {
                what: what.to_owned(),
                check: Arc::new(move |w: &World| w.resource::<R>().is_none_or(&check)),
            },
        );
        self
    }

    /// The words naming the value rule on `name` (a component's [`Target::check`], a covered name's cover
    /// ([`Target::covered`]) or a resource's [`Target::check_resource`]), if it has a named one.
    pub fn rule_words(&self, name: &str) -> Option<&str> {
        self.rule_words
            .get(name)
            .or_else(|| self.resource_rules.get(name).map(|r| &r.what))
            .map(String::as_str)
    }

    /// The resources of `w` whose value rule refuses them, by name.
    pub fn refused_resources(&self, w: &World) -> Vec<&str> {
        self.resource_rules
            .iter()
            .filter(|(_, r)| !(r.check)(w))
            .map(|(n, _)| n.as_str())
            .collect()
    }

    pub fn legality(&self, name: &str) -> Option<&Legality> {
        self.rules.get(name)
    }

    /// Every name the target states a rule for, with the rule's kind (its checks left out): what a coverage report
    /// lists of the writer.
    pub fn rules(&self) -> Vec<(&str, Rule)> {
        self.rules
            .iter()
            .map(|(name, legality)| {
                let rule = match legality {
                    Legality::Legal => Rule::Legal,
                    Legality::Dynamic(_) => Rule::Dynamic,
                    Legality::Covered(_) => Rule::Covered,
                    Legality::Illegal => Rule::Illegal,
                };
                (name.as_str(), rule)
            })
            .collect()
    }

    /// The resources it checks the values of ([`Target::check_resource`]), by name.
    pub fn checked_resources(&self) -> Vec<&str> {
        self.resource_rules.keys().map(String::as_str).collect()
    }

    /// Check a prepared world against the target without changing it: what the writer won't write (an illegal or an
    /// unlisted name, and a covered name's uncovered entities: [`Prepared::unsupported`], entity counts by name; a
    /// resource it neither lists nor rules on: [`Prepared::unsupported_resources`]; the conversion's own resources,
    /// [`crate::convert::OPERATIONAL_RESOURCES`], aside), and values of names it writes that its value rule refuses
    /// ([`Prepared::invalid`], by name with the rule's words) or resources it refuses ([`Prepared::invalid_resources`]).
    /// A resource is listed like a component ([`Target::legal`], [`Target::illegal`]).
    pub fn check_prepared(&self, w: &World) -> Prepared {
        let mut out = Prepared::default();
        for name in w.component_names() {
            let all = w.entities_named(name);
            match self.rules.get(name) {
                Some(Legality::Legal) => {}
                Some(Legality::Dynamic(f)) => {
                    let bad: Vec<Entity> = all.into_iter().filter(|&e| !f(w, e)).collect();
                    if !bad.is_empty() {
                        let words = self.rule_words(name).unwrap_or_default().to_owned();
                        out.invalid.push((name.to_owned(), words, bad));
                    }
                }
                Some(Legality::Covered(f)) => {
                    let uncovered = all.iter().filter(|&&e| !f(w, e)).count();
                    if uncovered > 0 {
                        let words = self.rule_words(name).unwrap_or_default();
                        out.unsupported
                            .push((format!("{name} not {words}"), uncovered as u64));
                    }
                }
                Some(Legality::Illegal) | None => out.unsupported.push((name.to_owned(), all.len() as u64)),
            }
        }
        out.invalid_resources = self.refused_resources(w).into_iter().map(str::to_owned).collect();
        for name in w.resource_names() {
            let written = crate::convert::OPERATIONAL_RESOURCES.contains(&name)
                || self.resource_rules.contains_key(name)
                || self
                    .rules
                    .get(name)
                    .is_some_and(|rule| !matches!(rule, Legality::Illegal));
            if !written {
                out.unsupported_resources.push(name.to_owned());
            }
        }
        out
    }

    /// Entities whose `name` component the target does not accept as it is (all of them for an illegal name).
    pub fn illegal_entities(&self, w: &World, name: &str, mode: Mode) -> Vec<Entity> {
        let all = w.entities_named(name);
        match self.rules.get(name) {
            Some(Legality::Legal) => Vec::new(),
            Some(Legality::Illegal) => all,
            Some(Legality::Dynamic(f)) => all.into_iter().filter(|&e| !f(w, e)).collect(),
            Some(Legality::Covered(_)) if mode == Mode::Partial => Vec::new(),
            Some(Legality::Covered(f)) => all.into_iter().filter(|&e| !f(w, e)).collect(),
            None if mode == Mode::Partial => Vec::new(),
            None => all,
        }
    }
}

/// What [`Target::check_prepared`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Names the writer doesn't write, with how many entities hold them; a covered name ([`Target::covered`]) as
    /// `<name> not <its cover>` (`core.light not covered by the destination's light`), for its uncovered entities.
    pub unsupported: Vec<(String, u64)>,
    /// Names it writes whose values its rule refuses: the name, the rule's words, the entities.
    pub invalid: Vec<(String, String, Vec<Entity>)>,
    /// Resources its rule refuses.
    pub invalid_resources: Vec<String>,
    /// Resources the writer doesn't write.
    pub unsupported_resources: Vec<String>,
}

/// A world-wide rule (one name -> one payload, frame sanity); returns its failures.
pub trait Validator: Send + Sync {
    fn name(&self) -> &'static str;
    fn check(&self, w: &World) -> Vec<(Option<Entity>, String)>;
}

/// One failure of the verifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyFailure {
    /// The component name or validator that failed.
    pub check: String,
    pub entity: Option<Entity>,
    pub message: String,
}

impl fmt::Display for VerifyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.entity {
            Some(e) => write!(f, "{} on {e:?}: {}", self.check, self.message),
            None => write!(f, "{}: {}", self.check, self.message),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Parse(String),
    #[error("pass {pass} left out {dropped} {measure} but declared {declared} (PassLog::dropped)")]
    Conservation {
        pass: String,
        measure: Measure,
        dropped: u64,
        declared: u64,
    },
    #[error("no pass named {0:?}")]
    UnknownPass(String),
    #[error("pass {pass}: {message}")]
    Options { pass: String, message: String },
    /// A [`TextForm`]'s options refused by its declaration or by it.
    #[error("{name}: {message}")]
    Stated { name: &'static str, message: String },
    #[error("pass {pass} requires `{name}`, which the world does not have")]
    Missing { pass: String, name: String },
    /// A world that already states a run setting otherwise than the program does on its manager.
    #[error("the world states `{name}` as {world}, the program as {program}")]
    SettingDisagrees {
        name: &'static str,
        world: String,
        program: String,
    },
    #[error("pass {pass} failed: {message}")]
    Pass { pass: String, message: String },
    #[error("pass {pass} {what} `{names:?}` without declaring it")]
    Undeclared {
        pass: String,
        what: &'static str,
        names: Vec<String>,
    },
    #[error("verify after {after}: {}", failures.iter().take(10).map(|f| f.to_string()).collect::<Vec<_>>().join("; "))]
    Verify {
        after: String,
        failures: Vec<VerifyFailure>,
    },
    #[error("instrument at {step}: {message}")]
    Instrument { step: String, message: String },
    #[error("{}", illegal_text(.target, .name, .entities, .rule, .lowered_by))]
    Illegal {
        target: String,
        name: String,
        /// The entities refused (none for a resource).
        entities: Vec<Entity>,
        /// The words naming the rule refused, empty when it has none.
        rule: String,
        /// The passes of the stated pipeline that remove or rewrite the name, as pipeline text; empty when nothing in
        /// it does (the pipeline is missing a lowering).
        lowered_by: Vec<String>,
    },
    #[error("{changed_by} provides `{name}` after {settled_by} settled it: its values would not be settled")]
    SettledThenChanged {
        settled_by: String,
        changed_by: String,
        name: String,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

fn illegal_text(target: &str, name: &str, entities: &[Entity], rule: &str, lowered_by: &[String]) -> String {
    let rule = if rule.is_empty() {
        String::new()
    } else {
        format!(" ({rule})")
    };
    let on = if entities.is_empty() {
        String::new()
    } else {
        format!(" on {entities:?}")
    };
    let why = if lowered_by.is_empty() {
        "nothing in the stated pipeline removes or rewrites it".to_owned()
    } else {
        format!("{} left it", lowered_by.join(", "))
    };
    format!("`{target}` does not accept `{name}`{rule}{on} after the pipeline: {why}")
}

/// A pipeline in which a pass provides a name an earlier pass settles ([`Pass::settles`]) and no pass after it settles
/// the name again.
fn check_settled(passes: &[Box<dyn Pass>]) -> Result<()> {
    for (i, a) in passes.iter().enumerate() {
        for &n in a.settles() {
            for (j, b) in passes.iter().enumerate().skip(i + 1) {
                let adds = b.provides().contains(&n) && !b.settles().contains(&n);
                if adds && !passes[j + 1..].iter().any(|c| c.settles().contains(&n)) {
                    return Err(Error::SettledThenChanged {
                        settled_by: pass_text(a.as_ref()),
                        changed_by: pass_text(b.as_ref()),
                        name: n.to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// One pass's run. Its equality is the determinism comparison: every field but [`StepReport::time`] (two runs that made
/// the same are equal however long they took).
#[derive(Clone, Debug)]
pub struct StepReport {
    /// The pass with its options, as in the pipeline text.
    pub pass: String,
    pub approximates: &'static str,
    pub log: PassLog,
    /// Component names that appeared and disappeared.
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// With conservation diagnostics reporting ([`Conservation::Report`]): each measure whose losses differ from what
    /// the pass declared. Empty otherwise.
    pub conservation: Vec<Gap>,
    /// `Some(why)` when the pass failed (its run, its declarations, conservation or the verifier refused it): the
    /// record keeps what its log had noted until then, so its omissions and approximations still reach the report;
    /// `added` and `removed` are then empty (not checked).
    pub failed: Option<String>,
    /// Required names absent when the step was skipped: a pass that opts in and finds none
    /// ([`Pass::may_lack_inputs`], [`SKIPPED`]), or the caller's explicit [`MissingInputs::Skip`]
    /// preparation policy ([`SKIPPED_UNFED`]). Empty when the pass ran.
    pub skipped: Vec<String>,
    /// How long the step took, by the wall clock (zero for a skipped step).
    pub time: StepTime,
    /// The run settings the pass read ([`Pass::settings`]), as the run stated them (`threads 16`), in its declared order;
    /// empty for a skipped or refused step. How the step was run, not what it made: a [`StepReport`]'s equality
    /// leaves them out, as it does the time.
    pub settings: Vec<String>,
}

/// A step's wall-clock time, as the pass manager measured it: for a program's log and for profiling, not part of what
/// a conversion makes (it differs from run to run), so a [`StepReport`]'s equality leaves it out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepTime {
    /// The pass's own [`Pass::run`].
    pub run: Duration,
    /// The manager's work around it: the requirements, the declarations and their effects, conservation diagnostics
    /// and the verifier ([`PassManager::with_pass_checks`]).
    pub checks: Duration,
}

impl PartialEq for StepReport {
    /// Every field but the time and the settings.
    fn eq(&self, other: &StepReport) -> bool {
        let StepReport {
            pass,
            approximates,
            log,
            added,
            removed,
            conservation,
            failed,
            skipped,
            time: _,
            settings: _,
        } = self;
        (
            pass,
            approximates,
            log,
            added,
            removed,
            conservation,
            failed,
            skipped,
        ) == (
            &other.pass,
            &other.approximates,
            &other.log,
            &other.added,
            &other.removed,
            &other.conservation,
            &other.failed,
            &other.skipped,
        )
    }
}
impl Eq for StepReport {}

impl RunReport {
    /// Every step's time added up.
    pub fn time(&self) -> StepTime {
        self.steps.iter().fold(StepTime::default(), |t, s| StepTime {
            run: t.run + s.time.run,
            checks: t.checks + s.time.checks,
        })
    }
}

/// The count a pass skipped for holding none of its required names gets in its log ([`Pass::may_lack_inputs`]), in a
/// whole run or a part run, so a conversion's totals give the skips per pass.
pub const SKIPPED: &str = "runs holding none of its required names, skipped";

/// The count for a preparation step omitted by an explicit [`MissingInputs::Skip`] policy.
pub const SKIPPED_UNFED: &str = "runs missing required names, skipped";

/// How a caller's preparation handles a pass whose required components or resources are absent.
/// A strict conversion keeps [`MissingInputs::Refuse`]; a preparation spanning optional source
/// content may explicitly skip unfed passes, retaining each one in the typed run report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissingInputs {
    /// Refuse absent inputs, except for a pass's own [`Pass::may_lack_inputs`] declaration.
    #[default]
    Refuse,
    /// Skip an unfed step, recording precisely which required names are absent.
    Skip,
}

/// What a conversion did. Its equality is the determinism comparison: every field, the steps compared without their
/// times ([`StepReport`]'s equality).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// The target a conversion checked the world against ([`PassManager::convert`]); empty for a bare run.
    pub target: String,
    /// The stated pipeline a conversion ran, as text; empty for a bare run.
    pub pipeline: String,
    pub steps: Vec<StepReport>,
}

impl fmt::Display for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "target {}: pipeline [{}]", self.target, self.pipeline)?;
        for s in &self.steps {
            write!(f, "  {}: +{:?} -{:?}", s.pass, s.added, s.removed)?;
            for (k, n) in &s.log.counts {
                write!(f, " {k}={n}")?;
            }
            for (k, n) in &s.log.approximations {
                write!(f, " ~{k}={n}")?;
            }
            for (k, n) in &s.log.omitted {
                write!(f, " -{k}={n}")?;
            }
            for ((m, why), n) in &s.log.drops {
                write!(f, " -{m}:{why}={n}")?;
            }
            for g in &s.conservation {
                write!(f, " !{g}")?;
            }
            if s.skipped.is_empty() {
                write!(
                    f,
                    " ({:.3} s, checks {:.3} s",
                    s.time.run.as_secs_f64(),
                    s.time.checks.as_secs_f64()
                )?;
                for setting in &s.settings {
                    write!(f, "; {setting}")?;
                }
                write!(f, ")")?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

/// One pass of a pipeline run, as an [`Instrument`] sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step<'a> {
    /// Position in the pipeline, from 0.
    pub index: usize,
    /// The pass's stable name (`select-lod`).
    pub name: &'static str,
    /// The pass with its options, as in the pipeline text (`select-lod{form=default}`).
    pub text: &'a str,
}

/// Hooks the manager calls around every pass of a run (DESIGN.md §4 instrumentation): before it runs, after it ran and
/// passed the verifier, and instead of `after_pass` when it fails (the world is then as the failure left it). An error
/// from `before_pass` or `after_pass` stops the run as [`Error::Instrument`]; one from `pass_failed` is dropped in
/// favour of the pass's own error. Instruments only look: they get the world by shared reference.
pub trait Instrument {
    fn before_pass(&mut self, _step: &Step<'_>, _w: &World) -> std::result::Result<(), String> {
        Ok(())
    }
    fn after_pass(
        &mut self,
        _step: &Step<'_>,
        _w: &World,
        _report: &StepReport,
    ) -> std::result::Result<(), String> {
        Ok(())
    }
    fn pass_failed(
        &mut self,
        _step: &Step<'_>,
        _w: &World,
        _error: &Error,
    ) -> std::result::Result<(), String> {
        Ok(())
    }
}

/// The instrument of an uninstrumented run.
struct NoInstrument;
impl Instrument for NoInstrument {}

/// Runs the pipelines a program states and verifies the world between steps.
#[derive(Default)]
pub struct PassManager {
    pub registry: PassRegistry,
    validators: Vec<Box<dyn Validator>>,
    conservation: Conservation,
    pass_checks: bool,
    missing_inputs: MissingInputs,
    settings: Option<Settings>,
}

impl fmt::Debug for PassManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PassManager")
            .field("registry", &self.registry)
            .field(
                "validators",
                &self.validators.iter().map(|v| v.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl PassManager {
    pub fn new(registry: PassRegistry) -> Self {
        PassManager {
            registry,
            validators: Vec::new(),
            conservation: Conservation::Off,
            pass_checks: cfg!(debug_assertions),
            missing_inputs: MissingInputs::Refuse,
            settings: None,
        }
    }

    /// State the program's run settings: before every pipeline it runs, plain or a conversion's part, the manager
    /// states them in the world, and a pass that declares a setting ([`Pass::settings`]) reads it there. A manager that
    /// states none runs only passes that read none (another is refused, [`Error::Missing`]); a world that already
    /// states a setting otherwise is refused ([`Error::SettingDisagrees`]).
    pub fn with_settings(mut self, settings: Settings) -> Self {
        self.settings = Some(settings);
        self
    }

    /// The program's settings ([`PassManager::with_settings`]), stated in `w`: refused where `w` already states one
    /// otherwise.
    fn state_settings(&self, w: &mut World) -> Result<()> {
        let Some(Settings { threads }) = self.settings else {
            return Ok(());
        };
        match w.resource::<primase_foundations::parallel::Threads>() {
            Some(&held) if held != threads => Err(Error::SettingDisagrees {
                name: <primase_foundations::parallel::Threads as Resource>::NAME,
                world: RunSetting::text(&held),
                program: RunSetting::text(&threads),
            }),
            Some(_) => Ok(()),
            None => {
                w.insert_resource(threads);
                Ok(())
            }
        }
    }

    /// The pass checks, after every pass: its effects entity by entity against its declarations (what it adds to or
    /// takes from an entity, what it despawns, the resources it writes: [`Pass::provides`], [`Pass::removes`],
    /// [`Pass::modifies`]), then the verifier ([`Component::verify`], [`crate::Resource::verify`], the validators) on
    /// what it leaves. On in debug builds (the test suites); off by default in release builds, for speed, so a pass
    /// that leaves an invalid component is not named there unless the program opts in with `true`. Measured on a
    /// level of 16,000 placements (12 passes, release): the verifier 0.26 s and the declaration diff another
    /// 0.46 s, against 1.6 s of passes. What a pass's names do across the whole world is checked either way, and a
    /// conversion ([`PassManager::convert`]) verifies the reader's output and the world before its writer either way.
    /// The in-place edits of values on entities that keep the name are not in the declaration diff in any build
    /// ([`Pass::modifies`]); only the verifier sees their result, and only after the pass when the checks are on.
    pub fn with_pass_checks(mut self, on: bool) -> Self {
        self.pass_checks = on;
        self
    }

    pub fn pass_checks(&self) -> bool {
        self.pass_checks
    }

    /// State how to handle unfed preparation steps. Skipping never supplies default data or
    /// calls an unfed pass: its [`StepReport::skipped`] lists the absent names, and later steps
    /// inspect the world actually left by earlier steps. The default remains strict refusal.
    pub fn with_missing_inputs(mut self, policy: MissingInputs) -> Self {
        self.missing_inputs = policy;
        self
    }

    /// Run the conservation diagnostics around every pass ([`crate::ledger`]): off by default.
    pub fn with_conservation(mut self, c: Conservation) -> Self {
        self.conservation = c;
        self
    }

    pub fn conservation(&self) -> Conservation {
        self.conservation
    }

    pub fn add_validator(&mut self, v: Box<dyn Validator>) {
        self.validators.push(v);
    }

    /// Every component's own checks and every world validator. `after` names the step for the error.
    pub fn verify(&self, w: &World, after: &str) -> Result<()> {
        let mut failures: Vec<VerifyFailure> = w
            .verify_components()
            .into_iter()
            .map(|(c, e, message)| VerifyFailure {
                check: c.to_owned(),
                entity: Some(e),
                message,
            })
            .collect();
        failures.extend(
            w.verify_resources()
                .into_iter()
                .map(|(r, message)| VerifyFailure {
                    check: r.to_owned(),
                    entity: None,
                    message,
                }),
        );
        for v in &self.validators {
            failures.extend(v.check(w).into_iter().map(|(entity, message)| VerifyFailure {
                check: v.name().to_owned(),
                entity,
                message,
            }));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Verify {
                after: after.to_owned(),
                failures,
            })
        }
    }

    /// Run a pipeline as written, verifying after every pass and checking each pass's declarations.
    pub fn run(&self, w: &mut World, pipeline: &Pipeline, report: &mut RunReport) -> Result<()> {
        self.run_instrumented(w, pipeline, report, &mut NoInstrument)
    }

    /// [`PassManager::run`], calling `instrument` around every pass.
    pub fn run_instrumented(
        &self,
        w: &mut World,
        pipeline: &Pipeline,
        report: &mut RunReport,
        instrument: &mut dyn Instrument,
    ) -> Result<()> {
        self.run_with(w, pipeline, report, instrument)
    }

    /// [`PassManager::run`] on one part of a conversion ([`crate::convert`]), with the same rule as any run: a pass that
    /// opts in ([`Pass::may_lack_inputs`]), requires names and finds none of them has nothing to do there and is
    /// skipped, recorded (its step's [`StepReport::skipped`], a [`SKIPPED`] count in its log). Any other pass whose
    /// required names are absent fails, as does an opted-in pass that finds some of them but not all; a pass that
    /// requires nothing always runs. An explicit [`MissingInputs::Skip`] policy also skips
    /// other unfed steps, retaining their missing names in the report.
    pub fn run_part(&self, w: &mut World, pipeline: &Pipeline, report: &mut RunReport) -> Result<()> {
        self.run_with(w, pipeline, report, &mut NoInstrument)
    }

    fn run_with(
        &self,
        w: &mut World,
        pipeline: &Pipeline,
        report: &mut RunReport,
        instrument: &mut dyn Instrument,
    ) -> Result<()> {
        pipeline.check()?;
        self.state_settings(w)?;
        for (index, p) in pipeline.passes.iter().enumerate() {
            let text = pass_text(p.as_ref());
            let missing: Vec<String> = p
                .requires()
                .iter()
                .filter(|r| !w.has_component_named(r) && !w.resource_names().contains(r))
                .map(|r| (*r).to_owned())
                .collect();
            let declared_skip =
                p.may_lack_inputs() && !p.requires().is_empty() && missing.len() == p.requires().len();
            if declared_skip || (self.missing_inputs == MissingInputs::Skip && !missing.is_empty()) {
                let mut log = PassLog::default();
                log.count(if declared_skip { SKIPPED } else { SKIPPED_UNFED }, 1);
                report.steps.push(StepReport {
                    pass: text,
                    approximates: p.approximates(),
                    log,
                    added: Vec::new(),
                    removed: Vec::new(),
                    conservation: Vec::new(),
                    failed: None,
                    skipped: missing,
                    time: StepTime::default(),
                    settings: Vec::new(),
                });
                continue;
            }
            let step = Step {
                index,
                name: p.name(),
                text: &text,
            };
            let hook = |r: std::result::Result<(), String>| {
                r.map_err(|message| Error::Instrument {
                    step: text.clone(),
                    message,
                })
            };
            hook(instrument.before_pass(&step, w))?;
            // what a part owns before the pass: a source the pass consumes still classifies what it placed
            let owned_before = w
                .resource::<crate::convert::PartScope>()
                .map(|s| s.owned(w))
                .unwrap_or_default();
            let mut log = PassLog::default();
            let started = Instant::now();
            let mut run = Duration::ZERO;
            let ran = self.run_pass(w, p.as_ref(), &text, &mut log, &mut run);
            let time = StepTime {
                run,
                checks: started.elapsed().saturating_sub(run),
            };
            match ran {
                Ok(mut done) => {
                    done.time = time;
                    // a part's declared output so far, for the passes after (PartScope::output)
                    crate::convert::PartScope::declare(
                        w,
                        &done.log.placed,
                        &done.log.placed_from,
                        &owned_before,
                    );
                    hook(instrument.after_pass(&step, w, &done))?;
                    report.steps.push(done);
                }
                Err(e) => {
                    // the failed step's record keeps what its log noted, marked failed
                    report.steps.push(StepReport {
                        pass: text.clone(),
                        approximates: p.approximates(),
                        log,
                        added: Vec::new(),
                        removed: Vec::new(),
                        conservation: Vec::new(),
                        failed: Some(e.to_string()),
                        skipped: Vec::new(),
                        time,
                        settings: Vec::new(),
                    });
                    // the pass's error is the one that matters; a failing hook has nothing to add to it
                    let _ = instrument.pass_failed(&step, w, &e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// One pass: its requirements, the run, its declarations, the verifier. `log` is the caller's, so what the pass
    /// noted survives a failure; `run` is set to how long the pass's own run took (left zero if it never ran).
    fn run_pass(
        &self,
        w: &mut World,
        p: &dyn Pass,
        text: &str,
        log: &mut PassLog,
        run: &mut Duration,
    ) -> Result<StepReport> {
        for &r in p.requires() {
            if !w.has_component_named(r) && !w.resource_names().contains(&r) {
                return Err(Error::Missing {
                    pass: text.to_owned(),
                    name: r.to_owned(),
                });
            }
        }
        let settings = p
            .settings()
            .iter()
            .map(|s| {
                s.text(w).ok_or_else(|| Error::Missing {
                    pass: text.to_owned(),
                    name: s.name.to_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let before: BTreeSet<String> = w.component_names().into_iter().map(str::to_owned).collect();
        let resources_before: BTreeSet<String> = w.resource_names().into_iter().map(str::to_owned).collect();
        // who held each name a despawning pass may make disappear without declaring it
        let holders: BTreeMap<&str, Vec<Entity>> = if p.despawns_entities() {
            before
                .iter()
                .filter(|n| !p.removes().contains(&n.as_str()))
                .map(|n| (n.as_str(), w.entities_named(n)))
                .collect()
        } else {
            BTreeMap::new()
        };
        // who held each name, per entity: what the pass adds to or takes from an entity is checked against its
        // declarations, not only the names that appear or disappear across the whole world
        let held_before: BTreeMap<&str, BTreeSet<Entity>> = if self.pass_checks {
            before
                .iter()
                .map(|n| (n.as_str(), w.entities_named(n).into_iter().collect()))
                .collect()
        } else {
            BTreeMap::new()
        };
        if self.pass_checks {
            w.start_effects();
        }
        let first_made = w.next_index();
        let ledger = (self.conservation != Conservation::Off).then(|| {
            w.start_rebuild_log();
            crate::ledger::Before::take(w)
        });
        let started = Instant::now();
        let ran = p.run(w, log);
        *run = started.elapsed();
        let rebuilds = w.take_rebuild_log();
        let mut effects = w.take_effects();
        effects.dropped = w.take_dropped();
        ran.map_err(|message| Error::Pass {
            pass: text.to_owned(),
            message,
        })?;
        count_dropped(log, &effects.dropped);
        let conservation = ledger.map_or_else(Vec::new, |b| b.gaps(w, &rebuilds, log));
        if self.conservation == Conservation::Enforce
            && let Some(g) = conservation.first()
        {
            return Err(Error::Conservation {
                pass: text.to_owned(),
                measure: g.measure,
                dropped: g.dropped,
                declared: g.declared,
            });
        }
        // what it placed it made (allocated during the pass: indices are never reused), and it is there, unless it was
        // an intermediate: made, placed, something else placed from it, then consumed in the same pass
        // (PartScope::declare still classifies what came from it); every source was made by someone
        let made = first_made..w.next_index();
        let consumed = |e: &Entity| log.placed_from.values().any(|s| s == e);
        if let Some(e) = log
            .placed
            .iter()
            .find(|e| !made.contains(&e.index()) || (!w.is_alive(**e) && !consumed(e)))
        {
            return Err(Error::Pass {
                pass: text.to_owned(),
                message: if e.index() < first_made {
                    format!("placed {e:?}, which it did not make")
                } else if e.index() >= made.end {
                    format!("placed {e:?}, which was never made")
                } else {
                    format!("placed {e:?}, which is not alive after it, and nothing it placed came from it")
                },
            });
        }
        // what it attributed an approximation to is alive: a pass that names a dead entity has a bug, not a notice
        if let Some((what, e)) = log
            .approximated_on
            .iter()
            .flat_map(|(what, entities)| entities.iter().map(move |e| (what, e)))
            .find(|(_, e)| !w.is_alive(**e))
        {
            return Err(Error::Pass {
                pass: text.to_owned(),
                message: format!("approximation {what:?} attributed to {e:?}, which is not alive after it"),
            });
        }
        if let Some((e, s)) = log.placed_from.iter().find(|(_, s)| s.index() >= made.end) {
            return Err(Error::Pass {
                pass: text.to_owned(),
                message: format!("placed {e:?} from {s:?}, which was never made"),
            });
        }
        let after: BTreeSet<String> = w.component_names().into_iter().map(str::to_owned).collect();
        let resources_after: BTreeSet<String> = w.resource_names().into_iter().map(str::to_owned).collect();
        let mut added: Vec<String> = after.difference(&before).cloned().collect();
        let mut removed: Vec<String> = before.difference(&after).cloned().collect();
        let despawned_with = |n: &str| {
            holders
                .get(n)
                .is_some_and(|held| held.iter().all(|&e| !w.is_alive(e)))
        };
        for (what, names, declared, removal) in [
            ("added", &added, p.provides(), false),
            ("removed", &removed, p.removes(), true),
        ] {
            let undeclared: Vec<String> = names
                .iter()
                .filter(|n| {
                    !declared.contains(&n.as_str())
                        && !(removal && despawned_with(n))
                        && !(removal && !dropped_from(&effects.dropped, n).is_empty())
                        && !(removal && p.removes_by_log() && log.removed.contains(n.as_str()))
                })
                .cloned()
                .collect();
            if !undeclared.is_empty() {
                return Err(Error::Undeclared {
                    pass: text.to_owned(),
                    what,
                    names: undeclared,
                });
            }
        }
        // resources by the same declarations; despawning never takes one away, so no exemption
        for (what, names, declared) in [
            (
                "added",
                resources_after.difference(&resources_before),
                p.provides(),
            ),
            (
                "removed",
                resources_before.difference(&resources_after),
                p.removes(),
            ),
        ] {
            let names: Vec<String> = names.cloned().collect();
            let undeclared: Vec<String> = names
                .iter()
                .filter(|n| !declared.contains(&n.as_str()))
                .cloned()
                .collect();
            if !undeclared.is_empty() {
                return Err(Error::Undeclared {
                    pass: text.to_owned(),
                    what,
                    names: undeclared,
                });
            }
            if what == "added" {
                added.extend(names);
            } else {
                removed.extend(names);
            }
        }
        added.sort();
        removed.sort();
        if self.pass_checks {
            self.check_entity_effects(w, p, text, log, &held_before, &effects)?;
            self.verify(w, text)?;
        }
        Ok(StepReport {
            pass: text.to_owned(),
            approximates: p.approximates(),
            log: std::mem::take(log),
            added,
            removed,
            conservation,
            failed: None,
            skipped: Vec::new(),
            time: StepTime::default(),
            settings,
        })
    }

    /// The pass's effects entity by entity, against its declarations: a name it put on an entity that didn't hold it
    /// (other than by copying an entity that did, [`World::duplicate`] / [`World::copy_components`]) is in
    /// [`Pass::provides`]; a name it took from an entity that survives is in [`Pass::removes`] (or, for
    /// [`Pass::removes_by_log`], in its log); an entity it despawned while it held names needs
    /// [`Pass::despawns_entities`]; a resource it replaced or borrowed to write is in [`Pass::modifies`] or
    /// [`Pass::provides`]. Values a pass edits in place on entities that keep the name (`get_mut`) are not seen: that is
    /// declared in [`Pass::modifies`] and not checked (a controlled editing interface would be needed to see it).
    fn check_entity_effects(
        &self,
        w: &World,
        p: &dyn Pass,
        text: &str,
        log: &PassLog,
        held_before: &BTreeMap<&str, BTreeSet<Entity>>,
        effects: &crate::world::Effects,
    ) -> Result<()> {
        let undeclared = |what: &'static str, names: Vec<String>| -> Result<()> {
            if names.is_empty() {
                Ok(())
            } else {
                Err(Error::Undeclared {
                    pass: text.to_owned(),
                    what,
                    names,
                })
            }
        };
        let some = |n: &str, es: &[Entity]| format!("{n} ({} entities, the first {:?})", es.len(), es[0]);
        let copied = |e: Entity, name: &str| {
            effects.copies.iter().any(|&(from, to)| {
                to == e
                    && (held_before.get(name).is_some_and(|h| h.contains(&from))
                        || w.entity_has_named(from, name))
            })
        };
        // added to entities
        let mut added = Vec::new();
        for name in w.component_names() {
            if p.provides().contains(&name) {
                continue;
            }
            let before = held_before.get(name);
            let new: Vec<Entity> = w
                .entities_named(name)
                .into_iter()
                .filter(|e| !before.is_some_and(|b| b.contains(e)) && !copied(*e, name))
                .collect();
            if !new.is_empty() {
                added.push(some(name, &new));
            }
        }
        undeclared("added to entities", added)?;
        // taken from entities that survive; entities despawned while they held names
        let (mut taken, mut despawned) = (Vec::new(), BTreeSet::new());
        for (&name, before) in held_before {
            let declared = p.removes().contains(&name) || (p.removes_by_log() && log.removed.contains(name));
            let gone: Vec<Entity> = before
                .iter()
                .copied()
                .filter(|&e| !w.entity_has_named(e, name))
                .collect();
            let (mut alive, dead): (Vec<Entity>, Vec<Entity>) =
                gone.into_iter().partition(|&e| w.is_alive(e));
            // the world's own drops, counted in the log ([`count_dropped`])
            let world_dropped = dropped_from(&effects.dropped, name);
            alive.retain(|e| !world_dropped.contains(e));
            if !declared && !alive.is_empty() {
                taken.push(some(name, &alive));
            }
            if !p.despawns_entities() {
                despawned.extend(dead);
            }
        }
        undeclared("taken from entities", taken)?;
        if let Some(first) = despawned.first() {
            return Err(Error::Pass {
                pass: text.to_owned(),
                message: format!(
                    "despawned {} entities (the first {first:?}) without declaring it (Pass::despawns_entities)",
                    despawned.len()
                ),
            });
        }
        // a resource taken away and held again afterwards was replaced: a write
        let names = w.resource_names();
        let replaced = effects
            .resources_removed
            .iter()
            .filter(|r| names.iter().any(|n| n == *r));
        let written: Vec<String> = effects
            .resources_written
            .iter()
            .chain(replaced)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|r| !p.modifies().contains(&r.as_str()) && !p.provides().contains(&r.as_str()))
            .cloned()
            .collect();
        undeclared("written (resources)", written)
    }

    /// Run the stated `pipeline` and check the world it leaves against `target`: verify the reader's world, run the
    /// pipeline as written (verifying after every pass with the pass checks), then check that the target accepts every
    /// component and resource in `mode`, and verify once more before the writer. A name the target still refuses is
    /// [`Error::Illegal`], naming the passes of the pipeline that remove or rewrite it, or none: the manager never adds a
    /// pass to lower it.
    pub fn convert(
        &self,
        w: &mut World,
        pipeline: &Pipeline,
        target: &Target,
        mode: Mode,
    ) -> Result<RunReport> {
        self.convert_instrumented(w, pipeline, target, mode, &mut NoInstrument)
    }

    /// [`PassManager::convert`], calling `instrument` around every pass of the pipeline.
    pub fn convert_instrumented(
        &self,
        w: &mut World,
        pipeline: &Pipeline,
        target: &Target,
        mode: Mode,
        instrument: &mut dyn Instrument,
    ) -> Result<RunReport> {
        self.verify(w, "the reader")?;
        let mut report = RunReport {
            target: target.name.clone(),
            pipeline: pipeline.to_string(),
            steps: Vec::new(),
        };
        self.run_instrumented(w, pipeline, &mut report, instrument)?;
        let refused = w
            .component_names()
            .into_iter()
            .map(|name| (name.to_owned(), target.illegal_entities(w, name, mode)))
            .find(|(_, bad)| !bad.is_empty())
            .or_else(|| {
                target
                    .refused_resources(w)
                    .first()
                    .map(|n| ((*n).to_owned(), Vec::new()))
            });
        if let Some((name, bad)) = refused {
            let lowered_by = pipeline
                .passes
                .iter()
                .filter(|p| p.removes().contains(&name.as_str()) || p.modifies().contains(&name.as_str()))
                .map(|p| pass_text(p.as_ref()))
                .collect();
            return Err(Error::Illegal {
                target: target.name.clone(),
                rule: target.rule_words(&name).unwrap_or_default().to_owned(),
                name,
                entities: bad.into_iter().take(10).collect(),
                lowered_by,
            });
        }
        self.verify(w, &format!("the pipeline, before {}", target.name))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Opaque, OpaqueData};

    /// The text form carries a value with commas between non-blank parts that hold no '=' (`-64,-64,64,64`), and not one
    /// whose part after a comma is blank (a trailing ','), holds '=', or has a brace.
    #[test]
    fn what_the_text_form_carries() {
        for carried in ["", "512", "-64,-64,64,64", "3:2,4:1", "a:x;b:y", "k=1:v"] {
            assert!(carries(carried), "{carried:?}");
        }
        for lost in ["a:x,", "a:1,;b=c:2", "a:1,b=2", "x{y}", " a", "a ", "a, b"] {
            assert!(!carries(lost), "{lost:?}");
        }
    }

    #[test]
    fn option_values_with_commas_parse_as_display_writes_them() {
        let parsed = parse_pipeline("a{rect=-64,-64,64,64,depth=3:2,4:1},b{by=1,2,3},c").unwrap();
        let opts = |pairs: &[(&str, &str)]| -> Options {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert_eq!(
            parsed,
            vec![
                (
                    "a".to_owned(),
                    opts(&[("rect", "-64,-64,64,64"), ("depth", "3:2,4:1")])
                ),
                ("b".to_owned(), opts(&[("by", "1,2,3")])),
                ("c".to_owned(), Options::new()),
            ]
        );
        assert!(parse_pipeline("a{-64,x=1}").is_err());
    }

    // toy components
    #[derive(Clone, Debug, PartialEq)]
    struct File(u32);
    impl Component for File {
        const NAME: &'static str = "toy.file";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Mesh(u32);
    impl Component for Mesh {
        const NAME: &'static str = "toy.mesh";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
        fn verify(&self, _e: Entity, _w: &World) -> std::result::Result<(), String> {
            if self.0 == 13 {
                Err("unlucky mesh".into())
            } else {
                Ok(())
            }
        }
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Layered(u32);
    impl Component for Layered {
        const NAME: &'static str = "toy.layered";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Pbr(u32);
    impl Component for Pbr {
        const NAME: &'static str = "toy.pbr";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }

    /// toy.file -> toy.mesh (needs nothing).
    struct Load;
    impl Pass for Load {
        fn name(&self) -> &'static str {
            "load"
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.mesh"]
        }
        fn removes(&self) -> &[&'static str] {
            &["toy.file"]
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.file"]
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            let files: Vec<(Entity, u32)> = w.iter::<File>().map(|(e, f)| (e, f.0)).collect();
            for (e, n) in files {
                w.remove::<File>(e);
                w.insert(e, Mesh(n));
                log.count("loaded", 1);
            }
            Ok(())
        }
    }
    const LOAD: PassSpec = PassSpec {
        name: "load",
        options: &[],
        make: |_| Ok(Box::new(Load)),
    };

    /// toy.layered -> toy.pbr, needs the mesh (bakes per vertex), option `size`.
    struct Bake {
        size: u32,
    }
    impl Pass for Bake {
        fn name(&self) -> &'static str {
            "bake"
        }
        fn options(&self) -> Options {
            if self.size == 1024 {
                Options::new()
            } else {
                Options::from([("size".to_owned(), self.size.to_string())])
            }
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.mesh", "toy.layered"]
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.pbr"]
        }
        fn removes(&self) -> &[&'static str] {
            &["toy.layered"]
        }
        fn approximates(&self) -> &'static str {
            "layer blend baked to one texture"
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            let ls: Vec<(Entity, u32)> = w.iter::<Layered>().map(|(e, l)| (e, l.0)).collect();
            for (e, n) in ls {
                w.remove::<Layered>(e);
                w.insert(e, Pbr(n));
                log.approximate("baked", 1);
            }
            Ok(())
        }
    }
    const BAKE: PassSpec = PassSpec {
        name: "bake",
        options: &[OptionSpec::optional(
            "size",
            "512",
            "texels: the baked texture's size",
            "1024",
        )],
        make: |g| {
            let size = match g.optional("size") {
                Some(v) => v.parse().map_err(|_| format!("size {v:?} is not a number"))?,
                None => 1024,
            };
            Ok(Box::new(Bake { size }))
        },
    };

    /// Adds a name it does not declare.
    struct Sneaky;
    impl Pass for Sneaky {
        fn name(&self) -> &'static str {
            "sneaky"
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            let e = w.spawn();
            w.insert(e, Pbr(1));
            Ok(())
        }
    }
    const SNEAKY: PassSpec = PassSpec {
        name: "sneaky",
        options: &[],
        make: |_| Ok(Box::new(Sneaky)),
    };

    fn registry() -> PassRegistry {
        let mut r = PassRegistry::new();
        r.add(&LOAD);
        r.add(&BAKE);
        r.add(&SNEAKY);
        r
    }

    fn world() -> World {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, File(1));
        w.insert(a, Layered(5));
        w.insert_opaque(
            a,
            "game.thing",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![]),
            },
        )
        .unwrap();
        w
    }

    fn gltf() -> Target {
        Target::new("gltf")
            .legal("toy.mesh")
            .legal("toy.pbr")
            .illegal("toy.file")
            .illegal("toy.layered")
    }

    /// Sleeps for its milliseconds in its run, doing nothing else.
    struct Slow(u64);
    impl Pass for Slow {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            std::thread::sleep(Duration::from_millis(self.0));
            Ok(())
        }
    }

    /// A pipeline's unfed passes are those a run would refuse, each with the names it lacks, checked with what the
    /// passes before it provide; what it holds after counts only the passes that can run.
    #[test]
    fn unfed_passes_are_named_with_what_they_lack_and_the_names_held_after() {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<String>>();
        let load_then_bake = || Pipeline {
            passes: vec![Box::new(Load), Box::new(Bake { size: 1024 })],
        };
        // load provides the mesh bake needs; a removed name is taken as kept
        assert_eq!(
            load_then_bake().unfed(&names(&["toy.file", "toy.layered"])),
            Unfed {
                passes: Vec::new(),
                held: names(&["toy.file", "toy.layered", "toy.mesh", "toy.pbr"]),
            }
        );
        // no file: load is refused, and bake, after it, lacks the mesh load would have provided; neither adds a name
        assert_eq!(
            load_then_bake().unfed(&names(&["toy.layered"])),
            Unfed {
                passes: vec![
                    ("load".to_owned(), vec!["toy.file"]),
                    ("bake".to_owned(), vec!["toy.mesh"])
                ],
                held: names(&["toy.layered"]),
            }
        );
        // a pass that may lack its inputs is skipped when it finds none, providing nothing, and refused when it finds
        // some
        struct Either;
        impl Pass for Either {
            fn name(&self) -> &'static str {
                "either"
            }
            fn requires(&self) -> &[&'static str] {
                &["toy.mesh", "toy.pbr"]
            }
            fn provides(&self) -> &[&'static str] {
                &["toy.lit"]
            }
            fn may_lack_inputs(&self) -> bool {
                true
            }
            fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
                Ok(())
            }
        }
        let either = || Pipeline {
            passes: vec![Box::new(Either)],
        };
        assert_eq!(
            either().unfed(&names(&[])),
            Unfed {
                passes: Vec::new(),
                held: names(&[]),
            }
        );
        assert_eq!(
            either().unfed(&names(&["toy.mesh"])).passes,
            [("either".to_owned(), vec!["toy.pbr"])]
        );
        assert_eq!(
            either().unfed(&names(&["toy.mesh", "toy.pbr"])),
            Unfed {
                passes: Vec::new(),
                held: names(&["toy.lit", "toy.mesh", "toy.pbr"]),
            }
        );
    }

    #[test]
    fn optional_preparation_reports_missing_inputs_and_uses_actual_preceding_outputs() {
        struct NeedsBoth;
        impl Pass for NeedsBoth {
            fn name(&self) -> &'static str {
                "needs-both"
            }
            fn requires(&self) -> &[&'static str] {
                &["toy.mesh", "toy.pbr"]
            }
            fn run(&self, _: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
                log.count("ran with both inputs", 1);
                Ok(())
            }
        }
        let pipeline = || Pipeline {
            passes: vec![
                Box::new(NeedsBoth),
                Box::new(Load),
                Box::new(NeedsBoth),
                Box::new(Bake { size: 1024 }),
                Box::new(NeedsBoth),
            ],
        };
        let mut input = world();
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_missing_inputs(MissingInputs::Skip)
            .run(&mut input, &pipeline(), &mut report)
            .unwrap();
        assert_eq!(report.steps.len(), 5);
        assert_eq!(report.steps[0].skipped, ["toy.mesh", "toy.pbr"]);
        assert_eq!(report.steps[2].skipped, ["toy.pbr"]);
        assert_eq!(report.steps[2].log.counts[SKIPPED_UNFED], 1);
        assert!(report.steps[0].added.is_empty() && report.steps[2].added.is_empty());
        assert!(report.steps[4].skipped.is_empty());
        assert_eq!(report.steps[4].log.counts["ran with both inputs"], 1);
        assert!(input.has_component_named("toy.pbr"));
        let mut strict_report = RunReport::default();
        assert!(matches!(
            PassManager::new(PassRegistry::new()).run(&mut world(), &pipeline(), &mut strict_report),
            Err(Error::Missing { .. })
        ));
        assert!(strict_report.steps[0].skipped.is_empty());
    }

    #[test]
    fn optional_preparation_does_not_hide_a_fed_pass_failure() {
        struct Fails;
        impl Pass for Fails {
            fn name(&self) -> &'static str {
                "fails"
            }
            fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
                Err("source data is malformed".into())
            }
        }
        let mut report = RunReport::default();
        let result = PassManager::new(PassRegistry::new())
            .with_missing_inputs(MissingInputs::Skip)
            .run(
                &mut world(),
                &Pipeline {
                    passes: vec![Box::new(Fails)],
                },
                &mut report,
            );
        assert!(result.is_err());
        assert!(
            report.steps[0]
                .failed
                .as_ref()
                .unwrap()
                .contains("source data is malformed")
        );
        assert!(report.steps[0].skipped.is_empty());
    }

    /// Each step's time is measured around its own run (a 30 ms run is at least 30 ms, the manager's checks apart);
    /// the report's time adds the steps up and its text shows each step's; a step a part skips takes none.
    #[test]
    fn steps_are_timed_around_their_runs() {
        let pipeline = Pipeline {
            passes: vec![Box::new(Slow(30)), Box::new(Load)],
        };
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_pass_checks(true)
            .run(&mut world(), &pipeline, &mut report)
            .unwrap();
        let [slow, load] = [&report.steps[0].time, &report.steps[1].time];
        assert!(slow.run >= Duration::from_millis(30), "{slow:?}");
        assert!(load.run < slow.run, "{load:?} {slow:?}");
        assert_eq!(report.time().run, slow.run + load.run);
        assert_eq!(report.time().checks, slow.checks + load.checks);
        let text = report.to_string();
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with("slow:"))
            .unwrap();
        assert!(
            line.ends_with(&format!(
                "({:.3} s, checks {:.3} s)",
                slow.run.as_secs_f64(),
                slow.checks.as_secs_f64()
            )),
            "{line}"
        );

        struct Skippable;
        impl Pass for Skippable {
            fn name(&self) -> &'static str {
                "skippable"
            }
            fn requires(&self) -> &[&'static str] {
                &["toy.pbr"]
            }
            fn may_lack_inputs(&self) -> bool {
                true
            }
            fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
                Ok(())
            }
        }
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .run_part(
                &mut world(),
                &Pipeline {
                    passes: vec![Box::new(Skippable)],
                },
                &mut report,
            )
            .unwrap();
        assert_eq!(report.steps[0].time, StepTime::default());
        assert!(!report.to_string().contains(" s, checks "));
        // reports compare without their times (the determinism comparison); times compare as themselves
        let mut later = report.clone();
        later.steps[0].time.run = Duration::from_secs(1);
        assert_eq!(later, report);
        assert_ne!(later.steps[0].time, report.steps[0].time);
        later.steps[0].pass = "other".into();
        assert_ne!(later, report);
    }

    /// A target's rules read back by kind, its checks left out; its checked resources too.
    #[test]
    fn a_targets_rules_read_back() {
        let t = Target::new("t").legal("toy.pbr").illegal("toy.layered");
        assert_eq!(
            t.rules(),
            [("toy.layered", Rule::Illegal), ("toy.pbr", Rule::Legal)]
        );
        assert!(t.checked_resources().is_empty());
    }

    /// A pass taking in a function's log keeps everything it reported: counts, approximations, omissions and drops
    /// add up, removals, placed entities and their sources join.
    #[test]
    fn absorb_keeps_every_part_of_the_log() {
        let mut w = World::new();
        let (a, b, source) = (w.spawn(), w.spawn(), w.spawn());
        let mut log = PassLog::default();
        log.count("kept", 1);
        log.approximate("rounded", 2);
        log.place(a);
        log.detail("explanation", serde_json::json!({"input": 1, "why": "first"}));
        let mut other = PassLog::default();
        other.count("kept", 3);
        other.count("made", 1);
        other.approximate("rounded", 1);
        other.omit("no form", 4);
        other.record_removed("toy.hat");
        other.place_from(b, source);
        other.dropped(Measure::Placements, "left out", 2);
        other.detail("explanation", serde_json::json!({"input": 2, "why": "second"}));
        other.detail("additional", serde_json::json!(["nested", {"result": true}]));
        log.dropped(Measure::Placements, "left out", 1);
        log.absorb(other);
        assert_eq!(
            log.counts,
            BTreeMap::from([("kept".to_owned(), 4), ("made".to_owned(), 1)])
        );
        assert_eq!(log.approximations, BTreeMap::from([("rounded".to_owned(), 3)]));
        assert_eq!(log.omitted, BTreeMap::from([("no form".to_owned(), 4)]));
        assert_eq!(log.removed, BTreeSet::from(["toy.hat".to_owned()]));
        assert_eq!(log.placed, BTreeSet::from([a, b]));
        assert_eq!(log.placed_from, BTreeMap::from([(b, source)]));
        assert_eq!(log.declared(Measure::Placements), 3);
        assert_eq!(
            log.details["explanation"],
            [
                serde_json::json!({"input": 1, "why": "first"}),
                serde_json::json!({"input": 2, "why": "second"}),
            ]
        );
        assert_eq!(
            log.details["additional"],
            [serde_json::json!(["nested", {"result": true}])]
        );
    }

    #[test]
    fn pipeline_text_round_trips() {
        let r = registry();
        let p = r.parse(" load , bake{ size = 512 } ,bake").unwrap();
        assert_eq!(p.to_string(), "load, bake{size=512}, bake");
        assert_eq!(r.parse(&p.to_string()).unwrap().to_string(), p.to_string());
        assert_eq!(r.parse("").unwrap().passes.len(), 0);
        assert!(matches!(r.parse("load,,bake"), Err(Error::Parse(_))));
        assert!(matches!(r.parse("bake{size=512"), Err(Error::Parse(_))));
        assert!(matches!(r.parse("nope"), Err(Error::UnknownPass(n)) if n == "nope"));
        assert!(matches!(r.parse("bake{size=big}"), Err(Error::Options { .. })));
        assert!(matches!(r.parse("bake{colour=red}"), Err(Error::Options { .. })));
    }

    /// A conversion runs the stated pipeline as written and checks what it leaves; the report names the target and the
    /// pipeline. Partial keeps a name the target does not list.
    #[test]
    fn a_conversion_runs_the_stated_pipeline() {
        let m = PassManager::new(registry());
        let mut w = world();
        let pipeline = m.registry.parse("load, bake").unwrap();
        let report = m.convert(&mut w, &pipeline, &gltf(), Mode::Partial).unwrap();
        assert_eq!(report.target, "gltf");
        assert_eq!(report.pipeline, "load, bake");
        assert_eq!(
            report.steps[1].log.approximations,
            BTreeMap::from([("baked".to_owned(), 1)])
        );
        assert_eq!(report.steps[1].approximates, "layer blend baked to one texture");
        assert_eq!(
            w.component_names(),
            ["game.thing", "toy.mesh", "toy.pbr"],
            "partial keeps game.thing"
        );
        assert!(
            report
                .to_string()
                .starts_with("target gltf: pipeline [load, bake]\n")
        );
    }

    /// What the target still refuses after the stated pipeline is an error that says nothing in the pipeline lowers it:
    /// the manager adds no pass.
    #[test]
    fn a_refused_name_says_the_stated_pipeline_does_not_lower_it() {
        let m = PassManager::new(registry());
        let pipeline = m.registry.parse("load, bake").unwrap();
        // Full refuses a name the target does not list
        let err = m
            .convert(&mut world(), &pipeline, &gltf(), Mode::Full)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`gltf` does not accept `game.thing` on [e0] after the pipeline: nothing in the stated pipeline removes or \
             rewrites it"
        );
        // stating what it accepts makes it pass
        let accepting = gltf().legal("game.thing");
        m.convert(&mut world(), &pipeline, &accepting, Mode::Full)
            .unwrap();
        // a pipeline that leaves an illegal name
        let mut w = world();
        let err = m
            .convert(&mut w, &m.registry.parse("load").unwrap(), &gltf(), Mode::Partial)
            .unwrap_err();
        assert!(
            matches!(&err, Error::Illegal { name, lowered_by, .. } if name == "toy.layered" && lowered_by.is_empty()),
            "{err}"
        );
        assert!(
            w.has_component_named("toy.layered"),
            "nothing was added to lower it"
        );
    }

    #[test]
    fn dynamic_legality_refuses_only_failing_values() {
        let m = PassManager::new(registry());
        let t = Target::new("skate")
            .legal("toy.mesh")
            .legal("toy.layered")
            .legal("game.thing")
            .dynamic_typed::<File>(|f| f.0 < 10);
        let none = Pipeline::default();
        let mut w = world(); // File(1): legal as it is
        let report = m.convert(&mut w, &none, &t, Mode::Full).unwrap();
        assert_eq!(report.pipeline, "");
        assert!(w.has_component_named("toy.file"));
        let failing = || {
            let mut w = world();
            let e = w.entities().next().unwrap();
            w.insert(e, File(99)); // fails the check
            w
        };
        let err = m.convert(&mut failing(), &none, &t, Mode::Full).unwrap_err();
        assert!(
            matches!(&err, Error::Illegal { name, .. } if name == "toy.file"),
            "{err}"
        );
        // a stated pass that lowers it
        let load = m.registry.parse("load").unwrap();
        let report = m.convert(&mut failing(), &load, &t, Mode::Full).unwrap();
        assert_eq!(report.pipeline, "load");
    }

    #[test]
    fn undeclared_changes_and_verifier_name_the_pass() {
        let m = PassManager::new(registry()).with_pass_checks(true);
        let mut w = world();
        let p = m.registry.parse("sneaky").unwrap();
        let err = m.run(&mut w, &p, &mut RunReport::default()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass sneaky added `[\"toy.pbr\"]` without declaring it"
        );

        let mut w = world();
        let e = w.entities().next().unwrap();
        w.insert(e, File(13)); // loads into the unlucky mesh
        let p = m.registry.parse("load").unwrap();
        let err = m.run(&mut w, &p, &mut RunReport::default()).unwrap_err();
        assert_eq!(err.to_string(), "verify after load: toy.mesh on e0: unlucky mesh");

        let p = m.registry.parse("bake").unwrap();
        let err = m.run(&mut world(), &p, &mut RunReport::default()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass bake requires `toy.mesh`, which the world does not have"
        );
    }

    /// Despawns entities holding toy.layered; with `strip`, also takes toy.file off the entities it keeps.
    struct Cull {
        opted: bool,
        strip: bool,
    }
    impl Pass for Cull {
        fn name(&self) -> &'static str {
            "cull"
        }
        fn despawns_entities(&self) -> bool {
            self.opted
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            for e in w.iter::<Layered>().map(|(e, _)| e).collect::<Vec<_>>() {
                w.despawn(e);
                log.count("culled", 1);
            }
            if self.strip {
                for e in w.iter::<File>().map(|(e, _)| e).collect::<Vec<_>>() {
                    w.remove::<File>(e);
                }
            }
            Ok(())
        }
    }

    /// Takes toy.layered off every entity without declaring it; `opted` says it removes by log, `record` that it
    /// records the removal there.
    struct LogStrip {
        opted: bool,
        record: bool,
    }
    impl Pass for LogStrip {
        fn name(&self) -> &'static str {
            "log-strip"
        }
        fn removes_by_log(&self) -> bool {
            self.opted
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            for e in w.iter::<Layered>().map(|(e, _)| e).collect::<Vec<_>>() {
                w.remove::<Layered>(e);
            }
            if self.record {
                log.record_removed("toy.layered");
                // recorded but still present elsewhere, or never present: fine
                log.record_removed("toy.file");
            }
            Ok(())
        }
    }

    #[test]
    fn a_pass_removing_by_log_may_take_the_names_it_recorded() {
        let run = |opted, record| {
            let pipeline = Pipeline {
                passes: vec![Box::new(LogStrip { opted, record })],
            };
            PassManager::new(PassRegistry::new()).run(&mut world(), &pipeline, &mut RunReport::default())
        };
        let undeclared = "pass log-strip removed `[\"toy.layered\"]` without declaring it";
        assert!(run(true, true).is_ok());
        // vanished unrecorded, or recorded by a pass that did not opt in: an error as ever
        assert_eq!(run(true, false).unwrap_err().to_string(), undeclared);
        assert_eq!(run(false, true).unwrap_err().to_string(), undeclared);
    }

    /// Places what `which` says: a new entity, the world's first (not its own) or a new one it despawns again.
    struct Placer {
        which: &'static str,
    }
    impl Pass for Placer {
        fn name(&self) -> &'static str {
            "placer"
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            let e = match self.which {
                "old" => w.entities().next().expect("an entity"),
                _ => w.spawn(),
            };
            if self.which == "gone" {
                w.despawn(e);
            }
            log.place(e);
            // an intermediate: something placed from it, then it is consumed
            if self.which == "consumed" {
                let derived = w.spawn();
                log.place_from(derived, e);
                w.despawn(e);
            }
            Ok(())
        }
    }

    #[test]
    fn a_pass_places_only_what_it_made_and_what_is_alive() {
        let run = |which| {
            let pipeline = Pipeline {
                passes: vec![Box::new(Placer { which })],
            };
            let mut report = RunReport::default();
            PassManager::new(PassRegistry::new())
                .run(&mut world(), &pipeline, &mut report)
                .map(|()| report.steps[0].log.placed.len())
                .map_err(|e| e.to_string())
        };
        assert_eq!(run("new"), Ok(1));
        assert!(run("old").unwrap_err().contains("which it did not make"));
        assert!(run("gone").unwrap_err().contains("which is not alive after it"));
        // a consumed intermediate with output placed from it passes; what came from it is placed
        assert_eq!(run("consumed"), Ok(2));
    }

    /// Attributes an approximation to what `which` says: a live entity, or one it despawns again.
    struct Attributer {
        which: &'static str,
    }
    impl Pass for Attributer {
        fn name(&self) -> &'static str {
            "attributer"
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            let e = w.spawn();
            if self.which == "gone" {
                w.despawn(e);
            }
            log.approximate_on(e, "base albedo could not be read", 1);
            Ok(())
        }
    }

    #[test]
    fn an_approximation_is_attributed_to_a_live_entity() {
        let run = |which| {
            let pipeline = Pipeline {
                passes: vec![Box::new(Attributer { which })],
            };
            let mut report = RunReport::default();
            PassManager::new(PassRegistry::new())
                .run(&mut world(), &pipeline, &mut report)
                .map(|()| report.steps[0].log.clone())
                .map_err(|e| e.to_string())
        };
        let log = run("live").expect("a live entity is accepted");
        assert_eq!(log.approximations["base albedo could not be read"], 1);
        assert_eq!(log.approximated_on["base albedo could not be read"].len(), 1);
        assert!(run("gone").unwrap_err().contains("attributed to"));
    }

    /// The entities an absorbed log attributed approximations to join, and an unattributed approximation names none.
    #[test]
    fn absorb_keeps_the_entities_an_approximation_was_attributed_to() {
        let mut w = World::new();
        let (a, b) = (w.spawn(), w.spawn());
        let mut log = PassLog::default();
        log.approximate_on(a, "unread", 1);
        log.approximate("rounded", 2);
        let mut other = PassLog::default();
        other.approximate_on(b, "unread", 3);
        log.absorb(other);
        assert_eq!(
            log.approximations,
            BTreeMap::from([("rounded".to_owned(), 2), ("unread".to_owned(), 4)])
        );
        assert_eq!(
            log.approximated_on,
            BTreeMap::from([("unread".to_owned(), BTreeSet::from([a, b]))])
        );
    }

    /// One entity with toy.layered, toy.file and game.thing (culled), one with toy.mesh and toy.file (kept).
    fn cull_world() -> World {
        let mut w = world();
        let kept = w.spawn();
        w.insert(kept, Mesh(2));
        w.insert(kept, File(2));
        w
    }

    fn cull(opted: bool, strip: bool, w: &mut World) -> Result<RunReport> {
        let mut report = RunReport::default();
        let pipeline = Pipeline {
            passes: vec![Box::new(Cull { opted, strip })],
        };
        PassManager::new(PassRegistry::new()).run(w, &pipeline, &mut report)?;
        Ok(report)
    }

    #[test]
    fn a_despawning_pass_may_take_names_only_despawned_entities_held() {
        // undeclared: a name gone with its entity is still an undeclared removal
        let err = cull(false, false, &mut cull_world()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass cull removed `[\"game.thing\", \"toy.layered\"]` without declaring it"
        );
        // declared: every holder despawned, so the names may go; the report keeps all of them
        let mut w = cull_world();
        let report = cull(true, false, &mut w).unwrap();
        assert_eq!(report.steps[0].removed, ["game.thing", "toy.layered"]);
        assert_eq!(w.component_names(), ["toy.file", "toy.mesh"]);
        // a surviving entity that lost a name is not covered by despawning
        let err = cull(true, true, &mut cull_world()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass cull removed `[\"toy.file\"]` without declaring it"
        );
        // names it adds are checked as for any pass
        struct Adds;
        impl Pass for Adds {
            fn name(&self) -> &'static str {
                "adds"
            }
            fn despawns_entities(&self) -> bool {
                true
            }
            fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
                let e = w.spawn();
                w.insert(e, Pbr(1));
                Ok(())
            }
        }
        let pipeline = Pipeline {
            passes: vec![Box::new(Adds)],
        };
        let err = PassManager::new(PassRegistry::new())
            .run(&mut world(), &pipeline, &mut RunReport::default())
            .unwrap_err();
        assert!(matches!(err, Error::Undeclared { what: "added", .. }), "{err}");
    }

    #[derive(Clone, Debug)]
    struct Env;
    impl crate::world::Resource for Env {
        const NAME: &'static str = "toy.env";
    }

    /// Inserts `toy.env` (`add`) or takes it away, declaring it or not.
    struct EnvPass {
        add: bool,
        declared: bool,
        despawns: bool,
    }
    impl Pass for EnvPass {
        fn name(&self) -> &'static str {
            "env"
        }
        fn provides(&self) -> &[&'static str] {
            if self.add && self.declared {
                &["toy.env"]
            } else {
                &[]
            }
        }
        fn removes(&self) -> &[&'static str] {
            if !self.add && self.declared {
                &["toy.env"]
            } else {
                &[]
            }
        }
        fn despawns_entities(&self) -> bool {
            self.despawns
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            if self.add {
                w.insert_resource(Env);
            } else {
                w.remove_resource::<Env>();
            }
            Ok(())
        }
    }

    fn env_run(pass: EnvPass, w: &mut World) -> Result<RunReport> {
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new()).with_pass_checks(true).run(
            w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )?;
        Ok(report)
    }

    #[test]
    fn resources_are_checked_by_the_same_declarations() {
        // inserted without declaring it: refused by name
        let err = env_run(
            EnvPass {
                add: true,
                declared: false,
                despawns: false,
            },
            &mut world(),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass env added `[\"toy.env\"]` without declaring it"
        );
        // declared: reported with the component names
        let mut w = world();
        let report = env_run(
            EnvPass {
                add: true,
                declared: true,
                despawns: false,
            },
            &mut w,
        )
        .unwrap();
        assert_eq!(report.steps[0].added, ["toy.env"]);
        // replacing one that is there is a write: refused undeclared, accepted declared (no change of names)
        let err = env_run(
            EnvPass {
                add: true,
                declared: false,
                despawns: false,
            },
            &mut w,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass env written (resources) `[\"toy.env\"]` without declaring it"
        );
        let report = env_run(
            EnvPass {
                add: true,
                declared: true,
                despawns: false,
            },
            &mut w,
        )
        .unwrap();
        assert!(report.steps[0].added.is_empty());
        // taken away undeclared: refused, also by a pass that despawns entities (no entity held it)
        let err = env_run(
            EnvPass {
                add: false,
                declared: false,
                despawns: true,
            },
            &mut w,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "pass env removed `[\"toy.env\"]` without declaring it"
        );
        // a refused pass has still run: the world is as it left it
        w.insert_resource(Env);
        let report = env_run(
            EnvPass {
                add: false,
                declared: true,
                despawns: false,
            },
            &mut w,
        )
        .unwrap();
        assert_eq!(report.steps[0].removed, ["toy.env"]);
        // a pass may require a resource
        struct NeedsEnv;
        impl Pass for NeedsEnv {
            fn name(&self) -> &'static str {
                "needs-env"
            }
            fn requires(&self) -> &[&'static str] {
                &["toy.env"]
            }
            fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
                Ok(())
            }
        }
        let needs = || Pipeline {
            passes: vec![Box::new(NeedsEnv)],
        };
        let m = PassManager::new(PassRegistry::new());
        assert!(matches!(
            m.run(&mut world(), &needs(), &mut RunReport::default()),
            Err(Error::Missing { .. })
        ));
        let mut w = world();
        w.insert_resource(Env);
        m.run(&mut w, &needs(), &mut RunReport::default()).unwrap();
    }

    /// Makes odd meshes even in place.
    struct Evens;
    impl Pass for Evens {
        fn name(&self) -> &'static str {
            "evens"
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.mesh"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["toy.mesh"]
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            if let Some(mut c) = w.column_mut::<Mesh>() {
                for (_, m) in c.iter_mut() {
                    m.0 += m.0 % 2;
                }
            }
            Ok(())
        }
    }
    const EVENS: PassSpec = PassSpec {
        name: "evens",
        options: &[],
        make: |_| Ok(Box::new(Evens)),
    };

    fn mesh_world(n: u32) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh(n));
        w
    }

    #[test]
    fn a_stated_pass_that_rewrites_in_place_fixes_failing_values() {
        let even = Target::new("even").dynamic_typed::<Mesh>(|m| m.0 % 2 == 0);
        let mut r = PassRegistry::new();
        r.add(&EVENS);
        let m = PassManager::new(r);
        let evens = m.registry.parse("evens").unwrap();
        let mut w = mesh_world(3);
        let report = m.convert(&mut w, &evens, &even, Mode::Partial).unwrap();
        assert_eq!(report.pipeline, "evens");
        assert_eq!(w.iter::<Mesh>().map(|(_, m)| m.0).collect::<Vec<_>>(), [4]);
        // legal values need nothing
        m.convert(&mut mesh_world(2), &Pipeline::default(), &even, Mode::Partial)
            .unwrap();
        // without the pass the failing value stays, refused
        let err = m
            .convert(&mut mesh_world(3), &Pipeline::default(), &even, Mode::Partial)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`even` does not accept `toy.mesh` on [e0] after the pipeline: nothing in the stated pipeline removes or \
             rewrites it"
        );
    }

    /// Removes toy.layered only when made with `lowers=true`: its declarations follow its options.
    struct Optional {
        lowers: bool,
    }
    impl Pass for Optional {
        fn name(&self) -> &'static str {
            "optional"
        }
        fn options(&self) -> Options {
            if self.lowers {
                Options::from([("lowers".to_owned(), "true".to_owned())])
            } else {
                Options::new()
            }
        }
        fn removes(&self) -> &[&'static str] {
            if self.lowers { &["toy.layered"] } else { &[] }
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            if !self.lowers {
                return Ok(());
            }
            let es: Vec<Entity> = w.iter::<Layered>().map(|(e, _)| e).collect();
            for e in es {
                w.remove::<Layered>(e);
            }
            Ok(())
        }
    }

    /// A stated pass is made with the options the pipeline gives it, and its declarations come from that instance.
    #[test]
    fn a_stated_pass_declares_by_its_options() {
        let mut r = PassRegistry::new();
        const OPTIONAL: PassSpec = PassSpec {
            name: "optional",
            options: &[OptionSpec::optional(
                "lowers",
                "true",
                "true: it removes toy.layered",
                "it removes nothing",
            )],
            make: |g| {
                Ok(Box::new(Optional {
                    lowers: g.optional("lowers").is_some_and(|v| v == "true"),
                }))
            },
        };
        r.add(&OPTIONAL);
        let m = PassManager::new(r);
        let t = Target::new("t").illegal("toy.layered");
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Layered(1));
        let err = m
            .convert(
                &mut w.clone(),
                &m.registry.parse("optional").unwrap(),
                &t,
                Mode::Full,
            )
            .unwrap_err();
        assert!(
            matches!(&err, Error::Illegal { lowered_by, .. } if lowered_by.is_empty()),
            "{err}"
        );
        let report = m
            .convert(
                &mut w,
                &m.registry.parse("optional{lowers=true}").unwrap(),
                &t,
                Mode::Full,
            )
            .unwrap();
        assert_eq!(report.pipeline, "optional{lowers=true}");
    }

    struct Decl {
        name: &'static str,
        requires: &'static [&'static str],
        modifies: &'static [&'static str],
    }
    impl Pass for Decl {
        fn name(&self) -> &'static str {
            self.name
        }
        fn requires(&self) -> &[&'static str] {
            self.requires
        }
        fn modifies(&self) -> &[&'static str] {
            self.modifies
        }
        fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn values_a_stated_pass_produces_are_checked_after_the_pipeline() {
        // load makes Mesh(3) from File(3); the target wants even meshes: evens, stated after load, fixes it
        let mut r = PassRegistry::new();
        r.add(&LOAD);
        r.add(&EVENS);
        let m = PassManager::new(r);
        let t = Target::new("even")
            .illegal("toy.file")
            .dynamic_typed::<Mesh>(|m| m.0 % 2 == 0);
        let mut w = w_with_file(3);
        let e = w.entities().next().unwrap();
        let report = m
            .convert(
                &mut w,
                &m.registry.parse("load, evens").unwrap(),
                &t,
                Mode::Partial,
            )
            .unwrap();
        assert_eq!(report.pipeline, "load, evens");
        assert_eq!(w.get::<Mesh>(e), Some(&Mesh(4)));
        // a produced value the check accepts needs nothing more
        let load = m.registry.parse("load").unwrap();
        m.convert(&mut w_with_file(2), &load, &t, Mode::Partial).unwrap();
        // a produced value the pipeline does not fix is refused after it
        let err = m
            .convert(&mut w_with_file(5), &load, &t, Mode::Partial)
            .unwrap_err();
        assert!(
            matches!(&err, Error::Illegal { name, lowered_by, .. } if name == "toy.mesh" && lowered_by.is_empty()),
            "{err}"
        );
    }

    fn w_with_file(n: u32) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, File(n));
        w
    }

    /// A table of rows a classifier fills in: `None` is a row not classified yet.
    #[derive(Clone, Debug, PartialEq)]
    struct Table(Vec<Option<u8>>);
    impl crate::world::Resource for Table {
        const NAME: &'static str = "toy.table";
    }

    /// Classifies every row of toy.table (in place) and settles it.
    struct Classify;
    impl Pass for Classify {
        fn name(&self) -> &'static str {
            "classify"
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.table"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["toy.table"]
        }
        fn settles(&self) -> &[&'static str] {
            &["toy.table"]
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            if let Some(t) = w.resource_mut::<Table>() {
                for row in t.0.iter_mut().filter(|r| r.is_none()) {
                    *row = Some(0);
                    log.count("rows classified", 1);
                }
            }
            Ok(())
        }
    }

    /// Adds an unclassified row to toy.table.
    struct AddRow;
    impl Pass for AddRow {
        fn name(&self) -> &'static str {
            "add-row"
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.table"]
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            match w.resource_mut::<Table>() {
                Some(t) => t.0.push(None),
                None => {
                    w.insert_resource(Table(vec![None]));
                }
            }
            Ok(())
        }
    }

    fn classified() -> Target {
        Target::new("t").check_resource::<Table>("every row classified", |t| t.0.iter().all(Option::is_some))
    }

    fn table_world(rows: Vec<Option<u8>>) -> World {
        let mut w = World::new();
        w.insert_resource(Table(rows));
        w
    }

    #[test]
    fn a_resource_value_rule_is_checked_after_the_pipeline() {
        let mut r = PassRegistry::new();
        const CLASSIFY: PassSpec = PassSpec {
            name: "classify",
            options: &[],
            make: |_| Ok(Box::new(Classify)),
        };
        r.add(&CLASSIFY);
        let m = PassManager::new(r);
        let mut w = table_world(vec![Some(3), None]);
        let classify = m.registry.parse("classify").unwrap();
        let report = m
            .convert(&mut w, &classify, &classified(), Mode::Partial)
            .unwrap();
        assert_eq!(report.pipeline, "classify");
        assert_eq!(w.resource::<Table>(), Some(&Table(vec![Some(3), Some(0)])));
        // a table the rule accepts needs nothing
        m.convert(
            &mut table_world(vec![Some(1)]),
            &Pipeline::default(),
            &classified(),
            Mode::Partial,
        )
        .unwrap();
        // nothing classifies: refused, naming the rule
        let err = m
            .convert(
                &mut table_world(vec![None]),
                &Pipeline::default(),
                &classified(),
                Mode::Partial,
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`t` does not accept `toy.table` (every row classified) after the pipeline: nothing in the stated pipeline \
             removes or rewrites it"
        );
        assert_eq!(classified().rule_words("toy.table"), Some("every row classified"));
    }

    /// A covered name is legal where its cover is: the uncovered entity is reported, by its cover's words and count,
    /// and kept in Partial; in Full it is illegal, the covered one still not.
    #[test]
    fn a_covered_name_reports_only_its_uncovered_entities() {
        let mut w = World::new();
        let covered = w.spawn();
        w.insert(covered, File(1));
        w.insert(covered, Pbr(1));
        let bare = w.spawn();
        w.insert(bare, File(2));
        let t = Target::new("cover")
            .legal(Pbr::NAME)
            .covered(File::NAME, "covered by toy.pbr", |w, e| w.has::<Pbr>(e));
        assert_eq!(
            t.check_prepared(&w).unsupported,
            vec![("toy.file not covered by toy.pbr".to_owned(), 1)]
        );
        assert_eq!(t.rule_words(File::NAME), Some("covered by toy.pbr"));
        assert!(t.illegal_entities(&w, File::NAME, Mode::Partial).is_empty());
        assert_eq!(t.illegal_entities(&w, File::NAME, Mode::Full), vec![bare]);
        // every entity covered: nothing to report
        w.insert(bare, Pbr(2));
        assert!(t.check_prepared(&w).unsupported.is_empty());
    }

    #[test]
    fn a_refused_value_names_its_rule_and_the_passes_that_left_it() {
        // `lazy` modifies toy.mesh but changes nothing: the check after the pipeline names the rule and the pass
        let mut r = PassRegistry::new();
        const LAZY: PassSpec = PassSpec {
            name: "lazy",
            options: &[],
            make: |_| {
                Ok(Box::new(Decl {
                    name: "lazy",
                    requires: &["toy.mesh"],
                    modifies: &["toy.mesh"],
                }))
            },
        };
        r.add(&LAZY);
        let m = PassManager::new(r);
        let t = Target::new("even").check::<Mesh>("even meshes", |m| m.0 % 2 == 0);
        let err = m
            .convert(
                &mut mesh_world(3),
                &m.registry.parse("lazy").unwrap(),
                &t,
                Mode::Partial,
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`even` does not accept `toy.mesh` (even meshes) on [e0] after the pipeline: lazy left it"
        );
        assert!(matches!(err, Error::Illegal { ref rule, .. } if rule == "even meshes"));
    }

    #[test]
    fn a_pass_that_provides_a_settled_name_after_it_refuses_the_pipeline() {
        let mut r = PassRegistry::new();
        const CLASSIFY: PassSpec = PassSpec {
            name: "classify",
            options: &[],
            make: |_| Ok(Box::new(Classify)),
        };
        r.add(&CLASSIFY);
        const ADD_ROW: PassSpec = PassSpec {
            name: "add-row",
            options: &[],
            make: |_| Ok(Box::new(AddRow)),
        };
        r.add(&ADD_ROW);
        let m = PassManager::new(r);
        // add-row after classify would leave a row unclassified: refused before anything runs
        let mut w = table_world(vec![None]);
        let err = m
            .run(
                &mut w,
                &m.registry.parse("classify, add-row").unwrap(),
                &mut RunReport::default(),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "add-row provides `toy.table` after classify settled it: its values would not be settled"
        );
        assert_eq!(w.resource::<Table>(), Some(&Table(vec![None])), "nothing ran");
        // the other way round is fine, and so is classifying again after adding
        let mut w = table_world(vec![None]);
        m.run(
            &mut w,
            &m.registry.parse("add-row, classify").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
        assert_eq!(w.resource::<Table>(), Some(&Table(vec![Some(0), Some(0)])));
        let mut w = table_world(vec![None]);
        m.run(
            &mut w,
            &m.registry.parse("classify, add-row, classify").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
        assert_eq!(w.resource::<Table>(), Some(&Table(vec![Some(0), Some(0)])));
    }

    #[test]
    fn validators_run_with_the_verifier() {
        struct NoPbr;
        impl Validator for NoPbr {
            fn name(&self) -> &'static str {
                "no-pbr"
            }
            fn check(&self, w: &World) -> Vec<(Option<Entity>, String)> {
                w.iter::<Pbr>()
                    .map(|(e, _)| (Some(e), "pbr is banned here".to_owned()))
                    .collect()
            }
        }
        let mut m = PassManager::new(registry()).with_pass_checks(true);
        m.add_validator(Box::new(NoPbr));
        let pipeline = m.registry.parse("load, bake").unwrap();
        let err = m
            .convert(&mut world(), &pipeline, &gltf(), Mode::Partial)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "verify after bake: no-pbr on e0: pbr is banned here"
        );
    }

    /// The verifier runs after a pass with the pass checks (a world the pass left invalid is refused, naming the
    /// pass), not without them; they are on by default in debug builds only.
    #[test]
    fn the_verifier_runs_after_passes_with_the_pass_checks() {
        struct NoPbr;
        impl Validator for NoPbr {
            fn name(&self) -> &'static str {
                "no-pbr"
            }
            fn check(&self, w: &World) -> Vec<(Option<Entity>, String)> {
                w.iter::<Pbr>()
                    .map(|(e, _)| (Some(e), "pbr is banned here".to_owned()))
                    .collect()
            }
        }
        assert_eq!(PassManager::new(registry()).pass_checks(), cfg!(debug_assertions));
        for checks in [true, false] {
            let mut m = PassManager::new(registry()).with_pass_checks(checks);
            m.add_validator(Box::new(NoPbr));
            let pipeline = m.registry.parse("load, bake").unwrap();
            let ran = m.run(&mut world(), &pipeline, &mut RunReport::default());
            if checks {
                assert_eq!(
                    ran.unwrap_err().to_string(),
                    "verify after bake: no-pbr on e0: pbr is banned here"
                );
            } else {
                assert!(ran.is_ok());
            }
        }
    }

    /// Reads the thread count, a setting it declares: what it ran on, as a count.
    struct Threaded;
    impl Pass for Threaded {
        fn name(&self) -> &'static str {
            "threaded"
        }
        fn may_lack_inputs(&self) -> bool {
            true
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.layered"]
        }
        fn settings(&self) -> &[Setting] {
            &[crate::parallel::THREADS]
        }
        fn approximates(&self) -> &'static str {
            ""
        }
        fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
            let threads = crate::parallel::threads(w).map_err(|e| e.to_string())?;
            log.count("threads", threads.get() as u64);
            Ok(())
        }
    }

    fn threads(n: usize) -> Settings {
        Settings {
            threads: primase_foundations::parallel::Threads::new(n).unwrap(),
        }
    }

    /// The program states its thread count once, on the manager: the manager states it in the world it runs, the
    /// pass reads it there, and the step's report says it (`threads 3`), outside the report's equality.
    #[test]
    fn the_manager_states_the_programs_settings_and_the_report_says_them() {
        let manager = PassManager::new(PassRegistry::new()).with_settings(threads(3));
        let mut w = world();
        let mut report = RunReport::default();
        let pipeline = Pipeline {
            passes: vec![Box::new(Threaded)],
        };
        manager.run(&mut w, &pipeline, &mut report).unwrap();
        assert_eq!(report.steps[0].log.counts["threads"], 3);
        assert_eq!(report.steps[0].settings, ["threads 3"]);
        assert!(report.to_string().contains("; threads 3)"), "{report}");
        let mut other = report.clone();
        other.steps[0].settings.clear();
        assert_eq!(report, other, "how a step ran is not what it made");
        // a world the manager already stated them in is run again as it is
        manager.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
    }

    /// A pass that declares a setting the program states nowhere is refused before it runs, naming the pass and the
    /// setting; one that is skipped for want of its inputs needs none.
    #[test]
    fn a_setting_the_program_does_not_state_refuses_the_pass_but_never_decides_a_skip() {
        let manager = PassManager::new(PassRegistry::new());
        let pipeline = Pipeline {
            passes: vec![Box::new(Threaded)],
        };
        let refused = manager
            .run(&mut world(), &pipeline, &mut RunReport::default())
            .unwrap_err();
        assert!(
            matches!(&refused, Error::Missing { pass, name } if pass == "threaded" && name == "core.threads"),
            "{refused}"
        );
        let mut report = RunReport::default();
        manager.run(&mut World::new(), &pipeline, &mut report).unwrap();
        assert_eq!(report.steps[0].skipped, ["toy.layered"]);
        assert!(report.steps[0].settings.is_empty());
    }

    /// A world that already states another thread count than the program's is a disagreement, refused naming both.
    #[test]
    fn a_world_stating_another_count_than_the_program_is_refused() {
        let manager = PassManager::new(PassRegistry::new()).with_settings(threads(3));
        let mut w = world();
        w.insert_resource(primase_foundations::parallel::Threads::new(2).unwrap());
        let pipeline = Pipeline {
            passes: vec![Box::new(Threaded)],
        };
        let refused = manager
            .run(&mut w, &pipeline, &mut RunReport::default())
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the world states `core.threads` as threads 2, the program as threads 3"
        );
    }
}
