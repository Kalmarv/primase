//! `summarize-reskate-layout{tile=T}`: what this part of a map written in parts lays out (`skate.layout_summary`), for
//! the map sink. Merged over the parts, it tells the sink which part holds each render tile's placements (a tile drawn
//! by a part that does not hold all of them is refused: partition aligned to the tile, `partition-grid{tile=T}`),
//! which parts' collision may fall in each 256 m collision cell (a cell is laid out once every part announcing it has
//! been written, its triangles from all of them), and the map's hull soups (named by rank).
//!
//! It files as the writer does, by the same functions: a placement in the tile it states or stands in
//! ([`primase_core::Tile::of`]), its collision in the cell of its pivot ([`crate::collision::prop_cell`]); a world
//! soup's triangles by their centres ([`crate::collision::triangle_cell`]). What it announces is what the part may
//! contribute before its passes run; a pass that removes content leaves an announced cell without it, which is
//! harmless, and content a pass adds where nothing was announced is refused by the sink.

use std::collections::BTreeSet;
use std::path::Path;

use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Resource, World};

use crate::assembly::writer_placements;
use crate::collision::{prop_cell, triangle_cell};
use crate::components::SkateLayoutSummary;
use crate::model::CollisionMode;
use crate::scene::{Scene, w_at};

/// See the module documentation. Made by [`SummarizeReskateLayout::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeReskateLayout {
    tile: f32,
}

impl SummarizeReskateLayout {
    /// For the writer's tile, `tile` metres: finite and above 0.
    pub fn new(tile: f32) -> Result<SummarizeReskateLayout, String> {
        if !(tile.is_finite() && tile > 0.0) {
            return Err(format!("tile {tile}: a finite number of metres above 0"));
        }
        Ok(SummarizeReskateLayout { tile })
    }

    /// From the pipeline text: `tile` (required: the writer's tile, its encoding's).
    pub fn from_options(o: &Options) -> Result<SummarizeReskateLayout, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "tile") {
            return Err(format!("unknown option {k}"));
        }
        let tile = o
            .get("tile")
            .ok_or("tile=T (the writer's tile, metres) is required")?
            .parse::<f32>()
            .map_err(|_| "tile=T: not a number".to_owned())?;
        SummarizeReskateLayout::new(tile)
    }

    pub fn tile(&self) -> f32 {
        self.tile
    }
}

/// [`SummarizeReskateLayout`] as a pass factory.
pub fn summarize_reskate_layout(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SummarizeReskateLayout::from_options(o)?))
}

impl Pass for SummarizeReskateLayout {
    fn name(&self) -> &'static str {
        "summarize-reskate-layout"
    }
    fn options(&self) -> Options {
        Options::from([("tile".to_owned(), self.tile.to_string())])
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &[SkateLayoutSummary::NAME]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
        let s = Scene::from_world(w, Path::new("")).map_err(|e| e.to_string())?;
        let owned = super::Owned::of(w);
        // the part's name and roots: its smallest root's planning id (the whole map: 0, no roots)
        let (tag, roots) = match w.resource::<PartScope>().filter(|s| !s.whole) {
            Some(sc) => (
                sc.roots.first().map_or(0, |e| u64::from(e.index())),
                sc.roots.iter().map(|e| u64::from(e.index())).collect(),
            ),
            None => (0, BTreeSet::new()),
        };
        let mut sum = SkateLayoutSummary {
            tile: self.tile,
            ..SkateLayoutSummary::default()
        };
        sum.parts.insert(tag, roots);
        // writer_placements maps the scene's placements one to one
        for (sp, p) in s.placements.iter().zip(writer_placements(&s, self.tile)) {
            let e = sp.entity;
            if !owned.contains(e) {
                continue;
            }
            sum.tiles
                .entry((p.tx, p.ty))
                .or_default()
                .entry(tag)
                .or_default()
                .insert(u64::from(e.index()));
            let proto = &s.protos[sp.proto as usize];
            if p.no_collision || proto.collision.is_none() {
                continue;
            }
            let (cx, cy) = prop_cell(p.pos);
            match proto.col_mode {
                CollisionMode::Hull => {}
                CollisionMode::Water => {
                    sum.cells.entry((true, cx, cy)).or_default().insert(tag);
                }
                CollisionMode::TriangleMesh => {
                    sum.cells.entry((false, cx, cy)).or_default().insert(tag);
                }
            }
        }
        for sd in s.soups.iter().filter(|sd| owned.contains(sd.entity)) {
            if sd.mode == CollisionMode::Hull {
                sum.hulls.insert((sd.raw, u64::from(sd.entity.index())));
                continue;
            }
            let water = sd.mode == CollisionMode::Water;
            let mut cells = BTreeSet::new();
            for t in 0..sd.surf.len() {
                let a = w_at(&sd.pos, sd.idx[t * 3] as usize * 3);
                let b = w_at(&sd.pos, sd.idx[t * 3 + 1] as usize * 3);
                let c = w_at(&sd.pos, sd.idx[t * 3 + 2] as usize * 3);
                cells.insert(triangle_cell(a, b, c));
            }
            for (cx, cy) in cells {
                sum.cells.entry((water, cx, cy)).or_default().insert(tag);
            }
        }
        w.insert_resource(sum);
        Ok(())
    }
}
