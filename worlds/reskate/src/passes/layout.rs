//! `summarize-reskate-layout{tile=T}`: what this part of a map written in parts lays out (`skate.layout_summary`), for
//! the map sink. Merged over the parts, it tells the sink which part holds each render tile's placements (a tile drawn
//! by a part that does not hold all of them is refused: partition aligned to the tile, `partition-grid{tile=T}`) and
//! the map's hull soups (named by rank). It files a placement as the writer does, in the tile it states or stands in
//! ([`primase_core::Tile::of`]). The map's 256 m collision cells are not the layout's: each part shares its collision
//! per cell (`share-reskate-collision`) and each cell is laid out whole from every part's shares.

use std::collections::BTreeSet;
use std::path::Path;

use primase_core::convert::PartScope;
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Resource, World};

use crate::assembly::writer_placements;
use crate::components::SkateLayoutSummary;
use crate::model::CollisionMode;
use crate::scene::Scene;

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

    pub fn tile(&self) -> f32 {
        self.tile
    }
}

/// `summarize-reskate-layout`'s declaration ([`primase_core::pass::PassRegistry::add`]): the writer's tile is the
/// program's to state, its encoding's.
pub const SUMMARIZE_RESKATE_LAYOUT: PassSpec = PassSpec {
    name: "summarize-reskate-layout",
    options: &[OptionSpec::required(
        "tile",
        "200",
        "metres above 0: the writer's tile, its encoding's",
    )],
    make: |g| {
        let tile = g.required("tile");
        let tile = tile
            .parse::<f32>()
            .map_err(|_| format!("tile={tile}: not a number"))?;
        Ok(Box::new(SummarizeReskateLayout::new(tile)?))
    },
};

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
            first_made: w
                .resource::<PartScope>()
                .and_then(|s| s.first_made)
                .unwrap_or(u32::MAX),
            ..SkateLayoutSummary::default()
        };
        sum.parts.insert(tag, roots);
        // writer_placements maps the scene's placements one to one
        for (sp, p) in s
            .placements
            .iter()
            .zip(writer_placements(&s, self.tile).map_err(|e| e.to_string())?)
        {
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
        }
        for sd in s
            .soups
            .iter()
            .filter(|sd| owned.contains(sd.entity) && sd.mode == CollisionMode::Hull)
        {
            sum.hulls.insert((sd.raw, u64::from(sd.entity.index())));
        }
        w.insert_resource(sum);
        Ok(())
    }
}
