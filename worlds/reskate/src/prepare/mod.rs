//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::dialect`]) its writers take.

pub mod budget;
pub mod floor;
pub mod grinds;
pub mod lights;
pub mod overlays;
pub mod raise;
pub mod region;
pub mod snap;
pub mod spawn;
pub mod textures;
pub mod water;

pub use budget::{BudgetReskateTextures, budget_reskate_textures};
pub use floor::{PrepareReskateFloor, prepare_reskate_floor};
pub use grinds::{PrepareReskateGrinds, prepare_reskate_grinds};
pub use lights::{SelectReskateLights, select_reskate_lights};
pub use overlays::{PrepareReskateOverlays, prepare_reskate_overlays};
pub use raise::{
    RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate, raise_skate_pass, studio_watts,
};
pub use region::{ClipReskateRegion, clip_reskate_region};
pub use snap::{SnapReskateMarkers, snap_reskate_markers};
pub use spawn::{PlaceReskateSpawn, ReskateSpawnAt, place_reskate_spawn};
pub use textures::{PrepareReskateTextures, tint_pixels};
pub use water::{OceanMaterials, PrepareReskateWater, WaterExtent, prepare_reskate_water};

use primase_core::World;
use primase_core::convert::{PartScope, PartitionPlan};

/// A ReSkate map is one part. A pass that works on the whole map (its floor, its region, its spawn, the marker
/// snaps) refuses a world that is one part of several: one whose `core.part_scope` loaded context from other parts,
/// or whose `core.partition_plan` has more than one group. What it would compute from a part (a floor percentile,
/// the collision a snap reaches, a spawn per part) is not the map's, until whole-map summaries reach a part.
pub fn require_one_part(w: &World, pass: &str) -> Result<(), String> {
    if let Some(s) = w.resource::<PartScope>()
        && !s.context.is_empty()
    {
        return Err(format!(
            "{pass}: a ReSkate map is one part, and this world is a part with {} entities of other parts as context",
            s.context.len()
        ));
    }
    if let Some(p) = w.resource::<PartitionPlan>()
        && p.groups.len() > 1
    {
        return Err(format!(
            "{pass}: a ReSkate map is one part, and this world is planned in {} parts ({})",
            p.groups.len(),
            p.by
        ));
    }
    Ok(())
}
