//! `select-anvil-entities{hidden=keep|drop, phantom=keep|drop, water-collision=keep|drop,
//! collision-only=keep|drop|overlap, collision-only-overlap=F, collision-only-max-ratio=R,
//! collision-only-min-extent=M}` (all required but the last three, which go with `overlap` and are required there):
//! which of what the reader
//! places a destination takes, where the source marks it as something other than ordinary content. The reader keeps
//! every placement with what the source states of it; this pass is where a program chooses.
//!
//! - `hidden`, `phantom`: placements of Entities and EntityGroups whose `IsHidden` or `IsPhantom` is set
//!   ([`AnvilEntityState`]: on Riders Republic UI, HUD and editor helpers, docs/placement.md §19.1). With `drop`, a
//!   placement whose owner sets the flag goes (one setting both goes when either says `drop`).
//! - `water-collision`: collision placed from a RigidBody that is a water surface
//!   ([`AnvilCollisionFilter::is_water_surface`]: `WaterLevel` set, `Static` clear; the river surfaces).
//! - `collision-only`: collision placed for an owner that places no visual ([`AnvilCollisionOnly`]: a RigidBody with a
//!   shape and no Visual with something to show; on Riders Republic the `MAD_COL_*` blockers: gates, boxes, a hidden
//!   perch, the blocks of a wreck). A destination that draws the visuals and collides with the shapes shows these as
//!   invisible walls and ledges unless a visual stands where they are. `keep` keeps every one; `drop` drops every one;
//!   `overlap` keeps the ones whose shape lies inside a visual's bounds and drops the rest: the shape's box (its owner's
//!   `BoundingVolume`, [`AnvilBounds`]) is kept when **at least `F` of its volume lies inside the box of one visual
//!   placement** of another owner (its `BoundingVolume` too), a visual whose box is no more than `R` times the
//!   shape's volume (a terrain-, tile- or vegetation-list-sized box covers anything). The three numbers are the
//!   program's choices, none read from the game, and all required with `overlap` (refused without it): `F`
//!   `collision-only-overlap`, a fraction in (0, 1]; `R` `collision-only-max-ratio`, at least 1; `M`
//!   `collision-only-min-extent`, metres above 0, the least a box's side is taken to be when its volume is (a flat
//!   shape or a thin visual has a volume). Approximation, stated: the volumes are the owners' axis-aligned bounding
//!   boxes, not the meshes: a visual shaped like an L passes a shape that stands in its corner's air. Every
//!   placement's decision is counted, each owner dropped or kept is listed in the log by its name, and the three
//!   numbers are echoed in the approximation's count.
//!
//! What goes is despawned and declared, counted per reason; the marks are removed from what stays (the filter stays: it
//! is the collision's own data).

use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, Name, World};

use crate::components::{AnvilBounds, AnvilCollisionFilter, AnvilCollisionOnly, AnvilEntityState};

/// Whether a kind of placement stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Keep,
    Drop,
}

/// The program's three numbers for `collision-only=overlap` (none read from the game).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlap {
    /// One stays when at least this fraction (in (0, 1]) of its box's volume lies inside the box of a visual placement
    /// of another owner; the others go.
    pub fraction: f64,
    /// A visual's box counts for a shape only when its volume is at most this many times the shape's (at least 1):
    /// past it the box is a whole building, tile or vegetation list that covers anything standing in its air.
    pub max_ratio: f64,
    /// The least a box's side is taken to be when its volume is (metres, above 0): a flat shape or a thin visual has a
    /// volume.
    pub min_extent: f64,
}

/// What `collision-only` does with collision placed for an owner without a visual.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CollisionOnly {
    /// Every one stays.
    Keep,
    /// Every one goes.
    Drop,
    /// The ones inside a visual's box stay ([`Overlap`]); the others go.
    Overlap(Overlap),
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectAnvilEntities {
    pub hidden: Keep,
    pub phantom: Keep,
    pub water_collision: Keep,
    pub collision_only: CollisionOnly,
}

impl SelectAnvilEntities {
    /// From its options: all required; `collision-only-overlap`, `-max-ratio` and `-min-extent` with
    /// `collision-only=overlap` only, and all three then.
    pub fn from_options(o: &Options) -> Result<SelectAnvilEntities, String> {
        let known = [
            "hidden",
            "phantom",
            "water-collision",
            "collision-only",
            "collision-only-overlap",
            "collision-only-max-ratio",
            "collision-only-min-extent",
        ];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let usage = "hidden=keep|drop, phantom=keep|drop, water-collision=keep|drop, \
                     collision-only=keep|drop|overlap (overlap with collision-only-overlap=F, 0 < F <= 1, \
                     collision-only-max-ratio=R, R >= 1, collision-only-min-extent=M, M > 0 metres)";
        let get = |k: &str| match o.get(k).map(String::as_str) {
            Some("keep") => Ok(Keep::Keep),
            Some("drop") => Ok(Keep::Drop),
            Some(v) => Err(format!("{k}={v}: keep or drop")),
            None => Err(format!("{k} is required: {usage}")),
        };
        let given = OVERLAP_OPTIONS.iter().any(|k| o.contains_key(*k));
        let collision_only = match o.get("collision-only").map(String::as_str) {
            Some("keep") if !given => CollisionOnly::Keep,
            Some("drop") if !given => CollisionOnly::Drop,
            Some("keep" | "drop") => {
                return Err(format!(
                    "{} go with collision-only=overlap only",
                    OVERLAP_OPTIONS.join(", ")
                ));
            }
            Some("overlap") => {
                let number = |k: &str, ok: fn(f64) -> bool, rule: &str| -> Result<f64, String> {
                    let v = o
                        .get(k)
                        .ok_or_else(|| format!("collision-only=overlap needs {k}: {usage}"))?;
                    let n = v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number"))?;
                    if n.is_finite() && ok(n) {
                        Ok(n)
                    } else {
                        Err(format!("{k}={v}: {rule}"))
                    }
                };
                CollisionOnly::Overlap(Overlap {
                    fraction: number(
                        "collision-only-overlap",
                        |f| f > 0.0 && f <= 1.0,
                        "a fraction, 0 < F <= 1",
                    )?,
                    max_ratio: number("collision-only-max-ratio", |r| r >= 1.0, "a volume ratio, R >= 1")?,
                    min_extent: number("collision-only-min-extent", |m| m > 0.0, "metres, M > 0")?,
                })
            }
            Some(v) => return Err(format!("collision-only={v}: keep, drop or overlap")),
            None => return Err(format!("collision-only is required: {usage}")),
        };
        Ok(SelectAnvilEntities {
            hidden: get("hidden")?,
            phantom: get("phantom")?,
            water_collision: get("water-collision")?,
            collision_only,
        })
    }
}

/// The options that go with `collision-only=overlap` only.
const OVERLAP_OPTIONS: [&str; 3] = [
    "collision-only-overlap",
    "collision-only-max-ratio",
    "collision-only-min-extent",
];

fn word(k: Keep) -> &'static str {
    match k {
        Keep::Keep => "keep",
        Keep::Drop => "drop",
    }
}

/// `b`'s volume, each side at least `min_extent`.
fn volume(b: &AnvilBounds, min_extent: f64) -> f64 {
    (0..3)
        .map(|k| f64::from(b.max[k] - b.min[k]).max(min_extent))
        .product()
}

/// The volume of `a` (each side at least `min_extent`, grown about its centre) inside `b`, as a fraction of `a`'s.
fn inside(a: &AnvilBounds, b: &AnvilBounds, min_extent: f64) -> f64 {
    let mut shared = 1.0f64;
    for k in 0..3 {
        let centre = f64::from(a.min[k] + a.max[k]) / 2.0;
        let half = f64::from(a.max[k] - a.min[k]).max(min_extent) / 2.0;
        let (lo, hi) = (centre - half, centre + half);
        let side = (hi.min(f64::from(b.max[k])) - lo.max(f64::from(b.min[k]))).max(0.0);
        shared *= side;
    }
    shared / volume(a, min_extent)
}

impl Pass for SelectAnvilEntities {
    fn name(&self) -> &'static str {
        "select-anvil-entities"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("hidden".to_owned(), word(self.hidden).to_owned()),
            ("phantom".to_owned(), word(self.phantom).to_owned()),
            (
                "water-collision".to_owned(),
                word(self.water_collision).to_owned(),
            ),
        ]);
        match self.collision_only {
            CollisionOnly::Keep => o.insert("collision-only".to_owned(), "keep".to_owned()),
            CollisionOnly::Drop => o.insert("collision-only".to_owned(), "drop".to_owned()),
            CollisionOnly::Overlap(v) => {
                o.insert("collision-only-overlap".to_owned(), v.fraction.to_string());
                o.insert("collision-only-max-ratio".to_owned(), v.max_ratio.to_string());
                o.insert("collision-only-min-extent".to_owned(), v.min_extent.to_string());
                o.insert("collision-only".to_owned(), "overlap".to_owned())
            }
        };
        o
    }
    fn removes(&self) -> &[&'static str] {
        &[
            AnvilEntityState::NAME,
            AnvilCollisionOnly::NAME,
            AnvilBounds::NAME,
        ]
    }
    fn approximates(&self) -> &'static str {
        "collision-only=overlap tests the owners' axis-aligned bounding boxes, not their meshes; its fraction, \
         visual-to-shape volume ratio and least box side are the program's choices"
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let marked: Vec<(Entity, AnvilEntityState)> =
            w.iter::<AnvilEntityState>().map(|(e, s)| (e, *s)).collect();
        for (e, s) in marked {
            let why = match (
                s.hidden && self.hidden == Keep::Drop,
                s.phantom && self.phantom == Keep::Drop,
            ) {
                (true, _) => Some("its owner's IsHidden is set (select-anvil-entities hidden=drop)"),
                (_, true) => Some("its owner's IsPhantom is set (select-anvil-entities phantom=drop)"),
                _ => None,
            };
            match why {
                Some(why) => {
                    primase_core::ledger::despawn(w, e, log, why);
                    log.count("placements of hidden or phantom owners dropped", 1);
                }
                None => {
                    w.remove::<AnvilEntityState>(e);
                    log.count("placements of hidden or phantom owners kept", 1);
                }
            }
        }
        let water: Vec<Entity> = w
            .iter::<AnvilCollisionFilter>()
            .filter(|(_, f)| f.is_water_surface())
            .map(|(e, _)| e)
            .collect();
        for e in water {
            if self.water_collision == Keep::Drop {
                primase_core::ledger::despawn(
                    w,
                    e,
                    log,
                    "collision of a water surface (select-anvil-entities water-collision=drop)",
                );
                log.count("water surface collision placements dropped", 1);
            } else {
                log.count("water surface collision placements kept", 1);
            }
        }
        self.collision_only(w, log);
        Ok(())
    }
}

impl SelectAnvilEntities {
    /// `collision-only`: the placements of owners without a visual ([`AnvilCollisionOnly`]).
    fn collision_only(&self, w: &mut World, log: &mut PassLog) {
        let only: Vec<(Entity, AnvilBounds, String)> = w
            .iter::<AnvilCollisionOnly>()
            .filter_map(|(e, _)| {
                let b = *w.get::<AnvilBounds>(e)?;
                Some((e, b, w.get::<Name>(e).map(|n| n.0.clone()).unwrap_or_default()))
            })
            .collect();
        // the visual placements' boxes (everything with bounds that is not collision-only), once each
        let visuals: Vec<AnvilBounds> = match self.collision_only {
            CollisionOnly::Overlap(_) => {
                let mut v: Vec<AnvilBounds> = w
                    .iter::<AnvilBounds>()
                    .filter(|(e, _)| w.get::<AnvilCollisionOnly>(*e).is_none())
                    .map(|(_, b)| *b)
                    .collect();
                v.sort_by(|a, b| {
                    a.min
                        .iter()
                        .chain(&a.max)
                        .map(|x| x.to_bits())
                        .cmp(b.min.iter().chain(&b.max).map(|x| x.to_bits()))
                });
                v.dedup();
                v
            }
            _ => Vec::new(),
        };
        let mut by_name: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut tested = 0u64;
        for (e, b, name) in only {
            let kept = match self.collision_only {
                CollisionOnly::Keep => true,
                CollisionOnly::Drop => false,
                CollisionOnly::Overlap(v) => {
                    let limit = volume(&b, v.min_extent) * v.max_ratio;
                    tested += 1;
                    visuals.iter().any(|vis| {
                        volume(vis, v.min_extent) <= limit && inside(&b, vis, v.min_extent) >= v.fraction
                    })
                }
            };
            let entry = by_name.entry(name).or_default();
            if kept {
                entry.0 += 1;
                w.remove::<AnvilCollisionOnly>(e);
                w.remove::<AnvilBounds>(e);
                log.count("collision-only placements kept", 1);
            } else {
                entry.1 += 1;
                primase_core::ledger::despawn(
                    w,
                    e,
                    log,
                    "collision of an owner with no visual (select-anvil-entities collision-only)",
                );
                log.count("collision-only placements dropped", 1);
            }
        }
        if let CollisionOnly::Overlap(v) = self.collision_only {
            log.approximate(
                &format!(
                    "collision-only=overlap placements tested against owners' bounding boxes (fraction {}, visual at most {} times the shape's volume, sides at least {} m)",
                    v.fraction, v.max_ratio, v.min_extent
                ),
                tested,
            );
        }
        for (name, (kept, dropped)) in by_name {
            if kept > 0 {
                log.count(&format!("collision-only owner kept: {name}"), kept);
            }
            if dropped > 0 {
                log.count(&format!("collision-only owner dropped: {name}"), dropped);
            }
        }
        // the bounds of the visual placements are not carried on
        let rest: Vec<Entity> = w.iter::<AnvilBounds>().map(|(e, _)| e).collect();
        for e in rest {
            w.remove::<AnvilBounds>(e);
        }
    }
}
