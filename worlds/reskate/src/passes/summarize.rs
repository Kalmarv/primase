//! `summarize-reskate-map{tile,min_instances,min_instance_tris,max_render_edge,limit,offset_x,offset_y,cull_ids}`: the
//! small record the map's whole-map decisions are made from, `skate.map_summary`: the cull IDs the map's encoding may
//! take, how many its content takes besides the blend chains (an ID per object per Studio 200 m cell it meets: tiles,
//! instances, hulls, collision, grind prisms, water; and one per light), counted by the writer's own layout of the
//! prepared world for this encoding, nothing encoded, and the tiles with a chain form.
//! `plan-reskate-blend-budget` decides the chain forms from it before the map is written.
//!
//! It runs after the passes that change what the writer lays out (the overlay stack, the blend chains, the collision
//! plan). A whole map counts its collision cells ([`CellsCounted::Included`]). A part of a map in several counts its
//! own content with the collision cells apart ([`CellsCounted::Apart`]): a cell is laid out from every part's
//! triangles, and counted by its group (`skate.cell_counts`). It runs in the conversion's gather pipeline
//! ([`primase_core::convert::convert_parts_gathered`]), handed the decisions that change the part's layout
//! (instancing, the region, the floor); the parts' summaries merge into the map's, which the finalize stage's
//! `finalize-reskate-blend-budget` decides from.

use primase_core::World;
use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};

use crate::assembly::{EncodingOptions, map_summary};
use crate::components::{CellsCounted, SkateMapSummary};

/// See the module documentation. Made by [`SummarizeReskateMap::for_encoding`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct SummarizeReskateMap {
    encoding: EncodingOptions,
}

impl SummarizeReskateMap {
    /// For the map written as `encoding` says (its grouping and its cull IDs); an error for an encoding whose grouping
    /// is not one (non-finite or non-positive metres, negative counts).
    pub fn for_encoding(encoding: &EncodingOptions) -> Result<SummarizeReskateMap, String> {
        super::overlays::check_grouping(encoding)?;
        Ok(SummarizeReskateMap {
            encoding: EncodingOptions {
                cull_ids: encoding.cull_ids,
                ..super::overlays::grouping_only(encoding)
            },
        })
    }

    /// From its options: the writer's grouping (`tile`, `min_instances`, `min_instance_tris`, `max_render_edge`,
    /// `limit`, `offset_x`, `offset_y`) and `cull_ids`, each the encoding's default when not given.
    pub fn from_options(o: &Options) -> Result<SummarizeReskateMap, String> {
        let mut grouping = o.clone();
        let cull_ids = match grouping.remove("cull_ids") {
            Some(v) => v
                .parse::<usize>()
                .map_err(|_| format!("cull_ids={v} is not a count"))?,
            None => EncodingOptions::default().cull_ids,
        };
        let e = super::overlays::encoding_of(&grouping)?;
        SummarizeReskateMap::for_encoding(&EncodingOptions { cull_ids, ..e })
    }
}

/// [`SummarizeReskateMap::from_options`] as a pass factory.
pub fn summarize_reskate_map(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SummarizeReskateMap::from_options(o)?))
}

impl Pass for SummarizeReskateMap {
    fn name(&self) -> &'static str {
        "summarize-reskate-map"
    }
    fn options(&self) -> Options {
        let mut o = super::overlays::encoding_options(&self.encoding);
        o.insert("cull_ids".into(), self.encoding.cull_ids.to_string());
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.map_summary"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateMapSummary>().is_some() {
            return Err("summarize-reskate-map: the map is already summarized (skate.map_summary)".into());
        }
        // a part counts what it writes: its output, as conversion keeps it (its roots and what the passes placed for
        // it), not the context it loaded
        // shared content keyed as every part keys it (the same entity in two parts is one key)
        let owned = super::Owned::of(w);
        let key = |e| owned.key(e);
        let summary = match w.resource::<PartScope>() {
            Some(scope) if !scope.whole => {
                let keep = scope.output(w).keep;
                map_summary(&w.subset(&keep), &self.encoding, CellsCounted::Apart, &key)
            }
            _ => map_summary(w, &self.encoding, CellsCounted::Included, &key),
        }
        .map_err(|e| e.to_string())?;
        let cells = summary.cells;
        log.count(
            match cells {
                CellsCounted::Included => "cull IDs the map takes besides its blend chains",
                CellsCounted::Apart => "cull IDs the part takes besides its blend chains and collision cells",
            },
            summary.other_cull_ids() as u64,
        );
        log.count("tiles with a chain form", summary.chains.len() as u64);
        w.insert_resource(summary);
        Ok(())
    }
}
