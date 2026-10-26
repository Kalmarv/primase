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
pub mod laid;
pub mod layered;
mod layered_lower;
pub mod layout;
pub mod lights;
pub mod lower;
pub mod overlays;
pub mod pause_map;
pub mod raise;
pub mod region;
pub mod repeat_clip;
pub use repeat_clip::{PREPARE_RESKATE_REPEAT_CLIPPING, PrepareReskateRepeatClipping};
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
    FINALIZE_RESKATE_BLEND_BUDGET, FinalizeReskateBlendBudget, PLAN_RESKATE_BLEND_BUDGET,
    PlanReskateBlendBudget,
};
/// Whether a soup (and its rock flags) can be cut by triangle: one surface and one rock flag per triangle, every index a
/// vertex. A pass runs on a world it may not have verified, so what it indexes it checks, naming the entity.
pub(crate) fn check_soup(
    what: &str,
    c: &primase_core::Collision,
    rock: Option<&crate::components::SkateRock>,
) -> Result<(), String> {
    if c.surfaces.len() != c.triangles.len() {
        return Err(format!(
            "{what}: {} surfaces for {} triangles",
            c.surfaces.len(),
            c.triangles.len()
        ));
    }
    if let Some(r) = rock
        && r.0.len() != c.triangles.len()
    {
        return Err(format!(
            "{what}: {} rock flags for {} triangles",
            r.0.len(),
            c.triangles.len()
        ));
    }
    let vertices = c.positions.len();
    if let Some(i) = c.triangles.iter().flatten().find(|&&i| i as usize >= vertices) {
        return Err(format!("{what}: a triangle names vertex {i} of {vertices}"));
    }
    Ok(())
}

/// Keep the triangles of `c` (its surfaces and `rock` flags with them) that `keep` marks, one mark per triangle; the soup was
/// checked with [`check_soup`].
pub(crate) fn keep_triangles(
    c: &mut primase_core::Collision,
    rock: Option<&mut crate::components::SkateRock>,
    keep: &[bool],
) {
    fn cut<T>(list: &mut Vec<T>, keep: &[bool]) {
        let mut marks = keep.iter();
        list.retain(|_| marks.next().copied().unwrap_or(false));
    }
    cut(&mut c.surfaces, keep);
    cut(&mut c.triangles, keep);
    if let Some(r) = rock {
        cut(&mut r.0, keep);
    }
}

pub use blend_order::{PREPARE_RESKATE_BLEND_ORDER, PrepareReskateBlendOrder};
pub use blended::{
    BlendedSelection, PREPARE_RESKATE_BLENDED_SURFACES, PrepareReskateBlendedSurfaces, UniformAlpha,
};
pub use budget::{
    BUDGET_RESKATE_TEXTURES, BudgetReskateTextures, SUMMARIZE_RESKATE_TEXTURES, SummarizeReskateTextures,
};
pub use collision::{
    PREPARE_RESKATE_CELL_COLLISION, PREPARE_RESKATE_COLLISION, PrepareReskateCellCollision,
    PrepareReskateCollision, SHARE_RESKATE_COLLISION, ShareReskateCollision,
};
pub use floor::{
    DECIDE_RESKATE_FLOOR, DecideReskateFloor, PREPARE_RESKATE_FLOOR, PrepareReskateFloor,
    SUMMARIZE_RESKATE_FLOOR, SummarizeReskateFloor,
};
pub use grind_curves::{LOWER_RESKATE_GRIND_CURVES, LowerReskateGrindCurves};
pub use grinds::{PREPARE_RESKATE_GRINDS, PrepareReskateGrinds};
pub use ground_markers::{GROUND_RESKATE_MARKERS, GroundReskateMarkers};
pub use laid::{PREPARE_RESKATE_LAID_LAYERS, PrepareReskateLaidLayers};
pub use layered_lower::LowerLayeredPbr;
pub use layout::{SUMMARIZE_RESKATE_LAYOUT, SummarizeReskateLayout};
pub use lights::{
    SELECT_RESKATE_LIGHTS, SUMMARIZE_RESKATE_LIGHTS, SelectReskateLights, SummarizeReskateLights,
};
pub use overlays::{PREPARE_RESKATE_OVERLAYS, PrepareReskateOverlays};
pub use pause_map::{
    PREPARE_RESKATE_PAUSE_MAP, PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut, PauseSource,
    PrepareReskatePauseMap, SkatePauseImage, SkatePausePicture,
};
pub use raise::{
    LightCalibration, RAISE_SKATE, RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, Unclassified, class_to_skate,
    raise_skate, studio_watts,
};
pub use region::{CLIP_RESKATE_REGION, ClipReskateRegion};
pub use sampling::{
    PREPARE_RESKATE_UV_TRANSFORMS, PREPARE_RESKATE_WRAP, PrepareReskateUvTransforms, PrepareReskateWrap,
    SamplingConflicts,
};
pub use shadows::{LOWER_RESKATE_SHADOWS, LowerReskateShadows};
pub use snap::{
    SNAP_RESKATE_MARKERS, SUMMARIZE_RESKATE_INSTANCES, SUMMARIZE_RESKATE_WATER, SnapReskateMarkers,
    SummarizeReskateInstances, SummarizeReskateWater,
};
pub use spawn::{
    PLACE_RESKATE_SPAWN, PlaceReskateSpawn, ReskateSpawnAt, SUMMARIZE_RESKATE_SPAWN, SummarizeReskateSpawn,
};
pub use summarize::{SUMMARIZE_RESKATE_MAP, SummarizeReskateMap};
pub use textures::{PREPARE_RESKATE_TEXTURES, PrepareReskateTextures, tint_pixels};
pub use unlit::{LOWER_RESKATE_UNLIT, LowerReskateUnlit, UnlitAs};
pub use uv_repeats::{PREPARE_RESKATE_UV_REPEATS, PrepareReskateUvRepeats, RepeatBlended, RepeatDecals};
pub use water::{
    OceanMaterials, PREPARE_RESKATE_WATER, PrepareReskateWater, SUMMARIZE_RESKATE_CONTENT,
    SummarizeReskateContent, WaterExtent,
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
