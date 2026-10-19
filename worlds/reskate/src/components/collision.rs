//! Which collision triangles are natural rock (`skate.rock`): scene-interchange v1's surface bit 7, beside the
//! entity's `core.collision` ([`crate::interchange::soup`] reads and writes it); and how the writer writes the map's
//! collision (`skate.collision_plan`, [`crate::passes::PrepareReskateCollision`]'s).

use std::collections::BTreeMap;

use primase_core::{Collision, Component, Entity, JsonError, Resource, World};
use serde_json::{Value, json};

/// Which triangles of the entity's `core.collision` are natural rock (v1's surface bit 7).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SkateRock(pub Vec<bool>);

impl Component for SkateRock {
    const NAME: &'static str = "skate.rock";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(
            json!({"triangles": self.0.len(), "rock": self.0.iter().filter(|&&r| r).count()}),
        ))
    }

    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        match w.get::<Collision>(e) {
            Some(c) if c.triangles.len() == self.0.len() => Ok(()),
            Some(c) => Err(format!(
                "{} rock flags for {} triangles",
                self.0.len(),
                c.triangles.len()
            )),
            None => Err("rock flags without core.collision".into()),
        }
    }
}

/// What `prepare-reskate-collision{surfaces=}` does with a leaf's minor surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionSurfaces {
    /// A standard surface with fewer than max(8, 1/20 of the leaf's) triangles is written as the leaf's most common
    /// standard surface of its kind (soft or hard).
    Coalesce,
    /// Each triangle keeps its surface.
    Exact,
}

/// What `prepare-reskate-collision{weld=}` merges of a piece's corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionWeld {
    /// Corners within the same millimetre (relative to the leaf's centre) merge; triangles that collapse are left out.
    Millimetre,
    /// Only bit-identical corners merge; every triangle is kept.
    Exact,
}

/// One leaf of the collision layout as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedLeaf {
    /// Its triangles per surface (an index of the plan's `table`): the layout the plan was made for.
    pub counts: BTreeMap<usize, usize>,
    /// The surfaces written as another (from, to): only those that change.
    pub into: BTreeMap<usize, usize>,
    /// Triangles its weld collapses, over all its pieces.
    pub collapsed: usize,
}

/// `skate.collision_plan`: how the writer writes the map's collision (`prepare-reskate-collision`'s), for the layout
/// it was made from: the writer lays the collision out into leaves (cells cut until sparse enough), each written as
/// one piece per surface, and replays the plan's surfaces and weld. Its check against the plan is an aggregate one: the
/// surface table, the leaves by name, each leaf's triangles per surface and the triangles its weld collapses must
/// match; a change to the geometry that keeps all of these (a triangle moved within its leaf) is not detected. Without
/// a plan the writer writes the collision exactly: no surface rewritten, only bit-identical corners
/// merged. Not part of scene-interchange v1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkateCollisionPlan {
    pub surfaces: CollisionSurfaces,
    pub weld: CollisionWeld,
    /// The surface table's names, in order: what the leaves' surface indices index.
    pub table: Vec<String>,
    /// Every leaf, by name.
    pub leaves: BTreeMap<String, PlannedLeaf>,
}

impl Resource for SkateCollisionPlan {
    const NAME: &'static str = "skate.collision_plan";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "surfaces": match self.surfaces {
                CollisionSurfaces::Coalesce => "coalesce",
                CollisionSurfaces::Exact => "exact",
            },
            "weld": match self.weld {
                CollisionWeld::Millimetre => "mm",
                CollisionWeld::Exact => "exact",
            },
            "table": self.table,
            "leaves": self.leaves.len(),
            "rewritten": self.leaves.values().map(|l| l.into.keys().map(|s| l.counts[s]).sum::<usize>()).sum::<usize>(),
            "collapsed": self.leaves.values().map(|l| l.collapsed).sum::<usize>(),
        })))
    }
}
