//! STUNTBOOST read over the MonoGame reader (`primase-monogame`): a level's parts (level, room, sky), every node with
//! its name parsed by the game's own rules, the meshes, materials and baked atlases, as stored, in the
//! core frame in metres at a scale the program states ([`read::read_level`], [Frame](crate#frame)); then lowerings
//! whose every choice is the program's ([`passes`]): collision with surfaces from the physics materials, the spawns
//! as markers, the materials with the baked lighting as base colour, emission or unlit (as the game draws a baked
//! level), and what the game never draws or a static map has no use for (triggers, editor nodes) left out.
//!
//! # Frame
//!
//! glTF's axes as the Blender exporter writes them are the core frame's (+Y up, right-handed), so a level
//! converts by its unit alone, and a STUNTBOOST unit has no one true length. Its props are modelled at real size in
//! centimetres (an AA battery 5.15 units, a door 204), while its fingerboard's deck is 0.967 units long. So the scale
//! is the program's, a required argument of [`read::read_level`]: `0.01` reads the props at their real size (the
//! board then 1 cm long, a fingerboard); about `0.83` reads the board as a 0.80 m skateboard deck (Caleb's choice for
//! skate., the room giant around it). Positions, translations and everything derived from them (collision) are in
//! metres; the application-specific vertex attributes stay as stored.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod components;
pub mod passes;
pub mod read;

pub use passes::{Spawn, Start, spawns};
pub use read::{LevelEntry, LevelReport, ModelReport, levels, models, read_level, read_model};

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
    Gltf(#[from] primase_gltf::Error),
    #[error(transparent)]
    Image(#[from] primase_image::Error),
    /// The level's files are not what the game ships.
    #[error("{0}")]
    Format(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
