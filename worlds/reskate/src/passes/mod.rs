//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::components`]) its writers take.

pub mod blend_budget;
pub mod blend_order;
pub mod budget;
pub mod floor;
pub mod grinds;
pub mod layered;
mod layered_lower;
pub mod lights;
pub mod lower;
pub mod overlays;
pub mod pause_map;
pub mod raise;
pub mod region;
pub mod snap;
pub mod spawn;
pub mod summarize;
pub mod textures;
pub mod tint;
pub mod vertex_alpha;
pub mod water;

pub use blend_budget::{PlanReskateBlendBudget, plan_reskate_blend_budget};
pub use blend_order::{PrepareReskateBlendOrder, prepare_reskate_blend_order};
pub use budget::{BudgetReskateTextures, budget_reskate_textures};
pub use floor::{PrepareReskateFloor, prepare_reskate_floor};
pub use grinds::{PrepareReskateGrinds, prepare_reskate_grinds};
pub use layered_lower::LowerLayeredPbr;
pub use lights::{SelectReskateLights, select_reskate_lights};
pub use overlays::{PrepareReskateOverlays, prepare_reskate_overlays};
pub use pause_map::{
    PauseExtent, PauseSource, PrepareReskatePauseMap, SkatePauseImage, SkatePausePicture,
    prepare_reskate_pause_map,
};
pub use raise::{
    RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate, raise_skate_pass, studio_watts,
};
pub use region::{ClipReskateRegion, clip_reskate_region};
pub use snap::{SnapReskateMarkers, snap_reskate_markers};
pub use spawn::{PlaceReskateSpawn, ReskateSpawnAt, place_reskate_spawn};
pub use summarize::{SummarizeReskateMap, summarize_reskate_map};
pub use textures::{PrepareReskateTextures, tint_pixels};
pub use water::{OceanMaterials, PrepareReskateWater, WaterExtent, prepare_reskate_water};

use primase_core::World;
use primase_core::convert::PartScope;

/// A ReSkate map is written whole. A pass that works on the whole map (its floor, its region, its spawn, the marker
/// snaps, its lights, water, pause map, overlays, blend chains and their budget) refuses a part that is not the whole
/// of what is written, as its part source states it (`core.part_scope`'s `whole`): what it would compute from a part
/// (a floor percentile, the collision a snap reaches, a spawn per part, a budget) is not the map's. A world outside the
/// part protocol (no `core.part_scope`: a program composing passes and the writer directly) is the whole map.
pub fn require_whole_map(w: &World, pass: &str) -> Result<(), String> {
    match w.resource::<PartScope>() {
        Some(s) if !s.whole => Err(format!(
            "{pass}: a ReSkate map is written whole, and this world is a part its source does not state is the whole \
             ({} roots, {} entities of other parts as context)",
            s.roots.len(),
            s.context.len()
        )),
        _ => Ok(()),
    }
}
