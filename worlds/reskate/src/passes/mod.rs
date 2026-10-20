//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::components`]) its writers take.

pub mod blend_budget;
pub mod blend_order;
pub mod blended;
pub mod budget;
pub mod collision;
pub mod floor;
pub mod grind_curves;
pub mod grinds;
pub mod ground_markers;
pub mod layered;
mod layered_lower;
pub mod layout;
pub mod lights;
pub mod lower;
pub mod overlays;
pub mod pause_map;
pub mod raise;
pub mod region;
pub mod sampling;
pub mod shadows;
pub mod snap;
pub mod spawn;
pub mod summarize;
pub mod textures;
pub mod tint;
pub mod unlit;
pub mod uv_repeats;
pub mod vertex_alpha;
pub mod water;

pub use blend_budget::{
    FinalizeReskateBlendBudget, PlanReskateBlendBudget, finalize_reskate_blend_budget,
    plan_reskate_blend_budget,
};
pub use blend_order::{PrepareReskateBlendOrder, prepare_reskate_blend_order};
pub use blended::{
    BlendedSelection, PrepareReskateBlendedSurfaces, UniformAlpha, prepare_reskate_blended_surfaces,
};
pub use budget::{
    BudgetReskateTextures, SummarizeReskateTextures, budget_reskate_textures, summarize_reskate_textures,
};
pub use collision::{
    PrepareReskateCellCollision, PrepareReskateCollision, ShareReskateCollision,
    prepare_reskate_cell_collision, prepare_reskate_collision, share_reskate_collision,
};
pub use floor::{
    DecideReskateFloor, PrepareReskateFloor, SummarizeReskateFloor, decide_reskate_floor,
    prepare_reskate_floor, summarize_reskate_floor,
};
pub use grind_curves::LowerReskateGrindCurves;
pub use grinds::{PrepareReskateGrinds, prepare_reskate_grinds};
pub use ground_markers::GroundReskateMarkers;
pub use layered_lower::LowerLayeredPbr;
pub use layout::{SummarizeReskateLayout, summarize_reskate_layout};
pub use lights::{
    SelectReskateLights, SummarizeReskateLights, select_reskate_lights, summarize_reskate_lights,
};
pub use overlays::{PrepareReskateOverlays, prepare_reskate_overlays};
pub use pause_map::{
    PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut, PauseSource, PrepareReskatePauseMap,
    SkatePauseImage, SkatePausePicture, prepare_reskate_pause_map,
};
pub use raise::{
    LightCalibration, RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate,
    raise_skate_pass, studio_watts,
};
pub use region::{ClipReskateRegion, clip_reskate_region};
pub use sampling::{
    PrepareReskateUvTransforms, PrepareReskateWrap, SamplingConflicts, prepare_reskate_uv_transforms,
    prepare_reskate_wrap,
};
pub use shadows::LowerReskateShadows;
pub use snap::{
    SnapReskateMarkers, SummarizeReskateInstances, SummarizeReskateWater, snap_reskate_markers,
    summarize_reskate_instances, summarize_reskate_water,
};
pub use spawn::{
    PlaceReskateSpawn, ReskateSpawnAt, SummarizeReskateSpawn, place_reskate_spawn, summarize_reskate_spawn,
};
pub use summarize::{SummarizeReskateMap, summarize_reskate_map};
pub use textures::{PrepareReskateTextures, tint_pixels};
pub use unlit::{LowerReskateUnlit, UnlitAs, lower_reskate_unlit};
pub use uv_repeats::{PrepareReskateUvRepeats, RepeatBlended, RepeatDecals, prepare_reskate_uv_repeats};
pub use water::{
    OceanMaterials, PrepareReskateWater, SummarizeReskateContent, WaterExtent, prepare_reskate_water,
    summarize_reskate_content,
};

use std::collections::BTreeSet;

use primase_core::convert::PartScope;
use primase_core::{Entity, World};

/// What a world owns: a part's output as core conversion writes it ([`PartScope::output`]): the closure of its roots
/// (what they reach, a dependency shared with another part included) and of what its passes declared as its output
/// (placed; content placed from an entity it only loaded as context is that part's, and content made and not placed is
/// not output). Every entity of a world outside the part protocol. Each owned entity has a key: its planning id when it
/// was loaded (the same in every part, so a dependency two parts both reach is one key, merged once), or, made in this
/// part, its index tagged by the part's smallest root (made ids of different parts may coincide).
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
        let Some(s) = w.resource::<PartScope>() else {
            return Owned::all();
        };
        Owned {
            part: Some(OwnedPart {
                owned: s.output(w).keep,
                first_made: s.first_made.unwrap_or(u32::MAX),
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
/// light cap, the texture budget, water's content extent, the spawn, the pause map's content extent and the marker
/// snaps' instancing and water take their decisions from summaries merged over every part
/// ([`primase_core::convert::convert_parts_summarized`]), and refuse a part without them through this. The overlays and
/// blend chains are each part's own; the map's summary is each part's, counted with its collision cells apart, and the
/// blend budget is decided once from the merged summaries (`finalize-reskate-blend-budget`), in the finalize stage of
/// [`primase_core::convert::convert_parts_gathered`]; the collision plan is left to the writer, which decides it over
/// the whole map by the pass's own rule (the map sink); the region clip and water's other extents are local. A world outside the part protocol (no `core.part_scope`: a program composing passes and the writer
/// directly) is the whole map.
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
