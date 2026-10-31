//! What a SHRED disc read is told, declared as a reader scope ([`READ_SCOPE`]) with the combinations the crate offers
//! by name ([`PRESETS`]). Nothing here is applied for a caller: a caller states every required choice, and a preset is
//! a name for choices the caller then makes.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: lists the layer archives of the disc folder the caller names"
)]

use std::path::Path;

use primase_core::pass::{OptionSpec, Options};
use primase_core::read_scope::{ReadPreset, ReadScope, ReadScopeSpec};

use super::disc::layer_listings;
use crate::{Result, invalid};

/// The options of a disc read: the base level, the layer archives joined to it, and the frame and unit the numbers are
/// converted through.
pub const READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "shred",
    options: &[
        OptionSpec::required(
            "level",
            "charlie",
            "a base level of the disc, lower case, as the disc's levels table lists it",
        ),
        OptionSpec::optional(
            "layers",
            "00000004.pak",
            "layer archives (lower-case file names, joined by ;) whose quartets of the level are read with it",
            "the base level's archive alone",
        ),
        OptionSpec::required(
            "frame",
            "xy-z",
            "the axes the source's numbers are converted through, xyz or xy-z: SHRED's native view and projection matrices \
             and five CHARLIE camera poses register with the source Z reflected (xy-z); physical units and north are not \
             established",
        ),
        OptionSpec::required(
            "scale",
            "1",
            "metres per source unit: the units are not established, so no value is assumed",
        ),
    ],
};

/// The combinations of choices the crate names. Hand-picked unless a note says observed.
pub const PRESETS: &[ReadPreset] = &[
    ReadPreset {
        name: "charlie-training",
        reader: "shred",
        choices: &[("level", "charlie"), ("layers", "00000004.pak")],
        note: "CHARLIE's training hangar and its standard and session geometry are in 00000004.PAK; the base level's archive alone omits them (observed in the archive's quartets). Which layers to read is the caller's choice, this one is a pick",
    },
    ReadPreset {
        name: "native-camera-frame",
        reader: "shred",
        choices: &[("frame", "xy-z")],
        note: "SHRED's native view and projection matrices and five CHARLIE camera poses register with the source Z reflected under xy-z (observed); physical units and north remain unproven, so no scale is part of the preset",
    },
];

/// The scope of a disc read with the caller's `choices` (every option [`READ_SCOPE`] requires, checked): the layer
/// archives of the chosen level that the choices leave unloaded are recorded with it, by file name, without opening
/// more than their listing.
pub fn read_scope(folder: &Path, choices: Options) -> Result<ReadScope> {
    let mut scope = READ_SCOPE
        .resolve(choices)
        .map_err(|why| invalid(0, format!("shred: {why}")))?;
    let level = scope.choices.get("level").cloned().unwrap_or_default();
    let chosen: Vec<String> = scope
        .choices
        .get("layers")
        .map(|layers| {
            layers
                .split(';')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for listing in layer_listings(folder)? {
        if listing.level == level && !chosen.contains(&listing.file) {
            READ_SCOPE
                .record_unloaded(
                    &mut scope,
                    "layers",
                    &listing.file,
                    format!("levels/layers/{}", listing.file),
                )
                .map_err(|why| invalid(0, format!("shred: {why}")))?;
        }
    }
    Ok(scope)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Every preset states only options the scope declares, with the scope's own name.
    #[test]
    fn presets_state_declared_options() {
        for preset in PRESETS {
            let options = preset.options(&READ_SCOPE).unwrap();
            assert!(!options.is_empty(), "{}", preset.name);
        }
    }

    /// A required choice that is not stated is refused, never filled; the optional layers stay absent.
    #[test]
    fn required_choices_are_never_filled() {
        assert!(READ_SCOPE.resolve(Options::new()).is_err());
        let all = Options::from([
            ("level".to_owned(), "charlie".to_owned()),
            ("frame".to_owned(), "xy-z".to_owned()),
            ("scale".to_owned(), "1".to_owned()),
        ]);
        let scope = READ_SCOPE.resolve(all).unwrap();
        assert!(!scope.choices.contains_key("layers"));
    }
}
