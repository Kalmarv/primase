//! The pass manager (DESIGN.md §4): passes that transform the world, the targets writers declare, a search for a
//! pipeline from what the world has to what a target accepts, and the verifier that runs between every step.
//!
//! - A [`Pass`] has a stable name, text options, and declares the component and resource names it **requires**,
//!   **provides**, **removes** and **modifies** (in place), plus what it approximates (a name is a component's or a
//!   resource's, never both). After a pass runs, the names that actually
//!   appeared and disappeared must be among those it declared. A pass that declares it **despawns entities** (leaves
//!   placements out, discards forms) may also make a name disappear that only despawned entities held: no
//!   surviving entity lost it. A pass that **removes by log** may make a component name disappear that it recorded
//!   as removed ([`PassLog::removed`]: what a hook dropped, known only when it runs). The planner counts on neither;
//!   it plans by the declared removals alone. A pass
//!   also declares the names it **legalizes** (lowers: removed, rewritten in place, or lowered into what it provides
//!   while kept), by default what it removes.
//! - A [`Target`] (a writer's declaration) gives each component name a [`Legality`]: legal, dynamic (a predicate on
//!   the entity's value), illegal, carried through untouched, or carried and lowered too. [`Mode::Partial`] keeps
//!   names the target does not list; [`Mode::Full`] requires every name to be legal or carried.
//! - [`PassManager::plan`] chooses passes by MLIR's legalization-depth model (`DialectConversion.cpp`
//!   `computeOpLegalizationDepth`): each name to lower gets the pass that lowers it with the smallest depth (1 + the
//!   depth of what it provides and of what it requires that the world lacks), ties broken by priority, then by name.
//!   Which passes may lower a name depends on its rule: an illegal name must be removed, a dynamic one's failing values
//!   removed or rewritten in place, a carried-and-lowered one only lowered ([`Legality`]). No greedy rewriting and no
//!   rollback: the plan is an explicit textual pipeline (`load-meshes, lower-skate-pbr{normals=rebuild}`), made with
//!   the caller's [`PassOptions`], recorded in the [`RunReport`], and runs as written.
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

pub use crate::ledger::{Conservation, Gap, Measure};
use crate::world::{Component, Entity, Resource, World};

/// A pass's text options (`name{key=value,...}`), sorted by key.
pub type Options = BTreeMap<String, String>;

/// The options of the passes a plan may choose, by pass name; a pass not listed is made with no options. A plan holds
/// one instance of a pass, so a pipeline that runs one pass twice with different options is written as text
/// ([`PassRegistry::parse`]).
pub type PassOptions = BTreeMap<String, Options>;

/// Counts a pass logs while it runs: what it changed, what it approximated and what it left out, and the entities it
/// placed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassLog {
    pub counts: BTreeMap<String, u64>,
    pub approximations: BTreeMap<String, u64>,
    /// What the pass left out because what it makes has no form for it, by reason (`collision triangles on a surface
    /// with no skate surface, left out: PHYS_CASTER`). A conversion reports each as an unsupported notice.
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
}

impl PassLog {
    pub fn count(&mut self, what: &str, n: u64) {
        *self.counts.entry(what.to_owned()).or_default() += n;
    }

    /// Record that the pass removed component `name` from some entity (see [`PassLog::removed`]).
    pub fn record_removed(&mut self, name: &str) {
        self.removed.insert(name.to_owned());
    }

    /// Record `n` items lowered with an approximation (`"layer blend baked at 1024 px"`).
    pub fn approximate(&mut self, what: &str, n: u64) {
        *self.approximations.entry(what.to_owned()).or_default() += n;
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
            omitted,
            removed,
            placed,
            placed_from,
            drops,
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
        for (k, n) in omitted {
            self.omit(&k, n);
        }
        self.removed.extend(removed);
        self.placed.extend(placed);
        self.placed_from.extend(placed_from);
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
    /// Whether a part run ([`PassManager::run_part`]) may skip it in a part that holds none of its required names: a
    /// per-part lowering whose input a part may simply not have (a web tile of only a light has no materials) opts in.
    /// Required names are prerequisites, so this is false unless the pass says otherwise: an absent name may be a
    /// failed load or a wrong order, and a pass that summarizes, decides or plans never opts in.
    fn part_may_lack_inputs(&self) -> bool {
        false
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
    /// Names whose values it edits in place, and resources it writes (replaced, also by removing and inserting again,
    /// or borrowed with [`World::resource_mut`]). The pass manager checks the resources; an in-place edit of a
    /// component's value on an entity that keeps it (`get_mut`, or [`World::insert`] over the value) is declared here
    /// and not checked: seeing it would take a controlled editing interface, later work. Adding a name to an entity, taking one away and despawning are checked entity by entity
    /// against [`Pass::provides`], [`Pass::removes`] and [`Pass::despawns_entities`].
    fn modifies(&self) -> &[&'static str] {
        &[]
    }
    /// Names it lowers: after it runs their values are in a form a target accepts, because it removed them, rewrote
    /// them in place (also in [`Pass::modifies`]) or lowered them into what it provides and kept them. The planner
    /// chooses passes by it ([`PassManager::plan`]). By default what it removes; a pass whose removals are side effects
    /// (the file link of a value it rewrote) or that keeps its source declares its own.
    fn legalizes(&self) -> &[&'static str] {
        self.removes()
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
    /// Whether the components it removes are not all known in advance (a hook decides, as
    /// [`crate::Component::apply_linear`] does). If so, a component name that disappears is accepted when the pass
    /// recorded it in [`PassLog::removed`] as well as when it is declared in [`Pass::removes`]; a name that disappears
    /// unrecorded and undeclared is an error as ever, and a resource it removes must be declared as for any pass.
    fn removes_by_log(&self) -> bool {
        false
    }
    /// Names whose values it brings to their final form (a classifier: every surface classified). A later pass in a
    /// pipeline that provides one of them adds values that never went through this pass: unless a pass after it settles
    /// the name again, [`PassManager::run`] refuses such a pipeline before it runs ([`Error::SettledThenChanged`]), and
    /// the planner orders this pass after every
    /// chosen pass that provides what it settles. Modifying settled values later is allowed. A modifier must not add
    /// values without declaring them in [`Pass::provides`]; the target's value rules check the result.
    fn settles(&self) -> &[&'static str] {
        &[]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String>;
}

/// Makes a pass from its options; unknown or bad options are an error.
pub type PassFactory = fn(&Options) -> Result<Box<dyn Pass>, String>;

/// The passes a manager knows, by name.
#[derive(Default)]
pub struct PassRegistry {
    /// Each pass's factory and the options it is made with when nobody gives any (none, or the example of a pass
    /// whose options have no defaults: [`PassRegistry::register_requiring`]).
    factories: BTreeMap<&'static str, (PassFactory, Options)>,
    /// The passes registered with required options ([`PassRegistry::register_requiring`]).
    requiring: BTreeSet<&'static str>,
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

    /// Register a pass. Panics when the name is taken, the factory does not make that pass with no options, or the
    /// pass legalizes a name it neither requires, removes nor modifies.
    pub fn register(&mut self, name: &'static str, factory: PassFactory) {
        self.register_requiring(name, factory, &[]);
    }

    /// Register a pass whose options have no defaults (a choice the program makes, as a partition's cell size): the
    /// declaration checks of [`PassRegistry::register`], and the planner reading its declarations, make it with
    /// `example` options; a pipeline that leaves them out fails to parse with the factory's reason, and a plan that
    /// chooses it without the caller's options for it is an error: the example is never run.
    pub fn register_requiring(&mut self, name: &'static str, factory: PassFactory, example: &[(&str, &str)]) {
        let options: Options = example
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let made = factory(&options).unwrap_or_else(|e| panic!("pass {name} with options {options:?}: {e}"));
        assert_eq!(
            made.name(),
            name,
            "factory registered as {name} makes {}",
            made.name()
        );
        for n in made.legalizes() {
            assert!(
                made.requires().contains(n) || made.removes().contains(n) || made.modifies().contains(n),
                "pass {name} legalizes {n}, which it neither requires, removes nor modifies"
            );
        }
        if !options.is_empty() {
            self.requiring.insert(name);
        }
        assert!(
            self.factories.insert(name, (factory, options)).is_none(),
            "pass {name} registered twice"
        );
    }

    /// Whether `name` was registered with required options: made with its example only to read its declarations,
    /// never to run.
    pub fn requires_options(&self, name: &str) -> bool {
        self.requiring.contains(name)
    }

    /// Every registered pass's name, in order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.factories.keys().copied()
    }

    /// The pass `name` made with the options it was registered with (none, or a required-option pass's example).
    pub fn make_registered(&self, name: &str) -> Result<Box<dyn Pass>, Error> {
        let (_, example) = self
            .factories
            .get(name)
            .ok_or_else(|| Error::UnknownPass(name.to_owned()))?;
        self.make(name, example)
    }

    pub fn make(&self, name: &str, options: &Options) -> Result<Box<dyn Pass>, Error> {
        let (f, _) = self
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

    /// Every registered pass, by name, made with its options in `options` (its registered example when it is not
    /// listed there). Options for a pass that is not registered are an error.
    fn instances(&self, options: &PassOptions) -> Result<Vec<Box<dyn Pass>>, Error> {
        if let Some(name) = options.keys().find(|n| !self.factories.contains_key(n.as_str())) {
            return Err(Error::UnknownPass(name.clone()));
        }
        self.factories
            .iter()
            .map(|(name, (_, example))| self.make(name, options.get(*name).unwrap_or(example)))
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
impl Pipeline {
    /// The checks [`PassManager::run`] makes from the passes' declarations before it runs anything: today, that no
    /// pass provides a name an earlier pass settles ([`Pass::settles`]) unless a later one settles it again. It needs
    /// no world, so a program's tests run it on every pipeline it composes (data-free, unlike a run).
    pub fn check(&self) -> Result<()> {
        check_settled(&self.passes)
    }
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

/// A pass with its options as pipeline text (`bake{size=512}`), as [`PassRegistry::parse`] reads it; what
/// `core.provenance` records for the passes that derive an entity.
pub fn pass_text(p: &dyn Pass) -> String {
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
    /// Written through another component: legal on the entities the check accepts (a `core.light` with the
    /// `skate.light` it was raised to); on the rest as an unlisted name (kept and reported as not written in
    /// [`Mode::Partial`], illegal in [`Mode::Full`], unless the target carries unknown names). See
    /// [`Target::covered`].
    Covered(fn(&World, Entity) -> bool),
    /// Must be lowered before writing.
    Illegal,
    /// Not interpreted, passed through untouched (glTF extras, v2 opaque tables).
    Carry,
    /// Passed through untouched as [`Legality::Carry`], and lowered too: the writer carries it but draws only what a
    /// pass lowers it to (a dialect material kept in the extras beside the PBR material lowered from it). A pass that
    /// legalizes it must run; the name may stay.
    LowerAndCarry,
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
    /// [`Legality::Carry`].
    Carry,
    /// [`Legality::LowerAndCarry`].
    LowerAndCarry,
}

impl fmt::Debug for Legality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Legality::Legal => "Legal",
            Legality::Dynamic(_) => "Dynamic",
            Legality::Covered(_) => "Covered",
            Legality::Illegal => "Illegal",
            Legality::Carry => "Carry",
            Legality::LowerAndCarry => "LowerAndCarry",
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
    carry_unknown: bool,
}

impl Target {
    pub fn new(name: impl Into<String>) -> Self {
        Target {
            name: name.into(),
            rules: BTreeMap::new(),
            rule_words: BTreeMap::new(),
            resource_rules: BTreeMap::new(),
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

    pub fn lower_and_carry(self, name: &str) -> Self {
        self.rule(name, Legality::LowerAndCarry)
    }

    pub fn dynamic(self, name: &str, check: impl Fn(&World, Entity) -> bool + Send + Sync + 'static) -> Self {
        self.rule(name, Legality::Dynamic(Arc::new(check)))
    }

    /// A name the writer writes through another component: legal on the entities `check` accepts, `what` naming the
    /// cover (`"covered by skate.light"`); the rest are reported as `<name> not <what>` ([`Legality::Covered`]).
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
    /// it (`"every surface classified"`). The planner lowers a refused resource like a refused component value, with a
    /// pass that legalizes the resource's name and modifies or removes it.
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

    /// Pass every unlisted name through (also in [`Mode::Full`]).
    pub fn carry_unknown(mut self) -> Self {
        self.carry_unknown = true;
        self
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
                    Legality::Carry => Rule::Carry,
                    Legality::LowerAndCarry => Rule::LowerAndCarry,
                };
                (name.as_str(), rule)
            })
            .collect()
    }

    /// Whether it passes every name it states no rule for through ([`Target::carry_unknown`]).
    pub fn carries_unknown(&self) -> bool {
        self.carry_unknown
    }

    /// The resources it checks the values of ([`Target::check_resource`]), by name.
    pub fn checked_resources(&self) -> Vec<&str> {
        self.resource_rules.keys().map(String::as_str).collect()
    }

    /// Check a prepared world against the target without changing it: what the writer won't write (an illegal or,
    /// unless carried, an unlisted name, and a covered name's uncovered entities: [`Prepared::unsupported`], entity
    /// counts by name; a resource it neither
    /// lists, rules on nor carries: [`Prepared::unsupported_resources`]; the conversion's own resources,
    /// [`crate::convert::OPERATIONAL_RESOURCES`], aside), and values of names it writes that its value rule refuses
    /// ([`Prepared::invalid`], by name with the rule's words) or resources it refuses ([`Prepared::invalid_resources`]).
    /// A resource is listed like a component ([`Target::legal`], [`Target::illegal`]).
    pub fn check_prepared(&self, w: &World) -> Prepared {
        let mut out = Prepared::default();
        for name in w.component_names() {
            let all = w.entities_named(name);
            match self.rules.get(name) {
                Some(Legality::Legal | Legality::Carry | Legality::LowerAndCarry) => {}
                Some(Legality::Dynamic(f)) => {
                    let bad: Vec<Entity> = all.into_iter().filter(|&e| !f(w, e)).collect();
                    if !bad.is_empty() {
                        let words = self.rule_words(name).unwrap_or_default().to_owned();
                        out.invalid.push((name.to_owned(), words, bad));
                    }
                }
                None if self.carry_unknown => {}
                Some(Legality::Covered(f)) => {
                    let uncovered = all.iter().filter(|&&e| !f(w, e)).count();
                    if uncovered > 0 && !self.carry_unknown {
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
                || match self.rules.get(name) {
                    Some(Legality::Illegal) => false,
                    Some(_) => true,
                    None => self.carry_unknown,
                };
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
            Some(Legality::Legal | Legality::Carry | Legality::LowerAndCarry) => Vec::new(),
            Some(Legality::Illegal) => all,
            Some(Legality::Dynamic(f)) => all.into_iter().filter(|&e| !f(w, e)).collect(),
            Some(Legality::Covered(_)) if mode == Mode::Partial || self.carry_unknown => Vec::new(),
            Some(Legality::Covered(f)) => all.into_iter().filter(|&e| !f(w, e)).collect(),
            None if mode == Mode::Partial || self.carry_unknown => Vec::new(),
            None => all,
        }
    }

    /// What a plan must do about `name`, which `w` has or a pass will provide. A dynamic name needs lowering only for
    /// the values `w` has that its check refuses; values a pass will provide count as legal (the check before the
    /// writer sees them).
    fn need(&self, w: &World, name: &str, mode: Mode) -> Need {
        if let Some(r) = self.resource_rules.get(name) {
            return if (r.check)(w) { Need::Nothing } else { Need::Fix };
        }
        match self.rules.get(name) {
            Some(Legality::Legal | Legality::Carry) => Need::Nothing,
            Some(Legality::LowerAndCarry) => Need::Lower,
            Some(Legality::Illegal) => Need::Remove,
            Some(Legality::Dynamic(f)) => {
                if w.entities_named(name).into_iter().all(|e| f(w, e)) {
                    Need::Nothing
                } else {
                    Need::Fix
                }
            }
            // the uncovered entities, as an unlisted name's
            Some(Legality::Covered(_)) if mode == Mode::Partial || self.carry_unknown => Need::Nothing,
            Some(Legality::Covered(f)) => {
                if w.entities_named(name).into_iter().all(|e| f(w, e)) {
                    Need::Nothing
                } else {
                    Need::Remove
                }
            }
            None if mode == Mode::Partial || self.carry_unknown => Need::Nothing,
            None => Need::Remove,
        }
    }
}

/// What [`Target::check_prepared`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// Names the writer doesn't write, with how many entities hold them; a covered name ([`Target::covered`]) as
    /// `<name> not <its cover>` (`core.light not covered by skate.light`), for its uncovered entities.
    pub unsupported: Vec<(String, u64)>,
    /// Names it writes whose values its rule refuses: the name, the rule's words, the entities.
    pub invalid: Vec<(String, String, Vec<Entity>)>,
    /// Resources its rule refuses.
    pub invalid_resources: Vec<String>,
    /// Resources the writer doesn't write.
    pub unsupported_resources: Vec<String>,
}

/// What a plan must do about one name, by the target's rule for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    /// Legal or carried as it is.
    Nothing,
    /// Illegal: a pass that legalizes and removes it.
    Remove,
    /// Dynamic with values the check refuses: a pass that legalizes it and removes it or rewrites it in place.
    Fix,
    /// Carried and lowered: a pass that legalizes it; the name may stay.
    Lower,
}

impl Need {
    /// Whether `p` lowers a name with this need.
    fn met_by(self, p: &dyn Pass, name: &str) -> bool {
        let legalizes = p.legalizes().contains(&name);
        match self {
            Need::Nothing => false,
            Need::Remove => legalizes && p.removes().contains(&name),
            Need::Fix => legalizes && (p.removes().contains(&name) || p.modifies().contains(&name)),
            Need::Lower => legalizes,
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
    #[error("{}", illegal_text(.target, .name, .entities, .rule))]
    Illegal {
        target: String,
        name: String,
        /// The entities refused (none for a resource).
        entities: Vec<Entity>,
        /// The words naming the rule refused, empty when it has none.
        rule: String,
    },
    #[error("{changed_by} provides `{name}` after {settled_by} settled it: its values would not be settled")]
    SettledThenChanged {
        settled_by: String,
        changed_by: String,
        name: String,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

fn illegal_text(target: &str, name: &str, entities: &[Entity], rule: &str) -> String {
    let rule = if rule.is_empty() {
        String::new()
    } else {
        format!(" ({rule})")
    };
    if entities.is_empty() {
        format!("`{target}` does not accept `{name}`{rule} after the pipeline")
    } else {
        format!("`{target}` does not accept `{name}`{rule} on {entities:?} after the pipeline")
    }
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
    /// With conservation diagnostics reporting ([`Conservation::Report`]): each measure whose losses differ from what
    /// the pass declared. Empty otherwise.
    pub conservation: Vec<Gap>,
    /// `Some(why)` when the pass failed (its run, its declarations, conservation or the verifier refused it): the
    /// record keeps what its log had noted until then, so its omissions and approximations still reach the report;
    /// `added` and `removed` are then empty (not checked).
    pub failed: Option<String>,
    /// In a part run ([`PassManager::run_part`]), for a pass that opts in ([`Pass::part_may_lack_inputs`]), its required
    /// names when none of them was in the part, so the pass did not run (its log counts it, [`SKIPPED`]); empty when it
    /// ran.
    pub skipped: Vec<String>,
}

/// The count a pass skipped in a part run gets in its log ([`PassManager::run_part`]), so a conversion's totals give the
/// skips per pass.
pub const SKIPPED: &str = "parts holding none of its required names, skipped";

/// Why the planner chose a pass: the name to lower (or the missing required name) and the chain depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanChoice {
    pub name: String,
    /// The pass with its options, as in the pipeline text.
    pub pass: String,
    pub depth: u32,
}

/// What a conversion did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub target: String,
    pub pipeline: String,
    pub plan: Vec<PlanChoice>,
    /// Passes the caller gave options for that the plan did not choose, by name.
    pub unused_options: Vec<String>,
    pub steps: Vec<StepReport>,
}

impl fmt::Display for RunReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "target {}: pipeline [{}]", self.target, self.pipeline)?;
        for c in &self.plan {
            writeln!(f, "  plan: `{}` by {} (depth {})", c.name, c.pass, c.depth)?;
        }
        if !self.unused_options.is_empty() {
            writeln!(f, "  options for passes not planned: {:?}", self.unused_options)?;
        }
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
    conservation: Conservation,
    pass_checks: bool,
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
        }
    }

    /// Check every pass as it runs: its effects entity by entity against its declarations (what it adds to or takes
    /// from an entity, what it despawns, the resources it writes: [`Pass::provides`], [`Pass::removes`],
    /// [`Pass::modifies`]), and the components' validators on what it leaves. Each costs a scan of the world per pass,
    /// so they are on in debug builds (the test suites) and off in release builds unless asked for, like the
    /// conservation diagnostics. What a pass's names do across the whole world is checked either way, and a conversion
    /// validates at its boundaries (what was read and loaded, the merged summaries, a part or group before writing)
    /// either way.
    pub fn with_pass_checks(mut self, on: bool) -> Self {
        self.pass_checks = on;
        self
    }

    pub fn pass_checks(&self) -> bool {
        self.pass_checks
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
        self.run_with(w, pipeline, report, instrument, false)
    }

    /// [`PassManager::run`] on one part of a conversion ([`crate::convert`]): a pass that opts in
    /// ([`Pass::part_may_lack_inputs`]), requires names and finds none of them in the part has nothing to do there and is
    /// skipped, recorded (its step's [`StepReport::skipped`], a [`SKIPPED`] count in its log). Any other pass whose
    /// required names are absent fails as in a whole run, as does an opted-in pass whose part holds some of them but not
    /// all; a pass that requires nothing always runs. Whole-world runs (a selection, a decision) stay strict.
    pub fn run_part(&self, w: &mut World, pipeline: &Pipeline, report: &mut RunReport) -> Result<()> {
        self.run_with(w, pipeline, report, &mut NoInstrument, true)
    }

    fn run_with(
        &self,
        w: &mut World,
        pipeline: &Pipeline,
        report: &mut RunReport,
        instrument: &mut dyn Instrument,
        part: bool,
    ) -> Result<()> {
        pipeline.check()?;
        for (index, p) in pipeline.passes.iter().enumerate() {
            let text = pass_text(p.as_ref());
            let absent = |r: &&str| !w.has_component_named(r) && !w.resource_names().contains(r);
            if part && p.part_may_lack_inputs() && !p.requires().is_empty() && p.requires().iter().all(absent)
            {
                let mut log = PassLog::default();
                log.count(SKIPPED, 1);
                report.steps.push(StepReport {
                    pass: text,
                    approximates: p.approximates(),
                    log,
                    added: Vec::new(),
                    removed: Vec::new(),
                    conservation: Vec::new(),
                    failed: None,
                    skipped: p.requires().iter().map(|r| (*r).to_owned()).collect(),
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
            match self.run_pass(w, p.as_ref(), &text, &mut log) {
                Ok(done) => {
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
    /// noted survives a failure.
    fn run_pass(&self, w: &mut World, p: &dyn Pass, text: &str, log: &mut PassLog) -> Result<StepReport> {
        for &r in p.requires() {
            if !w.has_component_named(r) && !w.resource_names().contains(&r) {
                return Err(Error::Missing {
                    pass: text.to_owned(),
                    name: r.to_owned(),
                });
            }
        }
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
        let ran = p.run(w, log);
        let rebuilds = w.take_rebuild_log();
        let effects = w.take_effects();
        ran.map_err(|message| Error::Pass {
            pass: text.to_owned(),
            message,
        })?;
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
            let (alive, dead): (Vec<Entity>, Vec<Entity>) = gone.into_iter().partition(|&e| w.is_alive(e));
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

    /// Choose the passes that take the world to what `target` accepts (see the module docs), in run order, each made
    /// with its options in `options`. A dynamic check is decided on the values the world has: values a planned pass
    /// produces are checked by [`PassManager::convert`], which plans again for those the target refuses.
    pub fn plan(
        &self,
        w: &World,
        target: &Target,
        mode: Mode,
        options: &PassOptions,
        report: &mut RunReport,
    ) -> Result<Pipeline> {
        self.plan_round(w, target, mode, options, &BTreeSet::new(), report)
            .map(|(pipeline, _)| pipeline)
    }

    /// [`PassManager::plan`] for one round of a conversion: carried-and-lowered names in `lowered_before` were lowered
    /// by an earlier round and need no lowering again unless a pass of this round provides more of them. Returns the
    /// pipeline and the carried-and-lowered names lowered once it has run.
    fn plan_round(
        &self,
        w: &World,
        target: &Target,
        mode: Mode,
        options: &PassOptions,
        lowered_before: &BTreeSet<String>,
        report: &mut RunReport,
    ) -> Result<(Pipeline, BTreeSet<String>)> {
        let planned = self.plan_round_unchecked(w, target, mode, options, lowered_before, report)?;
        // a pass with required options runs only with the caller's: the planner doesn't choose them
        if let Some(p) = planned
            .0
            .passes
            .iter()
            .find(|p| self.registry.requires_options(p.name()) && !options.contains_key(p.name()))
        {
            return Err(Error::Options {
                pass: p.name().to_owned(),
                message: "the plan needs it and it has required options: give them (PassOptions)".into(),
            });
        }
        Ok(planned)
    }

    fn plan_round_unchecked(
        &self,
        w: &World,
        target: &Target,
        mode: Mode,
        options: &PassOptions,
        lowered_before: &BTreeSet<String>,
        report: &mut RunReport,
    ) -> Result<(Pipeline, BTreeSet<String>)> {
        let passes = self.registry.instances(options)?;
        let present: BTreeSet<&str> = w.component_names().into_iter().collect();
        let resources: BTreeSet<&str> = w.resource_names().into_iter().collect();
        let have = |n: &str| present.contains(n) || resources.contains(n);
        // what each name the world has, or a pass provides or lowers, needs
        let mut needs: BTreeMap<&str, Need> = BTreeMap::new();
        let declared = passes
            .iter()
            .flat_map(|p| p.provides().iter().chain(p.legalizes()).copied());
        // resources the target has a value rule for are planned like components
        let ruled: BTreeSet<&str> = resources
            .iter()
            .copied()
            .filter(|n| target.resource_rules.contains_key(*n))
            .collect();
        for n in present.iter().chain(&ruled).copied().chain(declared) {
            needs.entry(n).or_insert_with(|| target.need(w, n, mode));
        }
        let need = |n: &str| needs.get(n).copied().unwrap_or_else(|| target.need(w, n, mode));
        // what a pass provides needs nothing more when it is legal, a root of its own (failing values the world
        // already has), or lowered by the pass itself
        let settled = |p: &dyn Pass, n: &str| {
            let need = need(n);
            matches!(need, Need::Nothing | Need::Fix) || need.met_by(p, n)
        };

        // depth[name]: chain depth to lower a name; supply[name]: to provide a missing required name. A pass has a
        // cost as a provider (`cost`) and one as the lowering of each name it lowers (`lowering`): as a lowering it
        // does not pay for the name it lowers, which is there whenever it is to be lowered (whoever provides it pays)
        let mut depth: BTreeMap<&str, u32> = BTreeMap::new();
        let mut supply: BTreeMap<&str, u32> = BTreeMap::new();
        let mut cost = vec![INF; passes.len()];
        let mut lowering: BTreeMap<(usize, &str), u32> = BTreeMap::new();
        let plus_one = |worst: u32| if worst == INF { INF } else { worst + 1 };
        loop {
            let mut changed = false;
            for (i, p) in passes.iter().enumerate() {
                let p = p.as_ref();
                let provided = p
                    .provides()
                    .iter()
                    .map(|&n| {
                        if settled(p, n) {
                            0
                        } else {
                            *depth.get(n).unwrap_or(&INF)
                        }
                    })
                    .max()
                    .unwrap_or(0);
                let required = |except: Option<&str>| {
                    p.requires()
                        .iter()
                        .filter(|&&r| Some(r) != except)
                        .map(|&r| {
                            if have(r) {
                                0
                            } else {
                                *supply.get(r).unwrap_or(&INF)
                            }
                        })
                        .max()
                        .unwrap_or(0)
                };
                let c = plus_one(provided.max(required(None)));
                let lowers: Vec<(&str, u32)> = p
                    .legalizes()
                    .iter()
                    .filter(|&&n| need(n).met_by(p, n))
                    .map(|&n| (n, plus_one(provided.max(required(Some(n))))))
                    .collect();
                if c < cost[i] {
                    cost[i] = c;
                    changed = true;
                    for &n in p.provides() {
                        if c < *supply.get(n).unwrap_or(&INF) {
                            supply.insert(n, c);
                        }
                    }
                }
                for (n, c) in lowers {
                    if c < *lowering.get(&(i, n)).unwrap_or(&INF) {
                        lowering.insert((i, n), c);
                        changed = true;
                        if c < *depth.get(n).unwrap_or(&INF) {
                            depth.insert(n, c);
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // pass i's depth as the lowering of `name` (lowering) or as its provider (supply); INF when it is neither
        let depth_as = |i: usize, name: &str, lowering_it: bool| -> u32 {
            if lowering_it {
                *lowering.get(&(i, name)).unwrap_or(&INF)
            } else if passes[i].provides().contains(&name) {
                cost[i]
            } else {
                INF
            }
        };
        // the cheapest pass for a name: smallest depth, then highest priority, then name
        let best = |name: &str, lowering_it: bool| -> Option<usize> {
            (0..passes.len())
                .filter(|&i| depth_as(i, name, lowering_it) != INF)
                .min_by_key(|&i| {
                    (
                        depth_as(i, name, lowering_it),
                        -passes[i].priority(),
                        passes[i].name(),
                    )
                })
        };
        // whether pass i lowers `name` (lowering) or provides it (supply)
        let serves = |i: usize, name: &str, lowering_it: bool| {
            if lowering_it {
                need(name).met_by(passes[i].as_ref(), name)
            } else {
                passes[i].provides().contains(&name)
            }
        };

        // the names the world has that must be lowered
        let roots: Vec<&str> = present
            .iter()
            .chain(&ruled)
            .copied()
            .filter(|&n| match need(n) {
                Need::Nothing => false,
                Need::Lower => !lowered_before.contains(n),
                Need::Remove | Need::Fix => true,
            })
            .collect();
        let mut chosen: BTreeSet<usize> = BTreeSet::new();
        // the obligations that made a choice, in choice order
        let mut choices: Vec<(&str, bool)> = Vec::new();
        let mut todo: Vec<(&str, bool)> = roots.iter().rev().map(|&n| (n, true)).collect();
        while let Some((name, lowering_it)) = todo.pop() {
            if chosen.iter().any(|&i| serves(i, name, lowering_it)) {
                continue;
            }
            let Some(i) = best(name, lowering_it) else {
                return Err(Error::NoLowering {
                    detail: self.why_not(&passes, name, lowering_it, need(name), &have),
                    name: name.to_owned(),
                    target: target.name.clone(),
                });
            };
            let p = passes[i].as_ref();
            choices.push((name, lowering_it));
            chosen.insert(i);
            for &r in p.requires().iter().rev() {
                if !have(r) {
                    todo.push((r, false));
                }
            }
            for &n in p.provides().iter().rev() {
                if !settled(p, n) {
                    todo.push((n, true));
                }
            }
        }

        // what a set of passes must provide or lower: the roots, the requirements the world lacks, and what they
        // provide that is not settled
        let obligations = |set: &BTreeSet<usize>| -> BTreeSet<(&str, bool)> {
            let mut out: BTreeSet<(&str, bool)> = roots.iter().map(|&n| (n, true)).collect();
            for &i in set {
                let p = passes[i].as_ref();
                out.extend(p.requires().iter().filter(|&&r| !have(r)).map(|&r| (r, false)));
                out.extend(
                    p.provides()
                        .iter()
                        .filter(|&&n| !settled(p, n))
                        .map(|&n| (n, true)),
                );
            }
            out
        };
        let covered = |set: &BTreeSet<usize>| {
            obligations(set)
                .iter()
                .all(|&(n, lowering_it)| set.iter().any(|&i| serves(i, n, lowering_it)))
        };
        // a chosen pass whose work the others cover is left out, the least preferred first, until none is
        // redundant (a pass chosen for one name may turn out to be covered by one chosen later for another)
        loop {
            let mut worst_first: Vec<usize> = chosen.iter().copied().collect();
            worst_first.sort_by_key(|&i| (cost[i], -passes[i].priority(), passes[i].name()));
            worst_first.reverse();
            let redundant = worst_first.into_iter().find(|&i| {
                let mut rest = chosen.clone();
                rest.remove(&i);
                covered(&rest)
            });
            match redundant {
                Some(i) => {
                    chosen.remove(&i);
                }
                None => break,
            }
        }
        // why each pass is in the plan: the obligations that made a choice, each with the pass that meets it now
        let left_to_meet = obligations(&chosen);
        for (name, lowering_it) in choices {
            if !left_to_meet.contains(&(name, lowering_it)) {
                continue;
            }
            let by = chosen
                .iter()
                .copied()
                .filter(|&i| serves(i, name, lowering_it))
                .min_by_key(|&i| {
                    (
                        depth_as(i, name, lowering_it),
                        -passes[i].priority(),
                        passes[i].name(),
                    )
                })
                .expect("every obligation is met");
            report.plan.push(PlanChoice {
                name: name.to_owned(),
                pass: pass_text(passes[by].as_ref()),
                depth: depth_as(by, name, lowering_it),
            });
        }

        let order = order_passes(&passes, &chosen)?;
        // check the order by simulating the declarations: what is required is there, what must go has gone, and
        // what must be lowered was lowered after the last pass that provided more of it
        let mut names: BTreeSet<&str> = present.iter().chain(&ruled).copied().collect();
        let mut lowered: BTreeSet<&str> = present
            .iter()
            .copied()
            .filter(|n| lowered_before.contains(*n))
            .collect();
        for &i in &order {
            let p = passes[i].as_ref();
            if let Some(r) = p
                .requires()
                .iter()
                .find(|r| !names.contains(*r) && !resources.contains(*r))
            {
                return Err(Error::Missing {
                    pass: p.name().to_owned(),
                    name: (*r).to_owned(),
                });
            }
            for n in p.removes() {
                names.remove(n);
            }
            for &n in p.provides() {
                names.insert(n);
                if need(n) == Need::Lower {
                    lowered.remove(n);
                }
            }
            for &n in p.legalizes() {
                if need(n).met_by(p, n) {
                    lowered.insert(n);
                }
            }
        }
        let left = names.iter().find(|&&n| match need(n) {
            Need::Nothing => false,
            Need::Remove => true,
            Need::Fix | Need::Lower => !lowered.contains(n),
        });
        if let Some(n) = left {
            return Err(Error::NoLowering {
                name: (*n).to_owned(),
                target: target.name.clone(),
                detail: " (the planned pipeline leaves it)".to_owned(),
            });
        }
        report.unused_options = options
            .keys()
            .filter(|n| !chosen.iter().any(|&i| passes[i].name() == n.as_str()))
            .cloned()
            .collect();
        let mut made: Vec<Option<Box<dyn Pass>>> = passes.into_iter().map(Some).collect();
        let pipeline = Pipeline {
            passes: order
                .into_iter()
                .map(|i| made[i].take().expect("each pass is ordered once"))
                .collect(),
        };
        report.target = target.name.clone();
        report.pipeline = pipeline.to_string();
        let lowered = lowered
            .into_iter()
            .filter(|n| need(n) == Need::Lower)
            .map(str::to_owned)
            .collect();
        Ok((pipeline, lowered))
    }

    /// The missing link behind a failed plan, for the error message.
    fn why_not(
        &self,
        passes: &[Box<dyn Pass>],
        name: &str,
        lowering: bool,
        need: Need,
        have: &dyn Fn(&str) -> bool,
    ) -> String {
        let candidates: Vec<String> = passes
            .iter()
            .map(|p| p.as_ref())
            .filter(|p| {
                if lowering {
                    p.legalizes().contains(&name)
                } else {
                    p.provides().contains(&name)
                }
            })
            .map(|p| {
                if lowering && !need.met_by(p, name) {
                    return match need {
                        Need::Remove => format!("{} lowers it and keeps it", p.name()),
                        _ => format!("{} lowers it but neither removes nor rewrites it", p.name()),
                    };
                }
                let missing: Vec<&str> = p.requires().iter().copied().filter(|r| !have(r)).collect();
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

    /// Plan (with `options` for the passes it chooses), run and check: verify the reader's world, run the planned
    /// pipeline (verifying after every pass), then check that the target accepts every component and verify once more
    /// before the writer. A dynamic check sees values only once they exist: while the target refuses values the
    /// pipeline produced, convert plans again on the world as it is and runs that too. A round that plans a pipeline
    /// an earlier round ran makes no progress, and the refused values are an error. The report holds every round: its
    /// pipeline is what ran, in order.
    pub fn convert(
        &self,
        w: &mut World,
        target: &Target,
        mode: Mode,
        options: &PassOptions,
    ) -> Result<RunReport> {
        self.convert_instrumented(w, target, mode, options, &mut NoInstrument)
    }

    /// [`PassManager::convert`], calling `instrument` around every pass of the planned pipeline.
    pub fn convert_instrumented(
        &self,
        w: &mut World,
        target: &Target,
        mode: Mode,
        options: &PassOptions,
        instrument: &mut dyn Instrument,
    ) -> Result<RunReport> {
        self.verify(w, "the reader")?;
        let mut report = RunReport::default();
        let mut ran: Vec<String> = Vec::new();
        let mut lowered = BTreeSet::new();
        let mut used: BTreeSet<String> = BTreeSet::new();
        let refused = |w: &World| {
            w.component_names()
                .into_iter()
                .map(|name| (name.to_owned(), target.illegal_entities(w, name, mode)))
                .find(|(_, bad)| !bad.is_empty())
                .or_else(|| {
                    target
                        .refused_resources(w)
                        .first()
                        .map(|n| ((*n).to_owned(), Vec::new()))
                })
        };
        loop {
            let mut round = RunReport::default();
            let (pipeline, now_lowered) = self.plan_round(w, target, mode, options, &lowered, &mut round)?;
            let text = pipeline.to_string();
            // a round planning a pipeline an earlier round ran makes no progress (and rounds cannot cycle)
            if pipeline.passes.is_empty() || ran.contains(&text) {
                break;
            }
            used.extend(pipeline.passes.iter().map(|p| p.name().to_owned()));
            report.plan.extend(round.plan);
            self.run_instrumented(w, &pipeline, &mut report, instrument)?;
            ran.push(text);
            lowered = now_lowered;
            if refused(w).is_none() {
                break;
            }
        }
        report.target = target.name.clone();
        report.pipeline = ran.join(", ");
        report.unused_options = options.keys().filter(|n| !used.contains(*n)).cloned().collect();
        if let Some((name, bad)) = refused(w) {
            return Err(Error::Illegal {
                target: target.name.clone(),
                rule: target.rule_words(&name).unwrap_or_default().to_owned(),
                name,
                entities: bad.into_iter().take(10).collect(),
            });
        }
        self.verify(w, &format!("the pipeline, before {}", target.name))?;
        Ok(report)
    }
}

/// Run order of the chosen passes. A pass reads the names it requires and those it legalizes but keeps. For each
/// name: a pass that provides it runs before one that reads, removes or modifies it; one that modifies it (without
/// providing it) runs before one that takes it as it is (reads or removes it, neither providing nor modifying it); one
/// that reads it runs before one that only removes it (neither providing nor modifying it). Between two passes that
/// both provide a name, its edge is weak: it gives way to any edge the other way (two passes that each add to and read
/// one name have no order between them). Ties by name.
fn order_passes(passes: &[Box<dyn Pass>], chosen: &BTreeSet<usize>) -> Result<Vec<usize>> {
    let ids: Vec<usize> = chosen.iter().copied().collect();
    let reads = |p: &dyn Pass, n: &&str| {
        p.requires().contains(n) || (p.legalizes().contains(n) && !p.removes().contains(n))
    };
    let uses = |p: &dyn Pass, n: &&str| reads(p, n) || p.removes().contains(n) || p.modifies().contains(n);
    // a provider of a name before a pass that uses it and provides it too
    let weak = |a: usize, b: usize| -> bool {
        let (pa, pb) = (passes[a].as_ref(), passes[b].as_ref());
        pa.provides()
            .iter()
            .any(|n| uses(pb, n) && pb.provides().contains(n))
    };
    let strong = |a: usize, b: usize| -> bool {
        let (pa, pb) = (passes[a].as_ref(), passes[b].as_ref());
        // neither provides nor modifies: what it reads or removes is the value as it is
        let as_is = |p: &dyn Pass, n: &&str| !p.provides().contains(n) && !p.modifies().contains(n);
        pa.provides()
            .iter()
            .any(|n| uses(pb, n) && !pb.provides().contains(n))
            || pa
                .provides()
                .iter()
                .any(|n| pb.settles().contains(n) && !pa.settles().contains(n))
            || pa.modifies().iter().any(|n| {
                !pa.provides().contains(n) && as_is(pb, n) && (reads(pb, n) || pb.removes().contains(n))
            })
            || pa
                .requires()
                .iter()
                .chain(pa.legalizes())
                .any(|n| reads(pa, n) && pb.removes().contains(n) && as_is(pb, n))
    };
    let before = |a: usize, b: usize| strong(a, b) || (weak(a, b) && !strong(b, a) && !weak(b, a));
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

    /// A pass taking in a function's log keeps everything it reported: counts, approximations, omissions and drops
    /// add up, removals, placed entities and their sources join.
    /// A target's rules read back by kind, its checks left out; carrying unknown names and its checked resources too.
    #[test]
    fn a_targets_rules_read_back() {
        let t = Target::new("t")
            .legal("toy.pbr")
            .illegal("toy.layered")
            .carry_unknown();
        assert_eq!(
            t.rules(),
            [("toy.layered", Rule::Illegal), ("toy.pbr", Rule::Legal)]
        );
        assert!(t.carries_unknown());
        assert!(t.checked_resources().is_empty());
        assert!(!Target::new("u").carries_unknown());
    }

    #[test]
    fn absorb_keeps_every_part_of_the_log() {
        let mut w = World::new();
        let (a, b, source) = (w.spawn(), w.spawn(), w.spawn());
        let mut log = PassLog::default();
        log.count("kept", 1);
        log.approximate("rounded", 2);
        log.place(a);
        let mut other = PassLog::default();
        other.count("kept", 3);
        other.count("made", 1);
        other.approximate("rounded", 1);
        other.omit("no form", 4);
        other.record_removed("toy.hat");
        other.place_from(b, source);
        other.dropped(Measure::Placements, "left out", 2);
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
        let report = m
            .convert(&mut w, &gltf(), Mode::Partial, &PassOptions::new())
            .unwrap();
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
                m.plan(&world(), &gltf(), Mode::Partial, &PassOptions::new(), &mut r)
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
            m.plan(&world(), &gltf(), Mode::Partial, &PassOptions::new(), &mut r)
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
            m.plan(&w, &gltf(), Mode::Partial, &PassOptions::new(), &mut r)
                .unwrap()
                .to_string(),
            "bake"
        );
    }

    #[test]
    fn full_conversion_names_the_missing_link() {
        let m = PassManager::new(registry(false));
        let mut w = world();
        let err = m
            .convert(&mut w, &gltf(), Mode::Full, &PassOptions::new())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `game.thing` to anything `gltf` accepts"
        );
        // carrying unknown names makes it pass
        let mut w = world();
        m.convert(&mut w, &gltf().carry_unknown(), Mode::Full, &PassOptions::new())
            .unwrap();
        // a lowering whose requirement nothing provides
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Layered(1));
        let err = m
            .convert(&mut w, &gltf(), Mode::Partial, &PassOptions::new())
            .unwrap_err();
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
        let report = m.convert(&mut w, &t, Mode::Full, &PassOptions::new()).unwrap();
        assert_eq!(report.pipeline, "");
        assert!(w.has_component_named("toy.file"));
        let mut w = world();
        let e = w.entities().next().unwrap();
        w.insert(e, File(99)); // fails the check: lowered
        let report = m.convert(&mut w, &t, Mode::Full, &PassOptions::new()).unwrap();
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
        PassManager::new(PassRegistry::new()).run(
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

    /// Makes odd meshes even in place: the only way to a target whose check wants even meshes. Without `legalizes`
    /// it only edits meshes (`touch`).
    struct Evens {
        legalizes: bool,
    }
    impl Pass for Evens {
        fn name(&self) -> &'static str {
            if self.legalizes { "evens" } else { "touch" }
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.mesh"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["toy.mesh"]
        }
        fn legalizes(&self) -> &[&'static str] {
            if self.legalizes { &["toy.mesh"] } else { &[] }
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

    /// toy.layered -> toy.pbr, keeping toy.layered (a lowering that keeps its source).
    struct LowerKeep;
    impl Pass for LowerKeep {
        fn name(&self) -> &'static str {
            "lower-keep"
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.layered"]
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.pbr"]
        }
        fn legalizes(&self) -> &[&'static str] {
            &["toy.layered"]
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            let ls: Vec<(Entity, u32)> = w.iter::<Layered>().map(|(e, l)| (e, l.0)).collect();
            for (e, n) in ls {
                w.insert(e, Pbr(n));
            }
            Ok(())
        }
    }

    fn mesh_world(n: u32) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh(n));
        w
    }

    #[test]
    fn a_pass_that_legalizes_in_place_lowers_failing_values() {
        let even = Target::new("even").dynamic_typed::<Mesh>(|m| m.0 % 2 == 0);
        let mut r = PassRegistry::new();
        r.register("evens", |_| Ok(Box::new(Evens { legalizes: true })));
        r.register("touch", |_| Ok(Box::new(Evens { legalizes: false })));
        let m = PassManager::new(r);
        let mut w = mesh_world(3);
        let report = m
            .convert(&mut w, &even, Mode::Partial, &PassOptions::new())
            .unwrap();
        // touch modifies toy.mesh too, but does not say it makes it legal
        assert_eq!(report.pipeline, "evens");
        assert_eq!(w.iter::<Mesh>().map(|(_, m)| m.0).collect::<Vec<_>>(), [4]);
        // legal values need nothing
        let report = m
            .convert(&mut mesh_world(2), &even, Mode::Partial, &PassOptions::new())
            .unwrap();
        assert_eq!(report.pipeline, "");
        // a pass that lowers a dynamic name but keeps its values as they are cannot fix them
        let mut r = PassRegistry::new();
        r.register("keeps", |_| {
            Ok(Box::new(Decl {
                name: "keeps",
                requires: &["toy.mesh"],
                provides: &["toy.pbr"],
                removes: &[],
                modifies: &[],
                legalizes: &["toy.mesh"],
            }))
        });
        let err = PassManager::new(r)
            .convert(&mut mesh_world(3), &even, Mode::Partial, &PassOptions::new())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `toy.mesh` to anything `even` accepts: keeps lowers it but neither removes nor \
             rewrites it"
        );
    }

    #[test]
    fn a_lowering_that_keeps_its_source() {
        let mut r = PassRegistry::new();
        r.register("lower-keep", |_| Ok(Box::new(LowerKeep)));
        let m = PassManager::new(r);
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Layered(4));
        let gltf = || Target::new("gltf").legal("toy.pbr");
        // carried and lowered: the lowering runs and the source stays
        let mut lowered = w.clone();
        let report = m
            .convert(
                &mut lowered,
                &gltf().lower_and_carry("toy.layered"),
                Mode::Full,
                &PassOptions::new(),
            )
            .unwrap();
        assert_eq!(report.pipeline, "lower-keep");
        assert_eq!(lowered.component_names(), ["toy.layered", "toy.pbr"]);
        // only carried: nothing to do
        let report = m
            .convert(
                &mut w.clone(),
                &gltf().carry("toy.layered"),
                Mode::Full,
                &PassOptions::new(),
            )
            .unwrap();
        assert_eq!(report.pipeline, "");
        // illegal: it must go, and the lowering keeps it
        let err = m
            .convert(
                &mut w,
                &gltf().illegal("toy.layered"),
                Mode::Full,
                &PassOptions::new(),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `toy.layered` to anything `gltf` accepts: lower-keep lowers it and keeps it"
        );
    }

    #[test]
    fn a_name_a_planned_pass_provides_is_lowered_after_it() {
        // load provides toy.mesh, which the target carries but draws only lowered
        let mut r = PassRegistry::new();
        r.register("load", load);
        r.register("lower-mesh", |_| {
            Ok(Box::new(Decl {
                name: "lower-mesh",
                requires: &["toy.mesh"],
                provides: &["toy.pbr"],
                removes: &[],
                modifies: &[],
                legalizes: &["toy.mesh"],
            }))
        });
        let t = Target::new("t")
            .illegal("toy.file")
            .lower_and_carry("toy.mesh")
            .legal("toy.pbr");
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, File(1));
        let mut report = RunReport::default();
        let p = PassManager::new(r)
            .plan(&w, &t, Mode::Full, &PassOptions::new(), &mut report)
            .unwrap();
        assert_eq!(p.to_string(), "load, lower-mesh");
        assert_eq!(report.plan[1].name, "toy.mesh");
    }

    #[test]
    fn a_removal_that_is_a_side_effect_is_not_a_lowering() {
        // a-weld drops the file link of the meshes it rewrites; name order prefers it, but it lowers only toy.mesh
        let mut r = PassRegistry::new();
        r.register("load", load);
        r.register("a-weld", |_| {
            Ok(Box::new(Decl {
                name: "a-weld",
                requires: &["toy.mesh"],
                provides: &[],
                removes: &["toy.file"],
                modifies: &["toy.mesh"],
                legalizes: &["toy.mesh"],
            }))
        });
        let mut w = world();
        let e = w.entities().next().unwrap();
        w.insert(e, Mesh(2));
        let t = Target::new("t").illegal("toy.file");
        let mut report = RunReport::default();
        let p = PassManager::new(r)
            .plan(&w, &t, Mode::Partial, &PassOptions::new(), &mut report)
            .unwrap();
        assert_eq!(p.to_string(), "load");
    }

    /// Lowers toy.layered only when made with `lowers=true`: its declarations follow its options.
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
            let es: Vec<Entity> = w.iter::<Layered>().map(|(e, _)| e).collect();
            for e in es {
                w.remove::<Layered>(e);
            }
            Ok(())
        }
    }

    #[test]
    fn plan_makes_passes_with_the_callers_options() {
        let m = PassManager::new(registry(false));
        let options = |o: &[(&str, &str, &str)]| -> PassOptions {
            let mut all = PassOptions::new();
            for (pass, k, v) in o {
                all.entry(pass.to_string())
                    .or_default()
                    .insert(k.to_string(), v.to_string());
            }
            all
        };
        let mut w = world();
        let report = m
            .convert(
                &mut w,
                &gltf(),
                Mode::Partial,
                &options(&[("bake", "size", "512")]),
            )
            .unwrap();
        assert_eq!(report.pipeline, "load, bake{size=512}");
        assert_eq!(report.plan[1].pass, "bake{size=512}");
        assert!(report.unused_options.is_empty());
        // options for a pass the plan does not choose are reported
        let mut report = RunReport::default();
        let o = options(&[("bake", "size", "512"), ("sneaky", "x", "1")]);
        // sneaky takes any option (its factory ignores them)
        m.plan(&world(), &gltf(), Mode::Partial, &o, &mut report).unwrap();
        assert_eq!(report.unused_options, ["sneaky"]);
        assert!(
            report
                .to_string()
                .contains("options for passes not planned: [\"sneaky\"]")
        );
        // options for a pass that is not registered, and bad options, are errors
        let mut report = RunReport::default();
        let err = m
            .plan(
                &world(),
                &gltf(),
                Mode::Partial,
                &options(&[("nope", "a", "b")]),
                &mut report,
            )
            .unwrap_err();
        assert!(matches!(err, Error::UnknownPass(n) if n == "nope"));
        let err = m
            .plan(
                &world(),
                &gltf(),
                Mode::Partial,
                &options(&[("bake", "size", "big")]),
                &mut report,
            )
            .unwrap_err();
        assert!(matches!(err, Error::Options { .. }), "{err}");
        // declarations come from the pass made with its options
        let mut r = PassRegistry::new();
        r.register("optional", |o| {
            Ok(Box::new(Optional {
                lowers: o.get("lowers").is_some_and(|v| v == "true"),
            }))
        });
        let m = PassManager::new(r);
        let t = Target::new("t").illegal("toy.layered").carry_unknown();
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Layered(1));
        assert!(matches!(
            m.convert(&mut w.clone(), &t, Mode::Full, &PassOptions::new()),
            Err(Error::NoLowering { .. })
        ));
        let report = m
            .convert(
                &mut w,
                &t,
                Mode::Full,
                &options(&[("optional", "lowers", "true")]),
            )
            .unwrap();
        assert_eq!(report.pipeline, "optional{lowers=true}");
    }

    struct Decl {
        name: &'static str,
        requires: &'static [&'static str],
        provides: &'static [&'static str],
        removes: &'static [&'static str],
        modifies: &'static [&'static str],
        legalizes: &'static [&'static str],
    }
    impl Pass for Decl {
        fn name(&self) -> &'static str {
            self.name
        }
        fn requires(&self) -> &[&'static str] {
            self.requires
        }
        fn provides(&self) -> &[&'static str] {
            self.provides
        }
        fn removes(&self) -> &[&'static str] {
            self.removes
        }
        fn modifies(&self) -> &[&'static str] {
            self.modifies
        }
        fn legalizes(&self) -> &[&'static str] {
            self.legalizes
        }
        fn run(&self, _: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            Ok(())
        }
    }

    fn order(passes: Vec<Decl>) -> Vec<&'static str> {
        let passes: Vec<Box<dyn Pass>> = passes.into_iter().map(|p| Box::new(p) as Box<dyn Pass>).collect();
        let all: BTreeSet<usize> = (0..passes.len()).collect();
        order_passes(&passes, &all)
            .unwrap()
            .into_iter()
            .map(|i| passes[i].name())
            .collect()
    }

    #[test]
    fn a_modifier_runs_between_providers_and_consumers() {
        fn decl(
            name: &'static str,
            requires: &'static [&'static str],
            provides: &'static [&'static str],
            removes: &'static [&'static str],
            modifies: &'static [&'static str],
        ) -> Decl {
            Decl {
                name,
                requires,
                provides,
                removes,
                modifies,
                legalizes: removes,
            }
        }
        // names chosen so that name order alone is wrong
        let bake = decl(
            "a-bake",
            &["toy.mesh", "toy.layered"],
            &["toy.pbr"],
            &["toy.layered"],
            &[],
        );
        let load = decl("m-load", &["toy.file"], &["toy.mesh"], &["toy.file"], &[]);
        let weld = decl("z-weld", &["toy.mesh"], &[], &[], &["toy.mesh"]);
        assert_eq!(order(vec![bake, load, weld]), ["m-load", "z-weld", "a-bake"]);
        // a pass that lowers a mesh away runs after the mesh is welded too
        let drop = decl("a-drop", &[], &[], &["toy.mesh"], &[]);
        let weld = decl("z-weld", &["toy.mesh"], &[], &[], &["toy.mesh"]);
        assert_eq!(order(vec![drop, weld]), ["z-weld", "a-drop"]);
        // two passes modifying one name keep name order
        let a = decl("a-tint", &["toy.mesh"], &[], &[], &["toy.mesh"]);
        let weld = decl("z-weld", &["toy.mesh"], &[], &[], &["toy.mesh"]);
        assert_eq!(order(vec![weld, a]), ["a-tint", "z-weld"]);
        // a pass that lowers a name and keeps it reads it: after its modifier
        let keep = Decl {
            name: "a-keep",
            requires: &[],
            provides: &["toy.pbr"],
            removes: &[],
            modifies: &[],
            legalizes: &["toy.mesh"],
        };
        let weld = decl("z-weld", &["toy.mesh"], &[], &[], &["toy.mesh"]);
        assert_eq!(order(vec![keep, weld]), ["z-weld", "a-keep"]);
    }

    #[test]
    fn values_a_planned_pass_produces_are_lowered_after_it() {
        // load makes Mesh(3) from File(3); the target wants even meshes: only once the mesh exists does the check
        // see it, so convert plans again on the world the first pipeline produced
        let mut r = PassRegistry::new();
        r.register("load", load);
        r.register("evens", |_| Ok(Box::new(Evens { legalizes: true })));
        let m = PassManager::new(r);
        let t = Target::new("even")
            .illegal("toy.file")
            .dynamic_typed::<Mesh>(|m| m.0 % 2 == 0);
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, File(3));
        let report = m.convert(&mut w, &t, Mode::Partial, &PassOptions::new()).unwrap();
        assert_eq!(report.pipeline, "load, evens");
        assert_eq!(w.get::<Mesh>(e), Some(&Mesh(4)));
        // a produced value the check accepts needs no second round
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, File(2));
        let report = m.convert(&mut w, &t, Mode::Partial, &PassOptions::new()).unwrap();
        assert_eq!(report.pipeline, "load");
        // a produced value no pass can fix is refused after the round that made it
        let mut r = PassRegistry::new();
        r.register("load", load);
        let err = PassManager::new(r)
            .convert(&mut w_with_file(5), &t, Mode::Partial, &PassOptions::new())
            .unwrap_err();
        assert!(
            matches!(err, Error::NoLowering { ref name, .. } if name == "toy.mesh"),
            "{err}"
        );
    }

    fn w_with_file(n: u32) -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, File(n));
        w
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Ta;
    impl Component for Ta {
        const NAME: &'static str = "toy.a";
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Tb;
    impl Component for Tb {
        const NAME: &'static str = "toy.b";
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Tx(u32);
    impl Component for Tx {
        const NAME: &'static str = "toy.x";
    }

    /// Removes toy.a (and toy.b with `both`); with `requires`, requires what it removes.
    struct Strip {
        name: &'static str,
        both: bool,
        requires: bool,
    }
    impl Pass for Strip {
        fn name(&self) -> &'static str {
            self.name
        }
        fn requires(&self) -> &[&'static str] {
            match (self.requires, self.both) {
                (false, _) => &[],
                (true, false) => &["toy.a"],
                (true, true) => &["toy.a", "toy.b"],
            }
        }
        fn removes(&self) -> &[&'static str] {
            if self.both {
                &["toy.a", "toy.b"]
            } else {
                &["toy.a"]
            }
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            for e in w.entities().collect::<Vec<_>>() {
                w.remove::<Ta>(e);
                if self.both {
                    w.remove::<Tb>(e);
                }
            }
            Ok(())
        }
    }

    fn ab_world() -> World {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Ta);
        w.insert(e, Tb);
        w
    }

    #[test]
    fn a_chosen_pass_another_choice_covers_is_left_out() {
        let t = Target::new("t").illegal("toy.a").illegal("toy.b");
        for requires in [false, true] {
            let mut r = PassRegistry::new();
            // name order chooses a-only for toy.a first; z-both, chosen for toy.b, also takes toy.a
            r.register("a-only", if requires { strip_a_req } else { strip_a });
            r.register("z-both", if requires { strip_both_req } else { strip_both });
            let mut w = ab_world();
            let report = PassManager::new(r)
                .convert(&mut w, &t, Mode::Partial, &PassOptions::new())
                .unwrap();
            assert_eq!(report.pipeline, "z-both", "requires {requires}");
            assert!(
                report.plan.iter().all(|c| c.pass == "z-both"),
                "{:?}",
                report.plan
            );
            assert!(w.component_names().is_empty());
        }
    }
    fn strip_a(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Strip {
            name: "a-only",
            both: false,
            requires: false,
        }))
    }
    fn strip_a_req(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Strip {
            name: "a-only",
            both: false,
            requires: true,
        }))
    }
    fn strip_both(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Strip {
            name: "z-both",
            both: true,
            requires: false,
        }))
    }
    fn strip_both_req(_: &Options) -> std::result::Result<Box<dyn Pass>, String> {
        Ok(Box::new(Strip {
            name: "z-both",
            both: true,
            requires: true,
        }))
    }

    /// Requires toy.x, removes its letter (toy.a or toy.b) and adds one to every toy.x, which it provides and
    /// modifies.
    struct Bump {
        name: &'static str,
        b: bool,
    }
    impl Pass for Bump {
        fn name(&self) -> &'static str {
            self.name
        }
        fn requires(&self) -> &[&'static str] {
            &["toy.x"]
        }
        fn provides(&self) -> &[&'static str] {
            &["toy.x"]
        }
        fn modifies(&self) -> &[&'static str] {
            &["toy.x"]
        }
        fn removes(&self) -> &[&'static str] {
            if self.b { &["toy.b"] } else { &["toy.a"] }
        }
        fn run(&self, w: &mut World, _: &mut PassLog) -> std::result::Result<(), String> {
            for e in w.entities().collect::<Vec<_>>() {
                if self.b {
                    w.remove::<Tb>(e);
                } else {
                    w.remove::<Ta>(e);
                }
            }
            if let Some(mut c) = w.column_mut::<Tx>() {
                for (_, x) in c.iter_mut() {
                    x.0 += 1;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn two_passes_that_provide_and_modify_one_name_do_not_form_a_cycle() {
        let t = Target::new("t").illegal("toy.a").illegal("toy.b").legal("toy.x");
        let bump_a: PassFactory = |_| {
            Ok(Box::new(Bump {
                name: "bump-a",
                b: false,
            }))
        };
        let bump_b: PassFactory = |_| {
            Ok(Box::new(Bump {
                name: "bump-b",
                b: true,
            }))
        };
        for order in [
            [("bump-a", bump_a), ("bump-b", bump_b)],
            [("bump-b", bump_b), ("bump-a", bump_a)],
        ] {
            let mut r = PassRegistry::new();
            for (name, f) in order {
                r.register(name, f);
            }
            let mut w = ab_world();
            let e = w.entities().next().unwrap();
            w.insert(e, Tx(0));
            let report = PassManager::new(r)
                .convert(&mut w, &t, Mode::Partial, &PassOptions::new())
                .unwrap();
            assert_eq!(report.pipeline, "bump-a, bump-b");
            assert_eq!(w.get::<Tx>(e), Some(&Tx(2)));
        }
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
        fn legalizes(&self) -> &[&'static str] {
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
    fn a_resource_value_rule_is_planned_and_checked() {
        let mut r = PassRegistry::new();
        r.register("classify", |_| Ok(Box::new(Classify)));
        let m = PassManager::new(r);
        let mut w = table_world(vec![Some(3), None]);
        let report = m
            .convert(&mut w, &classified(), Mode::Partial, &PassOptions::new())
            .unwrap();
        assert_eq!(report.pipeline, "classify");
        assert_eq!(w.resource::<Table>(), Some(&Table(vec![Some(3), Some(0)])));
        // a table the rule accepts needs nothing
        let report = m
            .convert(
                &mut table_world(vec![Some(1)]),
                &classified(),
                Mode::Partial,
                &PassOptions::new(),
            )
            .unwrap();
        assert_eq!(report.pipeline, "");
        // nothing classifies: no lowering
        let err = PassManager::new(PassRegistry::new())
            .convert(
                &mut table_world(vec![None]),
                &classified(),
                Mode::Partial,
                &PassOptions::new(),
            )
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no pass lowers `toy.table` to anything `t` accepts"
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
    fn a_refused_value_names_its_rule() {
        // `lazy` claims to legalize toy.mesh in place but changes nothing: the check after the pipeline names the rule
        let mut r = PassRegistry::new();
        r.register("lazy", |_| {
            Ok(Box::new(Decl {
                name: "lazy",
                requires: &["toy.mesh"],
                provides: &[],
                removes: &[],
                modifies: &["toy.mesh"],
                legalizes: &["toy.mesh"],
            }))
        });
        let t = Target::new("even").check::<Mesh>("even meshes", |m| m.0 % 2 == 0);
        let err = PassManager::new(r)
            .convert(&mut mesh_world(3), &t, Mode::Partial, &PassOptions::new())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`even` does not accept `toy.mesh` (even meshes) on [e0] after the pipeline"
        );
        assert!(matches!(err, Error::Illegal { ref rule, .. } if rule == "even meshes"));
    }

    #[test]
    fn a_pass_that_provides_a_settled_name_after_it_refuses_the_pipeline() {
        let mut r = PassRegistry::new();
        r.register("classify", |_| Ok(Box::new(Classify)));
        r.register("add-row", |_| Ok(Box::new(AddRow)));
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
        // the planner puts the classifier after the pass that adds rows
        let passes: Vec<Box<dyn Pass>> = vec![Box::new(Classify), Box::new(AddRow)];
        let order: Vec<&str> = order_passes(&passes, &BTreeSet::from([0, 1]))
            .unwrap()
            .into_iter()
            .map(|i| passes[i].name())
            .collect();
        assert_eq!(order, ["add-row", "classify"]);
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
        let err = m
            .convert(&mut world(), &gltf(), Mode::Partial, &PassOptions::new())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "verify after bake: no-pbr on e0: pbr is banned here"
        );
    }
}
