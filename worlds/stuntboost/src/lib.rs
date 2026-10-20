//! STUNTBOOST read over the MonoGame reader (`primase-monogame`): a level's parts (level, room, sky), every node with
//! its name parsed by the game's Blender exporter grammar, the meshes, materials and baked atlases, as stored and in
//! the game's units ([`read::read_level`]); then lowerings whose every choice is the program's ([`passes`]): collision
//! with surfaces from the physics materials, the spawns as markers, the materials with the baked lighting as base
//! colour or emission, and what the game never draws or a static map has no use for (triggers, editor nodes) left out.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

pub use read::{LevelReport, read_level};

/// Registers STUNTBOOST's passes by name; every option of each is the program's, without defaults.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register_requiring(
        "lower-stuntboost-collision",
        |o| Ok(Box::new(passes::LowerStuntboostCollision::from_options(o)?)),
        &[
            ("plain", "wood"),
            ("rails", "metal"),
            ("kill", "leave-out"),
            ("unassigned", "C_Default"),
        ],
    );
    reg.register_requiring(
        "lower-stuntboost-spawns",
        |o| Ok(Box::new(passes::LowerStuntboostSpawns::from_options(o)?)),
        &[("start", "start"), ("others", "travel")],
    );
    reg.register_requiring(
        "lower-stuntboost-materials",
        |o| Ok(Box::new(passes::LowerStuntboostMaterials::from_options(o)?)),
        &[("baked", "base-colour"), ("transparent", "blend")],
    );
    reg.register_requiring(
        "omit-stuntboost-hidden",
        |o| Ok(Box::new(passes::OmitStuntboostHidden::from_options(o)?)),
        &[
            ("invisible", "leave-out"),
            ("shadow-only", "leave-out"),
            ("triggers", "leave-out"),
            ("classes", "PreviewCamera+Sun+SetBakedLighting"),
        ],
    );
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Monogame(#[from] primase_monogame::xnb::Error),
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    #[error(transparent)]
    Image(#[from] primase_image::Error),
    /// The level's files are not what the game ships.
    #[error("{0}")]
    Format(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
