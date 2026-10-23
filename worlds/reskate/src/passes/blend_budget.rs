//! `plan-reskate-blend-budget{objects=N|pool}` and `finalize-reskate-blend-budget{objects=N|pool}`: how many
//! blend-chain objects the map takes, decided over the whole map before it is written.
//!
//! Each chain object is a MeshSet of its own and takes a cull ID from ReSkate 2.1.0-pre's pool. The passes rank the
//! tiles with a chain form in key order (row, then column), decide which take it ([`crate::blend_order::take_chains`]:
//! each in rank order if it still fits), report those left plain (drawn in an unstable order) as an approximation, and
//! record `skate.blend_budget`. With a fixed count (`objects=N`; `objects=0` writes every tile plain) the count is the
//! room. With the pool (`objects=pool`), the room is what the map's encoding may take of the cull IDs less what its
//! other content takes: one ID is held back for every tile still to be decided, so the plain forms always fit. The
//! decision is recorded with the basis it was made on; the writer replays it, refuses a map whose own count differs,
//! and refuses a map with blend chains and no decision. A map without chain forms gets an empty plan.
//!
//! A map written whole: `plan-reskate-blend-budget` ([`PlanReskateBlendBudget`]), the last pass before the writer, on
//! the blend chains' record (`prepare-reskate-blend-order`'s) and in pool mode the map's summary
//! (`summarize-reskate-map`, run before it).
//!
//! A map written in parts: `finalize-reskate-blend-budget` ([`FinalizeReskateBlendBudget`]), a pass of the
//! conversion's finalize stage ([`primase_core::convert::convert_parts_gathered`]), by the same rule on the merged
//! summaries: the parts' map summary, counted with their collision cells apart (`summarize-reskate-map` in the gather
//! pipeline: their content's cull IDs and their tiles with a chain form), and the collision cells' own counts
//! (`skate.cell_counts`, made by the cells' groups). The map's other cull IDs are the two summed. The decision is
//! handed to every part as it is written. A part itself is refused by both: what a part holds is not the map's.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::World;
use primase_core::convert::PartScope;
use primase_core::pass::{Options, Pass, PassLog};

use crate::blend_order::take_chains;
use crate::components::{
    BlendBudgetMode, BudgetDecision, CellsCounted, ChainCandidate, PoolBasis, SkateBlendBudget,
    SkateBlendChains, SkateCellCounts, SkateMapSummary,
};

/// See the module documentation: the budget of a map written whole.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanReskateBlendBudget {
    mode: BlendBudgetMode,
}

/// See the module documentation: [`PlanReskateBlendBudget`]'s rule over a map in parts, in the finalize stage.
#[derive(Clone, Debug, PartialEq)]
pub struct FinalizeReskateBlendBudget {
    mode: BlendBudgetMode,
}

/// The tiles of `chains` with a chain form, by name.
pub(crate) fn candidates(chains: &SkateBlendChains) -> BTreeMap<String, ChainCandidate> {
    chains
        .tiles
        .iter()
        .filter(|(_, t)| t.steps.is_some())
        .map(|(name, t)| {
            let c = ChainCandidate {
                row: t.row,
                column: t.column,
                objects: t.groups.len(),
            };
            (name.clone(), c)
        })
        .collect()
}

/// The decision for the tiles with a chain form `tiles` by `mode`: ranked (row, then column) and the ones that take
/// it; in pool mode on `pool`, the map's cull IDs and what its other content takes.
fn decide(
    tiles: &BTreeMap<String, ChainCandidate>,
    mode: BlendBudgetMode,
    pool: Option<PoolBasis>,
) -> Result<SkateBlendBudget, String> {
    let mut ranked: Vec<(i32, i32, &str, usize)> = tiles
        .iter()
        .map(|(name, c)| (c.row, c.column, name.as_str(), c.objects))
        .collect();
    ranked.sort();
    let objects: Vec<usize> = ranked.iter().map(|t| t.3).collect();
    let (room, hold_back, pool) = match mode {
        BlendBudgetMode::Objects(n) => (n, false, None),
        BlendBudgetMode::Pool => {
            let b = pool.ok_or(
                "objects=pool: the cull-ID pool's room is decided from the map's summary (skate.map_summary): run \
                 summarize-reskate-map first",
            )?;
            (b.cull_ids.saturating_sub(b.other_cull_ids), true, Some(b))
        }
    };
    let chosen = take_chains(&objects, room, hold_back)
        .into_iter()
        .zip(&ranked)
        .filter(|(take, _)| *take)
        .map(|(_, t)| t.2.to_owned())
        .collect::<BTreeSet<String>>();
    Ok(SkateBlendBudget {
        mode,
        decision: Some(BudgetDecision {
            ranked: ranked.iter().map(|t| t.2.to_owned()).collect(),
            chosen,
            pool,
        }),
    })
}

/// The plan of a map written whole, for its blend chains `chains` (`None`: none recorded) by `mode`; in pool mode from
/// `summary`, which must be the whole map's (its collision cells counted), counted for the chains' writer settings.
pub(crate) fn plan(
    chains: Option<&SkateBlendChains>,
    mode: BlendBudgetMode,
    summary: Option<&SkateMapSummary>,
) -> Result<SkateBlendBudget, String> {
    let pool = match (mode, summary) {
        (BlendBudgetMode::Pool, Some(s)) => {
            if s.cells != CellsCounted::Included {
                return Err(
                    "the map's summary counts a part's content with its collision cells apart, not the whole map's: \
                     a map in parts is budgeted by finalize-reskate-blend-budget"
                        .into(),
                );
            }
            if let Some(c) = chains
                && c.grouping != Some(s.grouping)
            {
                return Err(format!(
                    "the map's summary was counted for the writer settings {:?}, but its blend chains were chosen for \
                     {:?}: give summarize-reskate-map the writer's",
                    s.grouping, c.grouping
                ));
            }
            Some(PoolBasis {
                cull_ids: s.cull_ids,
                other_cull_ids: s.other_cull_ids(),
            })
        }
        _ => None,
    };
    let tiles = chains.map(candidates).unwrap_or_default();
    decide(&tiles, mode, pool)
}

/// The plan of a map written in parts by `mode`, from its merged summary `summary` (its parts' content with their
/// collision cells apart, and its tiles with a chain form) and its collision cells' counts `cells`: the map's other
/// cull IDs are the parts' and the cells' summed.
fn plan_parts(
    summary: &SkateMapSummary,
    cells: &SkateCellCounts,
    mode: BlendBudgetMode,
) -> Result<SkateBlendBudget, String> {
    if summary.cells != CellsCounted::Apart {
        return Err(
            "the map's summary counts its collision cells: it is a whole map's, budgeted by \
             plan-reskate-blend-budget, not the merged summary of a map's parts"
                .into(),
        );
    }
    let pool = PoolBasis {
        cull_ids: summary.cull_ids,
        other_cull_ids: summary.other_cull_ids() + cells.cull_ids(),
    };
    decide(&summary.chains, mode, Some(pool))
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
        Ok(PlanReskateBlendBudget { mode: mode_of(o)? })
    }
}

impl FinalizeReskateBlendBudget {
    /// At most `objects` chain objects.
    pub fn objects(objects: usize) -> FinalizeReskateBlendBudget {
        FinalizeReskateBlendBudget {
            mode: BlendBudgetMode::Objects(objects),
        }
    }

    /// As many as the cull-ID pool has room for after the map's other objects.
    pub fn pool() -> FinalizeReskateBlendBudget {
        FinalizeReskateBlendBudget {
            mode: BlendBudgetMode::Pool,
        }
    }

    pub fn mode(&self) -> BlendBudgetMode {
        self.mode
    }

    /// From its options, as [`PlanReskateBlendBudget::from_options`].
    pub fn from_options(o: &Options) -> Result<FinalizeReskateBlendBudget, String> {
        Ok(FinalizeReskateBlendBudget { mode: mode_of(o)? })
    }
}

/// The mode the options `o` state: `objects`, required: a count, or `pool`.
fn mode_of(o: &Options) -> Result<BlendBudgetMode, String> {
    if let Some(k) = o.keys().find(|k| k.as_str() != "objects") {
        return Err(format!("unknown option {k}"));
    }
    match o.get("objects").map(String::as_str) {
        None => {
            Err("objects is required: a count of chain objects, or pool (the cull-ID pool's room)".into())
        }
        Some("pool") => Ok(BlendBudgetMode::Pool),
        Some(v) => v
            .parse::<usize>()
            .map(BlendBudgetMode::Objects)
            .map_err(|_| format!("objects={v}: a count, or pool")),
    }
}

/// The options stating `mode`.
fn options_of(mode: BlendBudgetMode) -> Options {
    let v = match mode {
        BlendBudgetMode::Objects(n) => n.to_string(),
        BlendBudgetMode::Pool => "pool".to_owned(),
    };
    Options::from([("objects".to_owned(), v)])
}

/// The refusals both passes share (`pass` names the one refusing): a budget already planned, a part of a map in
/// several.
fn check_unplanned(w: &World, pass: &str) -> Result<(), String> {
    if w.resource::<SkateBlendBudget>().is_some() {
        return Err(format!(
            "{pass}: the budget is already planned (skate.blend_budget)"
        ));
    }
    if w.resource::<PartScope>().is_some_and(|s| !s.whole) {
        return Err(format!(
            "{pass}: a part of a map in several holds only its own tiles and objects: the budget is decided once over \
             the whole map, by finalize-reskate-blend-budget in the conversion's finalize stage"
        ));
    }
    Ok(())
}

/// The budget `p` recorded in `w` and logged as both passes log it: the tiles ranked and given the chain form, and
/// those written plain as an approximation.
fn record(w: &mut World, log: &mut PassLog, p: SkateBlendBudget) {
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
}

/// [`PlanReskateBlendBudget::from_options`] as a pass factory.
pub fn plan_reskate_blend_budget(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PlanReskateBlendBudget::from_options(o)?))
}

/// [`FinalizeReskateBlendBudget::from_options`] as a pass factory.
pub fn finalize_reskate_blend_budget(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(FinalizeReskateBlendBudget::from_options(o)?))
}

impl Pass for PlanReskateBlendBudget {
    fn name(&self) -> &'static str {
        "plan-reskate-blend-budget"
    }
    fn options(&self) -> Options {
        options_of(self.mode)
    }
    // the chains are optional: a map without them gets an empty plan
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.blend_budget"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        check_unplanned(w, self.name())?;
        let p = plan(
            w.resource::<SkateBlendChains>(),
            self.mode,
            w.resource::<SkateMapSummary>(),
        )?;
        record(w, log, p);
        Ok(())
    }
}

impl Pass for FinalizeReskateBlendBudget {
    fn name(&self) -> &'static str {
        "finalize-reskate-blend-budget"
    }
    fn options(&self) -> Options {
        options_of(self.mode)
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.map_summary", "skate.cell_counts"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.blend_budget"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        check_unplanned(w, self.name())?;
        let (Some(summary), Some(cells)) = (w.resource::<SkateMapSummary>(), w.resource::<SkateCellCounts>())
        else {
            return Err(format!(
                "{}: the map's merged summary (skate.map_summary) and its cells' counts (skate.cell_counts) are \
                 required",
                self.name()
            ));
        };
        let p = plan_parts(summary, cells, self.mode)?;
        record(w, log, p);
        Ok(())
    }
}
