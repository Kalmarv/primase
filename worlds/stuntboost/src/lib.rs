//! STUNTBOOST read over the MonoGame reader (`monogame-formats`): a level's parts (level, room, sky), every node with
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
pub use read::{
    LevelEntry, LevelReport, LevelSpawnEntry, level_spawns, levels, model_gltf, model_image, models,
    read_level,
};

/// Registers STUNTBOOST's passes by their declarations; every option of each is the program's, without defaults.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &passes::LOWER_STUNTBOOST_COLLISION,
        &passes::LOWER_STUNTBOOST_SPAWNS,
        &passes::LOWER_STUNTBOOST_MATERIALS,
        &passes::OMIT_STUNTBOOST_HIDDEN,
    ] {
        reg.add(spec);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Monogame(#[from] monogame_formats::xnb::Error),
    #[error(transparent)]
    Glb(#[from] primase_glb::Error),
    #[error(transparent)]
    Image(#[from] primase_image::Error),
    /// The level's files are not what the game ships.
    #[error("{0}")]
    Format(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use primase_core::pass::{Options, PassRegistry};

    /// The crate registers exactly these passes by their declarations, each made only with its options and saying
    /// what it takes.
    #[test]
    fn the_stuntboost_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "lower-stuntboost-collision",
            "lower-stuntboost-materials",
            "lower-stuntboost-spawns",
            "omit-stuntboost-hidden",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        for name in names {
            assert!(reg.requires_options(name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert!(reg.make(name, &Options::new()).is_err(), "{name}");
        }
    }
}
