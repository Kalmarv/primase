//! The `skatebird.*` dialect: what SkateBIRD's scripts state about an object, as stored. Each is on its own entity
//! with the object's world `core.transform`, `core.name` (its GameObject's), `core.source` and, when the object is
//! stored inactive or the script disabled, a `core.condition` on the Unity level read's axes. The object's forward
//! (Unity's +Z) is that transform's -Z column in the core frame: a yaw of atan2(-x, -z) over it gives the game's own
//! (level2's start 7.0659 degrees, level1's 234.8157).
use primase_core::{Component, JsonError};
use primase_unity::components::UnityObject;
use serde::Serialize;

/// A `ResetPosition` script: a point the player is put back at, and whether it is where the level starts
/// (`startingPosition`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdResetPosition {
    pub starting_position: bool,
    /// The script's MonoBehaviour.
    pub behaviour: UnityObject,
    /// Its GameObject's path in the scene.
    pub path: String,
}

impl Component for SkatebirdResetPosition {
    const NAME: &'static str = "skatebird.reset_position";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A `MissionGiver` script: where a mission is given, and its title as the script stores it (its first field; `None`
/// when empty).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdMissionGiver {
    pub title: Option<String>,
    /// The script's MonoBehaviour.
    pub behaviour: UnityObject,
    /// Its GameObject's path in the scene.
    pub path: String,
}

impl Component for SkatebirdMissionGiver {
    const NAME: &'static str = "skatebird.mission_giver";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A SkateBIRD script that says something about every object under its GameObject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkatebirdRole {
    /// `SkateCollectible`: a mission's collectible (shown while its mission runs).
    Collectible,
    /// `EnableOneAtRandom`: its children are candidates the game shows one of, picked at random.
    EnableOneAtRandom,
    /// `MissionGiver`: a bird that gives a mission.
    MissionGiver,
    /// `BirbDecorator`: a bird's dressing (hats, accessories).
    BirbDecorator,
    /// `PermanentUnlockingCollectible`: an unlock (a deck, wheels, clothes) shown until collected.
    UnlockingCollectible,
    /// `SkateboardGateway`: the player's own board, placed in every level scene.
    PlayerBoard,
    /// A scene camera (a Camera component, no script): what hangs under it moves with the view (the HUD's mission
    /// arrow).
    Camera,
}

impl SkatebirdRole {
    /// Every role, in order.
    pub const ALL: [SkatebirdRole; 7] = [
        SkatebirdRole::Collectible,
        SkatebirdRole::EnableOneAtRandom,
        SkatebirdRole::MissionGiver,
        SkatebirdRole::BirbDecorator,
        SkatebirdRole::UnlockingCollectible,
        SkatebirdRole::PlayerBoard,
        SkatebirdRole::Camera,
    ];

    /// The role of a script class (its name without the namespace). Classes are compared in lowercase: the game's
    /// classes differ in more than case, and the architecture check's word splitter reads the capitalised
    /// `SkateCollectible` as the skate destination's name, which this crate may not use.
    pub fn of_class(class: &str) -> Option<SkatebirdRole> {
        match class.to_ascii_lowercase().as_str() {
            "skatecollectible" => Some(SkatebirdRole::Collectible),
            "enableoneatrandom" => Some(SkatebirdRole::EnableOneAtRandom),
            "missiongiver" => Some(SkatebirdRole::MissionGiver),
            "birbdecorator" => Some(SkatebirdRole::BirbDecorator),
            "permanentunlockingcollectible" => Some(SkatebirdRole::UnlockingCollectible),
            "skateboardgateway" => Some(SkatebirdRole::PlayerBoard),
            _ => None,
        }
    }

    /// Its option word (`collectible`, `enable_one_at_random`, `mission_giver`, `birb_decorator`,
    /// `unlocking_collectible`, `player_board`, `camera`).
    pub fn word(self) -> &'static str {
        match self {
            SkatebirdRole::Collectible => "collectible",
            SkatebirdRole::EnableOneAtRandom => "enable_one_at_random",
            SkatebirdRole::MissionGiver => "mission_giver",
            SkatebirdRole::BirbDecorator => "birb_decorator",
            SkatebirdRole::UnlockingCollectible => "unlocking_collectible",
            SkatebirdRole::PlayerBoard => "player_board",
            SkatebirdRole::Camera => "camera",
        }
    }

    /// The role an option word names.
    pub fn of_word(word: &str) -> Option<SkatebirdRole> {
        SkatebirdRole::ALL.into_iter().find(|r| r.word() == word)
    }
}

/// One script over an object: its role, its MonoBehaviour (a camera's Camera component), whether it is stored enabled,
/// and the path of the GameObject carrying it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdScript {
    pub role: SkatebirdRole,
    pub behaviour: UnityObject,
    pub enabled: bool,
    pub path: String,
}

/// The [`SkatebirdRole`] scripts on the GameObject an entity of the Unity level read comes from (a renderer's, a
/// collider's or a light's) or on a GameObject above it, the outermost first. On the entity itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdUnder(pub Vec<SkatebirdScript>);

impl Component for SkatebirdUnder {
    const NAME: &'static str = "skatebird.under";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// One node of a grind rail as stored: its `GrindableRail` MonoBehaviour, its GameObject's path, the script's two
/// object references after `prev` and `next` (their meaning not known; `None`: null) and its eight flag bytes (each 0
/// or 1, meaning not known).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdRailNode {
    pub behaviour: UnityObject,
    pub path: String,
    pub third: Option<UnityObject>,
    pub fourth: Option<UnityObject>,
    pub flags: [u8; 8],
}

/// A grind rail: a chain of `GrindableRail` nodes linked by their `prev`/`next` references, in order from the node
/// with no `prev` (a closed chain from its lowest path id). Its entity carries the chain as `core.curve` (kind `rail`,
/// the nodes' world positions); this keeps the nodes as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkatebirdRail {
    pub nodes: Vec<SkatebirdRailNode>,
}

impl Component for SkatebirdRail {
    const NAME: &'static str = "skatebird.rail";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
