//! `gta-leave-out-interiors`: the placements an interior holds, left out.
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Entity, World};

use crate::components::GtaEntity;

/// `gta-leave-out-interiors`: placements of entities placed in an interior are despawned (counted). The game draws an
/// interior only from inside it or through its portals; their prototypes stay for other placements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaveOutInteriors;

/// `gta-leave-out-interiors`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, an interior
/// placement is the game's.
pub const GTA_LEAVE_OUT_INTERIORS: PassSpec = PassSpec {
    name: "gta-leave-out-interiors",
    options: &[],
    make: |_| Ok(Box::new(LeaveOutInteriors)),
};

impl Pass for LeaveOutInteriors {
    fn name(&self) -> &'static str {
        "gta-leave-out-interiors"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.entity"]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let inside: Vec<Entity> = w
            .iter::<GtaEntity>()
            .filter(|(_, g)| g.interior.is_some())
            .map(|(e, _)| e)
            .collect();
        for e in inside {
            primase_core::ledger::despawn(w, e, log, "placements in an interior left out");
        }
        Ok(())
    }
}
