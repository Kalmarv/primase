//! `lower-trackmania-spawns{others=travel|leave-out}`: the map's spawn locations (`trackmania.spawn`, one per block
//! whose model is a way point and states one) become `core.marker`s on their own entities. The start's (way point 0,
//! or 4 for a start and finish) is the `spawn`; a map has one. The others (the checkpoints' and the finish's) are
//! `travel` markers, or left out (counted), the program's choice (no default).
//!
//! A car's spawn location faces its local +Z (the start blocks' spawns face down their roads, away from the start line's
//! back: of the 25 campaign starts with road on one side only, 23 have it ahead), as a marker faces its own, so the
//! location is the marker's transform as read.

use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Marker, World};

use crate::components::TrackmaniaSpawn;

/// What the spawns other than the start become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherSpawns {
    /// `core.marker` kind `travel`.
    Travel,
    /// Despawned, counted.
    LeaveOut,
}

impl Choice for OtherSpawns {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("travel", OtherSpawns::Travel),
        ("leave-out", OtherSpawns::LeaveOut),
    ];
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerTrackmaniaSpawns {
    pub others: OtherSpawns,
}

/// `lower-trackmania-spawns`' declaration ([`primase_core::pass::PassRegistry::add`]): the option is the program's.
pub const LOWER_TRACKMANIA_SPAWNS: PassSpec = PassSpec {
    name: "lower-trackmania-spawns",
    options: &[OptionSpec::choice::<OtherSpawns>(
        "others",
        "the checkpoints' and the finish's spawns as travel markers, or left out (counted)",
    )],
    make: |g| {
        Ok(Box::new(LowerTrackmaniaSpawns {
            others: g.choice("others"),
        }))
    },
};

/// Whether a way point type is a start (0, or 4 for a start and finish).
fn starts(waypoint: Option<i32>) -> bool {
    matches!(waypoint, Some(0 | 4))
}

impl Pass for LowerTrackmaniaSpawns {
    fn name(&self) -> &'static str {
        "lower-trackmania-spawns"
    }
    fn options(&self) -> Options {
        Options::from([("others".to_owned(), self.others.text().to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[TrackmaniaSpawn::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME]
    }
    fn despawns_entities(&self) -> bool {
        self.others == OtherSpawns::LeaveOut
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let spawns: Vec<(Entity, Option<i32>)> = w
            .iter::<TrackmaniaSpawn>()
            .map(|(e, s)| (e, s.waypoint))
            .collect();
        let start = spawns
            .iter()
            .find(|(_, waypoint)| starts(*waypoint))
            .map(|(e, _)| *e)
            .ok_or("the map has no start block with a spawn")?;
        for (e, _) in spawns {
            let kind = if e == start {
                "spawn"
            } else {
                match self.others {
                    OtherSpawns::Travel => "travel",
                    OtherSpawns::LeaveOut => {
                        w.despawn(e);
                        log.count("spawns other than the start left out", 1);
                        continue;
                    }
                }
            };
            log.count(&format!("spawns made {kind} markers"), 1);
            w.insert(
                e,
                Marker {
                    kind: kind.into(),
                    ground: None,
                    yaw: None,
                },
            );
        }
        Ok(())
    }
}
