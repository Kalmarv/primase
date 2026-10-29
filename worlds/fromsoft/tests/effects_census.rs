//! Every effect file of both installs lowered into `core.particles` (the lowering of `lower-fromsoft-effects`, with a resolver
//! that finds every file): none is refused, every system passes core's own check, and the draws made plus the appearance
//! actions left in configs no state chooses are exactly the appearance actions the files hold, by action type, against an
//! independent walker of the on-disk layout (a stdlib Python reader, `census_actions`, counting the distinct effect
//! ids' actions: 2,669 Sekiro and 11,912 Elden Ring effects, copies being byte-identical).
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` and `oracles.uxm.{sekiro,elden}/dictionary.txt`; a
//! game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::fxr::{self, Fxr};
use fromsoft_formats::{Binder, Game, Keys, NameDictionary, dcx::Decoder};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_core::{Component, Entity, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::effects::{Resolver, lower};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Appearance actions of each type over the distinct effects: Sekiro's 2,669 and Elden Ring's 11,912.
const SEKIRO_APPEARANCES: &[(i16, u32)] = &[
    (600, 563),
    (601, 29),
    (602, 92),
    (603, 6888),
    (604, 1383),
    (605, 881),
    (606, 238),
    (607, 148),
    (608, 424),
    (609, 744),
    (10000, 44),
    (10012, 1980),
    (10013, 51),
    (10300, 75),
    (10301, 1),
    (11000, 13),
];
const ELDEN_APPEARANCES: &[(i16, u32)] = &[
    (600, 3365),
    (601, 6),
    (602, 2409),
    (603, 17252),
    (604, 13737),
    (605, 1482),
    (606, 122),
    (607, 1348),
    (608, 1328),
    (609, 4957),
    (10000, 97),
    (10001, 5),
    (10012, 6397),
    (10013, 171),
    (10014, 91),
    (10015, 3047),
    (10300, 104),
    (11000, 17),
];

/// A resolver that finds every file: the census checks the structure, not the textures.
struct Everything;
impl Resolver for Everything {
    fn texture(&mut self, _: fxr::resources::TextureKind, _: i32) -> Option<Entity> {
        Some(Entity::from_index(1))
    }
    fn model(&mut self, _: i32) -> Option<Entity> {
        Some(Entity::from_index(2))
    }
    fn animation(&mut self, _: i32) -> Option<Entity> {
        Some(Entity::from_index(3))
    }
    fn effect(&mut self, _: i32) -> Option<Entity> {
        Some(Entity::from_index(4))
    }
}

#[test]
fn every_installed_effect_lowers_and_its_draws_add_up() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data, expected, effects) in [
        (Game::Sekiro, Data::Sekiro, SEKIRO_APPEARANCES, 2_669usize),
        (Game::EldenRing, Data::Elden, ELDEN_APPEARANCES, 11_912),
    ] {
        let (Some(root), Some(names), Some(folder)) =
            (install(data), oracle(OracleSet::Uxm, data, "dictionary.txt"), keys(data))
        else {
            continue;
        };
        let key = Keys::from_folder(folder, game, game.param_key_sha256())?;
        let install = common::install(root, game, key);
        let archive = install.open()?;
        let dictionary = NameDictionary::read_for(names, game)?;
        let paths: BTreeSet<&str> = dictionary
            .names
            .values()
            .flatten()
            .map(String::as_str)
            .filter(|p| p.starts_with("/sfx/") && p.contains(".ffxbnd"))
            .filter(|p| archive.contains(p))
            .collect();
        let mut files: BTreeMap<i32, Fxr> = BTreeMap::new();
        for path in paths {
            let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
            for member in &binder.files {
                let Some(id) = member.name.as_deref().and_then(fxr::member_id) else {
                    continue;
                };
                files.entry(id).or_insert(Fxr::read(&member.bytes)?);
            }
        }
        assert_eq!(files.len(), effects, "{game:?}: distinct effects");
        let mut world = World::default();
        let probe = world.spawn();
        let (mut drawn, mut unchosen): (BTreeMap<i16, u32>, BTreeMap<i16, u32>) = Default::default();
        let (mut emitters, mut lost, mut approximated) = (0usize, BTreeMap::<String, u32>::new(), BTreeMap::<String, u32>::new());
        for (id, file) in &files {
            let lowered = lower(file, game, 0, &mut Everything).map_err(|e| format!("{game:?} effect {id}: {e}"))?;
            lowered
                .system
                .verify(probe, &world)
                .map_err(|e| format!("{game:?} effect {id}: {e}"))?;
            emitters += lowered.system.emitters.len();
            for (what, n) in &lowered.notes.placed {
                if let Some(action) = what.strip_prefix("draw of action ") {
                    *drawn.entry(action.parse()?).or_default() += n;
                }
            }
            for (reason, n) in &lowered.notes.lost {
                if let Some(rest) = reason.strip_prefix("appearance action ") {
                    let action: i16 = rest.split(' ').next().unwrap_or("0").parse()?;
                    *unchosen.entry(action).or_default() += n;
                }
                *lost.entry(reason.clone()).or_default() += n;
            }
            for (reason, n) in &lowered.notes.approximated {
                *approximated.entry(reason.clone()).or_default() += n;
            }
        }
        // placed + left in an unchosen config = what the files hold, by action type
        for &(action, held) in expected {
            let made = drawn.get(&action).copied().unwrap_or(0);
            let left = unchosen.get(&action).copied().unwrap_or(0);
            assert_eq!(made + left, held, "{game:?}: appearance action {action}: {made} drawn + {left} unchosen of {held}");
        }
        let types: BTreeSet<i16> = expected.iter().map(|(t, _)| *t).collect();
        assert!(drawn.keys().chain(unchosen.keys()).all(|t| types.contains(t)), "{game:?}: {drawn:?} {unchosen:?}");
        assert!(emitters > files.len(), "{game:?}: {emitters} emitters");
        println!(
            "FS_EFFECTS_LOWERED={}",
            json!({"game": game.name(), "effects": files.len(), "emitters": emitters, "drawn": drawn,
                   "unchosen": unchosen, "lost": lost, "approximated": approximated})
        );
    }
    Ok(())
}
