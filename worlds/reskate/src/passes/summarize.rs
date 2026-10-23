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
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};

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

    /// From its options: the writer's grouping ([`super::overlays::encoding_from`]) and `cull_ids`.
    fn from_given(g: &Given) -> Result<SummarizeReskateMap, String> {
        let cull_ids = match g.optional("cull_ids") {
            Some(v) => v
                .parse::<usize>()
                .map_err(|_| format!("cull_ids={v} is not a count"))?,
            None => EncodingOptions::default().cull_ids,
        };
        let e = super::overlays::encoding_from(g)?;
        SummarizeReskateMap::for_encoding(&EncodingOptions { cull_ids, ..e })
    }
}

/// `cull_ids`: the cull IDs the map may take.
const CULL_IDS: OptionSpec = OptionSpec::optional(
    "cull_ids",
    "209715",
    "a count: the cull IDs the map may take",
    "209715, the writer's: 0.8 of ReSkate 2.1.0-pre's pool of 262144",
);

const SUMMARY_OPTIONS: [OptionSpec; 8] = super::overlays::with_encoding([CULL_IDS]);

/// `summarize-reskate-map`'s declaration ([`primase_core::pass::PassRegistry::add`]): the encoding is the writer's
/// unless stated.
pub const SUMMARIZE_RESKATE_MAP: PassSpec = PassSpec {
    name: "summarize-reskate-map",
    options: &SUMMARY_OPTIONS,
    make: |g| Ok(Box::new(SummarizeReskateMap::from_given(g)?)),
};

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

#[cfg(test)]
mod tests {
    /// What no `cull_ids` means is the writer's default, the value [`super::EncodingOptions::default`] holds.
    #[test]
    fn no_cull_ids_states_the_writers_default() {
        let d = super::EncodingOptions::default().cull_ids;
        assert_eq!(super::CULL_IDS.example(), d.to_string());
        assert!(
            super::CULL_IDS
                .absent()
                .unwrap()
                .starts_with(&format!("{d}, the writer's")),
            "{:?}",
            super::CULL_IDS.absent()
        );
    }
}
