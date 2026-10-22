//! `select-anvil-entities{hidden=keep|drop, phantom=keep|drop, water-collision=keep|drop}` (all required): which of
//! what the reader places a destination takes, where the source marks it as something other than ordinary content.
//! The reader keeps every placement with what the source states of it; this pass is where a program chooses.
//!
//! - `hidden`, `phantom`: placements of Entities and EntityGroups whose `IsHidden` or `IsPhantom` is set
//!   ([`AnvilEntityState`]: on Riders Republic UI, HUD and editor helpers, docs/placement.md §19.1). With `drop`, a
//!   placement whose owner sets the flag goes (one setting both goes when either says `drop`).
//! - `water-collision`: collision placed from a RigidBody that is a water surface
//!   ([`AnvilCollisionFilter::is_water_surface`]: `WaterLevel` set, `Static` clear; the river surfaces).
//!
//! What goes is despawned and declared, counted per reason; the marks are removed from what stays (the filter stays: it
//! is the collision's own data).

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, World};

use crate::components::{AnvilCollisionFilter, AnvilEntityState};

/// Whether a kind of placement stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Keep,
    Drop,
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectAnvilEntities {
    pub hidden: Keep,
    pub phantom: Keep,
    pub water_collision: Keep,
}

impl SelectAnvilEntities {
    /// From its options, all required.
    pub fn from_options(o: &Options) -> Result<SelectAnvilEntities, String> {
        let known = ["hidden", "phantom", "water-collision"];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| match o.get(k).map(String::as_str) {
            Some("keep") => Ok(Keep::Keep),
            Some("drop") => Ok(Keep::Drop),
            Some(v) => Err(format!("{k}={v}: keep or drop")),
            None => Err(format!(
                "{k} is required: hidden=keep|drop, phantom=keep|drop, water-collision=keep|drop"
            )),
        };
        Ok(SelectAnvilEntities {
            hidden: get("hidden")?,
            phantom: get("phantom")?,
            water_collision: get("water-collision")?,
        })
    }
}

fn word(k: Keep) -> &'static str {
    match k {
        Keep::Keep => "keep",
        Keep::Drop => "drop",
    }
}

impl Pass for SelectAnvilEntities {
    fn name(&self) -> &'static str {
        "select-anvil-entities"
    }
    fn options(&self) -> Options {
        Options::from([
            ("hidden".to_owned(), word(self.hidden).to_owned()),
            ("phantom".to_owned(), word(self.phantom).to_owned()),
            (
                "water-collision".to_owned(),
                word(self.water_collision).to_owned(),
            ),
        ])
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilEntityState::NAME]
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
        Ok(())
    }
}
