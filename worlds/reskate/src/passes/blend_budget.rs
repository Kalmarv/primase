//! `plan-reskate-blend-budget{objects=N|pool}`: how many blend-chain objects the map takes, planned on the blend
//! chains' record (`prepare-reskate-blend-order`'s), the last pass before the writer.
//!
//! Each chain object is a MeshSet of its own and takes a cull ID from ReSkate 2.1.0-pre's pool. The pass ranks the
//! tiles with a chain form in key order (row, then column) and records `skate.blend_budget`. With a fixed count
//! (`objects=N`) it decides here which tiles take the chain form ([`crate::blend_order::take_chains`]: each in rank
//! order if it still fits) and reports those left plain. With the pool (`objects=pool`), what is left of the level's
//! share of the pool after the map's other objects is the format's capacity, known only when the map is written: the
//! writer gives the ranked tiles the chain form in that order by the same rule while there is room, and reports each
//! tile it writes plain. Without this pass the writer writes every tile's chains plain (reported). A map without chain
//! forms gets an empty plan.

use std::collections::BTreeSet;

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};

use crate::blend_order::take_chains;
use crate::components::{BlendBudgetMode, SkateBlendBudget, SkateBlendChains};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanReskateBlendBudget {
    mode: BlendBudgetMode,
}

/// The plan for the blend chains `chains` (`None`: none recorded) by `mode`: the tiles with a chain form ranked
/// (row, then column), and for a fixed count the ones that take it.
pub(crate) fn plan(chains: Option<&SkateBlendChains>, mode: BlendBudgetMode) -> SkateBlendBudget {
    let mut tiles: Vec<(i32, i32, &str, usize)> = chains
        .into_iter()
        .flat_map(|c| c.tiles.iter())
        .filter(|(_, t)| t.steps.is_some())
        .map(|(name, t)| (t.row, t.column, name.as_str(), t.groups.len()))
        .collect();
    tiles.sort();
    let ranked: Vec<String> = tiles.iter().map(|t| t.2.to_owned()).collect();
    let chosen = match mode {
        BlendBudgetMode::Objects(n) => {
            let objects: Vec<usize> = tiles.iter().map(|t| t.3).collect();
            Some(
                take_chains(&objects, n, false)
                    .into_iter()
                    .zip(&ranked)
                    .filter(|(take, _)| *take)
                    .map(|(_, t)| t.clone())
                    .collect::<BTreeSet<String>>(),
            )
        }
        BlendBudgetMode::Pool => None,
    };
    SkateBlendBudget { mode, ranked, chosen }
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
        super::require_whole_map(w, self.name())?;
        if w.resource::<SkateBlendBudget>().is_some() {
            return Err(
                "plan-reskate-blend-budget: the budget is already planned (skate.blend_budget)".into(),
            );
        }
        let p = plan(w.resource::<SkateBlendChains>(), self.mode);
        log.count("tiles ranked for the chain form", p.ranked.len() as u64);
        if let Some(chosen) = &p.chosen {
            log.count("tiles given the chain form", chosen.len() as u64);
            let plain = (p.ranked.len() - chosen.len()) as u64;
            if plain > 0 {
                log.approximate(
                    "tiles whose blend chains are written plain, past the chain-object budget (drawn in an unstable order)",
                    plain,
                );
            }
        }
        w.insert_resource(p);
        Ok(())
    }
}
