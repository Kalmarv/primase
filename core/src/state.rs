//! World state: variables a level can be in (a game's event flags, a door's open state, a level's instance group, an
//! object's active flag) and what depends on them, resolved to one concrete world by [`ResolveState`].
//!
//! - An **axis** ([`StateAxis`]) is an entity: a stable `key`, a small domain (bool, or up to 64 named values), a
//!   default where the source states one (none where a script the reader doesn't read sets it), a scope (the whole
//!   world, or one entity such as a door) and a class tools act on (`door`, `ipl`, ...).
//!   Axes are variables, so independent toggles never multiply into enumerated variants.
//! - A **condition** ([`Condition`]) on a placed entity, on a prototype or on a part or alternative of an asset
//!   (`core.asset`: an outfit's pieces), says when it exists: an OR of clauses, each an AND of literals "axis is one of these values"
//!   (a value mask). No condition: always present. A [`PrimitiveCondition`] says the same of each primitive of one
//!   mesh (a hat drawn as one primitive of the body's mesh), and follows the primitives through every edit
//!   ([`Component::remap_primitives`]).
//! - An **override** ([`StateOverride`]) on a placed entity changes its transform, material variant or prototype in
//!   the states its clause matches. Poses (a door's open animation) stay in the dialect that owns the clip: its own pass
//!   lowers them into these overrides first.
//! - Values come from [`ResolveState`]'s `assign` option, else the [`StateAssignment`] resource (a save snapshot), else
//!   each axis's default. An axis with none of them (its default unknown, nothing assigned) stays unresolved, with
//!   what depends on it: a value the source doesn't state is not chosen for the caller.
//!
//! Writers that cannot toggle declare `core.condition` and `core.state_override` illegal, so the planner runs
//! `resolve-state` before them. A world without state components is never touched.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::asset::{Asset, Role};
use crate::clip::ClipBinding;
use crate::components::{Instance, Mesh, Transform};
use crate::json::JsonError;
use crate::ledger::Measure;
use crate::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use crate::world::{Component, Entity, Resource, World};

/// The values an axis can take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Domain {
    /// `off` (0) and `on` (1).
    Bool,
    /// Named values, 1..=64 of them, by index.
    Enum(Vec<String>),
}

impl Domain {
    /// The number of values.
    pub fn len(&self) -> usize {
        match self {
            Domain::Bool => 2,
            Domain::Enum(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The name of value `i` (`off`/`on` for a bool).
    pub fn name(&self, i: u32) -> Option<&str> {
        match self {
            Domain::Bool => ["off", "on"].get(i as usize).copied(),
            Domain::Enum(v) => v.get(i as usize).map(String::as_str),
        }
    }

    /// A value from its name or its index.
    pub fn parse(&self, text: &str) -> Option<u32> {
        let n = self.len() as u32;
        if let Ok(i) = text.parse::<u32>() {
            return (i < n).then_some(i);
        }
        (0..n).find(|&i| self.name(i) == Some(text))
    }

    /// The mask of every value.
    fn full_mask(&self) -> u64 {
        // a domain of more than 64 values is a verifier's error ([`StateAxis`]), not a shift past the word
        match self.len() {
            n @ 0..=63 => (1u64 << n) - 1,
            _ => u64::MAX,
        }
    }
}

/// Whether an axis belongs to the whole world or to one entity (a door's own open state).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Scope {
    Global,
    Local(Entity),
}

/// A state variable, on its own entity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StateAxis {
    /// Stable and unique in the world (`<dialect>.flag.8302`, `<dialect>.ipl.<name>`): assignments, reports and
    /// the text form name axes by it.
    pub key: String,
    pub domain: Domain,
    /// The value without an assignment: the game's own start state, where the source states it (an authored flag, a
    /// stored active state). `None`: unknown (a script decides it, and the reader doesn't read that script);
    /// [`ResolveState`] leaves the axis unresolved unless it is assigned.
    pub default: Option<u32>,
    pub scope: Scope,
    /// What kind of state it is (`event`, `door`, `ipl`, `entity_set`, `active`, `time`, ...), so tools can act on a
    /// class ("open every door").
    pub class: String,
    /// Where it came from (`event flag 8302`, an IPL name).
    pub source: String,
}

impl Component for StateAxis {
    const NAME: &'static str = "core.state_axis";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.key.is_empty() || self.key.contains([':', ';']) {
            return Err(format!(
                "key {:?} is empty or holds ':' or ';' (the assign syntax)",
                self.key
            ));
        }
        let n = self.domain.len();
        if !(1..=64).contains(&n) {
            return Err(format!("{}: {n} values, the domain holds 1..=64", self.key));
        }
        if let Domain::Enum(names) = &self.domain {
            for (i, a) in names.iter().enumerate() {
                if a.is_empty() || a.contains([':', ';']) || names[..i].contains(a) {
                    return Err(format!(
                        "{}: value name {a:?} empty, repeated or holding ':' or ';'",
                        self.key
                    ));
                }
            }
        }
        if let Some(d) = self.default
            && d as usize >= n
        {
            return Err(format!("{}: default {d} outside its {n} values", self.key));
        }
        // a local axis's owner is a reference: the world checks that it is alive
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        if let Scope::Local(owner) = self.scope {
            out.push(owner);
        }
    }
}

/// "`axis` has one of the values in `values`" (bit `i` set: value `i`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Literal {
    pub axis: Entity,
    pub values: u64,
}

/// The values `resolve-state` gave the axes it resolved, for a component that holds [`Clause`]s of its own
/// ([`crate::world::Component::restate`]); an axis it kept or could not resolve has no value here.
#[derive(Clone, Copy, Debug)]
pub struct Resolved<'a>(&'a BTreeMap<Entity, u32>);

impl Resolved<'_> {
    /// `clause` with its literals on resolved axes evaluated, as for a `core.condition`: `None` when one of them fails,
    /// else the literals on the other axes (empty: the clause holds whatever they are).
    pub fn reduce(&self, clause: &Clause) -> Option<Clause> {
        clause.reduce(self.0)
    }
    /// Whether `axis` was resolved.
    pub fn has(&self, axis: Entity) -> bool {
        self.0.contains_key(&axis)
    }
}

/// An AND of literals; an empty clause holds in every state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Clause(pub Vec<Literal>);

impl Clause {
    /// The clause with its literals on the axes in `values` evaluated: `None` when one of them fails, else the
    /// literals on the other axes (empty: the clause holds whatever they are).
    fn reduce(&self, values: &BTreeMap<Entity, u32>) -> Option<Clause> {
        let mut rest = Vec::new();
        for l in &self.0 {
            match values.get(&l.axis) {
                // a value past the mask's 64 bits is in no literal's set
                Some(&v) if 1u64.checked_shl(v).is_none_or(|bit| l.values & bit == 0) => return None,
                Some(_) => {}
                None => rest.push(*l),
            }
        }
        Some(Clause(rest))
    }

    fn verify(&self, w: &World) -> Result<(), String> {
        for l in &self.0 {
            let axis = w
                .get::<StateAxis>(l.axis)
                .ok_or_else(|| format!("literal on {:?}, which is not a live axis", l.axis))?;
            if l.values == 0 {
                return Err(format!("{}: empty value mask", axis.key));
            }
            if l.values & !axis.domain.full_mask() != 0 {
                return Err(format!(
                    "{}: mask {:#x} has values beyond its {}",
                    axis.key,
                    l.values,
                    axis.domain.len()
                ));
            }
        }
        Ok(())
    }

    fn axes(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.iter().map(|l| l.axis));
    }
}

fn placed(e: Entity, w: &World) -> Result<(), String> {
    if w.has::<Transform>(e) {
        Ok(())
    } else {
        Err("on an entity without a transform: only placed things carry state".into())
    }
}

/// Whether `e` is a prototype: it has a mesh (loaded or still in its file), so placements of it may share it.
fn is_prototype(e: Entity, w: &World) -> bool {
    w.has::<Mesh>(e) || w.has::<crate::MeshFile>(e)
}

/// Whether `e` is a part or an alternative of some asset: a member whose presence a choice decides.
fn is_choosable_member(e: Entity, w: &World) -> bool {
    w.iter::<Asset>().any(|(_, a)| {
        a.members
            .iter()
            .any(|&(m, r)| m == e && matches!(r, Role::Part | Role::Alternative))
    })
}

/// When a placed entity, a prototype (an entity with a mesh, which placements share) or a part or alternative member
/// of an asset exists: an OR of clauses. Zero clauses: never (resolve-state counts these).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Condition(pub Vec<Clause>);

impl Component for Condition {
    const NAME: &'static str = "core.condition";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !is_prototype(e, w) && !is_choosable_member(e, w) {
            placed(e, w).map_err(|_| {
                "on an entity without a transform that is neither a prototype (a mesh) nor a part or alternative of an asset: only placed things, prototypes and chosen members carry state"
                    .to_owned()
            })?;
        }
        self.0.iter().try_for_each(|c| c.verify(w))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        self.0.iter().for_each(|c| c.axes(out));
    }
}

/// When each primitive of the entity's `core.mesh` is drawn: one entry per primitive, `None` for one drawn in every
/// state. A primitive whose condition fails in the resolved state is dropped from the mesh (declared as a drop); the
/// condition follows its primitive through every edit that splits, drops or reorders primitives, as every
/// per-primitive component does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PrimitiveCondition(pub Vec<Option<Condition>>);

impl Component for PrimitiveCondition {
    const NAME: &'static str = "core.primitive_condition";
    const GEOMETRY: crate::Geometry = crate::Geometry::PerPrimitive;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(crate::remap_list(&self.0, from).map(PrimitiveCondition))
    }
    /// One entry per primitive of the entity's mesh, which it has; every literal names a live axis.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mesh = w.get::<Mesh>(e).ok_or("on an entity without a core.mesh")?;
        if mesh.primitives().len() != self.0.len() {
            return Err(format!(
                "{} conditions for {} primitives",
                self.0.len(),
                mesh.primitives().len()
            ));
        }
        self.0
            .iter()
            .flatten()
            .flat_map(|c| c.0.iter())
            .try_for_each(|c| c.verify(w))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        self.0
            .iter()
            .flatten()
            .for_each(|c| c.0.iter().for_each(|cl| cl.axes(out)));
    }
}

/// What an override changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Override {
    Transform(Transform),
    /// `Instance::variant`, against the prototype's material variants.
    Variant(u32),
    /// `Instance::prototype`.
    Prototype(Entity),
}

impl Override {
    fn kind(&self) -> &'static str {
        match self {
            Override::Transform(_) => "transform",
            Override::Variant(_) => "variant",
            Override::Prototype(_) => "prototype",
        }
    }
}

/// Changes to a placed entity by state, in order: the first clause that holds applies; none: unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StateOverride(pub Vec<(Clause, Override)>);

impl Component for StateOverride {
    const NAME: &'static str = "core.state_override";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        placed(e, w)?;
        for (c, o) in &self.0 {
            c.verify(w)?;
            match o {
                Override::Transform(t) => t.verify(e, w)?,
                Override::Variant(_) | Override::Prototype(_) if !w.has::<Instance>(e) => {
                    return Err(format!("{} override on an entity without an instance", o.kind()));
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for (c, o) in &self.0 {
            c.axes(out);
            if let Override::Prototype(p) = o {
                out.push(*p);
            }
        }
    }
}

/// Values for axes by key (a save snapshot): above each axis's default, below `resolve-state`'s `assign` option.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StateAssignment(pub BTreeMap<String, u32>);

impl Resource for StateAssignment {
    const NAME: &'static str = "core.state_assignment";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// What resolving state removes: the model's components and the assignment resource.
const REMOVED: &[&str] = &[
    StateAxis::NAME,
    Condition::NAME,
    PrimitiveCondition::NAME,
    StateOverride::NAME,
    StateAssignment::NAME,
];

/// What resolving state changes in what stays: placements, their prototypes, meshes that lose primitives, assets that
/// lose members.
const MODIFIED: &[&str] = &[
    Transform::NAME,
    Instance::NAME,
    Mesh::NAME,
    Asset::NAME,
    ClipBinding::NAME,
];

/// A condition with its literals on resolved axes evaluated.
enum Reduced {
    /// A clause holds whatever the unresolved axes are: no condition is left.
    Always,
    /// No clause can hold.
    Never,
    /// The clauses that may still hold, reduced to their literals on unresolved axes.
    Some(Vec<Clause>),
}

fn reduce_condition(c: &Condition, values: &BTreeMap<Entity, u32>) -> Reduced {
    let reduced: Vec<Clause> = c.0.iter().filter_map(|cl| cl.reduce(values)).collect();
    if reduced.iter().any(|cl| cl.0.is_empty()) {
        Reduced::Always
    } else if reduced.is_empty() {
        Reduced::Never
    } else {
        Reduced::Some(reduced)
    }
}

/// Take `e`, which is not present in the resolved state, out of the world. A part or alternative of an asset also
/// leaves its asset's member list, and the placements of a prototype go with it (counted): a prototype nothing draws
/// any more leaves nothing referring to it. Any other entity is despawned as it is, and what still references it is
/// left to the verifier.
fn remove_not_present(w: &mut World, e: Entity, log: &mut PassLog, why: &str) {
    let member = is_choosable_member(e, w);
    if member || (is_prototype(e, w) && !w.has::<Transform>(e)) {
        let roots: Vec<Entity> = w
            .iter::<Asset>()
            .filter(|(_, a)| a.members.iter().any(|&(m, _)| m == e))
            .map(|(r, _)| r)
            .collect();
        for r in roots {
            if let Some(a) = w.get_mut::<Asset>(r) {
                a.members.retain(|&(m, _)| m != e);
            }
        }
        let placements: Vec<Entity> = w
            .iter::<Instance>()
            .filter(|(p, i)| i.prototype == e && *p != e)
            .map(|(p, _)| p)
            .collect();
        for p in placements {
            crate::ledger::despawn(w, p, log, "its prototype is not present in the resolved state");
            log.count("placements of an absent member, despawned", 1);
        }
        if member {
            log.count("asset members despawned", 1);
        }
    }
    crate::ledger::despawn(w, e, log, why);
}

/// `resolve-state{assign=key:value;key:value}`: one concrete world, as far as the values are known. Each axis takes
/// its `assign` value, else the [`StateAssignment`] resource's, else its class's `assign-class` value
/// (`assign-class=class:value;...`: a program's choice for every axis of a kind, a level of detail where the source
/// picks by screen size), else its default (values by name or index). Keys
/// without an axis, in `assign`, `keep` or the resource, are counted by key and ignored: a program states its choices
/// once for every world it converts, and a region, level or save holds axes for some of them only. A value outside an
/// axis's domain is an error. Placed entities whose condition does not hold are despawned, then each remaining
/// entity's first matching override applies. Every resolved axis, every condition and override on resolved axes only,
/// and the assignment resource are removed. A despawned entity that something else still references (a self-placed
/// mesh used as another instance's prototype) leaves that reference dangling, and the verifier after the pass fails on
/// it.
///
/// An item the pipeline text form cannot carry back ([`crate::pass::carries`]: a ',' at the end, or one before an '=') is
/// refused by name, so its text reads back as the same options.
///
/// `keep=key;key` leaves those axes unresolved, for a consumer that switches them itself (a time of day). An axis
/// whose default is unknown and that nothing assigns stays unresolved the same way (counted, `axis <key> unresolved`):
/// choosing a value for it would assert a state the source doesn't state. An unresolved axis stays, and so do the
/// conditions and overrides that depend on it, reduced to their literals on unresolved axes (every literal on a
/// resolved axis evaluated: a clause one of them fails is dropped, a clause left with no literal holds in every
/// state). An entity whose condition keeps no clause is despawned, one with a clause that always holds loses its
/// condition; an override list keeps its order up to the first clause that always holds, which applies now when it is
/// the first. The snapshot's ([`StateAssignment`]) values for kept axes stay, as the resource's only entries. A key
/// both kept and assigned is an error, and so is an axis scoped to an entity (which a resolved condition may despawn)
/// that would stay unresolved: only global axes stay.
///
/// A part or alternative of an asset whose condition fails leaves the asset's member list and is despawned with the
/// placements of it (`asset members despawned`, `placements of an absent member, despawned`). A primitive whose
/// [`PrimitiveCondition`] fails is dropped from its mesh, every per-primitive component following (declared as a
/// primitive drop; counted per axis as `<axis>=<value> primitives dropped`); a mesh left with no primitive is
/// not present: its entity is removed as a member would be.
#[derive(Clone, Debug, Default)]
pub struct ResolveState {
    /// key -> value text, as given.
    pub assign: BTreeMap<String, String>,
    /// class -> value text, for every axis of that class nothing else gives a value.
    pub assign_class: BTreeMap<String, String>,
    /// The keys of the axes left unresolved.
    pub keep: BTreeSet<String>,
}

/// `resolve-state`'s declaration ([`crate::pass::PassRegistry::add`]).
pub const RESOLVE_STATE: PassSpec = PassSpec {
    name: "resolve-state",
    options: &[
        OptionSpec::optional(
            "assign",
            "time:noon",
            "key:value items, `;` between: each axis's value",
            "no axis assigned by its key: each takes its class's assignment, else is kept, else resolves by its default",
        ),
        OptionSpec::optional(
            "assign-class",
            "event:off",
            "class:value items, `;` between: the value of every axis of that class nothing else gives one",
            "no class assigned",
        ),
        OptionSpec::optional(
            "keep",
            "time",
            "axis keys, `;` between: the axes left unresolved",
            "every axis resolved",
        ),
    ],
    make: resolve_state,
};

fn resolve_state(g: &Given) -> Result<Box<dyn Pass>, String> {
    let items = |v: Option<&str>| -> Vec<String> {
        v.unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    // `key:value` items into a map, each key once
    let pairs = |option: &str, what: &str| -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        for item in items(g.optional(option)) {
            let (key, value) = item
                .split_once(':')
                .ok_or_else(|| format!("{option} item {item:?} is not {what}"))?;
            if out
                .insert(key.trim().to_owned(), value.trim().to_owned())
                .is_some()
            {
                return Err(format!("{option}: {} twice", key.trim()));
            }
        }
        Ok(out)
    };
    let assign = pairs("assign", "key:value")?;
    let assign_class = pairs("assign-class", "class:value")?;
    let mut keep = BTreeSet::new();
    for key in items(g.optional("keep")) {
        if !keep.insert(key.clone()) {
            return Err(format!("keep: {key} twice"));
        }
    }
    if let Some(k) = keep.iter().find(|k| assign.contains_key(*k)) {
        return Err(format!("{k} is both assigned and kept"));
    }
    let pass = ResolveState {
        assign,
        assign_class,
        keep,
    };
    // its text form must read back as it: the first item (in the order it is written) whose option the pipeline text
    // cannot carry (a value ending in ',', a key or value with '=' after a comma) is refused by name
    for (option, value) in pass.options() {
        if crate::pass::carries(&value) {
            continue;
        }
        let items: Vec<&str> = value.split(';').collect();
        let item = (1..=items.len())
            .find(|&n| !crate::pass::carries(&items[..n].join(";")))
            .map_or(value.as_str(), |n| items[n - 1]);
        return Err(format!(
            "{option} item {item:?}: the pipeline text cannot carry it (a ',' at its end or before '=', or a brace)"
        ));
    }
    Ok(Box::new(pass))
}

impl Pass for ResolveState {
    fn name(&self) -> &'static str {
        "resolve-state"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if !self.assign.is_empty() {
            let items: Vec<String> = self.assign.iter().map(|(k, v)| format!("{k}:{v}")).collect();
            o.insert("assign".to_owned(), items.join(";"));
        }
        if !self.assign_class.is_empty() {
            let items: Vec<String> = self
                .assign_class
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect();
            o.insert("assign-class".to_owned(), items.join(";"));
        }
        if !self.keep.is_empty() {
            let keys: Vec<&str> = self.keep.iter().map(String::as_str).collect();
            o.insert("keep".to_owned(), keys.join(";"));
        }
        o
    }
    /// The state model's components, and the `core.state_assignment` resource it applied.
    fn removes(&self) -> &[&'static str] {
        REMOVED
    }
    fn modifies(&self) -> &[&'static str] {
        MODIFIED
    }
    /// Entities not present in the resolved state are despawned (declared as drops).
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the value of every axis: assign option > assignment resource > its class's assign-class > default
        let axes: Vec<(Entity, StateAxis)> = w.iter::<StateAxis>().map(|(e, a)| (e, a.clone())).collect();
        let by_key: BTreeMap<&str, (Entity, &StateAxis)> =
            axes.iter().map(|(e, a)| (a.key.as_str(), (*e, a))).collect();
        if by_key.len() != axes.len() {
            return Err("two axes share a key".into());
        }
        let mut values: BTreeMap<Entity, u32> = axes
            .iter()
            .filter_map(|(e, a)| a.default.map(|d| (*e, d)))
            .collect();
        // the axes the snapshot or a key assignment set: a class assignment doesn't reach them
        let mut explicit = BTreeSet::new();
        // a save holds every flag the game knows, a world has axes for a few: keys without an axis are counted
        let resource = w.remove_resource::<StateAssignment>().unwrap_or_default();
        for (k, &v) in &resource.0 {
            let Some(&(e, a)) = by_key.get(k.as_str()) else {
                log.count("assignment keys without an axis", 1);
                continue;
            };
            if v as usize >= a.domain.len() {
                return Err(format!(
                    "state assignment {k}={v} is outside its {} values",
                    a.domain.len()
                ));
            }
            values.insert(e, v);
            explicit.insert(e);
        }
        // a program's choices name axes some of its worlds lack: counted by key
        for (k, text) in &self.assign {
            let Some(&(e, a)) = by_key.get(k.as_str()) else {
                log.count(&format!("assigned {k}, which no axis has: ignored"), 1);
                continue;
            };
            let v = a
                .domain
                .parse(text)
                .ok_or_else(|| format!("assign {k}:{text} is not one of its values"))?;
            values.insert(e, v);
            explicit.insert(e);
        }
        // the kept axes: no value, their literals stay
        let mut kept = BTreeSet::new();
        // the snapshot's values for kept axes stay with them, for whoever resolves them later
        let mut kept_assignment = StateAssignment::default();
        for k in &self.keep {
            let Some(&(e, a)) = by_key.get(k.as_str()) else {
                log.count(&format!("kept {k}, which no axis has: ignored"), 1);
                continue;
            };
            // a resolved condition may despawn the entity a local axis is scoped to
            if let Scope::Local(_) = a.scope {
                return Err(format!(
                    "keep names {k}, which is scoped to an entity: only global axes are kept"
                ));
            }
            values.remove(&e);
            kept.insert(e);
            if let Some(&v) = resource.0.get(k) {
                kept_assignment.0.insert(k.clone(), v);
            }
            log.count(&format!("axis {k} kept unresolved"), 1);
        }
        // a program's choice for a kind of axis, over the source's default only: the value is read against each axis
        // it falls to, not those a key assignment, the snapshot or keep settled
        for (class, text) in &self.assign_class {
            let mut any = false;
            for (e, a) in axes
                .iter()
                .filter(|(e, a)| &a.class == class && !explicit.contains(e) && !kept.contains(e))
            {
                let v = a
                    .domain
                    .parse(text)
                    .ok_or_else(|| format!("assign-class {class}:{text} is not a value of axis {}", a.key))?;
                values.insert(*e, v);
                any = true;
            }
            if any {
                log.count(&format!("axes of class {class} assigned {text}"), 1);
            } else {
                log.count(
                    &format!("assigned class {class}, which no axis it falls to has: ignored"),
                    1,
                );
            }
        }
        // the axes nothing gave a value (their default unknown) stay as the kept ones do
        let mut unresolved = BTreeSet::new();
        for (e, a) in &axes {
            if values.contains_key(e) || kept.contains(e) {
                continue;
            }
            if let Scope::Local(_) = a.scope {
                return Err(format!(
                    "{}: no known default and no value, and scoped to an entity (only global axes stay unresolved): \
                     assign it",
                    a.key
                ));
            }
            unresolved.insert(*e);
            log.count(
                &format!("axis {} unresolved: no known default, not assigned", a.key),
                1,
            );
        }
        kept.extend(unresolved.iter().copied());
        if !kept_assignment.0.is_empty() {
            log.count(
                "snapshot values kept for kept axes",
                kept_assignment.0.len() as u64,
            );
            w.insert_resource(kept_assignment);
        }
        // run() may see an unverified world: every literal must name an axis
        let named_by = |e: Entity, clauses: &mut dyn Iterator<Item = &Clause>| {
            clauses
                .flat_map(|c| c.0.iter().map(|l| l.axis))
                .find(|a| !values.contains_key(a) && !kept.contains(a))
                .map_or(Ok(()), |a| {
                    Err(format!("{e:?}: a literal names {a:?}, which is not an axis"))
                })
        };
        for (e, c) in w.iter::<Condition>() {
            named_by(e, &mut c.0.iter())?;
        }
        for (e, o) in w.iter::<StateOverride>() {
            named_by(e, &mut o.0.iter().map(|(c, _)| c))?;
        }
        for (e, pc) in w.iter::<PrimitiveCondition>() {
            named_by(e, &mut pc.0.iter().flatten().flat_map(|c| c.0.iter()))?;
        }
        let axis_of: BTreeMap<Entity, &StateAxis> = axes.iter().map(|(e, a)| (*e, a)).collect();
        let label = |e: &Entity| {
            let a = axis_of[e];
            match values.get(e) {
                Some(&v) => format!("{}={}", a.key, a.domain.name(v).unwrap_or("?")),
                None if unresolved.contains(e) => format!("{} (unresolved)", a.key),
                None => format!("{} (kept)", a.key),
            }
        };

        // conditions: despawn what does not exist in this state
        let conditions: Vec<(Entity, Condition)> =
            w.iter::<Condition>().map(|(e, c)| (e, c.clone())).collect();
        for (e, c) in &conditions {
            let reduced = reduce_condition(c, &values);
            let present = !matches!(reduced, Reduced::Never);
            if c.0.is_empty() {
                log.count("never present", 1);
            }
            let mut mentioned = Vec::new();
            c.references(&mut mentioned);
            mentioned.sort();
            mentioned.dedup();
            for a in &mentioned {
                log.count(
                    &format!("{} {}", label(a), if present { "kept" } else { "despawned" }),
                    1,
                );
            }
            match reduced {
                Reduced::Always => {
                    w.remove::<Condition>(*e);
                }
                Reduced::Some(r) => {
                    w.insert(*e, Condition(r));
                    log.count("conditions kept on unresolved axes", 1);
                }
                Reduced::Never => remove_not_present(w, *e, log, "not present in the resolved state"),
            }
        }

        // primitive conditions: drop the primitives that do not exist in this state
        let primitive_conditions: Vec<(Entity, PrimitiveCondition)> = w
            .iter::<PrimitiveCondition>()
            .map(|(e, c)| (e, c.clone()))
            .collect();
        for (e, pc) in primitive_conditions {
            if !w.is_alive(e) {
                continue;
            }
            let mut keep: Vec<usize> = Vec::new();
            let mut conditions: Vec<Option<Condition>> = Vec::new();
            for (i, c) in pc.0.iter().enumerate() {
                let Some(c) = c else {
                    keep.push(i);
                    conditions.push(None);
                    continue;
                };
                if c.0.is_empty() {
                    log.count("primitives never present", 1);
                }
                let mut mentioned = Vec::new();
                c.references(&mut mentioned);
                mentioned.sort();
                mentioned.dedup();
                let reduced = reduce_condition(c, &values);
                let present = !matches!(reduced, Reduced::Never);
                for a in &mentioned {
                    log.count(
                        &format!(
                            "{} primitives {}",
                            label(a),
                            if present { "kept" } else { "dropped" }
                        ),
                        1,
                    );
                }
                match reduced {
                    Reduced::Always => {
                        keep.push(i);
                        conditions.push(None);
                    }
                    Reduced::Some(r) => {
                        keep.push(i);
                        conditions.push(Some(Condition(r)));
                        log.count("primitive conditions kept on unresolved axes", 1);
                    }
                    Reduced::Never => {}
                }
            }
            if keep.is_empty() {
                // nothing of it is drawn in this state: the entity is not present
                remove_not_present(
                    w,
                    e,
                    log,
                    "every primitive's condition fails in the resolved state",
                );
                continue;
            }
            let dropped = pc.0.len() - keep.len();
            if dropped > 0 {
                w.rebuild_primitives(e, |mesh| {
                    let primitives = mesh.primitives();
                    // one condition per primitive is the verifier's rule, which an unverified world may break
                    let kept = keep
                        .iter()
                        .map(|&i| {
                            primitives.get(i).cloned().ok_or_else(|| {
                                format!(
                                    "{e:?}: {} primitive conditions for {} primitives",
                                    pc.0.len(),
                                    primitives.len()
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((kept, keep.clone()))
                })?;
                log.dropped(
                    Measure::Primitives,
                    "primitive condition fails in the resolved state",
                    dropped as u64,
                );
                log.count("primitives dropped by their condition", dropped as u64);
            }
            if conditions.iter().all(Option::is_none) {
                w.remove::<PrimitiveCondition>(e);
            } else {
                w.insert(e, PrimitiveCondition(conditions));
            }
        }

        // overrides on what remains
        let overrides: Vec<(Entity, StateOverride)> =
            w.iter::<StateOverride>().map(|(e, o)| (e, o.clone())).collect();
        for (e, o) in overrides {
            w.remove::<StateOverride>(e);
            // in order, the clauses that may still hold, up to the first that always holds
            let mut reduced: Vec<(Clause, Override)> = Vec::new();
            for (c, ov) in &o.0 {
                if let Some(r) = c.reduce(&values) {
                    let last = r.0.is_empty();
                    reduced.push((r, *ov));
                    if last {
                        break;
                    }
                }
            }
            let ov = match reduced.as_slice() {
                [] => continue,
                [(c, ov)] if c.0.is_empty() => ov,
                _ => {
                    w.insert(e, StateOverride(reduced));
                    log.count("overrides kept on unresolved axes", 1);
                    continue;
                }
            };
            match *ov {
                Override::Transform(t) => {
                    w.insert(e, t);
                }
                Override::Variant(v) => {
                    w.get_mut::<Instance>(e)
                        .ok_or("variant override without an instance")?
                        .variant = v;
                }
                Override::Prototype(p) => {
                    w.get_mut::<Instance>(e)
                        .ok_or("prototype override without an instance")?
                        .prototype = p;
                }
            }
            log.count(&format!("override {}", ov.kind()), 1);
        }

        // components that hold clauses of their own, reduced as the conditions are
        for e in w.restate_components(&Resolved(&values)) {
            if w.is_alive(e) {
                remove_not_present(w, e, log, "not present in the resolved state");
            }
        }
        for (e, _) in &axes {
            if !kept.contains(e) {
                w.remove::<StateAxis>(*e);
            }
        }
        Ok(())
    }
}

/// `select-outfit{choices=key:value;key:value}` (required): resolves the named axes only, to the named values (by name
/// or index), and leaves every other global axis unresolved, with the conditions and primitive conditions that
/// depend on it reduced as [`ResolveState`] reduces them. It is what a recipe of chosen ids replays: a program (a
/// website) picks some outfits and accessories from a catalog and the rest stays open. Everything else is as for
/// `resolve-state`, whose report it gives (the members and primitives dropped, counted per axis and value), with the
/// differences that a key naming no axis is an error naming the axes there are, and that an axis scoped to an entity
/// and not named resolves by its default (an error when it has none) as it cannot stay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOutfit {
    /// key -> value text, as given; never empty.
    pub choices: BTreeMap<String, String>,
}

/// `select-outfit`'s declaration ([`crate::pass::PassRegistry::add`]).
pub const SELECT_OUTFIT: PassSpec = PassSpec {
    name: "select-outfit",
    options: &[OptionSpec::required(
        "choices",
        "axis:value",
        "key:value items, `;` between, at least one: the outfit axes' values",
    )],
    make: select_outfit,
};

fn select_outfit(g: &Given) -> Result<Box<dyn Pass>, String> {
    let mut choices = BTreeMap::new();
    for item in g
        .required("choices")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (key, value) = item
            .split_once(':')
            .ok_or_else(|| format!("choices item {item:?} is not key:value"))?;
        if choices
            .insert(key.trim().to_owned(), value.trim().to_owned())
            .is_some()
        {
            return Err(format!("choices: {key} twice"));
        }
    }
    if choices.is_empty() {
        return Err("choices: no key:value item".into());
    }
    Ok(Box::new(SelectOutfit { choices }))
}

impl Pass for SelectOutfit {
    fn name(&self) -> &'static str {
        "select-outfit"
    }
    fn options(&self) -> Options {
        let items: Vec<String> = self.choices.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        Options::from([("choices".to_owned(), items.join(";"))])
    }
    fn requires(&self) -> &[&'static str] {
        &[StateAxis::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        REMOVED
    }
    fn modifies(&self) -> &[&'static str] {
        MODIFIED
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let axes: Vec<&StateAxis> = w.iter::<StateAxis>().map(|(_, a)| a).collect();
        if let Some(k) = self.choices.keys().find(|k| !axes.iter().any(|a| &a.key == *k)) {
            let mut there: Vec<&str> = axes.iter().map(|a| a.key.as_str()).collect();
            there.sort_unstable();
            let shown = there.iter().take(20).copied().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "choice {k}: no axis has that key (the world's: {shown}{})",
                if there.len() > 20 {
                    format!(" and {} more", there.len() - 20)
                } else {
                    String::new()
                }
            ));
        }
        let keep = axes
            .iter()
            .filter(|a| a.scope == Scope::Global && !self.choices.contains_key(&a.key))
            .map(|a| a.key.clone())
            .collect();
        ResolveState {
            assign: self.choices.clone(),
            assign_class: BTreeMap::new(),
            keep,
        }
        .run(w, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A domain of more than 64 values (the verifier refuses it) and a value past the word are not a shift past it.
    #[test]
    fn a_domain_beyond_the_word_does_not_overflow_a_shift() {
        let big = Domain::Enum((0..65).map(|i| format!("v{i}")).collect());
        assert_eq!(big.full_mask(), u64::MAX);
        assert_eq!(Domain::Bool.full_mask(), 0b11);
        let mut w = World::new();
        let axis = w.spawn();
        let clause = Clause(vec![Literal {
            axis,
            values: u64::MAX,
        }]);
        assert_eq!(clause.reduce(&BTreeMap::from([(axis, 64)])), None);
        assert_eq!(
            clause.reduce(&BTreeMap::from([(axis, 63)])),
            Some(Clause(Vec::new()))
        );
    }
    use crate::components::Name;
    use crate::pass::{Mode, PassManager, PassOptions, PassRegistry, Target};

    fn axis(w: &mut World, key: &str) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: key.into(),
                domain: Domain::Bool,
                default: Some(0),
                scope: Scope::Global,
                class: "event".into(),
                source: format!("event flag {}", key.rsplit('.').next().unwrap()),
            },
        );
        e
    }

    fn on(a: Entity) -> Literal {
        Literal {
            axis: a,
            values: 0b10,
        }
    }
    fn off(a: Entity) -> Literal {
        Literal {
            axis: a,
            values: 0b01,
        }
    }

    fn placement(w: &mut World, proto: Entity, name: &str) -> Entity {
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(e, Name(name.into()));
        e
    }

    /// A source's castle: intact walls (entity group 1117201: disabled while 8306 or 8302 is on), the same walls
    /// with charcoal (1117204: disabled while 8302 is off), and a piece shown only with 8301 on (1117203).
    struct Castle {
        w: World,
        intact: Entity,
        burnt: Entity,
        f8301: Entity,
    }

    fn castle() -> Castle {
        let mut w = World::new();
        let f8301 = axis(&mut w, "fromsoft.flag.8301");
        let f8302 = axis(&mut w, "fromsoft.flag.8302");
        let f8306 = axis(&mut w, "fromsoft.flag.8306");
        let proto = w.spawn();
        w.insert(proto, Name("m003312".into()));
        let intact = placement(&mut w, proto, "m003312_3802");
        w.insert(intact, Condition(vec![Clause(vec![off(f8306), off(f8302)])]));
        let burnt = placement(&mut w, proto, "m003412_3902");
        w.insert(burnt, Condition(vec![Clause(vec![on(f8302)])]));
        let only8301 = placement(&mut w, proto, "m300102_3700");
        w.insert(only8301, Condition(vec![Clause(vec![on(f8301)])]));
        Castle {
            w,
            intact,
            burnt,
            f8301,
        }
    }

    fn run(w: &mut World, assign: &str) -> PassLog {
        let opts = if assign.is_empty() {
            Options::new()
        } else {
            Options::from([("assign".to_owned(), assign.to_owned())])
        };
        let mut log = PassLog::default();
        RESOLVE_STATE.instance(&opts).unwrap().run(w, &mut log).unwrap();
        log
    }

    fn names(w: &World) -> Vec<String> {
        w.iter::<Instance>()
            .map(|(e, _)| w.get::<Name>(e).unwrap().0.clone())
            .collect()
    }

    fn run_keep(w: &mut World, assign: &str, keep: &str) -> Result<PassLog, String> {
        let mut opts = Options::from([("keep".to_owned(), keep.to_owned())]);
        if !assign.is_empty() {
            opts.insert("assign".to_owned(), assign.to_owned());
        }
        let mut log = PassLog::default();
        RESOLVE_STATE.instance(&opts)?.run(w, &mut log)?;
        Ok(log)
    }

    /// A record of parts, each with a clause of its own (as a collision file's parts are): not `core.condition`, so it is
    /// the component's own [`Component::restate`] that `resolve-state` runs.
    #[derive(Clone, Debug, PartialEq)]
    struct Parts(Vec<(&'static str, Option<Clause>)>);
    impl Component for Parts {
        const NAME: &'static str = "test.parts";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
        fn references(&self, out: &mut Vec<Entity>) {
            for (_, c) in &self.0 {
                if let Some(c) = c {
                    out.extend(c.0.iter().map(|l| l.axis));
                }
            }
        }
        fn restate(&self, resolved: &Resolved<'_>) -> crate::world::Restated<Self> {
            use crate::world::Restated;
            let mut parts = Vec::new();
            for (name, clause) in &self.0 {
                let clause = match clause {
                    None => None,
                    Some(c) => match resolved.reduce(c) {
                        None => continue,
                        Some(r) if r.0.is_empty() => None,
                        Some(r) => Some(r),
                    },
                };
                parts.push((*name, clause));
            }
            if parts.is_empty() {
                Restated::Absent
            } else if parts == self.0 {
                Restated::Unchanged
            } else {
                Restated::Replaced(Parts(parts))
            }
        }
    }

    /// A clause a component holds is reduced as a condition is: a part whose clause fails leaves the record, one that always
    /// holds loses its clause, a literal on a kept axis stays, and a record left with no part is despawned with the entity
    /// that has only it. With 8302 kept the clause on it stays, as a condition's would.
    #[test]
    fn a_clause_inside_a_component_is_reduced_by_resolving_state() {
        let mut c = castle();
        let f8302 =
            c.w.iter::<StateAxis>()
                .find(|(_, a)| a.key == "fromsoft.flag.8302")
                .unwrap()
                .0;
        let record = c.w.spawn();
        c.w.insert(
            record,
            Parts(vec![
                ("always", None),
                ("needs 8301", Some(Clause(vec![on(c.f8301)]))),
                ("not 8301", Some(Clause(vec![off(c.f8301)]))),
                (
                    "8301 off and 8302 on",
                    Some(Clause(vec![off(c.f8301), on(f8302)])),
                ),
            ]),
        );
        let only = c.w.spawn();
        c.w.insert(only, Parts(vec![("needs 8301", Some(Clause(vec![on(c.f8301)])))]));
        run_keep(&mut c.w, "", "fromsoft.flag.8302").unwrap();
        // 8301 is off by default: the part needing it leaves, the other clauses keep what is still open
        assert_eq!(
            c.w.get::<Parts>(record),
            Some(&Parts(vec![
                ("always", None),
                ("not 8301", None),
                ("8301 off and 8302 on", Some(Clause(vec![on(f8302)]))),
            ]))
        );
        // a record none of whose parts is present is not
        assert!(!c.w.is_alive(only));
        // and with everything resolved no clause on an axis is left to name a removed one
        let mut all = castle();
        let record = all.w.spawn();
        all.w.insert(
            record,
            Parts(vec![("not 8301", Some(Clause(vec![off(all.f8301)])))]),
        );
        run(&mut all.w, "");
        assert_eq!(all.w.get::<Parts>(record), Some(&Parts(vec![("not 8301", None)])));
    }

    /// Keeping 8302 (8301 and 8306 at their defaults, off): the intact walls keep only their 8302 literal, the
    /// charcoal walls their whole condition, the 8301 piece is despawned; 8302 stays an axis, the others go. A later
    /// resolve-state with 8302 on finishes the job as resolving everything at once would.
    #[test]
    fn kept_axes_stay_with_the_conditions_reduced_to_them() {
        let mut c = castle();
        let f8302 =
            c.w.iter::<StateAxis>()
                .find(|(_, a)| a.key == "fromsoft.flag.8302")
                .unwrap()
                .0;
        let log = run_keep(&mut c.w, "", "fromsoft.flag.8302").unwrap();
        assert_eq!(log.counts["axis fromsoft.flag.8302 kept unresolved"], 1);
        assert_eq!(
            c.w.get::<Condition>(c.intact),
            Some(&Condition(vec![Clause(vec![off(f8302)])]))
        );
        assert_eq!(
            c.w.get::<Condition>(c.burnt),
            Some(&Condition(vec![Clause(vec![on(f8302)])]))
        );
        assert!(!c.w.is_alive(c.f8301) || !c.w.has::<StateAxis>(c.f8301));
        assert_eq!(names(&c.w), vec!["m003312_3802", "m003412_3902"]);
        let keys: Vec<String> = c.w.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
        assert_eq!(keys, vec!["fromsoft.flag.8302"]);
        run(&mut c.w, "fromsoft.flag.8302:on");
        assert_eq!(names(&c.w), vec!["m003412_3902"]);
        assert!(c.w.iter::<Condition>().next().is_none());
    }

    /// An override list keeps its order on the kept axis up to the clause that always holds; when a resolved axis
    /// makes the first clause always hold, it applies now.
    #[test]
    fn overrides_on_kept_axes_stay_in_order() {
        for (assign, want) in [("", None), ("a:on", Some(1))] {
            let mut w = World::new();
            let a = axis(&mut w, "a");
            let k = axis(&mut w, "k");
            let proto = w.spawn();
            let e = placement(&mut w, proto, "p");
            w.insert(
                e,
                StateOverride(vec![
                    (Clause(vec![on(a)]), Override::Variant(1)),
                    (Clause(vec![on(k), off(a)]), Override::Variant(2)),
                    (Clause(vec![]), Override::Variant(3)),
                    (Clause(vec![on(k)]), Override::Variant(4)),
                ]),
            );
            run_keep(&mut w, assign, "k").unwrap();
            match want {
                None => assert_eq!(
                    w.get::<StateOverride>(e),
                    Some(&StateOverride(vec![
                        (Clause(vec![on(k)]), Override::Variant(2)),
                        (Clause(vec![]), Override::Variant(3)),
                    ]))
                ),
                Some(v) => {
                    assert!(!w.has::<StateOverride>(e));
                    assert_eq!(w.get::<Instance>(e).unwrap().variant, v);
                }
            }
        }
    }

    /// A save snapshot's value for a kept axis stays (as the only entry of core.state_assignment), so resolving the
    /// axis later reads it: 8302 saved on, kept, then resolved without an assignment leaves the charcoal walls.
    #[test]
    fn a_kept_axis_keeps_its_snapshot_value() {
        let mut c = castle();
        c.w.insert_resource(StateAssignment(BTreeMap::from([
            ("fromsoft.flag.8302".to_owned(), 1),
            ("fromsoft.flag.8306".to_owned(), 0),
        ])));
        let log = run_keep(&mut c.w, "", "fromsoft.flag.8302").unwrap();
        assert_eq!(log.counts["snapshot values kept for kept axes"], 1);
        assert_eq!(
            c.w.resource::<StateAssignment>(),
            Some(&StateAssignment(BTreeMap::from([(
                "fromsoft.flag.8302".to_owned(),
                1
            )])))
        );
        run(&mut c.w, "");
        assert_eq!(names(&c.w), vec!["m003412_3902"]);
    }

    /// Keeping or assigning an axis the world lacks is counted by key and changes nothing (a program's choices cover
    /// worlds that hold some of their axes only); keeping one that is also assigned is an error.
    #[test]
    fn a_missing_axis_is_counted_an_assigned_and_kept_one_is_an_error() {
        let mut plain = castle();
        run(&mut plain.w, "");
        let mut c = castle();
        let log = run_keep(&mut c.w, "fromsoft.flag.7777:on", "fromsoft.flag.9999").unwrap();
        assert_eq!(names(&c.w), names(&plain.w));
        assert_eq!(
            log.counts["kept fromsoft.flag.9999, which no axis has: ignored"],
            1
        );
        assert_eq!(
            log.counts["assigned fromsoft.flag.7777, which no axis has: ignored"],
            1
        );
        assert!(!c.w.has_component_named(StateAxis::NAME));
        let both = Options::from([
            ("keep".to_owned(), "fromsoft.flag.8302".to_owned()),
            ("assign".to_owned(), "fromsoft.flag.8302:on".to_owned()),
        ]);
        assert!(RESOLVE_STATE.instance(&both).is_err());
    }

    /// An item the pipeline text cannot carry is refused by name; what is accepted prints as text that parses back to
    /// the same options.
    #[test]
    fn resolve_state_refuses_what_its_text_form_cannot_carry() {
        let opts = |k: &str, v: &str| Options::from([(k.to_owned(), v.to_owned())]);
        for (option, value, item) in [
            ("assign", "a:1,", "a:1,"),
            ("assign", "a:1,b=2", "a:1,b=2"),
            ("assign", "a:1,;b=c:2", "a:1,"),
            ("keep", "a;z,", "z,"),
        ] {
            let error = RESOLVE_STATE.instance(&opts(option, value)).err().expect(value);
            assert!(error.contains(&format!("{item:?}")), "{value}: {error}");
        }
        // a ',' with a part after it that holds no '=' is carried
        assert!(RESOLVE_STATE.instance(&opts("keep", "k,;x")).is_ok());
        let accepted = RESOLVE_STATE
            .instance(&opts("assign", "fromsoft.flag.8302:on;gta.ipl.x:off"))
            .unwrap();
        let text = crate::pass::pass_text(accepted.as_ref());
        let mut r = PassRegistry::new();
        crate::register(&mut r);
        let again = r.parse(&text).unwrap();
        assert_eq!(again.passes[0].options(), accepted.options());
    }

    #[test]
    fn castle_defaults_keep_the_intact_walls() {
        let mut c = castle();
        assert_eq!(c.w.verify_components(), vec![]);
        let log = run(&mut c.w, "");
        assert_eq!(names(&c.w), ["m003312_3802"]);
        assert!(c.w.is_alive(c.intact) && !c.w.is_alive(c.burnt));
        assert_eq!(log.counts["fromsoft.flag.8302=off kept"], 1);
        assert_eq!(log.counts["fromsoft.flag.8302=off despawned"], 1);
        assert_eq!(log.counts["fromsoft.flag.8301=off despawned"], 1);
        for name in [StateAxis::NAME, Condition::NAME, StateOverride::NAME] {
            assert!(!c.w.has_component_named(name), "{name} left");
        }
    }

    #[test]
    fn castle_burnt_by_assignment() {
        let mut c = castle();
        run(&mut c.w, "fromsoft.flag.8302:on");
        assert_eq!(names(&c.w), ["m003412_3902"]);
    }

    #[test]
    fn assign_beats_resource_beats_default() {
        let mut c = castle();
        c.w.insert_resource(StateAssignment(BTreeMap::from([
            ("fromsoft.flag.8301".to_owned(), 1),
            ("fromsoft.flag.8302".to_owned(), 1),
        ])));
        run(&mut c.w, "fromsoft.flag.8302:0");
        assert_eq!(names(&c.w), ["m003312_3802", "m300102_3700"]);
        assert!(c.w.resource::<StateAssignment>().is_none());
        assert!(
            c.w.is_alive(c.f8301) && !c.w.has::<StateAxis>(c.f8301),
            "axes lose their component, the entity stays"
        );
    }

    /// A class assignment falls back after the key assignments, the snapshot and keep: its value is read only against
    /// the axes it falls to (here A, of three levels; B, of two, is assigned lod0 by key, so lod2 is never read against
    /// it), and an invalid value for an axis it does fall to is an error.
    #[test]
    fn a_class_assignment_reaches_only_the_axes_nothing_else_set() {
        let lod_axis = |w: &mut World, key: &str, levels: usize| {
            let e = w.spawn();
            w.insert(
                e,
                StateAxis {
                    key: key.into(),
                    domain: Domain::Enum((0..levels).map(|i| format!("lod{i}")).collect()),
                    default: None,
                    scope: Scope::Global,
                    class: "lod".into(),
                    source: key.into(),
                },
            );
            e
        };
        let world = || {
            let mut w = World::new();
            let proto = w.spawn();
            w.insert(proto, Name("proto".into()));
            let (a, b) = (lod_axis(&mut w, "lod.a", 3), lod_axis(&mut w, "lod.b", 2));
            // a placement for A at lod2 and one for B at lod0
            for (axis, level, name) in [(a, 2, "a2"), (b, 0, "b0")] {
                let p = placement(&mut w, proto, name);
                w.insert(
                    p,
                    Condition(vec![Clause(vec![Literal {
                        axis,
                        values: 1 << level,
                    }])]),
                );
            }
            w
        };
        let resolve = |w: &mut World, assign: &[(&str, &str)]| {
            ResolveState {
                assign: assign.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
                assign_class: [("lod".to_owned(), "lod2".to_owned())].into(),
                ..Default::default()
            }
            .run(w, &mut PassLog::default())
        };
        let mut w = world();
        resolve(&mut w, &[("lod.b", "lod0")]).unwrap();
        let mut left: Vec<String> = w.iter::<Name>().map(|(_, n)| n.0.clone()).collect();
        left.sort();
        assert_eq!(left, ["a2", "b0", "proto"]);
        // without B's key assignment, lod2 falls to B too, which has no lod2
        let e = resolve(&mut world(), &[]).unwrap_err();
        assert!(
            e.contains("assign-class lod:lod2 is not a value of axis lod.b"),
            "{e}"
        );
    }

    #[test]
    fn a_save_with_unrelated_flags_is_fine() {
        let mut c = castle();
        c.w.insert_resource(StateAssignment(BTreeMap::from([
            ("fromsoft.flag.8302".to_owned(), 1),
            ("fromsoft.flag.11110100".to_owned(), 1),
            ("fromsoft.flag.6000".to_owned(), 0),
        ])));
        let log = run(&mut c.w, "");
        assert_eq!(names(&c.w), ["m003412_3902"]);
        assert_eq!(log.counts["assignment keys without an axis"], 2);
    }

    #[test]
    fn run_on_an_unverified_world_errs_instead_of_panicking() {
        let mut w = World::new();
        let proto = w.spawn();
        let not_axis = w.spawn();
        let e = placement(&mut w, proto, "p");
        w.insert(
            e,
            Condition(vec![Clause(vec![Literal {
                axis: not_axis,
                values: 1,
            }])]),
        );
        let err = RESOLVE_STATE
            .instance(&Options::new())
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("not an axis"), "{err}");
        let mut w = World::new();
        let e = placement(&mut w, proto, "p");
        w.insert(
            e,
            StateOverride(vec![(
                Clause(vec![Literal {
                    axis: not_axis,
                    values: 1,
                }]),
                Override::Variant(1),
            )]),
        );
        assert!(
            RESOLVE_STATE
                .instance(&Options::new())
                .unwrap()
                .run(&mut w, &mut PassLog::default())
                .is_err()
        );
    }

    /// An axis whose default is unknown stays unresolved unless assigned: with 8302's default unknown, the walls on it
    /// keep their 8302 literals (8306 resolved off), the 8301 piece goes, and 8302 stays an axis (counted); assigned,
    /// or given by the snapshot, it resolves as a known default would. Scoped to an entity, it must be assigned.
    #[test]
    fn an_unknown_default_stays_unresolved_unless_assigned() {
        let unknown = |c: &mut Castle| {
            let (e, _) =
                c.w.iter::<StateAxis>()
                    .find(|(_, a)| a.key == "fromsoft.flag.8302")
                    .unwrap();
            c.w.get_mut::<StateAxis>(e).unwrap().default = None;
            e
        };
        let mut c = castle();
        let f8302 = unknown(&mut c);
        assert_eq!(c.w.verify_components(), vec![]);
        let log = run(&mut c.w, "");
        assert_eq!(names(&c.w), ["m003312_3802", "m003412_3902"]);
        assert_eq!(
            log.counts["axis fromsoft.flag.8302 unresolved: no known default, not assigned"],
            1
        );
        assert_eq!(log.counts["fromsoft.flag.8302 (unresolved) kept"], 2);
        assert_eq!(log.counts["conditions kept on unresolved axes"], 2);
        let axes: Vec<&str> = c.w.iter::<StateAxis>().map(|(_, a)| a.key.as_str()).collect();
        assert_eq!(axes, ["fromsoft.flag.8302"]);
        assert_eq!(
            c.w.get::<Condition>(c.intact),
            Some(&Condition(vec![Clause(vec![off(f8302)])]))
        );
        assert_eq!(c.w.verify_components(), vec![]);
        // a later resolve-state with a value finishes the job
        run(&mut c.w, "fromsoft.flag.8302:on");
        assert_eq!(names(&c.w), ["m003412_3902"]);
        assert!(!c.w.has_component_named(StateAxis::NAME));

        for given in ["assign", "snapshot"] {
            let mut c = castle();
            unknown(&mut c);
            if given == "snapshot" {
                c.w.insert_resource(StateAssignment(BTreeMap::from([(
                    "fromsoft.flag.8302".to_owned(),
                    0,
                )])));
                run(&mut c.w, "");
            } else {
                run(&mut c.w, "fromsoft.flag.8302:off");
            }
            assert_eq!(names(&c.w), ["m003312_3802"], "{given}");
            assert!(!c.w.has_component_named(StateAxis::NAME), "{given}");
        }

        let mut c = castle();
        let e = unknown(&mut c);
        let door = c.intact;
        c.w.get_mut::<StateAxis>(e).unwrap().scope = Scope::Local(door);
        let err = RESOLVE_STATE
            .instance(&Options::new())
            .unwrap()
            .run(&mut c.w, &mut PassLog::default())
            .unwrap_err();
        assert!(err.contains("assign it"), "{err}");
    }

    #[test]
    fn values_outside_the_domain_and_bad_options_are_errors() {
        for bad in ["fromsoft.flag.8302:maybe", "fromsoft.flag.8302:2"] {
            let mut c = castle();
            let opts = Options::from([("assign".to_owned(), bad.to_owned())]);
            assert!(
                RESOLVE_STATE
                    .instance(&opts)
                    .unwrap()
                    .run(&mut c.w, &mut PassLog::default())
                    .is_err(),
                "{bad}"
            );
        }
        let mut c = castle();
        // a resource value outside the axis's domain is an error (only keys without an axis are tolerated)
        c.w.insert_resource(StateAssignment(BTreeMap::from([(
            "fromsoft.flag.8302".to_owned(),
            2,
        )])));
        assert!(
            RESOLVE_STATE
                .instance(&Options::new())
                .unwrap()
                .run(&mut c.w, &mut PassLog::default())
                .is_err()
        );
        assert!(
            RESOLVE_STATE
                .instance(&Options::from([("assign".to_owned(), "x".to_owned())]))
                .is_err()
        );
        assert!(
            RESOLVE_STATE
                .instance(&Options::from([("colour".to_owned(), "red".to_owned())]))
                .is_err()
        );
    }

    #[test]
    fn empty_clause_always_zero_clauses_never() {
        let mut w = World::new();
        let proto = w.spawn();
        let always = placement(&mut w, proto, "always");
        w.insert(always, Condition(vec![Clause::default()]));
        let never = placement(&mut w, proto, "never");
        w.insert(never, Condition(vec![]));
        assert_eq!(w.verify_components(), vec![]);
        let log = run(&mut w, "");
        assert_eq!(names(&w), ["always"]);
        assert_eq!(log.counts["never present"], 1);
    }

    #[test]
    fn overrides_first_match_applies() {
        let mut w = World::new();
        let door = axis(&mut w, "fromsoft.door.1110432");
        let proto = w.spawn();
        let open = w.spawn();
        w.insert(open, Name("door open".into()));
        let e = placement(&mut w, proto, "door");
        let mut moved = Transform::IDENTITY;
        moved.cols[3] = [1.0, 0.0, 0.0];
        w.insert(
            e,
            StateOverride(vec![
                (Clause(vec![on(door)]), Override::Prototype(open)),
                (Clause(vec![on(door)]), Override::Transform(moved)),
                (Clause::default(), Override::Variant(3)),
            ]),
        );
        assert_eq!(w.verify_components(), vec![]);
        let mut a = w.clone();
        let log = run(&mut a, "fromsoft.door.1110432:on");
        assert_eq!(a.get::<Instance>(e).unwrap().prototype, open);
        assert_eq!(
            *a.get::<Transform>(e).unwrap(),
            Transform::IDENTITY,
            "only the first match applies"
        );
        assert_eq!(log.counts["override prototype"], 1);
        run(&mut w, "");
        assert_eq!(
            *w.get::<Instance>(e).unwrap(),
            Instance {
                prototype: proto,
                variant: 3
            }
        );
    }

    /// One failing world per rule `verify()` enforces, each asserting the exact (component, entity, message) that
    /// rule reports: a world that fails some other rule does not pass here.
    #[test]
    fn verify_rejects_each_rule() {
        let failing = |build: &dyn Fn(&mut World) -> (&'static str, Entity, String)| {
            let mut w = World::new();
            let (component, entity, message) = build(&mut w);
            assert_eq!(
                w.verify_components(),
                [(component, entity, message)],
                "one failure, the rule's own"
            );
        };
        let mut good = StateAxis {
            key: "k".into(),
            domain: Domain::Bool,
            default: Some(0),
            scope: Scope::Global,
            class: "event".into(),
            source: String::new(),
        };
        let with_axis = move |w: &mut World, a: StateAxis| {
            let e = w.spawn();
            w.insert(e, a);
            e
        };
        // the axis
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    key: String::new(),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                r#"key "" is empty or holds ':' or ';' (the assign syntax)"#.to_owned(),
            )
        });
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    default: Some(2),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                "k: default 2 outside its 2 values".to_owned(),
            )
        });
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec![]),
                    default: Some(0),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                "k: 0 values, the domain holds 1..=64".to_owned(),
            )
        });
        failing(&|w| {
            let names = (0..65).map(|i| format!("v{i}")).collect();
            let e = with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(names),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                "k: 65 values, the domain holds 1..=64".to_owned(),
            )
        });
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec!["a".into(), "a".into()]),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                r#"k: value name "a" empty, repeated or holding ':' or ';'"#.to_owned(),
            )
        });
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec!["a:b".into()]),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                r#"k: value name "a:b" empty, repeated or holding ':' or ';'"#.to_owned(),
            )
        });
        failing(&|w| {
            let e = with_axis(
                w,
                StateAxis {
                    key: "k;x".into(),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                r#"key "k;x" is empty or holds ':' or ';' (the assign syntax)"#.to_owned(),
            )
        });
        failing(&|w| {
            let gone = w.spawn();
            w.despawn(gone);
            let e = with_axis(
                w,
                StateAxis {
                    scope: Scope::Local(gone),
                    ..good.clone()
                },
            );
            (
                "core.state_axis",
                e,
                format!("refers to {gone:?}, which is not alive"),
            )
        });
        // literals
        let cond = |w: &mut World, lit: Literal, transform: bool| {
            let e = w.spawn();
            if transform {
                w.insert(e, Transform::IDENTITY);
            }
            w.insert(e, Condition(vec![Clause(vec![lit])]));
            e
        };
        failing(&|w| {
            let a = with_axis(w, good.clone());
            let e = cond(w, Literal { axis: a, values: 0 }, true);
            ("core.condition", e, "k: empty value mask".to_owned())
        });
        failing(&|w| {
            let a = with_axis(w, good.clone());
            let e = cond(
                w,
                Literal {
                    axis: a,
                    values: 0b100,
                },
                true,
            );
            (
                "core.condition",
                e,
                "k: mask 0x4 has values beyond its 2".to_owned(),
            )
        });
        failing(&|w| {
            let not_axis = w.spawn();
            w.insert(not_axis, Name("x".into()));
            let e = cond(
                w,
                Literal {
                    axis: not_axis,
                    values: 1,
                },
                true,
            );
            (
                "core.condition",
                e,
                format!("literal on {not_axis:?}, which is not a live axis"),
            )
        });
        failing(&|w| {
            let a = with_axis(w, good.clone());
            let e = cond(w, on(a), false);
            (
                "core.condition",
                e,
                "on an entity without a transform that is neither a prototype (a mesh) nor a part or alternative of an asset: only placed things, prototypes and chosen members carry state".to_owned(),
            )
        });
        // overrides
        failing(&|w| {
            let e = w.spawn();
            w.insert(e, Transform::IDENTITY);
            w.insert(e, StateOverride(vec![(Clause::default(), Override::Variant(1))]));
            (
                "core.state_override",
                e,
                "variant override on an entity without an instance".to_owned(),
            )
        });
        failing(&|w| {
            let proto = w.spawn();
            let gone = w.spawn();
            w.despawn(gone);
            let e = placement(w, proto, "p");
            w.insert(
                e,
                StateOverride(vec![(Clause::default(), Override::Prototype(gone))]),
            );
            (
                "core.state_override",
                e,
                format!("refers to {gone:?}, which is not alive"),
            )
        });
        failing(&|w| {
            let proto = w.spawn();
            let e = placement(w, proto, "p");
            let mut t = Transform::IDENTITY;
            t.cols[3][0] = f32::NAN;
            w.insert(
                e,
                StateOverride(vec![(Clause::default(), Override::Transform(t))]),
            );
            (
                "core.state_override",
                e,
                "transform: value 9 is NaN, not finite".to_owned(),
            )
        });
        // and a 64-value enum with a full mask is fine
        good.domain = Domain::Enum((0..64).map(|i| format!("v{i}")).collect());
        let mut w = World::new();
        let a = with_axis(&mut w, good);
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        w.insert(
            e,
            Condition(vec![Clause(vec![Literal {
                axis: a,
                values: u64::MAX,
            }])]),
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    #[test]
    fn references_reach_axes_and_targets() {
        let c = castle();
        let reached = c.w.closure([c.intact]);
        let mut keys: Vec<&str> =
            c.w.iter::<StateAxis>()
                .filter(|(e, _)| reached.contains(e))
                .map(|(_, a)| a.key.as_str())
                .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["fromsoft.flag.8302", "fromsoft.flag.8306"]);
    }

    fn registry() -> PassRegistry {
        let mut r = PassRegistry::new();
        r.add(&RESOLVE_STATE);
        r
    }

    fn skate() -> Target {
        Target::new("skate")
            .legal(Transform::NAME)
            .legal(Instance::NAME)
            .legal(Name::NAME)
            .illegal(Condition::NAME)
            .illegal(StateOverride::NAME)
            .illegal(StateAxis::NAME)
    }

    fn dump(w: &World) -> String {
        let mut s = String::new();
        for e in w.entities() {
            for c in w.components_json(e).unwrap() {
                s += &format!("{e:?} {} {} {:?}\n", c.name, c.version, c.value);
            }
        }
        for r in w.resources_json().unwrap() {
            s += &format!("resource {} {} {:?}\n", r.name, r.version, r.value);
        }
        s
    }

    /// A world without state components plans no pass and comes out byte for byte the same.
    #[test]
    fn stateless_world_is_untouched() {
        let mut c = castle();
        run(&mut c.w, "");
        let before = dump(&c.w);
        let m = PassManager::new(registry());
        let report = m
            .convert(&mut c.w, &skate(), Mode::Partial, &PassOptions::new())
            .unwrap();
        assert_eq!(report.pipeline, "");
        assert_eq!(dump(&c.w), before);
    }

    #[test]
    fn planner_resolves_before_a_writer_that_cannot_toggle() {
        let mut c = castle();
        let m = PassManager::new(registry());
        let report = m
            .convert(&mut c.w, &skate(), Mode::Partial, &PassOptions::new())
            .unwrap();
        assert_eq!(report.pipeline, "resolve-state");
        assert_eq!(names(&c.w), ["m003312_3802"]);
    }
}
