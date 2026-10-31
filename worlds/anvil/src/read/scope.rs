//! What a Riders Republic world read is told, declared as a reader scope ([`READ_SCOPE`]) with the places and grid
//! selections the crate names ([`PRESETS`]). Nothing here is applied for a caller: a caller states every required
//! choice, and a preset is a name for choices the caller then makes.

use primase_core::pass::OptionSpec;
use primase_core::read_scope::{ReadPreset, ReadScopeSpec};

/// The options of a section read of a world.
pub const READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "anvil",
    options: &[
        OptionSpec::required(
            "world",
            "usnat",
            "the world: usnat (the open world) or whiteroom (the small test level)",
        ),
        OptionSpec::one_of(
            "place",
            "rect",
            "-3584,1152,-3456,1280",
            "x0,y0,x1,y1 in the grid's units (metres, x east and y north): the cells whose squares overlap it are read",
        ),
        OptionSpec::one_of(
            "place",
            "cells",
            "20;27;28;35;36",
            "cell numbers (the GridPartition's record index) of the world's one grid, joined by ;",
        ),
        OptionSpec::required(
            "grids",
            "ShortRange;GRN_Template_TerrainWorldSmall",
            "the World's grids to read, by name, joined by ;: what to select from is the game's partition tables",
        ),
        OptionSpec::choice::<bool>(
            "terrain",
            "whether the terrain under the section is read (the open world has one)",
        ),
        OptionSpec::required(
            "coarse",
            "overlap",
            "overlap, skip or all: the coarse placements read are those whose extents overlap the rectangle, none, or all",
        ),
        OptionSpec::required(
            "ground",
            "splat",
            "splat (the near ground's surface detail) or tile (a far view preview)",
        ),
        OptionSpec::optional(
            "far_view",
            "1km",
            "none, 1km or 3km: the horizon read beyond the section",
            "none",
        ),
        OptionSpec::optional(
            "events",
            "",
            "DataLayer events switched on, joined by ;",
            "no event is on",
        ),
        OptionSpec::choice::<bool>(
            "every_placement",
            "whether mutually exclusive placements are all stacked instead of honouring event conditions",
        ),
        OptionSpec::choice::<bool>(
            "game_shaders",
            "whether the game's programs are read for inspection and drawing",
        ),
    ],
};

/// The places and grid selections the crate names. Hand-picked: the places are regions chosen for a viewer, not read
/// from the game; the grid selections are the partitions the open world's section reads and the one grid of the test
/// level.
pub const PRESETS: &[ReadPreset] = &[
    ReadPreset {
        name: "cityschool",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-3584,1152,-3456,1280")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "area52",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "2304,-7552,3072,-6784")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "thedeck",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-7680,-5888,-6912,-5248")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "skatebowls",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-7552,-5504,-7168,-5120")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "mammothpark",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-4608,1536,-4352,1792")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "xgames",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-7808,-1536,-6784,-1024")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "eaglepark",
        reader: "anvil",
        choices: &[("world", "usnat"), ("rect", "-5120,-768,-4352,-256")],
        note: "a place of the open world, hand-picked",
    },
    ReadPreset {
        name: "whiteroom",
        reader: "anvil",
        choices: &[("world", "whiteroom"), ("cells", "20;27;28;35;36")],
        note: "the test level's cells 20, 27, 28, 35 and 36 of its one grid, hand-picked",
    },
    ReadPreset {
        name: "usnat-section-grids",
        reader: "anvil",
        choices: &[
            ("world", "usnat"),
            ("grids", "ShortRange;GRN_Template_TerrainWorldSmall"),
            ("terrain", "true"),
        ],
        note: "the open world's short-range placements and its terrain partition, with the terrain read; hand-picked from the world's partitions",
    },
    ReadPreset {
        name: "whiteroom-grids",
        reader: "anvil",
        choices: &[
            ("world", "whiteroom"),
            ("grids", "MAD_WhiteRoom"),
            ("terrain", "false"),
        ],
        note: "the test level's one grid; it holds no terrain",
    },
];

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Every preset states only options the scope declares.
    #[test]
    fn presets_state_declared_options() {
        for preset in PRESETS {
            assert!(
                !preset.options(&READ_SCOPE).unwrap().is_empty(),
                "{}",
                preset.name
            );
        }
    }

    /// The rectangles are well formed, and the places do not repeat a name.
    #[test]
    fn the_places_are_rectangles_with_distinct_names() {
        let mut names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len());
        for preset in PRESETS {
            if let Some((_, rect)) = preset.choices.iter().find(|(k, _)| *k == "rect") {
                let v: Vec<f64> = rect.split(',').map(|n| n.parse().unwrap()).collect();
                assert!(v.len() == 4 && v[0] < v[2] && v[1] < v[3], "{}", preset.name);
            }
        }
    }
}
