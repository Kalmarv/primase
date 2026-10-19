//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::components`]) its writers take.

pub mod blend_budget;
pub mod blend_order;
pub mod budget;
pub mod collision;
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
pub mod uv_repeats;
pub mod vertex_alpha;
pub mod water;

pub use blend_budget::{PlanReskateBlendBudget, plan_reskate_blend_budget};
pub use blend_order::{PrepareReskateBlendOrder, prepare_reskate_blend_order};
pub use budget::{
    BudgetReskateTextures, SummarizeReskateTextures, budget_reskate_textures, summarize_reskate_textures,
};
pub use collision::{PrepareReskateCollision, prepare_reskate_collision};
pub use floor::{
    DecideReskateFloor, PrepareReskateFloor, SummarizeReskateFloor, decide_reskate_floor,
    prepare_reskate_floor, summarize_reskate_floor,
};
pub use grinds::{PrepareReskateGrinds, prepare_reskate_grinds};
pub use layered_lower::LowerLayeredPbr;
pub use lights::{
    SelectReskateLights, SummarizeReskateLights, select_reskate_lights, summarize_reskate_lights,
};
pub use overlays::{PrepareReskateOverlays, prepare_reskate_overlays};
pub use pause_map::{
    PauseExtent, PauseSource, PrepareReskatePauseMap, SkatePauseImage, SkatePausePicture,
    prepare_reskate_pause_map,
};
pub use raise::{
    LightCalibration, RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate,
    raise_skate_pass, studio_watts,
};
pub use region::{ClipReskateRegion, clip_reskate_region};
pub use snap::{SnapReskateMarkers, snap_reskate_markers};
pub use spawn::{
    PlaceReskateSpawn, ReskateSpawnAt, SummarizeReskateSpawn, place_reskate_spawn, summarize_reskate_spawn,
};
pub use summarize::{SummarizeReskateMap, summarize_reskate_map};
pub use textures::{PrepareReskateTextures, tint_pixels};
pub use uv_repeats::{PrepareReskateUvRepeats, RepeatBlended, RepeatDecals, prepare_reskate_uv_repeats};
pub use water::{
    OceanMaterials, PrepareReskateWater, SummarizeReskateContent, WaterExtent, prepare_reskate_water,
    summarize_reskate_content,
};

use std::collections::BTreeSet;

use primase_core::convert::PartScope;
use primase_core::{Entity, World};

/// What a world owns, as core conversion defines a part's output: the closure of its roots (what they reach, a
/// dependency shared with another part included) and what its passes made that its context's closure does not hold;
/// every entity of a world outside the part protocol or of a whole part. What it only loaded as context is another
/// part's. Each owned entity has a key: its planning id when it was loaded (the same in every part, so a dependency two
/// parts both reach is one key, merged once), or, made in this part, its index tagged by the part's smallest root (made
/// ids of different parts may coincide).
#[derive(Debug)]
pub(crate) struct Owned {
    /// `None`: every entity, keyed by its index.
    part: Option<OwnedPart>,
}

#[derive(Debug)]
struct OwnedPart {
    owned: BTreeSet<Entity>,
    first_made: u32,
    tag: u64,
}

impl Owned {
    /// Every entity, each keyed by its index.
    pub(crate) fn all() -> Owned {
        Owned { part: None }
    }

    /// What `w` owns (see [`Owned`]).
    pub(crate) fn of(w: &World) -> Owned {
        let Some(s) = w.resource::<PartScope>().filter(|s| !s.whole) else {
            return Owned::all();
        };
        let mut owned = w.closure(s.roots.iter().copied());
        let first_made = s.first_made.unwrap_or(u32::MAX);
        let reached = w.closure(s.roots.iter().chain(&s.context).copied());
        owned.extend(
            w.entities()
                .filter(|e| e.index() >= first_made && !reached.contains(e)),
        );
        Owned {
            part: Some(OwnedPart {
                owned,
                first_made,
                tag: s.roots.first().map_or(0, |e| u64::from(e.index()) + 1),
            }),
        }
    }

    pub(crate) fn contains(&self, e: Entity) -> bool {
        self.part.as_ref().is_none_or(|p| p.owned.contains(&e))
    }

    /// `e`'s key, if owned.
    pub(crate) fn key(&self, e: Entity) -> Option<u64> {
        match &self.part {
            None => Some(u64::from(e.index())),
            Some(p) if !p.owned.contains(&e) => None,
            Some(p) if e.index() >= p.first_made => Some(p.tag << 32 | u64::from(e.index())),
            Some(_) => Some(u64::from(e.index())),
        }
    }
}

/// A pass that works on the whole map refuses a part that is not the whole of what is written, as its part source
/// states it (`core.part_scope`'s `whole`), unless the decision was made over every part: what it would compute from a
/// part (a floor percentile, the collision a snap reaches, a spawn per part, a budget) is not the map's. The floor, the
/// light cap, the texture budget, water's content extent and the spawn take their decisions from summaries merged over
/// every part ([`primase_core::convert::convert_parts_summarized`]); the marker snaps, the pause map, overlays, blend
/// chains and their budget, the collision plan and the map's summary refuse parts until the writer lays out parts
/// (B2c part 2, step 9 incomplete); the region clip and water's other extents are local. A world outside the part protocol (no
/// `core.part_scope`: a program composing passes and the writer directly) is the whole map.
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
