//! `drop-fromsoft-far`: the distant-backdrop prototypes and their placements dropped by name.
use crate::sekiro::source::PrototypeSource;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Component, Entity, Instance, World};
use std::collections::BTreeSet;

/// `drop-fromsoft-far`: the placements of distant-backdrop prototypes (`fromsoft.prototype` `far`) dropped by name
/// (a named drop of each placement), and those prototypes despawned, counted (they hold no core content yet);
/// nothing else changes.
#[derive(Clone, Copy, Debug)]
pub struct DropFromSoftFar;

impl Pass for DropFromSoftFar {
    fn name(&self) -> &'static str {
        "drop-fromsoft-far"
    }
    fn requires(&self) -> &[&'static str] {
        &[PrototypeSource::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let far: BTreeSet<Entity> = world
            .iter::<PrototypeSource>()
            .filter(|(_, s)| s.far)
            .map(|(e, _)| e)
            .collect();
        let placed: Vec<Entity> = world
            .iter::<Instance>()
            .filter(|(_, i)| far.contains(&i.prototype))
            .map(|(e, _)| e)
            .collect();
        for e in placed {
            primase_core::ledger::despawn(world, e, log, "distant-backdrop placements");
        }
        for e in far {
            primase_core::ledger::despawn(world, e, log, "distant-backdrop prototypes");
            log.count("distant-backdrop prototypes dropped", 1);
        }
        Ok(())
    }
}
