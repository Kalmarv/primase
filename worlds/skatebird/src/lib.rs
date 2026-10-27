//! SkateBIRD read over the Unity reader (`primase-unity`): what the game's own scripts state that Unity's meanings
//! don't. Its reset points (`ResetPosition`, with the one the player starts at) and its mission givers
//! (`MissionGiver`), each as an entity with its world transform, its state (the Unity level read's own `unity.active`
//! and `unity.enabled` axes) and its `skatebird.*` component ([`read::read_markers`]).
//!
//! What lies under a mission's collectibles, random-pick groups, mission givers and birds' dressing, and the grind rails
//! as curves, come from [`read::read_scripted`]; `omit-skatebird-mission-props` ([`passes::mission`]) leaves the
//! mission props out.
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

pub use read::{
    MarkersReport, ScriptedReport, SkatebirdReader, read_markers, read_scripted, read_voronoi_programs,
};

/// Registers SkateBIRD's passes by their declarations: `lower-skatebird-voronoi`, whose options (`keywords`, `tier`,
/// `cells`, `max-texels`) are the program's choices, without defaults; `omit-skatebird-mission-props`, whose `roles`
/// are too; `place-skatebird-markers`, every option stated.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    for spec in [
        &passes::PLACE_SKATEBIRD_MARKERS,
        &passes::OMIT_SKATEBIRD_MISSION_PROPS,
        &passes::LOWER_SKATEBIRD_VORONOI,
    ] {
        reg.add(spec);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Unity(#[from] unity_formats::Error),
    /// A script's stored fields are not a layout this crate knows for the player's Unity version.
    #[error("{0}")]
    Layout(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use primase_core::pass::{Options, PassRegistry};

    /// The crate registers exactly these passes by their declarations, each made only with its options and saying
    /// what it takes.
    #[test]
    fn the_skatebird_passes_register_by_name() {
        let mut reg = PassRegistry::new();
        super::register(&mut reg);
        let names = [
            "lower-skatebird-voronoi",
            "omit-skatebird-mission-props",
            "place-skatebird-markers",
        ];
        assert_eq!(format!("{reg:?}"), format!("{names:?}"));
        for name in names {
            assert!(reg.requires_options(name), "{name}");
            assert!(reg.describe(name).is_some_and(|d| d.starts_with(name)), "{name}");
            assert!(reg.make(name, &Options::new()).is_err(), "{name}");
        }
    }
}
