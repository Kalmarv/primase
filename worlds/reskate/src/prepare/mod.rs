//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::dialect`]) its writers take.

pub mod budget;
pub mod floor;
pub mod raise;
pub mod region;
pub mod snap;
pub mod spawn;
pub mod textures;

pub use budget::{BudgetReskateTextures, budget_reskate_textures};
pub use floor::{PrepareReskateFloor, prepare_reskate_floor};
pub use raise::{
    RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate, raise_skate_pass, studio_watts,
};
pub use region::{ClipReskateRegion, clip_reskate_region};
pub use snap::{SnapReskateMarkers, snap_reskate_markers};
pub use spawn::{PlaceReskateSpawn, ReskateSpawnAt, place_reskate_spawn};
pub use textures::{PrepareReskateTextures, tint_pixels};
