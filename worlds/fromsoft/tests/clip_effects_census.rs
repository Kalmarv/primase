//! The effects the animation events of the installs start (`clip_effect_uses`), against an independent walk of the same
//! events: every authored event of every `.tae` member of every animation binder the dictionaries name and the archives hold
//! (336,787 events in Sekiro, 1,310,023 in Elden Ring), counted by a stdlib script from the payload words alone (the first
//! word above 0 for the effect number, the s16 at byte 4 of a blade event for its dummy polygon source). The uses by event
//! type and the events counted as lost by reason have to be the script's, so the community template's field names, the
//! decoding of their values and the rules over them agree with the stored bytes on every event.
//!
//! Needs `installs.{sekiro,elden}`, `keys.{sekiro,elden}`, `tools.oodle6` and `oracles.uxm.{sekiro,elden}/dictionary.txt`;
//! a game that is not configured is skipped.
mod common;
mod inputs;

use fromsoft_formats::tae::{Header, Tae};
use fromsoft_formats::{Binder, Game, Keys, NameDictionary, dcx::Decoder};
use inputs::{Game as Data, OracleSet, Tool, install, keys, oodle, oracle};
use primase_fromsoft::sekiro::character_data::ClipEvents;
use primase_fromsoft::sekiro::effect_data::clip_effect_uses;
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// What the script counts, by game: uses by event type, and the lost events by reason (goods and magic, throw and foot, types
/// the files do not bear out, a weapon's dummy polygon, an additional spawn condition, a special-effect state). The two
/// conditions were counted after the script: the 60 + 2,016 (Sekiro) and 154 + 1,622 (Elden Ring) events with one are all of
/// type 96, and the script's 7,586 and 20,078 uses of type 96 are exactly the uses here and those events together.
const SEKIRO: ([(i64, u32); 3], [u32; 6]) = (
    [(96, 5_510), (118, 174), (120, 4)],
    [103, 8_755, 2_949, 8, 60, 2_016],
);
const ELDEN: ([(i64, u32); 3], [u32; 6]) = (
    [(96, 18_302), (118, 2_341), (120, 1_473)],
    [1_209, 31_760, 1_754, 382, 154, 1_622],
);

#[test]
fn the_effects_events_start_are_what_their_stored_words_say() -> Result<()> {
    let Some(library) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let codec = common::oodle(library)?;
    let decoder = Decoder {
        oodle: Some(&codec),
        max_output: 1 << 30,
    };
    for (game, data, (uses_expected, lost_expected)) in [
        (Game::Sekiro, Data::Sekiro, SEKIRO),
        (Game::EldenRing, Data::Elden, ELDEN),
    ] {
        let (Some(root), Some(folder), Some(names)) = (
            install(data),
            keys(data),
            oracle(OracleSet::Uxm, data, "dictionary.txt"),
        ) else {
            continue;
        };
        let archive = common::open_game(root, game, &Keys::from_folder(folder, game, None)?)?;
        let dictionary = NameDictionary::read_for(names, game)?;
        let paths: BTreeSet<&str> = dictionary
            .names
            .values()
            .flatten()
            .map(String::as_str)
            .filter(|p| p.contains(".anibnd") && archive.contains(p))
            .collect();
        let mut uses: BTreeMap<i64, u32> = BTreeMap::new();
        let mut lost = [0u32; 6];
        let mut events = 0usize;
        for path in paths {
            let binder = Binder::read(&archive.read(path, &decoder)?, &decoder)?;
            for member in &binder.files {
                if !member
                    .name
                    .as_deref()
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(".tae"))
                {
                    continue;
                }
                for animation in &Tae::read_member(&binder, member)?.animations {
                    events += animation.events.len();
                    let typed = primase_fromsoft::sekiro::decode_clip_events(game, &animation.events);
                    let source = ClipEvents {
                        file: String::new(),
                        animation: 0,
                        header: Header::None,
                        file_name: None,
                        authored_events: animation.events.clone(),
                        events: animation.events.clone(),
                        typed,
                    };
                    let found = clip_effect_uses(&source);
                    for u in &found.uses {
                        *uses.entry(u.kind).or_default() += 1;
                        assert!(u.effect > 0, "{game:?}: {u:?}");
                    }
                    for (reason, n) in &found.lost {
                        let slot = if reason.contains("goods or magic") {
                            0
                        } else if reason.contains("throw direction or foot") {
                            1
                        } else if reason.contains("do not show to hold effect numbers") {
                            2
                        } else if reason.contains("weapon's dummy polygon") {
                            3
                        } else if reason == primase_fromsoft::sekiro::effect_data::SPAWN_CONDITION {
                            4
                        } else if reason == primase_fromsoft::sekiro::effect_data::STATE_CONDITION {
                            5
                        } else {
                            panic!("{game:?}: lost for a reason the script does not count: {reason}");
                        };
                        lost[slot] += n;
                    }
                }
            }
        }
        println!("FS_CLIP_EFFECT_USES={game:?} {uses:?} {lost:?}");
        let expected: BTreeMap<i64, u32> = uses_expected.into_iter().collect();
        assert_eq!(uses, expected, "{game:?}: uses by event type");
        assert_eq!(lost, lost_expected, "{game:?}: lost by reason");
        assert_eq!(
            events,
            if game == Game::Sekiro { 336_787 } else { 1_310_023 },
            "{game:?}: events"
        );
    }
    Ok(())
}
