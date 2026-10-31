//! What a Riders Republic world read is told, declared as reader scopes ([`READ_SCOPE`] for a section of a world,
//! [`ENTRY_SCOPE`] for one forge entry) with the places and grid selections the crate names ([`PRESETS`]). Nothing
//! here is applied for a caller: a caller states every required choice, and a preset is a name for choices the
//! caller then makes. [`section`] and [`entry`] check a caller's choices against the declarations and return them
//! parsed, so which choices a read needs and what their values mean are stated once, here.

use primase_core::pass::{Choice, OptionSpec, Options};
use primase_core::read_scope::{ReadPreset, ReadScope, ReadScopeSpec};

use super::placements::{InstancedCollision, InstancedLevels};

/// The worlds the forges hold: the open world and the small test level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldName {
    /// The open world.
    Usnat,
    /// The small test level.
    Whiteroom,
}

impl Choice for WorldName {
    const VALUES: &'static [(&'static str, Self)] =
        &[("usnat", WorldName::Usnat), ("whiteroom", WorldName::Whiteroom)];
}

/// What a coarser grid level than 0 adds to a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coarse {
    /// The placements whose extents overlap the rectangle.
    Overlap,
    /// Level 0 only.
    Skip,
    /// Every non-coarsest level's whole cells.
    All,
}

impl Choice for Coarse {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("overlap", Coarse::Overlap),
        ("skip", Coarse::Skip),
        ("all", Coarse::All),
    ];
}

/// The colour the terrain's ground is read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    /// The near ground's surface detail.
    Splat,
    /// The nodes' far-view colour tiles.
    Tile,
}

impl Choice for Ground {
    const VALUES: &'static [(&'static str, Self)] = &[("splat", Ground::Splat), ("tile", Ground::Tile)];
}

/// The horizon read beyond a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FarView {
    /// None.
    None,
    /// The authored LongRange and canopy lists within a kilometre.
    OneKm,
    /// The same within three kilometres.
    ThreeKm,
}

impl Choice for FarView {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("none", FarView::None),
        ("1km", FarView::OneKm),
        ("3km", FarView::ThreeKm),
    ];
}

impl FarView {
    /// How far beyond the section the horizon reads, in metres.
    pub fn radius(self) -> f64 {
        match self {
            FarView::None => 0.0,
            FarView::OneKm => 1000.0,
            FarView::ThreeKm => 3000.0,
        }
    }
}

/// The options of a section read of a world.
pub const READ_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "anvil",
    options: &[
        OptionSpec::choice::<WorldName>(
            "world",
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
            "whether the terrain under the section is read (only the open world has one)",
        ),
        OptionSpec::choice::<Coarse>(
            "coarse",
            "what a coarser grid level than 0 adds: the placements whose extents overlap the rectangle, nothing, or every non-coarsest level's whole cells",
        ),
        OptionSpec::choice::<Ground>(
            "ground",
            "splat (the near ground's surface detail) or tile (a far view preview)",
        ),
        OptionSpec::optional_choice::<FarView>("far_view", "the horizon read beyond the section", "none"),
        OptionSpec::optional(
            "events",
            "",
            "DataLayer events switched on, joined by ;",
            "no event is on",
        ),
        OptionSpec::choice::<InstancedLevels>(
            "instanced_levels",
            "what an instanced visual places at each instance: its slot's level, the selector's levels or every level; \
             whether the game draws the other levels there is not read",
        ),
        OptionSpec::choice::<InstancedCollision>(
            "instanced_collision",
            "what an instanced entity's rigid body places: nothing, or its shape at each instance",
        ),
        OptionSpec::choice::<bool>(
            "grass_rasters",
            "whether each terrain node's grass rasters are read too",
        ),
        OptionSpec::optional(
            "terrain_layer",
            "",
            "the TerrainNodeLayer to read, by entry name",
            "the first in the forges' order (the install holds one)",
        ),
        OptionSpec::optional_choice::<bool>(
            "every_placement",
            "whether mutually exclusive placements are all stacked instead of honouring event conditions",
            "event conditions are honoured",
        ),
        OptionSpec::choice::<bool>(
            "game_shaders",
            "whether the game's programs are read for inspection and drawing",
        ),
    ],
};

/// The options of a read of one forge entry.
pub const ENTRY_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "anvil-entry",
    options: &[
        OptionSpec::choice::<WorldName>(
            "world",
            "the world whose forges hold the entry: usnat (the open world) or whiteroom (the small test level)",
        ),
        OptionSpec::choice::<bool>(
            "game_shaders",
            "whether the game's programs are read for inspection and drawing",
        ),
        OptionSpec::optional_choice::<Ground>(
            "ground",
            "the colour of terrain an entry holds: splat or tile",
            "refused when the entry holds terrain",
        ),
        OptionSpec::optional_choice::<FarView>(
            "far_view",
            "the horizon read beyond an entry preview",
            "none",
        ),
        OptionSpec::optional(
            "events",
            "",
            "DataLayer events switched on, joined by ;",
            "no event is on",
        ),
        OptionSpec::optional_choice::<bool>(
            "every_placement",
            "whether mutually exclusive placements are all stacked instead of honouring event conditions",
            "event conditions are honoured",
        ),
    ],
};

/// The options of a rider read from the game's customization tables.
pub const RIDER_SCOPE: ReadScopeSpec = ReadScopeSpec {
    name: "anvil-rider",
    options: &[OptionSpec::required(
        "gender",
        "male",
        "male or female: the rider tables carry two gender conditions as hashes and nothing names which is which, so \
         no gender is assumed",
    )],
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
        name: "preview-instances",
        reader: "anvil",
        choices: &[
            ("instanced_levels", "every"),
            ("instanced_collision", "per-instance"),
            ("grass_rasters", "false"),
        ],
        note: "every mesh level at each instance (the instances' stored bounds cover them: the placement census), the collision shape at each instance, and no grass rasters; a previewing pick",
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

/// The place a preset names: the world it is in, and the rectangle or the numbered cells it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// The world the place is in.
    pub world: WorldName,
    /// The cells' rectangle, `[x0, y0, x1, y1]`.
    pub rect: Option<[f64; 4]>,
    /// The numbered cells of the world's one grid.
    pub cells: Vec<usize>,
}

/// The presets that name a place (a rectangle or cells).
pub fn place_presets() -> impl Iterator<Item = &'static ReadPreset> {
    PRESETS
        .iter()
        .filter(|p| p.choices.iter().any(|(key, _)| matches!(*key, "rect" | "cells")))
}

/// `x0,y0,x1,y1` as a rectangle: four finite numbers with `x0 < x1` and `y0 < y1`.
pub fn parse_rect(text: &str) -> Result<[f64; 4], String> {
    let numbers = text
        .split(',')
        .map(|n| n.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("rect {text:?}: {error}"))?;
    let rect = <[f64; 4]>::try_from(numbers).map_err(|_| format!("rect {text:?}: expected x0,y0,x1,y1"))?;
    let [x0, y0, x1, y1] = rect;
    if !rect.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
        return Err(format!(
            "rect {text:?}: expected x0,y0,x1,y1 with x0 < x1 and y0 < y1"
        ));
    }
    Ok(rect)
}

/// `20;27;28` as cell numbers.
pub fn parse_cells(text: &str) -> Result<Vec<usize>, String> {
    text.split(';')
        .map(|n| n.trim().parse::<usize>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cells {text:?}: {error}"))
}

/// The place `preset` names.
pub fn place_of(preset: &ReadPreset) -> Result<Place, String> {
    let stated = preset.options(&READ_SCOPE)?;
    let world = stated
        .get("world")
        .and_then(|w| choice_of::<WorldName>(w))
        .ok_or_else(|| format!("area {}: names no world", preset.name))?;
    let rect = stated.get("rect").map(|t| parse_rect(t)).transpose()?;
    let cells = stated
        .get("cells")
        .map(|t| parse_cells(t))
        .transpose()?
        .unwrap_or_default();
    if rect.is_none() && cells.is_empty() {
        return Err(format!("area {}: names no rectangle or cells", preset.name));
    }
    Ok(Place { world, rect, cells })
}

/// The place the preset `name` names; an unknown name is refused, listing the places.
pub fn place_named(name: &str) -> Result<Place, String> {
    let preset = place_presets().find(|p| p.name == name).ok_or_else(|| {
        format!(
            "area {name:?}: expected one of {}",
            place_presets().map(|p| p.name).collect::<Vec<_>>().join(", ")
        )
    })?;
    place_of(preset)
}

fn choice_of<T: Choice>(text: &str) -> Option<T> {
    T::VALUES.iter().find(|(t, _)| *t == text).map(|(_, v)| *v)
}

fn stated<T: Choice>(options: &Options, key: &str) -> Result<T, String> {
    options
        .get(key)
        .and_then(|text| choice_of::<T>(text))
        .ok_or_else(|| format!("{key} is none of the values it takes"))
}

fn list(options: &Options, key: &str) -> Vec<String> {
    options
        .get(key)
        .map(|text| {
            text.split(';')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The world a read is in and the place an `area` names: the world is the caller's, or the area's; the two must agree.
pub fn locate(area: Option<&str>, world: Option<&str>) -> Result<(WorldName, Option<Place>), String> {
    let place = area.map(place_named).transpose()?;
    let stated_world = world
        .map(|w| choice_of::<WorldName>(w).ok_or_else(|| format!("world {w:?}: expected usnat or whiteroom")))
        .transpose()?;
    let world = match (stated_world, &place) {
        (Some(world), _) => world,
        (None, Some(place)) => place.world,
        (None, None) => return Err("world is required (or an area, which names it)".to_owned()),
    };
    if let (Some(name), Some(place)) = (area, &place)
        && place.world != world
    {
        return Err(format!(
            "area {name} is in the {} world, not {}",
            place.world.text(),
            world.text()
        ));
    }
    Ok((world, place))
}

/// A section read's choices, checked against [`READ_SCOPE`] and parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// The world read.
    pub world: WorldName,
    /// The cells' rectangle, if the place is one.
    pub rect: Option<[f64; 4]>,
    /// The numbered cells, if the place is those.
    pub cells: Vec<usize>,
    /// The grids read, by name.
    pub grids: Vec<String>,
    /// Whether the terrain is read: stated, and the world has one (only the open world does).
    pub terrain: bool,
    /// What coarser levels add.
    pub coarse: Coarse,
    /// The terrain's ground colour.
    pub ground: Ground,
    /// The horizon beyond the section.
    pub far_view: FarView,
    /// The DataLayer events switched on.
    pub events: Vec<String>,
    /// Whether every placement is stacked whatever its event conditions.
    pub every_placement: bool,
    /// Whether the game's programs are read.
    pub game_shaders: bool,
    /// What an instanced visual places at each instance.
    pub instanced_levels: InstancedLevels,
    /// What an instanced entity's body places.
    pub instanced_collision: InstancedCollision,
    /// Whether the terrain's grass rasters are read.
    pub grass_rasters: bool,
    /// The TerrainNodeLayer by entry name, if one is named.
    pub terrain_layer: Option<String>,
}

/// The section a caller's `choices` state, with the place of the preset `area` (if any) under them: the caller's
/// `rect` or `cells` replaces the area's place, and the world the area is in must be the one stated. Checked against
/// [`READ_SCOPE`]: a choice it requires and the caller left out is refused, naming it. Returns the scope too.
pub fn section(area: Option<&str>, choices: Options) -> Result<(Section, ReadScope), String> {
    let (world, _) = locate(area, choices.get("world").map(String::as_str))?;
    let mut all = Options::new();
    all.insert("world".to_owned(), world.text().to_owned());
    if let Some(name) = area
        && !choices.contains_key("rect")
        && !choices.contains_key("cells")
        && let Some(preset) = place_presets().find(|p| p.name == name)
    {
        all.extend(
            preset
                .options(&READ_SCOPE)?
                .into_iter()
                .filter(|(k, _)| k != "world"),
        );
    }
    all.extend(choices);
    let scope = READ_SCOPE.resolve(all)?;
    let c = &scope.choices;
    let rect = c.get("rect").map(|t| parse_rect(t)).transpose()?;
    let cells = c
        .get("cells")
        .map(|t| parse_cells(t))
        .transpose()?
        .unwrap_or_default();
    let section = Section {
        world,
        rect,
        cells,
        grids: list(c, "grids"),
        terrain: stated::<bool>(c, "terrain")? && world == WorldName::Usnat,
        coarse: stated(c, "coarse")?,
        ground: stated(c, "ground")?,
        far_view: if c.contains_key("far_view") {
            stated(c, "far_view")?
        } else {
            FarView::None
        },
        events: list(c, "events"),
        every_placement: if c.contains_key("every_placement") {
            stated(c, "every_placement")?
        } else {
            false
        },
        game_shaders: stated(c, "game_shaders")?,
        instanced_levels: stated(c, "instanced_levels")?,
        instanced_collision: stated(c, "instanced_collision")?,
        grass_rasters: stated(c, "grass_rasters")?,
        terrain_layer: c.get("terrain_layer").filter(|l| !l.is_empty()).cloned(),
    };
    if section.grids.is_empty() {
        return Err("grids is required: name at least one grid".to_owned());
    }
    Ok((section, scope))
}

/// One forge entry read's choices, checked against [`ENTRY_SCOPE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The world whose forges hold the entry.
    pub world: WorldName,
    /// Whether the game's programs are read.
    pub game_shaders: bool,
    /// The colour of terrain the entry holds, if stated.
    pub ground: Option<Ground>,
    /// The horizon beyond the preview.
    pub far_view: FarView,
    /// The DataLayer events switched on.
    pub events: Vec<String>,
    /// Whether every placement is stacked whatever its event conditions.
    pub every_placement: bool,
}

/// The entry read `choices` state. A choice [`ENTRY_SCOPE`] requires and the caller left out is refused, naming it.
pub fn entry(choices: Options) -> Result<Entry, String> {
    let scope = ENTRY_SCOPE.resolve(choices)?;
    let c = &scope.choices;
    Ok(Entry {
        world: stated(c, "world")?,
        game_shaders: stated(c, "game_shaders")?,
        ground: if c.contains_key("ground") {
            Some(stated(c, "ground")?)
        } else {
            None
        },
        far_view: if c.contains_key("far_view") {
            stated(c, "far_view")?
        } else {
            FarView::None
        },
        events: list(c, "events"),
        every_placement: if c.contains_key("every_placement") {
            stated(c, "every_placement")?
        } else {
            false
        },
    })
}

/// The ground colour terrain is lowered with: the stated one, or a refusal when none is (a read that holds terrain
/// needs it).
pub fn ground_for_terrain(ground: Option<Ground>) -> Result<Ground, String> {
    ground
        .ok_or_else(|| "ground is required: the read holds terrain, whose colour is splat or tile".to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeSet;

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

    /// The rider's gender is stated, never filled.
    #[test]
    fn the_riders_gender_is_required() {
        assert!(RIDER_SCOPE.resolve(Default::default()).is_err());
    }

    /// The places do not repeat a name, and each place preset resolves to a world and a rectangle or cells.
    #[test]
    fn the_places_resolve_with_distinct_names() {
        let mut names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PRESETS.len());
        let mut places = BTreeSet::new();
        for preset in place_presets() {
            let place = place_of(preset).unwrap();
            assert!(place.rect.is_some() != !place.cells.is_empty(), "{}", preset.name);
            places.insert(preset.name);
        }
        assert_eq!(places.len(), 8);
        assert!(place_named("nowhere").unwrap_err().contains("cityschool"));
    }

    fn stated_section() -> Options {
        [
            ("grids", "ShortRange;GRN_Template_TerrainWorldSmall"),
            ("terrain", "true"),
            ("coarse", "overlap"),
            ("ground", "splat"),
            ("instanced_levels", "every"),
            ("instanced_collision", "per-instance"),
            ("grass_rasters", "false"),
            ("game_shaders", "true"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    /// A section needs every choice the scope requires, refused by option name; the area gives the world and place.
    #[test]
    fn a_section_states_what_the_scope_requires() {
        let (section, scope) = section(Some("cityschool"), stated_section()).unwrap();
        assert_eq!(section.world, WorldName::Usnat);
        assert_eq!(section.rect, Some([-3584.0, 1152.0, -3456.0, 1280.0]));
        assert!(section.terrain && section.far_view == FarView::None && !section.every_placement);
        assert_eq!(scope.reader, "anvil");
        for key in [
            "grids",
            "terrain",
            "coarse",
            "ground",
            "game_shaders",
            "instanced_levels",
            "instanced_collision",
            "grass_rasters",
        ] {
            let mut missing = stated_section();
            missing.remove(key);
            let refused = super::section(Some("cityschool"), missing).unwrap_err();
            assert!(refused.contains(key), "{key}: {refused}");
        }
        assert!(super::section(None, stated_section()).is_err());
    }

    /// The caller's rect replaces the area's place; the area's world must be the stated one; the test level has no terrain.
    #[test]
    fn the_rect_replaces_the_areas_place_and_worlds_must_agree() {
        let mut choices = stated_section();
        choices.insert("rect".into(), "0,0,10,10".into());
        let (section, _) = super::section(Some("cityschool"), choices).unwrap();
        assert_eq!(section.rect, Some([0.0, 0.0, 10.0, 10.0]));
        let mut choices = stated_section();
        choices.insert("world".into(), "whiteroom".into());
        assert!(
            super::section(Some("cityschool"), choices)
                .unwrap_err()
                .contains("whiteroom")
        );
        let mut choices = stated_section();
        choices.insert("world".into(), "whiteroom".into());
        let (room, _) = super::section(Some("whiteroom"), choices).unwrap();
        assert!(!room.terrain && room.cells == [20, 27, 28, 35, 36]);
        let mut choices = stated_section();
        choices.insert("rect".into(), "10,0,0,10".into());
        assert!(super::section(Some("cityschool"), choices).is_err());
    }

    /// An entry states its world and shaders; terrain's ground is stated or refused.
    #[test]
    fn an_entry_states_its_world_and_shaders() {
        let choices: Options = [("world", "usnat"), ("game_shaders", "true")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let entry = entry(choices.clone()).unwrap();
        assert_eq!(entry.ground, None);
        assert!(ground_for_terrain(entry.ground).unwrap_err().contains("ground"));
        let mut missing = choices;
        missing.remove("game_shaders");
        assert!(super::entry(missing).unwrap_err().contains("game_shaders"));
    }
}
