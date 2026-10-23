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

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
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

/// Options `invisible`, `shadow-only` and `triggers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hidden {
    LeaveOut,
    Keep,
}

impl Choice for Hidden {
    const VALUES: &'static [(&'static str, Self)] =
        &[("leave-out", Hidden::LeaveOut), ("keep", Hidden::Keep)];
}

impl OmitStuntboostHidden {
    /// From its declared options.
    fn from_given(g: &Given) -> OmitStuntboostHidden {
        let leave_out = |k: &str| g.choice::<Hidden>(k) == Hidden::LeaveOut;
        OmitStuntboostHidden {
            invisible: leave_out("invisible"),
            shadow_only: leave_out("shadow-only"),
            triggers: leave_out("triggers"),
            classes: g
                .required("classes")
                .split('+')
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// `omit-stuntboost-hidden`'s declaration ([`primase_core::pass::PassRegistry::add`]): every option is the program's,
/// without defaults.
pub const OMIT_STUNTBOOST_HIDDEN: PassSpec = PassSpec {
    name: "omit-stuntboost-hidden",
    options: &[
        OptionSpec::choice::<Hidden>("invisible", "the invisible nodes left out, or kept"),
        OptionSpec::choice::<Hidden>(
            "shadow-only",
            "the nodes drawn only into shadows left out, or kept",
        ),
        OptionSpec::choice::<Hidden>("triggers", "the trigger nodes left out, or kept"),
        OptionSpec::required(
            "classes",
            "PreviewCamera+Sun+SetBakedLighting",
            "the node classes left out, joined by + (empty for none)",
        ),
    ],
    make: |g| Ok(Box::new(OmitStuntboostHidden::from_given(g))),
};

impl Pass for OmitStuntboostHidden {
    fn name(&self) -> &'static str {
        "omit-stuntboost-hidden"
    }
    fn options(&self) -> Options {
        let word = |b: bool| if b { Hidden::LeaveOut } else { Hidden::Keep }.text().to_owned();
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
