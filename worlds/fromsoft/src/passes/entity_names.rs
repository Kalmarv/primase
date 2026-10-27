//! Credited human labels for source ids. Smithbox lists: cbd477a8, MIT (the complete
//! original licence is assets/names/SMITHBOX-LICENSE). Source ids/names remain unchanged.
//! An MSB enemy's explicit NpcParam row may name game FMG text; a model number alone
//! never chooses one NPC row or invents a connection to an FMG entry.
use super::clip_events::{game_option, game_text};
use crate::components::stored::UnplacedPart;
use fromsoft_formats::Game;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Clip, Component, DisplayName, Name, NameOrigin, World};
use std::collections::BTreeMap;
use std::sync::OnceLock;
pub const CREDIT: &str = "Smithbox cbd477a8; Vawser, Katalash, Meowmaritus and contributors (MIT)";

#[derive(serde::Deserialize)]
struct Row {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
}
fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let rows: Vec<Row> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| (r.id.to_ascii_lowercase(), r.name.trim().to_owned()))
        .collect())
}
struct Names {
    characters: BTreeMap<String, String>,
    maps: BTreeMap<String, String>,
    parts: BTreeMap<String, String>,
    animations: BTreeMap<String, String>,
    time_acts: BTreeMap<String, String>,
}
fn names(game: Game) -> Result<&'static Names, String> {
    static SDT: OnceLock<Result<Names, String>> = OnceLock::new();
    static ER: OnceLock<Result<Names, String>> = OnceLock::new();
    macro_rules! load {
        ($folder:literal) => {
            Ok(Names {
                characters: parse(include_str!(concat!(
                    "../../assets/names/",
                    $folder,
                    "/Characters.json"
                )))?,
                maps: parse(include_str!(concat!(
                    "../../assets/names/",
                    $folder,
                    "/MapNames.json"
                )))?,
                parts: parse(include_str!(concat!(
                    "../../assets/names/",
                    $folder,
                    "/Parts.json"
                )))?,
                animations: parse(include_str!(concat!(
                    "../../assets/names/",
                    $folder,
                    "/Animations.json"
                )))?,
                time_acts: parse(include_str!(concat!(
                    "../../assets/names/",
                    $folder,
                    "/TimeActs.json"
                )))?,
            })
        };
    }
    let result = match game {
        Game::Sekiro => SDT.get_or_init(|| load!("sdt")),
        Game::EldenRing => ER.get_or_init(|| load!("er")),
    };
    result.as_ref().map_err(Clone::clone)
}

/// The community character name for this exact model id, case-insensitive.
pub fn character(game: Game, id: &str) -> Option<&'static str> {
    names(game)
        .ok()?
        .characters
        .get(&id.to_ascii_lowercase())
        .map(String::as_str)
}
/// A map id or its shortened area id padded with source map-number zeroes.
pub fn map(game: Game, id: &str) -> Option<&'static str> {
    let id = id.to_ascii_lowercase();
    let list = &names(game).ok()?.maps;
    list.get(&id)
        .or_else(|| match id.matches('_').count() {
            1 => list.get(&format!("{id}_00_00")),
            2 => list.get(&format!("{id}_00")),
            _ => None,
        })
        .map(String::as_str)
}
pub fn part(game: Game, id: &str) -> Option<&'static str> {
    let id = id.to_ascii_lowercase();
    let list = &names(game).ok()?.parts;
    list.get(&id)
        .or_else(|| {
            let (stem, suffix) = id.rsplit_once('_')?;
            (suffix.len() == 1 && suffix.chars().all(|c| c.is_ascii_alphabetic()))
                .then(|| list.get(stem))
                .flatten()
        })
        .map(String::as_str)
}
pub fn animation(game: Game, id: &str) -> Option<&'static str> {
    names(game)
        .ok()?
        .animations
        .get(&id.to_ascii_lowercase())
        .map(String::as_str)
}
pub fn time_act(game: Game, model: &str, clip: &str) -> Option<&'static str> {
    let group = clip
        .strip_prefix('a')?
        .split('_')
        .next()
        .filter(|g| g.len() == 3 && g.bytes().all(|c| c.is_ascii_digit()))?;
    names(game)
        .ok()?
        .time_acts
        .get(&format!("{}_{group}", model.to_ascii_lowercase()))
        .map(String::as_str)
}
pub fn describe(game: Game, id: &str) -> Option<&'static str> {
    character(game, id)
        .or_else(|| map(game, id))
        .or_else(|| part(game, id))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameFromSoftEntities {
    pub game: Game,
}
pub const NAME_FROMSOFT_ENTITIES: PassSpec = PassSpec {
    name: "name-fromsoft-entities",
    options: &[OptionSpec::required(
        "game",
        "sekiro",
        "game whose own text and credited community ids are used",
    )],
    make: |g: &Given| {
        Ok(Box::new(NameFromSoftEntities {
            game: game_option(g)?,
        }))
    },
};
impl NameFromSoftEntities {
    fn lower(
        &self,
        world: &mut World,
        text: &BTreeMap<i32, String>,
        log: &mut PassLog,
    ) -> Result<(), String> {
        names(self.game)?; // a corrupt bundled list is a named error, never an empty lookup table
        let entities: Vec<_> = world.entities().collect();
        for entity in entities {
            if world
                .get::<DisplayName>(entity)
                .is_some_and(|name| name.origin == NameOrigin::Game)
            {
                // A later community-only naming step must not downgrade an authored name.
                continue;
            }
            let placement = world.get::<UnplacedPart>(entity);
            let game = placement.and_then(|p| p.npc_param).and_then(|row| text.get(&row));
            let community = placement
                .and_then(|p| p.model.as_deref())
                .and_then(|id| describe(self.game, id))
                .or_else(|| {
                    world.get::<Name>(entity).and_then(|n| {
                        let id =
                            n.0.strip_suffix(" havok skeleton")
                                .or_else(|| n.0.strip_suffix(" skeleton"))
                                .unwrap_or(&n.0);
                        describe(self.game, id)
                    })
                })
                .or_else(|| {
                    world
                        .get::<crate::clip_source::ClipSource>(entity)
                        .and_then(|source| {
                            animation(self.game, &source.name)
                                .or_else(|| time_act(self.game, &source.character, &source.name))
                        })
                })
                .or_else(|| {
                    world
                        .get::<Clip>(entity)
                        .and_then(|clip| animation(self.game, &clip.name))
                });
            let label = match (game, community) {
                (Some(name), _) => Some(DisplayName {
                    name: name.clone(),
                    origin: NameOrigin::Game,
                }),
                (None, Some(name)) => Some(DisplayName {
                    name: name.into(),
                    origin: NameOrigin::Community {
                        credit: CREDIT.into(),
                    },
                }),
                (None, None) => None,
            };
            if let Some(label) = label {
                world.insert(entity, label);
                log.count("entities given human display names", 1);
            }
        }
        Ok(())
    }
}
impl Pass for NameFromSoftEntities {
    fn name(&self) -> &'static str {
        "name-fromsoft-entities"
    }
    fn options(&self) -> Options {
        Options::from([("game".into(), game_text(self.game).into())])
    }
    fn provides(&self) -> &[&'static str] {
        &[DisplayName::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let text = match world.resource::<crate::sekiro::character_data::NpcNames>() {
            Some(rows) if rows.game != self.game => {
                return Err("NPC-name resource belongs to a different game".into());
            }
            Some(rows) => rows.names.clone(),
            None => {
                let count = world
                    .iter::<UnplacedPart>()
                    .filter(|(_, part)| part.npc_param.is_some())
                    .count();
                if count > 0 {
                    log.omit(
                        "NPC game names were not loaded; only credited community labels are available",
                        count as u64,
                    );
                }
                BTreeMap::new()
            }
        };
        self.lower(world, &text, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::stored::Stored;
    use fromsoft_formats::msb;
    #[test]
    fn bundled_labels_keep_source_ids_and_credited_origins() {
        assert_eq!(character(Game::Sekiro, "C1010"), Some("Ashina Soldier"));
        assert_eq!(map(Game::Sekiro, "m11_01"), Some("Ashina Castle"));
        assert_eq!(
            part(Game::EldenRing, "am_f_1540_l"),
            Some("Chain Gauntlets (Female)")
        );
        assert_eq!(
            time_act(Game::EldenRing, "c0000", "a002_003000"),
            Some("Idle: One Hand on Shoulder")
        );
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Name("c1010".into()));
        NameFromSoftEntities { game: Game::Sekiro }
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        assert_eq!(world.get::<Name>(e).unwrap().0, "c1010");
        assert!(matches!(
            world.get::<DisplayName>(e).unwrap().origin,
            NameOrigin::Community { .. }
        ));
        assert_eq!(world.get::<DisplayName>(e).unwrap().name, "Ashina Soldier");
    }
    #[test]
    fn an_authored_npc_row_prefers_its_game_name_and_does_not_rename_the_source() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Name("enemy_0001".into()));
        world.insert(
            e,
            Stored(msb::UnplacedPart {
                index: 0,
                name: "enemy_0001".into(),
                kind: "enemy",
                model: Some("c1010".into()),
                npc_param: Some(42),
                position: [0.0; 3],
                rotation_degrees: [0.0; 3],
                scale: [1.0; 3],
                entity_id: 0,
                entity_group_ids: [0; 8],
            }),
        );
        let pass = NameFromSoftEntities { game: Game::Sekiro };
        world.insert_resource(crate::sekiro::character_data::NpcNames {
            game: Game::Sekiro,
            language: "engus".into(),
            report: vec![],
            names: BTreeMap::from([(42, "Authored soldier".into())]),
        });
        pass.run(&mut world, &mut PassLog::default()).unwrap();
        assert_eq!(
            world.get::<DisplayName>(e),
            Some(&DisplayName {
                name: "Authored soldier".into(),
                origin: NameOrigin::Game
            })
        );
        assert_eq!(world.get::<Name>(e).unwrap().0, "enemy_0001");
    }
}
