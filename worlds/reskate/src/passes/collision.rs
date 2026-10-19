//! `prepare-reskate-collision{surfaces=coalesce|exact,weld=mm|exact,tile,min_instances,min_instance_tris,
//! max_render_edge,limit,offset_x,offset_y}`: how the map's collision is written, `skate.collision_plan`. The writer lays
//! the collision out into leaves (each 256 m cell with the placed props' collision, cut into quadrants until sparse
//! enough for skate.'s grind-edge analysis) and writes each leaf as one piece per surface; this pass lays it out the
//! same way for the writer settings given (each the encoding's default when not given), nothing encoded, and decides
//! per leaf:
//!
//! - `surfaces=coalesce`: a standard surface with fewer than max(8, 1/20 of the leaf's) triangles is written as the
//!   leaf's most common standard surface of its kind (soft or hard), fewer pieces for a little surface fidelity;
//!   reported as an approximation per (surface, the surface written as). `exact`: each triangle keeps its surface.
//! - `weld=mm`: a piece's corners within the same millimetre merge, and the triangles that collapse are left out,
//!   declared as dropped collision triangles (found by running the weld here). `exact`: only bit-identical corners
//!   merge, every triangle kept.
//!
//! The writer replays the plan, refusing one made for another layout (a leaf's triangles per surface, the surface
//! table); without a plan it writes the collision exactly (as `surfaces=exact,weld=exact`). The writer used to coalesce
//! and weld by itself. It runs on the whole map, after every pass that changes the map's collision or what the writer
//! groups, and before `summarize-reskate-map` (whose count of the collision's cull IDs is the planned collision's).

use primase_core::World;
use primase_core::ledger::Measure;
use primase_core::pass::{Options, Pass, PassLog};

use crate::assembly::{EncodingOptions, collision_plan};
use crate::components::{CollisionSurfaces, CollisionWeld, SkateCollisionPlan};

/// See the module documentation. Made by [`PrepareReskateCollision::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateCollision {
    surfaces: CollisionSurfaces,
    weld: CollisionWeld,
    encoding: EncodingOptions,
}

impl PrepareReskateCollision {
    /// The surfaces and weld, both stated, for the map written as `encoding` says (its grouping); an error for an
    /// encoding whose grouping is not one (non-finite or non-positive metres, negative counts).
    pub fn new(
        surfaces: CollisionSurfaces,
        weld: CollisionWeld,
        encoding: &EncodingOptions,
    ) -> Result<PrepareReskateCollision, String> {
        super::overlays::check_grouping(encoding)?;
        Ok(PrepareReskateCollision {
            surfaces,
            weld,
            encoding: super::overlays::grouping_only(encoding),
        })
    }

    /// From its options: `surfaces=coalesce|exact` and `weld=mm|exact`, both required, and the writer's grouping
    /// (`tile`, `min_instances`, `min_instance_tris`, `max_render_edge`, `limit`, `offset_x`, `offset_y`), each the
    /// encoding's default when not given.
    pub fn from_options(o: &Options) -> Result<PrepareReskateCollision, String> {
        let mut grouping = o.clone();
        let surfaces = match grouping.remove("surfaces").as_deref() {
            Some("coalesce") => CollisionSurfaces::Coalesce,
            Some("exact") => CollisionSurfaces::Exact,
            Some(v) => return Err(format!("surfaces={v}: coalesce or exact")),
            None => return Err("surfaces is required: coalesce or exact".into()),
        };
        let weld = match grouping.remove("weld").as_deref() {
            Some("mm") => CollisionWeld::Millimetre,
            Some("exact") => CollisionWeld::Exact,
            Some(v) => return Err(format!("weld={v}: mm or exact")),
            None => return Err("weld is required: mm or exact".into()),
        };
        PrepareReskateCollision::new(surfaces, weld, &super::overlays::encoding_of(&grouping)?)
    }
}

/// [`PrepareReskateCollision::from_options`] as a pass factory.
pub fn prepare_reskate_collision(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateCollision::from_options(o)?))
}

impl Pass for PrepareReskateCollision {
    fn name(&self) -> &'static str {
        "prepare-reskate-collision"
    }
    fn options(&self) -> Options {
        let mut o = super::overlays::encoding_options(&self.encoding);
        o.insert(
            "surfaces".into(),
            match self.surfaces {
                CollisionSurfaces::Coalesce => "coalesce",
                CollisionSurfaces::Exact => "exact",
            }
            .into(),
        );
        o.insert(
            "weld".into(),
            match self.weld {
                CollisionWeld::Millimetre => "mm",
                CollisionWeld::Exact => "exact",
            }
            .into(),
        );
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.collision_plan"]
    }
    fn approximates(&self) -> &'static str {
        "minor collision surfaces of a leaf written as its most common one"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        super::require_whole_map(w, self.name())?;
        if w.resource::<SkateCollisionPlan>().is_some() {
            return Err(
                "prepare-reskate-collision: the collision is already planned (skate.collision_plan)".into(),
            );
        }
        let (plan, rewritten) =
            collision_plan(w, &self.encoding, self.surfaces, self.weld).map_err(|e| e.to_string())?;
        for ((from, to), n) in rewritten {
            log.approximate(
                &format!("collision triangles of surface {from} written as their leaf's {to}"),
                n as u64,
            );
        }
        log.dropped(
            Measure::CollisionTriangles,
            "collision triangles the 1 mm weld collapses",
            plan.leaves.values().map(|l| l.collapsed as u64).sum(),
        );
        log.count("collision leaves", plan.leaves.len() as u64);
        w.insert_resource(plan);
        Ok(())
    }
}
