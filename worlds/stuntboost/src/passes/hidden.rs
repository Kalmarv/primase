//! `omit-stuntboost-hidden{invisible=..,shadow-only=..,triggers=..,classes=C+C...}`: what the game never draws, and
//! the gameplay and editor nodes a static map has no use for, left out (counted by what and class). Each choice is the
//! program's, stated (no default):
//!
//! - `invisible=leave-out|keep`: nodes named invisible (`_`, never drawn: collision-only `_<`, helpers). One that
//!   collides (it has `core.collision`) keeps its entity and collision and loses only its mesh placement; any other is
//!   despawned.
//! - `shadow-only=leave-out|keep`: nodes the game draws only into its shadows (`BackfaceShadowCaster`: the rooms'
//!   ceilings), despawned.
//! - `triggers=leave-out|keep`: trigger nodes (`#`: checkpoints, goals, boosters, collectibles), despawned.
//! - `classes`: nodes of these classes (`PreviewCamera`, `Sun`, `SetBakedLighting`, `GoalCameraPoints`,
//!   `CheckpointDirection`, ...; joined by `+`, empty for none), despawned.
//!
//! A node `lower-stuntboost-spawns` made a marker stays. Run it after `lower-stuntboost-collision` and
//! `lower-stuntboost-spawns`.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Instance, Marker, World};

use crate::components::{StuntboostCollision, StuntboostDraw, StuntboostNode};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmitStuntboostHidden {
    pub invisible: bool,
    pub shadow_only: bool,
    pub triggers: bool,
    pub classes: BTreeSet<String>,
}

fn choice(o: &Options, k: &str) -> Result<bool, String> {
    match o.get(k).map(String::as_str) {
        Some("leave-out") => Ok(true),
        Some("keep") => Ok(false),
        Some(v) => Err(format!("{k}={v}: leave-out or keep")),
        None => Err(format!(
            "{k} is required: invisible=, shadow-only=, triggers= (leave-out|keep), classes= (joined by +, empty for none)"
        )),
    }
}

impl OmitStuntboostHidden {
    /// From its options, every one required.
    pub fn from_options(o: &Options) -> Result<OmitStuntboostHidden, String> {
        if let Some(k) = o
            .keys()
            .find(|k| !["invisible", "shadow-only", "triggers", "classes"].contains(&k.as_str()))
        {
            return Err(format!("unknown option {k}"));
        }
        let classes = o
            .get("classes")
            .ok_or("classes is required: class names joined by +, empty for none")?;
        Ok(OmitStuntboostHidden {
            invisible: choice(o, "invisible")?,
            shadow_only: choice(o, "shadow-only")?,
            triggers: choice(o, "triggers")?,
            classes: classes
                .split('+')
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}

impl Pass for OmitStuntboostHidden {
    fn name(&self) -> &'static str {
        "omit-stuntboost-hidden"
    }
    fn options(&self) -> Options {
        let word = |b: bool| if b { "leave-out" } else { "keep" }.to_owned();
        Options::from([
            ("invisible".to_owned(), word(self.invisible)),
            ("shadow-only".to_owned(), word(self.shadow_only)),
            ("triggers".to_owned(), word(self.triggers)),
            (
                "classes".to_owned(),
                self.classes.iter().cloned().collect::<Vec<_>>().join("+"),
            ),
        ])
    }
    /// A collision-only node loses its placement (it keeps its entity and collision).
    fn removes(&self) -> &[&'static str] {
        &[<Instance as primase_core::Component>::NAME]
    }
    /// It lowers nothing: what it takes away is left out, not made into another form.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // (entity, what, despawn: else only its placement goes)
        let mut gone: Vec<(Entity, String, bool)> = Vec::new();
        for (e, n) in w.iter::<StuntboostNode>() {
            // a spawn lowered to a marker stays whatever its name says
            if w.has::<Marker>(e) {
                continue;
            }
            let class = n.name.class_name();
            let what = if self.classes.contains(class) {
                Some(format!("{class} nodes"))
            } else if self.triggers && n.name.collision == StuntboostCollision::Trigger {
                Some(format!("triggers ({class})"))
            } else if self.shadow_only && n.name.draw() == StuntboostDraw::ShadowOnly {
                Some("shadow-only nodes".to_owned())
            } else if self.invisible && n.name.draw() == StuntboostDraw::Invisible {
                Some("invisible nodes".to_owned())
            } else {
                None
            };
            if let Some(what) = what {
                let keep_collision = what == "invisible nodes" && w.has::<Collision>(e);
                gone.push((e, what, !keep_collision));
            }
        }
        for (e, what, despawn) in gone {
            if despawn {
                w.despawn(e);
                log.count(&format!("{what} left out"), 1);
            } else if w.remove::<Instance>(e).is_some() {
                log.count("invisible collision nodes kept without their mesh", 1);
            }
        }
        Ok(())
    }
}
