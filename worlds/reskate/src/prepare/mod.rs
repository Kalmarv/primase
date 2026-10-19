//! Preparation for the ReSkate target: passes a program selects to turn core data into the `skate.*` dialect
//! ([`crate::dialect`]) its writers take.

pub mod raise;

pub use raise::{
    RaiseOptions, RaiseSkate, SKATE_LIGHT_SCALE, class_to_skate, raise_skate, raise_skate_pass, studio_watts,
};
