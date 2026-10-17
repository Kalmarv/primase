//! The pass manager (DESIGN.md §4): passes that transform the world, the targets writers declare, a search for a
//! pipeline from what the world has to what a target accepts, and the verifier that runs between every step.
//!
//! - A [`Pass`] has a stable name, text options, and declares the component names it **requires**, **provides**,
//!   **removes** and **modifies** (in place), plus what it approximates. After a pass runs, the names that actually
//!   appeared and disappeared must be among those it declared. A pass that declares it **despawns entities** (leaves
//!   placements out, discards forms) may also make a name disappear that only despawned entities held: no
//!   surviving entity lost it. The planner does not count on that; it plans by the declared removals alone.
//! - A [`Target`] (a writer's declaration) gives each component name a [`Legality`]: legal, dynamic (a predicate on
//!   the entity's value), illegal, or carried through untouched. [`Mode::Partial`] keeps names the target does not
//!   list; [`Mode::Full`] requires every name to be legal or carried.
//! - [`PassManager::plan`] chooses passes by MLIR's legalization-depth model (`DialectConversion.cpp`
//!   `computeOpLegalizationDepth`): each illegal name gets the pass that removes it with the smallest depth (1 + the
//!   depth of what it provides and of what it requires that the world lacks), ties broken by priority, then by name.
//!   No greedy rewriting and no rollback: the plan is an explicit textual pipeline (`load-meshes,
//!   lower-skate-pbr{normals=rebuild}`), recorded in the [`RunReport`], and runs as written.
//! - The verifier ([`Component::verify`] on every component, [`crate::Resource::verify`] on every resource, plus the
//!   manager's world [`Validator`]s) runs after the
//!   reader, after every pass and before the writer; its errors name the step and the entity.
//! - An [`Instrument`] sees the world before and after every pass and when one fails (`print-world-after`, see
//!   [`crate::print`]). [`PassManager::run`] and [`PassManager::convert`] are [`PassManager::run_instrumented`] and
//!   [`PassManager::convert_instrumented`] with an instrument that does nothing.
//!
//! Everything is deterministic: passes, names and candidates are walked in sorted order.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::world::{Component, Entity, World};

/// A pass's text options (`name{key=value,...}`), sorted by key.
pub type Options = BTreeMap<String, String>;

/// Counts a pass logs while it runs: what it changed and what it approximated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassLog {
    pub counts: BTreeMap<String, u64>,
    pub approximations: BTreeMap<String, u64>,
}

impl PassLog {
    pub fn count(&mut self, what: &str, n: u64) {
        *self.counts.entry(what.to_owned()).or_default() += n;
    }

    /// Record `n` items lowered with an approximation (`"layer blend baked at 1024 px"`).
    pub fn approximate(&mut self, what: &str, n: u64) {
        *self.approximations.entry(what.to_owned()).or_default() += n;
    }
}

/// A transformation of the world. Declarations are component names (`core.mesh`, `skate.material`).
pub trait Pass: Send + Sync {
    /// Stable name, used in pipelines and reports (`load-meshes`).
    fn name(&self) -> &'static str;
    /// The options this instance was made with (printed into the pipeline).
    fn options(&self) -> Options {
        Options::new()
    }
    /// Names that must be present before it runs.
    fn requires(&self) -> &[&'static str] {
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
    /// Names whose values it edits in place.
    fn modifies(&self) -> &[&'static str] {
        &[]
    }
    /// What it approximates, in words (empty when it is exact).
    fn approximates(&self) -> &'static str {
        ""
    }
    /// Preference among passes of equal depth: higher first.
    fn priority(&self) -> i32 {
        0
    }
    /// Whether it despawns entities. If so, a component name it does not declare in [`Pass::removes`] may disappear
    /// when every entity that held the name before the pass was despawned by it (a dropped placement's
    /// `core.name`, a discarded form's dialect data); a name still held by a surviving entity before the pass must
    /// be declared as ever. Added names are checked as for every pass.
    fn despawns_entities(&self) -> bool {
        false
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String>;
}

/// Makes a pass from its options; unknown or bad options are an error.
pub type PassFactory = fn(&Options) -> Result<Box<dyn Pass>, String>;

/// The passes a manager knows, by name.
#[derive(Default)]
pub struct PassRegistry {
    factories: BTreeMap<&'static str, PassFactory>,
}

impl fmt::Debug for PassRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.factories.keys()).finish()
    }
}

impl PassRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pass. Panics when the name is taken or the factory does not make that pass with no options.
    pub fn register(&mut self, name: &'static str, factory: PassFactory) {
        let made = factory(&Options::new()).unwrap_or_else(|e| panic!("pass {name} with no options: {e}"));
        assert_eq!(
            made.name(),
            name,
            "factory registered as {name} makes {}",
            made.name()
        );
        assert!(
            self.factories.insert(name, factory).is_none(),
            "pass {name} registered twice"
        );
    }

    pub fn make(&self, name: &str, options: &Options) -> Result<Box<dyn Pass>, Error> {
        let f = self
            .factories
            .get(name)
            .ok_or_else(|| Error::UnknownPass(name.to_owned()))?;
        f(options).map_err(|message| Error::Options {
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

    /// Every registered pass with its default options, by name.
    fn defaults(&self) -> Vec<Box<dyn Pass>> {
        self.factories
            .values()
            .map(|f| f(&Options::new()).expect("checked at registration"))
            .collect()
    }
}

fn is_word(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn parse_pipeline(text: &str) -> Result<Vec<(String, Options)>, Error> {
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
            for item in r[..close].split(',').filter(|s| !s.trim().is_empty()) {
                let (k, v) = item
                    .split_once('=')
                    .ok_or_else(|| bad(format!("{name}: option {item:?} is not key=value")))?;
                let (k, v) = (k.trim(), v.trim());
                if !is_word(k) || v.contains('{') {
                    return Err(bad(format!("{name}: bad option {item:?}")));
                }
                if options.insert(k.to_owned(), v.to_owned()).is_some() {
                    return Err(bad(format!("{name}: option {k} twice")));
                }
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

impl fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pipeline({self})")
    }
}

/// The text form, which [`PassRegistry::parse`] reads back.
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

fn pass_text(p: &dyn Pass) -> String {
    let opts = p.options();
    if opts.is_empty() {
        return p.name().to_owned();
    }
    let items: Vec<String> = opts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{}{{{}}}", p.name(), items.join(","))
}

/// A predicate deciding whether one entity's component is legal for a target (e.g. a texture format it writes).
pub type LegalityCheck = Arc<dyn Fn(&World, Entity) -> bool + Send + Sync>;

/// What a target does with one component name.
#[derive(Clone)]
pub enum Legality {
    /// The writer writes it.
    Legal,
    /// Legal for the entities the predicate accepts; the others must be lowered.
    Dynamic(LegalityCheck),
    /// Must be lowered before writing.
    Illegal,
    /// Not interpreted, passed through untouched (glTF extras, v2 opaque tables).
    Carry,
}

impl fmt::Debug for Legality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Legality::Legal => "Legal",
            Legality::Dynamic(_) => "Dynamic",
            Legality::Illegal => "Illegal",
            Legality::Carry => "Carry",
        })
    }
}

/// How strict a conversion is about names the target does not list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Only illegal (and failing dynamic) names must go; unlisted names stay.
    Partial,
    /// Every name must be legal or carried; unlisted names are illegal unless the target carries unknown names.
    Full,
}

/// A writer's declaration of what it accepts.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    rules: BTreeMap<String, Legality>,
    carry_unknown: bool,
}

impl Target {
    pub fn new(name: impl Into<String>) -> Self {
        Target {
            name: name.into(),
            rules: BTreeMap::new(),
            carry_unknown: false,
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

    pub fn carry(self, name: &str) -> Self {
        self.rule(name, Legality::Carry)
    }

    pub fn dynamic(self, name: &str, check: impl Fn(&World, Entity) -> bool + Send + Sync + 'static) -> Self {
        self.rule(name, Legality::Dynamic(Arc::new(check)))
    }

    /// The legal checks of a typed component: legal when `check` accepts the value.
    pub fn dynamic_typed<T: Component>(self, check: impl Fn(&T) -> bool + Send + Sync + 'static) -> Self {
        self.dynamic(T::NAME, move |w, e| w.get::<T>(e).is_some_and(&check))
    }

    /// Pass every unlisted name through (also in [`Mode::Full`]).
    pub fn carry_unknown(mut self) -> Self {
        self.carry_unknown = true;
        self
    }

    pub fn legality(&self, name: &str) -> Option<&Legality> {
        self.rules.get(name)
    }

    /// Entities whose `name` component the target does not accept as it is (all of them for an illegal name).
    pub fn illegal_entities(&self, w: &World, name: &str, mode: Mode) -> Vec<Entity> {
        let all = w.entities_named(name);
        match self.rules.get(name) {
            Some(Legality::Legal | Legality::Carry) => Vec::new(),
            Some(Legality::Illegal) => all,
            Some(Legality::Dynamic(f)) => all.into_iter().filter(|&e| !f(w, e)).collect(),
            None if mode == Mode::Partial || self.carry_unknown => Vec::new(),
            None => all,
        }
    }

    /// For planning a name that is not in the world yet: dynamic names count as legal (the check before the writer
    /// sees their values).
    fn planned_legal(&self, name: &str, mode: Mode) -> bool {
        match self.rules.get(name) {
            Some(Legality::Legal | Legality::Carry | Legality::Dynamic(_)) => true,
            Some(Legality::Illegal) => false,
            None => mode == Mode::Partial || self.carry_unknown,
        }
    }
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
    #[error("no pass named {0:?}")]
    UnknownPass(String),
    #[error("pass {pass}: {message}")]
    Options { pass: String, message: String },
    #[error("no pass lowers `{name}` to anything `{target}` accepts{detail}")]
    NoLowering {
        name: String,
        target: String,
        detail: String,
    },
    #[error("cannot order passes {0:?}: they depend on each other")]
    Cycle(Vec<String>),
    #[error("pass {pass} requires `{name}`, which the world does not have")]
    Missing { pass: String, name: String },
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
    #[error("`{target}` does not accept `{name}` on {entities:?} after the pipeline")]
    Illegal {
        target: String,
        name: String,
        entities: Vec<Entity>,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// One pass's run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    /// The pass with its options, as in the pipeline text.
    pub pass: String,
    pub approximates: &'static str,
    pub log: PassLog,
    /// Component names that appeared and disappeared.
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Why the planner chose a pass: the illegal (or missing) name and the chain depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanChoice {
    pub name: String,
    pub pass: String,
    pub depth: u32,
}

/// What a conversion did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub target: String,
    pub pipeline: String,
    pub plan: Vec<PlanChoice>,
    pub steps: Vec<StepReport>,
}

impl fmt::Display for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "target {}: pipeline [{}]", self.target, self.pipeline)?;
        for c in &self.plan {
            writeln!(f, "  plan: `{}` by {} (depth {})", c.name, c.pass, c.depth)?;
        }
        for s in &self.steps {
            write!(f, "  {}: +{:?} -{:?}", s.pass, s.added, s.removed)?;
            for (k, n) in &s.log.counts {
                write!(f, " {k}={n}")?;
            }
            for (k, n) in &s.log.approximations {
                write!(f, " ~{k}={n}")?;
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

const INF: u32 = u32::MAX;

/// Runs pipelines, plans them, and verifies the world between steps.
#[derive(Default)]
pub struct PassManager {
    pub registry: PassRegistry,
    validators: Vec<Box<dyn Validator>>,
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
        }
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
        for (index, p) in pipeline.passes.iter().enumerate() {
            let text = pass_text(p.as_ref());
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
            match self.run_pass(w, p.as_ref(), &text) {
                Ok(done) => {
                    hook(instrument.after_pass(&step, w, &done))?;
                    report.steps.push(done);
                }
                Err(e) => {
                    // the pass's error is the one that matters; a failing hook has nothing to add to it
                    let _ = instrument.pass_failed(&step, w, &e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// One pass: its requirements, the run, its declarations, the verifier.
    fn run_pass(&self, w: &mut World, p: &dyn Pass, text: &str) -> Result<StepReport> {
        for &r in p.requires() {
            if !w.has_component_named(r) {
                return Err(Error::Missing {
                    pass: text.to_owned(),
                    name: r.to_owned(),
                });
            }
        }
        let before: BTreeSet<String> = w.component_names().into_iter().map(str::to_owned).collect();
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
        let mut log = PassLog::default();
        p.run(w, &mut log).map_err(|message| Error::Pass {
            pass: text.to_owned(),
            message,
        })?;
        let after: BTreeSet<String> = w.component_names().into_iter().map(str::to_owned).collect();
        let added: Vec<String> = after.difference(&before).cloned().collect();
        let removed: Vec<String> = before.difference(&after).cloned().collect();
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
                .filter(|n| !declared.contains(&n.as_str()) && !(removal && despawned_with(n)))
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
        self.verify(w, text)?;
        Ok(StepReport {
            pass: text.to_owned(),
            approximates: p.approximates(),
            log,
            added,
            removed,
        })
    }

    /// Choose the passes that take the world to what `target` accepts (see the module docs), in run order.
    pub fn plan(&self, w: &World, target: &Target, mode: Mode, report: &mut RunReport) -> Result<Pipeline> {
        let passes = self.registry.defaults();
        let present: BTreeSet<&str> = w.component_names().into_iter().collect();
        let illegal_now: Vec<&str> = present
            .iter()
            .copied()
            .filter(|n| !target.illegal_entities(w, n, mode).is_empty())
            .collect();
        let legal = |n: &str| {
            if present.contains(n) {
                !illegal_now.contains(&n)
            } else {
                target.planned_legal(n, mode)
            }
        };

        // depth[name]: chain depth to lower an illegal name; supply[name]: to provide a missing required name
        let mut depth: BTreeMap<&str, u32> = BTreeMap::new();
        let mut supply: BTreeMap<&str, u32> = BTreeMap::new();
        let mut cost = vec![INF; passes.len()];
        loop {
            let mut changed = false;
            for (i, p) in passes.iter().enumerate() {
                let provided = p.provides().iter().map(|n| {
                    if legal(n) {
                        0
                    } else {
                        *depth.get(n).unwrap_or(&INF)
                    }
                });
                let required = p.requires().iter().map(|n| {
                    if present.contains(n) {
                        0
                    } else {
                        *supply.get(n).unwrap_or(&INF)
                    }
                });
                let worst = provided.chain(required).max().unwrap_or(0);
                let c = if worst == INF { INF } else { worst + 1 };
                if c < cost[i] {
                    cost[i] = c;
                    changed = true;
                    for &n in p.removes() {
                        if !legal(n) && c < *depth.get(n).unwrap_or(&INF) {
                            depth.insert(n, c);
                        }
                    }
                    for &n in p.provides() {
                        if c < *supply.get(n).unwrap_or(&INF) {
                            supply.insert(n, c);
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // the cheapest pass for a name: smallest cost, then highest priority, then name
        let best = |name: &str, lowering: bool| -> Option<usize> {
            (0..passes.len())
                .filter(|&i| {
                    cost[i] != INF
                        && if lowering {
                            passes[i].removes().contains(&name)
                        } else {
                            passes[i].provides().contains(&name)
                        }
                })
                .min_by_key(|&i| (cost[i], -passes[i].priority(), passes[i].name()))
        };

        let mut chosen: BTreeSet<usize> = BTreeSet::new();
        let mut todo: Vec<(String, bool)> = illegal_now.iter().rev().map(|n| (n.to_string(), true)).collect();
        while let Some((name, lowering)) = todo.pop() {
            let already = chosen.iter().any(|&i| {
                if lowering {
                    passes[i].removes().contains(&name.as_str())
                } else {
                    passes[i].provides().contains(&name.as_str())
                }
            });
            if already {
                continue;
            }
            let Some(i) = best(&name, lowering) else {
                return Err(Error::NoLowering {
                    detail: self.why_not(&passes, &name, lowering, &present),
                    name,
                    target: target.name.clone(),
                });
            };
            report.plan.push(PlanChoice {
                name: name.clone(),
                pass: passes[i].name().to_owned(),
                depth: cost[i],
            });
            chosen.insert(i);
            let p = &passes[i];
            for &r in p.requires().iter().rev() {
                if !present.contains(r) {
                    todo.push((r.to_owned(), false));
                }
            }
            for &n in p.provides().iter().rev() {
                if !legal(n) {
                    todo.push((n.to_owned(), true));
                }
            }
        }

        let order = order_passes(&passes, &chosen)?;
        // check the order by simulating the declarations
        let mut names = present.clone();
        for &i in &order {
            let p = &passes[i];
            if let Some(r) = p.requires().iter().find(|r| !names.contains(*r)) {
                return Err(Error::Missing {
                    pass: p.name().to_owned(),
                    name: (*r).to_owned(),
                });
            }
            for n in p.removes() {
                names.remove(n);
            }
            names.extend(p.provides().iter().copied());
        }
        if let Some(n) = names.iter().find(|n| !legal(n)) {
            return Err(Error::NoLowering {
                name: (*n).to_owned(),
                target: target.name.clone(),
                detail: " (the planned pipeline leaves it)".to_owned(),
            });
        }
        let pipeline = Pipeline {
            passes: order
                .into_iter()
                .map(|i| self.registry.make(passes[i].name(), &Options::new()))
                .collect::<Result<_>>()?,
        };
        report.target = target.name.clone();
        report.pipeline = pipeline.to_string();
        Ok(pipeline)
    }

    /// The missing link behind a failed plan, for the error message.
    fn why_not(
        &self,
        passes: &[Box<dyn Pass>],
        name: &str,
        lowering: bool,
        present: &BTreeSet<&str>,
    ) -> String {
        let candidates: Vec<String> = passes
            .iter()
            .filter(|p| {
                if lowering {
                    p.removes().contains(&name)
                } else {
                    p.provides().contains(&name)
                }
            })
            .map(|p| {
                let missing: Vec<&str> = p
                    .requires()
                    .iter()
                    .copied()
                    .filter(|r| !present.contains(r))
                    .collect();
                if missing.is_empty() {
                    format!("{} leaves something illegal", p.name())
                } else {
                    format!("{} needs {missing:?}", p.name())
                }
            })
            .collect();
        let what = if lowering {
            ""
        } else {
            " (required, nothing provides it)"
        };
        if candidates.is_empty() {
            what.to_owned()
        } else {
            format!("{what}: {}", candidates.join("; "))
        }
    }

    /// Plan, run and check: verify the reader's world, run the planned pipeline (verifying after every pass), then
    /// check that the target accepts every component and verify once more before the writer.
    pub fn convert(&self, w: &mut World, target: &Target, mode: Mode) -> Result<RunReport> {
        self.convert_instrumented(w, target, mode, &mut NoInstrument)
    }

    /// [`PassManager::convert`], calling `instrument` around every pass of the planned pipeline.
    pub fn convert_instrumented(
        &self,
        w: &mut World,
        target: &Target,
        mode: Mode,
        instrument: &mut dyn Instrument,
    ) -> Result<RunReport> {
        self.verify(w, "the reader")?;
        let mut report = RunReport::default();
        let pipeline = self.plan(w, target, mode, &mut report)?;
        self.run_instrumented(w, &pipeline, &mut report, instrument)?;
        for name in w.component_names() {
            let bad = target.illegal_entities(w, name, mode);
            if !bad.is_empty() {
                return Err(Error::Illegal {
                    target: target.name.clone(),
                    name: name.to_owned(),
                    entities: bad.into_iter().take(10).collect(),
                });
            }
        }
        self.verify(w, &format!("the pipeline, before {}", target.name))?;
        Ok(report)
    }
}

/// Run order of the chosen passes: a pass that provides what another requires, removes or modifies runs first; a
/// pass that requires what another removes runs before it. Ties by name.
fn order_passes(passes: &[Box<dyn Pass>], chosen: &BTreeSet<usize>) -> Result<Vec<usize>> {
    let ids: Vec<usize> = chosen.iter().copied().collect();
    let before = |a: usize, b: usize| -> bool {
        let (pa, pb) = (&passes[a], &passes[b]);
        let consumes = |p: &dyn Pass, n: &&str| {
            p.requires().contains(n) || p.removes().contains(n) || p.modifies().contains(n)
        };
        pa.provides().iter().any(|n| consumes(pb.as_ref(), n))
            || pa
                .requires()
                .iter()
                .any(|n| pb.removes().contains(n) && !pb.provides().contains(n))
    };
    let mut indeg: BTreeMap<usize, usize> = ids.iter().map(|&i| (i, 0)).collect();
    for &a in &ids {
        for &b in &ids {
            if a != b && before(a, b) {
                *indeg.get_mut(&b).expect("chosen") += 1;
            }
        }
    }
    let mut out = Vec::new();
    while !indeg.is_empty() {
        let next = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(&i, _)| i)
            .min_by_key(|&i| passes[i].name());
        let Some(n) = next else {
            return Err(Error::Cycle(
                indeg.keys().map(|&i| passes[i].name().to_owned()).collect(),
            ));
        };
        indeg.remove(&n);
        for (&b, d) in indeg.iter_mut() {
            if before(n, b) {
                *d -= 1;
            }
        }
        out.push(n);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Opaque, OpaqueData};

    // toy components
    #[derive(Clone, Debug, PartialEq)]
    struct File(u32);
    impl Component for File {
        const NAME: &'static str = "toy.file";
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Mesh(u32);
    impl Component for Mesh {
        const NAME: &'static str = "toy.mesh";
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
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Pbr(u32);
    impl Component for Pbr {
        const NAME: &'static str = "toy.pbr";
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
    fn load(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Load))
    }

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
    fn bake(o: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        let mut size = 1024;
        for (k, v) in o {
            match k.as_str() {
                "size" => size = v.parse().map_err(|_| format!("size {v:?} is not a number"))?,
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Box::new(Bake { size }))
    }

    /// A cheaper-looking but worse way to drop toy.layered (priority -1): lowers it straight to nothing useful.
    struct Drop;
    impl Pass for Drop {
        fn name(&self) -> &'static str {
            "drop-layers"
        }
        fn removes(&self) -> &[&'static str] {
            &["toy.layered"]
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.pbr"]
        }
        fn priority(&self) -> i32 {
            -1
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            let es: Vec<Entity> = w.iter::<Layered>().map(|(e, _)| e).collect();
            for e in es {
                w.remove::<Layered>(e);
                w.insert(e, Pbr(0));
            }
            Ok(())
        }
    }
    fn drop_layers(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Drop))
    }

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
    fn sneaky(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Sneaky))
    }

    fn registry(with_drop: bool) -> PassRegistry {
        let mut r = PassRegistry::new();
        r.register("load", load);
        r.register("bake", bake);
        r.register("sneaky", sneaky);
        if with_drop {
            r.register("drop-layers", drop_layers);
        }
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
        );
        w
    }

    fn gltf() -> Target {
        Target::new("gltf")
            .legal("toy.mesh")
            .legal("toy.pbr")
            .illegal("toy.file")
            .illegal("toy.layered")
    }

    #[test]
    fn pipeline_text_round_trips() {
        let r = registry(false);
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

    #[test]
    fn plan_finds_the_chain_and_orders_it() {
        let m = PassManager::new(registry(false));
        let mut w = world();
        let report = m.convert(&mut w, &gltf(), Mode::Partial).unwrap();
        // bake needs toy.mesh, which only load provides: load runs first
        assert_eq!(report.pipeline, "load, bake");
        assert_eq!(
            report.plan,
            [
                PlanChoice {
                    name: "toy.file".into(),
                    pass: "load".into(),
                    depth: 1
                },
                PlanChoice {
                    name: "toy.layered".into(),
                    pass: "bake".into(),
                    depth: 2
                },
            ]
        );
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
        // the same plan every time
        for _ in 0..3 {
            let mut r = RunReport::default();
            assert_eq!(
                m.plan(&world(), &gltf(), Mode::Partial, &mut r)
                    .unwrap()
                    .to_string(),
                "load, bake"
            );
        }
    }

    #[test]
    fn depth_then_priority_then_name() {
        // drop-layers has depth 1 (needs nothing) against bake's 2 (needs load): depth wins over priority
        let m = PassManager::new(registry(true));
        let mut r = RunReport::default();
        assert_eq!(
            m.plan(&world(), &gltf(), Mode::Partial, &mut r)
                .unwrap()
                .to_string(),
            "drop-layers, load"
        );
        // with the mesh already loaded both have depth 1: bake's priority 0 beats drop-layers' -1
        let mut w = world();
        let e = w.entities().next().unwrap();
        w.remove::<File>(e);
        w.insert(e, Mesh(1));
        let mut r = RunReport::default();
        assert_eq!(
            m.plan(&w, &gltf(), Mode::Partial, &mut r).unwrap().to_string(),
            "bake"
        );
    }

    #[test]
    fn full_conversion_names_the_missing_link() {
        let m = PassManager::new(registry(false));
        let mut w = world();
        let err = m.convert(&mut w, &gltf(), Mode::Full).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `game.thing` to anything `gltf` accepts"
        );
        // carrying unknown names makes it pass
        let mut w = world();
        m.convert(&mut w, &gltf().carry_unknown(), Mode::Full).unwrap();
        // a lowering whose requirement nothing provides
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Layered(1));
        let err = m.convert(&mut w, &gltf(), Mode::Partial).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `toy.layered` to anything `gltf` accepts: bake needs [\"toy.mesh\"]"
        );
    }

    #[test]
    fn dynamic_legality_lowers_only_failing_values() {
        let m = PassManager::new(registry(false));
        let t = Target::new("skate")
            .legal("toy.mesh")
            .legal("toy.layered")
            .dynamic_typed::<File>(|f| f.0 < 10)
            .carry_unknown();
        let mut w = world(); // File(1): legal as it is
        let report = m.convert(&mut w, &t, Mode::Full).unwrap();
        assert_eq!(report.pipeline, "");
        assert!(w.has_component_named("toy.file"));
        let mut w = world();
        let e = w.entities().next().unwrap();
        w.insert(e, File(99)); // fails the check: lowered
        let report = m.convert(&mut w, &t, Mode::Full).unwrap();
        assert_eq!(report.pipeline, "load");
        assert_eq!(report.plan[0].name, "toy.file");
    }

    #[test]
    fn undeclared_changes_and_verifier_name_the_pass() {
        let m = PassManager::new(registry(false));
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
        let mut m = PassManager::new(registry(false));
        m.add_validator(Box::new(NoPbr));
        let err = m.convert(&mut world(), &gltf(), Mode::Partial).unwrap_err();
        assert_eq!(
            err.to_string(),
            "verify after bake: no-pbr on e0: pbr is banned here"
        );
    }
}
