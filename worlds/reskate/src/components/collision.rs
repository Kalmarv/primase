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

/// `skate.collision_plan`: how the writer writes the collision of the world it was made for
/// (`prepare-reskate-collision`'s): the whole map's, or one collision cell's (a group of a map in parts). The writer
/// lays the collision out into leaves (cells cut until sparse enough), each written as one piece per surface, and
/// replays the plan's surfaces and weld. Its check against the plan is an aggregate one: the surface table, the leaves
/// by name, each leaf's triangles per surface and the triangles its weld collapses must match; a change to the geometry
/// that keeps all of these (a triangle moved within its leaf) is not detected. Without a plan the writer writes the
/// collision exactly: no surface rewritten, only bit-identical corners merged. Not part of scene-interchange v1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkateCollisionPlan {
    pub surfaces: CollisionSurfaces,
    pub weld: CollisionWeld,
    /// The surface table's names, in order: what the leaves' surface indices index.
    pub table: Vec<String>,
    /// Every leaf of the world it was made for, by name.
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

/// A 256 m collision cell as the writer lays cells out: (water, column, row).
pub type CellKey = (bool, i32, i32);

/// One world soup's triangles filed in a cell: loose (9 writer-frame coordinates each), their surfaces and, a ground
/// soup's, which are natural rock; `order` is the soup's place in the scene's order (raw soups after raised ones, each
/// by its entity's planning id), which the cell's triangles are laid out in.
#[derive(Clone, Debug, PartialEq)]
pub struct CellSoup {
    pub order: (bool, u32),
    pub ground: bool,
    pub v: Vec<f32>,
    pub s: Vec<u8>,
    pub rock: Vec<bool>,
}

/// The placed props' collision one render tile files in a cell: loose triangles (9 writer-frame coordinates each),
/// their raw surface bytes (bit 7: rock) and, when any of them is, which are ground. A cell's props are laid out by
/// tile (row, then column).
#[derive(Clone, Debug, PartialEq)]
pub struct CellProps {
    pub tile: (i32, i32),
    pub pos: Vec<f32>,
    pub surf: Vec<u8>,
    pub ground: Option<Vec<bool>>,
}

/// `skate.cell_share`: what one part of a map in parts lays out in one collision cell, as the writer lays it out
/// (`share-reskate-collision`, in the gather traversal): its world soups' triangles and its tiles' props there. A
/// cell's shares from every part, gathered (the cell spool) and laid out together, are the cell the whole map lays out.
/// Not part of scene-interchange v1.
#[derive(Clone, Debug, PartialEq)]
pub struct SkateCellShare {
    pub cell: CellKey,
    pub soups: Vec<CellSoup>,
    pub props: Vec<CellProps>,
}

impl SkateCellShare {
    /// The triangles it holds.
    pub fn triangles(&self) -> usize {
        self.soups.iter().map(|g| g.s.len()).sum::<usize>()
            + self.props.iter().map(|p| p.surf.len()).sum::<usize>()
    }

    /// The bytes of its payload: coordinates, surfaces and flags.
    pub fn payload_bytes(&self) -> usize {
        self.soups
            .iter()
            .map(|g| 4 * g.v.len() + g.s.len() + g.rock.len())
            .sum::<usize>()
            + self
                .props
                .iter()
                .map(|p| 4 * p.pos.len() + p.surf.len() + p.ground.as_ref().map_or(0, Vec::len))
                .sum::<usize>()
    }
}

impl Component for SkateCellShare {
    const NAME: &'static str = "skate.cell_share";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "cell": [self.cell.0, self.cell.1, self.cell.2],
            "soups": self.soups.len(),
            "props": self.props.len(),
            "triangles": self.triangles(),
        })))
    }
}

/// `skate.cell_layout`: what a collision cell is laid out with, the same in every part of a map: the map's name, the
/// collision surface table (name and packed material), the map's limit and the writer's offset. Put beside the shares
/// (`share-reskate-collision`) and carried into every cell. Not part of scene-interchange v1.
#[derive(Clone, Debug, PartialEq)]
pub struct SkateCellLayout {
    /// The map's name: its folder.
    pub name: String,
    pub surfaces: Vec<(String, i32)>,
    pub limit: f32,
    pub offset: [f32; 3],
}

impl Resource for SkateCellLayout {
    const NAME: &'static str = "skate.cell_layout";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "name": self.name,
            "surfaces": self.surfaces.len(),
            "limit": self.limit,
            "offset": self.offset,
        })))
    }
}

/// What a collision cell is written as: its pieces (map objects) and the cull IDs they take (one per object per
/// Studio cell it meets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellCount {
    pub pieces: usize,
    pub cull_ids: usize,
}

/// `skate.cell_counts`: each collision cell of a map in parts as planned (`prepare-reskate-collision` on the cell), by
/// cell. Merged over the cells (each cell is planned once; a cell counted twice is refused): what the map's collision
/// cells take of the cull IDs. Not part of scene-interchange v1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkateCellCounts {
    pub cells: BTreeMap<CellKey, CellCount>,
}

impl SkateCellCounts {
    /// The cull IDs every cell counted takes.
    pub fn cull_ids(&self) -> usize {
        self.cells.values().map(|c| c.cull_ids).sum()
    }
}

impl Resource for SkateCellCounts {
    const NAME: &'static str = "skate.cell_counts";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "cells": self.cells.len(),
            "pieces": self.cells.values().map(|c| c.pieces).sum::<usize>(),
            "cull_ids": self.cull_ids(),
        })))
    }
}

impl primase_core::summary::Summary for SkateCellCounts {
    fn merge(&mut self, other: Self) -> Result<(), String> {
        for (k, c) in other.cells {
            if self.cells.insert(k, c).is_some() {
                return Err(format!("collision cell {k:?} counted by two groups"));
            }
        }
        Ok(())
    }
    fn retained_bytes(&self) -> usize {
        self.cells.len() * 32
    }
}
