//! SkateBIRD read over the Unity reader (`primase-unity`): what the game's own scripts state that Unity's meanings
//! don't. Its reset points (`ResetPosition`, with the one the player starts at) and its mission givers
//! (`MissionGiver`), each as an entity with its world transform, its state (the Unity level read's own `unity.active`
//! and `unity.enabled` axes) and its `skatebird.*` component ([`read::read_markers`]).
//!
//! The reader states facts only: which point a program spawns at, which givers become travel points and what they
//! are called are the program's choices, made in its own passes before it raises the world for a destination.
//!
//! Its own shader, `Bumped Specular Voronoi`, is baked from its compiled program ([`passes::voronoi`], pass
//! `lower-skatebird-voronoi`, after `lower-unity-pbr`), the programs read after the level by
//! [`read::read_voronoi_programs`].
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

pub use read::{MarkersReport, read_markers, read_voronoi_programs};

/// Registers SkateBIRD's passes by name: `lower-skatebird-voronoi`, which takes no option.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register("lower-skatebird-voronoi", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(passes::voronoi::LowerSkatebirdVoronoi)),
    });
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Unity(#[from] primase_unity::Error),
    /// A script's stored fields are not a layout this crate knows for the player's Unity version.
    #[error("{0}")]
    Layout(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
