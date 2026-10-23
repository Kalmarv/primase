//! `lower-stuntboost-spawns{start=NAME|first|none,others=travel|leave-out}`: the level's spawn nodes (class `Spawn`,
//! `=Spawn(<name>[,<n>])`) become `core.marker`s on their own entities, placed by their transforms: the start spawn of
//! kind `spawn`, the others of kind `travel` or left out (counted). A level holds several spawns when gameplay levels
//! share its geometry, one per level; which one a map starts at is the program's choice, stated (no default):
//!
//! - `start=NAME`: the spawn named so (its first argument; `start=` for `=Spawn()` or `=Spawn(,n)`); a `start` no
//!   spawn names is an error. [`spawns`] lists the names.
//! - `start=first`: the first spawn in stored order (the order of [`spawns`]); a level without a spawn is an error.
//!   `first` is this word, not a spawn named `first`; `start=name:first` is that spawn.
//! - `start=none`: no spawn starts; every spawn is one of the others.
//!
//! The node keeps its `stuntboost.node` (its arguments, as stored).

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
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

impl Choice for OtherSpawns {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("travel", OtherSpawns::Travel),
        ("leave-out", OtherSpawns::LeaveOut),
    ];
}

/// Which spawn starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// The spawn with this name (its first argument, empty when none).
    Named(String),
    /// The first spawn in stored order.
    First,
    /// None: every spawn is one of the others.
    None,
}

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowerStuntboostSpawns {
    pub start: Start,
    pub others: OtherSpawns,
}

/// A spawn node of the level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub entity: Entity,
    /// The spawn's name: its first argument, empty when none.
    pub name: String,
}

/// The level's spawn nodes in stored order (the order of the nodes in their parts, parts in order), each with its
/// name. Read from the nodes' parsed names, so it holds what [`crate::read_level`] read, whichever lowering ran.
pub fn spawns(w: &World) -> Vec<Spawn> {
    w.iter::<StuntboostNode>()
        .filter_map(|(entity, n)| {
            Some(Spawn {
                entity,
                name: spawn_name(n)?.to_owned(),
            })
        })
        .collect()
}

impl LowerStuntboostSpawns {
    /// From its declared options: `start` a spawn's name (which may be empty, written `name:<name>` when it is one
    /// of the words or starts so), `first` or `none` (see the module documentation).
    fn from_given(g: &Given) -> LowerStuntboostSpawns {
        LowerStuntboostSpawns {
            start: match g.required("start") {
                "first" => Start::First,
                "none" => Start::None,
                v => Start::Named(v.strip_prefix("name:").unwrap_or(v).to_owned()),
            },
            others: g.choice("others"),
        }
    }
}

/// `lower-stuntboost-spawns`' declaration ([`primase_core::pass::PassRegistry::add`]): both options are the program's,
/// without defaults.
pub const LOWER_STUNTBOOST_SPAWNS: PassSpec = PassSpec {
    name: "lower-stuntboost-spawns",
    options: &[
        OptionSpec::required(
            "start",
            "start",
            "the spawn that starts: its name (may be empty; name:<name> for one called first or none), first (the \
             first in stored order) or none",
        ),
        OptionSpec::choice::<OtherSpawns>(
            "others",
            "the other spawns as travel markers, or left out (counted)",
        ),
    ],
    make: |g| Ok(Box::new(LowerStuntboostSpawns::from_given(g))),
};

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
            (
                "start".to_owned(),
                match &self.start {
                    Start::First => "first".to_owned(),
                    Start::None => "none".to_owned(),
                    // the words are the options' own; a spawn named so is written with its prefix
                    Start::Named(n) if matches!(n.as_str(), "first" | "none") || n.starts_with("name:") => {
                        format!("name:{n}")
                    }
                    Start::Named(n) => n.clone(),
                },
            ),
            ("others".to_owned(), self.others.text().to_owned()),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[Marker::NAME]
    }
    fn despawns_entities(&self) -> bool {
        self.others == OtherSpawns::LeaveOut
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let spawns = spawns(w);
        let start = match &self.start {
            Start::Named(name) => match spawns.iter().position(|spawn| spawn.name == *name) {
                Some(i) => Some(i),
                None => {
                    let names: Vec<&str> = spawns.iter().map(|spawn| spawn.name.as_str()).collect();
                    return Err(format!("no spawn named {name:?}; the spawns: {names:?}"));
                }
            },
            Start::First if spawns.is_empty() => return Err("start=first: the level has no spawn".to_owned()),
            Start::First => Some(0),
            Start::None => None,
        };
        for (i, Spawn { entity: e, .. }) in spawns.into_iter().enumerate() {
            // the first spawn of the start's name starts; a second of the same name is another
            let kind = if Some(i) == start {
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
