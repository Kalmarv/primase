//! `select-anvil-listed{coarse=keep|drop|overlap[, rect=x0,y0,x1,y1]}`: which of what a section read found a program
//! takes, by the grid level that lists it. The reader keeps every object its cells list ([`AnvilListed`]); an object
//! larger than a cell is listed only at a coarser level, whose cell is far larger than the section, so reading those
//! cells whole brings in everything else they list. This pass is where a program chooses what to do with them.
//!
//! An owner is *coarse* when `reached_from` (the finest level of any cell that reaches it, directly or as a member of a
//! listed group) is above 0. `coarse=keep` keeps every coarse owner's placements and `drop` drops them all (the
//! section's level-0 cells alone). `overlap` keeps what reaches `rect` (x0, y0, x1, y1 in the grid's units, the
//! game's ground axes, metres) and drops the rest:
//!
//! - an owner with one visual placement is kept when that placement's box ([`AnvilBounds`], its owner's
//!   `BoundingVolume`) overlaps `rect` in a positive area; an owner without bounds on its visual is kept;
//! - an owner with several placements (instances of one group, each carrying the whole group's box) is decided placement
//!   by placement, by the placement's own position inside `rect`, its collision included;
//! - an owner's collision follows its visuals (kept when one of them is, or when it has none), unless it has a box of its
//!   own (a collision-only owner): then by that box;
//! - lakes and decal sets made for the owner follow the owner's placements the same way.
//!
//! Approximation, stated: the boxes are the owners' stored bounds, not their meshes. Every decision is counted.
//! The marks ([`AnvilListed`]) are removed.

use std::collections::BTreeMap;

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Instance, Transform, World};

use crate::components::{AnvilBounds, AnvilCollisionFilter, AnvilListed};

/// What to do with the placements of owners that only a coarser level lists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coarse {
    /// Keep every one.
    Keep,
    /// Drop every one.
    Drop,
    /// Keep what reaches the rectangle `[x0, y0, x1, y1]` (the game's ground axes, metres).
    Overlap([f64; 4]),
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectAnvilListed {
    pub coarse: Coarse,
}

/// Option `coarse`: [`Coarse`]'s kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoarseKind {
    Keep,
    Drop,
    Overlap,
}

impl Choice for CoarseKind {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("keep", CoarseKind::Keep),
        ("drop", CoarseKind::Drop),
        ("overlap", CoarseKind::Overlap),
    ];
}

impl SelectAnvilListed {
    /// From its declared options: `rect` with `coarse=overlap` only, and required there.
    fn from_given(g: &Given) -> Result<SelectAnvilListed, String> {
        let coarse = match (g.choice::<CoarseKind>("coarse"), g.optional("rect")) {
            (CoarseKind::Keep, None) => Coarse::Keep,
            (CoarseKind::Drop, None) => Coarse::Drop,
            (CoarseKind::Keep | CoarseKind::Drop, Some(_)) => {
                return Err("rect goes with coarse=overlap only".to_owned());
            }
            (CoarseKind::Overlap, None) => return Err("coarse=overlap needs rect".to_owned()),
            (CoarseKind::Overlap, Some(rect)) => {
                let v: Vec<f64> = rect
                    .split(',')
                    .map(|v| {
                        v.trim()
                            .parse::<f64>()
                            .map_err(|_| format!("rect: {v} is not a number"))
                    })
                    .collect::<Result<_, _>>()?;
                let [x0, y0, x1, y1] = v[..] else {
                    return Err("rect takes four numbers, x0,y0,x1,y1".to_owned());
                };
                if !([x0, y0, x1, y1].iter().all(|v| v.is_finite()) && x0 < x1 && y0 < y1) {
                    return Err(format!("rect {x0},{y0},{x1},{y1}: finite, x0 < x1 and y0 < y1"));
                }
                Coarse::Overlap([x0, y0, x1, y1])
            }
        };
        Ok(SelectAnvilListed { coarse })
    }
}

/// `select-anvil-listed`'s declaration ([`primase_core::pass::PassRegistry::add`]): what becomes of what only a coarser
/// level lists is the program's.
pub const SELECT_ANVIL_LISTED: PassSpec = PassSpec {
    name: "select-anvil-listed",
    options: &[
        OptionSpec::choice::<CoarseKind>(
            "coarse",
            "the placements only a coarser level lists kept, dropped, or kept where they reach rect",
        ),
        OptionSpec::optional(
            "rect",
            "-64,-64,64,64",
            "x0,y0,x1,y1 on the game's ground axes (metres), finite, x0 < x1 and y0 < y1 (coarse=overlap)",
            "required with coarse=overlap, not taken with keep or drop",
        ),
    ],
    make: |g| Ok(Box::new(SelectAnvilListed::from_given(g)?)),
};

impl Pass for SelectAnvilListed {
    fn name(&self) -> &'static str {
        "select-anvil-listed"
    }
    fn options(&self) -> Options {
        match self.coarse {
            Coarse::Keep => Options::from([("coarse".to_owned(), CoarseKind::Keep.text().to_owned())]),
            Coarse::Drop => Options::from([("coarse".to_owned(), CoarseKind::Drop.text().to_owned())]),
            Coarse::Overlap([x0, y0, x1, y1]) => Options::from([
                ("coarse".to_owned(), CoarseKind::Overlap.text().to_owned()),
                ("rect".to_owned(), format!("{x0},{y0},{x1},{y1}")),
            ]),
        }
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilListed::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilListed::NAME]
    }
    fn approximates(&self) -> &'static str {
        "coarse=overlap tests the owners' stored bounding boxes (an instanced group's placements by their positions), \
         not their meshes"
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // every marked entity, by its owner
        let mut owners: BTreeMap<u64, Vec<(Entity, AnvilListed)>> = BTreeMap::new();
        for (e, l) in w.iter::<AnvilListed>() {
            owners.entry(l.owner).or_default().push((e, l.clone()));
        }
        let mut tested = 0u64;
        let mut drop: Vec<Entity> = Vec::new();
        for group in owners.values() {
            if group.iter().all(|(_, l)| l.reached_from == 0) {
                log.count(
                    "placements of owners a level-0 cell reaches kept",
                    group.len() as u64,
                );
                continue;
            }
            let go = match self.coarse {
                Coarse::Keep => Vec::new(),
                Coarse::Drop => group.iter().map(|(e, _)| *e).collect(),
                Coarse::Overlap(rect) => {
                    tested += group.len() as u64;
                    overlapping(w, group, rect)
                }
            };
            log.count(
                "placements of owners only a coarser level reaches kept",
                (group.len() - go.len()) as u64,
            );
            log.count(
                "placements of owners only a coarser level reaches dropped",
                go.len() as u64,
            );
            drop.extend(go);
        }
        for e in drop {
            primase_core::ledger::despawn(
                w,
                e,
                log,
                "owner only a coarser grid level lists (select-anvil-listed)",
            );
        }
        let marked: Vec<Entity> = w.iter::<AnvilListed>().map(|(e, _)| e).collect();
        for e in marked {
            w.remove::<AnvilListed>(e);
        }
        if tested > 0 {
            log.approximate(
                "placements of coarse-level owners tested against stored bounding boxes (or, for several placements \
                 of one owner, their positions)",
                tested,
            );
        }
        Ok(())
    }
}

/// The entities of one coarse owner that don't reach `rect`.
fn overlapping(w: &World, group: &[(Entity, AnvilListed)], rect: [f64; 4]) -> Vec<Entity> {
    // the owner's visual placements (placed meshes), its collision, and what else was made for it
    let (mut visuals, mut collisions, mut others) = (Vec::new(), Vec::new(), Vec::new());
    for (e, l) in group {
        if w.has::<AnvilCollisionFilter>(*e) {
            collisions.push((*e, l));
        } else if w.has::<Instance>(*e) {
            visuals.push((*e, l));
        } else {
            others.push((*e, l));
        }
    }
    // a box in the game's ground axes, and a position
    let boxed = |e: Entity, l: &AnvilListed| {
        w.get::<AnvilBounds>(e).map(|b| {
            let (lo, hi) = (l.frame.point_to_source(b.min), l.frame.point_to_source(b.max));
            let [x0, x1] = [lo[0].min(hi[0]), lo[0].max(hi[0])].map(f64::from);
            let [y0, y1] = [lo[1].min(hi[1]), lo[1].max(hi[1])].map(f64::from);
            x0 < rect[2] && x1 > rect[0] && y0 < rect[3] && y1 > rect[1]
        })
    };
    let placed_in = |e: Entity, l: &AnvilListed| {
        w.get::<Transform>(e).map(|t| {
            let p = l.frame.point_to_source(t.cols[3]);
            let (x, y) = (f64::from(p[0]), f64::from(p[1]));
            x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3]
        })
    };
    let mut go = Vec::new();
    let mut kept_any = false;
    if visuals.len() > 1 || collisions.len() > 1 {
        for (e, l) in visuals.iter().chain(&collisions) {
            if placed_in(*e, l) == Some(false) {
                go.push(*e);
            } else {
                kept_any = true;
            }
        }
    } else {
        let mut kept_visual = visuals.is_empty();
        for (e, l) in &visuals {
            if boxed(*e, l) == Some(false) {
                go.push(*e);
            } else {
                kept_visual = true;
            }
        }
        for (e, l) in &collisions {
            let keep = boxed(*e, l).unwrap_or(kept_visual);
            if keep {
                kept_any = true;
            } else {
                go.push(*e);
            }
        }
        kept_any |= kept_visual && !visuals.is_empty();
    }
    // lakes and decal sets made for the owner follow what stays of it (all of them when it placed nothing else)
    if !(kept_any || visuals.is_empty() && collisions.is_empty()) {
        go.extend(others.iter().map(|(e, _)| *e));
    }
    go
}
