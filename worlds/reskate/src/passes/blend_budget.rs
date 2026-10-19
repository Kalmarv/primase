//! `plan-reskate-blend-budget{objects=N|pool}`: how many blend-chain objects the map takes, planned on the blend
//! chains' record (`prepare-reskate-blend-order`'s), the last pass before the writer.
//!
//! Each chain object is a MeshSet of its own and takes a cull ID from ReSkate 2.1.0-pre's pool. The pass ranks the
//! tiles with a chain form in key order (row, then column), decides which take it ([`crate::blend_order::take_chains`]:
//! each in rank order if it still fits), reports those left plain (drawn in an unstable order), and records
//! `skate.blend_budget`. With a fixed count (`objects=N`; `objects=0` writes every tile plain) the count is the room.
//! With the pool (`objects=pool`), the room is what the map's encoding may take of the cull IDs less what its other
//! content takes, from `skate.map_summary` (`summarize-reskate-map`, run before this pass): one ID is held back for
//! every tile still to be decided, so the plain forms always fit. The decision is recorded with the basis it was made
//! on; the writer replays it and refuses a map whose own count differs. A map with blend chains and no budget is
//! refused by the writer. A map without chain forms gets an empty plan.
//!
//! A part of a map converted in several holds only its own tiles, and in pool mode not the whole map's other objects:
//! it records the mode only, and the writer decides by this pass's own rule ([`plan`]) once it has laid the whole map
//! out, on the whole map's chains and its own count, reporting the decision with its basis.

use std::collections::BTreeSet;

use primase_core::World;
use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};

use crate::blend_order::take_chains;
use crate::components::{
    BlendBudgetMode, BudgetDecision, PoolBasis, SkateBlendBudget, SkateBlendChains, SkateMapSummary,
};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanReskateBlendBudget {
    mode: BlendBudgetMode,
}

/// The plan for the blend chains `chains` (`None`: none recorded) by `mode`: the tiles with a chain form ranked (row,
/// then column) and the ones that take it; in pool mode from `summary`, which must have been counted for the chains'
/// writer settings.
pub(crate) fn plan(
    chains: Option<&SkateBlendChains>,
    mode: BlendBudgetMode,
    summary: Option<&SkateMapSummary>,
) -> Result<SkateBlendBudget, String> {
    let mut tiles: Vec<(i32, i32, &str, usize)> = chains
        .into_iter()
        .flat_map(|c| c.tiles.iter())
        .filter(|(_, t)| t.steps.is_some())
        .map(|(name, t)| (t.row, t.column, name.as_str(), t.groups.len()))
        .collect();
    tiles.sort();
    let ranked: Vec<String> = tiles.iter().map(|t| t.2.to_owned()).collect();
    let objects: Vec<usize> = tiles.iter().map(|t| t.3).collect();
    let (room, hold_back, pool) = match mode {
        BlendBudgetMode::Objects(n) => (n, false, None),
        BlendBudgetMode::Pool => {
            let s = summary.ok_or(
                "objects=pool: the cull-ID pool's room is decided from the map's summary (skate.map_summary): run \
                 summarize-reskate-map first",
            )?;
            if let Some(c) = chains
                && c.grouping != Some(s.grouping)
            {
                return Err(format!(
                    "the map's summary was counted for the writer settings {:?}, but its blend chains were chosen for \
                     {:?}: give summarize-reskate-map the writer's",
                    s.grouping, c.grouping
                ));
            }
            let basis = PoolBasis {
                cull_ids: s.cull_ids,
                other_cull_ids: s.other_cull_ids,
            };
            (s.cull_ids.saturating_sub(s.other_cull_ids), true, Some(basis))
        }
    };
    let chosen = take_chains(&objects, room, hold_back)
        .into_iter()
        .zip(&ranked)
        .filter(|(take, _)| *take)
        .map(|(_, t)| t.clone())
        .collect::<BTreeSet<String>>();
    Ok(SkateBlendBudget {
        mode,
        decision: Some(BudgetDecision { ranked, chosen, pool }),
    })
}

impl PlanReskateBlendBudget {
    /// At most `objects` chain objects.
    pub fn objects(objects: usize) -> PlanReskateBlendBudget {
        PlanReskateBlendBudget {
            mode: BlendBudgetMode::Objects(objects),
        }
    }

    /// As many as the cull-ID pool has room for after the map's other objects.
    pub fn pool() -> PlanReskateBlendBudget {
        PlanReskateBlendBudget {
            mode: BlendBudgetMode::Pool,
        }
    }

    pub fn mode(&self) -> BlendBudgetMode {
        self.mode
    }

    /// From its options: `objects`, required: a count, or `pool`.
    pub fn from_options(o: &Options) -> Result<PlanReskateBlendBudget, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "objects") {
            return Err(format!("unknown option {k}"));
        }
        match o.get("objects").map(String::as_str) {
            None => {
                Err("objects is required: a count of chain objects, or pool (the cull-ID pool's room)".into())
            }
            Some("pool") => Ok(PlanReskateBlendBudget::pool()),
            Some(v) => v
                .parse::<usize>()
                .map(PlanReskateBlendBudget::objects)
                .map_err(|_| format!("objects={v}: a count, or pool")),
        }
    }
}

/// [`PlanReskateBlendBudget::from_options`] as a pass factory.
pub fn plan_reskate_blend_budget(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PlanReskateBlendBudget::from_options(o)?))
}

impl Pass for PlanReskateBlendBudget {
    fn name(&self) -> &'static str {
        "plan-reskate-blend-budget"
    }
    fn options(&self) -> Options {
        let v = match self.mode {
            BlendBudgetMode::Objects(n) => n.to_string(),
            BlendBudgetMode::Pool => "pool".to_owned(),
        };
        Options::from([("objects".to_owned(), v)])
    }
    // the chains are optional: a map without them gets an empty plan
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.blend_budget"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateBlendBudget>().is_some() {
            return Err(
                "plan-reskate-blend-budget: the budget is already planned (skate.blend_budget)".into(),
            );
        }
        // a part of a map in several: the mode only, decided by the writer by this rule over the whole map
        if w.resource::<PartScope>().is_some_and(|s| !s.whole) {
            w.insert_resource(SkateBlendBudget {
                mode: self.mode,
                decision: None,
            });
            log.count(
                "blend budgets left to the writer, over the whole map (a part of a map in several)",
                1,
            );
            return Ok(());
        }
        let p = plan(
            w.resource::<SkateBlendChains>(),
            self.mode,
            w.resource::<SkateMapSummary>(),
        )?;
        let d = p.decision.as_ref().expect("plan decides");
        log.count("tiles ranked for the chain form", d.ranked.len() as u64);
        log.count("tiles given the chain form", d.chosen.len() as u64);
        let plain = (d.ranked.len() - d.chosen.len()) as u64;
        if plain > 0 {
            log.approximate(
                "tiles whose blend chains are written plain, past the chain-object budget (drawn in an unstable order)",
                plain,
            );
        }
        w.insert_resource(p);
        Ok(())
    }
}
