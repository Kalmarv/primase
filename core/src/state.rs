//! World state: variables a level can be in (a game's event flags, a door's open state, a GTA IPL group, a Unity
//! object's active flag) and what depends on them, resolved to one concrete world by [`ResolveState`].
//!
//! - An **axis** ([`StateAxis`]) is an entity: a stable `key`, a small domain (bool, or up to 64 named values), a
//!   default, a scope (the whole world, or one entity such as a door) and a class tools act on (`door`, `ipl`, ...).
//!   Axes are variables, so independent toggles never multiply into enumerated variants.
//! - A **condition** ([`Condition`]) on a placed entity says when it exists: an OR of clauses, each an AND of literals
//!   "axis is one of these values" (a value mask). No condition: always present.
//! - An **override** ([`StateOverride`]) on a placed entity changes its transform, material variant or prototype in
//!   the states its clause matches. Poses (a door's open animation) stay in the dialect that owns the clip: its own pass
//!   lowers them into these overrides first.
//! - Values come from [`ResolveState`]'s `assign` option, else the [`StateAssignment`] resource (a save snapshot), else
//!   each axis's default.
//!
//! Writers that cannot toggle declare `core.condition` and `core.state_override` illegal, so the planner runs
//! `resolve-state` before them. A world without state components is never touched.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::components::{Instance, Transform};
use crate::json::JsonError;
use crate::pass::{Options, Pass, PassLog};
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
        match self.len() {
            64 => u64::MAX,
            n => (1u64 << n) - 1,
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
    /// Stable and unique in the world (`fromsoft.flag.8302`, `gta.ipl.<name>`): assignments, reports and the text
    /// form name axes by it.
    pub key: String,
    pub domain: Domain,
    /// The value without an assignment: the game's own start state.
    pub default: u32,
    pub scope: Scope,
    /// What kind of state it is (`event`, `door`, `ipl`, `entity_set`, `active`, `time`, ...), so tools can act on a
    /// class ("open every door").
    pub class: String,
    /// Where it came from (`event flag 8302`, an IPL name).
    pub source: String,
}

impl Component for StateAxis {
    const NAME: &'static str = "core.state_axis";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
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
        if self.default as usize >= n {
            return Err(format!(
                "{}: default {} outside its {n} values",
                self.key, self.default
            ));
        }
        if let Scope::Local(owner) = self.scope
            && !w.is_alive(owner)
        {
            return Err(format!("{}: owner {owner:?} is not alive", self.key));
        }
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

/// An AND of literals; an empty clause holds in every state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Clause(pub Vec<Literal>);

impl Clause {
    fn holds(&self, values: &BTreeMap<Entity, u32>) -> bool {
        self.0
            .iter()
            .all(|l| values.get(&l.axis).is_some_and(|&v| l.values & (1u64 << v) != 0))
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

/// When a placed entity exists: an OR of clauses. Zero clauses: never (resolve-state counts these).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Condition(pub Vec<Clause>);

impl Component for Condition {
    const NAME: &'static str = "core.condition";
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        placed(e, w)?;
        self.0.iter().try_for_each(|c| c.verify(w))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        self.0.iter().for_each(|c| c.axes(out));
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
                Override::Prototype(p) if !w.is_alive(*p) => {
                    return Err(format!("prototype override to {p:?}, which is not alive"));
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

/// `resolve-state{assign=key:value;key:value}`: one concrete world. Each axis takes its `assign` value, else the
/// [`StateAssignment`] resource's, else its default (values by name or index). An `assign` key without an axis is an
/// error (a caller's typo); resource keys without an axis are counted and ignored (a save knows every flag). Placed
/// entities whose condition does not hold are despawned, then each remaining entity's first matching override applies.
/// Every axis, condition, override and the assignment resource are removed. A despawned entity that something else
/// still references (a self-placed mesh used as another instance's prototype) leaves that reference dangling, and the
/// verifier after the pass fails on it.
#[derive(Clone, Debug, Default)]
pub struct ResolveState {
    /// key -> value text, as given.
    pub assign: BTreeMap<String, String>,
}

/// The pass factory (`PassRegistry::register("resolve-state", resolve_state)`).
pub fn resolve_state(o: &Options) -> Result<Box<dyn Pass>, String> {
    let mut assign = BTreeMap::new();
    for (k, v) in o {
        match k.as_str() {
            "assign" => {
                for item in v.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                    let (key, value) = item
                        .split_once(':')
                        .ok_or_else(|| format!("assign item {item:?} is not key:value"))?;
                    if assign
                        .insert(key.trim().to_owned(), value.trim().to_owned())
                        .is_some()
                    {
                        return Err(format!("assign: {key} twice"));
                    }
                }
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(ResolveState { assign }))
}

impl Pass for ResolveState {
    fn name(&self) -> &'static str {
        "resolve-state"
    }
    fn options(&self) -> Options {
        if self.assign.is_empty() {
            return Options::new();
        }
        let items: Vec<String> = self.assign.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        Options::from([("assign".to_owned(), items.join(";"))])
    }
    /// The state model's components, and the `core.state_assignment` resource it applied.
    fn removes(&self) -> &[&'static str] {
        &[
            StateAxis::NAME,
            Condition::NAME,
            StateOverride::NAME,
            StateAssignment::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Transform::NAME, Instance::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // the value of every axis: assign option > assignment resource > default
        let axes: Vec<(Entity, StateAxis)> = w.iter::<StateAxis>().map(|(e, a)| (e, a.clone())).collect();
        let by_key: BTreeMap<&str, (Entity, &StateAxis)> =
            axes.iter().map(|(e, a)| (a.key.as_str(), (*e, a))).collect();
        if by_key.len() != axes.len() {
            return Err("two axes share a key".into());
        }
        let mut values: BTreeMap<Entity, u32> = axes.iter().map(|(e, a)| (*e, a.default)).collect();
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
        }
        for (k, text) in &self.assign {
            let &(e, a) = by_key
                .get(k.as_str())
                .ok_or_else(|| format!("assign names {k}, which no axis has"))?;
            let v = a
                .domain
                .parse(text)
                .ok_or_else(|| format!("assign {k}:{text} is not one of its values"))?;
            values.insert(e, v);
        }
        // run() may see an unverified world: every literal must name an axis
        for (e, mut axes_named) in w
            .iter::<Condition>()
            .map(|(e, c)| (e, c.0.clone()))
            .chain(
                w.iter::<StateOverride>()
                    .map(|(e, o)| (e, o.0.iter().map(|(c, _)| c.clone()).collect())),
            )
            .map(|(e, clauses)| {
                (
                    e,
                    clauses
                        .iter()
                        .flat_map(|c| c.0.iter().map(|l| l.axis))
                        .collect::<Vec<_>>(),
                )
            })
        {
            axes_named.retain(|a| !values.contains_key(a));
            if let Some(a) = axes_named.first() {
                return Err(format!("{e:?}: a literal names {a:?}, which is not an axis"));
            }
        }
        let axis_of: BTreeMap<Entity, &StateAxis> = axes.iter().map(|(e, a)| (*e, a)).collect();
        let label = |e: &Entity| {
            let a = axis_of[e];
            format!("{}={}", a.key, a.domain.name(values[e]).unwrap_or("?"))
        };

        // conditions: despawn what does not exist in this state
        let conditions: Vec<(Entity, Condition)> =
            w.iter::<Condition>().map(|(e, c)| (e, c.clone())).collect();
        for (e, c) in &conditions {
            let present = c.0.iter().any(|cl| cl.holds(&values));
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
            if present {
                w.remove::<Condition>(*e);
            } else {
                w.despawn(*e);
            }
        }

        // overrides on what remains
        let overrides: Vec<(Entity, StateOverride)> =
            w.iter::<StateOverride>().map(|(e, o)| (e, o.clone())).collect();
        for (e, o) in overrides {
            w.remove::<StateOverride>(e);
            let Some((_, ov)) = o.0.iter().find(|(c, _)| c.holds(&values)) else {
                continue;
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

        for (e, _) in &axes {
            w.remove::<StateAxis>(*e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Name;
    use crate::pass::{Mode, PassManager, PassRegistry, Target};

    fn axis(w: &mut World, key: &str) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: key.into(),
                domain: Domain::Bool,
                default: 0,
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

    /// The Sekiro castle: intact walls (entity group 1117201: disabled while 8306 or 8302 is on), the same walls
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
        resolve_state(&opts).unwrap().run(w, &mut log).unwrap();
        log
    }

    fn names(w: &World) -> Vec<String> {
        w.iter::<Instance>()
            .map(|(e, _)| w.get::<Name>(e).unwrap().0.clone())
            .collect()
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
        let err = resolve_state(&Options::new())
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
            resolve_state(&Options::new())
                .unwrap()
                .run(&mut w, &mut PassLog::default())
                .is_err()
        );
    }

    #[test]
    fn unknown_keys_and_values_are_errors() {
        for bad in [
            "fromsoft.flag.9999:on",
            "fromsoft.flag.8302:maybe",
            "fromsoft.flag.8302:2",
        ] {
            let mut c = castle();
            let opts = Options::from([("assign".to_owned(), bad.to_owned())]);
            assert!(
                resolve_state(&opts)
                    .unwrap()
                    .run(&mut c.w, &mut PassLog::default())
                    .is_err(),
                "{bad}"
            );
        }
        let mut c = castle();
        // a resource value outside the axis's domain is an error (only unknown resource keys are tolerated)
        c.w.insert_resource(StateAssignment(BTreeMap::from([(
            "fromsoft.flag.8302".to_owned(),
            2,
        )])));
        assert!(
            resolve_state(&Options::new())
                .unwrap()
                .run(&mut c.w, &mut PassLog::default())
                .is_err()
        );
        assert!(resolve_state(&Options::from([("assign".to_owned(), "x".to_owned())])).is_err());
        assert!(resolve_state(&Options::from([("colour".to_owned(), "red".to_owned())])).is_err());
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

    /// One failing world per rule `verify()` enforces.
    #[test]
    fn verify_rejects_each_rule() {
        let failing = |build: &dyn Fn(&mut World)| {
            let mut w = World::new();
            build(&mut w);
            assert!(!w.verify_components().is_empty(), "should fail");
        };
        let mut good = StateAxis {
            key: "k".into(),
            domain: Domain::Bool,
            default: 0,
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
            with_axis(
                w,
                StateAxis {
                    key: String::new(),
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            with_axis(
                w,
                StateAxis {
                    default: 2,
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec![]),
                    default: 0,
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            let names = (0..65).map(|i| format!("v{i}")).collect();
            with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(names),
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec!["a".into(), "a".into()]),
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            with_axis(
                w,
                StateAxis {
                    domain: Domain::Enum(vec!["a:b".into()]),
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            with_axis(
                w,
                StateAxis {
                    key: "k;x".into(),
                    ..good.clone()
                },
            );
        });
        failing(&|w| {
            let gone = w.spawn();
            w.despawn(gone);
            with_axis(
                w,
                StateAxis {
                    scope: Scope::Local(gone),
                    ..good.clone()
                },
            );
        });
        // literals
        let cond = |w: &mut World, lit: Literal, transform: bool| {
            let e = w.spawn();
            if transform {
                w.insert(e, Transform::IDENTITY);
            }
            w.insert(e, Condition(vec![Clause(vec![lit])]));
        };
        failing(&|w| {
            let a = with_axis(w, good.clone());
            cond(w, Literal { axis: a, values: 0 }, true);
        });
        failing(&|w| {
            let a = with_axis(w, good.clone());
            cond(
                w,
                Literal {
                    axis: a,
                    values: 0b100,
                },
                true,
            );
        });
        failing(&|w| {
            let not_axis = w.spawn();
            w.insert(not_axis, Name("x".into()));
            cond(
                w,
                Literal {
                    axis: not_axis,
                    values: 1,
                },
                true,
            );
        });
        failing(&|w| {
            let a = with_axis(w, good.clone());
            cond(w, on(a), false);
        });
        // overrides
        failing(&|w| {
            let e = w.spawn();
            w.insert(e, Transform::IDENTITY);
            w.insert(e, StateOverride(vec![(Clause::default(), Override::Variant(1))]));
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
        assert!(
            c.w.iter::<StateAxis>()
                .filter(|(e, _)| reached.contains(e))
                .count()
                == 2,
            "8306 and 8302"
        );
    }

    fn registry() -> PassRegistry {
        let mut r = PassRegistry::new();
        r.register("resolve-state", resolve_state);
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
        let report = m.convert(&mut c.w, &skate(), Mode::Partial).unwrap();
        assert_eq!(report.pipeline, "");
        assert_eq!(dump(&c.w), before);
    }

    #[test]
    fn planner_resolves_before_a_writer_that_cannot_toggle() {
        let mut c = castle();
        let m = PassManager::new(registry());
        let report = m.convert(&mut c.w, &skate(), Mode::Partial).unwrap();
        assert_eq!(report.pipeline, "resolve-state");
        assert_eq!(names(&c.w), ["m003312_3802"]);
    }
}
