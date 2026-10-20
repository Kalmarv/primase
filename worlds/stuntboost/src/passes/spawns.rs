//! `lower-stuntboost-spawns{start=NAME,others=travel|leave-out}`: the level's spawn nodes (class `Spawn`,
//! `=Spawn(<name>[,<n>])`) become `core.marker`s on their own entities, placed by their transforms: the spawn named
//! `start` (its first argument; `start=` for `=Spawn()` or `=Spawn(,n)`) of kind `spawn`, the others of kind `travel`
//! or left out (counted). A level holds several spawns when gameplay levels share its geometry, one per level; which
//! one a map starts at is the program's choice, stated (no default). A `start` no spawn names is an error. The node
//! keeps its `stuntboost.node` (its arguments, as stored).

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, Marker, World};

use crate::components::StuntboostNode;

/// What the spawns other than the start become.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherSpawns {
    /// `core.marker` kind `travel`.
    Travel,
    /// Despawned, counted.
    LeaveOut,
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerStuntboostSpawns {
    pub start: String,
    pub others: OtherSpawns,
}

impl LowerStuntboostSpawns {
    /// From its options, both required: `start` (a spawn's name, may be empty), `others` (`travel` or `leave-out`).
    pub fn from_options(o: &Options) -> Result<LowerStuntboostSpawns, String> {
        if let Some(k) = o.keys().find(|k| !["start", "others"].contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let get = |k: &str| {
            o.get(k).ok_or_else(|| {
                format!("{k} is required: start=<spawn name, may be empty>, others=travel|leave-out")
            })
        };
        Ok(LowerStuntboostSpawns {
            start: get("start")?.clone(),
            others: match get("others")?.as_str() {
                "travel" => OtherSpawns::Travel,
                "leave-out" => OtherSpawns::LeaveOut,
                v => return Err(format!("others={v}: travel or leave-out")),
            },
        })
    }
}

/// A spawn node's name: its first argument, empty when none.
fn spawn_name(n: &StuntboostNode) -> Option<&str> {
    let class = n.name.class.as_ref().filter(|c| c.name == "Spawn")?;
    Some(
        class
            .args
            .as_ref()
            .and_then(|a| a.first())
            .map_or("", String::as_str),
    )
}

impl Pass for LowerStuntboostSpawns {
    fn name(&self) -> &'static str {
        "lower-stuntboost-spawns"
    }
    fn options(&self) -> Options {
        Options::from([
            ("start".to_owned(), self.start.clone()),
            (
                "others".to_owned(),
                match self.others {
                    OtherSpawns::Travel => "travel",
                    OtherSpawns::LeaveOut => "leave-out",
                }
                .to_owned(),
            ),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME]
    }
    fn despawns_entities(&self) -> bool {
        self.others == OtherSpawns::LeaveOut
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let spawns: Vec<(Entity, String)> = w
            .iter::<StuntboostNode>()
            .filter_map(|(e, n)| Some((e, spawn_name(n)?.to_owned())))
            .collect();
        if !spawns.iter().any(|(_, name)| *name == self.start) {
            let names: Vec<&str> = spawns.iter().map(|(_, n)| n.as_str()).collect();
            return Err(format!("no spawn named {:?}; the spawns: {names:?}", self.start));
        }
        let mut started = false;
        for (e, name) in spawns {
            // the first spawn of the start's name starts; a second of the same name is another
            let kind = if name == self.start && !started {
                started = true;
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
