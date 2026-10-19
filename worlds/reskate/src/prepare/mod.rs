//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::dialect`]) its writers take.

pub mod budget;
pub mod raise;
pub mod textures;

pub use budget::{BudgetReskateTextures, budget_reskate_textures};
pub use raise::{
    RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate, raise_skate_pass, studio_watts,
};
pub use textures::{PrepareReskateTextures, tint_pixels};
