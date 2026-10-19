//! Authored placement optionality, independent of any consumer's filtering policy, and the explicit pass that
//! filters by it (`drop-optional`).

use std::collections::BTreeSet;

use crate::pass::{Options, Pass, PassLog};
use crate::{Component, Entity, Instance, JsonError, Mesh, MeshFile, Transform, World};
use serde::Serialize;
use serde_json::Value;

/// How optional a placement is (`core.priority`): 0 is required and higher values
/// are more optional. Absence uses the required-placement behavior.
///
/// The dimensionless range is 0-255. This component does not filter placements;
/// a consumer with narrower storage must reject values it cannot preserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Priority(pub u8);

impl Component for Priority {
    const NAME: &'static str = "core.priority";
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }

    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }

    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        if world.has::<Instance>(entity)
            || (world.has::<Transform>(entity)
                && (world.has::<Mesh>(entity) || world.has::<MeshFile>(entity)))
        {
            Ok(())
        } else {
            Err("priority on an entity that is not placed".into())
        }
    }
}

/// `drop-optional{max=N}`: every placement more optional than `max` (its [`Priority`] above it) is despawned with
/// everything it holds, counted per priority; the others are untouched. Without options `max` is 255 and nothing is
/// dropped: leaving placements out is always a pipeline's explicit choice. A dropped entity that a kept one refers to
/// (a self-placed mesh other placements use as their prototype) is refused, and nothing is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropOptional {
    pub max: u8,
}

impl Default for DropOptional {
    fn default() -> Self {
        DropOptional { max: u8::MAX }
    }
}

/// The pass factory (`PassRegistry::register("drop-optional", drop_optional)`).
pub fn drop_optional(o: &Options) -> Result<Box<dyn Pass>, String> {
    let mut pass = DropOptional::default();
    for (k, v) in o {
        match k.as_str() {
            "max" => {
                pass.max = v
                    .parse()
                    .map_err(|_| format!("max={v} is not a priority 0-255"))?
            }
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(Box::new(pass))
}

impl Pass for DropOptional {
    fn name(&self) -> &'static str {
        "drop-optional"
    }
    fn options(&self) -> Options {
        if self.max == u8::MAX {
            Options::new()
        } else {
            Options::from([("max".to_owned(), self.max.to_string())])
        }
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let dropped: Vec<(Entity, u8)> = w
            .iter::<Priority>()
            .filter(|(_, p)| p.0 > self.max)
            .map(|(e, p)| (e, p.0))
            .collect();
        let gone: BTreeSet<Entity> = dropped.iter().map(|(e, _)| *e).collect();
        if let Some((name, holder, target)) = w
            .references_to(&gone)
            .into_iter()
            .find(|(_, holder, _)| !gone.contains(holder))
        {
            return Err(format!(
                "{name} on {holder:?} refers to {target:?}, a placement above priority {}",
                self.max
            ));
        }
        for (e, p) in dropped {
            crate::ledger::despawn(w, e, log, &format!("placements at priority {p} left out"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Name;
    use crate::pass::{PassManager, PassRegistry, RunReport};

    #[test]
    fn priority_preserves_full_range_and_requires_a_placement() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Priority(255));
        assert_eq!(world.verify_components().len(), 1);
        world.insert(entity, Mesh::default());
        assert_eq!(world.verify_components().len(), 1);
        world.insert(entity, Transform::IDENTITY);
        assert!(world.verify_components().is_empty());
        for value in [0, 15, 16, 255] {
            assert_eq!(Priority(value).to_json().unwrap(), Some(Value::from(value)));
        }
    }

    /// A prototype placed at no priority, 0, 2 and 3 (named "optional"), and maximum 2.
    fn placed_world() -> (World, Entity, Vec<Entity>) {
        let mut w = World::new();
        let proto = w.spawn();
        w.insert(proto, Mesh::default());
        let mut placed = Vec::new();
        for p in [None, Some(0), Some(2), Some(3)] {
            let e = w.spawn();
            w.insert(
                e,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(e, crate::Transform::IDENTITY);
            if let Some(p) = p {
                w.insert(e, Priority(p));
            }
            if p == Some(3) {
                w.insert(e, Name("optional".into()));
            }
            placed.push(e);
        }
        (w, proto, placed)
    }

    #[test]
    fn drop_optional_leaves_out_placements_above_the_maximum_through_the_manager() {
        let (mut w, _, placed) = placed_world();
        let mut registry = PassRegistry::new();
        registry.register("drop-optional", drop_optional);
        let m = PassManager::new(registry);
        let pipeline = m.registry.parse("drop-optional{max=2}").unwrap();
        let mut report = RunReport::default();
        m.run(&mut w, &pipeline, &mut report).unwrap();
        let step = &report.steps[0];
        assert_eq!(step.pass, "drop-optional{max=2}");
        assert_eq!(
            step.removed,
            ["core.name"],
            "only the dropped placement had a name"
        );
        assert!(step.log.counts.is_empty());
        assert_eq!(
            step.log.drops,
            [(
                (
                    crate::ledger::Measure::Placements,
                    "placements at priority 3 left out".to_owned()
                ),
                1
            )]
            .into()
        );
        assert_eq!(
            placed.iter().map(|e| w.is_alive(*e)).collect::<Vec<_>>(),
            [true, true, true, false]
        );
        // without options nothing goes
        let (mut w, _, _) = placed_world();
        let all = w.entity_count();
        let pipeline = m.registry.parse("drop-optional").unwrap();
        m.run(&mut w, &pipeline, &mut RunReport::default()).unwrap();
        assert_eq!(w.entity_count(), all);
        assert!(drop_optional(&Options::from([("max".into(), "256".into())])).is_err());
        assert!(drop_optional(&Options::from([("level".into(), "2".into())])).is_err());
    }

    #[test]
    fn a_dropped_placement_something_kept_refers_to_is_refused() {
        let (mut w, proto, placed) = placed_world();
        // the prototype is placed as itself too, at priority 5: its placements refer to it
        w.insert(proto, Transform::IDENTITY);
        w.insert(proto, Priority(5));
        let err = DropOptional { max: 2 }
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(
            err.contains("core.instance") && err.contains("above priority 2"),
            "{err}"
        );
        assert!(
            w.is_alive(proto) && placed.iter().all(|e| w.is_alive(*e)),
            "nothing dropped"
        );
    }
}
