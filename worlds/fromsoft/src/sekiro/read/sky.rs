//! The sky of a Sekiro area, read with it when asked ([`Sky::Include`]): the sky map `m89_00_00_99`, whose layout
//! places at the origin one dome per sky state (`m9000xx`), each dome's ring of far mountains (`m9001xx`, `m900200`)
//! and the moon (the object `o005900`), and the area's own domes that its layout does not place (`m10_00`, `m11_01` and
//! `m17_00` keep some).
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
use crate::sekiro::source::{SkyPart, SkyRole};
use fromsoft_formats::msb::{MapLayout, Part, PartKind};
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Entity, Name, SkyAnchor, World};
use std::collections::BTreeMap;

/// The sky map.
pub const SKY_MAP: &str = "m89_00_00_99";
/// The moon's object model.
pub const MOON: &str = "o005900";
/// The state axis of the sky.
pub const AXIS: &str = "fromsoft.sky";
/// The moon's distance from the camera, as the game's in-game step stores it.
pub const MOON_DISTANCE: f32 = 25.0;

/// Whether an area's read brings in the sky.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sky {
    /// Not read (today's output for every target).
    #[default]
    None,
    /// The sky map and the area's own unplaced domes, as described in the module documentation.
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
    let placed: std::collections::BTreeSet<&str> = layout
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
    /// By placement index: its role and, for a dome or ring, its state.
    pub parts: BTreeMap<usize, (SkyRole, Option<u32>)>,
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
}

impl Report {
    pub fn notices(&self) -> Vec<primase_core::convert::Notice> {
        use primase_core::convert::{Notice, NoticeKind};
        let stage = "fromsoft reader";
        let mut out = Vec::new();
        if !self.asked {
            return out;
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
        for (items, reason) in [
            (
                &self.unpaired_rings,
                "sky rings no dome pairs with (entity id + 1, model + 100), not placed",
            ),
            (
                &self.other_parts,
                "parts of the sky map that are no dome, ring or moon, not placed",
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

/// The states and roles of the kept sky placements `items`: each dome a value in the order the
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
                let value = out.values.len() as u32;
                out.values.push(name);
                if item.entity > 0 {
                    by_entity.insert((item.map.clone(), item.entity), value);
                }
                if let Some(n) = number(&item.model) {
                    by_model.insert((item.map.clone(), n), value);
                }
                out.parts.insert(item.index, (SkyRole::Dome, Some(value)));
                out.report.domes += 1;
                out.report.area_domes += usize::from(item.map != SKY_MAP);
            }
            Some(SkyRole::Moon) => {
                out.parts.insert(item.index, (SkyRole::Moon, None));
                out.report.moons += 1;
            }
            Some(SkyRole::Ring) => {}
            None => out.report.other_parts.push(item.model.clone()),
        }
    }
    if out.values.len() > 64 {
        return Err(format!(
            "{} sky domes: the state axis holds at most 64",
            out.values.len()
        ));
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
                out.parts.insert(item.index, (SkyRole::Ring, Some(value)));
                out.report.rings += 1;
            }
            None => out.report.unpaired_rings.push(item.model.clone()),
        }
    }
    out.report.states = out.values.clone();
    Ok(out)
}

/// The `fromsoft.sky` axis with `values`, no default (the game's choice is not read), on an entity of its own.
pub fn spawn_axis(world: &mut World, values: &[String]) -> Entity {
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
            source: format!(
                "the domes of the sky map {SKY_MAP} and the area's own; the game's choice of one is not read"
            ),
        },
    );
    e
}

/// Put on the placement `e` what its plan says: `fromsoft.sky_part`, for a dome or ring its condition (the axis at its
/// value) and `core.sky_anchor` horizontal. The moon's anchor waits for the sun (`lower-fromsoft-sky`).
pub fn attach(world: &mut World, e: Entity, axis: Option<Entity>, role: SkyRole, state: Option<u32>) {
    world.insert(e, SkyPart { role, state });
    if let (Some(axis), Some(value)) = (axis, state) {
        world.insert(
            e,
            Condition(vec![Clause(vec![Literal {
                axis,
                values: 1 << value,
            }])]),
        );
    }
    if role != SkyRole::Moon {
        world.insert(e, SkyAnchor::Horizontal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(plan.parts[&4], (SkyRole::Ring, Some(1)));
        assert_eq!(plan.parts[&5], (SkyRole::Ring, Some(2)), "by model: 900175 - 100");
        assert_eq!(plan.parts[&6], (SkyRole::Ring, Some(3)), "by entity: 8904101 - 1");
        assert_eq!(plan.parts[&7], (SkyRole::Moon, None));
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

    /// A dome and a ring on the axis at their value, anchored horizontally; the moon without condition or anchor.
    #[test]
    fn attached_parts_carry_their_state_and_anchor() {
        let mut w = World::new();
        let axis = spawn_axis(&mut w, &["m900000-Morning".into(), "m900020-Evening".into()]);
        let (dome, moon) = (w.spawn(), w.spawn());
        attach(&mut w, dome, Some(axis), SkyRole::Dome, Some(1));
        attach(&mut w, moon, Some(axis), SkyRole::Moon, None);
        assert_eq!(
            w.get::<Condition>(dome),
            Some(&Condition(vec![Clause(vec![Literal { axis, values: 0b10 }])]))
        );
        assert_eq!(w.get::<SkyAnchor>(dome), Some(&SkyAnchor::Horizontal));
        assert!(w.get::<Condition>(moon).is_none() && w.get::<SkyAnchor>(moon).is_none());
        let a = w.get::<StateAxis>(axis).unwrap();
        assert_eq!((a.default, a.class.as_str()), (None, "sky"));
    }
}
