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
//! The writer replays the plan, refusing one whose aggregate check fails (the surface table, the leaves, each leaf's
//! triangles per surface, its weld's collapses: not a fingerprint of the geometry, so run this pass after every pass
//! that changes the collision); without a plan it writes the collision exactly (as `surfaces=exact,weld=exact`). The writer used to coalesce
//! and weld by itself. It runs on the whole map, after every pass that changes the map's collision or what the writer
//! groups, and before `summarize-reskate-map` (whose count of the collision's cull IDs is the planned collision's).
//!
//! A map converted in parts plans its collision cell by cell: each part shares what it lays out in each 256 m cell
//! (`share-reskate-collision`, in the gather traversal), the shares of a cell are gathered from every part (the cell
//! spool), and `prepare-reskate-cell-collision{surfaces,weld}`, run on the cell (the group pipeline), plans the cell's
//! leaves by the same rule, the cell laid out as the writer lays it out, and records what the cell is written as
//! (`skate.cell_counts`: its pieces and their cull IDs). `prepare-reskate-collision` refuses a part of a map in several:
//! it holds none of its boundary cells whole.
//!
//! `share-reskate-collision{tile,min_instances,min_instance_tris,max_render_edge,limit,offset_x,offset_y}`: what this
//! part lays out in each collision cell, as the map sink lays the part out for the writer settings given (the region,
//! instancing and preparation records as it takes them, the map's from the summaries when in parts), nothing encoded:
//! an entity per cell with its `skate.cell_share` (its world soups' triangles there and its tiles' props), placed for
//! the part, and the cell layout beside them (`skate.cell_layout`). It runs in the gather traversal, after every pass
//! that changes the map's collision or what the writer groups; the hulls are not shared (each part writes its own
//! whole).

use primase_core::World;
use primase_core::convert::PartScope;
use primase_core::ledger::Measure;
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};

use crate::assembly::{EncodingOptions, cell_plan, cell_shares, collision_plan};
use crate::components::SkateCellLayout;
use crate::components::{CollisionSurfaces, CollisionWeld, SkateCollisionPlan};

use super::overlays::{encoding_from, with_encoding};

impl Choice for CollisionSurfaces {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("coalesce", CollisionSurfaces::Coalesce),
        ("exact", CollisionSurfaces::Exact),
    ];
}

impl Choice for CollisionWeld {
    const VALUES: &'static [(&'static str, Self)] =
        &[("mm", CollisionWeld::Millimetre), ("exact", CollisionWeld::Exact)];
}

/// `surfaces` and `weld`, as `prepare-reskate-collision` and `prepare-reskate-cell-collision` take them: the program's,
/// no default.
const SURFACES_AND_WELD: [OptionSpec; 2] = [
    OptionSpec::choice::<CollisionSurfaces>(
        "surfaces",
        "a leaf's minor surfaces written as its most common one (coalesce), or each triangle's kept (exact)",
    ),
    OptionSpec::choice::<CollisionWeld>(
        "weld",
        "corners within the same millimetre merged (mm), or only bit-identical ones (exact)",
    ),
];

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

    /// From its options: `surfaces` and `weld`, and the writer's grouping ([`encoding_from`]).
    fn from_given(g: &Given) -> Result<PrepareReskateCollision, String> {
        PrepareReskateCollision::new(g.choice("surfaces"), g.choice("weld"), &encoding_from(g)?)
    }
}

const COLLISION_OPTIONS: [OptionSpec; 9] = with_encoding(SURFACES_AND_WELD);

/// `prepare-reskate-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): the surfaces and weld are the
/// program's, no default; the grouping the writer's unless stated.
pub const PREPARE_RESKATE_COLLISION: PassSpec = PassSpec {
    name: "prepare-reskate-collision",
    options: &COLLISION_OPTIONS,
    make: |g| Ok(Box::new(PrepareReskateCollision::from_given(g)?)),
};

/// The plan `plan` recorded, what it rewrites reported as approximations and what its weld collapses as dropped.
fn record(w: &mut World, log: &mut PassLog, plan: SkateCollisionPlan, rewritten: crate::assembly::Rewritten) {
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
}

/// `prepare-reskate-cell-collision{surfaces=coalesce|exact,weld=mm|exact}`: one collision cell of a map in parts planned
/// (see the module documentation), the surfaces and weld as `prepare-reskate-collision` takes them. Made by
/// [`PrepareReskateCellCollision::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateCellCollision {
    surfaces: CollisionSurfaces,
    weld: CollisionWeld,
}

impl PrepareReskateCellCollision {
    pub fn new(surfaces: CollisionSurfaces, weld: CollisionWeld) -> PrepareReskateCellCollision {
        PrepareReskateCellCollision { surfaces, weld }
    }
}

/// `prepare-reskate-cell-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): the surfaces and weld
/// are the program's, no default.
pub const PREPARE_RESKATE_CELL_COLLISION: PassSpec = PassSpec {
    name: "prepare-reskate-cell-collision",
    options: &SURFACES_AND_WELD,
    make: |g| {
        Ok(Box::new(PrepareReskateCellCollision::new(
            g.choice("surfaces"),
            g.choice("weld"),
        )))
    },
};

impl Pass for PrepareReskateCellCollision {
    fn name(&self) -> &'static str {
        "prepare-reskate-cell-collision"
    }
    fn options(&self) -> Options {
        Options::from([
            ("surfaces".to_owned(), self.surfaces.text().to_owned()),
            ("weld".to_owned(), self.weld.text().to_owned()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.cell_layout", "skate.cell_share"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.collision_plan", "skate.cell_counts"]
    }
    fn approximates(&self) -> &'static str {
        "minor collision surfaces of a leaf written as its most common one"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateCollisionPlan>().is_some() {
            return Err(
                "prepare-reskate-cell-collision: the cell is already planned (skate.collision_plan)".into(),
            );
        }
        let (plan, rewritten, count) = cell_plan(w, self.surfaces, self.weld).map_err(|e| e.to_string())?;
        record(w, log, plan, rewritten);
        w.insert_resource(count);
        Ok(())
    }
}

/// `share-reskate-collision`: see the module documentation. Made by [`ShareReskateCollision::new`]; from options by the
/// same call.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareReskateCollision {
    encoding: EncodingOptions,
}

impl ShareReskateCollision {
    /// For the map written as `encoding` says (its grouping); an error for an encoding whose grouping is not one
    /// (non-finite or non-positive metres, negative counts).
    pub fn new(encoding: &EncodingOptions) -> Result<ShareReskateCollision, String> {
        super::overlays::check_grouping(encoding)?;
        Ok(ShareReskateCollision {
            encoding: super::overlays::grouping_only(encoding),
        })
    }
}

/// `share-reskate-collision`'s declaration ([`primase_core::pass::PassRegistry::add`]): the grouping is the writer's
/// unless stated.
pub const SHARE_RESKATE_COLLISION: PassSpec = PassSpec {
    name: "share-reskate-collision",
    options: &super::overlays::ENCODING,
    make: |g| Ok(Box::new(ShareReskateCollision::new(&encoding_from(g)?)?)),
};

impl Pass for ShareReskateCollision {
    fn name(&self) -> &'static str {
        "share-reskate-collision"
    }
    fn options(&self) -> Options {
        super::overlays::encoding_options(&self.encoding)
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.cell_share", "skate.cell_layout"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateCellLayout>().is_some() {
            return Err(
                "share-reskate-collision: the collision is already shared (skate.cell_layout)".into(),
            );
        }
        // what the part writes: its output, as conversion keeps it (its roots and what the passes placed for it), each
        // soup keyed as the part owns it (a soup two parts both reach is one)
        let owned = super::Owned::of(w);
        let key = |e: primase_core::Entity| owned.key(e).unwrap_or(u64::from(e.index()));
        let shares = match w.resource::<PartScope>() {
            Some(scope) => {
                let keep = scope.output(w).keep;
                cell_shares(&w.subset(&keep), &self.encoding, &key)
            }
            None => cell_shares(w, &self.encoding, &key),
        };
        let (layout, shares) = shares.map_err(|e| e.to_string())?;
        log.count("collision cells shared", shares.len() as u64);
        log.count(
            "collision triangles shared",
            shares.values().map(|s| s.triangles() as u64).sum(),
        );
        for share in shares.into_values() {
            let e = w.spawn();
            w.insert(e, share);
            log.place(e);
        }
        w.insert_resource(layout);
        Ok(())
    }
}

impl Pass for PrepareReskateCollision {
    fn name(&self) -> &'static str {
        "prepare-reskate-collision"
    }
    fn options(&self) -> Options {
        let mut o = super::overlays::encoding_options(&self.encoding);
        o.insert("surfaces".into(), self.surfaces.text().into());
        o.insert("weld".into(), self.weld.text().into());
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
        if w.resource::<SkateCollisionPlan>().is_some() {
            return Err(
                "prepare-reskate-collision: the collision is already planned (skate.collision_plan)".into(),
            );
        }
        // a part of a map in several holds none of its boundary cells whole: its cells are shared
        // (share-reskate-collision) and each planned on its own (prepare-reskate-cell-collision)
        if w.resource::<PartScope>().is_some_and(|s| !s.whole) {
            return Err(
                "prepare-reskate-collision: a part of a map in several plans no collision; its cells are shared \
                 (share-reskate-collision in the gather pipeline) and each planned on its own \
                 (prepare-reskate-cell-collision in the group pipeline)"
                    .into(),
            );
        }
        let (plan, rewritten) =
            collision_plan(w, &self.encoding, self.surfaces, self.weld).map_err(|e| e.to_string())?;
        record(w, log, plan, rewritten);
        Ok(())
    }
}
