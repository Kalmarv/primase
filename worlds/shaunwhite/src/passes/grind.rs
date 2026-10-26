//! `lower-shaunwhite-grinds`: the game's grind guidance curves (`core.curve` of kind `guidance` or `curved_vert` with
//! `shaunwhite.guidance`) as the shared grind kinds: `curved_vert` (the lips of vert ramps and quarter pipes) becomes
//! `coping`; `guidance` becomes `rail` or `ledge` by its edges' two face normals (CHOICE, from level 01's names): a rail's
//! faces both slope away from its top (fences, stair rails, a clothesline: both normals tilted 25-60 degrees from up),
//! a ledge has a flat top or a vertical side (benches, planters, curbs: one normal up, the other level). An edge is a
//! rail's when neither normal's vertical part is under [`RAIL_MIN_UP`] or over [`RAIL_MAX_UP`]; a curve is a rail when
//! its rail edges are at least half its stored length. Other sub-types keep their kind. The dialect component is taken
//! off every curve.

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Curve, World};

use crate::components::Guidance;

/// A rail edge's faces are tilted at most this far from level: its normals' up part is at least this.
pub const RAIL_MIN_UP: f32 = 0.5;
/// And at least this far from flat: its normals' up part is at most this.
pub const RAIL_MAX_UP: f32 = 0.97;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowerShaunwhiteGrinds;

pub const LOWER_SHAUNWHITE_GRINDS: PassSpec = PassSpec {
    name: "lower-shaunwhite-grinds",
    options: &[],
    make: |_| Ok(Box::new(LowerShaunwhiteGrinds)),
};

/// Whether an edge with face normals `normals` (core frame: +Y up) is a rail's.
pub fn rail_edge(normals: &[[f32; 3]; 2]) -> bool {
    normals
        .iter()
        .all(|n| (RAIL_MIN_UP..=RAIL_MAX_UP).contains(&n[1]))
}

impl Pass for LowerShaunwhiteGrinds {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-grinds"
    }
    fn requires(&self) -> &[&'static str] {
        &[Guidance::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[Guidance::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Curve::NAME]
    }
    fn approximates(&self) -> &'static str {
        "rail or ledge is chosen from the edges' face normals (a rule from level 01's names), not a field the source states"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let chains: Vec<_> = w.iter::<Guidance>().map(|(e, g)| (e, g.clone())).collect();
        for (e, g) in chains {
            w.remove::<Guidance>(e);
            let Some(curve) = w.get_mut::<Curve>(e) else {
                return Err("a shaunwhite.guidance without its core.curve".into());
            };
            let kind = match curve.kind.as_str() {
                "curved_vert" => "coping",
                "guidance" => {
                    let total: f32 = g.lengths.iter().sum();
                    let rail: f32 = g
                        .normals
                        .iter()
                        .zip(&g.lengths)
                        .filter(|(n, _)| rail_edge(n))
                        .map(|(_, l)| l)
                        .sum();
                    if rail * 2. >= total { "rail" } else { "ledge" }
                }
                other => {
                    log.count(&format!("guidance curves of kind {other} kept"), 1);
                    continue;
                }
            };
            log.count(&format!("guidance curves as {kind}"), 1);
            curve.kind = kind.into();
        }
        Ok(())
    }
}
