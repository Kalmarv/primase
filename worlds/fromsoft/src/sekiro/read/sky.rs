//! The sky of an area, read with it when asked ([`Sky::Include`]).
//!
//! # Sekiro
//!
//! The sky map `m89_00_00_99`, whose layout places at the origin one dome per sky state (`m9000xx`), each dome's ring
//! of far mountains (`m9001xx`, `m900200`) and the moon (the object `o005900`), and the area's own domes that its layout
//! does not place (`m10_00`, `m11_01` and `m17_00` keep some).
//!
//! What the game does with them, from its code (the run-time image of Sekiro 1.06): it loads every model of the sky map
//! and the moon whatever the area; every frame its in-game step gives each enabled part of the sky map the world matrix
//! "translation (camera x, 0, camera z) times the part's own transform", then places the moon 25 units from the camera
//! towards the light (25.0 is what the step's constructor stores; a debug value, `Moon Distance`, can change it). So a
//! dome and a ring are [`primase_core::SkyAnchor::Horizontal`] and the moon is `TowardsLight { the sun, 25 }`, which
//! `lower-fromsoft-sky` attaches once the environment has a sun.
//!
//! Which state the game draws is not read: it turns on one dome (a recorded frame at 18:00 drew `M[Sky]_SkyDome_Evening`,
//! field for field) by a rule the reader does not know yet. So the states are the values of one global state axis,
//! `fromsoft.sky`, with no default: `resolve-state` leaves it unresolved for a consumer that switches it, and a program
//! picks one by `assign=fromsoft.sky:<value>`. A value is named `<model>-<state>`, the state the word after `SkyDome_`
//! in the dome's material (`m900020-Evening`), and an area's own dome `<area>-<model>-<state>` (`m11_01-m900000-Morning`).
//! A ring is drawn in its dome's state: its entity id is the dome's plus one, else its model number the dome's plus 100
//! (`m900175`, entity 8904176, is the ring of `m900075`, entity 8904075). The moon has no condition: it is drawn in the
//! recorded state, and no other state has been observed.
//!
//! # Elden Ring
//!
//! An area's sky stands in a sky map of its own ([`elden_sky_map`]): `m60_00_00_99` for an open-world tile, whose
//! frame is centred on the level-2 tile `m60_10_08_02` (its far-view pieces `m90XXZZ`, models centred on their 1024 m
//! cell, stand at `((XX - 10) * 1024, 0, (ZZ - 8) * 1024)`); `mAA_BB_00_99` in the legacy map's own frame for a legacy
//! map `mAA_BB_..` (the param's definition calls them the open and legacy maps' sky-placement maps). The weather
//! decides which of the sky map's assets show: `WeatherAssetReplaceParam` gives, for the sky map, a weather, a
//! fire-ash state (before or after the map burns) and the creation limits of the maps it applies on, the assets shown
//! (sky spheres `AEG096_010`.. 53 km and `AEG096_101`/`_102` 86 km across, cloud layers, the burning Erdtree's
//! pieces). An open-world tile's limit is its `MapDefaultInfoParam` row's (`m60_42_36_00`: 0); a legacy map sets none
//! and its rows set none.
//!
//! So each (weather, fire-ash state) a row of the sky map gives for the area is a value of the `fromsoft.sky` axis,
//! `weather-<WeatherParam id>` or `weather-<id>-ashen`, with no default (which weather the game draws is drawn by lot,
//! `WeatherLotParam`, by a rule not read), and each asset of the sky map that some value shows is read with the values
//! that show it as its condition. An asset the area's rows name for no value (another region's sky) is not read; the
//! sky map's parts that no row of it names (its far view of the land, the Erdtree) are not read either, counted.
//!
//! Where the game draws them, from a recorded frame of Stormveil (`m10_00_00_99`'s assets, the camera at three places a
//! few metres apart): the cloud layer `AEG096_603`, drawn with `M[Clouds9]`, has the world matrix "translation
//! (camera x, the sky map origin's height, camera z)", while the light shaft `AEG096_611` (`M[WorldTree_LightShaft]`)
//! stays at the sky map's origin. So an asset whose materials are all of the sky class
//! ([`crate::sekiro::source::SkyRole::WeatherSky`]) is read with its position relative to the sky map's origin and
//! `core.sky_anchor` horizontal (the area's offset into the sky map is the camera's to supply); any other
//! ([`crate::sekiro::source::SkyRole::Weather`]) is placed where the layout places it, moved into the area's frame.
//! The spheres themselves are not yet seen drawn: that they follow as the cloud layer does is inferred from the
//! shared program family.
use crate::sekiro::source::{SkyPart, SkyRole};
use fromsoft_formats::msb::{MapLayout, Part, PartKind};
use fromsoft_formats::weather_assets::{WeatherAssetParam, asset_model_name};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Entity, Name, SkyAnchor, World};
use std::collections::{BTreeMap, BTreeSet};

/// Sekiro's sky map.
pub const SKY_MAP: &str = "m89_00_00_99";
/// The moon's object model.
pub const MOON: &str = "o005900";
/// The state axis of the sky.
pub const AXIS: &str = "fromsoft.sky";
/// Elden Ring's open-world sky map, and where its frame's origin lies in metres from the corner of the tile grid (the
/// centre of the level-2 tile `m60_10_08_02`).
pub const OPEN_WORLD_SKY_MAP: &str = "m60_00_00_99";
const OPEN_WORLD_SKY_ORIGIN: [f64; 2] = [10.0 * 1024.0 + 512.0, 8.0 * 1024.0 + 512.0];
/// The edge of an open-world tile of level 0, in metres.
const TILE: f64 = 256.0;

/// Whether an area's read brings in the sky.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sky {
    /// Not read (today's output for every target).
    #[default]
    None,
    /// The sky, as described in the module documentation.
    Include,
}

/// What a sky part is, by its model: a dome `m900000..=m900099`, a ring `m900100..=m900299`, the moon object; `None`
/// for any other model of the sky map.
pub fn role(model: &str, object: bool) -> Option<SkyRole> {
    if object {
        return (model == MOON).then_some(SkyRole::Moon);
    }
    match model.strip_prefix('m')?.parse::<u32>().ok()? {
        900_000..=900_099 => Some(SkyRole::Dome),
        900_100..=900_299 => Some(SkyRole::Ring),
        _ => None,
    }
}

/// The state a dome's materials name: the word after `SkyDome_` in the first material definition (MTD) file name
/// holding it, its case kept (`M[Sky]_SkyDome_Evening.mtd` -> `Evening`, `M[m10_00]_SkyDome_Night1.mtd` -> `Night1`),
/// else `Dome`.
pub fn state_word<'a>(mtds: impl IntoIterator<Item = &'a str>) -> String {
    mtds.into_iter()
        .map(|path| {
            let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
            leaf.rsplit_once('.').map_or(leaf, |x| x.0)
        })
        .find_map(|stem| stem.split_once("SkyDome_").map(|(_, w)| w.to_owned()))
        .filter(|w| !w.is_empty() && !w.contains([':', ';']))
        .unwrap_or_else(|| "Dome".to_owned())
}

/// The area's own dome models its layout does not place: `m9000NN` for NN in 00, 05, ... 95 whose binder `exists`
/// (given the model's digits, `9000NN`).
pub fn area_domes(layout: &MapLayout, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let placed: BTreeSet<&str> = layout
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::MapPiece)
        .filter_map(|p| p.model.map(|i| layout.models[i].name.as_str()))
        .collect();
    (0..100)
        .step_by(5)
        .map(|n| format!("9000{n:02}"))
        .filter(|digits| exists(digits))
        .map(|digits| format!("m{digits}"))
        .filter(|model| !placed.contains(model.as_str()))
        .collect()
}

/// A part for an area dome no layout places: at the origin, unrotated, of unit scale, with no entity id and the
/// source's default controls; named after its model.
pub fn unplaced_part(model: &str) -> Part {
    Part {
        index: usize::MAX,
        name: model.to_owned(),
        kind: PartKind::MapPiece,
        model: None,
        hit_filter: None,
        npc_param: None,
        sfx_param_relative_id: None,
        position: [0.; 3],
        rotation_degrees: [0.; 3],
        scale: [1.; 3],
        entity_id: -1,
        entity_group_ids: [-1; 8],
        shadows: Default::default(),
        draw: Default::default(),
        tile_load: None,
    }
}

/// The map `mAA_BB_CC_DD` as the eight-digit number the params key maps by (`m60_42_36_00` -> 60423600).
pub fn map_number(map: &str) -> Option<i32> {
    let digits: String = map.strip_prefix('m')?.split('_').collect();
    (digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Elden Ring: the sky map of the area `area` (a full map name, `m60_42_36_00`) and where the area's frame origin lies
/// in the sky map's frame (source x, z), as the module documentation describes. `Ok(None)` for an area of no known sky
/// map (an open-world tile name of no level 0..2); an error for the later addition's open world (`m61`), whose sky
/// map's frame is not established.
pub fn elden_sky_map(area: &str) -> Result<Option<(String, [f64; 2])>, String> {
    let groups: Vec<&str> = area.strip_prefix('m').unwrap_or_default().split('_').collect();
    let [aa, bb, cc, dd] = groups.as_slice() else {
        return Ok(None);
    };
    match *aa {
        "61" => Err(format!(
            "{area}: where the later addition's open-world tiles stand in their sky map's frame is not established"
        )),
        "60" => {
            // `_0L` or `_1L` (an alternate tile), of level L 0..2
            let level = match dd.as_bytes() {
                [b'0' | b'1', l @ b'0'..=b'2'] => u32::from(l - b'0'),
                _ => return Ok(None),
            };
            let (Ok(x), Ok(z)) = (bb.parse::<u32>(), cc.parse::<u32>()) else {
                return Ok(None);
            };
            let span = TILE * f64::from(1u32 << level);
            let centre = |i: u32, origin: f64| f64::from(i) * span + span / 2.0 - origin;
            Ok(Some((
                OPEN_WORLD_SKY_MAP.to_owned(),
                [
                    centre(x, OPEN_WORLD_SKY_ORIGIN[0]),
                    centre(z, OPEN_WORLD_SKY_ORIGIN[1]),
                ],
            )))
        }
        _ => Ok(Some((format!("m{aa}_{bb}_00_99"), [0.0; 2]))),
    }
}

/// An Elden Ring area's weathers and the assets of its sky map each shows: the rows of `WeatherAssetReplaceParam` for
/// the sky map that apply on the area (creation limit `limit`, -1 for a map that sets none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Weathers {
    /// The sky map.
    pub sky_map: String,
    /// The area's creation limit, as read.
    pub limit: i8,
    /// The axis values (`weather-<id>`, `weather-<id>-ashen`), in the order (fire-ash state, weather).
    pub values: Vec<String>,
    /// The asset models (`AEG096_010`) each value shows, by value index.
    pub shown: Vec<BTreeSet<String>>,
    /// The asset models some row of the sky map names, for any limit, weather or state.
    pub named: BTreeSet<String>,
}

impl Weathers {
    /// The weathers of the sky map `sky_map` on a map of creation limit `limit`, from the param's `rows`. Errors when a
    /// row that applies holds a fire-ash state other than 0 or 1 (the value 2 of the later addition's rows is not
    /// understood), when two rows that apply give the same weather and state, or when there are more than 64 values
    /// (the axis holds at most 64).
    pub fn new(sky_map: &str, limit: i8, param: &WeatherAssetParam) -> Result<Weathers, String> {
        let mut by_state = BTreeMap::<(u8, i16), (i32, BTreeSet<String>)>::new();
        let mut named = BTreeSet::new();
        for row in param.rows.iter().filter(|r| r.map_name() == sky_map) {
            let assets: BTreeSet<String> = row.assets.iter().map(|&a| asset_model_name(a)).collect();
            named.extend(assets.iter().cloned());
            if !row.applies_at(limit) {
                continue;
            }
            if row.fire_ash > 1 {
                return Err(format!(
                    "{sky_map}: row {} applies with fire-ash state {}, which is not understood",
                    row.id, row.fire_ash
                ));
            }
            if let Some((other, _)) = by_state.insert((row.fire_ash, row.weather), (row.id, assets)) {
                return Err(format!(
                    "{sky_map}: rows {other} and {} both apply for weather {} in fire-ash state {}",
                    row.id, row.weather, row.fire_ash
                ));
            }
        }
        if by_state.len() > 64 {
            return Err(format!(
                "{sky_map}: {} weathers, the state axis holds at most 64",
                by_state.len()
            ));
        }
        let (values, shown) = by_state
            .into_iter()
            .map(|((ash, weather), (_, assets))| {
                let name = if ash == 1 {
                    format!("weather-{weather}-ashen")
                } else {
                    format!("weather-{weather}")
                };
                (name, assets)
            })
            .unzip();
        Ok(Weathers {
            sky_map: sky_map.to_owned(),
            limit,
            values,
            shown,
            named,
        })
    }

    /// The mask of the values that show the asset model `model` (case ignored); 0 for none.
    pub fn states_of(&self, model: &str) -> u64 {
        let model = model.to_ascii_uppercase();
        self.shown
            .iter()
            .enumerate()
            .filter(|(_, s)| s.contains(&model))
            .fold(0, |mask, (i, _)| mask | 1 << i)
    }

    /// Whether some row of the sky map names the asset model `model` (case ignored).
    pub fn names(&self, model: &str) -> bool {
        self.named.contains(&model.to_ascii_uppercase())
    }

    /// The plan of the kept sky placements `items` (by index, model and whether all its materials are of the sky
    /// class): each asset that some value shows, with those values, a [`SkyRole::WeatherSky`] when it draws with sky
    /// programs only, else a [`SkyRole::Weather`]. The report counts the rest; `unread` is what the read left out of
    /// the sky map before ([`Report`]'s `unshown_parts`, `unnamed_parts`, `unnamed_sky_models`).
    pub fn plan(&self, items: &[(usize, String, bool)], unread: Report) -> Plan {
        let mut out = Plan {
            values: self.values.clone(),
            report: Report {
                asked: true,
                states: self.values.clone(),
                sky_map: Some(self.sky_map.clone()),
                limit: Some(self.limit),
                ..unread
            },
            ..Plan::default()
        };
        for (index, model, sky_programs) in items {
            match self.states_of(model) {
                0 => out.report.unshown_parts += 1,
                mask => {
                    let role = if *sky_programs {
                        SkyRole::WeatherSky
                    } else {
                        SkyRole::Weather
                    };
                    out.parts.insert(*index, (role, mask));
                    out.report.weather_assets += 1;
                }
            }
        }
        out
    }
}

/// A sky placement the read kept, for [`plan`].
#[derive(Clone, Debug)]
pub struct Item {
    /// The placement's index in the read's placement list.
    pub index: usize,
    /// The map that holds it: the sky map, or the area for its own domes.
    pub map: String,
    pub model: String,
    pub object: bool,
    /// Its stored entity id (-1 or 0: none).
    pub entity: i32,
    /// Its model's material definition paths, in material order.
    pub mtds: Vec<String>,
}

/// The sky's states and what each kept placement is.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The axis' values, by index.
    pub values: Vec<String>,
    /// By placement index: its role and the mask of the values it is drawn in (0: every state).
    pub parts: BTreeMap<usize, (SkyRole, u64)>,
    pub report: Report,
}

/// What the sky's read found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Whether the read was asked for it.
    pub asked: bool,
    /// The axis' values.
    pub states: Vec<String>,
    pub domes: usize,
    /// Of `domes`, the area's own ones no layout places.
    pub area_domes: usize,
    pub rings: usize,
    pub moons: usize,
    /// Rings no dome pairs with (by entity id, then model number): not placed.
    pub unpaired_rings: Vec<String>,
    /// Parts of the sky map that are no dome, ring or moon (its player start): not placed.
    pub other_parts: Vec<String>,
    /// Elden Ring: the sky map read, if the area has one the archives hold, and the area's creation limit.
    pub sky_map: Option<String>,
    pub limit: Option<i8>,
    /// Elden Ring: the sky map an area of no known sky map, or whose sky map the archives do not hold, would have read
    /// (nothing of the sky is read).
    pub missing_sky_map: Option<String>,
    /// Elden Ring: the placed weather assets, with the values that show them.
    pub weather_assets: usize,
    /// Elden Ring: parts of the sky map that its rows name but that no weather of the area shows (another region's sky,
    /// another fire-ash state's): not placed.
    pub unshown_parts: usize,
    /// Elden Ring: parts of the sky map that no row of it names (its far view of the land, the Erdtree): not placed.
    pub unnamed_parts: usize,
    /// Of `unnamed_parts`, the models of the sky-sphere family (`AEG096_*`): sky the weather does not choose, whose
    /// display is not read.
    pub unnamed_sky_models: Vec<String>,
}

impl Report {
    pub fn notices(&self) -> Vec<primase_core::convert::Notice> {
        use primase_core::convert::{Notice, NoticeKind};
        let stage = "fromsoft reader";
        let mut out = Vec::new();
        if !self.asked {
            return out;
        }
        if let Some(map) = &self.missing_sky_map {
            out.push(
                Notice::new(
                    stage,
                    NoticeKind::Unsupported,
                    "the area's sky map is not in the archives (or the area has none known): no sky read",
                    1,
                )
                .example(map.clone()),
            );
        }
        if !self.states.is_empty() {
            out.push(self.states.iter().fold(
                Notice::new(
                    stage,
                    NoticeKind::Selection,
                    "sky states read as the values of the fromsoft.sky axis, with no default: the game's choice (by \
                     time of day and weather) is not read, so a program or viewer picks one",
                    self.states.len() as u64,
                ),
                |n, s| n.example(s.clone()),
            ));
        }
        if self.area_domes > 0 {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                "the area's own sky domes, which its layout does not place: read at the origin and moved with the \
                 camera as the sky map's are (the game's code moves the sky map's parts; these are not observed drawn)",
                self.area_domes as u64,
            ));
        }
        if self.weather_assets > 0 {
            out.push(Notice::new(
                stage,
                NoticeKind::Approximation,
                "weather assets of the sky map: those drawn with sky programs anchored to the camera's x and z (as a \
                 recorded frame draws a cloud layer), the others placed where the layout places them; where the frame \
                 draws the sky spheres themselves is not yet observed",
                self.weather_assets as u64,
            ));
        }
        for (count, reason) in [
            (
                self.unshown_parts,
                "parts of the sky map its weather rows name but no weather of this area shows (another region's or \
                 fire-ash state's sky), not placed",
            ),
            (
                self.unnamed_parts,
                "parts of the sky map no weather row names (its far view of the land, the Erdtree), not placed",
            ),
        ] {
            if count > 0 {
                out.push(Notice::new(stage, NoticeKind::Unsupported, reason, count as u64));
            }
        }
        for (items, reason) in [
            (
                &self.unpaired_rings,
                "sky rings no dome pairs with (entity id + 1, model + 100), not placed",
            ),
            (
                &self.other_parts,
                "parts of the sky map that are no dome, ring or moon, not placed",
            ),
            (
                &self.unnamed_sky_models,
                "sky-sphere models (AEG096) of the sky map that no weather row names: their display is not read, \
                 not placed",
            ),
        ] {
            if !items.is_empty() {
                out.push(items.iter().fold(
                    Notice::new(stage, NoticeKind::Unsupported, reason, items.len() as u64),
                    |n, i| n.example(i.clone()),
                ));
            }
        }
        out
    }
}

/// Sekiro: the states and roles of the kept sky placements `items`: each dome a value in the order the
/// items come, each ring its dome's value, the moon none. Errors when more than 64 domes are kept (the axis holds at
/// most 64 values) or two get the same name.
pub fn plan(items: &[Item]) -> Result<Plan, String> {
    let mut out = Plan {
        report: Report {
            asked: true,
            ..Report::default()
        },
        ..Plan::default()
    };
    let number = |model: &str| model.strip_prefix('m').and_then(|d| d.parse::<u32>().ok());
    let mut by_entity = BTreeMap::new();
    let mut by_model = BTreeMap::new();
    for item in items {
        match role(&item.model, item.object) {
            Some(SkyRole::Dome) => {
                let word = state_word(item.mtds.iter().map(String::as_str));
                let name = if item.map == SKY_MAP {
                    format!("{}-{word}", item.model)
                } else {
                    format!("{}-{}-{word}", &item.map[..item.map.len().min(6)], item.model)
                };
                if out.values.contains(&name) {
                    return Err(format!("two sky domes are named {name}"));
                }
                if out.values.len() == 64 {
                    return Err("more than 64 sky domes: the state axis holds at most 64".to_owned());
                }
                let value = out.values.len() as u32;
                out.values.push(name);
                if item.entity > 0 {
                    by_entity.insert((item.map.clone(), item.entity), value);
                }
                if let Some(n) = number(&item.model) {
                    by_model.insert((item.map.clone(), n), value);
                }
                out.parts.insert(item.index, (SkyRole::Dome, 1u64 << value));
                out.report.domes += 1;
                out.report.area_domes += usize::from(item.map != SKY_MAP);
            }
            Some(SkyRole::Moon) => {
                out.parts.insert(item.index, (SkyRole::Moon, 0));
                out.report.moons += 1;
            }
            Some(SkyRole::Ring) => {}
            Some(SkyRole::Weather | SkyRole::WeatherSky) | None => {
                out.report.other_parts.push(item.model.clone())
            }
        }
    }
    for item in items
        .iter()
        .filter(|i| role(&i.model, i.object) == Some(SkyRole::Ring))
    {
        let dome = (item.entity > 0)
            .then(|| by_entity.get(&(item.map.clone(), item.entity - 1)))
            .flatten()
            .or_else(|| {
                number(&item.model)
                    .and_then(|n| n.checked_sub(100))
                    .and_then(|n| by_model.get(&(item.map.clone(), n)))
            });
        match dome {
            Some(&value) => {
                out.parts.insert(item.index, (SkyRole::Ring, 1u64 << value));
                out.report.rings += 1;
            }
            None => out.report.unpaired_rings.push(item.model.clone()),
        }
    }
    out.report.states = out.values.clone();
    Ok(out)
}

/// The `fromsoft.sky` axis with `values`, no default (the game's choice is not read), on an entity of its own; `source`
/// says where the values come from.
pub fn spawn_axis(world: &mut World, values: &[String], source: String) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(AXIS.to_owned()));
    world.insert(
        e,
        StateAxis {
            key: AXIS.to_owned(),
            domain: Domain::Enum(values.to_vec()),
            default: None,
            scope: Scope::Global,
            class: "sky".to_owned(),
            source,
        },
    );
    e
}

/// Put on the placement `e` what its plan says: `fromsoft.sky_part`; its condition, the axis at the values of `states`
/// (none for 0, a part drawn in every state); for a Sekiro dome or ring and an Elden Ring weather asset of sky programs
/// `core.sky_anchor` horizontal. The moon's anchor waits for the sun (`lower-fromsoft-sky`); an Elden Ring weather
/// asset of other programs gets none.
pub fn attach(world: &mut World, e: Entity, axis: Option<Entity>, role: SkyRole, states: u64) {
    world.insert(e, SkyPart { role, states });
    if let Some(axis) = axis.filter(|_| states != 0) {
        world.insert(e, Condition(vec![Clause(vec![Literal { axis, values: states }])]));
    }
    if matches!(role, SkyRole::Dome | SkyRole::Ring | SkyRole::WeatherSky) {
        world.insert(e, SkyAnchor::Horizontal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fromsoft_formats::weather_assets::WeatherAssetRow;

    fn item(index: usize, map: &str, model: &str, entity: i32, mtd: &str) -> Item {
        Item {
            index,
            map: map.into(),
            model: model.into(),
            object: model.starts_with('o'),
            entity,
            mtds: vec![
                format!("N:\\NTC\\data\\Material\\mtd\\map\\{mtd}.mtd"),
                "N:\\NTC\\data\\Material\\mtd\\map\\M[Shadow].mtd".into(),
            ],
        }
    }

    /// The sky map's own pairs as its layout stores them (dome and ring entity ids, the two rings paired by model),
    /// the moon, its player start, an area dome and a ring that pairs with nothing.
    #[test]
    fn domes_are_states_and_rings_follow_their_dome() {
        let items = [
            item(0, SKY_MAP, "m900000", 8904000, "M[Sky]_SkyDome_Morning"),
            item(1, SKY_MAP, "m900020", 8904020, "M[Sky]_SkyDome_Evening"),
            item(2, SKY_MAP, "m900075", 8904075, "M[Sky]_SkyDome_DarkClouds"),
            item(3, SKY_MAP, "m900095", 8904100, "M[Sky]_SkyDome_Fire"),
            item(4, SKY_MAP, "m900120", 8904021, "M[LandScape]_FarMtn_Evening"),
            item(5, SKY_MAP, "m900175", 8904176, "M[LandScape]_FarMtn_DarkClouds"),
            item(6, SKY_MAP, "m900200", 8904101, "M[LandScape]_FarMtn_Fire"),
            item(7, SKY_MAP, "o005900", 8901000, "M[Moon]"),
            item(8, SKY_MAP, "c0000", -1, "x"),
            item(9, "m11_01_00_00", "m900000", -1, "M[m11]_SkyDome_Morning"),
            item(10, SKY_MAP, "m900190", 8904091, "M[LandScape]_FarMtn_SeaOfCloud"),
        ];
        let plan = plan(&items).unwrap();
        assert_eq!(
            plan.values,
            [
                "m900000-Morning",
                "m900020-Evening",
                "m900075-DarkClouds",
                "m900095-Fire",
                "m11_01-m900000-Morning"
            ]
        );
        assert_eq!(plan.parts[&1], (SkyRole::Dome, 0b10));
        assert_eq!(plan.parts[&4], (SkyRole::Ring, 0b10));
        assert_eq!(plan.parts[&5], (SkyRole::Ring, 0b100), "by model: 900175 - 100");
        assert_eq!(plan.parts[&6], (SkyRole::Ring, 0b1000), "by entity: 8904101 - 1");
        assert_eq!(plan.parts[&7], (SkyRole::Moon, 0));
        assert!(!plan.parts.contains_key(&8) && !plan.parts.contains_key(&10));
        assert_eq!(plan.report.other_parts, ["c0000"]);
        assert_eq!(plan.report.unpaired_rings, ["m900190"]);
        assert_eq!(
            (
                plan.report.domes,
                plan.report.area_domes,
                plan.report.rings,
                plan.report.moons
            ),
            (5, 1, 3, 1)
        );
    }

    #[test]
    fn roles_and_state_words_come_from_the_models_and_materials() {
        assert_eq!(role("m900095", false), Some(SkyRole::Dome));
        assert_eq!(role("m900200", false), Some(SkyRole::Ring));
        assert_eq!(role("m900300", false), None);
        assert_eq!(role("o005900", true), Some(SkyRole::Moon));
        assert_eq!(role("o005901", true), None);
        assert_eq!(
            state_word(["M[Shadow].mtd", "N:\\a\\M[m10_00]_SkyDome_Night1.mtd"]),
            "Night1"
        );
        assert_eq!(state_word(["M[Moon].mtd"]), "Dome");
    }

    /// A dome and a ring on the axis at their value, anchored horizontally; the moon without condition or anchor; a
    /// weather asset on the axis at every value that shows it, anchored horizontally when it draws with sky programs
    /// only and without anchor otherwise.
    #[test]
    fn attached_parts_carry_their_state_and_anchor() {
        let mut w = World::new();
        let axis = spawn_axis(
            &mut w,
            &["m900000-Morning".into(), "m900020-Evening".into()],
            "test".into(),
        );
        let (dome, moon, weather, cloud) = (w.spawn(), w.spawn(), w.spawn(), w.spawn());
        attach(&mut w, dome, Some(axis), SkyRole::Dome, 0b10);
        attach(&mut w, moon, Some(axis), SkyRole::Moon, 0);
        attach(&mut w, weather, Some(axis), SkyRole::Weather, 0b11);
        attach(&mut w, cloud, Some(axis), SkyRole::WeatherSky, 0b01);
        assert_eq!(w.get::<SkyAnchor>(cloud), Some(&SkyAnchor::Horizontal));
        assert_eq!(
            w.get::<Condition>(dome),
            Some(&Condition(vec![Clause(vec![Literal { axis, values: 0b10 }])]))
        );
        assert_eq!(w.get::<SkyAnchor>(dome), Some(&SkyAnchor::Horizontal));
        assert!(w.get::<Condition>(moon).is_none() && w.get::<SkyAnchor>(moon).is_none());
        assert_eq!(
            w.get::<Condition>(weather),
            Some(&Condition(vec![Clause(vec![Literal { axis, values: 0b11 }])]))
        );
        assert!(w.get::<SkyAnchor>(weather).is_none());
        assert_eq!(
            w.get::<SkyPart>(weather),
            Some(&SkyPart {
                role: SkyRole::Weather,
                states: 0b11
            })
        );
        let a = w.get::<StateAxis>(axis).unwrap();
        assert_eq!((a.default, a.class.as_str()), (None, "sky"));
    }

    /// Open-world tiles of each level stand in `m60_00_00_99`'s frame by their centre (the level-2 tile `m60_10_08_02`
    /// at its origin, as its far-view piece `m901008` is); a legacy map in its own `_99` map's frame; the later
    /// addition's tiles are refused.
    #[test]
    fn an_areas_sky_map_and_its_frame() {
        let open = |a: &str| elden_sky_map(a).unwrap().unwrap();
        assert_eq!(open("m60_10_08_02"), (OPEN_WORLD_SKY_MAP.into(), [0.0, 0.0]));
        assert_eq!(open("m60_42_36_00"), (OPEN_WORLD_SKY_MAP.into(), [128.0, 640.0]));
        assert_eq!(open("m60_21_17_01"), (OPEN_WORLD_SKY_MAP.into(), [256.0, 256.0]));
        assert_eq!(open("m60_42_36_10").1, [128.0, 640.0]);
        assert_eq!(open("m10_00_00_00"), ("m10_00_00_99".into(), [0.0, 0.0]));
        assert_eq!(open("m11_05_00_00").0, "m11_05_00_99");
        assert_eq!(elden_sky_map("m60_42_36_99").unwrap(), None);
        assert!(elden_sky_map("m61_44_44_00").is_err());
        assert_eq!(map_number("m60_42_36_00"), Some(60423600));
        assert_eq!(map_number("m10_00_00"), None);
    }

    fn row(id: i32, weather: i16, fire_ash: u8, assets: &[i32], limits: &[i8]) -> WeatherAssetRow {
        WeatherAssetRow {
            id,
            map: 60000099,
            weather,
            fire_ash,
            assets: assets.to_vec(),
            limits: limits.to_vec(),
        }
    }

    /// The rows of the sky map that apply at the area's limit give the values, in (state, weather) order, and the
    /// assets each shows; an asset is placed with the values that show it, one another region's rows name is not.
    #[test]
    fn weathers_are_states_and_assets_carry_the_weathers_that_show_them() {
        let param = WeatherAssetParam {
            rows: vec![
                row(1, 0, 0, &[96010, 96400], &[0]),
                row(2, 20, 0, &[96404], &[0]),
                row(3, 0, 1, &[96010, 94104], &[0]),
                row(4, 0, 0, &[96864], &[1]),
                WeatherAssetRow {
                    map: 10000099,
                    ..row(5, 0, 0, &[96603], &[])
                },
            ],
        };
        let w = Weathers::new(OPEN_WORLD_SKY_MAP, 0, &param).unwrap();
        assert_eq!(w.values, ["weather-0", "weather-20", "weather-0-ashen"]);
        assert_eq!(w.states_of("aeg096_010"), 0b101);
        assert_eq!(w.states_of("AEG094_104"), 0b100);
        assert_eq!(w.states_of("AEG096_864"), 0, "limit 1's");
        assert!(w.names("AEG096_864") && !w.names("AEG096_603") && !w.names("AEG004_114"));
        let items = [
            (0, "AEG096_010".to_owned(), true),
            (1, "AEG096_404".to_owned(), false),
            (2, "AEG096_864".to_owned(), true),
        ];
        let plan = w.plan(
            &items,
            Report {
                unnamed_parts: 7,
                ..Report::default()
            },
        );
        assert_eq!(plan.parts[&0], (SkyRole::WeatherSky, 0b101));
        assert_eq!(plan.parts[&1], (SkyRole::Weather, 0b10));
        assert!(!plan.parts.contains_key(&2));
        assert_eq!(
            (
                plan.report.weather_assets,
                plan.report.unshown_parts,
                plan.report.unnamed_parts
            ),
            (2, 1, 7)
        );
        assert_eq!(plan.report.sky_map.as_deref(), Some(OPEN_WORLD_SKY_MAP));
        // a legacy map's rows set no limit: they apply where the map sets none
        let legacy = Weathers::new("m10_00_00_99", -1, &param).unwrap();
        assert_eq!(legacy.values, ["weather-0"]);
    }

    /// Two rows for one weather and state, and a fire-ash state of 2, are refused where they apply.
    #[test]
    fn weathers_refuse_what_they_do_not_understand() {
        let twice = WeatherAssetParam {
            rows: vec![row(1, 0, 0, &[96010], &[]), row(2, 0, 0, &[96012], &[0])],
        };
        let e = Weathers::new(OPEN_WORLD_SKY_MAP, 0, &twice).unwrap_err();
        assert!(e.contains("rows 1 and 2"), "{e}");
        assert!(Weathers::new(OPEN_WORLD_SKY_MAP, 1, &twice).is_ok());
        let two = WeatherAssetParam {
            rows: vec![row(1, 0, 2, &[96010], &[])],
        };
        assert!(Weathers::new(OPEN_WORLD_SKY_MAP, -1, &two).is_err());
    }
}
