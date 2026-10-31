//! What a Unity player read is told, declared as a reader scope ([`READ_SCOPE`]). Nothing here is applied for a
//! caller: a caller states the one thing it reads, and a quality level where the player's levels differ.

use primase_core::pass::{OptionSpec, Options};
use primase_core::read_scope::{ReadScope, ReadScopeSpec};
use unity_formats::player::Player;

use crate::invalid;

/// The options of a read of a player: the scene, object or character read, and the quality level whose render
/// pipeline asset and shadow settings a scene is read with.
pub const READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "unity",
    options: &[
        OptionSpec::one_of(
            "target",
            "level",
            "level1",
            "the scene file to read, as the build's scene list names it (`level<index>`)",
        ),
        OptionSpec::one_of(
            "target",
            "asset",
            "resources.assets/1234",
            "one object of the player to read alone, as <file>/<path id>",
        ),
        OptionSpec::one_of(
            "target",
            "character",
            "level2/5678",
            "the Animator, as <file>/<path id>, whose character is read instead of a scene",
        ),
        OptionSpec::optional(
            "quality",
            "Ultra",
            "the name of the player's quality level whose render pipeline asset and shadow settings the scene is read with; \
             the current level is a run-time setting, not data",
            "the setting every quality level of the player agrees on; refused where they differ",
        ),
    ],
};

/// The scope of a read of `player` with the caller's `choices` (every option [`READ_SCOPE`] requires, checked): the
/// scene files of the build that a level read leaves unread, and the quality levels it does not take, each recorded
/// with where the player lists it. A quality level the player does not list is refused.
pub fn read_scope(player: &Player, choices: Options) -> unity_formats::Result<ReadScope> {
    let mut scope = READ_SCOPE
        .resolve(choices)
        .map_err(|why| invalid(format!("unity: {why}")))?;
    if let Some(level) = scope.choices.get("level").cloned() {
        for scene in player.scenes()? {
            if scene.file != level {
                READ_SCOPE
                    .record_unloaded(&mut scope, "level", &scene.file, scene.path.clone())
                    .map_err(|why| invalid(format!("unity: {why}")))?;
            }
        }
    }
    if let Some(quality) = scope.choices.get("quality").cloned() {
        let levels = player.quality_levels()?;
        if !levels.iter().any(|l| l.name == quality) {
            return Err(invalid(format!(
                "unity: quality level {quality:?} is not one of the player's: {}",
                levels
                    .iter()
                    .map(|l| format!("{:?}", l.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        for other in levels.iter().filter(|l| l.name != quality) {
            READ_SCOPE
                .record_unloaded(
                    &mut scope,
                    "quality",
                    &other.name,
                    format!("QualitySettings/{}", other.name),
                )
                .map_err(|why| invalid(format!("unity: {why}")))?;
        }
    }
    Ok(scope)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A target is required (one of three), and the quality stays absent unless stated.
    #[test]
    fn a_target_is_stated_never_filled() {
        assert!(READ_SCOPE.resolve(Options::new()).is_err());
        let level = Options::from([("level".to_owned(), "level1".to_owned())]);
        let scope = READ_SCOPE.resolve(level).unwrap();
        assert!(!scope.choices.contains_key("quality"));
        let both = Options::from([
            ("level".to_owned(), "level1".to_owned()),
            ("asset".to_owned(), "a/1".to_owned()),
        ]);
        assert!(READ_SCOPE.resolve(both).is_err());
    }
}
