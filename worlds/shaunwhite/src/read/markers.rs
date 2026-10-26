//! Which Entities mark a place rather than draw one: the table from this game's names to `core.marker` kinds.
//!
//! Two sources say what an Entity marks. A **spawn node** carries a component of the SpawnPoint class
//! ([`crate::read::scene::SPAWN_POINT`]): the data states it, so the kind `spawn_point` is read from the class, not a
//! name (82 in level 01: 74 are `01_A_/01_B_SpawnPoint_Plaza_NNN`, the rest nodes named after NPC trajectories).
//! A **player spawner** carries a component of the PlayerSpawner class ([`crate::read::scene::PLAYER_SPAWNER`]): where
//! a mission puts the skater down (level 01: one in a cell; the Stronghold's root entry: ten), and so does a
//! multiplayer world's [`crate::read::scene::MULTI_POINT_PLAYER_SPAWNER`] (one in each root entry);
//! it is read as a `start`, named by its Entity. A **start position** is an Entity with no component at all, whose only
//! statement is its name; [`START_NAMES`] is the table of those names. The reader places each as a `core.marker` at the Entity's matrix; which local axis the game
//! calls forward is not read, so the marker carries the whole matrix and no yaw.
//!
//! What no row maps is not guessed: an Entity that has neither a Visual nor a marker is counted in the reader's
//! report (`entities with neither a Visual nor a marker`).

use crate::read::scene::{MULTI_POINT_PLAYER_SPAWNER, PLAYER_SPAWNER, SPAWN_POINT};

/// The `core.marker` kind of a spawn node (a SpawnPoint component).
pub const SPAWN_POINT_KIND: &str = "spawn_point";
/// The `core.marker` kind of a start position (a name in [`START_NAMES`], or a PlayerSpawner component).
pub const START_KIND: &str = "start";

/// One row of the start-position table: a name that contains `needle` marks where something starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartName {
    pub needle: &'static str,
    /// Names of level 01 that the row maps.
    pub examples: &'static str,
}

/// The names that mark a start position, from level 01's Entities that have no components and a name like these.
/// `01_C_Train01_StartPos` is where a train starts, not the player; the kind says "start" and the name keeps the rest.
pub const START_NAMES: [StartName; 5] = [
    StartName {
        needle: "StartPosition",
        examples: "CH_GAME_StartPosition, D01_C04_StartPosition, CH_STRIDE_Flow_StartPosition",
    },
    StartName {
        needle: "Start_Pos",
        examples: "D01_C05_Start_Pos, D01_C09_Start_Pos",
    },
    StartName {
        needle: "StartPos",
        examples: "01_C_Train01_StartPos, 01_C_Train03_StartPos_001",
    },
    StartName {
        needle: "FinalPlayerPos",
        examples: "Q01_Intro_FinalPlayerPos (the intro cinematic's hand-over to the skater), Q20_sq03_Outro_FinalPlayerPos_01B",
    },
    StartName {
        needle: "PlayerRefpos",
        examples: "Q03_Intro_PlayerRefpos_01C",
    },
];

/// The marker kind of an Entity named `name` whose components have `components` as their classes, or `None` when
/// the Entity marks nothing this table knows.
pub fn kind(name: &str, components: &[u32]) -> Option<&'static str> {
    if components.contains(&SPAWN_POINT) {
        Some(SPAWN_POINT_KIND)
    } else if components.contains(&PLAYER_SPAWNER)
        || components.contains(&MULTI_POINT_PLAYER_SPAWNER)
        || START_NAMES.iter().any(|r| name.contains(r.needle))
    {
        Some(START_KIND)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows map the names their examples quote, from the class alone for a spawn node, and map nothing else.
    #[test]
    fn rows_map_their_examples_and_a_spawn_node_by_its_class() {
        for r in START_NAMES {
            assert!(
                kind(&format!("D01_C99_{}", r.needle), &[]) == Some(START_KIND),
                "{}",
                r.needle
            );
        }
        for name in [
            "CH_GAME_StartPosition",
            "D01_C05_Start_Pos",
            "01_C_Train02_StartPos_001",
            "Q01_Intro_FinalPlayerPos",
            "Q03_Intro_PlayerRefpos_01C",
        ] {
            assert_eq!(kind(name, &[]), Some(START_KIND), "{name}");
        }
        // a spawn node is named by its class, whatever its name
        assert_eq!(kind("anything", &[0x1234, SPAWN_POINT]), Some(SPAWN_POINT_KIND));
        // a player spawner is a start by its class, whatever its name
        assert_eq!(kind("anything", &[PLAYER_SPAWNER]), Some(START_KIND));
        assert_eq!(kind("anything", &[MULTI_POINT_PLAYER_SPAWNER]), Some(START_KIND));
        // an Entity that draws or scripts something else is no marker
        for name in [
            "01_B_Arc_06_Building_03",
            "Q01_Intro_SCE_01B",
            "SND_Music_Map01_V1_Start",
            "CH_GAME_ChallengeGiver",
        ] {
            assert_eq!(kind(name, &[0x1234]), None, "{name}");
        }
    }
}
